//! `vfs/internal.Common` and `vfs/iovfs.From`: the compiler's filesystem over
//! an `io/fs` filesystem, dispatched by path root.
//!
//! The operations are total, as the pin's are: a missing file is `false`, `None`
//! or the echoed path, never an error. Errors come only from mutation and from
//! a walk. A path that is not absolute is a caller bug and panics with the
//! pinned sentence.
use crate::{
    iofs::{self, Fs, Info, IoError, Time, WalkError},
    Entries,
};
use std::{collections::BTreeSet, sync::Arc};
use tsr_jsstring::JsString;
use tsr_tspath as path;

/// # Panics
/// When the path has no root.
/// port: tsc/internal/vfs/internal/internal.go:RootLength
pub fn root_length(p: &[u8]) -> usize {
    let length = path::encoded_root_length(p);
    assert!(
        length != 0,
        "vfs: path {} is not absolute",
        iofs::go_quote(p)
    );
    path::root_length(p)
}
/// The root's exact spelling and the remainder without a trailing separator.
/// port: tsc/internal/vfs/internal/internal.go:SplitPath
pub fn split_path(p: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let normal = path::normalize(p);
    let root = root_length(&normal);
    (
        normal[..root].to_vec(),
        path::remove_trailing_directory_separator(&normal[root..]).to_vec(),
    )
}

pub type RootFor = Box<dyn Fn(&[u8]) -> Option<Arc<dyn Fs>> + Send + Sync>;
pub type IsReparsePoint = Box<dyn Fn(&[u8]) -> bool + Send + Sync>;

