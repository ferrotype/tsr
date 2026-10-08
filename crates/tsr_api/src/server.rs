//! The API server behind `tsrust --api`: one connection over stdio or a
//! Unix-domain socket, speaking the synchronous msgpack protocol or the
//! asynchronous JSON-RPC protocol, with the client's file-system callbacks
//! wired into the file system the session reads. Until A2 the session is a
//! skeleton: `initialize`, `ping` and `echo` answer; every other method
//! fails by name.
//! port: tsc/internal/api/server.go
use crate::callbackfs::CallbackFs;
use crate::proto::{InitializeResponse, Method};
use crate::protocol_msgpack::MessagePackProtocol;
use std::sync::Arc;
use tsr_ipc::{
    AsyncConn, Conn, Context, Error, Handler, HandlerError, HandlerResult, PipeTransport, Response,
    StdioTransport, Stream, Transport,
};
use tsr_jsstring::JsString;
use tsr_vfs::FileSystem;

/// The pin's `StdioServerOptions` of tsc/internal/api/server.go.
pub struct StdioServerOptions {
    pub cwd: JsString,
    pub default_library_path: JsString,
    /// The bundled file system over the disk; callbacks wrap it.
    pub fs: Arc<dyn FileSystem>,
    /// Listen on a Unix-domain socket instead of stdio.
    pub pipe_path: Option<String>,
    /// The file-system operations delegated to the client.
    pub callbacks: Vec<String>,
    /// JSON-RPC with the asynchronous connection instead of msgpack.
    pub async_mode: bool,
    pub collect_timing: bool,
    pub run_external_code: bool,
    pub mapper_spawner: Option<Arc<dyn tsr_contentmapper::Spawner>>,
    /// A test host's wrapper around the production session, such as the
    /// Phase 6 panic witness's fault control; `tsrust` sets none, so the
    /// witness serves on the path `tsrust --api` takes.
    pub session_hook: Option<SessionHook>,
}

/// Wraps the session a `StdioServer` built before it serves it.
pub type SessionHook =
    Arc<dyn Fn(Arc<crate::session::ApiSession>) -> Arc<dyn Session> + Send + Sync>;

pub struct StdioServer {
    options: StdioServerOptions,
}

/// The session's file system and, when callbacks are enabled, the wrapper
/// that needs the connection.
type SessionFs = (Arc<dyn FileSystem>, Option<Arc<CallbackFs>>);

/// The session's request surface the connections dispatch to. A2 replaces
/// the skeleton with the real session; this trait keeps the connection code
/// independent of that change.
pub trait Session: Send + Sync {
    /// `api-session-<n>` from one process-wide counter.
    /// port: tsc/internal/api/session.go:Session.ID
    fn id(&self) -> &str;
    fn handle_request(&self, ctx: &Context, method: &str, params: &[u8]) -> HandlerResult;
    /// The msgpack protocol asks for raw bytes where JSON-RPC takes base64.
    fn set_binary_responses(&self, _enabled: bool) {}
    fn close(&self) {}
}

/// Until the session exists, every method fails by name; `initialize`,
/// `ping` and `echo` answer.
pub struct Skeleton {
    id: String,
    cwd: JsString,
    case_sensitive: bool,
    binary: std::sync::atomic::AtomicBool,
}

static SESSION_ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The next session id; the pin's `sessionIDCounter` formatted by `newSession`.
/// port: tsc/internal/api/session.go:newSession
#[must_use]
pub fn next_session_id() -> String {
    let id = SESSION_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    format!("api-session-{id}")
}

