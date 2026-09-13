//! Diagnostic-only full frozen-corpus executor. No baseline parity is inferred.
use serde_json::{json, Value};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
};
use ts_checker::{CheckerOwner, Error};
use ts_compiler as ts_compiler_error;
use ts_compiler::{FileCache, Program, ProgramOptions};
#[path = "../../../tools/s08/p4/diagnostics.rs"]
mod diagnostics;
#[path = "../../../tools/s07/program/rust_observation.rs"]
mod observation;

fn failure(reason: impl std::fmt::Display, class: &str) -> Value {
    json!({"state":"failed","class":class,"reason":reason.to_string()})
}
fn checker_failure(error: Error) -> Value {
    match error {
        Error::Unsupported(reason) => failure(reason, "unsupported"),
        _ => failure(error, "checker_error"),
    }
}
fn compiler_failure(error: ts_compiler_error::Error) -> Value {
    match error {
        ts_compiler_error::Error::Checker(error) => checker_failure(error),
        ts_compiler_error::Error::Unsupported(reason) => failure(reason, "unsupported"),
        error => failure(format!("{error:?}"), "compiler_error"),
    }
}
fn absent(reason: &str) -> Value {
    json!({"state":"not_implemented","reason":reason})
}
fn observe(request: &Value) -> Value {
    let counters = ts_arena::Counters::new();
    let mut cache = FileCache::new();
    let mut phases = json!({});
    for phase in request["diagnostic_phases"]
        .as_array()
        .expect("validated phase array")
    {
        phases[phase.as_str().expect("validated phase name")] =
            absent("diagnostic phase not reached");
    }
    let mut row = json!({"version":1,"id":request["id"],"acceptance_tier":request["acceptance_tier"],
        "load":null,"phases":phases,"type_symbol_baselines":if request["type_baseline_requested"] == true {
            absent("P5 native baseline walker/display schedule")
        } else { json!({"state":"not_requested"}) }});
    let program = match observation::try_load(&request["loading"], &mut cache, &counters) {
        Ok(program) => Arc::new(program),
        Err(ts_compiler_error::Error::Unsupported(reason)) => {
            row["load"] = failure(reason, "unsupported");
            return row;
        }
        Err(error) => {
            row["load"] = failure(format!("{error:?}"), "compiler_error");
            return row;
        }
    };
    row["load"] = json!({"state":"executed","graph":observation::observe(request["id"].as_str().unwrap(), &program)});
    row["phases"]["config"] = diagnostics::phase(&program, &program.config().errors);
    row["phases"]["program"] = match program.program_diagnostics() {
        Ok(values) => diagnostics::phase(&program, values),
        Err(error) => failure(format!("{error:?}"), "compiler_error"),
    };
    let mut syntactic = Vec::new();
    let mut bind = Vec::new();
    for file in program.files() {
        let source = file.bound().view().source_file().expect("published source");
        syntactic.extend_from_slice(source.diagnostics());
        bind.extend_from_slice(source.bind_diagnostics());
    }
    row["phases"]["syntactic"] = diagnostics::phase(&program, &syntactic);
    // Keep raw bind diagnostics for attribution; the production semantic API
    // separately applies native selection, directives and plain-JS filtering.
    row["bind_diagnostics"] = diagnostics::phase(&program, &bind);
    let generation = ts_arena::Generation::new(&counters);
    let owner = match CheckerOwner::for_program(
        ts_arena::CheckerIdentity::new(generation, &counters),
        &counters,
        Arc::new(ts_compiler::ProgramCheckerHost::new(program.clone())),
    ) {
        Ok(owner) => Arc::new(owner),
        Err(error) => {
            row["phases"]["semantic"] = checker_failure(error);
            row["phases"]["global"] = absent("checker initialization failed");
            return row;
        }
    };
    let mut op = match owner.operation() {
        Ok(op) => op,
        Err(error) => {
            row["phases"]["semantic"] = checker_failure(error);
            row["phases"]["global"] = absent("checker operation failed");
            return row;
        }
    };
    let mut semantic = Vec::new();
    for file in program.files() {
        let source = file.bound().view().source_file().expect("published source");
        let name = diagnostics::hex(source.parse_options().file_name.as_bytes());
        let result = match program.skip_type_checking(file, false) {
            Ok(skipped) => match program.semantic_diagnostics_with_checker(&mut op, file) {
                Ok(values) => {
                    let mut value = diagnostics::phase(&program, &values);
                    value["selection"] = json!(if skipped { "native_skip" } else { "checked" });
                    value
                }
                Err(error) => compiler_failure(error),
            },
            Err(error) => compiler_failure(error),
        };
        semantic.push(json!({"file_hex":name,"result":result}));
    }
    row["phases"]["semantic"] = json!({"state":if semantic.iter().all(|r|r["result"]["state"]=="executed") {"executed"} else {"failed"},"files":semantic,"api":"Program.getSemanticDiagnosticsWithChecker"});
    row["phases"]["global"] = match op.global_diagnostics() {
        Ok(values) => diagnostics::phase(&program, &values),
        Err(error) => checker_failure(error),
    };
    if row["phases"].get("declaration").is_some() {
        let mut declarations = Vec::new();
        for file in program.files() {
            let source = file.bound().view().source_file().expect("published source");
            let name = diagnostics::hex(source.parse_options().file_name.as_bytes());
            let result = match program.declaration_diagnostics_with_checker(&mut op, file) {
                Ok(values) => diagnostics::phase(&program, &values),
                Err(error) => compiler_failure(error),
            };
            declarations.push(json!({"file_hex":name,"result":result}));
        }
        row["phases"]["declaration"] = json!({"state":if declarations.iter().all(|r|r["result"]["state"]=="executed") {"executed"} else {"failed"},"files":declarations,"api":"Program.getDeclarationDiagnostics"});
    }
    if row["phases"].get("suggestion").is_some() {
        let mut suggestions = Vec::new();
        for file in program.files() {
            let source = file.bound().view().source_file().expect("published source");
            let name = diagnostics::hex(source.parse_options().file_name.as_bytes());
            let result = match op.recorded_suggestions(file.source()) {
                Ok(values) => diagnostics::phase(&program, &values),
                Err(error) => checker_failure(error),
            };
            suggestions.push(json!({"file_hex":name,"result":result}));
        }
        row["phases"]["suggestion"] = json!({"state":if suggestions.iter().all(|r|r["result"]["state"]=="executed") {"executed"} else {"failed"},"files":suggestions,"api":"Checker.GetSuggestionDiagnostics"});
    }
    row
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: p4_inventory request.json observation.json".into());
    }
    let request: Value = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let result = catch_unwind(AssertUnwindSafe(|| observe(&request)));
    let row = match result {
        Ok(row) => row,
        Err(payload) => {
            let reason = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic payload");
            json!({"version":1,"id":request["id"],"acceptance_tier":request["acceptance_tier"],"fatal":failure(reason,"panic")})
        }
    };
    let mut raw = serde_json::to_vec(&row)?;
    raw.push(b'\n');
    std::fs::write(&args[2], raw)?;
    Ok(())
}
