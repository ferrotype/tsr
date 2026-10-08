//! The API session: snapshots by handle with reference counts, the open
//! project and file sets, and the request dispatch of the pin's
//! `Session.HandleRequest`. The session shares the LSP server's project
//! session when hosted by it; a standalone session (`tsrust --api`) owns a
//! private project session, which plays the pin's snapshot host: its
//! current snapshot is the compatibility snapshot of the linear
//! `updateSnapshot` chain, and `api_update` is `CloneSnapshot` over it.
//! port: tsc/internal/api/session.go
mod batch;
mod checker;
mod checker_responses;
#[cfg(test)]
mod checker_tests;
mod config;
mod diagnostics;
mod emit;
pub mod handles;
pub mod responses;
#[cfg(test)]
mod responses_tests;
mod service;
#[cfg(test)]
mod service_tests;
mod sources;
#[cfg(test)]
mod tests;

use crate::proto::{
    CreateProgramParams, CreateProgramResponse, GetDefaultProjectForFileParams, InitializeResponse,
    Method, Params, ProjectId, ReleaseParams, SnapshotId, UpdateSnapshotParams,
    UpdateSnapshotResponse, UpdateTemporarySnapshotParams,
};
use crate::server::{next_session_id, Session};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tsr_ipc::{Context, HandlerError, HandlerResult, Response};
use tsr_jsstring::JsString;
use tsr_project::api::ApiSnapshotRequest;
use tsr_project::file_change::FileChangeSummary;
use tsr_project::session::Session as ProjectSession;
use tsr_project::{Project, Snapshot};
use tsr_vfs::FileSystem;

/// The pin's error classes (`ErrClientError`, `ErrInvalidRequest` of
/// tsc/internal/api/proto.go), with their prefixes.
#[derive(Debug)]
pub enum SessionError {
    /// `api: client error: <message>`
    Client(String),
    /// `api: invalid request: <message>`
    InvalidRequest(String),
    /// An error without a class, as the pin's `fmt.Errorf` without `%w`.
    Other(String),
    /// `panic: <message>`: a failure the pin reaches by panicking, which a
    /// batch item reports under this prefix.
    Panic(String),
}
impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client(message) => write!(f, "api: client error: {message}"),
            Self::InvalidRequest(message) => write!(f, "api: invalid request: {message}"),
            Self::Other(message) => f.write_str(message),
            Self::Panic(message) => write!(f, "panic: {message}"),
        }
    }
}
impl std::error::Error for SessionError {}

pub type SessionResult<T> = Result<T, SessionError>;

impl ApiSession {
    /// Arms the panic witness: the next checker operation of `snapshot`
    /// panics inside its operation, so the lease retires the pool
    /// generation as a real checker fault would (ADR 0012). Only the private
    /// test server exposes this.
    #[cfg(feature = "fault-injection")]
    pub fn arm_fault(&self, snapshot: SnapshotId) {
        *self.fault.lock().expect("fault") = Some(snapshot.0);
    }

    /// Trips an armed fault for `snapshot`; called while an operation is held.
    #[cfg(feature = "fault-injection")]
    pub(super) fn trip_fault(&self, snapshot: SnapshotId) {
        let armed = self
            .fault
            .lock()
            .expect("fault")
            .take_if(|armed| *armed == snapshot.0);
        assert!(
            armed.is_none(),
            "injected fault in a checker operation of snapshot {}",
            snapshot.0
        );
    }
}

pub(super) fn client_error(message: impl Into<String>) -> SessionError {
    SessionError::Client(message.into())
}

/// A checker failure as the pin reports it. The pin reaches a type-kind
/// mismatch (`AsTypeReference` on an intrinsic type) by panicking, which a
/// batch item reports as `panic: …`; the Rust accessor refuses it with
/// `Error::UnexpectedType`, which takes the same prefix.
pub(super) fn checker_error<E: std::fmt::Display + std::any::Any>(error: E) -> SessionError {
    let message = format!("{error}");
    if let Some(tsr_checker::Error::UnexpectedType { .. }) =
        (&error as &dyn std::any::Any).downcast_ref::<tsr_checker::Error>()
    {
        return SessionError::Panic(message);
    }
    SessionError::Other(message)
}

