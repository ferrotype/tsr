//! Mapper processes have independently cancellable pipes. A descendant keeping
//! stdout/stderr open cannot keep Close waiting after the launcher was reaped.
use rustix::event::{poll, PollFd, PollFlags};
use rustix::fd::OwnedFd;
use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
use rustix::io::{read, write, Errno};
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tsr_ipc::{Closer, Stream};
use tsr_jsstring::JsString;

// port: tsc/cmd/tsc/isprocessalive_unix.go:isProcessAlive
#[allow(
    unsafe_code,
    reason = "kill(pid, 0) is a process-existence probe; no signal is sent"
)]
pub(crate) fn is_process_alive(pid: isize) -> bool {
    // Go's syscall boundary narrows its native-width int to pid_t. Preserve
    // that conversion for flag.Int values, including values outside int32.
    // SAFETY: kill with signal zero takes no pointers and cannot deliver a signal.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub(crate) fn cancellation_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    #[cfg(target_os = "linux")]
    {
        Ok(rustix::pipe::pipe_with(
            rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
        )?)
    }
    #[cfg(target_os = "macos")]
    {
        let (read, write) = rustix::pipe::pipe()?;
        for fd in [&read, &write] {
            rustix::io::fcntl_setfd(fd, rustix::io::FdFlags::CLOEXEC)?;
            fcntl_setfl(fd, fcntl_getfl(fd)? | OFlags::NONBLOCK)?;
        }
        Ok((read, write))
    }
}

struct Cancel {
    read: OwnedFd,
    write: OwnedFd,
}
impl Cancel {
    fn new() -> io::Result<Self> {
        let (read, write) = cancellation_pipe()?;
        Ok(Self { read, write })
    }
    fn cancel(&self) {
        let _ = write(&self.write, &[1]);
    }
}

