//! Native path primitives. Unix paths remain bytes; Windows uses Go-style UTF-8
//! decoding at its UTF-16 boundary. No process-global working directory changes.
use crate::iofs::IoError;
use std::path::{Path, PathBuf};
#[cfg(unix)]
pub fn path(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}
#[cfg(not(unix))]
pub fn path(bytes: &[u8]) -> PathBuf {
    let mut text = String::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let (r, n) = tsr_jsstring::wtf8::decode_utf8(&bytes[offset..]);
        offset += n;
        text.push(char::from_u32(r as u32).unwrap_or('\u{fffd}'));
    }
    PathBuf::from(text)
}
#[cfg(unix)]
pub fn bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}
#[cfg(not(unix))]
pub fn bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}
pub fn failure(op: &'static str, path: &[u8], error: std::io::Error) -> IoError {
    IoError::path(op, path, error.into())
}
/// port: tsc/internal/nativepath/symlink_other.go:IsSymlinkOrReparsePoint
#[cfg(not(windows))]
pub fn is_symlink_or_reparse_point(name: &[u8]) -> bool {
    symlink_metadata(path(name)).is_ok_and(|m| m.file_type().is_symlink())
}
#[cfg(windows)]
pub fn is_symlink_or_reparse_point(name: &[u8]) -> bool {
    use std::os::windows::fs::MetadataExt;
    symlink_metadata(path(name)).is_ok_and(|m| m.file_attributes() & 0x400 != 0)
}
/// Retry only a raw syscall EINTR, not a caller-created or wrapped error that
/// merely has `ErrorKind::Interrupted`.
/// port: tsc/internal/nativepath/eintr_unix.go:ignoringEINTR
#[cfg(unix)]
pub(super) fn ignoring_eintr<T>(
    mut operation: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    loop {
        match operation() {
            Err(error) if error.raw_os_error() == Some(rustix::io::Errno::INTR.raw_os_error()) => {}
            result => return result,
        }
    }
}
// Rust's Unix std metadata/readlink functions do not retry EINTR; Go's os
// functions do. Keep retries around each syscall, not whole filesystem actions.
#[cfg(not(unix))]
pub(super) fn ignoring_eintr<T>(
    mut operation: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    operation()
}
pub(super) fn metadata(path: impl AsRef<Path>) -> std::io::Result<std::fs::Metadata> {
    ignoring_eintr(|| std::fs::metadata(path.as_ref()))
}
pub(super) fn symlink_metadata(path: impl AsRef<Path>) -> std::io::Result<std::fs::Metadata> {
    ignoring_eintr(|| std::fs::symlink_metadata(path.as_ref()))
}
#[cfg(not(windows))]
fn read_link(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    ignoring_eintr(|| std::fs::read_link(path.as_ref()))
}
/// port: tsc/internal/nativepath/realpath_linux.go:Realpath
#[cfg(target_os = "linux")]
pub fn realpath(name: &[u8]) -> Result<Vec<u8>, IoError> {
    use rustix::fs::{openat, Mode, OFlags, CWD};
    use std::os::fd::AsRawFd;
    static PROC: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*PROC.get_or_init(|| metadata("/proc/self/fd").is_ok()) {
        return eval_symlinks(name);
    }
    let fd = ignoring_eintr(|| {
        openat(
            CWD,
            path(name),
            OFlags::PATH | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)
    })
    .map_err(|error| failure("open", name, error))?;
    let proc_path = format!("/proc/self/fd/{}", fd.as_raw_fd());
    let resolved = read_link(&proc_path).map_err(|e| failure("readlink", name, e))?;
    Ok(bytes(&resolved))
}
/// port: tsc/internal/nativepath/realpath_other.go:Realpath
#[cfg(all(not(windows), not(target_os = "linux")))]
pub fn realpath(name: &[u8]) -> Result<Vec<u8>, IoError> {
    eval_symlinks(name)
}
#[cfg(windows)]
pub fn realpath(name: &[u8]) -> Result<Vec<u8>, IoError> {
    let resolved = std::fs::canonicalize(path(name)).map_err(|e| failure("CreateFile", name, e))?;
    let mut bytes = bytes(&resolved);
    if let Some(tail) = bytes.strip_prefix(b"\\\\?\\UNC\\") {
        let mut out = b"\\\\".to_vec();
        out.extend_from_slice(tail);
        bytes = out;
    } else if let Some(tail) = bytes.strip_prefix(b"\\\\?\\") {
        bytes = tail.to_vec();
    }
    Ok(bytes)
}
/// filepath.walkSymlinks on Unix: preserve relative output and the error's
/// operation, unlike canonicalize which always returns an absolute path.
#[cfg(not(windows))]
fn eval_symlinks(name: &[u8]) -> Result<Vec<u8>, IoError> {
    let mut name = name.to_vec();
    let mut volume = usize::from(name.starts_with(b"/"));
    let mut dest = name[..volume].to_vec();
    let mut at = volume;
    let mut links = 0;
    while at < name.len() {
        while at < name.len() && name[at] == b'/' {
            at += 1;
        }
        let start = at;
        while at < name.len() && name[at] != b'/' {
            at += 1;
        }
        let part = &name[start..at];
        if part.is_empty() {
            break;
        }
        if part == b"." {
            continue;
        }
        if part == b".." {
            let split = dest[volume..]
                .iter()
                .rposition(|b| *b == b'/')
                .map(|i| i + volume);
            match split {
                Some(i) if &dest[i + 1..] != b".." => dest.truncate(i),
                _ => {
                    if dest.len() > volume {
                        dest.push(b'/');
                    }
                    dest.extend_from_slice(b"..");
                }
            }
            continue;
        }
        if !dest.is_empty() && !dest.ends_with(b"/") {
            dest.push(b'/');
        }
        dest.extend_from_slice(part);
        let info = symlink_metadata(path(&dest)).map_err(|e| failure("lstat", &dest, e))?;
        if !info.file_type().is_symlink() {
            if !info.is_dir() && at < name.len() {
                return Err(std::io::Error::from(std::io::ErrorKind::NotADirectory).into());
            }
            continue;
        }
        links += 1;
        if links > 255 {
            return Err(IoError::message("EvalSymlinks: too many links"));
        }
        let target = bytes(&read_link(path(&dest)).map_err(|e| failure("readlink", &dest, e))?);
        let mut next = target.clone();
        next.extend_from_slice(&name[at..]);
        name = next;
        if target.starts_with(b"/") {
            dest = b"/".to_vec();
            volume = 1;
            at = 1;
        } else {
            let split = dest[volume..]
                .iter()
                .rposition(|b| *b == b'/')
                .map(|i| i + volume)
                .unwrap_or(volume);
            dest.truncate(split);
            at = 0;
        }
    }
    Ok(crate::iofs::mapfs::clean_join(&dest, b""))
}
/// port: tsc/internal/osutil/osutil.go:Executable
pub fn executable() -> Result<Vec<u8>, IoError> {
    std::env::current_exe()
        .and_then(std::path::absolute)
        .map(|p| bytes(&p))
        .map_err(IoError::from)
}
/// port: tsc/internal/osutil/osutil.go:Args
pub fn args() -> Vec<Vec<u8>> {
    std::env::args_os().map(|s| bytes(Path::new(&s))).collect()
}

