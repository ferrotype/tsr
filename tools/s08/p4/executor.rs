//! Shared diagnostic phase executor. An optional P5 callback uses the same
//! checked owner after all requested diagnostic phases. Measurement hooks mark
//! the checker interval of `data/s08/checker-workload.json`; the inventory
//! executables pass `NoHooks`.
use serde_json::{json, Value};
use std::sync::Arc;
use tsr_checker::{CheckerOwner, Error};
use tsr_compiler as ts_compiler_error;
use tsr_compiler::{FileCache, Program, ProgramOptions};
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
    /// Construction policy for external embedding consumers. Native producers
    /// keep their existing loader and checker ownership/initialization timing.
    fn load_program(
        &mut self,
        options: ProgramOptions,
        cache: &mut FileCache,
        counters: &tsr_arena::Counters,
    ) -> Result<Arc<Program>, ts_compiler_error::Error> {
        Program::load(options, cache, counters).map(Arc::new)
    }
    fn create_checker(
        &mut self,
        program: Arc<Program>,
        counters: &tsr_arena::Counters,
    ) -> Result<Arc<CheckerOwner>, Error> {
        CheckerOwner::for_program(
            tsr_arena::CheckerIdentity::new(tsr_arena::Generation::new(counters), counters),
            counters,
            Arc::new(tsr_compiler::ProgramCheckerHost::new(program)),
        )
        .map(Arc::new)
    }
    /// The program is loaded, parsed and bound; nothing is timed yet.
    fn loaded(&mut self, _program: &Program) {}
    fn interval_start(&mut self) {}
    fn init_start(&mut self) {}
    fn init_end(&mut self) {}
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    /// Query results the walker retains as roots of the retained checkpoint.
    fn roots(&mut self, _types: &[tsr_checker::TypeRef]) {}
    fn checkpoint(&mut self, _op: &mut tsr_checker::Operation<'_>) {}
    /// Whether the row needs the loaded graph's observation (file digests,
    /// metadata, imports). A driver that compares only checker output skips
    /// it: hashing every file text per variant is most of the child's time
    /// and streams megabytes through the caches just before the interval.
    fn wants_graph(&self) -> bool {
        true
    }
}
#[allow(dead_code)]
pub struct NoHooks;
impl Hooks for NoHooks {}

pub fn failure(reason: impl std::fmt::Display, class: &str) -> Value {
    json!({"state":"failed","class":class,"reason":reason.to_string()})
}
pub fn checker_failure(error: Error) -> Value {
    match error {
        Error::Unsupported(reason) => failure(reason, "unsupported"),
        _ => failure(error, "checker_error"),
    }
}
pub fn compiler_failure(error: ts_compiler_error::Error) -> Value {
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
        &mut tsr_checker::Operation<'_>,
        &Value,
        Option<&[tsr_ast::Diagnostic]>,
        &mut dyn Hooks,
        &mut FileCache,
    ) -> BaselineResults,
) -> Value {
    let counters = tsr_arena::Counters::new();
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
    let program = match hooks.load_program(
        observation::program_options(&request["loading"], parsed_config),
        cache,
        &counters,
    ) {
        Ok(program) => {
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
    row["load"] = if hooks.wants_graph() {
        json!({"state":"executed","graph":observation::observe(request["id"].as_str().unwrap(), &program)})
    } else {
        json!({"state":"executed"})
    };
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
    hooks.init_start();
    let owner = match hooks.create_checker(program.clone(), &counters) {
        Ok(owner) => owner,
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
            // Program.GetDeclarationDiagnostics(ctx, file): sorted and deduplicated.
            let values = program.declaration_diagnostics(&mut op, Some(file));
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
            cache,
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

#[allow(dead_code)]
/// The configuration's program and checker loaded again, as the pin's harness
/// loads its post-emit program: a fresh load outside any measurement hooks.
pub struct Fresh {
    pub program: Arc<Program>,
    pub owner: Arc<CheckerOwner>,
    _counters: tsr_arena::Counters,
}

#[allow(dead_code)]
pub fn load_fresh(request: &Value, cache: &mut FileCache) -> Result<Fresh, Value> {
    let counters = tsr_arena::Counters::new();
    let parsed = config::parse(request).map_err(|error| failure(error, "config_parse"))?;
    let mut hooks = NoHooks;
    let program = hooks
        .load_program(
            observation::program_options(&request["loading"], parsed),
            cache,
            &counters,
        )
        .map_err(compiler_failure)?;
    let owner = hooks
        .create_checker(program.clone(), &counters)
        .map_err(checker_failure)?;
    Ok(Fresh {
        program,
        owner,
        _counters: counters,
    })
}

#[allow(dead_code)]
/// A program's diagnostics in the pin's harness collection order: config,
/// program, syntactic, semantic, global, then the requested declaration and
/// suggestion phases (`harnessutil.compileFilesWithHost`), unsorted.
pub fn harness_diagnostics(
    program: &Program,
    op: &mut tsr_checker::Operation<'_>,
    phases: &Value,
) -> Result<Vec<tsr_ast::Diagnostic>, Value> {
    let requested = |name: &str| {
        phases
            .as_array()
            .is_some_and(|phases| phases.iter().any(|phase| phase == name))
    };
    let mut values = program.config().config_file_parsing_diagnostics();
    values.extend_from_slice(program.program_diagnostics().map_err(compiler_failure)?);
    values.extend(
        program
            .syntactic_diagnostics(None)
            .map_err(compiler_failure)?,
    );
    for file in program.files() {
        values.extend(
            program
                .semantic_diagnostics_with_checker(op, file)
                .map_err(compiler_failure)?,
        );
    }
    values.extend(op.global_diagnostics().map_err(checker_failure)?);
    if requested("declaration") {
        for file in program.files() {
            values.extend(
                program
                    .declaration_diagnostics(op, Some(file))
                    .map_err(compiler_failure)?,
            );
        }
    }
    if requested("suggestion") {
        for file in program.files() {
            values.extend(
                program
                    .suggestion_diagnostics_with_checker(op, file)
                    .map_err(compiler_failure)?,
            );
        }
    }
    Ok(values)
}

#[allow(dead_code)]
/// The diagnostics `noEmitOnError` asks for before emit, in the pin's
/// `GetDiagnosticsOfAnyProgram` order: each later phase runs only while the
/// earlier ones found nothing.
pub fn any_program_diagnostics(
    program: &Program,
    op: &mut tsr_checker::Operation<'_>,
) -> Result<Vec<tsr_ast::Diagnostic>, Value> {
    let mut values = program.config().config_file_parsing_diagnostics();
    let config = values.len();
    values.extend(
        program
            .syntactic_diagnostics(None)
            .map_err(compiler_failure)?,
    );
    if values.len() == config {
        values.extend_from_slice(program.program_diagnostics().map_err(compiler_failure)?);
        if !program.options().list_files_only.is_true() {
            values.extend(op.global_diagnostics().map_err(checker_failure)?);
            if values.len() == config {
                for file in program.files() {
                    values.extend(
                        program
                            .semantic_diagnostics_with_checker(op, file)
                            .map_err(compiler_failure)?,
                    );
                }
                values.extend(op.global_diagnostics().map_err(checker_failure)?);
            }
            if program.options().emit_declarations() && values.len() == config {
                values.extend(
                    program
                        .declaration_diagnostics(op, None)
                        .map_err(compiler_failure)?,
                );
            }
        }
    }
    Ok(values)
}
