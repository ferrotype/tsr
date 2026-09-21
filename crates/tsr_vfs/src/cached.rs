//! Five independent caches, keyed by the caller's raw path. In keeping with
//! the source contract writes do not invalidate cached metadata. Cache guards
//! never cross a delegate call; enable is re-read before publishing a miss.
use crate::iofs::Time;
use crate::{
    Entries, Error, FileContent, FileInfo, FileSystem, ReadResult, SnapshotId, WalkCallback,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tsr_core::collections::SyncMap;
use tsr_jsstring::JsString;
pub struct CachedFs {
    inner: Arc<dyn FileSystem>,
    enabled: AtomicBool,
    directory_exists: SyncMap<JsString, bool>,
    file_exists: SyncMap<JsString, bool>,
    entries: SyncMap<JsString, Entries>,
    realpath: SyncMap<JsString, JsString>,
    stat: SyncMap<JsString, Option<FileInfo>>,
}
impl CachedFs {
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:From
    pub fn new(inner: Arc<dyn FileSystem>) -> Self {
        Self {
            inner,
            enabled: AtomicBool::new(true),
            directory_exists: SyncMap::default(),
            file_exists: SyncMap::default(),
            entries: SyncMap::default(),
            realpath: SyncMap::default(),
            stat: SyncMap::default(),
        }
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.Enable
    pub fn enable(&self) {
        self.enabled.store(true, Ordering::SeqCst);
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.ClearCache
    pub fn clear_cache(&self) {
        self.directory_exists.clear();
        self.file_exists.clear();
        self.entries.clear();
        self.realpath.clear();
        self.stat.clear();
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.DisableAndClearCache
    pub fn disable_and_clear_cache(&self) {
        if self
            .enabled
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.clear_cache();
        }
    }
    fn get<T: Clone>(
        &self,
        map: &SyncMap<JsString, T>,
        path: &[u8],
        load: impl FnOnce() -> Result<T, Error>,
    ) -> Result<T, Error> {
        if self.enabled.load(Ordering::SeqCst) {
            if let Some(value) = map.load(path) {
                return Ok(value);
            }
        }
        let value = load()?;
        if self.enabled.load(Ordering::SeqCst) {
            map.store(JsString::from_bytes(path), value.clone());
        }
        Ok(value)
    }
}
impl FileSystem for CachedFs {
    fn walk_dir_owned(&self, path: &[u8], visit: crate::OwnedWalkCallback) -> Result<(), Error> {
        self.inner.walk_dir_owned(path, visit)
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        self.inner.snapshot_id()
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        self.read_file_result(path).map(ReadResult::into_file)
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.UseCaseSensitiveFileNames
    fn use_case_sensitive_file_names(&self) -> bool {
        self.inner.use_case_sensitive_file_names()
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.FileExists
    fn file_exists(&self, path: &[u8]) -> Result<bool, Error> {
        self.get(&self.file_exists, path, || self.inner.file_exists(path))
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.ReadFile
    fn read_file_result(&self, path: &[u8]) -> Result<ReadResult, Error> {
        self.inner.read_file_result(path)
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.WriteFile
    fn write_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        self.inner.write_file(path, data)
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.AppendFile
    fn append_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        self.inner.append_file(path, data)
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.Remove
    fn remove(&self, path: &[u8]) -> Result<(), Error> {
        self.inner.remove(path)
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.Chtimes
    fn change_times(&self, path: &[u8], a_time: Time, m_time: Time) -> Result<(), Error> {
        self.inner.change_times(path, a_time, m_time)
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.DirectoryExists
    fn directory_exists(&self, path: &[u8]) -> Result<bool, Error> {
        self.get(&self.directory_exists, path, || {
            self.inner.directory_exists(path)
        })
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.GetAccessibleEntries
    fn entries(&self, path: &[u8]) -> Result<Entries, Error> {
        self.get(&self.entries, path, || self.inner.entries(path))
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.Stat
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        self.get(&self.stat, path, || self.inner.stat(path))
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.WalkDir
    fn walk_dir(&self, path: &[u8], visit: &mut WalkCallback<'_>) -> Result<(), Error> {
        self.inner.walk_dir(path, visit)
    }
    /// port: tsc/internal/vfs/cachedvfs/cachedvfs.go:FS.Realpath
    fn realpath(&self, path: &[u8]) -> Result<JsString, Error> {
        self.get(&self.realpath, path, || self.inner.realpath(path))
    }
}
