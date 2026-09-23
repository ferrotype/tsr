//! Descriptor-relative deletion never follows a swapped directory symlink.
//! Like os.RemoveAll, visit siblings after an error and return the first one.
use super::native;
use crate::iofs::IoError;
use rustix::{
    fd::AsFd,
    fs::{openat, unlinkat, AtFlags, Dir, Mode, OFlags, CWD},
    io::{retry_on_intr, Errno},
};
pub(super) fn remove_all(path: &[u8]) -> Result<(), IoError> {
    if path.is_empty() {
        return Ok(());
    }
    if path == b"." || path.ends_with(b"/.") {
        return Err(IoError::path("RemoveAll", path, IoError::Invalid));
    }
    let p = native::path(path);
    // The fast path also removes empty, unreadable directories.
    let first = native::ignoring_eintr(|| std::fs::remove_file(&p))
        .or_else(|_| native::ignoring_eintr(|| std::fs::remove_dir(&p)));
    if first.is_ok()
        || first
            .as_ref()
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(());
    }
    let parent = p
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let base = p
        .file_name()
        .ok_or_else(|| IoError::path("remove", path, IoError::Invalid))?;
    let fd = match retry_on_intr(|| {
        openat(
            CWD,
            parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
    }) {
        Ok(fd) => fd,
        Err(Errno::NOENT) => return Ok(()),
        Err(e) => return Err(native::failure("open", &native::bytes(parent), e.into())),
    };
    remove_from(&fd, std::path::Path::new(base), path)
}
fn remove_from(parent: impl AsFd, base: &std::path::Path, path: &[u8]) -> Result<(), IoError> {
    let unlink = match retry_on_intr(|| unlinkat(&parent, base, AtFlags::empty())) {
        Ok(()) | Err(Errno::NOENT) => return Ok(()),
        Err(e) => e,
    };
    if !matches!(unlink, Errno::ISDIR | Errno::PERM | Errno::ACCESS) {
        return Err(native::failure("unlinkat", path, unlink.into()));
    }
    let fd = match retry_on_intr(|| {
        openat(
            &parent,
            base,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
    }) {
        Ok(fd) => fd,
        Err(Errno::NOENT) => return Ok(()),
        Err(Errno::NOTDIR | Errno::LOOP) => {
            return Err(native::failure("unlinkat", path, unlink.into()))
        }
        Err(e) => return Err(native::failure("openfdat", path, e.into())),
    };
    let mut directory =
        Dir::read_from(&fd).map_err(|e| native::failure("readdirnames", path, e.into()))?;
    let mut first = None;
    for item in &mut directory {
        let entry = match item {
            Ok(entry) => entry,
            Err(e) => {
                first.get_or_insert_with(|| native::failure("readdirnames", path, e.into()));
                break;
            }
        };
        let name = entry.file_name().to_bytes();
        if matches!(name, b"." | b"..") {
            continue;
        }
        let child = [path, b"/", name].concat();
        if let Err(e) = remove_from(&fd, &native::path(name), &child) {
            first.get_or_insert(e);
        }
    }
    if let Err(e) = retry_on_intr(|| unlinkat(&parent, base, AtFlags::REMOVEDIR)) {
        if e != Errno::NOENT {
            first.get_or_insert_with(|| native::failure("unlinkat", path, e.into()));
        }
    }
    first.map_or(Ok(()), Err)
}
