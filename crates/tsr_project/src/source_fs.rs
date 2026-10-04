//! A project's dependency observations are independent of the shared snapshot
//! cache. Missing files and missing parent directories are dependencies too.
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tsr_jsstring::JsString;
use tsr_vfs::{Entries, Error, FileContent, FileInfo, FileSystem, SnapshotId};

pub struct SourceFs {
    source: Mutex<(Arc<dyn FileSystem>, bool)>,
    cwd: JsString,
    tracking: AtomicBool,
    seen_files: Mutex<BTreeSet<JsString>>,
    missing_directories: Mutex<BTreeSet<JsString>>,
}
impl SourceFs {
    // port: tsc/internal/project/snapshotfs.go:newSourceFS
    pub fn new(source: Arc<dyn FileSystem>, cwd: JsString, tracking: bool) -> Self {
        Self {
            source: Mutex::new((source, false)),
            cwd,
            tracking: AtomicBool::new(tracking),
            seen_files: Mutex::default(),
            missing_directories: Mutex::default(),
        }
    }
    fn source(&self) -> Arc<dyn FileSystem> {
        self.source.lock().expect("source filesystem").0.clone()
    }
    /// Replace the private construction host once, after its loaders join.
    /// This releases the builder and its previous-snapshot roots.
    pub(crate) fn freeze(&self, source: Arc<dyn FileSystem>) {
        self.disable_tracking();
        let mut state = self.source.lock().expect("source filesystem");
        assert!(!state.1, "compiler host is already frozen");
        let previous = std::mem::replace(&mut *state, (source, true));
        drop(state);
        drop(previous);
    }
    pub(crate) fn inherit_dependencies(&self, previous: &Self) {
        self.seen_files
            .lock()
            .unwrap()
            .clone_from(&previous.seen_files.lock().unwrap());
        self.missing_directories
            .lock()
            .unwrap()
            .clone_from(&previous.missing_directories.lock().unwrap());
    }
    fn path(&self, name: &[u8]) -> JsString {
        tsr_tspath::to_path(
            name,
            self.cwd.as_bytes(),
            self.source().use_case_sensitive_file_names(),
        )
    }
    // port: tsc/internal/project/snapshotfs.go:sourceFS.DisableTracking
    pub fn disable_tracking(&self) {
        self.tracking.store(false, Ordering::Release);
    }
    // port: tsc/internal/project/snapshotfs.go:sourceFS.Track
    pub fn track(&self, name: &[u8]) {
        if self.tracking.load(Ordering::Acquire) {
            let path = self.path(name);
            self.seen_files.lock().unwrap().insert(path);
        }
    }
    pub fn seen_files(&self) -> BTreeSet<JsString> {
        self.seen_files.lock().unwrap().clone()
    }
    // port: tsc/internal/project/snapshotfs.go:sourceFS.SeenFile
    pub fn seen_file(&self, path: &JsString) -> bool {
        self.seen_files.lock().unwrap().contains(path)
    }
    // port: tsc/internal/project/snapshotfs.go:sourceFS.SeenFileOrMissingParentDirectory
    pub fn seen_file_or_missing_parent_directory(&self, path: &JsString) -> bool {
        if self.seen_file(path) {
            return true;
        }
        let missing = self.missing_directories.lock().unwrap();
        if missing.is_empty() {
            return false;
        }
        let mut path = path.clone();
        loop {
            if missing.contains(&path) {
                return true;
            }
            let parent = JsString::from_bytes(tsr_tspath::directory(path.as_bytes()));
            if parent == path {
                return false;
            }
            path = parent;
        }
    }
}
impl FileSystem for SourceFs {
    fn snapshot_id(&self) -> Option<SnapshotId> {
        self.source().snapshot_id()
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        self.source().use_case_sensitive_file_names()
    }
    // port: tsc/internal/project/snapshotfs.go:sourceFS.ReadFile
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        self.track(path);
        self.source().read_file(path)
    }
    // port: tsc/internal/project/snapshotfs.go:sourceFS.FileExists
    fn file_exists(&self, path: &[u8]) -> Result<bool, Error> {
        self.track(path);
        self.source().file_exists(path)
    }
    // port: tsc/internal/project/snapshotfs.go:sourceFS.DirectoryExists
    fn directory_exists(&self, path: &[u8]) -> Result<bool, Error> {
        let exists = self.source().directory_exists(path)?;
        if !exists && self.tracking.load(Ordering::Acquire) {
            self.missing_directories
                .lock()
                .unwrap()
                .insert(self.path(path));
        }
        Ok(exists)
    }
    fn entries(&self, path: &[u8]) -> Result<Entries, Error> {
        self.source().entries(path)
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        self.source().stat(path)
    }
    fn realpath(&self, path: &[u8]) -> Result<JsString, Error> {
        self.source().realpath(path)
    }
    fn walk_dir(&self, path: &[u8], visit: &mut tsr_vfs::WalkCallback<'_>) -> Result<(), Error> {
        self.source().walk_dir(path, visit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tracks_misses_and_ancestors_only_until_frozen() {
        let fs = tsr_vfs::MemoryBuilder::new(b"/", false).finish();
        let source = SourceFs::new(Arc::new(fs), JsString::from_bytes(&b"/"[..]), true);
        assert!(!source.file_exists(b"/source/A.ts").unwrap());
        assert!(source.read_file(b"/source/B.ts").unwrap().is_none());
        assert!(!source.directory_exists(b"/source/missing").unwrap());
        assert!(source.seen_file(&JsString::from_bytes(&b"/source/a.ts"[..])));
        assert!(
            source.seen_file_or_missing_parent_directory(&JsString::from_bytes(
                &b"/source/missing/deep/c.ts"[..]
            ))
        );
        assert!(
            !source.seen_file_or_missing_parent_directory(&JsString::from_bytes(
                &b"/source/missing-other/c.ts"[..]
            ))
        );
        source.disable_tracking();
        source.read_file(b"/late.ts").unwrap();
        source.directory_exists(b"/late").unwrap();
        assert_eq!(source.seen_files().len(), 2);
        assert!(!source
            .seen_file_or_missing_parent_directory(&JsString::from_bytes(&b"/late/a.ts"[..])));
    }
}
