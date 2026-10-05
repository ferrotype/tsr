use std::{
    os::unix::ffi::OsStrExt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tsr_ipc::Context;
use tsr_jsstring::JsString;
use tsr_lsp::{connection::Connection, dynamic_queue::DynamicQueue, runtime::Options};
mod flags;
mod npm;
use flags::parse as flags;

// port: tsc/cmd/tsc/lsp.go:runLSP
pub fn run(args: &[JsString]) -> i32 {
    let flags = match flags(args) {
        Ok(flags) => flags,
        Err(error) => {
            if !error.is_empty() {
                eprintln!("{error}");
            }
            eprint!("{}", flags::USAGE);
            return 2;
        }
    };
    if !flags.stdio {
        eprintln!("only stdio is supported");
        return 1;
    }
    if !flags.pprof.is_empty() {
        eprintln!("LSP pprof profiles are not implemented (Phase 7)");
        return tsr_tsc::ExitStatus::NotImplemented.0;
    }
    let parent = flags.parent;
    let context = Context::background().with_cancel();
    let _signals = match crate::signals::Scope::new(context.clone()) {
        Ok(scope) => scope,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let fs = Arc::new(tsr_bundled::BundledFs::new(tsr_vfs::os::shared_fs()));
    let mut options = Options::new(
        tsr_project::session::SessionOptions {
            current_directory: JsString::from_bytes(cwd.as_os_str().as_bytes()),
            default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
            ..Default::default()
        },
        fs,
    );
    let system = match crate::system::OsSystem::new() {
        Ok(system) => Arc::new(system),
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    options.project.mapper_spawner = Some(system);
    options.project.npm_executor = Some(Arc::new(npm::NativeNpm));
    options.project.typings_location =
        JsString::from_bytes(tsr_vfs::os::global_typings_cache_location());
    options.parent_process = parent_watchdog(&context, parent);
    let outgoing: DynamicQueue<tsr_json::RawValue> = DynamicQueue::new();
    let output_queue = outgoing.clone();
    let output_context = context.clone();
    let (connection, input) = Connection::new(
        options,
        &context,
        Arc::new(move |value| {
            output_queue
                .put(&output_context, value)
                .map_err(|e| tsr_lsproto::ResponseError {
                    code: -32603,
                    message: e.to_string(),
                    data: None,
                })
        }),
        Box::new(std::io::stderr()),
    );
    let failed = Arc::new(AtomicBool::new(false));
    let writer_context = context.clone();
    let writer_failed = failed.clone();
    std::thread::spawn(move || {
        let mut writer = tsr_lsproto::Writer::new(std::io::stdout());
        while let Ok(message) = outgoing.get(&writer_context) {
            if let Err(e) = writer.write(&message.0) {
                eprintln!("{e}");
                writer_failed.store(true, Ordering::Relaxed);
                writer_context.cancel();
                break;
            }
        }
    });
    let reader_failed = failed.clone();
    std::thread::spawn(move || {
        let mut reader = tsr_lsproto::Reader::new(std::io::stdin());
        loop {
            let data = match reader.read() {
                Ok(data) => data,
                Err(tsr_lsproto::FramingError::Eof) => break,
                Err(e) => {
                    eprintln!("{e}");
                    reader_failed.store(true, Ordering::Relaxed);
                    break;
                }
            };
            if let Err(e) = input.receive_bytes(&data) {
                eprintln!("{}", e.message);
                reader_failed.store(true, Ordering::Relaxed);
                break;
            }
        }
        input.end();
    });
    // Like the pin, neither an orphaned stdin read nor a blocked stdout write
    // prevents process exit after cancellation. The process owns these threads.
    connection.run();
    context.cancel();
    i32::from(failed.load(Ordering::Relaxed))
}

// port: tsc/cmd/tsc/lsp.go:newParentProcessWatchdog
fn parent_watchdog(
    context: &Context,
    override_pid: isize,
) -> Option<Arc<dyn Fn(i32) + Send + Sync>> {
    let callback_context = context.clone();
    let callback = Arc::new(move |pid: i32| {
        start_parent_watchdog(&callback_context, pid as isize, Duration::from_secs(5))
    });
    if override_pid > 0 {
        start_parent_watchdog(context, override_pid, Duration::from_secs(5));
        None
    } else {
        Some(callback)
    }
}
// port: tsc/cmd/tsc/lsp.go:startParentProcessWatchdog
fn start_parent_watchdog(context: &Context, parent: isize, interval: Duration) {
    if parent <= 0 {
        return;
    }
    let context = context.clone();
    std::thread::spawn(move || {
        let queue: DynamicQueue<()> = DynamicQueue::new();
        loop {
            let tick = context.with_timeout(interval);
            let _ = queue.get(&tick);
            if context.err().is_some() {
                return;
            }
            if !crate::process::is_process_alive(parent) {
                eprintln!("Parent process {parent} has exited, shutting down.");
                context.cancel();
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn go_flag_spelling_and_boolean_values() {
        let parse = |args: &[&[u8]]| {
            flags(
                &args
                    .iter()
                    .map(|arg| JsString::from_bytes(*arg))
                    .collect::<Vec<_>>(),
            )
        };
        let value = parse(&[b"--stdio", b"--clientProcessId=12"]).unwrap();
        assert!(value.stdio);
        assert_eq!(value.parent, 12);
        assert!(!parse(&[b"-stdio=false"]).unwrap().stdio);
        assert!(
            parse(&[b"--stdio=1", b"--", b"--not-a-flag"])
                .unwrap()
                .stdio
        );
        assert!(parse(&[b"-clientProcessId"]).is_err());
        assert!(parse(&[b"-stdio=maybe"]).is_err());
    }
    #[test]
    fn watchdog_observes_disappearance_and_ignores_invalid_ids() {
        let ctx = Context::background().with_cancel();
        for parent in [0, -1] {
            start_parent_watchdog(&ctx, parent, Duration::from_millis(2));
        }
        assert!(ctx.err().is_none());
        let mut child = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id() as isize;
        child.wait().unwrap();
        start_parent_watchdog(&ctx, pid, Duration::from_millis(2));
        let queue: DynamicQueue<()> = DynamicQueue::new();
        assert_eq!(
            queue.get(&ctx.with_timeout(Duration::from_secs(2))),
            Err(tsr_ipc::ContextError::Canceled)
        );
    }
}
