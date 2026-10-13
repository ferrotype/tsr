mod allocator;
mod discovery;
mod engine;
mod metrics;
mod protocol;
mod stream;

use std::io::{BufRead, Read, Write};
const MAX_FRAME: usize = 256 * 1024 * 1024;

fn run() -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let mut engine = engine::Engine::default();
    loop {
        let mut bytes = Vec::new();
        let count = (&mut input)
            .take((MAX_FRAME + 1) as u64)
            .read_until(b'\n', &mut bytes)
            .map_err(|e| e.to_string())?;
        if count == 0 {
            return Ok(());
        }
        if count > MAX_FRAME {
            return Err("budget frame exceeds 256 MiB".into());
        }
        let response = engine.execute_command(&bytes);
        match response {
            Ok(response) => {
                serde_json::to_writer(&mut output, &response).map_err(|e| e.to_string())?
            }
            Err(error) => {
                let envelope = protocol::envelope(&bytes).ok();
                let version = envelope.as_ref().map_or(1, |e| e.version);
                let id = envelope.map_or_else(String::new, |e| e.id.to_owned());
                serde_json::to_writer(
                    &mut output,
                    &protocol::Response {
                        version,
                        id,
                        kind: "failure",
                        value: &error,
                    },
                )
                .map_err(|e| e.to_string())?;
                output.write_all(b"\n").map_err(|e| e.to_string())?;
                output.flush().map_err(|e| e.to_string())?;
                return Err(error);
            }
        }
        output.write_all(b"\n").map_err(|e| e.to_string())?;
        output.flush().map_err(|e| e.to_string())?;
    }
}
fn run_cli(arguments: Vec<std::ffi::OsString>) -> Result<(), String> {
    if arguments.is_empty() {
        return run();
    }
    if arguments.len() != 7
        || arguments[0] != "--discover"
        || arguments[1] != "--metadata"
        || arguments[3] != "--input"
        || arguments[5] != "--output"
    {
        return Err(
            "usage: phase7_rust_budget [--discover --metadata FILE --input FILE --output NEW_FILE]"
                .into(),
        );
    }
    discovery::run(
        std::path::Path::new(&arguments[2]),
        std::path::Path::new(&arguments[4]),
        std::path::Path::new(&arguments[6]),
    )
}
fn main() {
    let arguments = std::env::args_os().skip(1).collect();
    let result = std::thread::Builder::new()
        .stack_size(tsr_core::workgroup::RESERVED_STACK)
        .spawn(move || run_cli(arguments))
        .map_err(|e| e.to_string())
        .and_then(|worker| {
            worker
                .join()
                .map_err(|_| "budget consumer panic invalidated process".into())
        })
        .and_then(|result| result);
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
