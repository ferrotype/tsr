//! The production host (`hostimpl.go`): one connection per mapper identity,
//! the projects that open mapper configuration handles on it, the transform
//! requests, and the validation that turns a mapper's response into a result.
use crate::host::{
    transform_error, Error, Host, InitializeError, InitializeErrorKind, MappedResult,
    MapperTimings, OperationTiming, OptionDiagnostic, OptionPathSegment, Project, ProjectErrorKind,
    ProjectSpec, Request, Timings, TransformErrorKind, TransformResult,
};
use crate::mapper::{
    diagnostic_name, hex, identity, is_supported_virtual_extension, transform_identity,
};
use crate::options_json::marshal_compiler_options;
use crate::protocol::{
    CloseProjectParams, DiagnosticDirectives, InitializeParams, InitializeResult, MappedOutput,
    OpenProjectParams, OpenProjectResult, PositionEncoding, TransformParams,
    TransformResultMessage, DIAGNOSTIC_DIRECTIVE_POLICY_EXPECT, DIAGNOSTIC_DIRECTIVE_POLICY_IGNORE,
    METHOD_CLOSE_PROJECT, METHOD_INITIALIZE, METHOD_OPEN_PROJECT, METHOD_TRANSFORM,
};
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};
use tsr_ast::span_map::SpanMap;
use tsr_ast::{Diagnostic, MappedDiagnosticDirective as AstDirective};
use tsr_core::{CompilerOptions, TextRange};
use tsr_ipc::{AsyncConn, Closer, Conn, Context, JsonRpcProtocol, Protocol, Stream};
use tsr_json::{Encode, Kind, RawValue};
use tsr_jsonrpc::{Id, Message, RequestMessage, ResponseError, ResponseMessage};
use tsr_jsstring::JsString;
use tsr_locale::Locale;
use tsr_tsoptions::config_mappers::ContentMapper;

const INITIALIZE_TIMEOUT_SECONDS: i32 = 5;
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(5);

/// A diagnostic directive's error kind, from the host contract.
use crate::host::DiagnosticDirectiveErrorKind as DirectiveErrorKind;

// ------------------------------------------------------------------ timing

#[derive(Default)]
struct OperationCollector {
    count: AtomicU64,
    duration: AtomicU64,
}

impl OperationCollector {
    /// port: tsc/internal/contentmapper/hostimpl.go:operationTiming.record
    fn record(&self, start: Instant) {
        self.count.fetch_add(1, Ordering::SeqCst);
        let elapsed = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.duration.fetch_add(elapsed, Ordering::SeqCst);
    }
    /// port: tsc/internal/contentmapper/hostimpl.go:operationTiming.snapshot
    fn snapshot(&self) -> OperationTiming {
        OperationTiming {
            count: self.count.load(Ordering::SeqCst),
            duration: Duration::from_nanos(self.duration.load(Ordering::SeqCst)),
        }
    }
}

#[derive(Default)]
struct MapperTimingCollector {
    spawn: OperationCollector,
    initialize: OperationCollector,
    open_project: OperationCollector,
    close_project: OperationCollector,
    transform: OperationCollector,
}

#[derive(Default)]
struct RequestWait {
    active: u64,
    start: Option<Instant>,
    elapsed: Duration,
}

#[derive(Default)]
struct TimingCollector {
    mappers: Mutex<HashMap<String, Arc<MapperTimingCollector>>>,
    requests: Mutex<RequestWait>,
}