/// A snapshot the session holds for its clients, with the registries the
/// checker handlers fill (A3): the pin's `snapshotData` of
/// tsc/internal/api/session.go.
pub struct SnapshotData {
    pub snapshot: Snapshot,
    /// Symbol, type and signature handles minted against this snapshot.
    pub registries: handles::Registries,
}
impl SnapshotData {
    /// port: tsc/internal/api/session.go:snapshotData.getProject
    pub fn project(&self, handle: &ProjectId) -> SessionResult<&Project> {
        self.snapshot
            .project_by_path(handle.0.as_bytes())
            .filter(|project| project.data().is_some())
            .ok_or_else(|| client_error(format!("project {} not found", handle.0)))
    }
    /// port: tsc/internal/api/session.go:snapshotData.getProgram
    pub fn program(&self, handle: &ProjectId) -> SessionResult<&Arc<tsr_compiler::Program>> {
        self.project(handle)?
            .program()
            .ok_or_else(|| client_error("project has no program"))
    }
}

struct Entry {
    data: Arc<SnapshotData>,
    refs: usize,
}

/// A snapshot reference held for one request (the pin's deferred release).
struct RetainedSnapshot<'a> {
    session: &'a ApiSession,
    handle: SnapshotId,
    data: Arc<SnapshotData>,
}
impl Drop for RetainedSnapshot<'_> {
    fn drop(&mut self) {
        let _ = self.session.release_snapshot(self.handle);
    }
}

/// Test hooks on the request path, per thread so parallel tests do not see
/// each other's.
#[cfg(test)]
pub(super) mod hooks {
    use std::cell::RefCell;
    thread_local! {
        static BEFORE_COMMIT: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    }
    /// Runs `hook` on this thread before every response commitment.
    pub fn set_before_commit(hook: Option<Box<dyn Fn()>>) {
        BEFORE_COMMIT.with(|slot| *slot.borrow_mut() = hook);
    }
    pub fn before_commit() {
        BEFORE_COMMIT.with(|slot| {
            if let Some(hook) = slot.borrow().as_ref() {
                hook();
            }
        });
    }
}
#[derive(Default)]
struct Snapshots {
    by_handle: HashMap<u64, Entry>,
    /// The diff base of the next update; zero before the first.
    latest: u64,
}

/// The projects and files this session holds open, by path. One ref each:
/// opens are idempotent and closes release only what the session holds.
#[derive(Default)]
struct OpenRefs {
    projects: BTreeSet<JsString>,
    files: BTreeSet<JsString>,
}

pub struct ApiSession {
    id: String,
    project: Arc<ProjectSession>,
    /// A standalone session owns its project session and closes it.
    standalone: bool,
    use_binary_responses: AtomicBool,
    snapshots: Mutex<Snapshots>,
    /// The pin's `updateMu`: serializes updates and the ref tracking.
    open: Mutex<OpenRefs>,
    /// Remaining pages of paginated batch responses by continuation token.
    batch_pages: Mutex<HashMap<String, Vec<tsr_json::RawValue>>>,
    /// The snapshot whose next checker operation panics (the A5 witness).
    #[cfg(feature = "fault-injection")]
    fault: Mutex<Option<u64>>,
    next_batch_page: std::sync::atomic::AtomicU64,
    closed: AtomicBool,
}

impl ApiSession {
    /// A session over the LSP server's project session.
    /// port: tsc/internal/api/session.go:NewLSPSession
    pub fn for_lsp(project: Arc<ProjectSession>) -> Arc<Self> {
        Arc::new(Self::new(project, false))
    }

    /// A session with its own project session over `fs`.
    /// port: tsc/internal/api/session.go:NewStandaloneSession
    pub fn standalone(
        options: tsr_project::session::SessionOptions,
        fs: Arc<dyn FileSystem>,
    ) -> Arc<Self> {
        let project = ProjectSession::new(options, fs, &tsr_arena::Counters::new());
        Arc::new(Self::new(project, true))
    }

