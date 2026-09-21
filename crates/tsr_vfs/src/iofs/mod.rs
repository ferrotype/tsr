//! The parts of Go's `io/fs` the pinned filesystem adapters are written against.
//!
//! The pin builds its virtual filesystems on `fs.FS`: a single `Open`, with
//! stat, directory listing, whole-file reads, sub-trees and walks derived from
//! it. These are the same derivations over a Rust trait, with the same path
//! validity rule, the same error shapes and the same walk control flow.
//! Names are slash-separated bytes relative to the filesystem's root; `.` is
//! the root itself.
use std::{fmt, sync::Arc};

pub(crate) mod mapfs;
mod time;
pub use mapfs::{MapFile, MapFs};
pub use time::Time;

/// The mode bits the pin reads: three type bits and the permission bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileMode(pub u32);
impl FileMode {
    pub const DIR: Self = Self(1 << 31);
    pub const SYMLINK: Self = Self(1 << 27);
    pub const IRREGULAR: Self = Self(1 << 19);
    /// Every type bit Go defines; a mode with none of them set is a regular file.
    const TYPE_MASK: u32 =
        (1 << 31) | (1 << 27) | (1 << 25) | (1 << 24) | (1 << 26) | (1 << 21) | (1 << 19);
    const PERM_MASK: u32 = 0o777;

    pub fn is_dir(self) -> bool {
        self.0 & Self::DIR.0 != 0
    }
    pub fn is_symlink(self) -> bool {
        self.0 & Self::SYMLINK.0 != 0
    }
    pub fn is_irregular(self) -> bool {
        self.0 & Self::IRREGULAR.0 != 0
    }
    pub fn is_regular(self) -> bool {
        self.0 & Self::TYPE_MASK == 0
    }
    /// The type bits alone.
    #[must_use]
    pub fn file_type(self) -> Self {
        Self(self.0 & Self::TYPE_MASK)
    }
    pub fn perm(self) -> u32 {
        self.0 & Self::PERM_MASK
    }
}
impl std::ops::BitOr for FileMode {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Go's `fs.FileInfo.Sys`: an opaque value the pin tests by dynamic type.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Sys {
    #[default]
    Nil,
    Int(i64),
    /// The test filesystem's wrapper: the entry's spelled path and the value
    /// the caller originally supplied.
    Wrapper {
        original: Box<Sys>,
        realpath: Vec<u8>,
    },
}

/// The sentinel errors of `io/fs` plus the shapes the pin wraps them in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IoError {
    NotExist,
    Invalid,
    Eof,
    /// `&fs.PathError{Op, Path, Err}`.
    Path {
        op: &'static str,
        path: Vec<u8>,
        source: Box<IoError>,
    },
    /// A symbolic link whose target does not resolve. `fs.ErrNotExist` is not
    /// in its chain.
    BrokenSymlink {
        from: Vec<u8>,
        to: Vec<u8>,
    },
    /// An error built from a fixed sentence, optionally wrapping another.
    Message {
        text: String,
        source: Option<Box<IoError>>,
    },
}
impl IoError {
    pub fn path(op: &'static str, path: &[u8], source: Self) -> Self {
        Self::Path {
            op,
            path: path.to_vec(),
            source: Box::new(source),
        }
    }
    pub fn message(text: impl Into<String>) -> Self {
        Self::Message {
            text: text.into(),
            source: None,
        }
    }
    /// `errors.Is(err, fs.ErrNotExist)`.
    pub fn is_not_exist(&self) -> bool {
        match self {
            Self::NotExist => true,
            Self::Path { source, .. }
            | Self::Message {
                source: Some(source),
                ..
            } => source.is_not_exist(),
            _ => false,
        }
    }
    pub fn is_broken_symlink(&self) -> bool {
        match self {
            Self::BrokenSymlink { .. } => true,
            Self::Path { source, .. }
            | Self::Message {
                source: Some(source),
                ..
            } => source.is_broken_symlink(),
            _ => false,
        }
    }
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotExist => f.write_str("file does not exist"),
            Self::Invalid => f.write_str("invalid argument"),
            Self::Eof => f.write_str("EOF"),
            Self::Path { op, path, source } => {
                write!(f, "{op} {}: {source}", String::from_utf8_lossy(path))
            }
            Self::BrokenSymlink { from, to } => {
                write!(f, "broken symlink {} -> {}", go_quote(from), go_quote(to))
            }
            Self::Message { text, .. } => f.write_str(text),
        }
    }
}
impl std::error::Error for IoError {}

