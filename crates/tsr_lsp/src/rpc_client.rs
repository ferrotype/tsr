//! Reverse requests and outgoing writes share a connection, not the project
//! worker. Responses can therefore unblock filesystem/configuration work.
use crate::{
    client::{raw, Client},
    dynamic_queue::DynamicQueue,
};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use tsr_ipc::Context;
use tsr_json::RawValue;
use tsr_lsproto::{Id, Message, ResponseError};

type Reply = Result<RawValue, ResponseError>;
type Sink = dyn Fn(RawValue) -> Result<(), ResponseError> + Send + Sync;
pub struct RpcClient {
    context: Context,
    output: Arc<Sink>,
    next: AtomicU64,
    pending: Mutex<HashMap<Id, DynamicQueue<Reply>>>,
}
impl RpcClient {
    pub fn new(context: Context, output: Arc<Sink>) -> Arc<Self> {
        Arc::new(Self {
            context,
            output,
            next: AtomicU64::new(0),
            pending: Mutex::default(),
        })
    }
    pub fn write(&self, message: &Message) -> Result<(), ResponseError> {
        (self.output)(raw(message)?)
    }
    pub fn reply(&self, id: Id, result: Reply) -> Result<(), ResponseError> {
        (self.output)(Self::encode_reply(id, result)?)
    }
    /// Prepare a response for a router which owns its request-completion
    /// barrier. Encoding failures still belong only to this request.
    pub fn encode_reply(id: Id, result: Reply) -> Result<RawValue, ResponseError> {
        let message = match result {
            Ok(value) => Message {
                id: Some(id),
                result: Some(value),
                ..Default::default()
            },
            Err(error) => Message {
                id: Some(id),
                error: Some(error),
                ..Default::default()
            },
        };
        // Encode before handing bytes to the writer. A serialization failure
        // belongs to this request and must not tear down the connection.
        raw(&message).or_else(|error| {
            raw(&Message {
                id: message.id.clone(),
                error: Some(crate::error(
                    -32603,
                    format!("failed to encode response: {}", error.message),
                )),
                ..Default::default()
            })
        })
    }
    /// Unknown or retired replies are ignored, as in the pin's pending map.
    pub fn receive(&self, message: &Message) -> bool {
        if !message.is_response() {
            return false;
        }
        let Some(queue) = self
            .pending
            .lock()
            .expect("reverse calls")
            .remove(message.id.as_ref().unwrap())
        else {
            return false;
        };
        let result = message.error.clone().map_or_else(
            || {
                Ok(message
                    .result
                    .clone()
                    .unwrap_or_else(|| RawValue(b"null".to_vec())))
            },
            Err,
        );
        let _ = queue.put(&Context::background(), result);
        true
    }
    fn next_id(&self) -> Id {
        Id::string(format!(
            "ts{}",
            self.next.fetch_add(1, Ordering::Relaxed) + 1
        ))
    }
}
impl Client for RpcClient {
    fn notify(&self, method: &str, params: RawValue) -> Result<(), ResponseError> {
        if self.context.err().is_some() {
            return Err(crate::canceled());
        }
        self.write(&Message {
            method: method.into(),
            params: (params.0 != b"null").then_some(params),
            ..Default::default()
        })
    }
    // port: tsc/internal/lsp/server.go:sendClientRequest
    fn request(&self, context: &Context, method: &str, params: RawValue) -> Reply {
        if context.err().is_some() || self.context.err().is_some() {
            return Err(crate::canceled());
        }
        let id = self.next_id();
        let queue = DynamicQueue::new();
        self.pending
            .lock()
            .expect("reverse calls")
            .insert(id.clone(), queue.clone());
        struct Remove<'a>(&'a RpcClient, Id);
        impl Drop for Remove<'_> {
            fn drop(&mut self) {
                self.0
                    .pending
                    .lock()
                    .expect("reverse calls")
                    .remove(&self.1);
            }
        }
        let _remove = Remove(self, id.clone());
        let scope = context.with_cancel();
        let cancel = scope.clone();
        let stop = self.context.after_func(move || cancel.cancel());
        struct Stop(tsr_ipc::AfterFuncStop);
        impl Drop for Stop {
            fn drop(&mut self) {
                self.0.stop();
            }
        }
        let _stop = Stop(stop);
        self.write(&Message {
            id: Some(id),
            method: method.into(),
            params: (params.0 != b"null").then_some(params),
            ..Default::default()
        })?;
        queue.get(&scope).map_err(|_| crate::canceled())?
    }
    // port: tsc/internal/lsp/server.go:sendClientRequestFireAndForget
    fn request_without_waiting(&self, method: &str, params: RawValue) -> Result<(), ResponseError> {
        if self.context.err().is_some() {
            return Err(crate::canceled());
        }
        self.write(&Message {
            id: Some(self.next_id()),
            method: method.into(),
            params: (params.0 != b"null").then_some(params),
            ..Default::default()
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_response_fails_only_its_request_and_late_replies_are_ignored() {
        let (send, receive) = std::sync::mpsc::channel();
        let client = RpcClient::new(
            Context::background(),
            Arc::new(move |value| {
                send.send(value).unwrap();
                Ok(())
            }),
        );
        client
            .reply(Id::string("bad"), Ok(RawValue(vec![b'['; 20_000])))
            .unwrap();
        client
            .reply(Id::string("good"), Ok(RawValue(b"null".to_vec())))
            .unwrap();
        let mut bad = Message::default();
        tsr_json::unmarshal(
            &receive.recv().unwrap().0,
            &mut bad,
            tsr_json::Options::default(),
        )
        .unwrap();
        assert_eq!(bad.error.unwrap().code, -32603);
        let mut good = Message::default();
        tsr_json::unmarshal(
            &receive.recv().unwrap().0,
            &mut good,
            tsr_json::Options::default(),
        )
        .unwrap();
        assert!(good.error.is_none());
        assert_eq!(good.id, Some(Id::string("good")));
        assert!(!client.receive(&Message {
            id: Some(Id::string("unknown")),
            result: Some(RawValue(b"null".to_vec())),
            ..Default::default()
        }));
    }
}
