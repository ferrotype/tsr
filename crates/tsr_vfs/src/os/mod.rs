//! Live OS filesystem. Unlike MemorySnapshot it advertises no snapshot identity.
//! Blocking calls have process-wide limits; callbacks release permits on unwind.
#[cfg(unix)]
mod remove;
mod snapshot;
pub use snapshot::ScopedOsFs;
mod cache_dir;
mod dir;
pub mod native;
use crate::{
    iofs::{Time, WalkError},
    iovfs::Common,
    Entries, Error, FileContent, FileInfo, FileSystem, SnapshotId, WalkCallback, WalkControl,
    WalkEntry,
};
pub use dir::{info, mode};
use std::{
    io::Write,
    sync::{Arc, OnceLock},
};
use tsr_core::semaphore::LimitedSemaphore;
use tsr_jsstring::JsString;
static BLOCKING: LimitedSemaphore = LimitedSemaphore::new(128);
static READ: LimitedSemaphore = LimitedSemaphore::new(128);
static WRITE: LimitedSemaphore = LimitedSemaphore::new(32);
pub struct OsFs {
    common: Common,
    case_sensitive: bool,
}
/// port: tsc/internal/vfs/osvfs/os.go:FS
pub fn fs() -> &'static OsFs {
    static FS: OnceLock<OsFs> = OnceLock::new();
    FS.get_or_init(|| OsFs {
        common: Common {
            root_for: Box::new(|root| {
                Some(Arc::new(dir::Dir {
                    root: native::path(root),
                }))
            }),
            is_reparse_point: Some(Box::new(is_reparse_point)),
        },
        case_sensitive: case_sensitive(),
    })
}
fn case_sensitive() -> bool {
    if cfg!(windows) {
        return false;
    }
    if cfg!(target_arch = "wasm32") {
        return true;
    }
    let exe = native::executable().expect("vfs: failed to get executable path");
    let swapped = swap_case(&exe);
    match native::metadata(native::path(&swapped)) {
        Ok(_) => false,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => panic!(
            "vfs: failed to stat {}: {e}",
            crate::iofs::go_quote(&swapped)
        ),
    }
}
/// port: tsc/internal/vfs/osvfs/os.go:swapCase
pub fn swap_case(text: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        let (rune, n) = tsr_jsstring::wtf8::decode_utf8(&text[at..]);
        at += n;
        let upper = tsr_jsstring::helpers::simple_upper_go(rune);
        let mapped = if upper == rune {
            tsr_jsstring::helpers::simple_lower_go(rune)
        } else {
            upper
        };
        let mut bytes = [0; 4];
        output.extend_from_slice(
            char::from_u32(mapped as u32)
                .expect("Unicode case mapping returns a scalar")
                .encode_utf8(&mut bytes)
                .as_bytes(),
        );
    }
    output
}
#[derive(Clone, Copy)]
pub enum WriteMode {
    Truncate,
    Append,
    CreateNew,
    ReadOnly,
    WriteOnly,
}
impl OsFs {
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.writeFileWithFlag
    pub fn write_with_mode(&self, path: &[u8], data: &[u8], mode: WriteMode) -> Result<(), Error> {
        let _permit = WRITE.acquire();
        let mut options = std::fs::OpenOptions::new();
        match mode {
            WriteMode::Truncate => {
                options.write(true).create(true).truncate(true);
            }
            WriteMode::Append => {
                options.append(true).create(true);
            }
            WriteMode::CreateNew => {
                options.write(true).create_new(true);
            }
            WriteMode::ReadOnly => {
                options.read(true);
            }
            WriteMode::WriteOnly => {
                options.write(true);
            }
        }
        let mut file = options
            .open(native::path(path))
            .map_err(|e| native::failure("open", path, e))?;
        file.write_all(data)
            .map_err(|e| Error::from(native::failure("write", path, e)))
    }
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.ensureDirectoryExists
    pub fn ensure_directory(&self, path: &[u8]) -> Result<(), Error> {
        let _permit = BLOCKING.acquire();
        native::mkdir_all(path).map_err(Error::from)
    }
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.writeFileEnsuringDir
    pub fn write_ensuring_directory(
        &self,
        path: &[u8],
        data: &[u8],
        mode: WriteMode,
    ) -> Result<(), Error> {
        crate::iovfs::root_length(path);
        if self.write_with_mode(path, data, mode).is_ok() {
            return Ok(());
        }
        self.ensure_directory(&tsr_tspath::directory(&tsr_tspath::normalize(path)))?;
        self.write_with_mode(path, data, mode)
    }
}
/// Stack-owned equivalent of the native pooled walker. Clearing releases the
/// callback's captures; invoking a cleared binding fails rather than reusing it.
pub struct LimitedWalk<C> {
    inner: Option<C>,
}
impl<C: FnMut(&[u8], Option<&WalkEntry>, Option<Error>) -> Result<WalkControl, Error>>
    LimitedWalk<C>
{
    /// port: tsc/internal/vfs/osvfs/os.go:getLimitedWalkDirFunc
    pub fn new(inner: C) -> Self {
        Self { inner: Some(inner) }
    }
    pub fn is_bound(&self) -> bool {
        self.inner.is_some()
    }
    /// port: tsc/internal/vfs/osvfs/os.go:putLimitedWalkDirFunc
    pub fn clear(&mut self) {
        self.inner = None;
    }
    /// port: tsc/internal/vfs/osvfs/os.go:limitedWalkDirFunc.walker
    pub fn visit(
        &mut self,
        path: &[u8],
        entry: Option<&WalkEntry>,
        error: Option<Error>,
    ) -> Result<WalkControl, Error> {
        let _permit = BLOCKING.acquire();
        self.inner.as_mut().expect("unbound walk callback")(path, entry, error)
    }
}
impl FileSystem for OsFs {
    fn snapshot_id(&self) -> Option<SnapshotId> {
        None
    }
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.UseCaseSensitiveFileNames
    fn use_case_sensitive_file_names(&self) -> bool {
        self.case_sensitive
    }
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.ReadFile
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        let _p = READ.acquire();
        Ok(self.common.read_file(path).map(FileContent::loaded))
    }
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.FileExists
    fn file_exists(&self, path: &[u8]) -> Result<bool, Error> {
        let _p = BLOCKING.acquire();
        Ok(self.common.file_exists(path))
    }
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.DirectoryExists
    fn directory_exists(&self, path: &[u8]) -> Result<bool, Error> {
        let _p = BLOCKING.acquire();
        Ok(self.common.directory_exists(path))
    }
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.Stat
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        let _p = BLOCKING.acquire();
        Ok(self.common.stat(path).map(|i| FileInfo::from(&*i)))
    }
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.GetAccessibleEntries
    fn entries(&self, path: &[u8]) -> Result<Entries, Error> {
        let _p = BLOCKING.acquire();
        Ok(self.common.get_accessible_entries(path))
    }
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.Realpath
    fn realpath(&self, path: &[u8]) -> Result<JsString, Error> {
        let _p = BLOCKING.acquire();
        Ok(JsString::from_bytes(realpath(path)))
    }
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.WriteFile
    fn write_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        self.write_ensuring_directory(path, data, WriteMode::Truncate)
    }
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.AppendFile
    fn append_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        self.write_ensuring_directory(path, data, WriteMode::Append)
    }
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.Remove
    fn remove(&self, path: &[u8]) -> Result<(), Error> {
        let _p = BLOCKING.acquire();
        #[cfg(unix)]
        {
            remove::remove_all(path).map_err(Error::from)
        }
        #[cfg(not(unix))]
        {
            if path.is_empty() {
                return Ok(());
            }
            let p = native::path(path);
            let result = match native::symlink_metadata(&p) {
                Ok(m) if m.is_dir() => std::fs::remove_dir_all(p),
                Ok(_) => std::fs::remove_file(p),
                Err(e) => Err(e),
            };
            match result {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(native::failure("remove", path, e).into()),
            }
        }
    }
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.Chtimes
    fn change_times(&self, path: &[u8], a: Time, m: Time) -> Result<(), Error> {
        let _p = BLOCKING.acquire();
        #[cfg(unix)]
        {
            use rustix::fs::{utimensat, AtFlags, Timestamps, CWD};
            // os.chtimesUtimes uses NsecToTimespec(time.UnixNano()). The
            // signed nanosecond intermediate wraps outside its 1678..2262 span.
            let wrap = |t: Time| {
                let (s, n) = t.unix();
                let nanos = s.wrapping_mul(1_000_000_000).wrapping_add(i64::from(n));
                (
                    nanos.div_euclid(1_000_000_000),
                    nanos.rem_euclid(1_000_000_000),
                )
            };
            let (a_sec, a_nano) = wrap(a);
            let (m_sec, m_nano) = wrap(m);
            let times = Timestamps {
                last_access: rustix::fs::Timespec {
                    tv_sec: a_sec,
                    tv_nsec: if a.is_zero() {
                        rustix::fs::UTIME_OMIT
                    } else {
                        a_nano
                    },
                },
                last_modification: rustix::fs::Timespec {
                    tv_sec: m_sec,
                    tv_nsec: if m.is_zero() {
                        rustix::fs::UTIME_OMIT
                    } else {
                        m_nano
                    },
                },
            };
            utimensat(CWD, native::path(path), &times, AtFlags::empty())
                .map_err(|e| Error::from(native::failure("chtimes", path, e.into())))
        }
        #[cfg(not(unix))]
        {
            let mut times = std::fs::FileTimes::new();
            if !a.is_zero() {
                times = times.set_accessed(a.to_system_time().ok_or(Error::InvalidPath)?);
            }
            if !m.is_zero() {
                times = times.set_modified(m.to_system_time().ok_or(Error::InvalidPath)?);
            }
            std::fs::File::open(native::path(path))
                .and_then(|f| f.set_times(times))
                .map_err(|e| native::failure("chtimes", path, e).into())
        }
    }
    /// port: tsc/internal/vfs/osvfs/os.go:osFS.WalkDir
    fn walk_dir(&self, path: &[u8], visit: &mut WalkCallback<'_>) -> Result<(), Error> {
        let mut visit = |p: &[u8], e: Option<&WalkEntry>, err| visit(p, e, err);
        let mut limited = LimitedWalk::new(&mut visit);
        let mut failure = None;
        let result = self.common.walk_dir(path, &mut |p, i, e| {
            let entry = i.map(|i| WalkEntry {
                name: JsString::from_bytes(i.name.as_slice()),
                info: FileInfo::from(i),
                symlink: i.mode.is_symlink(),
            });
            match limited.visit(p, entry.as_ref(), e.map(Error::from)) {
                Ok(WalkControl::Continue) => Ok(()),
                Ok(WalkControl::SkipDir) => Err(WalkError::SkipDir),
                Ok(WalkControl::SkipAll) => Err(WalkError::SkipAll),
                Err(e) => {
                    failure = Some(e);
                    Err(WalkError::SkipAll)
                }
            }
        });
        limited.clear();
        if let Some(e) = failure {
            return Err(e);
        }
        result.map_err(Error::from)
    }
}
/// port: tsc/internal/vfs/osvfs/os.go:osFSRealpath
pub fn realpath(name: &[u8]) -> Vec<u8> {
    crate::iovfs::root_length(name);
    let Ok(resolved) = native::realpath(name) else {
        return name.to_vec();
    };
    let Ok(absolute) = std::path::absolute(native::path(&resolved)) else {
        return name.to_vec();
    };
    tsr_tspath::normalize_slashes(&native::bytes(&absolute)).into_owned()
}
/// The versioned directory used by automatic type acquisition at this pin.
/// port: tsc/internal/vfs/osvfs/os.go:GetGlobalTypingsCacheLocation
pub fn global_typings_cache_location() -> Vec<u8> {
    let base =
        cache_dir::user_cache_dir(|key| std::env::var_os(key)).unwrap_or_else(std::env::temp_dir);
    tsr_tspath::combine(
        &native::bytes(&base),
        &[
            if cfg!(windows) {
                b"Microsoft/TypeScript"
            } else {
                b"typescript"
            },
            b"7.1",
        ],
    )
}

/// port: tsc/internal/vfs/osvfs/os.go:isReparsePoint
pub fn is_reparse_point(name: &[u8]) -> bool {
    native::is_symlink_or_reparse_point(name)
}