impl TimingCollector {
    /// port: tsc/internal/contentmapper/hostimpl.go:timingCollector.mapper
    fn mapper(&self, identity: &str) -> Arc<MapperTimingCollector> {
        self.mappers
            .lock()
            .expect("timing lock")
            .entry(identity.to_owned())
            .or_default()
            .clone()
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:timingCollector.snapshot
    fn snapshot(&self) -> Timings {
        let request_wait = {
            let requests = self.requests.lock().expect("timing lock");
            requests.elapsed
                + if requests.active == 0 {
                    Duration::ZERO
                } else {
                    requests
                        .start
                        .map_or(Duration::ZERO, |start| start.elapsed())
                }
        };
        let mappers = self.mappers.lock().expect("timing lock").clone();
        Timings {
            mappers: mappers
                .into_iter()
                .map(|(identity, timing)| {
                    (
                        identity,
                        MapperTimings {
                            spawn: timing.spawn.snapshot(),
                            initialize: timing.initialize.snapshot(),
                            open_project: timing.open_project.snapshot(),
                            close_project: timing.close_project.snapshot(),
                            transform: timing.transform.snapshot(),
                        },
                    )
                })
                .collect(),
            request_wait,
        }
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:mapperTimingCollector.startRequest
    fn start_request(&self) -> Instant {
        let mut requests = self.requests.lock().expect("timing lock");
        if requests.active == 0 {
            requests.start = Some(Instant::now());
        }
        requests.active += 1;
        Instant::now()
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:mapperTimingCollector.finishRequest
    fn finish_request(&self, operation: &OperationCollector, start: Instant) {
        operation.record(start);
        let mut requests = self.requests.lock().expect("timing lock");
        requests.active -= 1;
        if requests.active == 0 {
            if let Some(start) = requests.start {
                requests.elapsed += start.elapsed();
            }
        }
    }
}

// ----------------------------------------------------------------- spawning

/// A spawn failure's text becomes the initialize error's detail.
pub type SpawnError = Box<dyn std::error::Error + Send + Sync>;

/// Starts a mapper's process and returns its stdio stream; closing the stream
/// tears the process down. Production hosts spawn a real process; the harness
/// serves mappers in process.
pub trait Spawner: Send + Sync {
    fn spawn(
        &self,
        command: &[JsString],
        dir: &[u8],
        stderr: Box<dyn Write + Send>,
    ) -> Result<Stream, SpawnError>;
}

/// Adapts a spawn function to [`Spawner`].
pub struct SpawnerFunc<F>(pub F);

impl<F> Spawner for SpawnerFunc<F>
where
    F: Fn(&[JsString], &[u8], Box<dyn Write + Send>) -> Result<Stream, SpawnError> + Send + Sync,
{
    /// port: tsc/internal/contentmapper/hostimpl.go:SpawnerFunc.Spawn
    fn spawn(
        &self,
        command: &[JsString],
        dir: &[u8],
        stderr: Box<dyn Write + Send>,
    ) -> Result<Stream, SpawnError> {
        (self.0)(command, dir, stderr)
    }
}

/// Receives protocol and process output as complete log lines.
pub type Logger = Arc<dyn Fn(String) + Send + Sync>;

/// Optional protocol and process logging.
#[derive(Clone, Default)]
pub struct HostOptions {
    pub logger: Option<Logger>,
}

/// Logs every message a protocol reads or writes.
struct LoggingProtocol {
    protocol: Arc<dyn Protocol>,
    mapper_name: String,
    logger: Logger,
}

impl LoggingProtocol {
    /// port: tsc/internal/contentmapper/hostimpl.go:loggingProtocol.log
    fn log(&self, direction: &str, message: &dyn Encode) {
        match tsr_json::marshal(message, tsr_json::Options::default()) {
            Ok(data) => (self.logger)(format!(
                "[content mapper: {}] {direction}: {}",
                self.mapper_name,
                String::from_utf8_lossy(&data)
            )),
            Err(error) => (self.logger)(format!(
                "[content mapper: {}] {direction}: <failed to serialize: {error}>",
                self.mapper_name
            )),
        }
    }
}

impl Protocol for LoggingProtocol {
    /// port: tsc/internal/contentmapper/hostimpl.go:loggingProtocol.ReadMessage
    fn read_message(&self) -> Result<Message, tsr_ipc::Error> {
        let message = self.protocol.read_message()?;
        self.log("receive", &message);
        Ok(message)
    }
    /// port: tsc/internal/contentmapper/hostimpl.go:loggingProtocol.WriteRequest
    fn write_request(
        &self,
        id: &Id,
        method: &str,
        params: Option<&dyn Encode>,
    ) -> Result<(), tsr_ipc::Error> {
        self.log(
            "send",
            &RequestMessage {
                id: Some(id),
                method,
                params,
            },
        );
        self.protocol.write_request(id, method, params)
    }
    /// port: tsc/internal/contentmapper/hostimpl.go:loggingProtocol.WriteNotification
    fn write_notification(
        &self,
        method: &str,
        params: Option<&dyn Encode>,
    ) -> Result<(), tsr_ipc::Error> {
        self.log(
            "send",
            &RequestMessage {
                id: None,
                method,
                params,
            },
        );
        self.protocol.write_notification(method, params)
    }
    /// port: tsc/internal/contentmapper/hostimpl.go:loggingProtocol.WriteResponse
    fn write_response(
        &self,
        id: Option<&Id>,
        result: Option<&dyn Encode>,
    ) -> Result<(), tsr_ipc::Error> {
        self.log(
            "send",
            &ResponseMessage {
                id,
                result,
                error: None,
            },
        );
        self.protocol.write_response(id, result)
    }
    /// port: tsc/internal/contentmapper/hostimpl.go:loggingProtocol.WriteError
    fn write_error(&self, id: Option<&Id>, error: &ResponseError) -> Result<(), tsr_ipc::Error> {
        self.log(
            "send",
            &ResponseMessage {
                id,
                result: None,
                error: Some(error),
            },
        );
        self.protocol.write_error(id, error)
    }
}

/// Splits a process's stderr into logged lines.
struct StderrLogger {
    mapper_name: String,
    logger: Logger,
    pending: Mutex<String>,
}

impl StderrLogger {
    /// port: tsc/internal/contentmapper/hostimpl.go:stderrLogger.flush
    fn flush_pending(&self) {
        let mut pending = self.pending.lock().expect("stderr lock");
        if !pending.is_empty() {
            let line = pending.strip_suffix('\r').unwrap_or(&pending).to_owned();
            self.log(&line);
            pending.clear();
        }
    }
    /// port: tsc/internal/contentmapper/hostimpl.go:stderrLogger.log
    fn log(&self, message: &str) {
        (self.logger)(format!(
            "[content mapper: {}] stderr: {message}",
            self.mapper_name
        ));
    }
}

struct StderrWriter(Arc<StderrLogger>);

impl Write for StderrWriter {
    /// port: tsc/internal/contentmapper/hostimpl.go:stderrLogger.Write
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let mut pending = self.0.pending.lock().expect("stderr lock");
        pending.push_str(&String::from_utf8_lossy(data));
        while let Some(index) = pending.find('\n') {
            let line = pending[..index]
                .strip_suffix('\r')
                .unwrap_or(&pending[..index])
                .to_owned();
            self.0.log(&line);
            pending.drain(..=index);
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A process whose close also flushes its partial stderr line.
struct LoggedProcess {
    closer: Arc<dyn Closer>,
    stderr: Arc<StderrLogger>,
}

impl Closer for LoggedProcess {
    /// port: tsc/internal/contentmapper/hostimpl.go:loggedProcess.Close
    fn close(&self) -> std::io::Result<()> {
        let result = self.closer.close();
        self.stderr.flush_pending();
        result
    }
    fn exit_code(&self) -> Option<i32> {
        self.closer.exit_code()
    }
}

/// Closes the underlying stream once and remembers the result.
struct CloseOnce {
    closer: Arc<dyn Closer>,
    result: Mutex<Option<std::io::Result<()>>>,
}

impl Closer for CloseOnce {
    /// port: tsc/internal/contentmapper/hostimpl.go:closeOnceReadWriteCloser.Close
    fn close(&self) -> std::io::Result<()> {
        let mut result = self.result.lock().expect("close lock");
        match &*result {
            Some(Ok(())) => Ok(()),
            Some(Err(error)) => Err(std::io::Error::new(error.kind(), error.to_string())),
            None => {
                let closed = self.closer.close();
                let copy = match &closed {
                    Ok(()) => Ok(()),
                    Err(error) => Err(std::io::Error::new(error.kind(), error.to_string())),
                };
                *result = Some(closed);
                copy
            }
        }
    }
    /// port: tsc/internal/contentmapper/hostimpl.go:closeOnceReadWriteCloser.ExitCode
    fn exit_code(&self) -> Option<i32> {
        self.closer.exit_code()
    }
}

/// Rejects every request a mapper initiates: the protocol is parent-driven.
struct RejectHandler;

impl tsr_ipc::Handler for RejectHandler {
    /// port: tsc/internal/contentmapper/hostimpl.go:rejectHandler.HandleRequest
    fn handle_request(&self, _: &Context, method: &str, _: &[u8]) -> tsr_ipc::HandlerResult {
        Err(format!("content mapper sent an unexpected request: {method}").into())
    }
    /// port: tsc/internal/contentmapper/hostimpl.go:rejectHandler.HandleNotification
    fn handle_notification(
        &self,
        _: &Context,
        _: &str,
        _: &[u8],
    ) -> Result<(), tsr_ipc::HandlerError> {
        Ok(())
    }
}

// --------------------------------------------------------------------- host

/// A running connection to one mapper and what its initialize returned.
struct Dialed {
    conn: Arc<dyn Conn>,
    closer: Arc<dyn Closer>,
    position_encoding: PositionEncoding,
    diagnostic_source: String,
}

/// Establishes a running connection to a mapper.
type Dial = Box<dyn Fn(&Context, &ContentMapper, &Locale) -> Result<Dialed, Error> + Send + Sync>;

#[derive(Default)]
struct MapperConn {
    conn: Option<Arc<dyn Conn>>,
    closer: Option<Arc<dyn Closer>>,
    /// A failed start, cached so a broken mapper is not respawned.
    error: Option<Error>,
    position_encoding: PositionEncoding,
    diagnostic_source: String,
    /// Active acquisitions retaining this identity.
    refs: usize,
}

struct ProjectEntry {
    mapper: ContentMapper,
    /// The mapper's index in its project's list.
    index: usize,
    spec: ProjectSpec,
    project_handle: String,
    opened: bool,
    config_identity: String,
    watched_files: Vec<String>,
    option_diagnostics: Vec<OptionDiagnostic>,
}

struct LeaseState {
    /// Each mapper's project entry key, by index.
    entries: Vec<String>,
    refs: usize,
}

#[derive(Default)]
struct HostState {
    /// `None` once the host is closed.
    conns: Option<HashMap<String, MapperConn>>,
    projects: Option<HashMap<String, ProjectEntry>>,
    project_leases: Option<HashMap<String, Arc<Mutex<LeaseState>>>>,
    next_project_id: u64,
}

struct HostInner {
    ctx: Context,
    stop: Mutex<Option<tsr_ipc::AfterFuncStop>>,
    dial: Dial,
    timing: Arc<TimingCollector>,
    lifecycle: RwLock<Locale>,
    state: Mutex<HostState>,
}

/// The production content-mapper host. Cloning shares it.
#[derive(Clone)]
pub struct HostImpl(Arc<HostInner>);

/// A host spawning each mapper through `spawner`.
/// port: tsc/internal/contentmapper/hostimpl.go:NewHost
pub fn new_host(ctx: &Context, spawner: Arc<dyn Spawner>, locale: Locale) -> HostImpl {
    new_host_with_options(ctx, spawner, locale, HostOptions::default())
}

/// port: tsc/internal/contentmapper/hostimpl.go:NewHostWithOptions
pub fn new_host_with_options(
    ctx: &Context,
    spawner: Arc<dyn Spawner>,
    locale: Locale,
    options: HostOptions,
) -> HostImpl {
    let logger = options.logger;
    let timing = Arc::new(TimingCollector::default());
    let dial_timing = timing.clone();
    let dial: Dial = Box::new(move |ctx, mapper, locale| {
        dial_process(
            ctx,
            mapper,
            locale,
            spawner.as_ref(),
            logger.as_ref(),
            &dial_timing,
        )
    });
    new_with_dial(ctx, locale, timing, dial)
}

/// Spawns a mapper, starts its connection and completes the handshake.
fn dial_process(
    ctx: &Context,
    mapper: &ContentMapper,
    locale: &Locale,
    spawner: &dyn Spawner,
    logger: Option<&Logger>,
    timing: &TimingCollector,
) -> Result<Dialed, Error> {
    let exec = mapper.manifest.exec.as_deref().unwrap_or_default();
    if exec.is_empty() {
        return Err(Error::Message(format!(
            "content mapper {} declares no command to run",
            tsr_jsstring::go_quote(mapper.package.as_bytes())
        )));
    }
    let mapper_timing = timing.mapper(&lossy(identity(mapper).as_bytes()));
    let name = lossy(diagnostic_name(mapper).as_bytes());
    let spawn_start = Instant::now();
    let stderr_log = logger.map(|logger| {
        Arc::new(StderrLogger {
            mapper_name: name.clone(),
            logger: logger.clone(),
            pending: Mutex::new(String::new()),
        })
    });
    let stderr: Box<dyn Write + Send> = match &stderr_log {
        Some(log) => Box::new(StderrWriter(log.clone())),
        None => Box::new(std::io::sink()),
    };
    let spawned = spawner.spawn(exec, mapper.package_directory.as_bytes(), stderr);
    mapper_timing.spawn.record(spawn_start);
    let stream = spawned.map_err(|error| {
        let mut failure = InitializeError::new(InitializeErrorKind::ProcessStart);
        failure.mapper_name = diagnostic_name(mapper).clone();
        failure.command = exec[0].clone();
        failure.detail = error.to_string();
        Error::Initialize(Box::new(failure))
    })?;
    let mut closer = stream.closer;
    if let Some(log) = &stderr_log {
        closer = Arc::new(LoggedProcess {
            closer,
            stderr: log.clone(),
        });
    }
    let closer: Arc<dyn Closer> = Arc::new(CloseOnce {
        closer,
        result: Mutex::new(None),
    });
    let mut protocol: Arc<dyn Protocol> = JsonRpcProtocol::new(stream.reader, stream.writer);
    if let Some(logger) = logger {
        protocol = Arc::new(LoggingProtocol {
            protocol,
            mapper_name: name.clone(),
            logger: logger.clone(),
        });
    }
    let conn = AsyncConn::with_protocol(Some(closer.clone()), protocol, Arc::new(RejectHandler));
    {
        let (conn, closer, ctx) = (conn.clone(), closer.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = conn.run(&ctx);
            let _ = closer.close();
        });
    }
    let initialize_ctx = ctx.with_timeout(INITIALIZE_TIMEOUT);
    let initialize_start = timing.start_request();
    let handshake = handshake(&initialize_ctx, &conn, locale);
    timing.finish_request(&mapper_timing.initialize, initialize_start);
    let initialize_ctx_error = initialize_ctx.err();
    initialize_ctx.cancel();
    match handshake {
        Ok((position_encoding, diagnostic_source)) => Ok(Dialed {
            conn: Arc::new(conn),
            closer,
            position_encoding,
            diagnostic_source,
        }),
        Err(error) => {
            let exit_code = closer.exit_code();
            let _ = closer.close();
            if let Error::Initialize(mut failure) = error {
                failure.mapper_name = diagnostic_name(mapper).clone();
                return Err(Error::Initialize(failure));
            }
            let mut failure = InitializeError::new(InitializeErrorKind::Request);
            failure.mapper_name = diagnostic_name(mapper).clone();
            if let Some(exit_code) = exit_code {
                failure.kind = InitializeErrorKind::ProcessExit;
                failure.exit_code = exit_code;
            } else if initialize_ctx_error.is_some()
                || matches!(&error, Error::Ipc(ipc) if ipc.context_error().is_some())
            {
                failure.kind = InitializeErrorKind::NoResponse;
                failure.timeout_seconds = INITIALIZE_TIMEOUT_SECONDS;
            } else {
                failure.detail = error.to_string();
            }
            Err(Error::Initialize(Box::new(failure)))
        }
    }
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// port: tsc/internal/contentmapper/hostimpl.go:newWithDial
fn new_with_dial(
    ctx: &Context,
    locale: Locale,
    timing: Arc<TimingCollector>,
    dial: Dial,
) -> HostImpl {
    let host = HostImpl(Arc::new(HostInner {
        ctx: ctx.with_cancel(),
        stop: Mutex::new(None),
        dial,
        timing,
        lifecycle: RwLock::new(locale),
        state: Mutex::new(HostState {
            conns: Some(HashMap::new()),
            projects: Some(HashMap::new()),
            project_leases: Some(HashMap::new()),
            next_project_id: 0,
        }),
    }));
    let weak: Weak<HostInner> = Arc::downgrade(&host.0);
    let stop = ctx.after_func(move || {
        if let Some(inner) = weak.upgrade() {
            let _ = HostImpl(inner).close();
        }
    });
    *host.0.stop.lock().expect("stop lock") = Some(stop);
    host
}

/// The shared-project key: the pin keys by the identity of the options and
/// mapper pointers, as the spec's shared allocations are here.
/// port: tsc/internal/contentmapper/hostimpl.go:projectSpecKey
fn project_spec_key(spec: &ProjectSpec) -> String {
    format!(
        "{}\0{:p}\0{:p}",
        lossy(spec.config_file_name.as_bytes()),
        Arc::as_ptr(&spec.compiler_options),
        Arc::as_ptr(&spec.mappers).cast::<u8>()
    )
}

/// port: tsc/internal/contentmapper/hostimpl.go:combinedIdentity
fn combined_identity(
    mapper: &ContentMapper,
    config_identity: &str,
    options: &CompilerOptions,
) -> String {
    let transform = transform_identity(mapper, Some(options));
    let identity = identity(mapper);
    let mapper_options = mapper.options.as_deref().unwrap_or_default();
    let mut buffer = Vec::new();
    buffer.extend_from_slice(identity.as_bytes());
    buffer.push(0);
    buffer.extend_from_slice(mapper_options);
    buffer.push(0);
    buffer.extend_from_slice(config_identity.as_bytes());
    buffer.push(0);
    buffer.extend_from_slice(&transform);
    let hash = xxhash_rust::xxh3::xxh3_128(&buffer).to_be_bytes();
    format!("{}:{}", lossy(identity.as_bytes()), hex(&hash))
}

/// A static mapper's project identity.
fn static_identity(mapper: &ContentMapper, options: &CompilerOptions) -> String {
    format!(
        "{}:{}",
        lossy(identity(mapper).as_bytes()),
        hex(&transform_identity(mapper, Some(options)))
    )
}

impl HostImpl {
    fn state(&self) -> std::sync::MutexGuard<'_, HostState> {
        self.0.state.lock().expect("host lock")
    }

    fn locale(&self) -> std::sync::RwLockReadGuard<'_, Locale> {
        self.0.lifecycle.read().expect("host lifecycle")
    }

    /// Opens the entry's mapper project once, validating its response.
    /// port: tsc/internal/contentmapper/hostimpl.go:host.openProjectLocked
    fn open_project_locked(
        &self,
        state: &mut HostState,
        locale: &Locale,
        key: &str,
    ) -> Result<(), Error> {
        let (mapper, spec, handle, index) = {
            let entry = state
                .projects
                .as_ref()
                .and_then(|projects| projects.get(key))
                .expect("project entry");
            if entry.opened {
                return Ok(());
            }
            (
                entry.mapper.clone(),
                entry.spec.clone(),
                entry.project_handle.clone(),
                entry.index,
            )
        };
        let (conn, _, diagnostic_source) = self.conn_for_locked(state, locale, &mapper)?;
        let compiler_options = RawValue(marshal_compiler_options(&spec.compiler_options)?);
        let mapper_timing = self.0.timing.mapper(&lossy(identity(&mapper).as_bytes()));
        let start = self.0.timing.start_request();
        let raw = conn.call(
            &self.0.ctx,
            METHOD_OPEN_PROJECT,
            &OpenProjectParams {
                config_file_name: lossy(spec.config_file_name.as_bytes()),
                project_handle: handle,
                options: mapper.options.clone().map(RawValue),
                compiler_options,
            },
        );
        self.0
            .timing
            .finish_request(&mapper_timing.open_project, start);
        let raw = raw?;
        let mut result = OpenProjectResult::default();
        tsr_json::unmarshal(&raw.0, &mut result, tsr_json::Options::default())
            .map_err(|_| Error::Project(ProjectErrorKind::MalformedResponse))?;
        let dynamic = mapper.manifest.dynamic_config;
        if dynamic && result.config_identity.is_empty() {
            return Err(Error::Project(ProjectErrorKind::MissingConfigIdentity));
        }
        if !dynamic && !result.config_identity.is_empty() {
            return Err(Error::Project(ProjectErrorKind::UnexpectedConfigIdentity));
        }
        if !dynamic && !result.watched_files.is_empty() {
            return Err(Error::Project(ProjectErrorKind::UnexpectedWatchedFiles));
        }
        let entry = state
            .projects
            .as_mut()
            .and_then(|projects| projects.get_mut(key))
            .expect("project entry");
        entry.config_identity.clone_from(&result.config_identity);
        if result
            .watched_files
            .iter()
            .any(|file| !tsr_tspath::path_is_absolute(file.as_bytes()))
        {
            return Err(Error::Project(ProjectErrorKind::NonAbsoluteWatchedFile));
        }
        entry.watched_files.clone_from(&result.watched_files);
        entry.option_diagnostics = Vec::with_capacity(result.option_diagnostics.len());
        for diagnostic in &result.option_diagnostics {
            let mut path = Vec::with_capacity(diagnostic.path.len());
            for raw_segment in &diagnostic.path {
                let malformed = || Error::Project(ProjectErrorKind::MalformedResponse);
                let segment = match raw_segment.kind() {
                    Kind::String => {
                        let mut property = String::new();
                        tsr_json::unmarshal(
                            &raw_segment.0,
                            &mut property,
                            tsr_json::Options::default(),
                        )
                        .map_err(|_| malformed())?;
                        OptionPathSegment {
                            property,
                            index: 0,
                            is_index: false,
                        }
                    }
                    Kind::Number => {
                        let mut index = 0i64;
                        tsr_json::unmarshal(
                            &raw_segment.0,
                            &mut index,
                            tsr_json::Options::default(),
                        )
                        .map_err(|_| malformed())?;
                        OptionPathSegment {
                            property: String::new(),
                            index: usize::try_from(index).map_err(|_| malformed())?,
                            is_index: true,
                        }
                    }
                    _ => return Err(malformed()),
                };
                path.push(segment);
            }
            entry.option_diagnostics.push(OptionDiagnostic {
                mapper: index,
                path,
                source: diagnostic_source.clone(),
                code: diagnostic.code,
                message_text: diagnostic.message_text.clone(),
            });
        }
        entry.opened = true;
        Ok(())
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:host.closeProject
    fn close_project(
        &self,
        mapper: &ContentMapper,
        conn: &Arc<dyn Conn>,
        handle: &str,
    ) -> Result<(), Error> {
        let mapper_timing = self.0.timing.mapper(&lossy(identity(mapper).as_bytes()));
        let start = self.0.timing.start_request();
        let result = conn.call(
            &self.0.ctx,
            METHOD_CLOSE_PROJECT,
            &CloseProjectParams {
                project_handle: handle.to_owned(),
            },
        );
        self.0
            .timing
            .finish_request(&mapper_timing.close_project, start);
        result.map(|_| ()).map_err(Error::Ipc)
    }

    /// Sends one file to its mapper and decodes the result. The caller holds
    /// the lifecycle read guard `locale` came from.
    /// port: tsc/internal/contentmapper/hostimpl.go:host.transformLocked
    fn transform_locked(
        &self,
        locale: &Locale,
        mapper: &ContentMapper,
        request: &Request,
        project_handle: &str,
    ) -> Result<TransformResult, Error> {
        if project_handle.is_empty() {
            return Err(Error::Message(
                "content mapper project handle is required".into(),
            ));
        }
        let (conn, position_encoding, diagnostic_source) = self
            .conn_for(locale, mapper)
            .map_err(|error| transform_error(TransformErrorKind::Initialize, Some(error)))?;
        let mapper_timing = self.0.timing.mapper(&lossy(identity(mapper).as_bytes()));
        let start = self.0.timing.start_request();
        let raw = conn.call(
            &self.0.ctx,
            METHOD_TRANSFORM,
            &TransformParams {
                file_name: lossy(request.file_name.as_bytes()),
                content: lossy(&request.content),
                project_handle: project_handle.to_owned(),
            },
        );
        self.0
            .timing
            .finish_request(&mapper_timing.transform, start);
        let raw = raw.map_err(|error| {
            transform_error(TransformErrorKind::Request, Some(Error::Ipc(error)))
        })?;
        decode_transform_result(
            &raw.0,
            &request.content,
            &position_encoding,
            &diagnostic_source,
        )
        .map_err(|error| transform_error(TransformErrorKind::Response, Some(error)))
    }

    /// The connection of a mapper's identity, spawning it on first use. Like
    /// the pin it takes only the state lock: the caller already holds the
    /// lifecycle read guard `locale` came from, and a second read could wait
    /// behind a queued `set_locale` or `close` that waits for the first.
    /// port: tsc/internal/contentmapper/hostimpl.go:host.connFor
    fn conn_for(
        &self,
        locale: &Locale,
        mapper: &ContentMapper,
    ) -> Result<(Arc<dyn Conn>, PositionEncoding, String), Error> {
        let mut state = self.state();
        self.conn_for_locked(&mut state, locale, mapper)
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:host.connForLocked
    fn conn_for_locked(
        &self,
        state: &mut HostState,
        locale: &Locale,
        mapper: &ContentMapper,
    ) -> Result<(Arc<dyn Conn>, PositionEncoding, String), Error> {
        let Some(conns) = state.conns.as_mut() else {
            return Err(Error::Message("content mapper host is closed".into()));
        };
        let entry = conns.entry(lossy(identity(mapper).as_bytes())).or_default();
        if let Some(error) = &entry.error {
            return Err(error.clone());
        }
        if let Some(conn) = &entry.conn {
            return Ok((
                conn.clone(),
                entry.position_encoding.clone(),
                entry.diagnostic_source.clone(),
            ));
        }
        match (self.0.dial)(&self.0.ctx, mapper, locale) {
            Ok(dialed) => {
                entry.conn = Some(dialed.conn.clone());
                entry.closer = Some(dialed.closer);
                entry.position_encoding = dialed.position_encoding.clone();
                entry
                    .diagnostic_source
                    .clone_from(&dialed.diagnostic_source);
                Ok((
                    dialed.conn,
                    dialed.position_encoding,
                    dialed.diagnostic_source,
                ))
            }
            Err(error) => {
                entry.error = Some(error.clone());
                Err(error)
            }
        }
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:host.release
    fn release(&self, identities: &[String]) {
        let mut closers = Vec::new();
        {
            let mut state = self.state();
            if let Some(conns) = state.conns.as_mut() {
                for identity in identities {
                    let Some(entry) = conns.get_mut(identity) else {
                        continue;
                    };
                    entry.refs -= 1;
                    if entry.refs == 0 {
                        if let Some(closer) = conns.remove(identity).and_then(|entry| entry.closer)
                        {
                            closers.push(closer);
                        }
                    }
                }
            }
        }
        for closer in closers {
            let _ = closer.close();
        }
    }
}

impl Host for HostImpl {
    /// port: tsc/internal/contentmapper/hostimpl.go:host.Timings
    fn timings(&self) -> Timings {
        self.0.timing.snapshot()
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:host.SetLocale
    fn set_locale(&self, locale: Locale) {
        let mut current = self.0.lifecycle.write().expect("host lifecycle");
        if current.to_string() == locale.to_string() {
            return;
        }
        *current = locale;
        let mut closers = Vec::new();
        {
            let mut state = self.state();
            for entry in state.conns.iter_mut().flat_map(HashMap::values_mut) {
                closers.extend(entry.closer.take());
                entry.conn = None;
                entry.error = None;
                entry.position_encoding = PositionEncoding::default();
                entry.diagnostic_source.clear();
            }
            for project in state.projects.iter_mut().flat_map(HashMap::values_mut) {
                project.opened = false;
            }
        }
        for closer in closers {
            let _ = closer.close();
        }
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:host.Project
    fn project(&self, spec: ProjectSpec) -> Option<Arc<dyn Project>> {
        let _lifecycle = self.locale();
        let key = project_spec_key(&spec);
        let mut state = self.state();
        state.projects.as_ref()?;
        if let Some(lease) = state
            .project_leases
            .as_ref()
            .and_then(|leases| leases.get(&key))
            .cloned()
        {
            // port: tsc/internal/contentmapper/hostimpl.go:projectLease.retainLocked
            lease.lock().expect("lease lock").refs += 1;
            return Some(Arc::new(ProjectLease {
                host: self.clone(),
                key,
                lease,
                closed: AtomicBool::new(false),
            }));
        }
        let mut entries = Vec::with_capacity(spec.mappers.len());
        for (index, mapper) in spec.mappers.iter().enumerate() {
            let identity = lossy(identity(mapper).as_bytes());
            let entry_key = format!("{identity}:{}", state.next_project_id);
            state.next_project_id += 1;
            state.projects.as_mut().expect("open host").insert(
                entry_key.clone(),
                ProjectEntry {
                    mapper: mapper.clone(),
                    index,
                    spec: spec.clone(),
                    project_handle: entry_key.clone(),
                    opened: false,
                    config_identity: String::new(),
                    watched_files: Vec::new(),
                    option_diagnostics: Vec::new(),
                },
            );
            state
                .conns
                .as_mut()
                .expect("open host")
                .entry(identity)
                .or_default()
                .refs += 1;
            entries.push(entry_key);
        }
        let lease = Arc::new(Mutex::new(LeaseState { entries, refs: 1 }));
        state
            .project_leases
            .as_mut()
            .expect("open host")
            .insert(key.clone(), lease.clone());
        Some(Arc::new(ProjectLease {
            host: self.clone(),
            key,
            lease,
            closed: AtomicBool::new(false),
        }))
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:host.Acquire
    fn acquire(&self, mappers: &[ContentMapper]) -> Box<dyn FnOnce() + Send> {
        let mut identities: Vec<String> = Vec::with_capacity(mappers.len());
        {
            let mut state = self.state();
            if let Some(conns) = state.conns.as_mut() {
                for mapper in mappers {
                    let identity = lossy(identity(mapper).as_bytes());
                    if identities.contains(&identity) {
                        continue;
                    }
                    conns.entry(identity.clone()).or_default().refs += 1;
                    identities.push(identity);
                }
            }
        }
        let host = self.clone();
        Box::new(move || host.release(&identities))
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:host.Transform
    fn transform(
        &self,
        mapper: &ContentMapper,
        request: &Request,
    ) -> Result<TransformResult, Error> {
        let Some(project) = self.project(ProjectSpec {
            config_file_name: JsString::default(),
            mappers: Arc::from(vec![mapper.clone()]),
            compiler_options: Arc::new(CompilerOptions::default()),
        }) else {
            return Err(Error::Message("content mapper project is closed".into()));
        };
        let result = project.transform(0, request);
        let _ = project.close();
        result
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:host.Close
    fn close(&self) -> Result<(), Error> {
        let _lifecycle = self.0.lifecycle.write().expect("host lifecycle");
        if let Some(stop) = self.0.stop.lock().expect("stop lock").take() {
            stop.stop();
        }
        self.0.ctx.cancel();
        let closers: Vec<Arc<dyn Closer>> = {
            let mut state = self.state();
            let closers = state
                .conns
                .iter()
                .flat_map(HashMap::values)
                .filter_map(|entry| entry.closer.clone())
                .collect();
            state.conns = None;
            state.projects = None;
            state.project_leases = None;
            closers
        };
        let errors: Vec<String> = closers
            .iter()
            .filter_map(|closer| closer.close().err())
            .map(|error| error.to_string())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(Error::Message(errors.join("\n")))
        }
    }
}

/// One reference to a shared project. Each reference closes once.
struct ProjectLease {
    host: HostImpl,
    key: String,
    lease: Arc<Mutex<LeaseState>>,
    closed: AtomicBool,
}

impl ProjectLease {
    fn entry_key(&self, mapper: usize) -> Option<String> {
        self.lease
            .lock()
            .expect("lease lock")
            .entries
            .get(mapper)
            .cloned()
    }

    fn entry_keys(&self) -> Vec<String> {
        self.lease.lock().expect("lease lock").entries.clone()
    }

    fn identity_locked(
        &self,
        state: &mut HostState,
        locale: &Locale,
        key: &str,
    ) -> Result<String, Error> {
        let (mapper, options, dynamic) = {
            let entry = &state.projects.as_ref().expect("open host")[key];
            (
                entry.mapper.clone(),
                entry.spec.compiler_options.clone(),
                entry.mapper.manifest.dynamic_config,
            )
        };
        if !dynamic {
            return Ok(static_identity(&mapper, &options));
        }
        self.host.open_project_locked(state, locale, key)?;
        let config_identity = state.projects.as_ref().expect("open host")[key]
            .config_identity
            .clone();
        Ok(combined_identity(&mapper, &config_identity, &options))
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:projectLease.release
    fn release(&self) -> Result<(), Error> {
        let locale = self.host.locale();
        let mut result: Option<Error> = None;
        let mut released = Vec::new();
        {
            let mut state = self.host.state();
            let entries = {
                let mut lease = self.lease.lock().expect("lease lock");
                assert!(
                    lease.refs > 0,
                    "content mapper project reference count below zero"
                );
                lease.refs -= 1;
                if lease.refs != 0 {
                    return Ok(());
                }
                lease.entries.clone()
            };
            if let Some(leases) = state.project_leases.as_mut() {
                if leases
                    .get(&self.key)
                    .is_some_and(|lease| Arc::ptr_eq(lease, &self.lease))
                {
                    leases.remove(&self.key);
                }
            }
            for key in entries {
                let Some(entry) = state
                    .projects
                    .as_ref()
                    .and_then(|projects| projects.get(&key))
                else {
                    continue;
                };
                let identity = lossy(identity(&entry.mapper).as_bytes());
                if entry.opened {
                    let conn = state
                        .conns
                        .as_ref()
                        .and_then(|conns| conns.get(&identity))
                        .and_then(|conn| conn.conn.clone());
                    if let Some(conn) = conn {
                        let (mapper, handle) = (entry.mapper.clone(), entry.project_handle.clone());
                        if let Err(error) = self.host.close_project(&mapper, &conn, &handle) {
                            result = Some(join(result, error));
                        }
                    }
                }
                state.projects.as_mut().expect("open host").remove(&key);
                released.push(identity);
            }
        }
        drop(locale);
        self.host.release(&released);
        result.map_or(Ok(()), Err)
    }
}

/// `errors.Join` of an optional error and another.
fn join(first: Option<Error>, second: Error) -> Error {
    match first {
        None => second,
        Some(first) => Error::Message(format!("{first}\n{second}")),
    }
}

impl Project for ProjectLease {
    /// port: tsc/internal/contentmapper/hostimpl.go:projectLease.Refresh
    fn refresh(&self) -> Result<(), Error> {
        let _locale = self.host.locale();
        let mut state = self.host.state();
        if state.projects.is_none() {
            return Ok(());
        }
        let mut result = None;
        for key in self.entry_keys() {
            let Some(entry) = state
                .projects
                .as_ref()
                .and_then(|projects| projects.get(&key))
            else {
                continue;
            };
            if !entry.opened {
                continue;
            }
            let identity = lossy(identity(&entry.mapper).as_bytes());
            let conn = state
                .conns
                .as_ref()
                .and_then(|conns| conns.get(&identity))
                .and_then(|conn| conn.conn.clone());
            if let Some(conn) = conn {
                let (mapper, handle) = (entry.mapper.clone(), entry.project_handle.clone());
                if let Err(error) = self.host.close_project(&mapper, &conn, &handle) {
                    result = Some(join(result, error));
                }
            }
            state
                .projects
                .as_mut()
                .expect("open host")
                .get_mut(&key)
                .expect("entry")
                .opened = false;
        }
        result.map_or(Ok(()), Err)
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:projectLease.Identities
    fn identities(&self) -> Result<Vec<String>, Error> {
        let locale = self.host.locale();
        let mut state = self.host.state();
        if state.projects.is_none() {
            return Ok(Vec::new());
        }
        let mut identities = Vec::new();
        for key in self.entry_keys() {
            if !state
                .projects
                .as_ref()
                .expect("open host")
                .contains_key(&key)
            {
                continue;
            }
            identities.push(self.identity_locked(&mut state, &locale, &key)?);
        }
        Ok(identities)
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:projectLease.Identity
    fn identity(&self, mapper: usize) -> Result<String, Error> {
        let locale = self.host.locale();
        let mut state = self.host.state();
        if state.projects.is_none() {
            return Ok(String::new());
        }
        let Some(key) = self.entry_key(mapper) else {
            return Ok(String::new());
        };
        if !state
            .projects
            .as_ref()
            .expect("open host")
            .contains_key(&key)
        {
            return Ok(String::new());
        }
        self.identity_locked(&mut state, &locale, &key)
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:projectLease.WatchedFiles
    fn watched_files(&self) -> Result<Vec<String>, Error> {
        let locale = self.host.locale();
        let mut state = self.host.state();
        if state.projects.is_none() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        for key in self.entry_keys() {
            let Some(dynamic) = state
                .projects
                .as_ref()
                .expect("open host")
                .get(&key)
                .map(|entry| entry.mapper.manifest.dynamic_config)
            else {
                continue;
            };
            if dynamic {
                self.host.open_project_locked(&mut state, &locale, &key)?;
            }
            files.extend(
                state.projects.as_ref().expect("open host")[&key]
                    .watched_files
                    .iter()
                    .cloned(),
            );
        }
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:projectLease.Diagnostics
    fn diagnostics(&self) -> Vec<OptionDiagnostic> {
        let _locale = self.host.locale();
        let state = self.host.state();
        let Some(projects) = state.projects.as_ref() else {
            return Vec::new();
        };
        let mut diagnostics = Vec::new();
        for key in self.entry_keys() {
            if let Some(entry) = projects.get(&key).filter(|entry| entry.opened) {
                diagnostics.extend(entry.option_diagnostics.iter().cloned());
            }
        }
        diagnostics
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:projectLease.Transform
    fn transform(&self, mapper: usize, request: &Request) -> Result<TransformResult, Error> {
        let locale = self.host.locale();
        let (content_mapper, handle) = {
            let mut state = self.host.state();
            let key = self.entry_key(mapper);
            let Some(key) = key.filter(|key| {
                state
                    .projects
                    .as_ref()
                    .is_some_and(|projects| projects.contains_key(key))
            }) else {
                return Err(Error::Message("content mapper project is closed".into()));
            };
            if let Err(error) = self.host.open_project_locked(&mut state, &locale, &key) {
                let kind = if error.initialize_error().is_some() {
                    TransformErrorKind::Initialize
                } else {
                    TransformErrorKind::Project
                };
                return Err(transform_error(kind, Some(error)));
            }
            let entry = &state.projects.as_ref().expect("open host")[&key];
            (entry.mapper.clone(), entry.project_handle.clone())
        };
        // One guard from opening to transforming: a locale change in between
        // would replace the connection that opened the project.
        self.host
            .transform_locked(&locale, &content_mapper, request, &handle)
    }

    /// Every reference, the first lease or a retained one, closes once.
    /// port: tsc/internal/contentmapper/hostimpl.go:projectLease.Close
    /// port: tsc/internal/contentmapper/hostimpl.go:retainedProject.Close
    fn close(&self) -> Result<(), Error> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.release()
    }
}

// ------------------------------------------------------------ the handshake

/// The native extensions without their dot, which a mapper may not claim as
/// its diagnostic source.
const NATIVE_EXTENSIONS: &[&[u8]] = &[
    b".ts", b".tsx", b".d.ts", b".cts", b".d.cts", b".mts", b".d.mts", b".js", b".jsx", b".cjs",
    b".mjs", b".json",
];

/// `strings.EqualFold` for the reserved names, all ASCII.
fn equal_fold(left: &str, right: &str) -> bool {
    left.chars()
        .flat_map(char::to_lowercase)
        .eq(right.chars().flat_map(char::to_lowercase))
}

/// Initializes a mapper connection: its position encoding and diagnostic source.
/// port: tsc/internal/contentmapper/hostimpl.go:handshake
fn handshake(
    ctx: &Context,
    conn: &AsyncConn,
    locale: &Locale,
) -> Result<(PositionEncoding, String), Error> {
    let raw = conn.call(
        ctx,
        METHOD_INITIALIZE,
        &InitializeParams {
            locale: locale.to_string(),
            position_encodings: vec![PositionEncoding::utf8(), PositionEncoding::utf16()],
        },
    )?;
    let mut result = InitializeResult::default();
    if let Err(error) = tsr_json::unmarshal(&raw.0, &mut result, tsr_json::Options::default()) {
        let mut failure = InitializeError::new(InitializeErrorKind::InvalidResponse);
        failure.detail = error.to_string();
        return Err(Error::Initialize(Box::new(failure)));
    }
    let encoding = result.position_encoding.as_str();
    if encoding != PositionEncoding::UTF8 && encoding != PositionEncoding::UTF16 {
        let mut failure = InitializeError::new(InitializeErrorKind::PositionEncoding);
        failure.position_encoding = result.position_encoding;
        return Err(Error::Initialize(Box::new(failure)));
    }
    if result.diagnostic_source.trim().is_empty() {
        return Err(Error::Initialize(Box::new(InitializeError::new(
            InitializeErrorKind::EmptyDiagnosticSource,
        ))));
    }
    let source = &result.diagnostic_source;
    let reserved = equal_fold(source, "typescript")
        || equal_fold(source, "tsc")
        || NATIVE_EXTENSIONS
            .iter()
            .any(|extension| equal_fold(source, &lossy(&extension[1..])));
    if reserved {
        let mut failure = InitializeError::new(InitializeErrorKind::ReservedDiagnosticSource);
        failure.diagnostic_source.clone_from(source);
        return Err(Error::Initialize(Box::new(failure)));
    }
    Ok((result.position_encoding, result.diagnostic_source))
}

// ------------------------------------------------------------- the response

/// Decodes and validates a transform response against the original text.
/// port: tsc/internal/contentmapper/hostimpl.go:decodeTransformResult
fn decode_transform_result(
    raw: &[u8],
    original_text: &[u8],
    position_encoding: &PositionEncoding,
    diagnostic_source: &str,
) -> Result<TransformResult, Error> {
    let mut message = TransformResultMessage::default();
    tsr_json::unmarshal(raw, &mut message, tsr_json::Options::default())?;
    let (mapped, original_positions) = decode_mapped_output(
        &message.output,
        original_text,
        position_encoding,
        diagnostic_source,
    )?;
    let mut result = TransformResult {
        text: mapped.text,
        virtual_extension: mapped.virtual_extension,
        diagnostics: Vec::new(),
        mappings: mapped.mappings,
        diagnostic_directives: mapped.diagnostic_directives,
        supplemental: Vec::new(),
    };
    for (supplemental_index, supplemental) in message.supplemental.iter().enumerate() {
        match decode_mapped_output(
            supplemental,
            original_text,
            position_encoding,
            diagnostic_source,
        ) {
            Ok((mapped, _)) => result.supplemental.push(mapped),
            Err(Error::DiagnosticDirective {
                kind,
                index,
                policy,
                ..
            }) => {
                return Err(Error::DiagnosticDirective {
                    kind,
                    index,
                    supplemental_index: Some(supplemental_index),
                    policy,
                })
            }
            Err(error) => return Err(error),
        }
    }
    for diagnostic in &message.diagnostics {
        if diagnostic.start < 0
            || diagnostic.length < 0
            || diagnostic.start > i64::MAX - diagnostic.length
        {
            return Err(Error::Message(format!(
                "invalid content mapper diagnostic range [{}, {})",
                diagnostic.start,
                diagnostic.start.wrapping_add(diagnostic.length)
            )));
        }
        let start = original_positions
            .normalize(diagnostic.start)
            .map_err(|error| {
                Error::Message(format!("invalid content mapper diagnostic start: {error}"))
            })?;
        let end = original_positions
            .normalize(diagnostic.start + diagnostic.length)
            .map_err(|error| {
                Error::Message(format!("invalid content mapper diagnostic end: {error}"))
            })?;
        result.diagnostics.push(Diagnostic::external(
            None,
            TextRange::new(start, end),
            JsString::from_bytes(diagnostic_source.as_bytes()),
            tsr_diagnostics::Category::Error as i32,
            diagnostic.code,
            JsString::from_bytes(diagnostic.message_text.as_bytes()),
        ));
    }
    Ok(result)
}

/// port: tsc/internal/contentmapper/hostimpl.go:decodeMappedOutput
fn decode_mapped_output(
    output: &MappedOutput,
    original_text: &[u8],
    position_encoding: &PositionEncoding,
    diagnostic_source: &str,
) -> Result<(MappedResult, PositionNormalizer), Error> {
    if !is_supported_virtual_extension(output.extension.as_bytes()) {
        return Err(Error::InvalidVirtualExtension(output.extension.clone()));
    }
    let virtual_positions = PositionNormalizer::new(output.text.as_bytes(), position_encoding)?;
    let original_positions = PositionNormalizer::new(original_text, position_encoding)?;
    // A successful transform always carries a span map; absent or empty
    // mappings describe fully synthesized output, never "not mapped".
    let mappings = match output.mappings.as_ref().filter(|raw| !raw.0.is_empty()) {
        Some(raw) => {
            let map = tsr_ast::span_map::unmarshal(&raw.0).map_err(Error::SpanMap)?;
            normalize_mappings(&map, &virtual_positions, &original_positions)?
        }
        None => SpanMap::new(&[]),
    };
    let diagnostic_directives = normalize_diagnostic_directives(
        output.diagnostic_directives.as_ref(),
        &virtual_positions,
        &original_positions,
        diagnostic_source,
    )?;
    Ok((
        MappedResult {
            text: output.text.clone(),
            virtual_extension: output.extension.clone(),
            mappings: Some(Arc::new(mappings)),
            diagnostic_directives,
        },
        original_positions,
    ))
}

/// port: tsc/internal/contentmapper/hostimpl.go:normalizeDiagnosticDirectives
fn normalize_diagnostic_directives(
    directives: Option<&DiagnosticDirectives>,
    virtual_positions: &PositionNormalizer,
    original_positions: &PositionNormalizer,
    diagnostic_source: &str,
) -> Result<Vec<AstDirective>, Error> {
    let Some(directives) = directives else {
        return Ok(Vec::new());
    };
    let mut result = Vec::with_capacity(directives.directives.len());
    for (index, directive) in directives.directives.iter().enumerate() {
        let error = |kind| Error::DiagnosticDirective {
            kind,
            index,
            supplemental_index: None,
            policy: 0,
        };
        let mut normalized = AstDirective {
            source: JsString::from_bytes(diagnostic_source.as_bytes()),
            ..AstDirective::default()
        };
        match directive.policy {
            DIAGNOSTIC_DIRECTIVE_POLICY_IGNORE => normalized.policy = 0,
            DIAGNOSTIC_DIRECTIVE_POLICY_EXPECT => {
                let unused = &directives.unused_expect_directive_diagnostics;
                let unused_index = match directive.unused_expect_directive_index {
                    Some(index) => index,
                    None if unused.len() != 1 => {
                        return Err(error(DirectiveErrorKind::ExpectMissingUnusedDiagnostic))
                    }
                    None => 0,
                };
                let Some(diagnostic) = usize::try_from(unused_index)
                    .ok()
                    .and_then(|i| unused.get(i))
                else {
                    return Err(error(DirectiveErrorKind::InvalidUnusedDiagnosticIndex));
                };
                normalized.policy = 1;
                normalized.unused_code = diagnostic.code;
                normalized.unused_message_text =
                    JsString::from_bytes(diagnostic.message_text.as_bytes());
            }
            policy => {
                return Err(Error::DiagnosticDirective {
                    kind: DirectiveErrorKind::InvalidPolicy,
                    index,
                    supplemental_index: None,
                    policy,
                })
            }
        }
        if directive.virtual_start < 0 || directive.virtual_end < directive.virtual_start {
            return Err(error(DirectiveErrorKind::InvalidRange));
        }
        let virtual_start = virtual_positions
            .normalize(directive.virtual_start)
            .map_err(|_| error(DirectiveErrorKind::InvalidRange))?;
        let virtual_end = virtual_positions
            .normalize(directive.virtual_end)
            .map_err(|_| error(DirectiveErrorKind::InvalidRange))?;
        normalized.virtual_range = TextRange::new(virtual_start, virtual_end);
        let mut valid_original = directive.original_start >= 0
            && directive.original_length >= 0
            && directive.original_start <= i64::MAX - directive.original_length;
        if valid_original {
            match (
                original_positions.normalize(directive.original_start),
                original_positions.normalize(directive.original_start + directive.original_length),
            ) {
                (Ok(start), Ok(end)) => normalized.original_range = TextRange::new(start, end),
                _ => valid_original = false,
            }
        }
        if normalized.policy == 1 && !valid_original {
            return Err(error(DirectiveErrorKind::InvalidRange));
        }
        result.push(normalized);
    }
    let mut sorted: Vec<(usize, &AstDirective)> = result.iter().enumerate().collect();
    tsr_core::sort_like_go(&mut sorted, &mut |a, b| {
        a.1.virtual_range.pos().cmp(&b.1.virtual_range.pos())
    });
    for pair in sorted.windows(2) {
        if pair[1].1.virtual_range.pos() < pair[0].1.virtual_range.end() {
            return Err(Error::DiagnosticDirective {
                kind: DirectiveErrorKind::Overlap,
                index: pair[1].0,
                supplemental_index: None,
                policy: 0,
            });
        }
    }
    Ok(result)
}

/// port: tsc/internal/contentmapper/hostimpl.go:normalizeMappings
fn normalize_mappings(
    mappings: &SpanMap,
    virtual_positions: &PositionNormalizer,
    original_positions: &PositionNormalizer,
) -> Result<SpanMap, Error> {
    let mut segments = mappings.segments().to_vec();
    for (index, segment) in segments.iter_mut().enumerate() {
        let normalize = |normalizer: &PositionNormalizer, position: i32, what: &str| {
            normalizer.normalize_text_pos(position).map_err(|error| {
                Error::Message(format!(
                    "invalid content mapper mapping {index} {what}: {error}"
                ))
            })
        };
        segment.virtual_start =
            normalize(virtual_positions, segment.virtual_start, "virtual start")?;
        segment.virtual_end = normalize(virtual_positions, segment.virtual_end, "virtual end")?;
        segment.original_start =
            normalize(original_positions, segment.original_start, "original start")?;
        segment.original_end = normalize(original_positions, segment.original_end, "original end")?;
    }
    Ok(SpanMap::new(&segments))
}

/// Converts a mapper's coordinates to byte offsets.
struct PositionNormalizer {
    text: Vec<u8>,
    encoding: String,
    position_map: Option<tsr_jsstring::position_map::PositionMap>,
    length: i64,
}

impl PositionNormalizer {
    /// port: tsc/internal/contentmapper/hostimpl.go:newPositionNormalizer
    fn new(text: &[u8], encoding: &PositionEncoding) -> Result<Self, Error> {
        let length = |value: usize| i64::try_from(value).unwrap_or(i64::MAX);
        match encoding.as_str() {
            PositionEncoding::UTF8 => Ok(Self {
                text: text.to_vec(),
                encoding: encoding.0.clone(),
                position_map: None,
                length: length(text.len()),
            }),
            PositionEncoding::UTF16 => {
                let map = tsr_jsstring::position_map::PositionMap::new(text);
                let utf16_length =
                    map.utf8_to_utf16(isize::try_from(text.len()).unwrap_or(isize::MAX));
                Ok(Self {
                    text: text.to_vec(),
                    encoding: encoding.0.clone(),
                    position_map: Some(map),
                    length: utf16_length as i64,
                })
            }
            other => Err(Error::Message(format!(
                "unsupported position encoding {}",
                tsr_jsstring::go_quote(other.as_bytes())
            ))),
        }
    }

    /// port: tsc/internal/contentmapper/hostimpl.go:positionNormalizer.normalizeTextPos
    fn normalize_text_pos(&self, position: i32) -> Result<i32, String> {
        let normalized = self.normalize(i64::from(position))?;
        // Go converts back to a TextPos (int32).
        #[allow(clippy::cast_possible_truncation)]
        Ok(normalized as i32)
    }

    /// A byte offset for `position`, which must lie within the text and not
    /// split a code point.
    /// port: tsc/internal/contentmapper/hostimpl.go:positionNormalizer.normalize
    fn normalize(&self, position: i64) -> Result<i64, String> {
        if position < 0 {
            return Err(format!("position {position} is negative"));
        }
        if position > self.length {
            return Err(format!(
                "position {position} exceeds {} length {}",
                self.encoding, self.length
            ));
        }
        let byte_position = match &self.position_map {
            None => position,
            Some(map) => map.utf16_to_utf8(isize::try_from(position).unwrap_or(isize::MAX)) as i64,
        };
        let is_continuation = usize::try_from(byte_position)
            .ok()
            .and_then(|index| self.text.get(index))
            .is_some_and(|&byte| byte & 0xC0 == 0x80);
        if is_continuation {
            return Err(format!("position {position} splits a Unicode code point"));
        }
        Ok(byte_position)
    }
}