    fn new(project: Arc<ProjectSession>, standalone: bool) -> Self {
        Self {
            id: next_session_id(),
            project,
            standalone,
            use_binary_responses: AtomicBool::new(false),
            snapshots: Mutex::default(),
            open: Mutex::default(),
            batch_pages: Mutex::default(),
            #[cfg(feature = "fault-injection")]
            fault: Mutex::default(),
            next_batch_page: std::sync::atomic::AtomicU64::new(0),
            closed: AtomicBool::new(false),
        }
    }

    pub fn project_session(&self) -> &Arc<ProjectSession> {
        &self.project
    }

    fn binary(&self) -> bool {
        self.use_binary_responses.load(Ordering::Relaxed)
    }

    /// port: tsc/internal/api/session.go:Session.currentDirectory
    fn current_directory(&self) -> &JsString {
        self.project.current_directory()
    }

    /// port: tsc/internal/api/session.go:Session.useCaseSensitiveFileNames
    fn use_case_sensitive_file_names(&self) -> bool {
        self.project.file_system().use_case_sensitive_file_names()
    }

    fn file_system(&self) -> &Arc<dyn FileSystem> {
        self.project.file_system()
    }

    /// port: tsc/internal/api/session.go:Session.toPath
    fn to_path(&self, file_name: &[u8]) -> JsString {
        tsr_tspath::to_path(
            file_name,
            self.current_directory().as_bytes(),
            self.use_case_sensitive_file_names(),
        )
    }

    /// port: tsc/internal/api/session.go:Session.getSnapshotData
    fn snapshot_data(&self, handle: SnapshotId) -> SessionResult<Arc<SnapshotData>> {
        self.snapshots
            .lock()
            .expect("snapshots")
            .by_handle
            .get(&handle.0)
            .map(|entry| entry.data.clone())
            .ok_or_else(|| client_error(format!("snapshot {} not found", handle.0)))
    }

    /// Stores a new snapshot or bumps the reference count of the stored one
    /// with the same id, and returns its data with the previous latest.
    /// Advancing `latest` is the caller's choice (temporary snapshots and
    /// programs do not).
    fn store_snapshot(
        &self,
        snapshot: Snapshot,
        advance_latest: bool,
    ) -> (Arc<SnapshotData>, Option<Arc<SnapshotData>>) {
        let handle = snapshot.id().expect("session snapshot");
        let mut snapshots = self.snapshots.lock().expect("snapshots");
        let data = if let Some(entry) = snapshots.by_handle.get_mut(&handle) {
            entry.refs += 1;
            entry.data.clone()
        } else {
            let data = Arc::new(SnapshotData {
                snapshot,
                registries: handles::Registries::default(),
            });
            snapshots.by_handle.insert(
                handle,
                Entry {
                    data: data.clone(),
                    refs: 1,
                },
            );
            data
        };
        let previous = snapshots
            .by_handle
            .get(&snapshots.latest)
            .map(|entry| entry.data.clone());
        if advance_latest {
            snapshots.latest = handle;
        }
        (data, previous)
    }

    /// port: tsc/internal/api/session.go:Session.releaseSnapshot
    fn release_snapshot(&self, handle: SnapshotId) -> SessionResult<()> {
        let mut snapshots = self.snapshots.lock().expect("snapshots");
        let Some(entry) = snapshots.by_handle.get_mut(&handle.0) else {
            return Err(client_error(format!("snapshot {} not found", handle.0)));
        };
        entry.refs -= 1;
        if entry.refs == 0 {
            snapshots.by_handle.remove(&handle.0);
        }
        Ok(())
    }