/// `os.MkdirAll(name, 0o777)`, as osvfs.ensureDirectoryExists calls it. Go
/// 1.27.1 (os/path.go) makes one level at a time and os.Mkdir passes each
/// mkdir(2) through ignoringEINTR; std's `create_dir_all` retries nothing and
/// reports the requested path, where Go reports the level that failed.
#[cfg(unix)]
pub(super) fn mkdir_all(name: &[u8]) -> Result<(), IoError> {
    use std::os::unix::fs::DirBuilderExt;
    mkdir_all_with(name, &mut |level| {
        #[cfg(test)]
        tests::injected_interruption()?;
        std::fs::DirBuilder::new().mode(0o777).create(level)
    })
}
#[cfg(not(unix))]
pub(super) fn mkdir_all(name: &[u8]) -> Result<(), IoError> {
    match metadata(path(name)) {
        Ok(m) if m.is_dir() => return Ok(()),
        Ok(_) => {
            return Err(failure(
                "mkdir",
                name,
                std::io::Error::from(std::io::ErrorKind::NotADirectory),
            ))
        }
        Err(_) => {}
    }
    std::fs::create_dir_all(path(name)).map_err(|e| failure("mkdir", name, e))
}
/// The os.MkdirAll recursion with the raw mkdir(2) supplied by the caller, so
/// a test can interrupt it; stat and lstat are the retrying helpers above.
#[cfg(unix)]
fn mkdir_all_with(
    name: &[u8],
    mkdir: &mut dyn FnMut(&Path) -> std::io::Result<()>,
) -> Result<(), IoError> {
    // Fast path: if we can tell whether name is a directory or file, stop with
    // success or error.
    match metadata(path(name)) {
        Ok(m) if m.is_dir() => return Ok(()),
        Ok(_) => {
            return Err(failure(
                "mkdir",
                name,
                std::io::Error::from(std::io::ErrorKind::NotADirectory),
            ))
        }
        Err(_) => {}
    }
    // Slow path: drop trailing separators and the last element; a parent
    // longer than the (empty, on Unix) volume name is made first.
    let mut end = name.len();
    while end > 0 && name[end - 1] == b'/' {
        end -= 1;
    }
    while end > 0 && name[end - 1] != b'/' {
        end -= 1;
    }
    let parent = &name[..end.saturating_sub(1)];
    if !parent.is_empty() {
        mkdir_all_with(parent, mkdir)?;
    }
    let level = path(name);
    match ignoring_eintr(|| mkdir(&level)) {
        Ok(()) => Ok(()),
        // Arguments like "foo/." fail with EEXIST; double-check the directory.
        Err(_) if symlink_metadata(&level).is_ok_and(|m| m.is_dir()) => Ok(()),
        Err(error) => Err(failure("mkdir", name, error)),
    }
}
/// `os.ReadDir`'s open: openDirNolog passes open(2) through ignoringEINTR,
/// while std's `read_dir` calls opendir(3) once.
pub(super) fn read_dir(name: &Path) -> std::io::Result<std::fs::ReadDir> {
    read_dir_with(name, &mut |p| {
        #[cfg(all(test, unix))]
        tests::injected_interruption()?;
        std::fs::read_dir(p)
    })
}
fn read_dir_with(
    name: &Path,
    open: &mut dyn FnMut(&Path) -> std::io::Result<std::fs::ReadDir>,
) -> std::io::Result<std::fs::ReadDir> {
    ignoring_eintr(|| open(name))
}