/// Go's `%q` for the paths these errors carry.
pub fn go_quote(bytes: &[u8]) -> String {
    tsr_jsstring::go_quote(bytes)
}

/// What `Stat` answers. `name` is the base name the filesystem reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    pub name: Vec<u8>,
    pub size: u64,
    pub mode: FileMode,
    pub mod_time: Time,
    pub sys: Sys,
}
impl Info {
    pub fn is_dir(&self) -> bool {
        self.mode.is_dir()
    }
}

/// An open file or directory. A directory hands out its entries in order and
/// remembers how many it has handed out.
#[derive(Clone, Debug)]
pub struct Handle {
    pub path: Vec<u8>,
    pub info: Arc<Info>,
    content: Content,
    /// Applied to each entry as `read_dir` hands it out, not when the directory
    /// is opened, so a wrapper's per-entry failure surfaces at the read.
    entry_map: Option<fn(&Info) -> Info>,
}
#[derive(Clone, Debug)]
enum Content {
    File(Arc<[u8]>),
    Directory {
        entries: Vec<Arc<Info>>,
        offset: usize,
    },
}
impl Handle {
    pub fn file(path: &[u8], info: Info, data: Arc<[u8]>) -> Self {
        Self {
            path: path.to_vec(),
            info: Arc::new(info),
            content: Content::File(data),
            entry_map: None,
        }
    }
    pub fn directory(path: &[u8], info: Info, entries: Vec<Info>) -> Self {
        let entries = entries.into_iter().map(Arc::new).collect();
        Self {
            path: path.to_vec(),
            info: Arc::new(info),
            content: Content::Directory { entries, offset: 0 },
            entry_map: None,
        }
    }
    /// Whether the handle implements Go's `fs.ReadDirFile`.
    pub fn is_read_dir_file(&self) -> bool {
        matches!(self.content, Content::Directory { .. })
    }
    /// The whole content; reading a directory is invalid.
    pub fn read_all(&self) -> Result<Arc<[u8]>, IoError> {
        match &self.content {
            Content::File(data) => Ok(data.clone()),
            Content::Directory { .. } => Err(IoError::path("read", &self.path, IoError::Invalid)),
        }
    }
    /// `ReadDir(n)`: at most `n` entries when positive, all remaining otherwise,
    /// and `Eof` for a positive `n` with nothing left.
    pub fn read_dir(&mut self, count: isize) -> Result<Vec<Arc<Info>>, IoError> {
        let Content::Directory { entries, offset } = &mut self.content else {
            return Err(IoError::path(
                "readdir",
                &self.path,
                IoError::message("not implemented"),
            ));
        };
        let mut available = entries.len() - *offset;
        if available == 0 && count > 0 {
            return Err(IoError::Eof);
        }
        if count > 0 {
            available = available.min(count.unsigned_abs());
        }
        let list = entries[*offset..*offset + available].to_vec();
        *offset += available;
        Ok(match self.entry_map {
            Some(map) => list.iter().map(|entry| Arc::new(map(entry))).collect(),
            None => list,
        })
    }
    /// Rewrites the reported metadata and maps entries as they are read.
    #[must_use]
    pub fn with_info(mut self, info: Info, entry_map: fn(&Info) -> Info) -> Self {
        self.info = Arc::new(info);
        self.entry_map = Some(entry_map);
        self
    }
}

/// Source type: io/fs.FS
pub trait Fs: Send + Sync {
    fn open(&self, name: &[u8]) -> Result<Handle, IoError>;
}

/// Unrooted, slash-separated, no empty, `.` or `..` element, valid UTF-8. The
/// root is the single name `.`.
/// Source operation: io/fs.ValidPath
pub fn valid_path(name: &[u8]) -> bool {
    if std::str::from_utf8(name).is_err() {
        return false;
    }
    name == b"."
        || name
            .split(|&b| b == b'/')
            .all(|element| !matches!(element, b"" | b"." | b".."))
}
/// Source operation: io/fs.Stat
pub fn stat(fs: &dyn Fs, name: &[u8]) -> Result<Arc<Info>, IoError> {
    fs.open(name).map(|handle| handle.info)
}
/// All entries, sorted by name.
/// Source operation: io/fs.ReadDir
pub fn read_dir(fs: &dyn Fs, name: &[u8]) -> Result<Vec<Arc<Info>>, IoError> {
    let mut handle = fs.open(name)?;
    if !handle.is_read_dir_file() {
        return Err(IoError::path(
            "readdir",
            name,
            IoError::message("not implemented"),
        ));
    }
    let mut list = handle.read_dir(-1)?;
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(list)
}
/// Source operation: io/fs.ReadFile
pub fn read_file(fs: &dyn Fs, name: &[u8]) -> Result<Arc<[u8]>, IoError> {
    fs.open(name)?.read_all()
}

