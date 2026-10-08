//! API sessions hosted by the LSP server: `custom/initializeAPISession`
//! listens on a Unix-domain socket, answers with the socket path and the
//! session id, and serves the one connection that arrives on a background
//! thread with the asynchronous JSON-RPC connection. The session shares the
//! LSP server's project session; it is removed from the map when its
//! connection ends, as the pin's is.
use crate::logger::Logger;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use tsr_ipc::{Context, PipeTransport, Transport};
use tsr_lsproto as lsp;

pub type ApiSessions = Arc<Mutex<HashMap<String, Arc<dyn tsr_api::server::Session>>>>;

/// `tsgo-api-<time hex>-<random hex>` under the temporary directory.
/// port: tsc/internal/lsp/server.go:Server.generateAPIPipePath
fn generate_pipe_path() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let random = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    tsr_ipc::generate_pipe_path(&format!("tsgo-api-{now:x}-{random:x}"))
        .to_string_lossy()
        .into_owned()
}

/// port: tsc/internal/lsp/server.go:Server.removeAPISession
fn remove(sessions: &ApiSessions, id: &str) {
    if let Ok(mut map) = sessions.lock() {
        map.remove(id);
    }
}

/// port: tsc/internal/lsp/server.go:Server.handleInitializeAPISession
pub fn initialize(
    sessions: &ApiSessions,
    project: &Arc<tsr_project::session::Session>,
    params: &lsp::InitializeAPISessionParams,
    logger: &Arc<Logger>,
    background: &Context,
) -> Result<lsp::InitializeAPISessionResult, lsp::ResponseError> {
    let mut map = sessions
        .lock()
        .map_err(|_| crate::error(-32603, "API sessions poisoned"))?;
    // A2 replaces the skeleton with the session over the project session's
    // snapshot host; the connection code does not change.
    let session: Arc<dyn tsr_api::server::Session> = Arc::new(tsr_api::server::Skeleton::new(
        project.current_directory().clone(),
        project.file_system().use_case_sensitive_file_names(),
    ));
    let pipe_path = match params.pipe.as_deref() {
        Some(pipe) if !pipe.is_empty() => pipe.clone(),
        _ => generate_pipe_path(),
    };
    let mut transport = PipeTransport::new(&pipe_path).map_err(|error| {
        crate::error(-32603, format!("failed to create API transport: {error}"))
    })?;
    let id = session.id().to_string();
    let worker = {
        let sessions = sessions.clone();
        let session = session.clone();
        let logger = logger.clone();
        let background = background.clone();
        let id = id.clone();
        move || {
            let accepted = transport.accept();
            let _ = transport.close();
            match accepted {
                Ok(stream) => {
                    let ctx = background.with_cancel();
                    let closer = stream.closer.clone();
                    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        tsr_api::server::serve(stream, session.clone(), None, true, false, &ctx)
                    }));
                    match outcome {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => logger.send(
                            lsp::MessageType::ERROR,
                            format!("API session {id}: {error}"),
                        ),
                        Err(panic) => {
                            logger.send(
                                lsp::MessageType::ERROR,
                                format!(
                                    "API session {id}: panic: {}",
                                    tsr_ipc::panic_message(panic.as_ref())
                                ),
                            );
                            ctx.cancel();
                            let _ = closer.close();
                        }
                    }
                }
                Err(error) => logger.send(
                    lsp::MessageType::ERROR,
                    format!("API session {id}: failed to accept connection: {error}"),
                ),
            }
            session.close();
            remove(&sessions, &id);
        }
    };
    std::thread::Builder::new()
        .name(format!("api-session {id}"))
        .spawn(worker)
        .map_err(|error| crate::error(-32603, format!("failed to start API session: {error}")))?;
    map.insert(id.clone(), session);
    Ok(lsp::InitializeAPISessionResult {
        session_id: id,
        pipe: pipe_path,
    })
}
