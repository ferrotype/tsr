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

struct Pending {
    message: Message,
    context: Context,
}
struct Admission {
    requests: DynamicQueue<Pending>,
    contexts: Mutex<HashMap<Id, Context>>,
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
                error: Some(crate::error(-32600, format!("invalid request: {error}"))),
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
            let context = self.0.contexts.lock().unwrap().get(&id).cloned();
            if let Some(context) = context {
                context.cancel();
            }
            return Ok(());
        }
        let context = self.0.context.with_cancel();
        if let Some(id) = &message.id {
            let mut pending = self.0.contexts.lock().unwrap();
            if pending.contains_key(id) {
                return Err(crate::error(-32600, "duplicate in-flight request ID"));
            }
            pending.insert(id.clone(), context.clone());
        }
        self.0
            .requests
            .put(&self.0.context, Pending { message, context })
            .map_err(|_| crate::canceled())
    }
    pub fn end(&self) {
        self.0.context.cancel();
    }
    pub fn client(&self) -> &Arc<RpcClient> {
        &self.0.client
    }
    fn cancel_requests(&self, except: Option<&Id>) {
        let contexts: Vec<_> = self
            .0
            .contexts
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| Some(*id) != except)
            .map(|(_, ctx)| ctx.clone())
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
        while let Ok(pending) = self.input.0.requests.get(&self.input.0.context) {
            let message = pending.message;
            if message.method == "shutdown" {
                self.input.cancel_requests(message.id.as_ref());
            }
            let result = self
                .runtime
                .prepare(&pending.context, &message, self.host.clone());
            match result {
                Ok(Dispatch::Exit) => break,
                Ok(Dispatch::Ready(value)) => {
                    self.input.complete(message.id, &pending.context, Ok(value));
                }
                Ok(Dispatch::Work(work)) => {
                    if jobs
                        .put(&query_context, (message.id, pending.context, work))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) => self
                    .input
                    .complete(message.id, &pending.context, Err(error)),
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
