//! Track raw read dependencies, including misses and walk callback arrivals.
//! Writes are forwarded without adding output paths. No callback runs under a lock.
use crate::iofs::Time;
use crate::{
    Entries, Error, FileContent, FileInfo, FileSystem, ReadResult, SnapshotId, WalkCallback,
};
use std::sync::Arc;
use tsr_core::collections::SyncSet;
use tsr_jsstring::JsString;
pub struct TrackingFs {
    pub inner: Arc<dyn FileSystem>,
    pub seen_files: Arc<SyncSet<JsString>>,
}
impl TrackingFs {
    pub fn new(inner: Arc<dyn FileSystem>) -> Self {
        Self {
            inner,
            seen_files: Arc::default(),
        }
    }
}
impl FileSystem for TrackingFs {
    fn walk_dir_owned(&self, path: &[u8], visit: crate::OwnedWalkCallback) -> Result<(), Error> {
        self.seen_files.insert(JsString::from_bytes(path));
        let seen = self.seen_files.clone();
        self.inner.walk_dir_owned(
            path,
            Arc::new(move |path, entry, error| {
                seen.insert(JsString::from_bytes(path));
                visit(path, entry, error)
            }),
        )
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        self.inner.snapshot_id()
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        self.read_file_result(path).map(ReadResult::into_file)
    }
    /// port: tsc/internal/vfs/trackingvfs/trackingvfs.go:FS.UseCaseSensitiveFileNames
    fn use_case_sensitive_file_names(&self) -> bool {
        self.inner.use_case_sensitive_file_names()
    }
    /// port: tsc/internal/vfs/trackingvfs/trackingvfs.go:FS.FileExists
    fn file_exists(&self, path: &[u8]) -> Result<bool, Error> {
        self.seen_files.insert(JsString::from_bytes(path));
        self.inner.file_exists(path)
    }
    /// port: tsc/internal/vfs/trackingvfs/trackingvfs.go:FS.ReadFile
    fn read_file_result(&self, path: &[u8]) -> Result<ReadResult, Error> {
        self.seen_files.insert(JsString::from_bytes(path));
        self.inner.read_file_result(path)
    }
    /// port: tsc/internal/vfs/trackingvfs/trackingvfs.go:FS.WriteFile
    fn write_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        self.inner.write_file(path, data)
    }
    /// port: tsc/internal/vfs/trackingvfs/trackingvfs.go:FS.AppendFile
    fn append_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        self.inner.append_file(path, data)
    }
    /// port: tsc/internal/vfs/trackingvfs/trackingvfs.go:FS.Remove
    fn remove(&self, path: &[u8]) -> Result<(), Error> {
        self.inner.remove(path)
    }
    /// port: tsc/internal/vfs/trackingvfs/trackingvfs.go:FS.Chtimes
    fn change_times(&self, path: &[u8], a_time: Time, m_time: Time) -> Result<(), Error> {
        self.inner.change_times(path, a_time, m_time)
    }
    /// port: tsc/internal/vfs/trackingvfs/trackingvfs.go:FS.DirectoryExists
    fn directory_exists(&self, path: &[u8]) -> Result<bool, Error> {
        self.seen_files.insert(JsString::from_bytes(path));
        self.inner.directory_exists(path)
    }
    /// port: tsc/internal/vfs/trackingvfs/trackingvfs.go:FS.GetAccessibleEntries
    fn entries(&self, path: &[u8]) -> Result<Entries, Error> {
        self.seen_files.insert(JsString::from_bytes(path));
        self.inner.entries(path)
    }
    /// port: tsc/internal/vfs/trackingvfs/trackingvfs.go:FS.Stat
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        self.seen_files.insert(JsString::from_bytes(path));
        self.inner.stat(path)
    }
    /// port: tsc/internal/vfs/trackingvfs/trackingvfs.go:FS.WalkDir
    fn walk_dir(&self, path: &[u8], visit: &mut WalkCallback<'_>) -> Result<(), Error> {
        self.seen_files.insert(JsString::from_bytes(path));
        self.inner.walk_dir(path, &mut |path, entry, error| {
            self.seen_files.insert(JsString::from_bytes(path));
            visit(path, entry, error)
        })
    }
    /// port: tsc/internal/vfs/trackingvfs/trackingvfs.go:FS.Realpath
    fn realpath(&self, path: &[u8]) -> Result<JsString, Error> {
        self.seen_files.insert(JsString::from_bytes(path));
        self.inner.realpath(path)
    }
}
