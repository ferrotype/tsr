//! Independent admission/reply pump, ordered preparation, and bounded checker
//! concurrency. A synchronous host callback never holds up the reply reader.
use crate::{
    dynamic_queue::DynamicQueue,
    rpc_client::RpcClient,
    runtime::{Dispatch, Options, Runtime, Work},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tsr_ipc::Context;
use tsr_json::RawValue;
use tsr_lsproto::{Id, Message, ResponseError};

struct Admission {
    requests: DynamicQueue<Message>,
    // Reserve IDs at admission (ADR 0019), but only running requests are
    // cancellable. Go registers their contexts when dispatch dequeues them.
    contexts: Mutex<HashMap<Id, Option<Context>>>,
    context: Context,
    client: Arc<RpcClient>,
}
#[derive(Clone)]
pub struct Input(Arc<Admission>);
impl Input {
    /// Framing errors end the connection; malformed JSON messages fail only
    /// their request. Keep reading so the next well-formed request can run.
    pub fn receive_bytes(&self, bytes: &[u8]) -> Result<(), ResponseError> {
        let mut message = Message::default();
        if let Err(error) = tsr_json::unmarshal(bytes, &mut message, tsr_json::Options::default()) {
            return self.0.client.write(&Message {
                error: Some(crate::coded_error(
                    tsr_lsproto::ErrorCode::INVALID_REQUEST,
                    Some(&error.to_string()),
                )),
                ..Default::default()
            });
        }
        let id = message.id.clone();
        if let Err(error) = self.receive(message) {
            return self.0.client.write(&Message {
                id,
                error: Some(error),
                ..Default::default()
            });
        }
        Ok(())
    }
    pub fn receive(&self, message: Message) -> Result<(), ResponseError> {
        if message.is_response() {
            self.0.client.receive(&message);
            return Ok(());
        }
        if message.method == "$/cancelRequest" {
            let params: tsr_lsproto::CancelParams = crate::decode(message.params.as_ref())?;
            let id = params.id.string.as_deref().map_or_else(
                || Id::int(params.id.integer.as_deref().copied().unwrap_or_default()),
                |s| Id::string(s.as_str()),
            );
            // cancel may run callbacks: never hold the contexts map across it.
            let context = self.0.contexts.lock().unwrap().get(&id).cloned().flatten();
            if let Some(context) = context {
                context.cancel();
            }
            return Ok(());
        }
        if let Some(id) = &message.id {
            let mut pending = self.0.contexts.lock().unwrap();
            if pending.contains_key(id) {
                return Err(crate::coded_error(
                    tsr_lsproto::ErrorCode::INVALID_REQUEST,
                    Some("duplicate in-flight request ID"),
                ));
            }
            pending.insert(id.clone(), None);
        }
        let id = message.id.clone();
        if self.0.requests.put(&self.0.context, message).is_err() {
            if let Some(id) = id {
                self.0.contexts.lock().unwrap().remove(&id);
            }
            return Err(crate::canceled());
        }
        Ok(())
    }
    pub fn end(&self) {
        self.0.context.cancel();
    }
    pub fn client(&self) -> &Arc<RpcClient> {
        &self.0.client
    }
    fn start_request(&self, id: Option<&Id>) -> Context {
        let context = self.0.context.with_cancel();
        if let Some(id) = id {
            *self
                .0
                .contexts
                .lock()
                .unwrap()
                .get_mut(id)
                .expect("request ID reserved at admission") = Some(context.clone());
        }
        context
    }
    fn cancel_requests(&self, except: Option<&Id>) {
        let contexts: Vec<_> = self
            .0
            .contexts
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| Some(*id) != except)
            .filter_map(|(_, ctx)| ctx.clone())
            .collect();
        for context in contexts {
            context.cancel();
        }
    }
    fn complete(&self, id: Option<Id>, context: &Context, result: Result<RawValue, ResponseError>) {
        if let Some(id) = id {
            let result = if context.err().is_some() {
                Err(crate::canceled())
            } else {
                result
            };
            // Retire before publishing: a client may reuse the ID as soon as
            // it receives this response, while the writer runs concurrently.
            self.0.contexts.lock().unwrap().remove(&id);
            if self.0.client.reply(id, result).is_err() {
                self.end();
            }
        } else if let Err(error) = result {
            let _ = crate::client::notify(
                self.0.client.as_ref(),
                "window/logMessage",
                &tsr_lsproto::LogMessageParams {
                    r#type: tsr_lsproto::MessageType::ERROR,
                    message: error.message,
                },
            );
        }
        context.cancel();
    }
}
type Job = (Option<Id>, Context, Work);
pub struct Connection {
    runtime: Runtime,
    input: Input,
    host: Arc<dyn tsr_vfs::FileSystem>,
}
impl Connection {
    pub fn new(
        options: Options,
        parent: &Context,
        output: Arc<dyn Fn(RawValue) -> Result<(), ResponseError> + Send + Sync>,
        stderr: Box<dyn std::io::Write + Send>,
    ) -> (Self, Input) {
        let context = parent.with_cancel();
        let client = RpcClient::new(context.clone(), output);
        let input = Input(Arc::new(Admission {
            requests: DynamicQueue::new(),
            contexts: Mutex::default(),
            context: context.clone(),
            client: client.clone(),
        }));
        let host = options.host.clone();
        let runtime = Runtime::new(options, context, client, stderr);
        (
            Self {
                runtime,
                input: input.clone(),
                host,
            },
            input,
        )
    }
    // port: tsc/internal/lsp/server.go:Server.dispatchLoop
    pub fn run(mut self) {
        let jobs: DynamicQueue<Job> = DynamicQueue::new();
        let query_context = self.input.0.context.with_cancel();
        let workers: Vec<_> = (0..3)
            .map(|_| {
                let jobs = jobs.clone();
                let context = query_context.clone();
                let input = self.input.clone();
                std::thread::spawn(move || {
                    while let Ok((id, scope, work)) = jobs.get(&context) {
                        let result = work();
                        input.complete(id, &scope, result);
                    }
                })
            })
            .collect();
        while let Ok(message) = self.input.0.requests.get(&self.input.0.context) {
            let context = self.input.start_request(message.id.as_ref());
            if message.method == "shutdown" {
                self.input.cancel_requests(message.id.as_ref());
            }
            let result = self.runtime.prepare(&context, &message, self.host.clone());
            match result {
                Ok(Dispatch::Exit) => break,
                Ok(Dispatch::Ready(value)) => {
                    self.input.complete(message.id, &context, Ok(value));
                }
                Ok(Dispatch::Work(work)) => {
                    if jobs
                        .put(&query_context, (message.id, context, work))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) => self.input.complete(message.id, &context, Err(error)),
            }
        }
        self.input.end();
        query_context.cancel();
        for worker in workers {
            let _ = worker.join();
        }
        self.runtime.close();
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.input.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_dispatched_requests_are_cancellable_but_all_ids_are_reserved() {
        let (send, receive) = std::sync::mpsc::channel();
        let (connection, input) = Connection::new(
            Options::new(
                tsr_project::session::SessionOptions::default(),
                Arc::new(tsr_vfs::MemoryBuilder::new(b"/", true).finish()),
            ),
            &Context::background(),
            Arc::new(move |raw| {
                send.send(raw).unwrap();
                Ok(())
            }),
            Box::new(std::io::sink()),
        );
        let message = Message {
            id: Some(Id::int(7)),
            method: "custom/projectInfo".into(),
            ..Default::default()
        };
        let cancel = || {
            input
                .receive(Message {
                    method: "$/cancelRequest".into(),
                    params: Some(RawValue(br#"{"id":7}"#.to_vec())),
                    ..Default::default()
                })
                .unwrap();
        };
        input.receive(message.clone()).unwrap();
        cancel();
        assert_eq!(
            input.receive(message.clone()).unwrap_err().message,
            "InvalidRequest: duplicate in-flight request ID"
        );
        let queued = input.0.requests.get(&input.0.context).unwrap();
        let context = input.start_request(queued.id.as_ref());
        assert!(
            context.err().is_none(),
            "queued cancellation must not survive dispatch"
        );
        cancel();
        assert!(context.err().is_some());
        input.complete(queued.id, &context, Ok(RawValue(b"null".to_vec())));
        let mut response = Message::default();
        tsr_json::unmarshal(
            &receive.recv().unwrap().0,
            &mut response,
            tsr_json::Options::default(),
        )
        .unwrap();
        let error = response.error.unwrap();
        assert_eq!(
            (error.code, error.message.as_str()),
            (-32800, "RequestCancelled")
        );
        input.receive(message).unwrap(); // Completion frees the ID.
        let next = input.start_request(Some(&Id::int(7)));
        input.end();
        assert!(next.err().is_some(), "teardown cancels active requests too");
        drop(connection);
    }
}
