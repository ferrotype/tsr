//! Shared diagnostic phase executor. An optional P5 callback uses the same
//! checked owner after all requested diagnostic phases. Measurement hooks mark
//! the checker interval of `data/s08/checker-workload.json`; the inventory
//! executables pass `NoHooks`.
use serde_json::{json, Value};
use std::sync::Arc;
use ts_checker::{CheckerOwner, Error};
use ts_compiler as ts_compiler_error;
use ts_compiler::{FileCache, Program, ProgramOptions};
mod config;
pub mod diagnostics;
#[path = "../../s07/program/rust_observation.rs"]
mod observation;

/// Checker-interval boundaries. Loading, parsing and binding precede
/// `interval_start`; JSON transport inside the interval is bracketed by
/// `pause`/`resume`; `checkpoint` runs after the complete schedule with the
/// checker and every escaping result live.
#[allow(dead_code)]
pub trait Hooks {
    /// The program is loaded, parsed and bound; nothing is timed yet.
    fn loaded(&mut self, _program: &Program) {}
    fn interval_start(&mut self) {}
    fn init_start(&mut self) {}
    fn init_end(&mut self) {}
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    /// Query results the walker retains as roots of the retained checkpoint.
    fn roots(&mut self, _types: &[ts_checker::TypeRef]) {}
    fn checkpoint(&mut self, _op: &mut ts_checker::Operation<'_>) {}
}
#[allow(dead_code)]
pub struct NoHooks;
impl Hooks for NoHooks {}

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
    cache: &mut FileCache,
    hooks: &mut dyn Hooks,
    baseline: impl FnOnce(
        &Program,
        &mut ts_checker::Operation<'_>,
        &Value,
        Option<&[ts_ast::Diagnostic]>,
        &mut dyn Hooks,
    ) -> BaselineResults,
) -> Value {
    let counters = ts_arena::Counters::new();
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
    let program = match observation::try_load(&request["loading"], cache, &counters, parsed_config)
    {
        Ok(program) => {
            let program = Arc::new(program);
            hooks.loaded(&program);
            program
        }
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
    // Bound inputs and the loader state are complete: the checker interval
    // begins. Diagnostic JSON conversion is transport, bracketed out of it.
    hooks.interval_start();
    let config_values = program.config().config_file_parsing_diagnostics();
    hooks.pause();
    row["phases"]["config"] =
        diagnostics::captured_phase(&program, &config_values, &mut diagnostic_values);
    hooks.resume();
    let program_values = program.program_diagnostics();
    hooks.pause();
    row["phases"]["program"] = match program_values {
        Ok(values) => diagnostics::captured_phase(&program, values, &mut diagnostic_values),
        Err(error) => failure(format!("{error:?}"), "compiler_error"),
    };
    hooks.resume();
    let mut bind = Vec::new();
    for file in program.files() {
        let source = file.bound().view().source_file().expect("published source");
        bind.extend_from_slice(source.bind_diagnostics());
    }
    let syntactic_values = program.syntactic_diagnostics(None);
    hooks.pause();
    row["phases"]["syntactic"] = match syntactic_values {
        Ok(values) => diagnostics::captured_phase(&program, &values, &mut diagnostic_values),
        Err(error) => failure(format!("{error:?}"), "compiler_error"),
    };
    // Keep raw bind diagnostics for attribution; the production semantic API
    // separately applies native selection, directives and plain-JS filtering.
    row["bind_diagnostics"] = diagnostics::phase(&program, &bind);
    hooks.resume();
    let generation = ts_arena::Generation::new(&counters);
    hooks.init_start();
    let owner = match CheckerOwner::for_program(
        ts_arena::CheckerIdentity::new(generation, &counters),
        &counters,
        Arc::new(ts_compiler::ProgramCheckerHost::new(program.clone())),
    ) {
        Ok(owner) => Arc::new(owner),
        Err(error) => {
            hooks.init_end();
            row["phases"]["semantic"] = checker_failure(error);
            row["phases"]["global"] = absent("checker initialization failed");
            return row;
        }
    };
    let mut op = match owner.operation() {
        Ok(op) => op,
        Err(error) => {
            hooks.init_end();
            row["phases"]["semantic"] = checker_failure(error);
            row["phases"]["global"] = absent("checker operation failed");
            return row;
        }
    };
    hooks.init_end();
    let mut semantic = Vec::new();
    for file in program.files() {
        let source = file.bound().view().source_file().expect("published source");
        hooks.pause();
        let name = diagnostics::hex(source.parse_options().file_name.as_bytes());
        hooks.resume();
        let result = match program.skip_type_checking(file, false) {
            Ok(skipped) => match program.semantic_diagnostics_with_checker(&mut op, file) {
                Ok(values) => {
                    hooks.pause();
                    let mut value =
                        diagnostics::captured_phase(&program, &values, &mut diagnostic_values);
                    value["selection"] = json!(if skipped { "native_skip" } else { "checked" });
                    hooks.resume();
                    value
                }
                Err(error) => compiler_failure(error),
            },
            Err(error) => compiler_failure(error),
        };
        hooks.pause();
        semantic.push(json!({"file_hex":name,"result":result}));
        hooks.resume();
    }
    hooks.pause();
    row["phases"]["semantic"] = json!({"state":if semantic.iter().all(|r|r["result"]["state"]=="executed") {"executed"} else {"failed"},"files":semantic,"api":"Program.getSemanticDiagnosticsWithChecker"});
    hooks.resume();
    let global_values = op.global_diagnostics();
    hooks.pause();
    row["phases"]["global"] = match global_values {
        Ok(values) => diagnostics::captured_phase(&program, &values, &mut diagnostic_values),
        Err(error) => checker_failure(error),
    };
    hooks.resume();
    if row["phases"].get("declaration").is_some() {
        let mut declarations = Vec::new();
        for file in program.files() {
            let source = file.bound().view().source_file().expect("published source");
            hooks.pause();
            let name = diagnostics::hex(source.parse_options().file_name.as_bytes());
            hooks.resume();
            let values = program.declaration_diagnostics_with_checker(&mut op, file);
            hooks.pause();
            let result = match values {
                Ok(values) => {
                    diagnostics::captured_phase(&program, &values, &mut diagnostic_values)
                }
                Err(error) => compiler_failure(error),
            };
            declarations.push(json!({"file_hex":name,"result":result}));
            hooks.resume();
        }
        hooks.pause();
        row["phases"]["declaration"] = json!({"state":if declarations.iter().all(|r|r["result"]["state"]=="executed") {"executed"} else {"failed"},"files":declarations,"api":"Program.getDeclarationDiagnostics"});
        hooks.resume();
    }
    if row["phases"].get("suggestion").is_some() {
        let mut suggestions = Vec::new();
        for file in program.files() {
            let source = file.bound().view().source_file().expect("published source");
            hooks.pause();
            let name = diagnostics::hex(source.parse_options().file_name.as_bytes());
            hooks.resume();
            let values = program.suggestion_diagnostics_with_checker(&mut op, file);
            hooks.pause();
            let result = match values {
                Ok(values) => {
                    diagnostics::captured_phase(&program, &values, &mut diagnostic_values)
                }
                Err(error) => compiler_failure(error),
            };
            suggestions.push(json!({"file_hex":name,"result":result}));
            hooks.resume();
        }
        hooks.pause();
        row["phases"]["suggestion"] = json!({"state":if suggestions.iter().all(|r|r["result"]["state"]=="executed") {"executed"} else {"failed"},"files":suggestions,"api":"Checker.GetSuggestionDiagnostics"});
        hooks.resume();
    }
    if request["type_baseline_requested"] == true || capture_errors {
        let results = baseline(
            &program,
            &mut op,
            &row["phases"],
            diagnostic_values.as_deref(),
            hooks,
        );
        hooks.pause();
        row["type_symbol_baselines"] = results.type_symbols;
        if capture_errors {
            row["error_baseline"] = results.errors;
        }
        hooks.resume();
    }
    hooks.checkpoint(&mut op);
    row
}
