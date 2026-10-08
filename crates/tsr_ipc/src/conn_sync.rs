//! The synchronous connection: one request at a time on the reading thread,
//! and outgoing calls serialized under one lock with their reply read inline,
//! because the msgpack protocol correlates by method name and handler threads
//! (the compiler reading files) may call the client concurrently.
//! port: tsc/internal/ipc/conn_sync.go
use crate::timing::{server_timing_snapshot, TimingCollector};
use crate::{
    Closer, Conn, Context, Error, Handler, Protocol, Response, Stream, METHOD_GET_SERVER_TIMING,
    METHOD_RESET_SERVER_TIMING,
};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tsr_json::{Encode, RawValue};
use tsr_jsonrpc::{Id, Message, ResponseError, CODE_INTERNAL_ERROR};

pub struct SyncConn {
    closer: Option<Arc<dyn Closer>>,
    protocol: Arc<dyn Protocol>,
    handler: Arc<dyn Handler>,
    timing: Mutex<Option<Arc<TimingCollector>>>,
    /// Serializes every protocol operation: a call's write and inline read
    /// must not interleave with another thread's.
    io: Mutex<()>,
}

impl SyncConn {
    /// port: tsc/internal/ipc/conn_sync.go:NewSyncConn
    pub fn new(stream: Stream, protocol: Arc<dyn Protocol>, handler: Arc<dyn Handler>) -> Self {
        Self::with_closer(Some(stream.closer), protocol, handler)
    }

    pub fn with_closer(
        closer: Option<Arc<dyn Closer>>,
        protocol: Arc<dyn Protocol>,
        handler: Arc<dyn Handler>,
    ) -> Self {
        Self {
            closer,
            protocol,
            handler,
            timing: Mutex::new(None),
            io: Mutex::new(()),
        }
    }

    /// port: tsc/internal/ipc/conn_sync.go:SyncConn.SetCollectTiming
    pub fn set_collect_timing(&self, enabled: bool) {
        *self.timing.lock().expect("timing lock") =
            enabled.then(|| Arc::new(TimingCollector::new()));
    }

    fn timing(&self) -> Option<Arc<TimingCollector>> {
        self.timing.lock().expect("timing lock").clone()
    }

    fn write_response(&self, id: Option<&Id>, result: Option<&dyn Encode>) -> Result<(), Error> {
        let _io = self.io.lock().expect("sync io");
        self.protocol.write_response(id, result)
    }

    fn write_error(&self, id: Option<&Id>, message: String) -> Result<(), Error> {
        let _io = self.io.lock().expect("sync io");
        self.protocol.write_error(
            id,
            &ResponseError {
                code: CODE_INTERNAL_ERROR,
                message,
                data: None,
            },
        )
    }

    /// port: tsc/internal/ipc/conn_sync.go:SyncConn.handleRequest
    fn handle_request(&self, ctx: &Context, message: &Message) -> Result<(), Error> {
        let id = message.id.as_ref();
        let timing = self.timing();
        match message.method.as_str() {
            METHOD_GET_SERVER_TIMING => {
                let snapshot = server_timing_snapshot(timing.as_deref());
                return self.write_response(id, Some(&snapshot)).map_err(|error| {
                    Error::Wrapped(
                        "ipc: failed to write server timing response".into(),
                        Box::new(error),
                    )
                });
            }
            METHOD_RESET_SERVER_TIMING => {
                if let Some(timing) = &timing {
                    timing.reset();
                }
                return self.write_response(id, None).map_err(|error| {
                    Error::Wrapped(
                        "ipc: failed to write reset server timing response".into(),
                        Box::new(error),
                    )
                });
            }
            _ => {}
        }
        let start = Instant::now();
        let handled = catch_unwind(AssertUnwindSafe(|| {
            self.handler
                .handle_request(ctx, &message.method, message.params_bytes())
        }));
        let result = match handled {
            Ok(result) => result,
            Err(payload) => {
                let value = crate::conn_async::panic_text(payload.as_ref());
                let stack = std::backtrace::Backtrace::force_capture();
                return self
                    .write_error(id, format!("panic: {value}\n{stack}"))
                    .map_err(|error| {
                        Error::Message(format!(
                            "ipc: failed to write panic error response: {error} (original panic: {value})"
                        ))
                    });
            }
        };
        if let Some(timing) = &timing {
            timing.record(&message.method, start.elapsed());
        }
        let written = match result {
            Err(error) => self.write_error(id, error.to_string()),
            Ok(None) => self.write_response(id, None),
            Ok(Some(Response::Json(value))) => {
                self.write_response(id, Some(value.as_ref() as &dyn Encode))
            }
            Ok(Some(Response::Binary(bytes))) => {
                let _io = self.io.lock().expect("sync io");
                self.protocol.write_binary_response(id, &bytes)
            }
        };
        written.map_err(|error| {
            Error::Wrapped("ipc: failed to write response".into(), Box::new(error))
        })
    }
}