struct Pipe {
    fd: Mutex<Option<OwnedFd>>,
    cancel: Arc<Cancel>,
}
impl Pipe {
    fn new(fd: OwnedFd, cancel: Arc<Cancel>) -> io::Result<Self> {
        fcntl_setfl(&fd, fcntl_getfl(&fd)? | OFlags::NONBLOCK)?;
        Ok(Self {
            fd: Mutex::new(Some(fd)),
            cancel,
        })
    }
    fn close(&self) {
        self.cancel.cancel();
        self.fd.lock().expect("process pipe lock").take();
    }
    fn io(&self, bytes: &mut [u8], output: Option<&[u8]>) -> io::Result<usize> {
        if output.map_or(bytes.is_empty(), <[u8]>::is_empty) {
            return Ok(0);
        }
        let lock = self.fd.lock().expect("process pipe lock");
        let Some(fd) = lock.as_ref() else {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        };
        loop {
            let mut fds = [
                PollFd::new(
                    fd,
                    if output.is_some() {
                        PollFlags::OUT
                    } else {
                        PollFlags::IN
                    },
                ),
                PollFd::new(&self.cancel.read, PollFlags::IN),
            ];
            match poll(&mut fds, None) {
                Err(Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
                Ok(_) => {}
            }
            if !fds[1].revents().is_empty() {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            let result = match output {
                Some(bytes) => write(fd, bytes),
                None => read(fd, &mut *bytes),
            };
            match result {
                Err(Errno::INTR | Errno::AGAIN) => continue,
                other => return other.map_err(Into::into),
            }
        }
    }
}

struct Process {
    child: Mutex<Option<Child>>,
    exit: Mutex<Option<i32>>,
    input: Pipe,
    output: Pipe,
    stderr: Arc<Pipe>,
    stderr_done: Arc<(Mutex<Option<io::Result<()>>>, Condvar)>,
    stderr_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    close_lock: Mutex<()>,
}
impl Closer for Process {
    /// port: tsc/cmd/tsc/sys.go:childProcess.Close
    fn close(&self) -> io::Result<()> {
        let _close = self
            .close_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(mut child) = self.child.lock().expect("process lock").take() else {
            return Ok(());
        };
        self.input.close();
        let _ = child.kill();
        let waited = child.wait();
        if let Ok(status) = &waited {
            *self.exit.lock().expect("exit lock") = Some(status.code().unwrap_or(-1));
        }
        self.output.close();
        // Go Cmd.WaitDelay bounds inherited stderr pipes after the child exits.
        // Successful stderr EOF finishes immediately; a descendant gets one second.
        let deadline = Instant::now() + Duration::from_secs(1);
        let (lock, wake) = self.stderr_done.as_ref();
        let mut done = lock.lock().expect("stderr completion lock");
        while done.is_none() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            done = wake
                .wait_timeout(done, left)
                .expect("stderr completion wait")
                .0;
        }
        let timed_out = done.is_none();
        drop(done);
        self.stderr.close();
        if let Some(thread) = self
            .stderr_thread
            .lock()
            .expect("stderr thread lock")
            .take()
        {
            let _ = thread.join();
        }
        let status = waited?;
        // Cmd.Wait gives process failures precedence over copy errors. Close
        // suppresses ExitError and WaitDelay, but successful children must
        // propagate errors from the caller's stderr writer.
        if status.success() && !timed_out {
            return lock
                .lock()
                .expect("stderr completion lock")
                .take()
                .unwrap_or(Ok(()));
        }
        Ok(())
    }
    /// port: tsc/cmd/tsc/sys.go:childProcess.ExitCode
    fn exit_code(&self) -> Option<i32> {
        *self.exit.lock().expect("exit lock")
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
struct ProcessReader(Arc<Process>);
impl Read for ProcessReader {
    /// port: tsc/cmd/tsc/sys.go:childProcess.Read
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.0.output.io(bytes, None)
    }
}
struct ProcessWriter(Arc<Process>);
impl Write for ProcessWriter {
    /// port: tsc/cmd/tsc/sys.go:childProcess.Write
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.input.io(&mut [], Some(bytes))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// port: tsc/cmd/tsc/sys.go:spawnProcess
pub fn spawn(
    command: &[JsString],
    dir: &[u8],
    mut stderr: Box<dyn Write + Send>,
) -> io::Result<Stream> {
    let first = command
        .first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty mapper command"))?;
    let mut cmd = Command::new(OsStr::from_bytes(first.as_bytes()));
    cmd.args(
        command[1..]
            .iter()
            .map(|arg| OsStr::from_bytes(arg.as_bytes())),
    );
    if !dir.is_empty() {
        cmd.current_dir(OsStr::from_bytes(dir));
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Allocate cancellation channels before spawning so failures cannot orphan a child.
    let input_cancel = Arc::new(Cancel::new()?);
    let output_cancel = Arc::new(Cancel::new()?);
    let error_cancel = Arc::new(Cancel::new()?);
    let mut child = cmd.spawn()?;
    let pipes = (|| {
        Ok::<_, io::Error>((
            Pipe::new(
                child.stdin.take().expect("piped stdin").into(),
                input_cancel,
            )?,
            Pipe::new(
                child.stdout.take().expect("piped stdout").into(),
                output_cancel,
            )?,
            Arc::new(Pipe::new(
                child.stderr.take().expect("piped stderr").into(),
                error_cancel,
            )?),
        ))
    })();
    let (input, output, errors) = match pipes {
        Ok(pipes) => pipes,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let done = Arc::new((Mutex::new(None), Condvar::new()));
    let thread_errors = errors.clone();
    let thread_done = done.clone();
    let thread = std::thread::Builder::new()
        .name("mapper stderr".into())
        .spawn(move || {
            let mut buffer = [0; 8192];
            let result = (|| loop {
                let count = thread_errors.io(&mut buffer, None)?;
                if count == 0 {
                    return Ok(());
                }
                stderr.write_all(&buffer[..count])?;
            })();
            let (lock, wake) = thread_done.as_ref();
            *lock.lock().expect("stderr completion lock") = Some(result);
            wake.notify_all();
        });
    let thread = match thread {
        Ok(thread) => thread,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let process = Arc::new(Process {
        child: Mutex::new(Some(child)),
        exit: Mutex::new(None),
        input,
        output,
        stderr: errors,
        stderr_done: done,
        stderr_thread: Mutex::new(Some(thread)),
        close_lock: Mutex::new(()),
    });
    Ok(Stream {
        reader: Box::new(ProcessReader(process.clone())),
        writer: Box::new(ProcessWriter(process.clone())),
        closer: process,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mapper_stdio_and_close_reap_the_child() {
        let mut stream = spawn(
            &[JsString::from_bytes(b"/bin/cat".as_slice())],
            b"",
            Box::new(io::sink()),
        )
        .unwrap();
        stream.writer.write_all(b"hello\n").unwrap();
        let mut bytes = [0; 6];
        stream.reader.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"hello\n");
        assert_eq!(stream.closer.exit_code(), None);
        stream.closer.close().unwrap();
        // close shuts stdin before it kills: cat is reaped either by the
        // kill (-1) or, when it reads the EOF first, by its own exit (0).
        assert!(matches!(stream.closer.exit_code(), Some(0 | -1)));
        stream.closer.close().unwrap();
        assert!(stream.writer.write_all(b"closed").is_err());
    }
    /// source: tsc/cmd/tsc/sys_unix_test.go:TestChildProcessCloseDoesNotWaitForLauncherDescendants
    #[test]
    fn close_does_not_wait_for_launcher_descendants() {
        let mut stream = spawn(
            &["/bin/sh", "-c", "sleep 60 & echo $!; wait"]
                .map(|s| JsString::from_bytes(s.as_bytes())),
            b"",
            Box::new(io::sink()),
        )
        .unwrap();
        let mut pid = String::new();
        io::BufReader::new(&mut stream.reader)
            .read_line(&mut pid)
            .unwrap();
        let pid = rustix::process::Pid::from_raw(pid.trim().parse().unwrap()).unwrap();
        struct Reap(rustix::process::Pid);
        impl Drop for Reap {
            fn drop(&mut self) {
                let _ = rustix::process::kill_process(self.0, rustix::process::Signal::KILL);
            }
        }
        let _reap = Reap(pid);
        let start = Instant::now();
        stream.closer.close().unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn successful_child_returns_stderr_writer_error_but_failed_child_suppresses_it() {
        struct Fails;
        impl Write for Fails {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("stderr sink failed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        for code in [0, 7] {
            let script = format!("printf diagnostic >&2; exit {code}");
            let mut stream = spawn(
                &[
                    JsString::from_bytes(b"/bin/sh".as_slice()),
                    JsString::from_bytes(b"-c".as_slice()),
                    JsString::from_bytes(script.as_bytes()),
                ],
                b"",
                Box::new(Fails),
            )
            .unwrap();
            // EOF establishes that the child has closed its output before
            // Close attempts Kill, preserving its successful/nonzero status.
            stream.reader.read_to_end(&mut Vec::new()).unwrap();
            let result = stream.closer.close();
            assert_eq!(stream.closer.exit_code(), Some(code));
            if code == 0 {
                assert_eq!(result.unwrap_err().to_string(), "stderr sink failed");
            } else {
                result.unwrap();
            }
        }
    }
    use std::io::BufRead;
}
