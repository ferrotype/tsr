//! Per-method replacement over a retained filesystem. An arbitrary replacement
//! may consult mutable state, so this wrapper never advertises snapshot identity.
use crate::iofs::Time;
#[cfg(feature = "harness")]
use crate::OwnedWalkCallback;
use crate::{
    Entries, Error, FileContent, FileInfo, FileSystem, ReadResult, SnapshotId, WalkCallback,
};
use std::sync::Arc;
use tsr_jsstring::JsString;

pub type UseCaseSensitiveFileNamesReplacement = Arc<dyn Fn() -> bool + Send + Sync>;
pub type FileExistsReplacement = Arc<dyn Fn(&[u8]) -> Result<bool, Error> + Send + Sync>;
pub type ReadFileResultReplacement = Arc<dyn Fn(&[u8]) -> Result<ReadResult, Error> + Send + Sync>;
pub type WriteFileReplacement = Arc<dyn Fn(&[u8], &[u8]) -> Result<(), Error> + Send + Sync>;
pub type AppendFileReplacement = Arc<dyn Fn(&[u8], &[u8]) -> Result<(), Error> + Send + Sync>;
pub type RemoveReplacement = Arc<dyn Fn(&[u8]) -> Result<(), Error> + Send + Sync>;
pub type ChangeTimesReplacement = Arc<dyn Fn(&[u8], Time, Time) -> Result<(), Error> + Send + Sync>;
pub type DirectoryExistsReplacement = Arc<dyn Fn(&[u8]) -> Result<bool, Error> + Send + Sync>;
pub type EntriesReplacement = Arc<dyn Fn(&[u8]) -> Result<Entries, Error> + Send + Sync>;
pub type StatReplacement = Arc<dyn Fn(&[u8]) -> Result<Option<FileInfo>, Error> + Send + Sync>;
pub type WalkDirReplacement =
    Arc<dyn Fn(&[u8], &mut WalkCallback<'_>) -> Result<(), Error> + Send + Sync>;
#[cfg(feature = "harness")]
pub type OwnedWalkDirReplacement =
    Arc<dyn Fn(&[u8], OwnedWalkCallback) -> Result<(), Error> + Send + Sync>;
pub type RealpathReplacement = Arc<dyn Fn(&[u8]) -> Result<JsString, Error> + Send + Sync>;

/// Source type: tsc/internal/vfs/wrapvfs/wrapvfs.go:Replacements
#[derive(Clone, Default)]
pub struct Replacements {
    pub use_case_sensitive_file_names: Option<UseCaseSensitiveFileNamesReplacement>,
    pub file_exists: Option<FileExistsReplacement>,
    pub read_file_result: Option<ReadFileResultReplacement>,
    pub write_file: Option<WriteFileReplacement>,
    pub append_file: Option<AppendFileReplacement>,
    pub remove: Option<RemoveReplacement>,
    pub change_times: Option<ChangeTimesReplacement>,
    pub directory_exists: Option<DirectoryExistsReplacement>,
    pub entries: Option<EntriesReplacement>,
    pub stat: Option<StatReplacement>,
    pub walk_dir: Option<WalkDirReplacement>,
    /// Retention-preserving form of the same operation, used for owned visitors.
    /// Set together with `walk_dir` when a replacement supports retained calls.
    #[cfg(feature = "harness")]
    pub walk_dir_owned: Option<OwnedWalkDirReplacement>,
    pub realpath: Option<RealpathReplacement>,
}
impl Replacements {
    /// Number of source VFS operations with an installed delegate.
    #[cfg(feature = "harness")]
    pub fn wired_count(&self) -> usize {
        [
            self.use_case_sensitive_file_names.is_some(),
            self.file_exists.is_some(),
            self.read_file_result.is_some(),
            self.write_file.is_some(),
            self.append_file.is_some(),
            self.remove.is_some(),
            self.change_times.is_some(),
            self.directory_exists.is_some(),
            self.entries.is_some(),
            self.stat.is_some(),
            self.walk_dir.is_some() || self.walk_dir_owned.is_some(),
            self.realpath.is_some(),
        ]
        .into_iter()
        .filter(|wired| *wired)
        .count()
    }
    /// Bind each source operation to this owner, not the caller's variable.
    pub fn forwarding(inner: Arc<dyn FileSystem>) -> Self {
        Self {
            use_case_sensitive_file_names: Some({
                let inner = inner.clone();
                Arc::new(move || inner.use_case_sensitive_file_names())
            }),
            file_exists: Some({
                let inner = inner.clone();
                Arc::new(move |path| inner.file_exists(path))
            }),
            read_file_result: Some({
                let inner = inner.clone();
                Arc::new(move |path| inner.read_file_result(path))
            }),
            write_file: Some({
                let inner = inner.clone();
                Arc::new(move |path, data| inner.write_file(path, data))
            }),
            append_file: Some({
                let inner = inner.clone();
                Arc::new(move |path, data| inner.append_file(path, data))
            }),
            remove: Some({
                let inner = inner.clone();
                Arc::new(move |path| inner.remove(path))
            }),
            change_times: Some({
                let inner = inner.clone();
                Arc::new(move |path, a_time, m_time| inner.change_times(path, a_time, m_time))
            }),
            directory_exists: Some({
                let inner = inner.clone();
                Arc::new(move |path| inner.directory_exists(path))
            }),
            entries: Some({
                let inner = inner.clone();
                Arc::new(move |path| inner.entries(path))
            }),
            stat: Some({
                let inner = inner.clone();
                Arc::new(move |path| inner.stat(path))
            }),
            walk_dir: Some({
                let inner = inner.clone();
                Arc::new(move |path, visit| inner.walk_dir(path, visit))
            }),
            #[cfg(feature = "harness")]
            walk_dir_owned: Some({
                let inner = inner.clone();
                Arc::new(move |path, visit| inner.walk_dir_owned(path, visit))
            }),
            realpath: Some(Arc::new(move |path| inner.realpath(path))),
        }
    }
}
pub struct WrappedFs {
    inner: Arc<dyn FileSystem>,
    replacements: Replacements,
}
impl WrappedFs {
    /// port: tsc/internal/vfs/wrapvfs/wrapvfs.go:Wrap
    pub fn new(inner: Arc<dyn FileSystem>, replacements: Replacements) -> Self {
        Self {
            inner,
            replacements,
        }
    }
}
impl FileSystem for WrappedFs {
    #[cfg(feature = "harness")]
    fn walk_dir_owned(&self, path: &[u8], visit: OwnedWalkCallback) -> Result<(), Error> {
        if let Some(replace) = &self.replacements.walk_dir_owned {
            replace(path, visit)
        } else if let Some(replace) = &self.replacements.walk_dir {
            replace(path, &mut |path, entry, error| visit(path, entry, error))
        } else {
            self.inner.walk_dir_owned(path, visit)
        }
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        None
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        self.read_file_result(path).map(ReadResult::into_file)
    }
    /// port: tsc/internal/vfs/wrapvfs/wrapvfs.go:wrappedFS.UseCaseSensitiveFileNames
    fn use_case_sensitive_file_names(&self) -> bool {
        match &self.replacements.use_case_sensitive_file_names {
            Some(replace) => replace(),
            None => self.inner.use_case_sensitive_file_names(),
        }
    }
    /// port: tsc/internal/vfs/wrapvfs/wrapvfs.go:wrappedFS.FileExists
    fn file_exists(&self, path: &[u8]) -> Result<bool, Error> {
        match &self.replacements.file_exists {
            Some(replace) => replace(path),
            None => self.inner.file_exists(path),
        }
    }
    /// port: tsc/internal/vfs/wrapvfs/wrapvfs.go:wrappedFS.ReadFile
    fn read_file_result(&self, path: &[u8]) -> Result<ReadResult, Error> {
        match &self.replacements.read_file_result {
            Some(replace) => replace(path),
            None => self.inner.read_file_result(path),
        }
    }
    /// port: tsc/internal/vfs/wrapvfs/wrapvfs.go:wrappedFS.WriteFile
    fn write_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        match &self.replacements.write_file {
            Some(replace) => replace(path, data),
            None => self.inner.write_file(path, data),
        }
    }
    /// port: tsc/internal/vfs/wrapvfs/wrapvfs.go:wrappedFS.AppendFile
    fn append_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        match &self.replacements.append_file {
            Some(replace) => replace(path, data),
            None => self.inner.append_file(path, data),
        }
    }
    /// port: tsc/internal/vfs/wrapvfs/wrapvfs.go:wrappedFS.Remove
    fn remove(&self, path: &[u8]) -> Result<(), Error> {
        match &self.replacements.remove {
            Some(replace) => replace(path),
            None => self.inner.remove(path),
        }
    }
    /// port: tsc/internal/vfs/wrapvfs/wrapvfs.go:wrappedFS.Chtimes
    fn change_times(&self, path: &[u8], a_time: Time, m_time: Time) -> Result<(), Error> {
        match &self.replacements.change_times {
            Some(replace) => replace(path, a_time, m_time),
            None => self.inner.change_times(path, a_time, m_time),
        }
    }
    /// port: tsc/internal/vfs/wrapvfs/wrapvfs.go:wrappedFS.DirectoryExists
    fn directory_exists(&self, path: &[u8]) -> Result<bool, Error> {
        match &self.replacements.directory_exists {
            Some(replace) => replace(path),
            None => self.inner.directory_exists(path),
        }
    }
    /// port: tsc/internal/vfs/wrapvfs/wrapvfs.go:wrappedFS.GetAccessibleEntries
    fn entries(&self, path: &[u8]) -> Result<Entries, Error> {
        match &self.replacements.entries {
            Some(replace) => replace(path),
            None => self.inner.entries(path),
        }
    }
    /// port: tsc/internal/vfs/wrapvfs/wrapvfs.go:wrappedFS.Stat
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        match &self.replacements.stat {
            Some(replace) => replace(path),
            None => self.inner.stat(path),
        }
    }
    /// port: tsc/internal/vfs/wrapvfs/wrapvfs.go:wrappedFS.WalkDir
    fn walk_dir(&self, path: &[u8], visit: &mut WalkCallback<'_>) -> Result<(), Error> {
        match &self.replacements.walk_dir {
            Some(replace) => replace(path, visit),
            None => self.inner.walk_dir(path, visit),
        }
    }
    /// port: tsc/internal/vfs/wrapvfs/wrapvfs.go:wrappedFS.Realpath
    fn realpath(&self, path: &[u8]) -> Result<JsString, Error> {
        match &self.replacements.realpath {
            Some(replace) => replace(path),
            None => self.inner.realpath(path),
        }
    }
}