/// What a walk callback asks for next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalkError {
    SkipDir,
    SkipAll,
    Other(IoError),
}
pub type WalkFn<'a> =
    dyn FnMut(&[u8], Option<&Info>, Option<IoError>) -> Result<(), WalkError> + 'a;

/// Lexical order, the root first. A stat failure on the root is reported to the
/// callback; `SkipDir` and `SkipAll` from it end the walk without an error.
/// Source operation: io/fs.WalkDir
pub fn walk_dir(fs: &dyn Fs, root: &[u8], visit: &mut WalkFn<'_>) -> Result<(), IoError> {
    let outcome = match stat(fs, root) {
        Err(error) => visit(root, None, Some(error)),
        Ok(info) => walk(fs, root, &info, visit),
    };
    match outcome {
        Ok(()) | Err(WalkError::SkipDir | WalkError::SkipAll) => Ok(()),
        Err(WalkError::Other(error)) => Err(error),
    }
}
fn walk(fs: &dyn Fs, name: &[u8], entry: &Info, visit: &mut WalkFn<'_>) -> Result<(), WalkError> {
    let first = visit(name, Some(entry), None);
    if first.is_err() || !entry.is_dir() {
        return match first {
            Err(WalkError::SkipDir) if entry.is_dir() => Ok(()),
            other => other,
        };
    }
    let children = match read_dir(fs, name) {
        Ok(children) => children,
        Err(error) => {
            // A second call for the same directory, carrying the listing error.
            match visit(name, Some(entry), Some(error)) {
                Ok(()) => Vec::new(),
                Err(WalkError::SkipDir) => return Ok(()),
                Err(other) => return Err(other),
            }
        }
    };
    for child in children {
        let child_name = join(name, &child.name);
        match walk(fs, &child_name, &child, visit) {
            Ok(()) => {}
            Err(WalkError::SkipDir) => break,
            Err(other) => return Err(other),
        }
    }
    Ok(())
}
/// Go's `path.Join` for two already-clean elements.
pub fn join(directory: &[u8], name: &[u8]) -> Vec<u8> {
    if directory == b"." {
        return name.to_vec();
    }
    [directory, b"/", name].concat()
}

/// A sub-tree view. Errors shorten their path back to the caller's spelling.
/// Source operation: io/fs.Sub
pub fn sub(fs: Arc<dyn Fs>, directory: &[u8]) -> Result<Arc<dyn Fs>, IoError> {
    if !valid_path(directory) {
        return Err(IoError::path("sub", directory, IoError::Invalid));
    }
    if directory == b"." {
        return Ok(fs);
    }
    Ok(Arc::new(SubFs {
        fs,
        directory: directory.to_vec(),
    }))
}
struct SubFs {
    fs: Arc<dyn Fs>,
    directory: Vec<u8>,
}
impl Fs for SubFs {
    fn open(&self, name: &[u8]) -> Result<Handle, IoError> {
        if !valid_path(name) {
            return Err(IoError::path("open", name, IoError::Invalid));
        }
        let full = join(&self.directory, name);
        let full = if name == b"." {
            self.directory.clone()
        } else {
            full
        };
        self.fs.open(&full).map_err(|error| match error {
            IoError::Path { op, path, source } => {
                let short = if path == self.directory {
                    b".".to_vec()
                } else {
                    path.strip_prefix(self.directory.as_slice())
                        .and_then(|rest| rest.strip_prefix(b"/"))
                        .map_or(path.clone(), <[u8]>::to_vec)
                };
                IoError::Path {
                    op,
                    path: short,
                    source,
                }
            }
            other => other,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::IoError;

    #[test]
    fn broken_symlink_detection_unwraps_path_and_message_errors() {
        // vfstest.isBrokenSymlinkError uses errors.AsType, including fs.PathError.Unwrap.
        let cause = IoError::BrokenSymlink {
            from: b"link".to_vec(),
            to: b"missing".to_vec(),
        };
        let wrapped = IoError::Message {
            text: "opening fixture".into(),
            source: Some(Box::new(IoError::path("open", b"link", cause))),
        };
        assert!(wrapped.is_broken_symlink());
        assert!(!wrapped.is_not_exist());
        assert!(!IoError::path("open", b"missing", IoError::NotExist).is_broken_symlink());
    }
}
