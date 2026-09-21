//! Explicit read hosts. Program construction requires an immutable snapshot.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
pub mod cached;
mod io_adapter;
pub mod iofs;
pub mod iovfs;
pub mod os;
pub mod recording;
pub mod tracking;
pub mod vfstest;
mod walk;
pub mod wrapped;
use tsr_jsstring::{JsString, SourceText};
use tsr_tspath as path;
pub use walk::{OwnedWalkCallback, WalkCallback, WalkControl, WalkEntry};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported(&'static str),
    OutsideScope,
    InvalidPath,
    SymlinkCycle,
    Io(std::io::ErrorKind),
    /// Live adapters preserve the source error's path and wrapping chain.
    Detailed(Arc<iofs::IoError>),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Detailed(error) => error.fmt(f),
            _ => write!(f, "{self:?}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<iofs::IoError> for Error {
    fn from(error: iofs::IoError) -> Self {
        Self::Detailed(Arc::new(error))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SnapshotId(u64);
static NEXT_SNAPSHOT: AtomicU64 = AtomicU64::new(1);
fn snapshot_id() -> SnapshotId {
    SnapshotId(
        NEXT_SNAPSHOT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .expect("filesystem snapshot identities exhausted"),
    )
}
#[derive(Clone, Debug)]
pub struct FileContent {
    pub raw: Arc<[u8]>,
    pub text: SourceText,
}
impl FileContent {
    pub fn physical(bytes: impl Into<Arc<[u8]>>) -> Self {
        let raw = bytes.into();
        Self {
            text: SourceText::from_bytes(raw.clone()),
            raw,
        }
    }
    pub fn loaded(bytes: impl Into<Arc<[u8]>>) -> Self {
        let raw = bytes.into();
        Self {
            text: SourceText::from_loaded_bytes(raw.clone()),
            raw,
        }
    }
}
/// The source VFS read contract keeps the content and success bit independent.
/// Compiler callers normally use `read_file`; transparent wrappers use this
/// representation so an unsuccessful read's supplied content is not discarded.
#[derive(Clone, Debug)]
pub struct ReadResult {
    pub content: FileContent,
    pub found: bool,
}
impl ReadResult {
    pub fn into_file(self) -> Option<FileContent> {
        self.found.then_some(self.content)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entries {
    /// None is a nil Go slice; Some(empty) retains an explicitly empty list.
    pub files: Option<Vec<JsString>>,
    pub directories: Option<Vec<JsString>>,
    pub symlinks: Option<BTreeSet<JsString>>,
}
/// What `stat` answers. Hosts that have no name, time or mode to report leave
/// them at their defaults; `basic` builds that form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    pub directory: bool,
    pub size: u64,
    /// The base name the host reports.
    pub name: JsString,
    pub mod_time: iofs::Time,
    pub mode: iofs::FileMode,
    pub sys: iofs::Sys,
}
impl FileInfo {
    pub fn basic(directory: bool, size: u64) -> Self {
        let mode = if directory {
            iofs::FileMode::DIR
        } else {
            iofs::FileMode::default()
        };
        Self {
            directory,
            size,
            name: JsString::default(),
            mod_time: iofs::Time::ZERO,
            mode,
            sys: iofs::Sys::Nil,
        }
    }
}
/// Reads never consult a fallback host. None means missing, including the Go
/// ReadFile failure case; unsupported operations remain explicit errors.
pub trait FileSystem: Send + Sync {
    fn use_case_sensitive_file_names(&self) -> bool;
    fn snapshot_id(&self) -> Option<SnapshotId>;
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error>;
    fn read_file_result(&self, path: &[u8]) -> Result<ReadResult, Error> {
        let file = self.read_file(path)?;
        Ok(ReadResult {
            found: file.is_some(),
            content: file.unwrap_or_else(|| FileContent::loaded(Arc::<[u8]>::from([]))),
        })
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error>;
    fn entries(&self, path: &[u8]) -> Result<Entries, Error>;
    fn realpath(&self, path: &[u8]) -> Result<JsString, Error>;
    fn file_exists(&self, path: &[u8]) -> Result<bool, Error> {
        Ok(self.stat(path)?.is_some_and(|f| !f.directory))
    }
    fn directory_exists(&self, path: &[u8]) -> Result<bool, Error> {
        Ok(self.stat(path)?.is_some_and(|f| f.directory))
    }
    fn entry(&self, path: &[u8]) -> Result<Option<WalkEntry>, Error> {
        Ok(self.stat(path)?.map(|info| WalkEntry {
            name: JsString::from_bytes(path.rsplit(|&byte| byte == b'/').next().unwrap_or(path)),
            info,
            symlink: false,
        }))
    }
    fn walk_dir(&self, root: &[u8], visit: &mut WalkCallback<'_>) -> Result<(), Error> {
        walk::walk(self, root, visit)
    }
    /// Retaining adapters override this; ordinary hosts use the same walk.
    fn walk_dir_owned(&self, root: &[u8], visit: OwnedWalkCallback) -> Result<(), Error> {
        self.walk_dir(root, &mut |path, entry, error| visit(path, entry, error))
    }
    fn write_file(&self, _path: &[u8], _data: &[u8]) -> Result<(), Error> {
        Err(Error::Unsupported("immutable filesystem write"))
    }
    fn append_file(&self, _path: &[u8], _data: &[u8]) -> Result<(), Error> {
        Err(Error::Unsupported("immutable filesystem append"))
    }
    fn remove(&self, _path: &[u8]) -> Result<(), Error> {
        Err(Error::Unsupported("immutable filesystem remove"))
    }
    /// Both instants travel, as the pinned `Chtimes` carries them.
    fn change_times(
        &self,
        _path: &[u8],
        _a_time: iofs::Time,
        _m_time: iofs::Time,
    ) -> Result<(), Error> {
        Err(Error::Unsupported("immutable filesystem timestamps"))
    }
}
#[derive(Clone, Debug)]
enum Entry {
    File(FileContent),
    Directory,
    Symlink(JsString),
}
#[derive(Clone, Debug)]
struct NamedEntry {
    name: JsString,
    value: Entry,
}
#[derive(Clone, Debug)]
pub struct MemoryBuilder {
    cwd: JsString,
    case_sensitive: bool,
    entries: BTreeMap<JsString, NamedEntry>,
}
impl MemoryBuilder {
    pub fn new(cwd: &[u8], case_sensitive: bool) -> Self {
        Self {
            cwd: JsString::from_bytes(path::absolute(cwd, b"")),
            case_sensitive,
            entries: BTreeMap::new(),
        }
    }
    pub fn from_snapshot(snapshot: &MemorySnapshot) -> Self {
        Self {
            cwd: snapshot.cwd.clone(),
            case_sensitive: snapshot.case_sensitive,
            entries: snapshot.entries.clone(),
        }
    }
    fn insert(&mut self, name: &[u8], value: Entry) {
        let name = path::absolute(name, self.cwd.as_bytes());
        for directory in path::ancestors(&path::directory(&name)) {
            let key = path::to_path(&directory, b"", self.case_sensitive);
            self.entries.entry(key).or_insert_with(|| NamedEntry {
                name: JsString::from_bytes(directory),
                value: Entry::Directory,
            });
        }
        let key = path::to_path(&name, b"", self.case_sensitive);
        self.entries.insert(
            key,
            NamedEntry {
                name: JsString::from_bytes(name),
                value,
            },
        );
    }
    pub fn insert_physical(&mut self, path: &[u8], bytes: impl Into<Arc<[u8]>>) {
        self.insert(path, Entry::File(FileContent::physical(bytes)));
    }
    pub fn insert_loaded(&mut self, path: &[u8], bytes: impl Into<Arc<[u8]>>) {
        self.insert(path, Entry::File(FileContent::loaded(bytes)));
    }
    pub fn insert_directory(&mut self, path: &[u8]) {
        self.insert(path, Entry::Directory);
    }
    pub fn insert_symlink(&mut self, path: &[u8], target: &[u8]) {
        self.insert(path, Entry::Symlink(JsString::from_bytes(target)));
    }
    pub fn remove(&mut self, path: &[u8]) {
        let key = path::to_path(path, self.cwd.as_bytes(), self.case_sensitive);
        self.entries.remove(&key);
    }
    pub fn finish(self) -> MemorySnapshot {
        MemorySnapshot {
            cwd: self.cwd,
            case_sensitive: self.case_sensitive,
            entries: self.entries,
            id: snapshot_id(),
        }
    }
}
#[derive(Debug)]
pub struct MemorySnapshot {
    cwd: JsString,
    case_sensitive: bool,
    entries: BTreeMap<JsString, NamedEntry>,
    id: SnapshotId,
}
impl MemorySnapshot {
    pub fn current_directory(&self) -> &[u8] {
        self.cwd.as_bytes()
    }
    fn resolve(&self, name: &[u8]) -> Result<Vec<u8>, Error> {
        let mut name = path::absolute(name, self.cwd.as_bytes());
        for _ in 0..40 {
            let mut replacement = None;
            for prefix in path::ancestors(&name) {
                let key = path::to_path(&prefix, b"", self.case_sensitive);
                if let Some(NamedEntry {
                    value: Entry::Symlink(target),
                    ..
                }) = self.entries.get(&key)
                {
                    let target = path::absolute(target.as_bytes(), &path::directory(&prefix));
                    let tail = &name[prefix.len()..];
                    let mut next = target;
                    next.extend_from_slice(tail);
                    replacement = Some(path::normalize(&next).into_owned());
                    break;
                }
            }
            match replacement {
                Some(next) => name = next,
                None => return Ok(name),
            }
        }
        Err(Error::SymlinkCycle)
    }
    fn get(&self, name: &[u8]) -> Result<Option<&NamedEntry>, Error> {
        let resolved = self.resolve(name)?;
        Ok(self
            .entries
            .get(&path::to_path(&resolved, b"", self.case_sensitive)))
    }
}
impl FileSystem for MemorySnapshot {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.case_sensitive
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        Some(self.id)
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        Ok(match self.get(path)? {
            Some(NamedEntry {
                value: Entry::File(content),
                ..
            }) => Some(content.clone()),
            _ => None,
        })
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        Ok(match self.get(path)? {
            Some(NamedEntry {
                value: Entry::File(content),
                ..
            }) => Some(FileInfo::basic(false, content.raw.len() as u64)),
            Some(NamedEntry {
                value: Entry::Directory,
                ..
            }) => Some(FileInfo::basic(true, 0)),
            _ => None,
        })
    }
    fn entries(&self, name: &[u8]) -> Result<Entries, Error> {
        let name = self.resolve(name)?;
        let key = path::to_path(&name, b"", self.case_sensitive);
        let mut result = Entries {
            symlinks: Some(BTreeSet::new()),
            ..Default::default()
        };
        for entry in self.entries.values() {
            if path::to_path(
                &path::directory(entry.name.as_bytes()),
                b"",
                self.case_sensitive,
            ) != key
                || entry.name.as_bytes() == name
            {
                continue;
            }
            let basename = JsString::from_bytes(path::base_name(entry.name.as_bytes()));
            if let Some(info) = self.stat(entry.name.as_bytes())? {
                if matches!(entry.value, Entry::Symlink(_)) {
                    result.symlinks.as_mut().unwrap().insert(basename.clone());
                }
                if info.directory {
                    result
                        .directories
                        .get_or_insert_with(Vec::new)
                        .push(basename);
                } else {
                    result.files.get_or_insert_with(Vec::new).push(basename);
                }
            }
        }
        if let Some(files) = &mut result.files {
            files.sort();
        }
        if let Some(directories) = &mut result.directories {
            directories.sort();
        }
        Ok(result)
    }
    fn realpath(&self, name: &[u8]) -> Result<JsString, Error> {
        let resolved = self.resolve(name)?;
        Ok(self
            .get(&resolved)?
            .map_or_else(|| JsString::from_bytes(resolved), |e| e.name.clone()))
    }
}
