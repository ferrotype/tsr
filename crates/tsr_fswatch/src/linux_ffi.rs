//! Audited Linux-only boundary for fanotify and FID operations rustix does not expose.
use crate::{fanotify::HandleKey, Error};
use rustix::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::ffi::CString;
fn errno() -> rustix::io::Errno {
    rustix::io::Errno::from_raw_os_error(
        std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO),
    )
}
pub(crate) fn init() -> Result<OwnedFd, rustix::io::Errno> {
    let flags = libc::FAN_CLASS_NOTIF
        | libc::FAN_CLOEXEC
        | libc::FAN_NONBLOCK
        | libc::FAN_REPORT_FID
        | libc::FAN_REPORT_DFID_NAME;
    // SAFETY: fanotify_init takes scalar flags and returns a new owned descriptor.
    let fd = unsafe { libc::fanotify_init(flags, (libc::O_RDONLY | libc::O_CLOEXEC) as u32) };
    if fd < 0 {
        return Err(errno());
    }
    // SAFETY: successful fanotify_init transfers this unique live descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
pub(crate) fn mark(
    fd: &OwnedFd,
    path: &[u8],
    flags: u32,
    mask: u64,
) -> Result<(), rustix::io::Errno> {
    let path = CString::new(path).map_err(|_| rustix::io::Errno::INVAL)?;
    // SAFETY: fd is borrowed live; path is NUL-terminated and stays alive for the call.
    let result =
        unsafe { libc::fanotify_mark(fd.as_raw_fd(), flags, mask, libc::AT_FDCWD, path.as_ptr()) };
    if result < 0 {
        Err(errno())
    } else {
        Ok(())
    }
}
pub(crate) fn handle_key(path: &[u8]) -> Result<HandleKey, Error> {
    let name = CString::new(path).map_err(|_| Error::Message("path contains NUL".into()))?;
    let mut capacity = 128usize;
    loop {
        let mut data = vec![0u64; (8 + capacity).div_ceil(8)];
        let handle = data.as_mut_ptr().cast::<libc::file_handle>();
        let mut mount = 0;
        // SAFETY: u64 storage is aligned for file_handle and has its header plus
        // capacity writable bytes. The kernel writes no more than handle_bytes.
        unsafe {
            (*handle).handle_bytes = capacity as u32;
        }
        // SAFETY: both out pointers designate writable live allocations and name
        // is terminated. The resulting handle bytes are opaque, never pointers.
        let result = unsafe {
            libc::name_to_handle_at(libc::AT_FDCWD, name.as_ptr(), handle, &mut mount, 0)
        };
        if result < 0 {
            let error = errno();
            // SAFETY: the initialized header remains in bounds after the syscall.
            let needed = unsafe { (*handle).handle_bytes as usize };
            if error == rustix::io::Errno::OVERFLOW && needed > capacity {
                capacity = needed;
                continue;
            }
            return Err(unsupported(error));
        }
        // SAFETY: success initializes the header and at most capacity bytes.
        let (length, handle_type) =
            unsafe { ((*handle).handle_bytes as usize, (*handle).handle_type) };
        if length > capacity {
            return Err(Error::Message("invalid kernel file handle length".into()));
        }
        // SAFETY: the bytes begin immediately after the 8-byte C header and fit
        // the initialized buffer returned successfully by name_to_handle_at.
        let bytes =
            unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>().add(8), length) }
                .to_vec();
        let stat = rustix::fs::statfs(path)?;
        // SAFETY: Linux fsid_t is repr(C) with exactly two 32-bit c_int values.
        // rustix preserves this ABI but keeps its fields private. This conversion
        // bridges that FID representation; no pointers or uninitialized padding.
        let fsid: [i32; 2] = unsafe { std::mem::transmute(stat.f_fsid) };
        return Ok(HandleKey {
            fsid,
            handle_type,
            handle: bytes,
        });
    }
}
pub(crate) fn close_event_fd(fd: i32) {
    if fd >= 0 {
        // SAFETY: a nonnegative fanotify event descriptor is transferred by the
        // kernel to the event reader; it is distinct from the watcher descriptor.
        drop(unsafe { OwnedFd::from_raw_fd(fd) });
    }
}
pub(crate) fn unsupported(error: rustix::io::Errno) -> Error {
    if error == rustix::io::Errno::OPNOTSUPP || error == rustix::io::Errno::NODEV {
        Error::TaggedFilesystemUnsupported {
            source: Box::new(error.into()),
        }
    } else {
        error.into()
    }
}
