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

#[cfg(all(test, unix))]
mod tests {
    use super::ignoring_eintr;

    #[test]
    fn interrupted_syscalls_retry_but_other_and_wrapped_errors_return_once() {
        let interrupted =
            || std::io::Error::from_raw_os_error(rustix::io::Errno::INTR.raw_os_error());
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