/// Source type: tsc/internal/vfs/internal/internal.go:Common
pub struct Common {
    /// The filesystem serving a root, by the root's exact spelling.
    pub root_for: RootFor,
    /// Consulted for irregular entries only.
    pub is_reparse_point: Option<IsReparsePoint>,
}
impl Common {
    /// An empty remainder becomes `.`.
    /// port: tsc/internal/vfs/internal/internal.go:Common.RootAndPath
    pub fn root_and_path(&self, p: &[u8]) -> (Option<Arc<dyn Fs>>, Vec<u8>, Vec<u8>) {
        let (root, mut rest) = split_path(p);
        if rest.is_empty() {
            rest = b".".to_vec();
        }
        ((self.root_for)(&root), root, rest)
    }
    /// `None` for every failure.
    /// port: tsc/internal/vfs/internal/internal.go:Common.Stat
    pub fn stat(&self, p: &[u8]) -> Option<Arc<Info>> {
        let (fs, _, rest) = self.root_and_path(p);
        iofs::stat(&*fs?, &rest).ok()
    }
    /// port: tsc/internal/vfs/internal/internal.go:Common.FileExists
    pub fn file_exists(&self, p: &[u8]) -> bool {
        self.stat(p).is_some_and(|info| !info.is_dir())
    }
    /// port: tsc/internal/vfs/internal/internal.go:Common.DirectoryExists
    pub fn directory_exists(&self, p: &[u8]) -> bool {
        self.stat(p).is_some_and(|info| info.is_dir())
    }
    /// Listing order is kept. A link is classified by what it points at and
    /// dropped when that does not resolve; other irregular entries are dropped.
    /// The link set is present even when empty.
    /// port: tsc/internal/vfs/internal/internal.go:Common.GetAccessibleEntries
    pub fn get_accessible_entries(&self, p: &[u8]) -> Entries {
        let mut result = Entries {
            symlinks: Some(BTreeSet::new()),
            ..Entries::default()
        };
        let add = |result: &mut Entries, name: &[u8], mode: iofs::FileMode, link: bool| {
            let name = JsString::from_bytes(name);
            if mode.is_dir() {
                result.directories.push(name.clone());
            } else if mode.is_regular() {
                result.files.push(name.clone());
            } else {
                return false;
            }
            if link {
                result
                    .symlinks
                    .get_or_insert_with(BTreeSet::new)
                    .insert(name);
            }
            true
        };
        for entry in self.entries(p) {
            let kind = entry.mode.file_type();
            if add(&mut result, &entry.name, kind, false) {
                continue;
            }
            let full = [p, b"/", &entry.name].concat();
            let through = kind.is_symlink()
                || kind.is_irregular()
                    && self
                        .is_reparse_point
                        .as_ref()
                        .is_some_and(|test| test(&full));
            if through {
                if let Some(info) = self.stat(&full) {
                    add(&mut result, &entry.name, info.mode, true);
                }
            }
        }
        result
    }
    /// port: tsc/internal/vfs/internal/internal.go:Common.getEntries
    fn entries(&self, p: &[u8]) -> Vec<Arc<Info>> {
        let (fs, _, rest) = self.root_and_path(p);
        fs.and_then(|fs| iofs::read_dir(&*fs, &rest).ok())
            .unwrap_or_default()
    }
    /// The callback sees rooted paths; the root itself is reported as the root.
    /// port: tsc/internal/vfs/internal/internal.go:Common.WalkDir
    pub fn walk_dir(&self, root: &[u8], visit: &mut iofs::WalkFn<'_>) -> Result<(), IoError> {
        let (fs, root_name, rest) = self.root_and_path(root);
        let Some(fs) = fs else {
            return Ok(());
        };
        iofs::walk_dir(&*fs, &rest, &mut |name, entry, error| {
            let name: &[u8] = if name == b"." { b"" } else { name };
            visit(&[root_name.as_slice(), name].concat(), entry, error)
        })
    }
    /// The content with a byte order mark removed and UTF-16 decoded.
    /// port: tsc/internal/vfs/internal/internal.go:Common.ReadFile
    pub fn read_file(&self, p: &[u8]) -> Option<Vec<u8>> {
        let (fs, _, rest) = self.root_and_path(p);
        let bytes = iofs::read_file(&*fs?, &rest).ok()?;
        Some(decode_bytes(&bytes))
    }
}
/// port: tsc/internal/vfs/internal/internal.go:decodeBytes
pub fn decode_bytes(bytes: &[u8]) -> Vec<u8> {
    match bytes {
        [0xff, 0xfe, rest @ ..] => decode_utf16(rest, u16::from_le_bytes),
        [0xfe, 0xff, rest @ ..] => decode_utf16(rest, u16::from_be_bytes),
        [0xef, 0xbb, 0xbf, rest @ ..] => rest.to_vec(),
        _ => bytes.to_vec(),
    }
}
/// A trailing odd byte is dropped; an unpaired surrogate becomes U+FFFD.
/// port: tsc/internal/vfs/internal/internal.go:decodeUtf16
fn decode_utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> Vec<u8> {
    let units = bytes.chunks_exact(2).map(|pair| unit([pair[0], pair[1]]));
    char::decode_utf16(units)
        .map(|decoded| decoded.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect::<String>()
        .into_bytes()
}

/// An `io/fs` filesystem that can also resolve a path.
/// Source type: tsc/internal/vfs/iovfs/iofs.go:RealpathFS
pub trait RealpathFs {
    fn realpath(&self, p: &[u8]) -> Result<Vec<u8>, IoError>;
}
/// An `io/fs` filesystem that can also be changed.
/// Source type: tsc/internal/vfs/iovfs/iofs.go:WritableFS
pub trait WritableFs {
    fn write_file(&self, p: &[u8], data: &[u8], perm: u32) -> Result<(), IoError>;
    fn append_file(&self, p: &[u8], data: &[u8], perm: u32) -> Result<(), IoError>;
    fn mkdir_all(&self, p: &[u8], perm: u32) -> Result<(), IoError>;
    fn remove(&self, p: &[u8]) -> Result<(), IoError>;
    fn chtimes(&self, p: &[u8], a_time: Time, m_time: Time) -> Result<(), IoError>;
}
/// What Go finds out with a type assertion, a backing states up front.
pub trait Backing: Fs {
    fn as_realpath(&self) -> Option<&dyn RealpathFs> {
        None
    }
    fn as_writable(&self) -> Option<&dyn WritableFs> {
        None
    }
}
impl Backing for iofs::MapFs {}

/// Source type: tsc/internal/vfs/iovfs/iofs.go:ioFS
pub struct IoVfs<B: Backing + 'static> {
    common: Common,
    case_sensitive: bool,
    backing: Arc<B>,
}
/// # Panics
/// Later, when a root other than `/` cannot be made into a sub-tree and is not a URL.
/// port: tsc/internal/vfs/iovfs/iofs.go:From
pub fn from<B: Backing + 'static>(
    backing: Arc<B>,
    use_case_sensitive_file_names: bool,
) -> IoVfs<B> {
    let roots = backing.clone();
    let root_for: RootFor = Box::new(move |root| {
        let whole: Arc<dyn Fs> = roots.clone();
        if root == b"/" {
            return Some(whole);
        }
        let directory = path::remove_trailing_directory_separator(root);
        match iofs::sub(whole, directory) {
            Ok(sub) => Some(sub),
            Err(_) if path::is_url(root) => None,
            Err(error) => panic!(
                "vfs: failed to create sub file system for {}: {error}",
                iofs::go_quote(directory)
            ),
        }
    });
    IoVfs {
        common: Common {
            root_for,
            is_reparse_point: None,
        },
        case_sensitive: use_case_sensitive_file_names,
        backing,
    }
}
fn strip_root(p: &[u8]) -> &[u8] {
    p.strip_prefix(b"/").unwrap_or(p)
}
impl<B: Backing + 'static> IoVfs<B> {
    /// port: tsc/internal/vfs/iovfs/iofs.go:ioFS.FSys
    pub fn fsys(&self) -> &Arc<B> {
        &self.backing
    }
    /// port: tsc/internal/vfs/iovfs/iofs.go:ioFS.UseCaseSensitiveFileNames
    pub fn use_case_sensitive_file_names(&self) -> bool {
        self.case_sensitive
    }
    /// port: tsc/internal/vfs/iovfs/iofs.go:ioFS.DirectoryExists
    pub fn directory_exists(&self, p: &[u8]) -> bool {
        self.common.directory_exists(p)
    }
    /// port: tsc/internal/vfs/iovfs/iofs.go:ioFS.FileExists
    pub fn file_exists(&self, p: &[u8]) -> bool {
        self.common.file_exists(p)
    }
    /// port: tsc/internal/vfs/iovfs/iofs.go:ioFS.GetAccessibleEntries
    pub fn get_accessible_entries(&self, p: &[u8]) -> Entries {
        self.common.get_accessible_entries(p)
    }
    /// port: tsc/internal/vfs/iovfs/iofs.go:ioFS.Stat
    pub fn stat(&self, p: &[u8]) -> Option<Arc<Info>> {
        root_length(p);
        self.common.stat(p)
    }
    /// port: tsc/internal/vfs/iovfs/iofs.go:ioFS.ReadFile
    pub fn read_file(&self, p: &[u8]) -> Option<Vec<u8>> {
        self.common.read_file(p)
    }
    /// port: tsc/internal/vfs/iovfs/iofs.go:ioFS.WalkDir
    pub fn walk_dir(&self, root: &[u8], visit: &mut iofs::WalkFn<'_>) -> Result<(), IoError> {
        self.common.walk_dir(root, visit)
    }
    /// A failure echoes the caller's path. Without a resolving backing the
    /// answer is the normalised path.
    /// port: tsc/internal/vfs/iovfs/iofs.go:ioFS.Realpath
    pub fn realpath(&self, p: &[u8]) -> Vec<u8> {
        let (root, rest) = split_path(p);
        let whole = [root, rest].concat();
        let Some(resolver) = self.backing.as_realpath() else {
            return whole;
        };
        match resolver.realpath(strip_root(&whole)) {
            Ok(real) if whole.starts_with(b"/") => [b"/".as_slice(), &real].concat(),
            Ok(real) => real,
            Err(_) => p.to_vec(),
        }
    }
    fn writable(&self, operation: &str) -> &dyn WritableFs {
        self.backing
            .as_writable()
            .unwrap_or_else(|| panic!("{operation} not supported"))
    }
    /// port: tsc/internal/vfs/iovfs/iofs.go:ioFS.Remove
    pub fn remove(&self, p: &[u8]) -> Result<(), IoError> {
        root_length(p);
        self.writable("remove").remove(strip_root(p))
    }
    /// port: tsc/internal/vfs/iovfs/iofs.go:ioFS.Chtimes
    pub fn chtimes(&self, p: &[u8], a_time: Time, m_time: Time) -> Result<(), IoError> {
        root_length(p);
        self.writable("chtimes")
            .chtimes(strip_root(p), a_time, m_time)
    }
    /// A failed write creates the directory and tries once more.
    /// port: tsc/internal/vfs/iovfs/iofs.go:ioFS.writeFileEnsuringDir
    fn write_ensuring_dir(
        &self,
        p: &[u8],
        operation: &str,
        write: impl Fn(&dyn WritableFs) -> Result<(), IoError>,
    ) -> Result<(), IoError> {
        root_length(p);
        if write(self.writable(operation)).is_ok() {
            return Ok(());
        }
        let directory = path::directory(&path::normalize(p));
        self.writable("mkdirAll")
            .mkdir_all(strip_root(&directory), 0o777)?;
        write(self.writable(operation))
    }
    /// port: tsc/internal/vfs/iovfs/iofs.go:ioFS.WriteFile
    pub fn write_file(&self, p: &[u8], content: &[u8]) -> Result<(), IoError> {
        self.write_ensuring_dir(p, "writeFile", |fs| {
            fs.write_file(strip_root(p), content, 0o666)
        })
    }
    /// port: tsc/internal/vfs/iovfs/iofs.go:ioFS.AppendFile
    pub fn append_file(&self, p: &[u8], content: &[u8]) -> Result<(), IoError> {
        self.write_ensuring_dir(p, "appendFile", |fs| {
            fs.append_file(strip_root(p), content, 0o666)
        })
    }
}

