//! `tsrust --api`: the pin's API server entry with its six flags.
use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;
use tsr_api::server::{StdioServer, StdioServerOptions};
use tsr_ipc::Context;
use tsr_jsstring::JsString;

mod flags;

// port: tsc/cmd/tsc/api.go:runAPI
pub fn run(args: &[JsString]) -> i32 {
    let flags = match flags::parse(args) {
        Ok(flags) => flags,
        Err(error) => {
            if !error.is_empty() {
                eprintln!("{error}");
            }
            eprint!("{}", flags::USAGE);
            return 2;
        }
    };
    let cwd = match flags.cwd {
        Some(cwd) => cwd,
        None => match std::env::current_dir() {
            Ok(cwd) => JsString::from_bytes(cwd.as_os_str().as_bytes()),
            Err(error) => {
                eprintln!("{error}");
                return 1;
            }
        },
    };
    let system = match crate::system::OsSystem::new() {
        Ok(system) => Arc::new(system),
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    let callbacks = if flags.callbacks.is_empty() {
        Vec::new()
    } else {
        flags.callbacks.split(',').map(str::to_owned).collect()
    };
    let server = StdioServer::new(StdioServerOptions {
        cwd,
        default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
        fs: Arc::new(tsr_bundled::BundledFs::new(tsr_vfs::os::shared_fs())),
        pipe_path: (!flags.pipe.is_empty()).then(|| flags.pipe.clone()),
        callbacks,
        async_mode: flags.r#async,
        collect_timing: flags.timing,
        run_external_code: flags.run_external_code,
        mapper_spawner: Some(system),
        session_hook: None,
    });
    let context = Context::background().with_cancel();
    let _signals = match crate::signals::Scope::new(context.clone()) {
        Ok(scope) => scope,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    if let Err(error) = server.run(&context) {
        eprintln!("{error}");
        return 1;
    }
    0
}
