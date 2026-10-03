//! Unix process-boundary FFI. The handler performs one async-signal-safe write;
//! normal Rust execution cancels the context. Previous handlers are restored at
//! scope exit, not after the first interrupt (the pinned NotifyContext behavior).
#![allow(unsafe_code)]
use rustix::event::{poll, PollFd, PollFlags};
use rustix::fd::{AsRawFd, OwnedFd};
use std::io;
use std::sync::{
    atomic::{AtomicBool, AtomicI32, Ordering},
    Arc, Mutex, MutexGuard, OnceLock,
};
use tsr_ipc::Context;

static REGISTRATION: Mutex<()> = Mutex::new(());
static WRITE_FD: AtomicI32 = AtomicI32::new(-1);
struct Wake {
    read: OwnedFd,
    write: OwnedFd,
}
// Process-lifetime descriptors avoid the close/reuse race with a handler that
// had already begun before sigaction restored the prior disposition.
static WAKE: OnceLock<Wake> = OnceLock::new();
extern "C" fn handler(_: libc::c_int) {
    // errno is thread-local and the interrupted syscall's value must survive.
    // SAFETY: each target exposes the current thread's errno slot.
    let errno = unsafe {
        #[cfg(target_os = "linux")]
        {
            libc::__errno_location()
        }
        #[cfg(target_os = "macos")]
        {
            libc::__error()
        }
    };
    // SAFETY: errno is a valid thread-local slot; the static fd stays live.
    unsafe {
        let saved = *errno;
        let byte = 1u8;
        let fd = WRITE_FD.load(Ordering::Relaxed);
        if fd >= 0 {
            libc::write(fd, (&raw const byte).cast(), 1);
        }
        *errno = saved;
    }
}

pub struct Scope {
    _registration: MutexGuard<'static, ()>,
    previous: [(libc::c_int, libc::sigaction); 2],
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Scope {
    pub fn new(context: Context) -> io::Result<Self> {
        let registration = REGISTRATION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let wake = if let Some(wake) = WAKE.get() {
            wake
        } else {
            let (read, write) = crate::process::cancellation_pipe()?;
            let _ = WAKE.set(Wake { read, write });
            WAKE.get()
                .expect("signal pipe installed under registration lock")
        };
        let mut bytes = [0; 128];
        while rustix::io::read(&wake.read, &mut bytes).is_ok_and(|count| count > 0) {}
        WRITE_FD.store(wake.write.as_raw_fd(), Ordering::Relaxed);
        // SAFETY: zero initializes this target's sigaction; sigemptyset creates
        // a valid empty mask and both previous actions are written before use.
        let (action, mut previous) = unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as *const () as usize;
            libc::sigemptyset(&raw mut action.sa_mask);
            action.sa_flags = libc::SA_RESTART;
            (
                action,
                [
                    (libc::SIGINT, std::mem::zeroed()),
                    (libc::SIGTERM, std::mem::zeroed()),
                ],
            )
        };
        for index in 0..previous.len() {
            // SAFETY: action and old-action pointers refer to initialized,
            // target-layout structures; signal numbers are fixed supported ones.
            if unsafe {
                libc::sigaction(
                    previous[index].0,
                    &raw const action,
                    &raw mut previous[index].1,
                )
            } != 0
            {
                let error = io::Error::last_os_error();
                for (signal, prior) in &previous[..index] {
                    // SAFETY: only successfully replaced dispositions are restored.
                    unsafe {
                        libc::sigaction(*signal, prior, std::ptr::null_mut());
                    }
                }
                return Err(error);
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = std::thread::Builder::new()
            .name("compiler signals".into())
            .spawn(move || {
                let mut bytes = [0; 128];
                loop {
                    if stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let mut fds = [PollFd::new(&wake.read, PollFlags::IN)];
                    match poll(&mut fds, None) {
                        Err(rustix::io::Errno::INTR) => continue,
                        Err(_) => break,
                        Ok(_) => {}
                    }
                    if stopped.load(Ordering::Acquire) {
                        break;
                    }
                    if rustix::io::read(&wake.read, &mut bytes).is_ok_and(|count| count > 0) {
                        context.cancel();
                    }
                }
            });
        match worker {
            Ok(thread) => Ok(Self {
                _registration: registration,
                previous,
                stop,
                thread: Some(thread),
            }),
            Err(error) => {
                for (signal, prior) in &previous {
                    // SAFETY: these are the dispositions saved above.
                    unsafe {
                        libc::sigaction(*signal, prior, std::ptr::null_mut());
                    }
                }
                Err(error)
            }
        }
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        for (signal, prior) in &self.previous {
            // SAFETY: Scope owns registration exclusively until after restoration.
            unsafe {
                libc::sigaction(*signal, prior, std::ptr::null_mut());
            }
        }
        self.stop.store(true, Ordering::Release);
        if let Some(wake) = WAKE.get() {
            let _ = rustix::io::write(&wake.write, &[0]);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The same local-time fields Go formats as `03:04:05 PM`, without reading
/// locale or timezone from inside compiler/reporting code.
pub fn local_time(time: tsr_vfs::iofs::Time) -> Vec<u8> {
    let seconds = time.unix().0 as libc::time_t;
    let mut value = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: input/output pointers have their exact C layouts; localtime_r
    // initializes output on success and has no shared result buffer.
    let result = unsafe { libc::localtime_r(&raw const seconds, value.as_mut_ptr()) };
    if result.is_null() {
        return tsr_tsc::diagnostics::format_watch_time(time);
    }
    // SAFETY: non-null return guarantees the output was initialized.
    let value = unsafe { value.assume_init() };
    let hour = match value.tm_hour % 12 {
        0 => 12,
        hour => hour,
    };
    format!(
        "{hour:02}:{:02}:{:02} {}",
        value.tm_min,
        value.tm_sec,
        if value.tm_hour < 12 { "AM" } else { "PM" }
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notification_cancels_without_retiring_the_signal_scope() {
        let context = Context::background().with_cancel();
        let scope = Scope::new(context.clone()).unwrap();
        // Exercise the real async handler and pipe, without sending a signal
        // to the test harness's other threads.
        handler(libc::SIGINT);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while context.err().is_none() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(context.err().is_some());
        // Both dispositions must remain ours until scope exit.
        for signal in [libc::SIGINT, libc::SIGTERM] {
            let mut action = std::mem::MaybeUninit::<libc::sigaction>::uninit();
            // SAFETY: null new action only queries a valid initialized output.
            assert_eq!(
                unsafe { libc::sigaction(signal, std::ptr::null(), action.as_mut_ptr()) },
                0
            );
            assert_eq!(
                unsafe { action.assume_init() }.sa_sigaction,
                handler as *const () as usize
            );
        }
        handler(libc::SIGTERM);
        drop(scope);
    }
}
