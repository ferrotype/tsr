//! LSP logs go to stderr before initialization and to the client afterwards.
use crate::client::{self, Client};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use tsr_ipc::Context;
use tsr_lsproto::{LogMessageParams, LogVerbosity, MessageType};

// The pin's logger state; verbosity is read without holding an output lock.
pub struct Logger {
    client: Arc<dyn Client>,
    context: Context,
    stderr: Mutex<Box<dyn Write + Send>>,
    initialized: AtomicBool,
    verbosity: AtomicI32,
}
impl Logger {
    // port: tsc/internal/lsp/logger.go:newLogger
    pub fn new(client: Arc<dyn Client>, context: Context, stderr: Box<dyn Write + Send>) -> Self {
        Self {
            client,
            context,
            stderr: Mutex::new(stderr),
            initialized: AtomicBool::new(false),
            verbosity: AtomicI32::new(LogVerbosity::INFO.0),
        }
    }
    pub fn initialize_started(&self) {
        self.initialized.store(true, Ordering::Relaxed);
    }
    // port: tsc/internal/lsp/logger.go:logger.SetVerbosity
    pub fn set_verbosity(&self, verbosity: LogVerbosity) {
        self.verbosity.store(verbosity.0, Ordering::Relaxed);
    }
    // port: tsc/internal/lsp/logger.go:logger.IsTracing
    pub fn is_tracing(&self) -> bool {
        self.verbosity.load(Ordering::Relaxed) == LogVerbosity::TRACE.0
    }
    // port: tsc/internal/lsp/logger.go:logger.IsVerbose
    pub fn is_verbose(&self) -> bool {
        (LogVerbosity::TRACE.0..=LogVerbosity::DEBUG.0)
            .contains(&self.verbosity.load(Ordering::Relaxed))
    }
    // port: tsc/internal/lsp/logger.go:logger.SetVerbose
    pub fn set_verbose(&self, verbose: bool) {
        self.set_verbosity(if verbose {
            LogVerbosity::DEBUG
        } else {
            LogVerbosity::INFO
        });
    }
    // port: tsc/internal/lsp/logger.go:logger.sendLogMessage
    pub fn send(&self, kind: MessageType, message: String) {
        if !self.initialized.load(Ordering::Relaxed) {
            self.fallback(&message);
            return;
        }
        let verbosity = self.verbosity.load(Ordering::Relaxed);
        if verbosity == LogVerbosity::OFF.0 || verbosity > max_verbosity_for_message_type(kind).0 {
            return;
        }
        let params = LogMessageParams {
            r#type: kind,
            message,
        };
        if client::notify(self.client.as_ref(), "window/logMessage", &params).is_err()
            && self.context.err().is_some()
        {
            self.fallback(&params.message);
        }
    }
    fn fallback(&self, message: &str) {
        let _ = writeln!(self.stderr.lock().expect("stderr lock"), "{message}");
    }
}
impl tsr_project::logging::LogSink for Logger {
    fn log(&self, message: &str) {
        self.send(MessageType::INFO, message.to_owned());
    }
    fn set_verbose(&self, verbose: bool) {
        self.set_verbose(verbose);
    }
    fn is_verbose(&self) -> bool {
        self.is_verbose()
    }
}
// port: tsc/internal/lsp/logger.go:maxVerbosityForMessageType
pub fn max_verbosity_for_message_type(kind: MessageType) -> LogVerbosity {
    match kind {
        MessageType::ERROR => LogVerbosity::ERROR,
        MessageType::WARNING => LogVerbosity::WARNING,
        MessageType::DEBUG => LogVerbosity::DEBUG,
        _ => LogVerbosity::INFO,
    }
}
// port: tsc/internal/lsp/logger.go:isValidLogVerbosity
pub fn is_valid_log_verbosity(verbosity: LogVerbosity) -> bool {
    (LogVerbosity::OFF.0..=LogVerbosity::ERROR.0).contains(&verbosity.0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn filter_levels_trace_and_shutdown_fallback() {
        let context = Context::background().with_cancel();
        let (send, receive) = std::sync::mpsc::channel();
        let client = crate::rpc_client::RpcClient::new(
            context.clone(),
            Arc::new(move |value| {
                send.send(value).unwrap();
                Ok(())
            }),
        );
        let stderr = Buffer::default();
        let logger = Logger::new(client, context.clone(), Box::new(stderr.clone()));
        logger.set_verbosity(LogVerbosity::OFF);
        logger.send(MessageType::DEBUG, "before init".into());
        assert_eq!(*stderr.0.lock().unwrap(), b"before init\n");
        logger.initialize_started();
        logger.send(MessageType::ERROR, "hidden".into());
        assert!(receive.try_recv().is_err());
        logger.set_verbosity(LogVerbosity::WARNING);
        logger.send(MessageType::INFO, "hidden".into());
        logger.send(MessageType::WARNING, "visible".into());
        assert!(String::from_utf8(receive.try_recv().unwrap().0)
            .unwrap()
            .contains("visible"));
        assert!(receive.try_recv().is_err());
        logger.set_verbosity(LogVerbosity::TRACE);
        assert!(logger.is_tracing() && logger.is_verbose());
        logger.set_verbose(false);
        assert!(!logger.is_tracing() && !logger.is_verbose());
        context.cancel();
        logger.send(MessageType::ERROR, "after shutdown".into());
        assert_eq!(*stderr.0.lock().unwrap(), b"before init\nafter shutdown\n");
    }
}