impl crate::vfstest::TestFs {
    /// The compiler filesystem over this test filesystem.
    pub fn into_vfs(self) -> IoVfs<Self> {
        let case_sensitive = self.use_case_sensitive_file_names();
        from(Arc::new(self), case_sensitive)
    }
}
impl Backing for crate::vfstest::TestFs {
    fn as_realpath(&self) -> Option<&dyn RealpathFs> {
        Some(self)
    }
    fn as_writable(&self) -> Option<&dyn WritableFs> {
        Some(self)
    }
}
impl RealpathFs for crate::vfstest::TestFs {
    fn realpath(&self, p: &[u8]) -> Result<Vec<u8>, IoError> {
        Self::realpath(self, p)
    }
}
impl WritableFs for crate::vfstest::TestFs {
    fn write_file(&self, p: &[u8], data: &[u8], perm: u32) -> Result<(), IoError> {
        Self::write_file(self, p, data, perm)
    }
    fn append_file(&self, p: &[u8], data: &[u8], perm: u32) -> Result<(), IoError> {
        Self::append_file(self, p, data, perm)
    }
    fn mkdir_all(&self, p: &[u8], perm: u32) -> Result<(), IoError> {
        Self::mkdir_all(self, p, perm)
    }
    fn remove(&self, p: &[u8]) -> Result<(), IoError> {
        Self::remove(self, p)
    }
    fn chtimes(&self, p: &[u8], a_time: Time, m_time: Time) -> Result<(), IoError> {
        Self::chtimes(self, p, a_time, m_time)
    }
}
/// What a walk callback returned, for adapters that forward it.
pub type WalkOutcome = Result<(), WalkError>;