    /// port: tsc/internal/api/session.go:Session.toFileChangeSummary
    fn to_file_change_summary(
        &self,
        changes: Option<&crate::proto::ApiFileChanges>,
    ) -> FileChangeSummary {
        let mut summary = FileChangeSummary::default();
        let Some(changes) = changes else {
            return summary;
        };
        if changes.invalidate_all {
            summary.invalidate_all = true;
            summary.includes_watch_change_outside_node_modules = true;
            return summary;
        }
        let cwd = self.current_directory().as_bytes();
        summary.changed = changes.changed.iter().map(|doc| doc.to_uri(cwd)).collect();
        summary.created = changes.created.iter().map(|doc| doc.to_uri(cwd)).collect();
        summary.deleted = changes.deleted.iter().map(|doc| doc.to_uri(cwd)).collect();
        if !summary.changed.is_empty() || !summary.created.is_empty() || !summary.deleted.is_empty()
        {
            summary.includes_watch_change_outside_node_modules = true;
        }
        summary
    }

    /// The project session's update: the pin's `CloneSnapshot` for a
    /// standalone session, `APIUpdate` for an LSP-hosted one. The request
    /// error travels with the snapshot, as the pin's does.
    /// port: tsc/internal/api/session.go:Session.apiUpdate
    fn api_update(
        &self,
        changes: FileChangeSummary,
        request: ApiSnapshotRequest,
    ) -> Result<(Snapshot, Option<JsString>), String> {
        match self.project.api_update(changes, request) {
            Ok(update) => Ok((update.snapshot, update.error)),
            Err(error) => Err(format!("{error}")),
        }
    }

    /// port: tsc/internal/api/session.go:Session.handleInitialize
    fn handle_initialize(&self) -> InitializeResponse {
        InitializeResponse {
            use_case_sensitive_file_names: self.use_case_sensitive_file_names(),
            current_directory: String::from_utf8_lossy(self.current_directory().as_bytes())
                .into_owned(),
        }
    }

    /// port: tsc/internal/api/session.go:Session.handleUpdateSnapshot
    fn handle_update_snapshot(
        &self,
        params: &UpdateSnapshotParams,
    ) -> SessionResult<UpdateSnapshotResponse> {
        // A callback that panics during the update unwinds through this
        // lock; the open sets change only after success, so the next update
        // continues with them, as the pin's deferred unlock lets it.
        let mut open = self
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let file_changes = self.to_file_change_summary(params.file_changes.as_deref());
        let cwd = self.current_directory().clone();
        let mut request = ApiSnapshotRequest::default();
        let mut opened_projects = Vec::new();
        for project in &params.open_projects {
            let config_file_name = project.to_absolute_file_name(cwd.as_bytes());
            let config_path = self.to_path(config_file_name.as_bytes());
            if open.projects.contains(&config_path) {
                continue;
            }
            request
                .open_projects
                .get_or_insert_with(BTreeSet::new)
                .insert(config_file_name);
            opened_projects.push(config_path);
        }
        let mut closed_projects = Vec::new();
        for project in &params.close_projects {
            let config_path =
                self.to_path(project.to_absolute_file_name(cwd.as_bytes()).as_bytes());
            if !open.projects.contains(&config_path) {
                continue;
            }
            request
                .close_projects
                .get_or_insert_with(BTreeSet::new)
                .insert(config_path.clone());
            closed_projects.push(config_path);
        }
        let mut opened_files = Vec::new();
        for file in &params.open_files {
            let uri = file.to_uri(cwd.as_bytes());
            let path = self.to_path(uri.file_name().as_bytes());
            if open.files.contains(&path) {
                continue;
            }
            request
                .open_files
                .get_or_insert_with(BTreeSet::new)
                .insert(uri);
            opened_files.push(path);
        }
        let mut closed_files = Vec::new();
        for file in &params.close_files {
            let path = self.to_path(file.to_uri(cwd.as_bytes()).file_name().as_bytes());
            if !open.files.contains(&path) {
                continue;
            }
            request
                .close_files
                .get_or_insert_with(BTreeSet::new)
                .insert(path.clone());
            closed_files.push(path);
        }
        let (snapshot, error) = self
            .api_update(file_changes, request)
            .map_err(|error| client_error(format!("failed to update snapshot: {error}")))?;
        if let Some(error) = error {
            return Err(client_error(format!(
                "failed to update snapshot: {}",
                String::from_utf8_lossy(error.as_bytes())
            )));
        }
        open.projects.extend(opened_projects);
        for path in &closed_projects {
            open.projects.remove(path);
        }
        open.files.extend(opened_files);
        for path in &closed_files {
            open.files.remove(path);
        }
        let (data, previous) = self.store_snapshot(snapshot, true);
        Ok(update_snapshot_response(
            &data,
            previous.as_deref().map(|previous| &previous.snapshot),
        ))
    }