impl Conn for SyncConn {
    /// port: tsc/internal/ipc/conn_sync.go:SyncConn.Run
    fn run(&self, ctx: &Context) -> Result<(), Error> {
        let result = loop {
            if let Some(error) = ctx.err() {
                break Err(Error::Context(error));
            }
            let message = match self.protocol.read_message() {
                Ok(message) => message,
                Err(error) if error.is_eof() => break Ok(()),
                Err(error) => break Err(error),
            };
            if message.is_request() {
                if let Err(error) = self.handle_request(ctx, &message) {
                    break Err(error);
                }
            } else if message.is_notification() {
                let _ =
                    self.handler
                        .handle_notification(ctx, &message.method, message.params_bytes());
            }
            // A response outside a call is the pin's silent drop: the reply
            // slot that would have taken it is gone.
        };
        if let Some(closer) = &self.closer {
            let _ = closer.close();
        }
        result
    }

    /// port: tsc/internal/ipc/conn_sync.go:SyncConn.Call
    fn call(&self, ctx: &Context, method: &str, params: &dyn Encode) -> Result<RawValue, Error> {
        let _io = self.io.lock().expect("sync io");
        let id = Id::string(method);
        self.protocol.write_request(&id, method, Some(params))?;
        if let Some(error) = ctx.err() {
            return Err(Error::Context(error));
        }
        let message = self.protocol.read_message()?;
        if message.is_response()
            && message
                .id
                .as_ref()
                .is_some_and(|got| got.to_string() == method)
        {
            if let Some(error) = message.error {
                return Err(Error::Remote {
                    code: error.code,
                    message: error.message,
                });
            }
            return Ok(message.result.unwrap_or_default());
        }
        Err(Error::Message(format!(
            "ipc: unexpected message while waiting for {method:?} response"
        )))
    }

    /// port: tsc/internal/ipc/conn_sync.go:SyncConn.Notify
    fn notify(&self, _ctx: &Context, method: &str, params: &dyn Encode) -> Result<(), Error> {
        let _io = self.io.lock().expect("sync io");
        self.protocol.write_notification(method, Some(params))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HandlerError, HandlerResult};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A protocol that hands out one message and fails every write, as the
    /// pin's `syncFailingResponseProtocol`.
    struct FailingResponses {
        message: Mutex<Option<Message>>,
    }
    impl Protocol for FailingResponses {
        fn read_message(&self) -> Result<Message, Error> {
            match self.message.lock().unwrap().take() {
                Some(message) => Ok(message),
                None => Err(Error::Framing(Arc::new(tsr_jsonrpc::FramingError::Eof))),
            }
        }
        fn write_request(&self, _: &Id, _: &str, _: Option<&dyn Encode>) -> Result<(), Error> {
            Ok(())
        }
        fn write_notification(&self, _: &str, _: Option<&dyn Encode>) -> Result<(), Error> {
            Ok(())
        }
        fn write_response(&self, _: Option<&Id>, _: Option<&dyn Encode>) -> Result<(), Error> {
            Err(Error::Message("response write failed".into()))
        }
        fn write_error(&self, _: Option<&Id>, _: &ResponseError) -> Result<(), Error> {
            Err(Error::Message("response write failed".into()))
        }
    }
    struct NoOp;
    impl Handler for NoOp {
        fn handle_request(&self, _: &Context, _: &str, _: &[u8]) -> HandlerResult {
            Ok(None)
        }
        fn handle_notification(&self, _: &Context, _: &str, _: &[u8]) -> Result<(), HandlerError> {
            Ok(())
        }
    }
    struct Panics;
    impl Handler for Panics {
        fn handle_request(&self, _: &Context, _: &str, _: &[u8]) -> HandlerResult {
            panic!("handler panic")
        }
        fn handle_notification(&self, _: &Context, _: &str, _: &[u8]) -> Result<(), HandlerError> {
            Ok(())
        }
    }
    fn transform() -> Message {
        Message {
            id: Some(Id::int(1)),
            method: "transform".into(),
            ..Default::default()
        }
    }

