//! Request-local panic recovery shared by preparation and checker workers.
use crate::{
    client::{self, Client},
    logger::Logger,
};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tsr_lsproto as lsp;

#[derive(Clone)]
pub(crate) struct Recovery {
    client: Arc<dyn Client>,
    logger: Arc<Logger>,
    telemetry: Arc<AtomicBool>,
}
impl Recovery {
    pub fn new(client: Arc<dyn Client>, logger: Arc<Logger>) -> Self {
        Self {
            client,
            logger,
            telemetry: Arc::default(),
        }
    }
    pub fn set_telemetry(&self, enabled: bool) {
        self.telemetry.store(enabled, Ordering::Relaxed);
    }
    // port: tsc/internal/lsp/server.go:Server.recover
    pub fn run<T>(
        &self,
        method: &str,
        work: impl FnOnce() -> Result<T, lsp::ResponseError>,
    ) -> Result<T, lsp::ResponseError> {
        match catch_unwind(AssertUnwindSafe(work)) {
            Ok(result) => result,
            Err(payload) => {
                let reason = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("non-string panic");
                let message = format!("panic handling request {method}: {reason}");
                let stack = std::backtrace::Backtrace::force_capture().to_string();
                self.logger
                    .send(lsp::MessageType::ERROR, format!("{message}\n{stack}"));
                if self.telemetry.load(Ordering::Relaxed) {
                    // The Go sanitizer deliberately redacts unknown frames.
                    // Rust backtraces therefore remain empty on the wire rather
                    // than disclosing paths or user names through telemetry.
                    let _ = client::notify(
                        self.client.as_ref(),
                        "telemetry/event",
                        &lsp::RequestFailureTelemetryEvent {
                            properties: Some(Box::new(lsp::RequestFailureTelemetryProperties {
                                error_code: "InternalError".into(),
                                request_method: method.replace('/', "."),
                                stack: crate::stack_sanitizer::sanitize_stack_trace(&stack),
                            })),
                            ..Default::default()
                        },
                    );
                }
                Err(crate::error(-32603, message))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panic_is_request_local_and_telemetry_is_opt_in_and_redacted() {
        let (send, receive) = std::sync::mpsc::channel();
        let context = tsr_ipc::Context::background();
        let client = crate::rpc_client::RpcClient::new(
            context.clone(),
            Arc::new(move |value| {
                send.send(value).unwrap();
                Ok(())
            }),
        );
        let logger = Arc::new(Logger::new(
            client.clone(),
            context,
            Box::new(std::io::sink()),
        ));
        logger.initialize_started();
        let recovery = Recovery::new(client, logger);
        for enabled in [false, true] {
            recovery.set_telemetry(enabled);
            let result: Result<(), _> =
                recovery.run("textDocument/diagnostic", || panic!("private fixture text"));
            assert_eq!(result.unwrap_err().code, -32603);
            assert_eq!(recovery.run("next", || Ok(42)).unwrap(), 42);
            let log = receive.recv().unwrap();
            assert!(String::from_utf8(log.0)
                .unwrap()
                .contains("window/logMessage"));
            if enabled {
                let mut message = lsp::Message::default();
                tsr_json::unmarshal(
                    &receive.recv().unwrap().0,
                    &mut message,
                    tsr_json::Options::default(),
                )
                .unwrap();
                assert_eq!(message.method, "telemetry/event");
                let mut event = lsp::RequestFailureTelemetryEvent::default();
                tsr_json::unmarshal(
                    &message.params.unwrap().0,
                    &mut event,
                    tsr_json::Options::default(),
                )
                .unwrap();
                let properties = event.properties.unwrap();
                assert_eq!(properties.error_code, "InternalError");
                assert_eq!(properties.request_method, "textDocument.diagnostic");
                assert!(properties.stack.is_empty());
            }
            assert!(receive.try_recv().is_err());
        }
    }
}
