//! Session transactions publish complete snapshots. File notifications queue in
//! arrival order; the request barrier applies them before selecting a project.
use crate::{
    config::{ConfigFileRegistry, ConfigRegistryBuilder},
    extended_config::{ConfigOwnership, ExtendedConfigCache},
    file_change::{FileChange, FileChangeKind, FileChangeSummary},
    overlay::OverlayFs,
    parse_cache::{ContentMappedParseCache, ParseCache},
    program_counter::ProgramCounter,
    ref_count_cache::RefCountCacheOptions,
    snapshot::SessionSnapshot,
    snapshot_fs::{SnapshotFs, SnapshotFsBuilder},
    Project, Snapshot,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock, RwLock,
    },
};
use tsr_arena::Counters;
use tsr_core::CompilerOptions;
use tsr_jsstring::{JsString, PositionEncoding};
use tsr_lsproto::{DocumentUri, LanguageKind, TextDocumentContentChangePartialOrWholeDocument};
use tsr_vfs::FileSystem;

mod api;
mod ata;
mod build;
#[cfg(test)]
mod tests;
mod timers;

#[derive(Clone)]
pub struct SessionOptions {
    pub current_directory: JsString,
    pub default_library_path: JsString,
    pub position_encoding: PositionEncoding,
    pub run_external_code: bool,
    pub background_context: tsr_ipc::Context,
    pub typings_location: JsString,
    pub npm_executor: Option<Arc<dyn crate::ata::NpmExecutor>>,
    pub disable_automatic_type_acquisition: bool,
    pub mapper_spawner: Option<Arc<dyn tsr_contentmapper::Spawner>>,
    pub mapper_logger: Option<tsr_contentmapper::Logger>,
    pub locale: tsr_locale::Locale,
    pub query_checkers: usize,
    pub relative_watch_patterns: bool,
    pub debounce_delay: std::time::Duration,
    pub logger: crate::logging::Logger,
}