    /// Pins a snapshot while an operation derives from it.
    /// port: tsc/internal/api/session.go:Session.retainSnapshotData
    /// Takes a reference on a snapshot for the rest of a request; the guard
    /// releases it on return and on unwinding alike, as the pin's deferred
    /// release does when a file-system callback panics mid-derivation.
    fn retain_snapshot(&self, handle: SnapshotId) -> SessionResult<RetainedSnapshot<'_>> {
        let mut snapshots = self.snapshots.lock().expect("snapshots");
        let Some(entry) = snapshots.by_handle.get_mut(&handle.0) else {
            return Err(client_error(format!("snapshot {} not found", handle.0)));
        };
        entry.refs += 1;
        Ok(RetainedSnapshot {
            session: self,
            handle,
            data: entry.data.clone(),
        })
    }

    /// A snapshot with one file's content overridden; it never becomes the
    /// session's latest, and its changes are relative to the client's base.
    /// port: tsc/internal/api/session.go:Session.handleUpdateTemporarySnapshot
    fn handle_update_temporary_snapshot(
        &self,
        params: &UpdateTemporarySnapshotParams,
    ) -> SessionResult<UpdateSnapshotResponse> {
        let base = self.retain_snapshot(params.snapshot)?;
        let uri = params.file.to_uri(self.current_directory().as_bytes());
        let snapshot = self
            .project
            .clone_with_temporary_file(
                &base.data.snapshot,
                &uri,
                JsString::from_bytes(params.new_text.as_bytes()),
            )
            .map_err(|error| {
                client_error(format!("failed to update temporary snapshot: {error}"))
            })?;
        let (data, _) = self.store_snapshot(snapshot, false);
        Ok(update_snapshot_response(&data, Some(&base.data.snapshot)))
    }

    /// port: tsc/internal/api/session.go:Session.handleCreateProgram
    fn handle_create_program(
        &self,
        params: &CreateProgramParams,
    ) -> SessionResult<CreateProgramResponse> {
        if params.file_changes.is_some() && params.old_program.is_none() {
            return Err(client_error("fileChanges requires an oldProgram"));
        }
        let cwd = self.current_directory().clone();
        let root_file_names: Vec<JsString> = params
            .root_files
            .iter()
            .map(|root| root.to_absolute_file_name(cwd.as_bytes()))
            .collect();
        let mut retained = None;
        let mut old_project = None;
        if let Some(old) = &params.old_program {
            retained = Some(self.retain_snapshot(old.snapshot)?);
            old_project = Some(old.project.clone());
        }
        let result = (|| {
            let old = retained.as_ref().map(|retained| &retained.data);
            let old_project = match (old, &old_project) {
                (Some(data), Some(handle)) => Some(data.project(handle)?.clone()),
                _ => None,
            };
            let mut file_changes = self.to_file_change_summary(params.file_changes.as_deref());
            let fresh;
            let base = if let Some(data) = old {
                &data.snapshot
            } else {
                let (snapshot, error) = self
                    .api_update(
                        std::mem::take(&mut file_changes),
                        ApiSnapshotRequest::default(),
                    )
                    .map_err(|error| client_error(format!("failed to update snapshot: {error}")))?;
                if let Some(error) = error {
                    return Err(client_error(format!(
                        "failed to update snapshot: {}",
                        String::from_utf8_lossy(error.as_bytes())
                    )));
                }
                fresh = snapshot;
                &fresh
            };
            let options = &params.create_program_options;
            let request = tsr_project::session::ProgramRequest {
                root_file_names,
                options: options.compiler_options.0.clone(),
                project_references: (!options.project_references.is_empty()).then(|| {
                    options
                        .project_references
                        .iter()
                        .flatten()
                        .map(|reference| reference.0.clone())
                        .collect()
                }),
                config_file_parsing_diagnostics: options
                    .config_file_parsing_diagnostics
                    .iter()
                    .flatten()
                    .map(|diagnostic| responses::to_diagnostic(diagnostic))
                    .collect(),
            };
            let snapshot = self
                .project
                .clone_for_program(base, request, old_project.as_ref(), file_changes)
                .map_err(|error| {
                    client_error(format!("failed to create synthetic project: {error}"))
                })?;
            let (data, _) = self.store_snapshot(snapshot, false);
            let project = data
                .snapshot
                .project_by_path(tsr_project::project::INFERRED_PROJECT_NAME)
                .filter(|project| project.data().is_some())
                .map(responses::project_response)
                .ok_or_else(|| client_error("failed to create synthetic project"))?;
            Ok(CreateProgramResponse {
                snapshot: SnapshotId(data.snapshot.id().expect("session snapshot")),
                project: Some(Box::new(project)),
            })
        })();
        drop(retained);
        result
    }

    /// port: tsc/internal/api/session.go:Session.handleRelease
    fn handle_release(&self, params: &ReleaseParams) -> SessionResult<bool> {
        if params.snapshot.0 == 0 {
            return Err(client_error("empty handle"));
        }
        self.release_snapshot(params.snapshot)?;
        Ok(true)
    }

    /// port: tsc/internal/api/session.go:Session.handleGetDefaultProjectForFile
    fn handle_get_default_project_for_file(
        &self,
        params: &GetDefaultProjectForFileParams,
    ) -> SessionResult<Option<crate::proto::ProjectResponse>> {
        let data = self.snapshot_data(params.snapshot)?;
        let uri = params.file.to_uri(self.current_directory().as_bytes());
        let path = self.to_path(uri.file_name().as_bytes());
        Ok(data
            .snapshot
            .project_for_file(path.as_bytes())
            .filter(|project| project.data().is_some())
            .map(responses::project_response))
    }

    /// Every project and file held open is released through one update of
    /// closes; standalone sessions release their whole project session.
    /// port: tsc/internal/api/session.go:Session.releaseOpenRefs
    fn release_open_refs(&self) {
        let mut open = self
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if open.projects.is_empty() && open.files.is_empty() {
            return;
        }
        if self.standalone {
            open.projects.clear();
            open.files.clear();
            return;
        }
        let request = ApiSnapshotRequest {
            close_projects: (!open.projects.is_empty()).then(|| open.projects.clone()),
            close_files: (!open.files.is_empty()).then(|| open.files.clone()),
            ..Default::default()
        };
        if self
            .api_update(FileChangeSummary::default(), request)
            .is_ok()
        {
            open.projects.clear();
            open.files.clear();
        }
    }

    pub(super) fn dispatch(
        &self,
        ctx: &Context,
        method: &str,
        payload: &[u8],
    ) -> Result<Option<Response>, HandlerError> {
        match method {
            // The batch decodes its own items: an unknown method inside one is
            // answered in that item, not refused for the whole request.
            "batchRequests" => {
                return self
                    .handle_batch_requests(ctx, payload)
                    .map(Some)
                    .map_err(Into::into)
            }
            "echo" => {
                return Ok(Some(if self.binary() {
                    Response::binary(payload.to_vec())
                } else {
                    Response::json(tsr_json::RawValue(payload.to_vec()))
                }));
            }
            "ping" => return Ok(Some(Response::json("pong".to_string()))),
            _ => {}
        }
        // The pin distinguishes a nil file list from an empty one: an omitted
        // `files` means every file (diagnostics) or is refused (selected
        // emit), while `[]` names no file.
        let files_named = matches!(
            method,
            "getSyntacticDiagnostics"
                | "getBindDiagnostics"
                | "getSemanticDiagnostics"
                | "getSuggestionDiagnostics"
                | "getDeclarationDiagnostics"
                | "getJavaScriptEmit"
                | "getDeclarationEmit"
        ) && diagnostics::names_files(payload);
        let Some(known) = Method::from_wire(method) else {
            return Err(crate::server::unsupported(method));
        };
        let params = Params::decode(known, payload)
            .map_err(|error| SessionError::InvalidRequest(format!("{error}")))?;
        let response = match params {
            Params::Initialize => Response::json(self.handle_initialize()),
            Params::UpdateSnapshot(params) => Response::json(self.handle_update_snapshot(&params)?),
            Params::Release(params) => Response::json(self.handle_release(&params)?),
            Params::UpdateTemporarySnapshot(params) => {
                Response::json(self.handle_update_temporary_snapshot(&params)?)
            }
            Params::CreateProgram(params) => Response::json(self.handle_create_program(&params)?),
            Params::GetDefaultProjectForFile(params) => {
                Response::json(self.handle_get_default_project_for_file(&params)?)
            }
            Params::ParseCommandLine(params) => {
                Response::json(self.handle_parse_command_line(&params))
            }
            Params::ReadConfigFile(params) => Response::json(self.handle_read_config_file(&params)),
            Params::ParseJsonConfigFile(params) => {
                Response::json(self.handle_parse_json_config_file_content(&params)?)
            }
            Params::ParseConfigFile(params) => {
                Response::json(self.handle_parse_config_file(&params)?)
            }
            Params::TranspileModule(params) => {
                Response::json(Self::handle_transpile(&params, false)?)
            }
            Params::TranspileDeclaration(params) => {
                Response::json(Self::handle_transpile(&params, true)?)
            }
            Params::TranspileModuleFromFile(params) => {
                Response::json(self.handle_transpile_from_file(&params, false)?)
            }
            Params::TranspileDeclarationFromFile(params) => {
                Response::json(self.handle_transpile_from_file(&params, true)?)
            }
            Params::GetSourceFile(params) => {
                return self
                    .handle_get_source_file(&params)
                    .map(Some)
                    .map_err(Into::into)
            }
            Params::GetSourceFileNames(params) => {
                Response::json(self.handle_get_source_file_names(&params)?)
            }
            Params::GetSourceFileMetadata(params) => {
                Response::json(self.handle_get_source_file_metadata(&params)?)
            }
            Params::GetConfigFileNames(params) => {
                Response::json(self.handle_get_config_file_names(&params)?)
            }
            Params::GetConfigSourceFile(params) => {
                return self
                    .handle_get_config_source_file(&params)
                    .map(Some)
                    .map_err(Into::into)
            }
            other => return self.dispatch_checker(ctx, other, method, files_named),
        };
        Ok(Some(response))
    }
}

