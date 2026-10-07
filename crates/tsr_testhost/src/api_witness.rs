//! `phase5_testserver --api`: the production `tsr_api` session behind the
//! pin's `api` flags, plus one test-only control for the Phase 6 panic
//! witness (`testhost/faultNextCheckerOperation`, which arms a panic in the
//! next checker operation of a named snapshot). Nothing here ships in
//! `tsrust`.
use std::sync::Arc;
use tsr_api::callbackfs::CallbackFs;
use tsr_api::server::{serve, Session};
use tsr_api::session::ApiSession;
use tsr_ipc::{Context, HandlerResult, PipeTransport, Response, StdioTransport, Stream, Transport};
use tsr_jsstring::JsString;

/// The method the witness script sends; `{"snapshot": n}` arms the fault.
pub const FAULT_METHOD: &str = "testhost/faultNextCheckerOperation";

#[derive(Default)]
struct Flags {
    cwd: Option<String>,
    pipe: Option<String>,
    callbacks: Vec<String>,
    r#async: bool,
    timing: bool,
}

/// The pin's `api` flags the clients send (`getAPIProcessArgs`) plus `-pipe`
/// and `-callbacks`: one or two dashes, with or without `=`.
fn parse(args: &[String]) -> Result<Flags, String> {
    let mut flags = Flags::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let name = arg.trim_start_matches('-');
        let (name, inline) = name.split_once('=').map_or((name, None), |(name, value)| {
            (name, Some(value.to_string()))
        });
        let mut value = || -> Result<String, String> {
            inline
                .clone()
                .or_else(|| iter.next().cloned())
                .ok_or_else(|| format!("flag needs an argument: -{name}"))
        };
        match name {
            "api" | "runExternalCode" => {}
            "async" => flags.r#async = true,
            "timing" => flags.timing = true,
            "cwd" => flags.cwd = Some(value()?),
            "pipe" => flags.pipe = Some(value()?),
            "callbacks" => flags.callbacks = value()?.split(',').map(str::to_owned).collect(),
            other => return Err(format!("flag provided but not defined: -{other}")),
        }
    }
    Ok(flags)
}

/// The production session with the fault control in front of it.
struct FaultControlled(Arc<ApiSession>);
impl Session for FaultControlled {
    fn id(&self) -> &str {
        self.0.id()
    }
    fn handle_request(&self, ctx: &Context, method: &str, params: &[u8]) -> HandlerResult {
        if method == FAULT_METHOD {
            let snapshot = fault_snapshot(params).ok_or_else(|| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "faultNextCheckerOperation needs {\"snapshot\": n}",
                )) as tsr_ipc::HandlerError
            })?;
            self.0.arm_fault(tsr_api::proto::SnapshotId(snapshot));
            return Ok(Some(Response::json(true)));
        }
        self.0.handle_request(ctx, method, params)
    }
    fn set_binary_responses(&self, enabled: bool) {
        self.0.set_binary_responses(enabled);
    }
    fn close(&self) {
        self.0.close();
    }
}

fn fault_snapshot(params: &[u8]) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_slice(params).ok()?;
    value.get("snapshot")?.as_u64()
}

/// Serves one connection like `tsrust --api`, returning the process exit code.
pub fn run(args: &[String]) -> i32 {
    let flags = match parse(args) {
        Ok(flags) => flags,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    let cwd = match flags.cwd {
        Some(cwd) => JsString::from_bytes(cwd.as_bytes()),
        None => match std::env::current_dir() {
            Ok(cwd) => JsString::from_bytes(cwd.to_string_lossy().as_bytes()),
            Err(error) => {
                eprintln!("{error}");
                return 1;
            }
        },
    };
    let base: Arc<dyn tsr_vfs::FileSystem> =
        Arc::new(tsr_bundled::BundledFs::new(tsr_vfs::os::shared_fs()));
    let (fs, callbacks): (Arc<dyn tsr_vfs::FileSystem>, Option<Arc<CallbackFs>>) =
        if flags.callbacks.is_empty() {
            (base, None)
        } else {
            match CallbackFs::new(base, &flags.callbacks) {
                Ok(callbacks) => {
                    let callbacks = Arc::new(callbacks);
                    (callbacks.clone(), Some(callbacks))
                }
                Err(error) => {
                    eprintln!("{error}");
                    return 1;
                }
            }
        };
    let ctx = Context::background();
    let session = ApiSession::standalone(
        tsr_project::session::SessionOptions {
            current_directory: cwd,
            default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
            position_encoding: tsr_jsstring::PositionEncoding::Utf8,
            background_context: ctx.clone(),
            ..Default::default()
        },
        fs,
    );
    let session: Arc<dyn Session> = Arc::new(FaultControlled(session));
    let mut transport: Box<dyn Transport> = match &flags.pipe {
        Some(path) => match PipeTransport::new(path) {
            Ok(transport) => Box::new(transport),
            Err(error) => {
                eprintln!("failed to create pipe transport: {error}");
                return 1;
            }
        },
        None => Box::new(StdioTransport::new()),
    };
    let stream: Stream = match transport.accept() {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("failed to accept connection: {error}");
            return 1;
        }
    };
    let result = serve(
        stream,
        session.clone(),
        callbacks,
        flags.r#async,
        flags.timing,
        &ctx,
    );
    session.close();
    let _ = transport.close();
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}
