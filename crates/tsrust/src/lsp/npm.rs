//! Native npm execution owns and reaps its process group. Nonblocking output
//! pipes keep cancellation responsive even when a descendant inherits stdout.
use std::ffi::OsStr;
use std::io::{self, Read};
use std::os::unix::{ffi::OsStrExt, process::CommandExt};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tsr_ipc::Context;
use tsr_jsstring::JsString;
use tsr_project::ata::{NpmError, NpmExecutor};

pub(super) struct NativeNpm;
impl NpmExecutor for NativeNpm {
    fn npm_install(
        &self,
        context: &Context,
        cwd: &[u8],
        args: &[JsString],
    ) -> Result<Vec<u8>, NpmError> {
        execute(context, b"npm", cwd, args)
    }
}

struct OwnedChild {
    child: Child,
    reaped: bool,
}
impl OwnedChild {
    fn terminate(&mut self) {
        if self.reaped {
            return;
        }
        if let Some(pid) = rustix::process::Pid::from_raw(self.child.id() as i32) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = self.child.kill();
        self.reaped = self.child.wait().is_ok();
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.terminate();
    }
}
fn execute(
    context: &Context,
    command: &[u8],
    cwd: &[u8],
    args: &[JsString],
) -> Result<Vec<u8>, NpmError> {
    let mut output = Vec::new();
    let mut stderr = Vec::new();
    let result = (|| -> io::Result<()> {
        if context.err().is_some() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let mut command = Command::new(OsStr::from_bytes(command));
        command.args(args.iter().map(|arg| OsStr::from_bytes(arg.as_bytes())));
        if !cwd.is_empty() {
            command.current_dir(OsStr::from_bytes(cwd));
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = OwnedChild {
            child: command.spawn()?,
            reaped: false,
        };
        let mut stdout = child.child.stdout.take().expect("piped npm stdout");
        let mut errors = child.child.stderr.take().expect("piped npm stderr");
        for fd in [
            &stdout as &dyn std::os::fd::AsFd,
            &errors as &dyn std::os::fd::AsFd,
        ] {
            rustix::fs::fcntl_setfl(
                fd,
                rustix::fs::fcntl_getfl(fd)? | rustix::fs::OFlags::NONBLOCK,
            )?;
        }
        loop {
            if context.err().is_some() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            drain(&mut stdout, &mut output)?;
            drain(&mut errors, &mut stderr)?;
            stderr.clear();
            if let Some(status) = child.child.try_wait()? {
                child.reaped = true;
                drain(&mut stdout, &mut output)?;
                drain(&mut errors, &mut stderr)?;
                return if status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other(format!("npm exited with {status}")))
                };
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    match result {
        Ok(()) => Ok(output),
        Err(error) => {
            let message = context
                .err()
                .map_or_else(|| error.to_string(), |error| error.to_string());
            Err(NpmError { message, output })
        }
    }
}
fn drain(pipe: &mut impl Read, output: &mut Vec<u8>) -> io::Result<()> {
    let mut bytes = [0; 8192];
    // Bound each sweep so a continuously writing child cannot starve cancellation.
    for _ in 0..32 {
        match pipe.read(&mut bytes) {
            Ok(0) => return Ok(()),
            Ok(count) => output.extend_from_slice(&bytes[..count]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(command: &str) -> Vec<JsString> {
        ["-c", command]
            .into_iter()
            .map(|s| JsString::from_bytes(s.as_bytes()))
            .collect()
    }
    #[test]
    fn local_executor_captures_success_and_failure_without_npm() {
        assert_eq!(
            execute(
                &Context::background(),
                b"/bin/sh",
                b"",
                &args("printf 'ok'")
            ),
            Ok(b"ok".to_vec())
        );
        let error = execute(
            &Context::background(),
            b"/bin/sh",
            b"",
            &args("printf out; printf err >&2; exit 7"),
        )
        .unwrap_err();
        assert_eq!(error.output, b"out");
        assert!(error.message.contains('7'));
    }
    #[test]
    fn local_executor_cancellation_reaps_child_and_descendants() {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("tsr-npm-cancel-{}-{id}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(directory.clone());
        let context = Context::background().with_timeout(Duration::from_secs(10));
        let worker_context = context.clone();
        let cwd = directory.as_os_str().as_bytes().to_vec();
        let worker = std::thread::spawn(move || {
            execute(
                &worker_context,
                b"/bin/sh",
                &cwd,
                // Publish readiness only after a descendant inheriting the
                // output pipes exists. The PID does not depend on output
                // draining before cancellation.
                &args("sleep 60 & printf '%s\\n' $$ > pending; mv pending ready; wait"),
            )
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let ready = loop {
            if let Ok(ready) = std::fs::read_to_string(directory.join("ready")) {
                break Some(ready);
            }
            if std::time::Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let start = std::time::Instant::now();
        context.cancel();
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.message, "context canceled");
        assert!(start.elapsed() < Duration::from_secs(2));
        let pid = ready
            .expect("shell did not publish process readiness")
            .trim()
            .parse()
            .unwrap();
        assert!(!crate::process::is_process_alive(pid));
    }

    #[test]
    #[ignore = "manual integration: executes npm and accesses the registry network"]
    fn manual_real_npm_registry_install() {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("tsr-ata-{}-{id}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(directory.clone());
        std::fs::write(directory.join("package.json"), b"{\"private\":true}").unwrap();
        let args: Vec<_> = ["install", "--ignore-scripts", "types-registry@latest"]
            .into_iter()
            .map(|arg| JsString::from_bytes(arg.as_bytes()))
            .collect();
        NativeNpm
            .npm_install(
                &Context::background().with_timeout(Duration::from_secs(120)),
                directory.as_os_str().as_bytes(),
                &args,
            )
            .unwrap();
        assert!(directory
            .join("node_modules/types-registry/index.json")
            .is_file());
    }
}
