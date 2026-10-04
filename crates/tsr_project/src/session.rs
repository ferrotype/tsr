//! Session transactions publish complete snapshots. File notifications queue in
//! arrival order; the request barrier applies them before selecting a project.
use crate::{
    config::{ConfigFileRegistry, ConfigRegistryBuilder},
    extended_config::{ConfigOwnership, ExtendedConfigCache},
    file_change::{FileChange, FileChangeKind, FileChangeSummary},
    overlay::OverlayFs,
    parse_cache::ParseCache,
    program_counter::ProgramCounter,
    ref_count_cache::RefCountCacheOptions,
    snapshot::SessionSnapshot,
    snapshot_fs::{SnapshotFs, SnapshotFsBuilder},
    Snapshot,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, RwLock,
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

#[derive(Clone)]
pub struct SessionOptions {
    pub current_directory: JsString,
    pub default_library_path: JsString,
    pub position_encoding: PositionEncoding,
    pub run_external_code: bool,
    pub query_checkers: usize,
}
impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/".as_slice()),
            position_encoding: PositionEncoding::Utf16,
            run_external_code: false,
            query_checkers: 3,
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
    extended_cache: Arc<ExtendedConfigCache>,
    program_counter: Arc<ProgramCounter>,
    snapshot: RwLock<Option<Snapshot>>,
    update: Mutex<()>,
    pending: Mutex<Pending>,
}
static NEXT_SNAPSHOT: AtomicU64 = AtomicU64::new(1);
fn next_snapshot_id() -> u64 {
    NEXT_SNAPSHOT
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .expect("snapshot identity exhausted")
}
impl Session {
    // port: tsc/internal/project/session.go:NewSession
    pub fn new(options: SessionOptions, fs: Arc<dyn FileSystem>, counters: &Counters) -> Self {
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
    ) -> Self {
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
        Self {
            options,
            fs,
            counters: counters.clone(),
            parse_cache,
            extended_cache,
            program_counter: Arc::default(),
            snapshot: RwLock::new(Some(Snapshot::from_session(state))),
            update: Mutex::new(()),
            pending: Mutex::default(),
        }
    }
    // port: tsc/internal/project/session.go:Session.Snapshot
    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        self.snapshot
            .read()
            .expect("session snapshot")
            .clone()
            .ok_or(Error::Closed)
    }
    pub fn parse_cache(&self) -> &Arc<ParseCache> {
        &self.parse_cache
    }
    pub fn extended_config_cache(&self) -> &Arc<ExtendedConfigCache> {
        &self.extended_cache
    }
    pub fn program_counter(&self) -> &Arc<ProgramCounter> {
        &self.program_counter
    }
    pub fn enqueue(&self, change: FileChange) -> Result<(), Error> {
        let current = self.snapshot.read().expect("session snapshot");
        if current.is_none() {
            return Err(Error::Closed);
        }
        self.pending
            .lock()
            .expect("session events")
            .changes
            .push(change);
        Ok(())
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
        let current = self.snapshot.read().expect("session snapshot");
        if current.is_none() {
            return Err(Error::Closed);
        }
        self.pending.lock().expect("session events").inferred = Some(Arc::new(options));
        Ok(())
    }
    pub fn set_custom_config_file_name(&self, name: JsString) -> Result<(), Error> {
        let current = self.snapshot.read().expect("session snapshot");
        if current.is_none() {
            return Err(Error::Closed);
        }
        self.pending.lock().expect("session events").custom_name = Some(name);
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
        // Serialize construction, while current-snapshot reads and notification
        // admission remain available during a synchronous filesystem callback.
        let _update = self
            .update
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = self.snapshot()?;
        let old = previous.state().expect("session snapshot");
        let pending = std::mem::take(&mut *self.pending.lock().expect("session events"));
        if pending.is_empty()
            && requested.is_none_or(|uri| {
                let path = tsr_tspath::to_path(
                    uri.file_name().as_bytes(),
                    self.options.current_directory.as_bytes(),
                    self.fs.use_case_sensitive_file_names(),
                );
                previous.project_for_file(path.as_bytes()).is_some()
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
            self.fs.clone(),
            self.options.current_directory.clone(),
            self.options.position_encoding,
        )
        .with_overlays(old.fs.overlays().clone());
        let mut changes = overlays.process_changes(&pending.changes)?;
        let fs = Arc::new(SnapshotFsBuilder::new(
            old.fs.clone(),
            overlays.overlays().clone(),
        ));
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
        );
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
        if clean && new_structure {
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
        *self.snapshot.write().expect("session snapshot") = Some(next.clone());
        transaction.pending = None;
        Ok(next)
    }
    // port: tsc/internal/project/session.go:Session.Close
    pub fn close(&self) {
        let _update = self
            .update
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let snapshot = self.snapshot.write().expect("session snapshot").take();
        *self.pending.lock().expect("session events") = Pending::default();
        drop(snapshot);
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