#[cfg(all(test, unix))]
mod tests {
    use super::{bytes, ignoring_eintr, mkdir_all, mkdir_all_with, read_dir_with};
    use crate::iofs::IoError;
    use std::path::{Path, PathBuf};

    fn interrupted() -> std::io::Error {
        std::io::Error::from_raw_os_error(rustix::io::Errno::INTR.raw_os_error())
    }

    thread_local! {
        /// How many of this thread's next raw mkdir(2) calls and directory
        /// opens in `mkdir_all` and `read_dir` fail with EINTR before reaching
        /// the system, so a test through `OsFs` interrupts the production calls.
        static PENDING_INTERRUPTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }
    pub(super) fn injected_interruption() -> std::io::Result<()> {
        PENDING_INTERRUPTIONS.with(|pending| match pending.get() {
            0 => Ok(()),
            n => {
                pending.set(n - 1);
                Err(interrupted())
            }
        })
    }
    fn interrupt_next(count: usize) {
        PENDING_INTERRUPTIONS.with(|pending| pending.set(count));
    }
    fn pending_interruptions() -> usize {
        PENDING_INTERRUPTIONS.with(std::cell::Cell::get)
    }

    /// A fresh directory under the system temporary directory, removed on drop.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(label: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "tsr-vfs-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn join(&self, relative: &str) -> Vec<u8> {
            bytes(&self.0.join(relative))
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn mkdir_all_retries_an_interrupted_mkdir_at_every_level() {
        let scratch = Scratch::new("mkdir-eintr");
        let target = scratch.join("x/y/z");
        let mut calls: Vec<PathBuf> = Vec::new();
        mkdir_all_with(&target, &mut |level| {
            let first = !calls.iter().any(|seen| seen == level);
            calls.push(level.to_path_buf());
            if first {
                Err(interrupted())
            } else {
                std::fs::create_dir(level)
            }
        })
        .unwrap();
        assert!(std::fs::metadata(scratch.0.join("x/y/z")).unwrap().is_dir());
        let expected: Vec<PathBuf> = ["x", "x", "x/y", "x/y", "x/y/z", "x/y/z"]
            .iter()
            .map(|level| scratch.0.join(level))
            .collect();
        assert_eq!(calls, expected);
        // An existing directory takes the stat fast path: no mkdir at all.
        mkdir_all_with(&target, &mut |_| panic!("mkdir on an existing directory")).unwrap();
    }

