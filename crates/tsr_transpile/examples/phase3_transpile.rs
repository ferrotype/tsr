//! Phase 3 C3: the pinned transpile runner over `tsr_transpile`
//! (`scripts/phase3_transpile.py run`). Each request is one configuration of
//! a transpile test with the runner's inputs (the test file's path, the
//! configuration's name, its units, its compiler options and the harness's
//! `ReportDiagnostics`, all from the native capture); the row records each
//! run's composed baseline and the per-unit outputs behind it
//! (`tools/phase3/harness/transpile.rs`).
//!
//!     phase3_transpile REQUESTS.ndjson ROWS.ndjson
//!
//! A configuration that panics or fails is a `failed` row with its class
//! (`panic`, `harness`) and reason; the other rows still run.
#[path = "../../../tools/s08/p5/errors.rs"]
#[allow(dead_code)]
mod errors;
#[path = "../../../tools/s08/p5/paths.rs"]
mod paths;
#[path = "../../../tools/phase3/harness/transpile.rs"]
mod transpile_runner;

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Mutex, PoisonError};
use transpile_runner::{run_test, run_value, unhex, Configuration, Unit};

/// The location of the last panic, so a failed row names the code that
/// panicked.
static PANIC_LOCATION: Mutex<Option<String>> = Mutex::new(None);

fn last_panic() -> Option<String> {
    PANIC_LOCATION
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn field<'v>(request: &'v Value, name: &str) -> Result<&'v str, String> {
    request[name]
        .as_str()
        .ok_or_else(|| format!("the request has no string {name}"))
}

fn observe(request: &Value) -> Result<Value, String> {
    let units = request["units"]
        .as_array()
        .ok_or("the request has no units")?
        .iter()
        .map(|unit| {
            Ok(Unit {
                name: unhex(field(unit, "name_hex")?)?,
                content: unhex(field(unit, "content_hex")?)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let options = tsr_tsoptions::raw::compiler_options(&request["options"])
        .map_err(|error| format!("the request's options do not read: {error:?}"))?;
    let report_diagnostics = request["report_diagnostics"]
        .as_bool()
        .ok_or("the request has no report_diagnostics")?;
    let file = field(request, "file")?.as_bytes();
    let configuration_name = field(request, "configuration_name")?.as_bytes();
    let runs = run_test(&Configuration {
        file,
        name: configuration_name,
        units: &units,
        options: &options,
        report_diagnostics,
    })?;
    let (configured_name, _) = transpile_runner::configured_name(file, configuration_name);
    Ok(json!({
        "id": request["id"], "state": "executed",
        "configured_name": String::from_utf8_lossy(&configured_name),
        "runs": runs.iter().map(run_value).collect::<Result<Vec<_>, _>>()?,
    }))
}

fn row(request: &Value) -> Value {
    match catch_unwind(AssertUnwindSafe(|| observe(request))) {
        Ok(Ok(row)) => row,
        Ok(Err(reason)) => {
            json!({"id": request["id"], "state": "failed", "class": "harness", "reason": reason})
        }
        Err(payload) => {
            let reason = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic payload");
            json!({"id": request["id"], "state": "failed", "class": "panic", "reason": reason,
                "location": last_panic()})
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::panic::set_hook(Box::new(|info| {
        *PANIC_LOCATION
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = info
            .location()
            .map(|at| format!("{}:{}", at.file(), at.line()));
        eprintln!("{info}");
    }));
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 {
        return Err("usage: phase3_transpile REQUESTS.ndjson ROWS.ndjson".into());
    }
    let requests = std::io::BufReader::new(std::fs::File::open(&args[1])?);
    let mut rows = std::io::BufWriter::new(std::fs::File::create(&args[2])?);
    for line in requests.lines() {
        let request: Value = serde_json::from_str(&line?)?;
        serde_json::to_writer(&mut rows, &row(&request))?;
        rows.write_all(b"\n")?;
    }
    rows.flush()?;
    Ok(())
}