/// port: tsc/internal/api/session.go:Session.handleUpdateSnapshot
fn update_snapshot_response(
    data: &SnapshotData,
    previous: Option<&Snapshot>,
) -> UpdateSnapshotResponse {
    let projects = data
        .snapshot
        .projects()
        .into_iter()
        .filter(|project| project.data().is_some())
        .map(|project| Some(Box::new(responses::project_response(project))))
        .collect();
    UpdateSnapshotResponse {
        snapshot: SnapshotId(data.snapshot.id().expect("session snapshot")),
        projects,
        changes: previous.map(|previous| {
            Box::new(responses::compute_snapshot_changes(
                previous,
                &data.snapshot,
            ))
        }),
    }
}

impl Session for ApiSession {
    fn id(&self) -> &str {
        &self.id
    }
    /// port: tsc/internal/api/session.go:Session.HandleRequest
    fn handle_request(&self, ctx: &Context, method: &str, params: &[u8]) -> HandlerResult {
        self.dispatch(ctx, method, params)
    }
    fn set_binary_responses(&self, enabled: bool) {
        self.use_binary_responses.store(enabled, Ordering::Relaxed);
    }
    /// port: tsc/internal/api/session.go:Session.Close
    fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.release_open_refs();
        self.snapshots.lock().expect("snapshots").by_handle.clear();
        self.batch_pages.lock().expect("batch pages").clear();
        if self.standalone {
            self.project.close();
        }
    }
}