    #[test]
    fn mkdir_all_returns_other_errors_once_for_the_level_that_failed() {
        let scratch = Scratch::new("mkdir-error");
        let denied = rustix::io::Errno::ACCESS.raw_os_error();
        let mut calls = 0;
        let error = mkdir_all_with(&scratch.join("p/q"), &mut |_| {
            calls += 1;
            Err(std::io::Error::from_raw_os_error(denied))
        })
        .unwrap_err();
        assert_eq!(calls, 1, "the parent level fails first and is not retried");
        let IoError::Path { op, path, .. } = error else {
            panic!("expected a PathError, got {error:?}");
        };
        assert_eq!(
            (op, String::from_utf8_lossy(&path)),
            ("mkdir", String::from_utf8_lossy(&scratch.join("p")))
        );
    }

    #[test]
    fn mkdir_all_reports_the_level_that_is_not_a_directory() {
        // Pinned Go 1.27.1 os.MkdirAll reports `mkdir <root>/afile: not a
        // directory` for all three requests (native probe), not the requested path.
        let scratch = Scratch::new("mkdir-file");
        std::fs::write(scratch.0.join("afile"), b"q").unwrap();
        for request in ["afile", "afile/sub", "afile/sub/deeper"] {
            let error = mkdir_all(&scratch.join(request)).unwrap_err();
            let IoError::Path { op, path, .. } = error else {
                panic!("expected a PathError, got {error:?}");
            };
            assert_eq!(
                (op, String::from_utf8_lossy(&path)),
                ("mkdir", String::from_utf8_lossy(&scratch.join("afile"))),
                "{request}"
            );
        }
        for request in ["made/deep", "made/deep/.", "made//x/"] {
            mkdir_all(&scratch.join(request)).unwrap();
        }
        assert!(std::fs::metadata(scratch.0.join("made/x"))
            .unwrap()
            .is_dir());
    }

