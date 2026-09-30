//! Bidirectional JSON-RPC with each incoming request handled on its own thread.
use crate::timing::{
    server_timing_snapshot, TimingCollector, METHOD_GET_SERVER_TIMING, METHOD_RESET_SERVER_TIMING,
};
use crate::{Closer, Conn, Context, Error, Handler, JsonRpcProtocol, Protocol, Stream};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tsr_json::{Encode, RawValue};
use tsr_jsonrpc::{Id, Message, ResponseError, CODE_INTERNAL_ERROR};

#[derive(Default)]
struct Pending {
    calls: HashMap<Id, SyncSender<Message>>,
    terminal: Option<Error>,
    has_cause: bool,
}

struct Inner {
    closer: Option<Arc<dyn Closer>>,
    protocol: Arc<dyn Protocol>,
    handler: Arc<dyn Handler>,
    /// When set, the time spent handling each request, for `getServerTiming`.
    timing: Mutex<Option<Arc<TimingCollector>>>,
    seq: AtomicI64,
    pending: Mutex<Pending>,
    write: Mutex<()>,
    handlers: Mutex<Vec<JoinHandle<()>>>,
}

/// A bidirectional JSON-RPC connection. Cloning shares the connection.
#[derive(Clone)]
pub struct AsyncConn(Arc<Inner>);

impl AsyncConn {
    /// A connection over `stream` with the JSON-RPC protocol.
    /// port: tsc/internal/ipc/conn_async.go:NewAsyncConn
    pub fn new(stream: Stream, handler: Arc<dyn Handler>) -> Self {
        let protocol = JsonRpcProtocol::new(stream.reader, stream.writer);
        Self::with_protocol(Some(stream.closer), protocol, handler)
    }

    /// port: tsc/internal/ipc/conn_async.go:NewAsyncConnWithProtocol
    pub fn with_protocol(
        closer: Option<Arc<dyn Closer>>,
        protocol: Arc<dyn Protocol>,
        handler: Arc<dyn Handler>,
    ) -> Self {
        Self(Arc::new(Inner {
            closer,
            protocol,
            handler,
            timing: Mutex::new(None),
            seq: AtomicI64::new(0),
            pending: Mutex::new(Pending::default()),
            write: Mutex::new(()),
            handlers: Mutex::new(Vec::new()),
        }))
    }

    /// Enables or disables per-request processing-time measurement.
    /// port: tsc/internal/ipc/conn_async.go:AsyncConn.SetCollectTiming
    pub fn set_collect_timing(&self, enabled: bool) {
        *self.0.timing.lock().expect("timing lock") =
            enabled.then(|| Arc::new(TimingCollector::new()));
    }

    fn timing(&self) -> Option<Arc<TimingCollector>> {
        self.0.timing.lock().expect("timing lock").clone()
    }

    fn spawn(&self, work: impl FnOnce() + Send + 'static) {
        let handle = std::thread::spawn(work);
        self.0.handlers.lock().expect("handler list").push(handle);
    }

    fn close(&self) {
        if let Some(closer) = &self.0.closer {
            let _ = closer.close();
        }
    }

    /// The read loop exited: record it and unblock every waiting call.
    /// port: tsc/internal/ipc/conn_async.go:AsyncConn.closePendingCalls
    fn close_pending_calls(&self, run_error: Option<&Error>) {
        let mut pending = self.0.pending.lock().expect("pending calls");
        Self::record_terminal_error_locked(&mut pending, run_error);
        Self::close_pending_calls_locked(&mut pending);
    }

    /// port: tsc/internal/ipc/conn_async.go:AsyncConn.recordRequestError
    fn record_request_error(&self, error: Error, errors: &SyncSender<Error>) -> bool {
        let mut pending = self.0.pending.lock().expect("pending calls");
        if !Self::record_terminal_error_locked(&mut pending, Some(&error)) {
            return false;
        }
        let _ = errors.try_send(error);
        Self::close_pending_calls_locked(&mut pending);
        true
    }

    /// port: tsc/internal/ipc/conn_async.go:AsyncConn.recordTerminalErrorLocked
    fn record_terminal_error_locked(pending: &mut Pending, error: Option<&Error>) -> bool {
        if pending.terminal.is_none() {
            pending.terminal = Some(Error::ConnClosed(None));
            if let Some(error) = error {
                pending.terminal = Some(Error::ConnClosed(Some(Box::new(error.clone()))));
                pending.has_cause = true;
                return true;
            }
        } else if !pending.has_cause {
            if let Some(error) = error {
                pending.terminal = Some(Error::ConnClosed(Some(Box::new(error.clone()))));
                pending.has_cause = true;
                return true;
            }
        }
        false
    }

    /// Dropping a waiting call's sender wakes it with the terminal error.
    /// port: tsc/internal/ipc/conn_async.go:AsyncConn.closePendingCallsLocked
    fn close_pending_calls_locked(pending: &mut Pending) {
        pending.calls.clear();
    }

    /// Hands a response to the call waiting for it.
    /// port: tsc/internal/ipc/conn_async.go:AsyncConn.handleResponse
    fn handle_response(&self, message: Message) {
        let Some(id) = message.id.clone() else {
            return;
        };
        let sender = self
            .0
            .pending
            .lock()
            .expect("pending calls")
            .calls
            .remove(&id);
        if let Some(sender) = sender {
            let _ = sender.try_send(message);
        }
    }