    // source: tsc/internal/ipc/conn_sync_test.go:TestSyncConnRunReturnsResponseWriteFailure
    #[test]
    fn run_returns_the_response_write_failure() {
        let protocol = Arc::new(FailingResponses {
            message: Mutex::new(Some(transform())),
        });
        let conn = SyncConn::with_closer(None, protocol, Arc::new(NoOp));
        let error = conn.run(&Context::background()).unwrap_err();
        assert!(
            error.to_string().contains("response write failed"),
            "{error}"
        );
    }

    // source: tsc/internal/ipc/conn_sync_test.go:TestSyncConnRunReturnsPanicResponseWriteFailure
    #[test]
    fn run_returns_the_panic_response_write_failure_with_the_panic() {
        let protocol = Arc::new(FailingResponses {
            message: Mutex::new(Some(transform())),
        });
        let conn = SyncConn::with_closer(None, protocol, Arc::new(Panics));
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let error = conn.run(&Context::background()).unwrap_err();
        std::panic::set_hook(previous);
        let text = error.to_string();
        assert!(text.contains("response write failed"), "{text}");
        assert!(text.contains("original panic: handler panic"), "{text}");
    }

    /// Calls from several handler threads share one reply slot and never
    /// interleave their frames.
    struct Echoing {
        writes: Mutex<Vec<String>>,
        replies: Mutex<Vec<Message>>,
        reads: AtomicUsize,
    }
    impl Protocol for Echoing {
        fn read_message(&self) -> Result<Message, Error> {
            let mut replies = self.replies.lock().unwrap();
            self.reads.fetch_add(1, Ordering::SeqCst);
            replies
                .pop()
                .ok_or_else(|| Error::Framing(Arc::new(tsr_jsonrpc::FramingError::Eof)))
        }
        fn write_request(
            &self,
            id: &Id,
            method: &str,
            _: Option<&dyn Encode>,
        ) -> Result<(), Error> {
            self.writes.lock().unwrap().push(format!("call {method}"));
            self.replies.lock().unwrap().push(Message {
                id: Some(id.clone()),
                result: Some(RawValue(format!("\"{method}\"").into_bytes())),
                ..Default::default()
            });
            Ok(())
        }
        fn write_notification(&self, _: &str, _: Option<&dyn Encode>) -> Result<(), Error> {
            Ok(())
        }
        fn write_response(&self, _: Option<&Id>, _: Option<&dyn Encode>) -> Result<(), Error> {
            Ok(())
        }
        fn write_error(&self, _: Option<&Id>, _: &ResponseError) -> Result<(), Error> {
            Ok(())
        }
    }

    #[test]
    fn calls_are_serialized_and_answered_by_method_name() {
        let protocol = Arc::new(Echoing {
            writes: Mutex::new(Vec::new()),
            replies: Mutex::new(Vec::new()),
            reads: AtomicUsize::new(0),
        });
        let conn = Arc::new(SyncConn::with_closer(
            None,
            protocol.clone(),
            Arc::new(NoOp),
        ));
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let conn = conn.clone();
                std::thread::spawn(move || {
                    let method = format!("readFile{i}");
                    let reply = conn.call(&Context::background(), &method, &"x").unwrap();
                    assert_eq!(reply.0, format!("\"{method}\"").into_bytes());
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(protocol.writes.lock().unwrap().len(), 8);
        assert_eq!(protocol.reads.load(Ordering::SeqCst), 8);
    }
}
