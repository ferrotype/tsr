//! Native process boundary. Compiler code receives every OS service through System.
mod allocation;
#[global_allocator]
static ALLOCATOR: allocation::CountingAllocator = allocation::CountingAllocator;
mod lsp;
mod process;
mod signals;
mod system;

use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;
use tsr_jsstring::JsString;

/// port: tsc/cmd/tsc/main.go:main
fn main() {
    // Rust reserves the complete stack for the CLI thread; recursive compiler
    // paths retain their existing stack-growth guards.
    let result = std::thread::Builder::new()
        .name("tsrust".into())
        .stack_size(tsr_core::workgroup::RESERVED_STACK)
        .spawn(run_main)
        .expect("start compiler thread")
        .join();
    match result {
        Ok(status) => std::process::exit(status),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// port: tsc/cmd/tsc/main.go:runMain
fn run_main() -> i32 {
    let args: Vec<_> = std::env::args_os()
        .skip(1)
        .map(|arg| JsString::from_bytes(arg.as_bytes()))
        .collect();
    if args.first().is_some_and(|arg| arg.as_bytes() == b"--lsp") {
        return lsp::run(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg.as_bytes() == b"--api") {
        eprintln!("This command is not implemented in this build.");
        return tsr_tsc::ExitStatus::NotImplemented.0;
    }
    let context = tsr_ipc::Context::background().with_cancel();
    let _signals = match signals::Scope::new(context.clone()) {
        Ok(scope) => scope,
        Err(error) => {
            eprintln!("Error installing signal handlers: {error}");
            return 3;
        }
    };
    let sys = match system::OsSystem::new() {
        Ok(sys) => sys,
        Err(error) => {
            eprintln!("Error getting current directory: {error}");
            return 3;
        }
    };
    match tsr_execute::command_line(&context, Arc::new(sys), &args, None) {
        Ok(result) => result.status.0,
        Err(error) => {
            if let Some(operation) = tsr_execute::unsupported_operation(&error) {
                eprintln!("This operation is not implemented in this build: {operation}");
                tsr_tsc::ExitStatus::NotImplemented.0
            } else {
                eprintln!("Command-line compilation failed: {error}");
                tsr_tsc::ExitStatus::InvalidProject_OutputsSkipped.0
            }
        }
    }
}