    fn write_response(&self, id: Option<&Id>, result: Option<&dyn Encode>) -> Result<(), Error> {
        let _write = self.0.write.lock().expect("write lock");
        self.0.protocol.write_response(id, result)
    }

    fn write_error(&self, id: Option<&Id>, message: String) -> Result<(), Error> {
        let _write = self.0.write.lock().expect("write lock");
        self.0.protocol.write_error(
            id,
            &ResponseError {
                code: CODE_INTERNAL_ERROR,
                message,
                data: None,
            },
        )
    }

    /// Answers one request: the timing meta-requests itself, anything else
    /// through the handler, with a panic answered as an internal error.
    /// port: tsc/internal/ipc/conn_async.go:AsyncConn.handleRequest
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
            self.0
                .handler
                .handle_request(ctx, &message.method, message.params_bytes())
        }));
        let result = match handled {
            Ok(result) => result,
            Err(payload) => {
                let value = panic_text(payload.as_ref());
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
            Ok(result) => {
                self.write_response(id, result.as_deref().map(|result| result as &dyn Encode))
            }
        };
        written.map_err(|error| {
            Error::Wrapped("ipc: failed to write response".into(), Box::new(error))
        })
    }
}

impl AsyncConn {
    /// port: tsc/internal/ipc/conn_async.go:AsyncConn.handleNotification
    fn handle_notification(&self, ctx: &Context, message: &Message) {
        let _ = self
            .0
            .handler
            .handle_notification(ctx, &message.method, message.params_bytes());
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_owned())
}

/// Removes a call's pending entry when the call returns.
struct PendingEntry<'a> {
    conn: &'a AsyncConn,
    id: Id,
}

impl Drop for PendingEntry<'_> {
    fn drop(&mut self) {
        self.conn
            .0
            .pending
            .lock()
            .expect("pending calls")
            .calls
            .remove(&self.id);
    }
}

/// How often a waiting call rechecks its context.
const CONTEXT_POLL: Duration = Duration::from_millis(10);

impl Conn for AsyncConn {
    /// port: tsc/internal/ipc/conn_async.go:AsyncConn.Run
    fn run(&self, ctx: &Context) -> Result<(), Error> {
        let handler_ctx = ctx.with_cancel();
        let (request_errors, request_error) = sync_channel::<Error>(1);
        let result = loop {
            if let Some(error) = ctx.err() {
                break Err(Error::Context(error));
            }
            let message = match self.0.protocol.read_message() {
                Ok(message) => message,
                Err(error) if error.is_eof() => break Ok(()),
                Err(error) => break Err(error),
            };
            if message.is_response() {
                self.handle_response(message);
            } else if message.is_request() {
                let (conn, ctx, errors) =
                    (self.clone(), handler_ctx.clone(), request_errors.clone());
                self.spawn(move || {
                    if let Err(error) = conn.handle_request(&ctx, &message) {
                        if conn.record_request_error(error, &errors) {
                            conn.close();
                        }
                    }
                });
            } else if message.is_notification() {
                let (conn, ctx) = (self.clone(), handler_ctx.clone());
                self.spawn(move || conn.handle_notification(&ctx, &message));
            }
        };
        self.close_pending_calls(result.as_ref().err());
        handler_ctx.cancel();
        let handlers = std::mem::take(&mut *self.0.handlers.lock().expect("handler list"));
        for handler in handlers {
            let _ = handler.join();
        }
        match (result, request_error.try_recv()) {
            (Ok(()), Ok(request_error)) => Err(request_error),
            (Err(error), Ok(request_error)) => Err(error.join(request_error)),
            (result, Err(_)) => result,
        }
    }

    /// port: tsc/internal/ipc/conn_async.go:AsyncConn.Call
    fn call(&self, ctx: &Context, method: &str, params: &dyn Encode) -> Result<RawValue, Error> {
        let id = Id::string(format!(
            "api{}",
            self.0.seq.fetch_add(1, Ordering::SeqCst) + 1
        ));
        let (sender, receiver) = sync_channel(1);
        {
            let mut pending = self.0.pending.lock().expect("pending calls");
            if let Some(terminal) = &pending.terminal {
                return Err(terminal.clone());
            }
            pending.calls.insert(id.clone(), sender);
        }
        let _entry = PendingEntry {
            conn: self,
            id: id.clone(),
        };
        {
            let _write = self.0.write.lock().expect("write lock");
            self.0.protocol.write_request(&id, method, Some(params))?;
        }
        loop {
            if let Some(error) = ctx.err() {
                return Err(Error::Context(error));
            }
            let wait = ctx.deadline().map_or(CONTEXT_POLL, |deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(CONTEXT_POLL)
            });
            match receiver.recv_timeout(wait) {
                Ok(response) => {
                    if let Some(error) = response.error {
                        return Err(Error::Remote {
                            code: error.code,
                            message: error.message,
                        });
                    }
                    return Ok(response.result.unwrap_or_default());
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let terminal = self
                        .0
                        .pending
                        .lock()
                        .expect("pending calls")
                        .terminal
                        .clone();
                    return Err(terminal.unwrap_or(Error::ConnClosed(None)));
                }
            }
        }
    }

    /// port: tsc/internal/ipc/conn_async.go:AsyncConn.Notify
    fn notify(&self, _ctx: &Context, method: &str, params: &dyn Encode) -> Result<(), Error> {
        if let Some(terminal) = &self.0.pending.lock().expect("pending calls").terminal {
            return Err(terminal.clone());
        }
        let _write = self.0.write.lock().expect("write lock");
        self.0.protocol.write_notification(method, Some(params))
    }
}
