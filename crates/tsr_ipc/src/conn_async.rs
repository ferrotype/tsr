//! Bidirectional JSON-RPC with each incoming request handled on its own thread.
use crate::timing::{
    server_timing_snapshot, TimingCollector, METHOD_GET_SERVER_TIMING, METHOD_RESET_SERVER_TIMING,
};
use crate::{Closer, Conn, Context, Error, Handler, JsonRpcProtocol, Protocol, Stream};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tsr_json::{Encode, RawValue};
use tsr_jsonrpc::{Id, Message, ResponseError, CODE_INTERNAL_ERROR};

#[derive(Default)]
struct Pending {
    calls: HashMap<Id, SyncSender<Message>>,
    terminal: Option<Error>,
    has_cause: bool,
}

/// The handlers still running, which `run` waits for before it returns (the
/// pin's `sync.WaitGroup`): a finished handler leaves nothing behind, however
/// long the connection lives.
#[derive(Default)]
struct Handlers {
    active: Mutex<usize>,
    idle: Condvar,
}

impl Handlers {
    fn wait(&self) {
        let mut active = self.active.lock().expect("handler count");
        while *active > 0 {
            active = self.idle.wait(active).expect("handler count");
        }
    }
}

/// Counts its handler done when the handler's thread ends, by a panic too.
struct HandlerDone(Arc<Inner>);

impl Drop for HandlerDone {
    fn drop(&mut self) {
        let handlers = &self.0.handlers;
        let mut active = handlers.active.lock().expect("handler count");
        *active -= 1;
        if *active == 0 {
            handlers.idle.notify_all();
        }
    }
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
    handlers: Handlers,
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
            handlers: Handlers::default(),
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

    /// Runs a handler on its own thread, detached: only the count of running
    /// handlers is kept.
    fn spawn(&self, work: impl FnOnce() + Send + 'static) {
        *self.0.handlers.active.lock().expect("handler count") += 1;
        let done = HandlerDone(self.0.clone());
        std::thread::spawn(move || {
            let _done = done;
            work();
        });
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
        self.0.handlers.wait();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{pipe, HandlerError, HandlerResult};

    /// Counts the notifications it handled; a `hold` notification waits
    /// until the test releases it.
    #[derive(Default)]
    struct Counting {
        handled: Mutex<usize>,
        released: Mutex<bool>,
        changed: Condvar,
    }

    impl Handler for Counting {
        fn handle_request(&self, _: &Context, _: &str, _: &[u8]) -> HandlerResult {
            Ok(None)
        }
        fn handle_notification(
            &self,
            _: &Context,
            method: &str,
            _: &[u8],
        ) -> Result<(), HandlerError> {
            if method == "hold" {
                let mut released = self.released.lock().unwrap();
                while !*released {
                    released = self.changed.wait(released).unwrap();
                }
            }
            *self.handled.lock().unwrap() += 1;
            self.changed.notify_all();
            Ok(())
        }
    }

    fn active(conn: &AsyncConn) -> usize {
        *conn.0.handlers.active.lock().unwrap()
    }

    /// Waits up to ten seconds for `ready`.
    fn eventually(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready() {
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn finished_handlers_leave_nothing_behind_and_run_waits_for_the_rest() {
        let (client_end, server_end) = pipe();
        let closer = client_end.closer.clone();
        let counting = Arc::new(Counting::default());
        let server = AsyncConn::new(server_end, counting.clone());
        let running = server.clone();
        let server_run = std::thread::spawn(move || running.run(&Context::background()));
        let client = AsyncConn::new(client_end, Arc::new(Counting::default()));
        let reading = client.clone();
        let client_run = std::thread::spawn(move || reading.run(&Context::background()));
        let ctx = Context::background();
        let params = RawValue(b"{}".to_vec());
        for _ in 0..2000 {
            client.notify(&ctx, "log", &params).unwrap();
        }
        for _ in 0..100 {
            client.call(&ctx, "nothing", &params).unwrap();
        }
        eventually(|| *counting.handled.lock().unwrap() == 2000 && active(&server) == 0);
        // A handler still running when the stream ends holds `run` back.
        client.notify(&ctx, "hold", &params).unwrap();
        eventually(|| active(&server) == 1);
        closer.close().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(!server_run.is_finished());
        *counting.released.lock().unwrap() = true;
        counting.changed.notify_all();
        server_run.join().unwrap().unwrap();
        assert_eq!(*counting.handled.lock().unwrap(), 2001);
        assert_eq!(active(&server), 0);
        let _ = client_run.join().unwrap();
    }
}
