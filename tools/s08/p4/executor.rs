//! Shared diagnostic phase executor. An optional P5 callback uses the same
//! checked owner after all requested diagnostic phases.
use serde_json::{json, Value};
use std::sync::Arc;
use ts_checker::{CheckerOwner, Error};
use ts_compiler as ts_compiler_error;
use ts_compiler::{FileCache, Program, ProgramOptions};
mod config;
pub mod diagnostics;
#[path = "../../s07/program/rust_observation.rs"]
mod observation;

pub fn failure(reason: impl std::fmt::Display, class: &str) -> Value {
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
pub struct BaselineResults {
    pub type_symbols: Value,
    pub errors: Value,
}

pub fn observe(
    request: &Value,
    baseline: impl FnOnce(
        &Program,
        &mut ts_checker::Operation<'_>,
        &Value,
        Option<&[ts_ast::Diagnostic]>,
    ) -> BaselineResults,
) -> Value {
    let counters = ts_arena::Counters::new();
    let mut cache = FileCache::new();
    let capture_errors = request["error_baseline_requested"] == true;
    let mut diagnostic_values = capture_errors.then(Vec::new);
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
    if capture_errors {
        row["error_baseline"] = absent("diagnostic aggregation not reached");
    }
    let parsed_config = match config::parse(request) {
        Ok(config) => config,
        Err(error) => {
            row["load"] = failure(error, "config_parse");
            return row;
        }
    };
    let program =
        match observation::try_load(&request["loading"], &mut cache, &counters, parsed_config) {
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
    row["phases"]["config"] = diagnostics::captured_phase(
        &program,
        &program.config().config_file_parsing_diagnostics(),
        &mut diagnostic_values,
    );
    row["phases"]["program"] = match program.program_diagnostics() {
        Ok(values) => diagnostics::captured_phase(&program, values, &mut diagnostic_values),
        Err(error) => failure(format!("{error:?}"), "compiler_error"),
    };
    let mut bind = Vec::new();
    for file in program.files() {
        let source = file.bound().view().source_file().expect("published source");
        bind.extend_from_slice(source.bind_diagnostics());
    }
    row["phases"]["syntactic"] = match program.syntactic_diagnostics(None) {
        Ok(values) => diagnostics::captured_phase(&program, &values, &mut diagnostic_values),
        Err(error) => failure(format!("{error:?}"), "compiler_error"),
    };
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
                    let mut value =
                        diagnostics::captured_phase(&program, &values, &mut diagnostic_values);
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
        Ok(values) => diagnostics::captured_phase(&program, &values, &mut diagnostic_values),
        Err(error) => checker_failure(error),
    };
    if row["phases"].get("declaration").is_some() {
        let mut declarations = Vec::new();
        for file in program.files() {
            let source = file.bound().view().source_file().expect("published source");
            let name = diagnostics::hex(source.parse_options().file_name.as_bytes());
            let result = match program.declaration_diagnostics_with_checker(&mut op, file) {
                Ok(values) => {
                    diagnostics::captured_phase(&program, &values, &mut diagnostic_values)
                }
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
                Ok(values) => {
                    diagnostics::captured_phase(&program, &values, &mut diagnostic_values)
                }
                Err(error) => checker_failure(error),
            };
            suggestions.push(json!({"file_hex":name,"result":result}));
        }
        row["phases"]["suggestion"] = json!({"state":if suggestions.iter().all(|r|r["result"]["state"]=="executed") {"executed"} else {"failed"},"files":suggestions,"api":"Checker.GetSuggestionDiagnostics"});
    }
    if request["type_baseline_requested"] == true || capture_errors {
        let results = baseline(
            &program,
            &mut op,
            &row["phases"],
            diagnostic_values.as_deref(),
        );
        row["type_symbol_baselines"] = results.type_symbols;
        if capture_errors {
            row["error_baseline"] = results.errors;
        }
    }
    row
}