impl Skeleton {
    #[must_use]
    pub fn new(cwd: JsString, case_sensitive: bool) -> Self {
        Self {
            id: next_session_id(),
            cwd,
            case_sensitive,
            binary: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

/// A known method the port has not reached yet fails as `not implemented:
/// <method>`; an unknown one fails as the pin's payload decoder does, under
/// the session's `api: invalid request` prefix.
/// port: tsc/internal/api/proto.go:unmarshalPayload
pub fn unsupported(method: &str) -> HandlerError {
    match Method::from_wire(method) {
        Some(_) => format!("not implemented: {method}").into(),
        None => format!(
            "api: invalid request: unknown API method {}",
            tsr_jsstring::go_quote(method.as_bytes())
        )
        .into(),
    }
}

impl Session for Skeleton {
    fn id(&self) -> &str {
        &self.id
    }
    fn handle_request(&self, _ctx: &Context, method: &str, params: &[u8]) -> HandlerResult {
        match method {
            "ping" => Ok(Some(Response::json("pong".to_string()))),
            "echo" => {
                if self.binary.load(std::sync::atomic::Ordering::Relaxed) {
                    Ok(Some(Response::binary(params.to_vec())))
                } else {
                    Ok(Some(Response::json(tsr_json::RawValue(params.to_vec()))))
                }
            }
            "initialize" => Ok(Some(Response::json(InitializeResponse {
                use_case_sensitive_file_names: self.case_sensitive,
                current_directory: String::from_utf8_lossy(self.cwd.as_bytes()).into_owned(),
            }))),
            _ => Err(unsupported(method)),
        }
    }
    fn set_binary_responses(&self, enabled: bool) {
        self.binary
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The connection handler: requests go to the session, notifications are
/// ignored, as the pin's `HandleNotification` is.
pub struct SessionHandler(pub Arc<dyn Session>);
impl Handler for SessionHandler {
    fn handle_request(&self, ctx: &Context, method: &str, params: &[u8]) -> HandlerResult {
        self.0.handle_request(ctx, method, params)
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

    /// The file system the session reads: the client's callbacks over the
    /// base when any are enabled.
    fn file_system(&self) -> Result<SessionFs, Error> {
        if self.options.callbacks.is_empty() {
            return Ok((self.options.fs.clone(), None));
        }
        let callbacks = Arc::new(
            CallbackFs::new(self.options.fs.clone(), &self.options.callbacks)
                .map_err(Error::Message)?,
        );
        Ok((callbacks.clone(), Some(callbacks)))
    }

    /// Serves one connection until it closes.
    /// port: tsc/internal/api/server.go:StdioServer.Run
    pub fn run(&self, ctx: &Context) -> Result<(), Error> {
        let (fs, callbacks) = self.file_system()?;
        // The pin's project session options for a standalone session: UTF-8
        // positions, logging off, the command's external-code and mapper
        // settings.
        let session = crate::session::ApiSession::standalone(
            tsr_project::session::SessionOptions {
                current_directory: self.options.cwd.clone(),
                default_library_path: self.options.default_library_path.clone(),
                position_encoding: tsr_jsstring::PositionEncoding::Utf8,
                run_external_code: self.options.run_external_code,
                mapper_spawner: self.options.mapper_spawner.clone(),
                background_context: ctx.clone(),
                ..Default::default()
            },
            fs,
        );
        let session: Arc<dyn Session> = match &self.options.session_hook {
            Some(hook) => hook(session),
            None => session,
        };
        let mut transport: Box<dyn Transport> = match &self.options.pipe_path {
            Some(path) => Box::new(PipeTransport::new(path).map_err(|error| {
                Error::Message(format!("failed to create pipe transport: {error}"))
            })?),
            None => Box::new(StdioTransport::new()),
        };
        let stream = transport
            .accept()
            .map_err(|error| Error::Message(format!("failed to accept connection: {error}")))?;
        let result = serve(
            stream,
            session.clone(),
            callbacks,
            self.options.async_mode,
            self.options.collect_timing,
            ctx,
        );
        session.close();
        let _ = transport.close();
        result
    }
}

/// Runs one accepted connection to its end.
pub fn serve(
    stream: Stream,
    session: Arc<dyn Session>,
    callbacks: Option<Arc<CallbackFs>>,
    async_mode: bool,
    collect_timing: bool,
    ctx: &Context,
) -> Result<(), Error> {
    session.set_binary_responses(!async_mode);
    let handler = Arc::new(SessionHandler(session));
    let conn: Arc<dyn Conn> = if async_mode {
        let conn = AsyncConn::new(stream, handler);
        conn.set_collect_timing(collect_timing);
        Arc::new(conn)
    } else {
        let protocol = MessagePackProtocol::new(stream.reader, stream.writer);
        let conn = tsr_ipc::SyncConn::with_closer(Some(stream.closer), protocol, handler);
        conn.set_collect_timing(collect_timing);
        Arc::new(conn)
    };
    if let Some(callbacks) = callbacks {
        callbacks.set_connection(ctx.clone(), &conn);
    }
    conn.run(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    const REQUEST: u8 = 1;
    const CALL_RESPONSE: u8 = 2;
    const RESPONSE: u8 = 4;
    const ERROR: u8 = 5;
    const CALL: u8 = 6;

    /// An independent reader of the pin's tuple framing: none of the server's
    /// protocol code is reused, so an interleaved or malformed frame fails here.
    fn read_tuple(stream: &mut impl Read) -> (u8, String, Vec<u8>) {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).unwrap();
        assert_eq!(byte[0], 0x93, "fixed three-element array");
        stream.read_exact(&mut byte).unwrap();
        let kind = if byte[0] == 0xcc {
            stream.read_exact(&mut byte).unwrap();
            byte[0]
        } else {
            assert!(byte[0] < 0x80, "positive fixint");
            byte[0]
        };
        let length = read_length(stream);
        let mut method = vec![0; length];
        stream.read_exact(&mut method).unwrap();
        let length = read_length(stream);
        let mut payload = vec![0; length];
        stream.read_exact(&mut payload).unwrap();
        (kind, String::from_utf8(method).unwrap(), payload)
    }

    /// A `bin8`/`bin16`/`bin32` length; the pin frames the method and the
    /// payload as binary.
    fn read_length(stream: &mut impl Read) -> usize {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).unwrap();
        match byte[0] {
            0xc4 => {
                stream.read_exact(&mut byte).unwrap();
                usize::from(byte[0])
            }
            0xc5 => {
                let mut two = [0u8; 2];
                stream.read_exact(&mut two).unwrap();
                usize::from(u16::from_be_bytes(two))
            }
            0xc6 => {
                let mut four = [0u8; 4];
                stream.read_exact(&mut four).unwrap();
                usize::try_from(u32::from_be_bytes(four)).unwrap()
            }
            other => panic!("binary marker {other:#x}"),
        }
    }

    fn write_tuple(stream: &mut impl Write, kind: u8, method: &str, payload: &[u8]) {
        let mut frame = vec![0x93, kind];
        frame.push(0xc4);
        frame.push(u8::try_from(method.len()).unwrap());
        frame.extend_from_slice(method.as_bytes());
        if payload.len() < 256 {
            frame.push(0xc4);
            frame.push(u8::try_from(payload.len()).unwrap());
        } else {
            frame.push(0xc5);
            frame.extend_from_slice(&u16::try_from(payload.len()).unwrap().to_be_bytes());
        }
        frame.extend_from_slice(payload);
        stream.write_all(&frame).unwrap();
        stream.flush().unwrap();
    }

    fn pair() -> (UnixStream, Stream) {
        let (client, server) = UnixStream::pair().unwrap();
        (client, Stream::from_unix(server).unwrap())
    }

    struct Ignore;
    impl Handler for Ignore {
        fn handle_request(&self, _: &Context, _: &str, _: &[u8]) -> HandlerResult {
            Ok(None)
        }
        fn handle_notification(&self, _: &Context, _: &str, _: &[u8]) -> Result<(), HandlerError> {
            Ok(())
        }
    }

    #[test]
    fn the_sync_protocol_answers_the_skeleton_methods_and_ends_on_eof() {
        let (mut client, stream) = pair();
        let session: Arc<dyn Session> = Arc::new(Skeleton::new(
            JsString::from_bytes(b"/work".as_slice()),
            true,
        ));
        let ctx = Context::background();
        let server = std::thread::spawn(move || serve(stream, session, None, false, true, &ctx));
        write_tuple(&mut client, REQUEST, "initialize", b"");
        let (kind, method, payload) = read_tuple(&mut client);
        assert_eq!((kind, method.as_str()), (RESPONSE, "initialize"));
        assert_eq!(
            payload,
            br#"{"useCaseSensitiveFileNames":true,"currentDirectory":"/work"}"#
        );
        write_tuple(&mut client, REQUEST, "ping", b"");
        assert_eq!(
            read_tuple(&mut client),
            (RESPONSE, "ping".into(), b"\"pong\"".to_vec())
        );
        let bytes: Vec<u8> = (0..=255u8).chain(0..=255u8).collect();
        write_tuple(&mut client, REQUEST, "echo", &bytes);
        assert_eq!(
            read_tuple(&mut client),
            (RESPONSE, "echo".into(), bytes),
            "binary responses are verbatim"
        );
        write_tuple(&mut client, REQUEST, "getSourceFile", b"{}");
        let (kind, method, payload) = read_tuple(&mut client);
        assert_eq!((kind, method.as_str()), (ERROR, "getSourceFile"));
        assert_eq!(
            String::from_utf8(payload).unwrap(),
            "not implemented: getSourceFile"
        );
        write_tuple(&mut client, REQUEST, "nope", b"");
        let (kind, _, payload) = read_tuple(&mut client);
        assert_eq!(kind, ERROR);
        assert_eq!(
            String::from_utf8(payload).unwrap(),
            "api: invalid request: unknown API method \"nope\""
        );
        write_tuple(&mut client, REQUEST, "getServerTiming", b"");
        let (kind, method, payload) = read_tuple(&mut client);
        assert_eq!((kind, method.as_str()), (RESPONSE, "getServerTiming"));
        let text = String::from_utf8(payload).unwrap();
        assert!(
            text.starts_with(r#"{"enabled":true,"totals":{"requestCount":5,"#),
            "every dispatched request is timed, the timing requests are not: {text}"
        );
        drop(client);
        server.join().unwrap().unwrap();
    }

    /// The plan's serialized-callbacks witness: eight threads read files
    /// through the callback file system while one request is in flight; the
    /// client sees one complete frame at a time and every reply reaches the
    /// thread that asked.
    #[test]
    fn callbacks_from_eight_threads_are_serialized_on_the_wire() {
        struct Fanout(Arc<CallbackFs>);
        impl Handler for Fanout {
            fn handle_request(&self, _: &Context, method: &str, _: &[u8]) -> HandlerResult {
                assert_eq!(method, "fanout");
                let contents: Vec<String> = std::thread::scope(|scope| {
                    let workers: Vec<_> = (0..8)
                        .map(|index| {
                            let fs = self.0.clone();
                            scope.spawn(move || {
                                let path = format!("/f{index}.ts");
                                let content = fs.read_file(path.as_bytes()).unwrap().unwrap();
                                String::from_utf8(content.raw.to_vec()).unwrap()
                            })
                        })
                        .collect();
                    workers
                        .into_iter()
                        .map(|worker| worker.join().unwrap())
                        .collect()
                });
                Ok(Some(Response::json(contents)))
            }
            fn handle_notification(
                &self,
                _: &Context,
                _: &str,
                _: &[u8],
            ) -> Result<(), HandlerError> {
                Ok(())
            }
        }
        let (mut client, stream) = pair();
        let base = Arc::new(tsr_vfs::MemoryBuilder::new(b"/", true).finish());
        let fs = Arc::new(CallbackFs::new(base, &["readFile".to_string()]).unwrap());
        let protocol = MessagePackProtocol::new(stream.reader, stream.writer);
        let conn: Arc<dyn Conn> = Arc::new(tsr_ipc::SyncConn::with_closer(
            Some(stream.closer),
            protocol,
            Arc::new(Fanout(fs.clone())),
        ));
        let ctx = Context::background();
        fs.set_connection(ctx.clone(), &conn);
        let server = {
            let conn = conn.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || conn.run(&ctx))
        };
        write_tuple(&mut client, REQUEST, "fanout", b"");
        let mut seen = Vec::new();
        for _ in 0..8 {
            let (kind, method, payload) = read_tuple(&mut client);
            assert_eq!((kind, method.as_str()), (CALL, "readFile"));
            let path: String = serde_json::from_slice(&payload).unwrap();
            let reply = format!(r#"{{"content":"content of {path}"}}"#);
            seen.push(path);
            write_tuple(&mut client, CALL_RESPONSE, "readFile", reply.as_bytes());
        }
        let (kind, method, payload) = read_tuple(&mut client);
        assert_eq!((kind, method.as_str()), (RESPONSE, "fanout"));
        let contents: Vec<String> = serde_json::from_slice(&payload).unwrap();
        for (index, content) in contents.iter().enumerate() {
            assert_eq!(
                content,
                &format!("content of /f{index}.ts"),
                "replies reach their callers"
            );
        }
        seen.sort();
        assert_eq!(
            seen,
            (0..8)
                .map(|index| format!("/f{index}.ts"))
                .collect::<Vec<_>>()
        );
        drop(client);
        server.join().unwrap().unwrap();
    }

    /// A finished callback-enabled connection leaves no cycle: the callback
    /// file system holds the connection weakly, so when `serve` returns the
    /// connection, its handler and the session are dropped even while the
    /// file system (owned by the project session in production) lives on.
    #[test]
    fn a_served_connection_is_dropped_with_its_session_when_callbacks_are_enabled() {
        let (mut client, stream) = pair();
        let base = Arc::new(tsr_vfs::MemoryBuilder::new(b"/", true).finish());
        let fs = Arc::new(CallbackFs::new(base, &["readFile".to_string()]).unwrap());
        let session: Arc<dyn Session> =
            Arc::new(Skeleton::new(JsString::from_bytes(b"/".as_slice()), true));
        let weak_session = Arc::downgrade(&session);
        let server = {
            let fs = fs.clone();
            std::thread::spawn(move || {
                serve(
                    stream,
                    session,
                    Some(fs),
                    false,
                    false,
                    &Context::background(),
                )
            })
        };
        write_tuple(&mut client, REQUEST, "ping", b"");
        let (kind, method, payload) = read_tuple(&mut client);
        assert_eq!(
            (kind, method.as_str(), payload.as_slice()),
            (RESPONSE, "ping", &b"\"pong\""[..])
        );
        drop(client);
        server.join().unwrap().unwrap();
        assert!(
            weak_session.upgrade().is_none(),
            "the session is dropped once the connection ends while the file system lives"
        );
        drop(fs);
    }

    /// The plan's disconnect witness: the client closes while its request is
    /// being handled; the connection ends and the socket file is unbound.
    #[test]
    fn a_client_that_disconnects_mid_request_ends_the_connection_and_unbinds_the_socket() {
        struct Slow;
        impl Handler for Slow {
            fn handle_request(&self, _: &Context, _: &str, _: &[u8]) -> HandlerResult {
                std::thread::sleep(Duration::from_millis(200));
                Ok(Some(Response::json("late".to_string())))
            }
            fn handle_notification(
                &self,
                _: &Context,
                _: &str,
                _: &[u8],
            ) -> Result<(), HandlerError> {
                Ok(())
            }
        }
        let path =
            std::env::temp_dir().join(format!("tsr-api-disconnect-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut transport = PipeTransport::new(&path).unwrap();
        assert!(path.exists(), "the listener binds the socket file");
        let server = std::thread::spawn(move || {
            let stream = transport.accept().unwrap();
            let protocol = MessagePackProtocol::new(stream.reader, stream.writer);
            let conn =
                tsr_ipc::SyncConn::with_closer(Some(stream.closer), protocol, Arc::new(Slow));
            let result = conn.run(&Context::background());
            transport.close().unwrap();
            result
        });
        let mut client = UnixStream::connect(&path).unwrap();
        write_tuple(&mut client, REQUEST, "slow", b"");
        drop(client);
        // The response write fails or the next read sees the end: either way
        // the connection ends instead of waiting.
        let _ = server.join().unwrap();
        assert!(
            !path.exists(),
            "closing the transport removes the socket file"
        );
        let _ = Ignore;
    }

    #[test]
    fn stale_socket_files_are_replaced_and_the_transport_serves_one_connection() {
        let path = std::env::temp_dir().join(format!("tsr-api-stale-{}.sock", std::process::id()));
        std::fs::write(&path, b"stale").unwrap();
        let mut transport = PipeTransport::new(&path).unwrap();
        assert_eq!(transport.path(), path);
        let server = std::thread::spawn(move || {
            let stream = transport.accept().unwrap();
            let protocol = MessagePackProtocol::new(stream.reader, stream.writer);
            let conn =
                tsr_ipc::SyncConn::with_closer(Some(stream.closer), protocol, Arc::new(Ignore));
            let result = conn.run(&Context::background());
            transport.close().unwrap();
            result
        });
        let mut client = UnixStream::connect(&path).unwrap();
        write_tuple(&mut client, REQUEST, "anything", b"");
        let (kind, method, payload) = read_tuple(&mut client);
        assert_eq!(
            (kind, method.as_str(), payload.as_slice()),
            (RESPONSE, "anything", &b"null"[..])
        );
        drop(client);
        server.join().unwrap().unwrap();
        assert!(!path.exists());
    }
}
