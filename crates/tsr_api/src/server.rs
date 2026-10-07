//! The API server behind `tsrust --api`: one connection over stdio (or, from
//! A1, a Unix-domain socket), speaking the synchronous msgpack protocol or the
//! asynchronous JSON-RPC protocol. This first slice answers every request
//! with the pin's error form naming the method; the session arrives with A2.
//! port: tsc/internal/api/server.go
use crate::proto::Method;
use crate::protocol_msgpack::MessagePackProtocol;
use std::sync::Arc;
use tsr_ipc::{AsyncConn, Conn, Context, Error, Handler, HandlerError, HandlerResult, Protocol};
use tsr_jsonrpc::{ResponseError, CODE_INTERNAL_ERROR};
use tsr_jsstring::JsString;

/// The pin's `StdioServerOptions` of tsc/internal/api/server.go.
pub struct StdioServerOptions {
    pub cwd: JsString,
    pub default_library_path: JsString,
    /// Listen on a Unix-domain socket instead of stdio.
    pub pipe_path: Option<String>,
    /// The file-system operations delegated to the client.
    pub callbacks: Vec<String>,
    /// JSON-RPC with the asynchronous connection instead of msgpack.
    pub async_mode: bool,
    pub collect_timing: bool,
    pub run_external_code: bool,
    pub mapper_spawner: Option<Arc<dyn tsr_contentmapper::Spawner>>,
}

pub struct StdioServer {
    options: StdioServerOptions,
}

/// Until the session exists, every method fails by name; `ping` answers.
struct Skeleton;

fn unsupported(method: &str) -> HandlerError {
    match Method::from_wire(method) {
        Some(_) => format!("not implemented: {method}").into(),
        None => format!("unknown method: {method}").into(),
    }
}

impl Handler for Skeleton {
    fn handle_request(&self, _ctx: &Context, method: &str, params: &[u8]) -> HandlerResult {
        match method {
            "ping" => Ok(Some(Box::new("pong".to_string()))),
            "echo" => Ok(Some(Box::new(tsr_json::RawValue(params.to_vec())))),
            _ => Err(unsupported(method)),
        }
    }
    fn handle_notification(
        &self,
        _ctx: &Context,
        _method: &str,
        _params: &[u8],
    ) -> Result<(), HandlerError> {
        Ok(())
    }
}

impl StdioServer {
    /// port: tsc/internal/api/server.go:NewStdioServer
    #[must_use]
    pub fn new(options: StdioServerOptions) -> Self {
        assert!(
            !options.cwd.is_empty(),
            "StdioServerOptions.Cwd is required"
        );
        Self { options }
    }

    /// Serves one connection until it closes.
    /// port: tsc/internal/api/server.go:StdioServer.Run
    pub fn run(&self, ctx: &Context) -> Result<(), Error> {
        if self.options.pipe_path.is_some() {
            return Err(Error::Message(
                "the pipe transport is not implemented yet (Phase 6 A1)".into(),
            ));
        }
        let stream = tsr_ipc::stdio();
        if self.options.async_mode {
            let conn = AsyncConn::new(stream, Arc::new(Skeleton));
            conn.set_collect_timing(self.options.collect_timing);
            return conn.run(ctx);
        }
        let protocol = MessagePackProtocol::new(stream.reader, stream.writer);
        let handler = Skeleton;
        loop {
            if let Some(error) = ctx.err() {
                return Err(error.into());
            }
            let message = match protocol.read_message() {
                Ok(message) => message,
                Err(error) if error.is_eof() => return Ok(()),
                Err(error) => return Err(error),
            };
            if !message.is_request() {
                continue;
            }
            let params = message.params.as_ref().map_or(&[][..], |p| p.0.as_slice());
            match handler.handle_request(ctx, &message.method, params) {
                Ok(result) => protocol.write_response(
                    message.id.as_ref(),
                    result
                        .as_deref()
                        .map(|value| value as &dyn tsr_json::Encode),
                )?,
                Err(error) => protocol.write_error(
                    message.id.as_ref(),
                    &ResponseError {
                        code: CODE_INTERNAL_ERROR,
                        message: error.to_string(),
                        data: None,
                    },
                )?,
            }
        }
    }
}
