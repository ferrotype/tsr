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
    Snapshot,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock, RwLock,
    },
};
use tsr_arena::Counters;
use tsr_core::CompilerOptions;
use tsr_jsstring::{JsString, PositionEncoding};
use tsr_lsproto::{DocumentUri, LanguageKind, TextDocumentContentChangePartialOrWholeDocument};
use tsr_vfs::FileSystem;

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
    pub query_checkers: usize,
    pub relative_watch_patterns: bool,
    pub debounce_delay: std::time::Duration,
    pub logger: crate::logging::Logger,
}

/// Publication and loading events for the server. Sending is nonblocking and
/// invokes no client code under the session's update lock. The receiver owns
/// delivery; snapshots retain all files used to format the notifications.
pub enum SessionEvent {
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
            Self::Compiler(error) => error.fmt(f),
            Self::Host(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for Error {}

#[derive(Default)]
struct Pending {
    changes: Vec<FileChange>,
    inferred: Option<Arc<CompilerOptions>>,
    custom_name: Option<JsString>,
}
impl Pending {
    fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.inferred.is_none() && self.custom_name.is_none()
    }
}

pub struct Session {
    options: SessionOptions,
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
            defaults: BTreeMap::new(),
            inferred_options: None,
            config_ownership: Arc::new(ConfigOwnership::new(extended_cache.clone(), id)),
            _programs: Vec::new(),
        };
        Arc::new_cyclic(|weak| Self {
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
            events: OnceLock::new(),
        })
    }
    // port: tsc/internal/project/session.go:Session.Snapshot
    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        self.snapshot
            .read()
            .expect("session snapshot")
            .clone()
            .ok_or(Error::Closed)
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
        if let Some(watches) = self.watches.get() {
            watches.wait();
        }
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
            // Ordinary document edits trigger client-side diagnostic pulls.
            // Content-mapped document refreshes are connected in L6.
            self.timers.cancel(timers::Kind::DiagnosticsRefresh);
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
            config |= state.configs.configs.contains_key(&path);
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
                // Content-mapper extensions and watched files are connected in L6.
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
            && requested.is_none_or(|uri| {
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
        let mut changes = overlays.process_changes(&pending.changes)?;
        let fs = Arc::new(SnapshotFsBuilder::with_host(
            old.fs.clone(),
            overlays.overlays().clone(),
            host,
        ));
        // Content-mapper extensions and watched files are connected in L6.
        fs.filter_watch_events(&mut changes, &[], &BTreeSet::new());
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
            },
        );
        let (projects, defaults) = builder.build(requested)?;
        configs.cleanup();
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
            defaults,
            inferred_options,
            config_ownership: ownership,
            _programs: programs,
        });
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
        if self.events.get().is_some() {
            self.send_event(SessionEvent::Published {
                previous,
                current: next.clone(),
            });
        }
        Ok(next)
    }
    // port: tsc/internal/project/session.go:Session.Close
    pub fn close(&self) {
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
        self.timers.stop();
        drop(update);
        self.timers.wait();
        if let Some(watches) = self.watches.get() {
            watches.close();
        }
        self.send_event(SessionEvent::Closed);
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.timers.stop();
        if let Some(watches) = self.watches.get() {
            watches.stop();
        }
    }
}
struct PendingTransaction<'a> {
    session: &'a Session,
    pending: Option<Pending>,
}
impl Drop for PendingTransaction<'_> {
    fn drop(&mut self) {
        if let Some(mut failed) = self.pending.take() {
            let mut queued = self.session.pending.lock().expect("session events");
            failed.changes.append(&mut queued.changes);
            queued.changes = failed.changes;
            if queued.inferred.is_none() {
                queued.inferred = failed.inferred;
            }
            if queued.custom_name.is_none() {
                queued.custom_name = failed.custom_name;
            }
        }
    }
}