/// Publication and loading events for the server. Sending is nonblocking and
/// invokes no client code under the session's update lock. The receiver owns
/// delivery; snapshots retain all files used to format the notifications.
pub enum SessionEvent {
    InstallingTypes {
        name: JsString,
        finished: bool,
    },
    ProjectLoading {
        name: JsString,
        finished: bool,
    },
    Published {
        previous: Snapshot,
        current: Snapshot,
    },
    DiagnosticsRefresh {
        // Remains cancellable while queued for the server's event observer.
        cancellation: tsr_core::CancellationToken,
    },
    Closed,
}
impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/".as_slice()),
            position_encoding: PositionEncoding::Utf16,
            run_external_code: false,
            background_context: tsr_ipc::Context::background(),
            typings_location: JsString::default(),
            npm_executor: None,
            disable_automatic_type_acquisition: false,
            mapper_spawner: None,
            mapper_logger: None,
            locale: tsr_locale::Locale::default(),
            query_checkers: 3,
            relative_watch_patterns: false,
            debounce_delay: std::time::Duration::ZERO,
            logger: crate::logging::Logger::nop(),
        }
    }
}
#[derive(Debug)]
pub enum Error {
    Closed,
    NoProjectForUnknownScriptKind,
    /// A temporary file whose name has no known script kind.
    UnsupportedFileExtension(JsString),
    Compiler(tsr_compiler::Error),
    Host(tsr_vfs::Error),
}
impl From<tsr_compiler::Error> for Error {
    fn from(value: tsr_compiler::Error) -> Self {
        Self::Compiler(value)
    }
}
impl From<tsr_vfs::Error> for Error {
    fn from(value: tsr_vfs::Error) -> Self {
        Self::Host(value)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("project session is closed"),
            Self::NoProjectForUnknownScriptKind => {
                f.write_str("no project for unknown script kind")
            }
            Self::UnsupportedFileExtension(name) => write!(
                f,
                "unsupported file extension: {}",
                String::from_utf8_lossy(name.as_bytes())
            ),
            Self::Compiler(error) => error.fmt(f),
            Self::Host(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for Error {}

#[derive(Default)]
struct Pending {
    mapper_locale_changed: bool,
    ata_changes: BTreeMap<JsString, ata::AtaChange>,
    ata_changed: bool,
    changes: Vec<FileChange>,
    inferred: Option<Arc<CompilerOptions>>,
    custom_name: Option<JsString>,
    validation_enabled: Option<bool>,
    contributions: Option<Arc<crate::content_mappers::Contributions>>,
}
impl Pending {
    fn is_empty(&self) -> bool {
        self.ata_changes.is_empty()
            && !self.ata_changed
            && !self.mapper_locale_changed
            && self.changes.is_empty()
            && self.inferred.is_none()
            && self.custom_name.is_none()
            && self.validation_enabled.is_none()
            && self.contributions.is_none()
    }
}

pub struct Session {
    ata: ata::BackgroundAta,
    ata_disabled: AtomicBool,
    options: SessionOptions,
    mapper_host: Option<crate::content_mappers::MapperHost>,
    context: tsr_ipc::Context,
    fs: Arc<dyn FileSystem>,
    counters: Counters,
    parse_cache: Arc<ParseCache>,
    mapped_parse_cache: Arc<ContentMappedParseCache>,
    extended_cache: Arc<ExtendedConfigCache>,
    program_counter: Arc<ProgramCounter>,
    snapshot: RwLock<Option<Snapshot>>,
    update: Mutex<()>,
    pending: Mutex<Pending>,
    watches: OnceLock<Arc<crate::watch::WatchManager>>,
    auto_import_watches: Mutex<crate::watch::WatchSet>,
    events: OnceLock<std::sync::mpsc::Sender<SessionEvent>>,
    timers: timers::Timers,
}
static NEXT_SNAPSHOT: AtomicU64 = AtomicU64::new(1);
fn next_snapshot_id() -> u64 {
    NEXT_SNAPSHOT
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .expect("snapshot identity exhausted")
}
impl Session {
    // port: tsc/internal/project/session.go:NewSession
    pub fn new(options: SessionOptions, fs: Arc<dyn FileSystem>, counters: &Counters) -> Arc<Self> {
        Self::with_parse_cache(
            options,
            fs,
            counters,
            Arc::new(ParseCache::new(RefCountCacheOptions::default())),
        )
    }
    /// The retained test cache is supplied explicitly, as by the native harness.
    pub fn with_parse_cache(
        options: SessionOptions,
        fs: Arc<dyn FileSystem>,
        counters: &Counters,
        parse_cache: Arc<ParseCache>,
    ) -> Arc<Self> {
        Self::with_caches(
            options,
            fs,
            counters,
            parse_cache,
            Arc::new(ContentMappedParseCache::new(RefCountCacheOptions::default())),
        )
    }
    /// Both caches may be injected by a batch worker. Production sessions use
    /// deletion-on-last-release; only the private test worker retains entries.
    pub fn with_caches(
        options: SessionOptions,
        fs: Arc<dyn FileSystem>,
        counters: &Counters,
        parse_cache: Arc<ParseCache>,
        mapped_parse_cache: Arc<ContentMappedParseCache>,
    ) -> Arc<Self> {
        Self::with_clock(
            options,
            fs,
            counters,
            parse_cache,
            mapped_parse_cache,
            crate::clock::system(),
        )
    }
    fn with_clock(
        options: SessionOptions,
        fs: Arc<dyn FileSystem>,
        counters: &Counters,
        parse_cache: Arc<ParseCache>,
        mapped_parse_cache: Arc<ContentMappedParseCache>,
        clock: Arc<dyn crate::clock::Clock>,
    ) -> Arc<Self> {
        assert!(
            options.query_checkers > 0,
            "a session needs a query checker"
        );
        let extended_cache = Arc::new(ExtendedConfigCache::default());
        let id = next_snapshot_id();
        let state = SessionSnapshot {
            id,
            parent: 0,
            fs: SnapshotFs::empty(fs.clone(), options.current_directory.clone()),
            configs: Arc::new(ConfigFileRegistry::default()),
            projects: BTreeMap::new(),
            delayed_projects: BTreeMap::new(),
            defaults: BTreeMap::new(),
            contributions: Arc::new(crate::content_mappers::Contributions::default()),
            api_state: crate::api::ApiState::default(),
            api_error: None,
            inferred_options: None,
            validation_enabled: true,
            config_ownership: Arc::new(ConfigOwnership::new(extended_cache.clone(), id)),
            _programs: Vec::new(),
        };
        let context = options.background_context.with_cancel();
        let mapper_host = crate::content_mappers::MapperHost::new(
            options.run_external_code,
            options.mapper_spawner.clone(),
            &context,
            options.locale.clone(),
            options.mapper_logger.clone(),
        );
        Arc::new_cyclic(|weak| Self {
            ata: ata::BackgroundAta::new(weak.clone(), &options, fs.clone()),
            ata_disabled: AtomicBool::new(options.disable_automatic_type_acquisition),
            mapper_host,
            context,
            timers: timers::Timers::new(weak.clone(), clock),
            options,
            fs,
            counters: counters.clone(),
            parse_cache,
            mapped_parse_cache,
            extended_cache,
            program_counter: Arc::default(),
            snapshot: RwLock::new(Some(Snapshot::from_session(state))),
            update: Mutex::new(()),
            pending: Mutex::default(),
            watches: OnceLock::new(),
            auto_import_watches: Mutex::default(),
            events: OnceLock::new(),
        })
    }
    // port: tsc/internal/project/session.go:Session.Snapshot
    /// The directory relative paths resolve against.
    /// port: tsc/internal/project/snapshothost.go:SnapshotHost.GetCurrentDirectory
    pub fn current_directory(&self) -> &JsString {
        &self.options.current_directory
    }
    /// The encoding of the session's LSP positions, which its snapshots'
    /// converters and language services translate with: the pin's
    /// `SessionOptions.PositionEncoding`.
    pub fn position_encoding(&self) -> PositionEncoding {
        self.options.position_encoding
    }
    /// The file system the session reads; the API session's `initialize`
    /// reports its case sensitivity.
    pub fn file_system(&self) -> &Arc<dyn FileSystem> {
        &self.fs
    }
    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        self.snapshot
            .read()
            .expect("session snapshot")
            .clone()
            .ok_or(Error::Closed)
    }
    /// Match the snapshot registry's one watcher over existing node_modules
    /// directories in open files' ancestor chains. Keep auxiliary read tracking
    /// for cache invalidation, separate from this client-visible watch contract.
    pub fn sync_auto_import_watches(&self) {
        let Some(manager) = self.watches.get() else {
            return;
        };
        let mut registered = self.auto_import_watches.lock().unwrap();
        let Ok(snapshot) = self.snapshot() else {
            return;
        };
        let Some(state) = snapshot.state() else {
            return;
        };
        let key = JsString::from_bytes(b"auto-import".as_slice());
        let watch = registered.get(&key).cloned().unwrap_or_else(|| {
            crate::watch::WatchedFiles::new(
                key.clone(),
                crate::watch::ALL_CHANGES,
                self.options.relative_watch_patterns,
            )
        });
        let mut directories = BTreeMap::new();
        for file in state.fs.overlays().values() {
            let name = file.file_name().as_bytes();
            if tsr_tspath::is_dynamic_file_name(name) {
                continue;
            }
            let mut directory = name.to_vec();
            loop {
                let parent = tsr_tspath::directory(&directory);
                if parent == directory {
                    break;
                }
                directory = parent;
                let path = state.fs.path(&directory);
                if directories.insert(path, directory.clone()).is_some() {
                    break;
                }
            }
        }
        let mut patterns = Vec::new();
        for directory in directories.values() {
            let modules = tsr_tspath::combine(directory, &[b"node_modules"]);
            // Native DirectoryExists returns false on filesystem errors.
            if state.fs.directory_exists(&modules).unwrap_or(false) {
                patterns.push(JsString::from_bytes(
                    [modules.as_slice(), b"/**/*"].concat(),
                ));
            }
        }
        patterns.sort();
        let next = BTreeMap::from([(
            key,
            watch.with_input(crate::watch::PatternsAndIgnored {
                patterns_inside_workspace: patterns,
                ..Default::default()
            }),
        )]);
        manager.enqueue(registered.clone(), next.clone());
        *registered = next;
    }
    pub fn subscribe(&self) -> std::sync::mpsc::Receiver<SessionEvent> {
        let (send, receive) = std::sync::mpsc::channel();
        assert!(
            self.events.set(send).is_ok(),
            "session already has an event subscriber"
        );
        receive
    }
    fn send_event(&self, event: SessionEvent) {
        if let Some(events) = self.events.get() {
            let _ = events.send(event);
        }
    }
    #[must_use]
    pub fn with_watch_client(
        self: Arc<Self>,
        client: Arc<dyn crate::watch::WatchClient>,
    ) -> Arc<Self> {
        assert!(
            self.snapshot().expect("open session").projects().is_empty(),
            "install the watch client before opening projects"
        );
        assert!(
            self.watches
                .set(crate::watch::WatchManager::new(client))
                .is_ok(),
            "watch client already installed"
        );
        self
    }
    pub fn wait_for_background_tasks(&self) {
        self.timers.wait();
        self.ata.wait();
        self.timers.wait();
        if let Some(watches) = self.watches.get() {
            watches.wait();
        }
    }
    pub(super) fn enqueue_background(
        &self,
        task: impl FnOnce(tsr_core::CancellationToken) + Send + 'static,
    ) -> bool {
        self.timers.enqueue(task)
    }
    pub fn take_background_errors(&self) -> Vec<Error> {
        self.timers.take_errors()
    }
    pub fn take_watch_errors(&self) -> Vec<String> {
        self.watches
            .get()
            .map_or_else(Vec::new, |watches| watches.take_errors())
    }
    pub fn parse_cache(&self) -> &Arc<ParseCache> {
        &self.parse_cache
    }
    pub fn mapped_parse_cache(&self) -> &Arc<ContentMappedParseCache> {
        &self.mapped_parse_cache
    }
    pub fn extended_config_cache(&self) -> &Arc<ExtendedConfigCache> {
        &self.extended_cache
    }
    pub fn program_counter(&self) -> &Arc<ProgramCounter> {
        &self.program_counter
    }
    pub fn enqueue(&self, change: FileChange) -> Result<(), Error> {
        let current = self.snapshot.read().expect("session snapshot");
        let snapshot = current.as_ref().ok_or(Error::Closed)?;
        let update =
            change.kind == FileChangeKind::Close
                || change.kind.is_watch()
                    && snapshot.state().unwrap().configs.configs.contains_key(
                        &tsr_tspath::to_path(
                            change.uri.file_name().as_bytes(),
                            self.options.current_directory.as_bytes(),
                            self.fs.use_case_sensitive_file_names(),
                        ),
                    );
        if change.kind == FileChangeKind::Change {
            let (extensions, _) = snapshot.state().unwrap().mapper_watch_state();
            if tsr_tspath::file_extension_is_one_of(change.uri.file_name().as_bytes(), &extensions)
            {
                self.timers.schedule(
                    timers::Kind::DiagnosticsRefresh,
                    self.options.debounce_delay,
                );
            } else {
                self.timers.cancel(timers::Kind::DiagnosticsRefresh);
            }
        }
        self.pending
            .lock()
            .expect("session events")
            .changes
            .push(change);
        drop(current);
        self.timers
            .schedule(timers::Kind::IdleClean, std::time::Duration::from_secs(30));
        if update {
            self.schedule_snapshot_update();
        }
        Ok(())
    }
    // port: tsc/internal/project/session.go:Session.DidChangeWatchedFiles
    pub fn did_change_watched_files(
        &self,
        changes: impl IntoIterator<Item = tsr_lsproto::FileEvent>,
    ) -> Result<(), Error> {
        let snapshot = self.snapshot()?;
        let state = snapshot.state().unwrap();
        let (mapper_extensions, mapper_watched_files) = state.mapper_watch_state();
        let mut pending = Vec::new();
        let mut relevant = false;
        let mut config = false;
        for event in changes {
            let kind = match event.r#type.0 {
                1 => FileChangeKind::WatchCreate,
                2 => FileChangeKind::WatchChange,
                3 => FileChangeKind::WatchDelete,
                _ => continue,
            };
            let name = event.uri.file_name();
            let path = tsr_tspath::to_path(
                name.as_bytes(),
                self.options.current_directory.as_bytes(),
                self.fs.use_case_sensitive_file_names(),
            );
            config |=
                state.configs.configs.contains_key(&path) || mapper_watched_files.contains(&path);
            relevant |= mapper_watched_files.contains(&path);
            if !relevant {
                let path = tsr_tspath::remove_trailing_directory_separator(path.as_bytes());
                let ext = path
                    .rsplit(|b| *b == b'/')
                    .next()
                    .unwrap_or_default()
                    .iter()
                    .rposition(|b| *b == b'.');
                if ext.is_none() {
                    relevant = if kind == FileChangeKind::WatchDelete {
                        state.fs.has_cached_directory(path)
                            || crate::snapshot_fs::is_node_modules_path(path)
                    } else {
                        self.fs.directory_exists(name.as_bytes())?
                    };
                } else {
                    relevant = crate::snapshot_fs::has_relevant_extension(path);
                }
                relevant |= tsr_tspath::file_extension_is_one_of(path, &mapper_extensions);
            }
            pending.push(FileChange::new(kind, event.uri));
        }
        // Hold the snapshot lock only for admission, never for live host I/O.
        let current = self.snapshot.read().expect("session snapshot");
        if current.is_none() {
            return Err(Error::Closed);
        }
        self.pending
            .lock()
            .expect("session events")
            .changes
            .extend(pending);
        if relevant {
            self.timers.schedule(
                timers::Kind::DiagnosticsRefresh,
                self.options.debounce_delay,
            );
        }
        if config {
            self.schedule_snapshot_update();
        }
        self.timers
            .schedule(timers::Kind::IdleClean, std::time::Duration::from_secs(30));
        Ok(())
    }
    // port: tsc/internal/project/session.go:Session.ScheduleSnapshotUpdate
    pub fn schedule_snapshot_update(&self) {
        self.timers
            .schedule(timers::Kind::Update, self.options.debounce_delay);
    }
    // port: tsc/internal/project/session.go:Session.DidOpenFile
    pub fn did_open_file(
        &self,
        uri: DocumentUri,
        version: i32,
        content: JsString,
        language: LanguageKind,
    ) -> Result<Snapshot, Error> {
        let mut change = FileChange::new(FileChangeKind::Open, uri);
        change.version = version;
        change.content = content;
        change.language = language;
        self.enqueue(change)?;
        self.flush(None)
    }
    // port: tsc/internal/project/session.go:Session.DidChangeFile
    pub fn did_change_file(
        &self,
        uri: DocumentUri,
        version: i32,
        changes: Vec<TextDocumentContentChangePartialOrWholeDocument>,
    ) -> Result<(), Error> {
        let mut change = FileChange::new(FileChangeKind::Change, uri);
        change.version = version;
        change.changes = changes;
        self.enqueue(change)
    }
    // port: tsc/internal/project/session.go:Session.DidCloseFile
    pub fn did_close_file(&self, uri: DocumentUri) -> Result<(), Error> {
        self.enqueue(FileChange::new(FileChangeKind::Close, uri))
    }
    // port: tsc/internal/project/session.go:Session.DidChangeCompilerOptionsForInferredProjects
    pub fn set_inferred_options(&self, options: CompilerOptions) -> Result<(), Error> {
        self.apply_inferred_options(options, self.fs.clone())
    }
    pub fn apply_inferred_options(
        &self,
        options: CompilerOptions,
        host: Arc<dyn FileSystem>,
    ) -> Result<(), Error> {
        let current = self.snapshot.read().expect("session snapshot");
        if current.is_none() {
            return Err(Error::Closed);
        }
        let options = Arc::new(options);
        let previous = self
            .pending
            .lock()
            .expect("session events")
            .inferred
            .replace(options.clone());
        drop(current);
        let result = self.flush_with_host(None, host).map(|_| ());
        if result.is_err() {
            let mut pending = self.pending.lock().expect("session events");
            if pending
                .inferred
                .as_ref()
                .is_some_and(|value| Arc::ptr_eq(value, &options))
            {
                pending.inferred = previous;
            }
        }
        result
    }
    /// Like Configure, stage the publication policy for the next snapshot.
    /// A configuration notification does not itself publish program diagnostics.
    pub fn set_validation_enabled(&self, enabled: bool) -> Result<(), Error> {
        let current = self.snapshot.read().expect("session snapshot");
        if current.is_none() {
            return Err(Error::Closed);
        }
        self.pending
            .lock()
            .expect("session events")
            .validation_enabled = Some(enabled);
        Ok(())
    }
    pub fn set_custom_config_file_name(&self, name: JsString) -> Result<(), Error> {
        let current = self.snapshot.read().expect("session snapshot");
        if current.is_none() {
            return Err(Error::Closed);
        }
        self.pending.lock().expect("session events").custom_name = Some(name);
        drop(current);
        // Keep newConfig pending until the next snapshot update, so its
        // requesting document participates in default-project invalidation.
        self.schedule_snapshot_update();
        Ok(())
    }
    /// Returns the snapshot as well as the selection: callers retain its program,
    /// config and filesystem roots for the complete request.
    // port: tsc/internal/project/session.go:Session.SetContentMapperContributions
    pub fn set_content_mapper_contributions(
        &self,
        contributions: crate::content_mappers::Contributions,
        documents: Vec<DocumentUri>,
    ) -> Result<Snapshot, Error> {
        if !self.options.run_external_code {
            return self.snapshot();
        }
        self.pending.lock().expect("session events").contributions = Some(Arc::new(contributions));
        self.flush_resources(
            &crate::api::ResourceRequest {
                configured_documents: documents,
                ..Default::default()
            },
            self.fs.clone(),
        )
    }
    pub fn set_locale(&self, locale: tsr_locale::Locale) {
        if let Some(host) = &self.mapper_host {
            if host.locale() != locale.to_string() {
                host.set_locale(locale);
                self.pending
                    .lock()
                    .expect("session events")
                    .mapper_locale_changed = true;
            }
        }
    }
    pub fn snapshot_for_file(&self, uri: &DocumentUri) -> Result<Snapshot, Error> {
        let snapshot = self.flush(Some(uri))?;
        let path = tsr_tspath::to_path(
            uri.file_name().as_bytes(),
            self.options.current_directory.as_bytes(),
            self.fs.use_case_sensitive_file_names(),
        );
        if snapshot.project_for_file(path.as_bytes()).is_none() {
            return Err(Error::NoProjectForUnknownScriptKind);
        }
        Ok(snapshot)
    }
    pub fn flush(&self, requested: Option<&DocumentUri>) -> Result<Snapshot, Error> {
        self.flush_with_host(requested, self.fs.clone())
    }
    /// Use the same injected filesystem with a request-scoped cancellation
    /// handle. It must describe the session's filesystem, not a different tree.
    pub fn flush_with_host(
        &self,
        requested: Option<&DocumentUri>,
        host: Arc<dyn FileSystem>,
    ) -> Result<Snapshot, Error> {
        self.flush_inner(requested, false, None, host)
    }
    fn flush_inner(
        &self,
        requested: Option<&DocumentUri>,
        clean_disk: bool,
        timer: Option<(timers::Kind, u64)>,
        host: Arc<dyn FileSystem>,
    ) -> Result<Snapshot, Error> {
        let resources = requested.cloned().map_or_else(
            crate::api::ResourceRequest::default,
            crate::api::ResourceRequest::document,
        );
        self.update_snapshot(&resources, clean_disk, timer, host)
    }
    pub fn flush_resources(
        &self,
        resources: &crate::api::ResourceRequest,
        host: Arc<dyn FileSystem>,
    ) -> Result<Snapshot, Error> {
        self.update_snapshot(resources, false, None, host)
    }
    // port: tsc/internal/project/api.go:Session.APIUpdate
    pub fn api_update(
        &self,
        changes: FileChangeSummary,
        request: crate::api::ApiSnapshotRequest,
    ) -> Result<crate::api::ApiUpdate, Error> {
        let snapshot = self.flush_resources(
            &crate::api::ResourceRequest {
                api: Some(request),
                watch_changes: changes,
                ..Default::default()
            },
            self.fs.clone(),
        )?;
        let error = snapshot.state().unwrap().api_error.clone();
        Ok(crate::api::ApiUpdate { snapshot, error })
    }
    fn update_snapshot(
        &self,
        resources: &crate::api::ResourceRequest,
        clean_disk: bool,
        timer: Option<(timers::Kind, u64)>,
        host: Arc<dyn FileSystem>,
    ) -> Result<Snapshot, Error> {
        // Serialize construction, while current-snapshot reads and notification
        // admission remain available during a synchronous filesystem callback.
        let _update = self
            .update
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = self.snapshot()?;
        assert_eq!(
            host.use_case_sensitive_file_names(),
            self.fs.use_case_sensitive_file_names(),
            "request host case sensitivity changed"
        );
        if let Some((kind, epoch)) = timer {
            if !self.timers.matches(kind, epoch) {
                return Ok(previous);
            }
        }
        self.timers.cancel(timers::Kind::Update);
        let old = previous.state().expect("session snapshot");
        let pending = std::mem::take(&mut *self.pending.lock().expect("session events"));
        if !clean_disk
            && pending.is_empty()
            && !resources.needs_update()
            && resources.documents.iter().all(|uri| {
                let path = tsr_tspath::to_path(
                    uri.file_name().as_bytes(),
                    self.options.current_directory.as_bytes(),
                    self.fs.use_case_sensitive_file_names(),
                );
                // Unopened dependencies normally have no explicit default.
                // Like getSnapshot, use program inclusion before rebuilding;
                // otherwise a node_modules request creates an inferred project
                // with different options and loses the importing files.
                previous
                    .project_for_file(path.as_bytes())
                    .is_some_and(|project| !project.data().unwrap().dirty)
            })
        {
            return Ok(previous);
        }
        let mut transaction = PendingTransaction {
            session: self,
            pending: Some(pending),
        };
        let pending = transaction.pending.as_ref().unwrap();
        let mut overlays = OverlayFs::new(
            host.clone(),
            self.options.current_directory.clone(),
            self.options.position_encoding,
        )
        .with_overlays(old.fs.overlays().clone());
        let changes = overlays.process_changes(&pending.changes)?;
        let next = self.derive_snapshot(
            Derivation {
                old,
                overlays,
                changes,
                resources,
                pending,
                clean_disk,
                host,
            },
            |builder| builder.build(resources),
        )?;
        let id = next.id().expect("session snapshot");
        for (key, project) in &old.projects {
            if next
                .project_by_path(key.as_bytes())
                .is_none_or(|next| !Arc::ptr_eq(project.pool(), next.pool()))
            {
                if let Some(scheduler) = project.scheduler() {
                    scheduler.discard();
                }
            }
        }
        *self.snapshot.write().expect("session snapshot") = Some(next.clone());
        transaction.pending = None;
        self.options.logger.log(format_args!(
            "Updated snapshot {} from {} ({} projects)",
            id,
            old.id,
            next.projects().len()
        ));
        if let Some(watches) = self.watches.get() {
            watches.enqueue(old.watches(), next.state().unwrap().watches());
        }
        self.sync_auto_import_watches();
        self.trigger_ata(&next);
        if self.events.get().is_some() {
            self.send_event(SessionEvent::Published {
                previous,
                current: next.clone(),
            });
        }
        Ok(next)
    }
    /// Builds the snapshot that follows `old` under `derivation` without
    /// publishing it: the session's update adopts the result, and the API's
    /// clones (a temporary file, a standalone program) keep theirs private,
    /// as the pin's snapshot host derives without session side effects.
    /// port: tsc/internal/project/snapshothost.go:SnapshotHost.update
    fn derive_snapshot(
        &self,
        derivation: Derivation<'_>,
        build: impl FnOnce(build::ProjectBuilder<'_>) -> Result<build::BuildOutput, Error>,
    ) -> Result<Snapshot, Error> {
        let Derivation {
            old,
            overlays,
            mut changes,
            resources,
            pending,
            clean_disk,
            host,
        } = derivation;
        if pending.mapper_locale_changed || pending.ata_changed || !pending.ata_changes.is_empty() {
            changes.invalidate_all = true;
        }
        changes.merge_watch_changes(&resources.watch_changes);
        let fs = Arc::new(SnapshotFsBuilder::with_host(
            old.fs.clone(),
            overlays.overlays().clone(),
            host,
        ));
        let (mapper_extensions, mapper_watched) = old.mapper_watch_state();
        fs.filter_watch_events(&mut changes, &mapper_extensions, &mapper_watched);
        old.fs.expand_realpath_aliases(&mut changes);
        fs.mark_dirty_files(&mut changes)?;
        fs.convert_open_and_close(&mut changes)?;
        if changes.invalidate_all {
            fs.invalidate_cache(false);
        }
        let id = next_snapshot_id();
        let ownership = Arc::new(ConfigOwnership::new(self.extended_cache.clone(), id));
        ownership.inherit(&old.config_ownership);
        let mut configs = ConfigRegistryBuilder::new(
            old.configs.clone(),
            fs.clone(),
            self.options.current_directory.clone(),
            overlays.overlays().clone(),
            pending
                .custom_name
                .clone()
                .unwrap_or_else(|| old.configs.custom_config_file_name.clone()),
            self.options.run_external_code,
            ownership.clone(),
        )
        .with_relative_patterns(self.options.relative_watch_patterns);
        let affected = configs.did_change_files(&changes)?;
        let inferred_options = pending
            .inferred
            .clone()
            .or_else(|| old.inferred_options.clone());
        let contributions = pending
            .contributions
            .clone()
            .unwrap_or_else(|| old.contributions.clone());
        let builder = build::ProjectBuilder::new(
            self,
            old,
            &mut configs,
            build::BuildInput {
                snapshot_id: id,
                fs: fs.clone(),
                overlays: overlays.overlays().clone(),
                changes: &changes,
                affected: &affected,
                inferred_options: inferred_options.clone(),
                contributions: contributions.clone(),
                ata_changes: &pending.ata_changes,
            },
        );
        let build::BuildOutput {
            projects,
            delayed_projects,
            defaults,
            api_state,
            api_error,
        } = build(builder)?;
        // The pin cleans unowned registry entries when recomputing the open
        // project set, not when merely loading resources for an LS request.
        // Such lookups may intentionally publish parsed solution configs that
        // have no project/open-file retainer yet.
        if changes.opened.is_some()
            || changes.reopened.is_some()
            || resources
                .api
                .as_ref()
                .is_some_and(|api| api.open_files.is_some() || api.close_files.is_some())
        {
            configs.cleanup();
        }
        let configs = configs.finalize();
        let clean = changes.opened.is_some()
            || changes.reopened.is_some()
            || !changes.closed.is_empty()
            || !changes.deleted.is_empty();
        let new_structure = projects.values().any(|project| {
            let data = project.data().unwrap();
            data.last_update == id && data.update_kind != crate::project::ProgramUpdateKind::Cloned
        });
        if clean_disk || clean && new_structure {
            fs.retain_files(|path| {
                projects
                    .values()
                    .any(|project| project.data().unwrap().host.seen_file(path))
            });
        }
        let fs = fs.finalize();
        for project in projects.values() {
            let data = project.data().unwrap();
            if data.last_update == id {
                data.host.freeze(fs.clone());
            }
        }
        let programs = projects
            .values()
            .filter_map(|project| project.program())
            .map(|program| self.program_counter.retain(program.clone()))
            .collect();
        let next = Snapshot::from_session(SessionSnapshot {
            id,
            parent: old.id,
            fs,
            configs,
            projects,
            delayed_projects,
            defaults,
            contributions,
            api_state,
            api_error,
            inferred_options,
            validation_enabled: pending.validation_enabled.unwrap_or(old.validation_enabled),
            config_ownership: ownership,
            _programs: programs,
        });
        Ok(next)
    }

    /// A snapshot derived from `base` with `uri` overridden by `new_text`,
    /// not adopted by the session. A new file must have a known script kind.
    /// port: tsc/internal/project/snapshot.go:Snapshot.cloneWithTemporaryFile
    pub fn clone_with_temporary_file(
        &self,
        base: &Snapshot,
        uri: &DocumentUri,
        new_text: JsString,
    ) -> Result<Snapshot, Error> {
        let _update = self
            .update
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let old = base.state().ok_or(Error::Closed)?;
        let path = uri.path(self.fs.use_case_sensitive_file_names());
        let mut overlays = (**old.fs.overlays()).clone();
        let mut changes = FileChangeSummary::default();
        let (version, kind) = if let Some(existing) = overlays.get(&path) {
            changes.changed.insert(uri.clone());
            (existing.version() + 1, existing.kind())
        } else {
            let kind = tsr_core::ScriptKind::from_file_name(uri.file_name().as_bytes());
            if kind == tsr_core::ScriptKind::UNKNOWN {
                return Err(Error::UnsupportedFileExtension(uri.file_name()));
            }
            changes.opened = Some(uri.clone());
            (0, kind)
        };
        overlays.insert(
            path,
            Arc::new(crate::overlay::FileHandle::overlay(
                uri.file_name(),
                new_text,
                version,
                kind,
            )),
        );
        let overlays = OverlayFs::new(
            self.fs.clone(),
            self.options.current_directory.clone(),
            self.options.position_encoding,
        )
        .with_overlays(Arc::new(overlays));
        let resources = crate::api::ResourceRequest::document(uri.clone());
        self.derive_snapshot(
            Derivation {
                old,
                overlays,
                changes,
                resources: &resources,
                pending: &Pending::default(),
                clean_disk: false,
                host: self.fs.clone(),
            },
            |builder| builder.build(&resources),
        )
    }

    /// An isolated snapshot with one synthetic inferred project built from
    /// explicit roots and options, seeded from `old_project` when given; the
    /// base is not adopted as session state.
    /// port: tsc/internal/project/snapshot.go:Snapshot.cloneForProgram
    pub fn clone_for_program(
        &self,
        base: &Snapshot,
        request: ProgramRequest,
        old_project: Option<&Project>,
        changes: FileChangeSummary,
    ) -> Result<Snapshot, Error> {
        let _update = self
            .update
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let old = base.state().ok_or(Error::Closed)?;
        let overlays = OverlayFs::new(
            self.fs.clone(),
            self.options.current_directory.clone(),
            self.options.position_encoding,
        )
        .with_overlays(old.fs.overlays().clone());
        let resources = crate::api::ResourceRequest::default();
        let mut command =
            tsr_tsoptions::ParsedCommandLine::new(request.options, request.root_file_names);
        command.project_references = request.project_references;
        command.errors = request.config_file_parsing_diagnostics;
        let command = Arc::new(command);
        self.derive_snapshot(
            Derivation {
                old,
                overlays,
                changes,
                resources: &resources,
                pending: &Pending::default(),
                clean_disk: false,
                host: self.fs.clone(),
            },
            move |builder| builder.build_program(command, old_project),
        )
    }
    // port: tsc/internal/project/session.go:Session.Close
    pub fn close(&self) {
        self.context.cancel();
        self.ata.cancel(true);
        let update = self
            .update
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let snapshot = self.snapshot.write().expect("session snapshot").take();
        if let Some(snapshot) = &snapshot {
            for project in snapshot.projects() {
                if let Some(scheduler) = project.scheduler() {
                    scheduler.discard();
                }
            }
        }
        *self.pending.lock().expect("session events") = Pending::default();
        drop(snapshot);
        self.context.cancel();
        self.ata.cancel(true);
        if let Some(mapper) = &self.mapper_host {
            mapper.close();
        }
        self.timers.stop();
        drop(update);
        self.ata.wait();
        self.timers.wait();
        if let Some(watches) = self.watches.get() {
            watches.close();
        }
        self.send_event(SessionEvent::Closed);
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.context.cancel();
        self.ata.cancel(true);
        self.ata.wait();
        self.timers.stop();
        if let Some(watches) = self.watches.get() {
            watches.stop();
        }
    }
}
/// The input of one snapshot derivation: the base, the overlays and file
/// changes already applied to them, the request, the pending session events
/// to fold in, and the file system the new snapshot reads.
struct Derivation<'a> {
    old: &'a Arc<SessionSnapshot>,
    overlays: OverlayFs,
    changes: FileChangeSummary,
    resources: &'a crate::api::ResourceRequest,
    pending: &'a Pending,
    clean_disk: bool,
    host: Arc<dyn FileSystem>,
}

/// The pin's `createProgram` input to `cloneForProgram`.
pub struct ProgramRequest {
    pub root_file_names: Vec<JsString>,
    pub options: CompilerOptions,
    pub project_references: Option<Vec<tsr_tsoptions::ProjectReference>>,
    pub config_file_parsing_diagnostics: Vec<tsr_ast::Diagnostic>,
}

struct PendingTransaction<'a> {
    session: &'a Session,
    pending: Option<Pending>,
}
impl Drop for PendingTransaction<'_> {
    fn drop(&mut self) {
        if let Some(mut failed) = self.pending.take() {
            let mut queued = self.session.pending.lock().expect("session events");
            for (key, change) in failed.ata_changes {
                queued.ata_changes.entry(key).or_insert(change);
            }
            queued.ata_changed |= failed.ata_changed;
            queued.mapper_locale_changed |= failed.mapper_locale_changed;
            failed.changes.append(&mut queued.changes);
            queued.changes = failed.changes;
            if queued.inferred.is_none() {
                queued.inferred = failed.inferred;
            }
            if queued.contributions.is_none() {
                queued.contributions = failed.contributions;
            }
            if queued.validation_enabled.is_none() {
                queued.validation_enabled = failed.validation_enabled;
            }
            if queued.custom_name.is_none() {
                queued.custom_name = failed.custom_name;
            }
        }
    }
}