    #[test]
    fn os_fs_directory_creation_reports_the_level_that_is_not_a_directory() {
        // Pinned tsgo (native probe of osvfs.FS()): WriteFile on afile/sub/x.txt,
        // afile/sub/deeper/x.txt and afile/x.txt all fail with
        // `mkdir <root>/afile: not a directory` from ensureDirectoryExists.
        use crate::{os::fs, Error, FileSystem};
        let scratch = Scratch::new("ensure-file");
        std::fs::write(scratch.0.join("afile"), b"q").unwrap();
        let expected = format!(
            "mkdir {}: not a directory",
            String::from_utf8_lossy(&scratch.join("afile"))
        );
        let results = [
            ("ensure_directory afile/sub", {
                fs().ensure_directory(&scratch.join("afile/sub"))
            }),
            ("ensure_directory afile/sub/deeper", {
                fs().ensure_directory(&scratch.join("afile/sub/deeper"))
            }),
            ("write_file afile/sub/x.txt", {
                fs().write_file(&scratch.join("afile/sub/x.txt"), b"q")
            }),
            ("write_file afile/x.txt", {
                fs().write_file(&scratch.join("afile/x.txt"), b"q")
            }),
        ];
        for (label, result) in results {
            let Err(Error::Detailed(error)) = result else {
                panic!("{label}: expected a PathError, got {result:?}");
            };
            let IoError::Path { op, path, .. } = &*error else {
                panic!("{label}: expected a PathError, got {error:?}");
            };
            assert_eq!(
                (*op, String::from_utf8_lossy(path)),
                ("mkdir", String::from_utf8_lossy(&scratch.join("afile"))),
                "{label}"
            );
            assert_eq!(error.to_string(), expected, "{label}");
        }
        fs().ensure_directory(&scratch.join("made/deep")).unwrap();
        fs().write_file(&scratch.join("written/deep/x.txt"), b"q")
            .unwrap();
        assert_eq!(
            std::fs::read(scratch.0.join("written/deep/x.txt")).unwrap(),
            b"q"
        );
        assert!(std::fs::metadata(scratch.0.join("made/deep"))
            .unwrap()
            .is_dir());
    }

    #[test]
    fn os_fs_retries_interrupted_mkdir_and_directory_opens() {
        use crate::{os::fs, FileSystem};
        let scratch = Scratch::new("entry-eintr");
        // ensureDirectoryExists: the first level's mkdir is interrupted three
        // times, then both levels are made.
        interrupt_next(3);
        fs().ensure_directory(&scratch.join("a/b")).unwrap();
        assert_eq!(pending_interruptions(), 0);
        assert!(std::fs::metadata(scratch.0.join("a/b")).unwrap().is_dir());
        // getAccessibleEntries swallows a failed ReadDir as an empty listing.
        std::fs::write(scratch.0.join("a/entry.ts"), b"").unwrap();
        interrupt_next(2);
        let entries = fs().entries(&scratch.join("a")).unwrap();
        assert_eq!(pending_interruptions(), 0);
        let names = |list: Option<Vec<tsr_jsstring::JsString>>| {
            list.unwrap_or_default()
                .iter()
                .map(|name| String::from_utf8_lossy(name.as_bytes()).into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(entries.files), ["entry.ts"]);
        assert_eq!(names(entries.directories), ["b"]);
    }

    #[test]
    fn read_dir_retries_an_interrupted_open() {
        let scratch = Scratch::new("readdir-eintr");
        std::fs::write(scratch.0.join("entry.ts"), b"").unwrap();
        let mut attempts = 0;
        let entries = read_dir_with(&scratch.0, &mut |p: &Path| {
            attempts += 1;
            if attempts == 1 {
                Err(interrupted())
            } else {
                std::fs::read_dir(p)
            }
        })
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
        assert_eq!(attempts, 2);
        assert_eq!(entries, ["entry.ts"]);
    }

    #[test]
    fn interrupted_syscalls_retry_but_other_and_wrapped_errors_return_once() {
        let mut attempts = 0;
        let result = ignoring_eintr(|| {
            attempts += 1;
            if attempts < 3 {
                Err(interrupted())
            } else {
                Ok(19)
            }
        });
        assert_eq!(result.unwrap(), 19);
        assert_eq!(attempts, 3);

        let mut attempts = 0;
        let error = ignoring_eintr::<()>(|| {
            attempts += 1;
            Err(std::io::Error::from_raw_os_error(
                rustix::io::Errno::NOENT.raw_os_error(),
            ))
        })
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert_eq!(
            error.raw_os_error(),
            Some(rustix::io::Errno::NOENT.raw_os_error())
        );

        let mut attempts = 0;
        let error = ignoring_eintr::<()>(|| {
            attempts += 1;
            assert_eq!(attempts, 1, "wrapped EINTR must not be retried");
            Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                interrupted(),
            ))
        })
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(error.raw_os_error(), None);
        assert_eq!(attempts, 1);
    }
}
