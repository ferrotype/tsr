//! The Rust side of the corpus syntax schedule.
//!
//! Each selected schedule row is loaded with the production `Program::load`
//! through the loader bridge S07 already shares with its program parity, then
//! observed with `Program::syntactic_diagnostics(None)` alone and rendered with
//! the production `DiagnosticWriter`. The row shape is the native probe's, so a
//! replay compares the two as values.

use std::error::Error;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tsr_compiler::diagnostic_writer::{DiagnosticWriter, FormattingOptions};
use tsr_compiler::{FileCache, Program};

use crate::{hex, observation, ts_compiler_error};

fn observe(request: &Value, cache: &mut FileCache, counters: &tsr_arena::Counters) -> Value {
    let program = match observation::try_load(request, cache, counters, None) {
        Ok(program) => program,
        Err(ts_compiler_error::Error::Unsupported(name)) => {
            return json!({"state": "not_implemented", "operation": name});
        }
        Err(error) => return json!({"state": "load_error", "error": format!("{error:?}")}),
    };
    match syntactic(request, &program) {
        Ok(mut row) => {
            row["state"] = json!("observed");
            row
        }
        Err(ts_compiler_error::Error::Unsupported(name)) => {
            json!({"state": "not_implemented", "operation": name})
        }
        Err(error) => json!({"state": "error", "error": format!("{error:?}")}),
    }
}

fn syntactic(request: &Value, program: &Program) -> Result<Value, ts_compiler_error::Error> {
    let names: Vec<Vec<u8>> = program
        .files()
        .iter()
        .map(|file| {
            Ok(file
                .bound()
                .view()
                .source_file()?
                .parse_options()
                .file_name
                .as_bytes()
                .to_vec())
        })
        .collect::<Result<_, ts_compiler_error::Error>>()?;
    let diagnostics = program.syntactic_diagnostics(None)?;
    let structured: Vec<Value> = diagnostics
        .iter()
        .map(|d| observation::diagnostic(d, program))
        .collect();
    let mut writer = DiagnosticWriter::new(
        program,
        FormattingOptions {
            new_line: b"\r\n".to_vec(),
            current_directory: request["cwd"]
                .as_str()
                .unwrap_or_default()
                .as_bytes()
                .to_vec(),
            case_sensitive: request["case_sensitive"].as_bool().unwrap_or_default(),
        },
    );
    let refs: Vec<_> = diagnostics.iter().collect();
    let plain = writer.format(&refs, false)?;
    let pretty = writer.format(&refs, true)?;
    Ok(json!({
        "files": names.len(),
        "file_names_sha256": hex(&Sha256::digest(names.join(&b'\n'))),
        "syntactic": structured,
        "plain_hex": hex(&plain),
        "pretty_hex": hex(&pretty),
    }))
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "non-string panic".to_owned())
}

/// `requests` holds native probe requests; `output` receives one JSON row per
/// request, in order, flushed as it is produced.
pub fn run(requests: &str, output: &str) -> Result<(), Box<dyn Error>> {
    let requests: Vec<Value> = serde_json::from_slice(&std::fs::read(requests)?)?;
    let mut out = std::io::BufWriter::new(std::fs::File::create(output)?);
    let counters = tsr_arena::Counters::new();
    let mut cache = FileCache::new();
    for probe in &requests {
        let id = probe["id"].as_str().ok_or("probe request without id")?;
        let mut row = if probe["load"].as_bool() == Some(true) {
            catch_unwind(AssertUnwindSafe(|| {
                observe(&probe["request"], &mut cache, &counters)
            }))
            .unwrap_or_else(|payload| json!({"state": "panic", "panic": panic_text(&*payload)}))
        } else {
            json!({"state": "not_loaded"})
        };
        row["id"] = json!(id);
        serde_json::to_writer(&mut out, &row)?;
        writeln!(out)?;
        out.flush()?;
        cache.prune();
    }
    Ok(())
}
