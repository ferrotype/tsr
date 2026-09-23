//! `syntacticDiagnostics` group (F4a plan task 4). Each request is a small
//! in-memory program. It is loaded by the production `Program::load` through
//! the shared S07 loader bridge, observed through
//! `Program::syntactic_diagnostics` or `Program::bind_diagnostics`, for the
//! requested phase and file scope, and rendered through the production
//! `DiagnosticWriter`. Nothing semantic runs.
//!
//! Diagnostics travel as the probe's positional arrays, [file, pos, end, code,
//! category, key, args, text, chain, related].

use serde_json::{json, Map, Value};
use tsr_compiler::diagnostic_writer::{DiagnosticWriter, FormattingOptions};
use tsr_compiler::{FileCache, ProgramFile};

use crate::api::{subject, Outcome};
use crate::{hex, observation, ts_compiler_error};

pub fn observe(request: &Value) -> Option<Outcome> {
    matches!(subject(request), "syntacticDiagnostics" | "bindDiagnostics").then(|| {
        match run(request) {
            Ok(value) => Outcome::Observed(value),
            Err(error) => Outcome::Failed(error),
        }
    })
}

/// The program spec in the shape the S07 loader bridge reads.
fn loader_request(program: &Value) -> Result<Value, String> {
    let files = program
        .get("files")
        .and_then(Value::as_object)
        .ok_or("program has no files")?;
    let files: Map<String, Value> = files
        .iter()
        .map(|(name, text)| {
            let text = text.as_str().ok_or("file text is not a string")?;
            Ok((name.clone(), Value::String(hex(text.as_bytes()))))
        })
        .collect::<Result<_, String>>()?;
    Ok(json!({
        "cwd": program["cwd"],
        "case_sensitive": program["case_sensitive"],
        "files": files,
        "symlinks": {},
        "roots": program["roots"],
        "options": program["options"],
        "skip_module_resolution": false,
    }))
}

fn file_name(file: &ProgramFile) -> Result<Vec<u8>, String> {
    Ok(file
        .bound()
        .view()
        .source_file()
        .map_err(|e| format!("{e:?}"))?
        .parse_options()
        .file_name
        .as_bytes()
        .to_vec())
}

/// The loader bridge's named diagnostic as the probe's positional array.
fn positional(d: &Value) -> Value {
    let list = |key: &str| match &d[key] {
        Value::Array(items) => Value::Array(items.iter().map(positional).collect()),
        _ => json!([]),
    };
    let args = match &d["Args"] {
        Value::Array(items) => Value::Array(items.clone()),
        _ => json!([]),
    };
    json!([
        d["File"],
        d["Pos"],
        d["End"],
        d["Code"],
        d["Category"],
        d["Key"],
        args,
        d["Text"],
        list("Chain"),
        list("Related")
    ])
}

fn run(request: &Value) -> Result<Value, String> {
    let spec = request.get("program").ok_or("request has no program")?;
    let loader = loader_request(spec)?;
    let counters = tsr_arena::Counters::new();
    let mut cache = FileCache::new();
    let program = observation::try_load(&loader, &mut cache, &counters, None)
        .map_err(|e| format!("program load failed: {e:?}"))?;
    let names = program
        .files()
        .iter()
        .map(|file| file_name(file))
        .collect::<Result<Vec<_>, _>>()?;
    let target = match request.get("scope").and_then(Value::as_str) {
        None => None,
        Some(scope) => {
            let index = names
                .iter()
                .position(|name| name.as_slice() == scope.as_bytes())
                .ok_or_else(|| format!("scope {scope} is not a program file"))?;
            Some(&program.files()[index])
        }
    };
    let diagnostics = if subject(request) == "bindDiagnostics" {
        program.bind_diagnostics(target.map(|file| file.source()))
    } else {
        program.syntactic_diagnostics(target.map(|file| &**file))
    }
    .map_err(|e: ts_compiler_error::Error| format!("{} failed: {e:?}", subject(request)))?;
    let ordered: Vec<Value> = diagnostics
        .iter()
        .map(|d| positional(&observation::diagnostic(d, &program)))
        .collect();
    let mut writer = DiagnosticWriter::new(
        &program,
        FormattingOptions {
            new_line: b"\r\n".to_vec(),
            current_directory: spec["cwd"].as_str().unwrap_or_default().as_bytes().to_vec(),
            case_sensitive: spec["case_sensitive"].as_bool().unwrap_or_default(),
            ..FormattingOptions::default()
        },
    );
    let refs: Vec<_> = diagnostics.iter().collect();
    let plain = writer.format(&refs, false).map_err(|e| format!("{e:?}"))?;
    let pretty = writer.format(&refs, true).map_err(|e| format!("{e:?}"))?;
    let files: Vec<Value> = names
        .iter()
        .map(|name| Value::String(String::from_utf8_lossy(name).into_owned()))
        .collect();
    Ok(json!({
        "ordered": ordered,
        "files": files,
        "plain_hex": hex(&plain),
        "pretty_hex": hex(&pretty),
    }))
}
