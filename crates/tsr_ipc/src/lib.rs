//! Bidirectional JSON-RPC connections: the handler and connection contracts,
//! the JSON-RPC protocol, the asynchronous connection and its timing, and the
//! in-memory transport in-process peers connect through.
mod conn;
mod conn_async;
mod context;
mod protocol;
mod timing;
mod transport;

pub use conn::{unmarshal_params, Conn, Error, Handler, HandlerError, HandlerResult};
pub use conn_async::AsyncConn;
pub use context::{AfterFuncStop, Context, ContextError};
pub use protocol::{JsonRpcProtocol, Protocol};
pub use timing::{
    server_timing_snapshot, ServerTimingInfo, TimingCollector, METHOD_GET_SERVER_TIMING,
    METHOD_RESET_SERVER_TIMING,
};
pub use transport::{pipe, stdio, Closer, Stream};

/// The pinned `ipc.Message`, a raw JSON-RPC message.
pub type Message = tsr_jsonrpc::Message;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tsr_json::RawValue;

    struct Echo;

    impl Handler for Echo {
        fn handle_request(&self, _ctx: &Context, method: &str, params: &[u8]) -> HandlerResult {
            match method {
                "echo" => Ok(Some(Box::new(RawValue(params.to_vec())))),
                "nothing" => Ok(None),
                "panic" => panic!("handler exploded"),
                _ => Err(format!("unexpected method {method:?}").into()),
            }
        }
        fn handle_notification(&self, _: &Context, _: &str, _: &[u8]) -> Result<(), HandlerError> {
            Ok(())
        }
    }

    struct Reject;

    impl Handler for Reject {
        fn handle_request(&self, _: &Context, method: &str, _: &[u8]) -> HandlerResult {
            Err(format!("unexpected request: {method}").into())
        }
        fn handle_notification(&self, _: &Context, _: &str, _: &[u8]) -> Result<(), HandlerError> {
            Ok(())
        }
    }

    type Connected = (
        AsyncConn,
        AsyncConn,
        std::thread::JoinHandle<Result<(), Error>>,
        Arc<dyn Closer>,
    );

    fn connected() -> Connected {
        let (client_end, server_end) = pipe();
        let closer = server_end.closer.clone();
        let client = AsyncConn::new(client_end, Arc::new(Reject));
        let server = AsyncConn::new(server_end, Arc::new(Echo));
        server.set_collect_timing(true);
        let running = server.clone();
        std::thread::spawn(move || running.run(&Context::background()));
        let reading = client.clone();
        let client_run = std::thread::spawn(move || reading.run(&Context::background()));
        (client, server, client_run, closer)
    }

    #[test]
    fn calls_get_results_errors_and_panics_as_responses() {
        let (client, _server, client_run, closer) = connected();
        let ctx = Context::background();
        let params = RawValue(br#"{"a":[1,2]}"#.to_vec());
        assert_eq!(
            client.call(&ctx, "echo", &params).unwrap().0,
            br#"{"a":[1,2]}"#
        );
        assert_eq!(client.call(&ctx, "nothing", &params).unwrap().0, b"null");
        let error = client.call(&ctx, "missing", &params).unwrap_err();
        assert_eq!(
            error.to_string(),
            "ipc: remote error [-32603]: unexpected method \"missing\""
        );
        let panic = client.call(&ctx, "panic", &params).unwrap_err().to_string();
        assert!(
            panic.starts_with("ipc: remote error [-32603]: panic: handler exploded\n"),
            "{panic}"
        );
        let timing = client
            .call(&ctx, METHOD_GET_SERVER_TIMING, &params)
            .unwrap();
        let text = String::from_utf8(timing.0).unwrap();
        assert!(
            text.starts_with(r#"{"enabled":true,"totals":{"requestCount":3,"#),
            "{text}"
        );
        client.notify(&ctx, "log", &params).unwrap();
        // The server's end closing ends the client's stream cleanly.
        closer.close().unwrap();
        client_run.join().unwrap().unwrap();
        let closed = client.call(&ctx, "echo", &params).unwrap_err();
        assert_eq!(closed.to_string(), "ipc: connection closed");
    }

    #[test]
    fn a_canceled_or_expired_context_ends_a_waiting_call() {
        let (client_end, _server_end) = pipe();
        let client = AsyncConn::new(client_end, Arc::new(Reject));
        let ctx = Context::background().with_timeout(std::time::Duration::from_millis(20));
        let params = RawValue(b"{}".to_vec());
        let error = client.call(&ctx, "echo", &params).unwrap_err();
        assert_eq!(error.context_error(), Some(ContextError::DeadlineExceeded));
        let canceled = Context::background().with_cancel();
        canceled.cancel();
        assert_eq!(
            client
                .call(&canceled, "echo", &params)
                .unwrap_err()
                .to_string(),
            "context canceled"
        );
    }

    #[test]
    fn params_decode_only_when_present() {
        assert_eq!(unmarshal_params::<String>(b"").unwrap(), None);
        assert_eq!(
            unmarshal_params::<String>(br#""x""#).unwrap(),
            Some("x".into())
        );
        assert!(unmarshal_params::<String>(b"1").is_err());
    }
}
