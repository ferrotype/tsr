//! The native build-info witness (`tools/phase3/incremental`, recorded by
//! `scripts/phase3_incremental.py` into `tests/fixtures/buildinfo.native.json`
//! from `tests/fixtures/buildinfo.requests.json`). Each case is a compiler
//! test source and a list of steps; each step is one compilation as the
//! pin's harness runs its post-emit compilation: the program loaded from the
//! step's loading request, the old program read from the build info on its
//! file system with the harness's test reader, `new_program` over both, and
//! the step's actions. This suite replays every step and requires the same
//! emit results, the same diagnostics and the same written files, the build
//! info's text byte for byte.
//!
//! `PHASE3_INCREMENTAL_CASE=<id>` narrows a run during development.
#[path = "../../../tools/s08/p4/executor.rs"]
#[allow(dead_code)]
mod executor;
/// The harness's `createProgram`, which the corpus harness shares.
#[path = "../../../tools/phase3/harness/incremental.rs"]
#[allow(dead_code)]
mod harness;

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use tsr_checker::CheckerRequest;
use tsr_compiler::{EmitOptions, EmitResult, FileCache, WriteFileData};

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/buildinfo.native.json");
    let document: Value = serde_json::from_slice(&std::fs::read(&path).expect("native fixture"))
        .expect("fixture JSON");
    assert_eq!(document["version"], 1, "unknown fixture version");
    document
}

/// The harness's output recorder: one entry per real path, the last write's
/// text.
#[derive(Default)]
struct Recorder(Mutex<BTreeMap<String, Vec<u8>>>);

impl Recorder {
    fn write(&self, name: &[u8], text: &[u8]) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(String::from_utf8_lossy(name).into_owned(), text.to_vec());
    }
}

fn emit_json(result: Option<&EmitResult>, program: &tsr_compiler::Program) -> Value {
    result.map_or(Value::Null, |emit| {
        json!({"emit_skipped":emit.emit_skipped,
            "emitted_files":emit.emitted_files.iter().map(|name| String::from_utf8_lossy(name.as_bytes()).into_owned()).collect::<Vec<_>>(),
            "diagnostics":diagnostics_json(&emit.diagnostics, program)})
    })
}

fn diagnostics_json(diagnostics: &[tsr_ast::Diagnostic], program: &tsr_compiler::Program) -> Value {
    Value::Array(
        diagnostics
            .iter()
            .map(|d| {
                let file = d.file.map_or_else(String::new, |id| {
                    if let Some(config) = program.config_source(id) {
                        return config.file.view().source_file(config.root).map_or_else(
                            |_| "?".to_owned(),
                            |source| String::from_utf8_lossy(source.file_name()).into_owned(),
                        );
                    }
                    program.file_of_node(id).map_or_else(
                        || "?".to_owned(),
                        |file| {
                            file.bound().view().source_file().map_or_else(
                                |_| "?".to_owned(),
                                |source| String::from_utf8_lossy(source.file_name()).into_owned(),
                            )
                        },
                    )
                });
                json!({"file":file,"pos":d.loc.pos(),"end":d.loc.end(),"code":d.code})
            })
            .collect(),
    )
}

/// The native diagnostics without their messages, which this suite does not
/// render.
fn native_diagnostics(values: &Value) -> Value {
    Value::Array(
        values
            .as_array()
            .expect("diagnostics")
            .iter()
            .map(|d| json!({"file":d["file"],"pos":d["pos"],"end":d["end"],"code":d["code"]}))
            .collect(),
    )
}

/// One step: the observation the native fixture records for it.
fn run_step(case: &Value, step: &Value) -> Result<Value, String> {
    let request = json!({"id":case["id"],"loading":step["loading"],"error_inputs":case["error_inputs"],"mode":"single"});
    let _scope = executor::content_mapper_scope(&request, tsr_contentmappertest::new_spawner())
        .map_err(|failure| failure.to_string())?;
    let checked = Arc::new(
        executor::load_fresh_checked(&request, &mut FileCache::new())
            .map_err(|failure| failure.to_string())?,
    );
    let loaded = checked.program().clone();
    // A case whose program is "incremental" wraps every program, as the
    // driver's `forceIncremental` does.
    let program = if case["program"] == "incremental" {
        harness::incremental_program(checked)
    } else {
        harness::create_program(checked)
    }
    .map_err(|error| format!("{error:?}"))?;
    let program = program.program_like();
    let recorder = Recorder::default();
    let write_file = |name: &[u8], text: &[u8], _: &mut WriteFileData| {
        let real = loaded
            .host()
            .realpath(name)
            .map_or_else(|_| name.to_vec(), |path| path.as_bytes().to_vec());
        recorder.write(&real, text);
        Ok(())
    };
    let request = CheckerRequest::default();
    let mut emits = Vec::new();
    let mut diagnostics = Vec::new();
    for action in step["actions"].as_array().expect("actions") {
        match action.as_str() {
            Some("emit") => {
                let result = program
                    .emit(
                        &request,
                        &EmitOptions {
                            write_file: Some(&write_file),
                            ..EmitOptions::default()
                        },
                    )
                    .map_err(|error| format!("{error:?}"))?;
                emits.push(emit_json(result.as_ref(), &loaded));
            }
            Some("diagnostics") => {
                let mut values = Vec::new();
                let mut collect = || -> Result<(), tsr_compiler::Error> {
                    values.extend(program.config_file_parsing_diagnostics());
                    values.extend(program.program_diagnostics()?);
                    values.extend(program.syntactic_diagnostics(&request, None)?);
                    values.extend(program.semantic_diagnostics(&request, None)?);
                    values.extend(program.global_diagnostics(&request)?);
                    if program.options().emit_declarations() {
                        values.extend(program.declaration_diagnostics(&request, None)?);
                    }
                    Ok(())
                };
                collect().map_err(|error| format!("{error:?}"))?;
                let sorted = loaded
                    .sort_and_deduplicate_diagnostics(&values)
                    .map_err(|error| format!("{error:?}"))?;
                diagnostics.push(diagnostics_json(&sorted, &loaded));
            }
            other => return Err(format!("unknown action {other:?}")),
        }
    }
    let outputs: Vec<Value> = recorder
        .0
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .into_iter()
        .map(|(name, text)| match String::from_utf8(text) {
            Ok(text) => json!({"name":name,"text":text}),
            Err(error) => json!({"name":name,"text_hex":hex(error.as_bytes())}),
        })
        .collect();
    Ok(json!({"emits":emits,"diagnostics":diagnostics,"outputs":outputs}))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").expect("writing to a string");
    }
    text
}

/// The native step's observation in the shape `run_step` returns.
fn native_step(step: &Value) -> Value {
    let emits: Vec<Value> = step["emits"]
        .as_array()
        .expect("emits")
        .iter()
        .map(|emit| {
            if emit.is_null() {
                return Value::Null;
            }
            json!({"emit_skipped":emit["emit_skipped"],"emitted_files":emit["emitted_files"],
                "diagnostics":native_diagnostics(&emit["diagnostics"])})
        })
        .collect();
    let diagnostics: Vec<Value> = step["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(native_diagnostics)
        .collect();
    json!({"emits":emits,"diagnostics":diagnostics,"outputs":step["outputs"]})
}

#[test]
fn affects_build_info_is_the_pins_declaration_list() {
    let document = fixture();
    let native: Vec<&str> = document["affects_build_info"]
        .as_array()
        .expect("affects_build_info")
        .iter()
        .map(|name| name.as_str().expect("name"))
        .collect();
    assert_eq!(
        tsr_tsoptions::affects::AFFECTS_BUILD_INFO,
        native.as_slice()
    );
}

#[test]
fn compiler_version_is_the_pins() {
    assert_eq!(fixture()["compiler_version"], tsr_core::version());
}

#[test]
fn build_info_and_outputs_match_native() {
    let document = fixture();
    let only = std::env::var("PHASE3_INCREMENTAL_CASE").ok();
    let mut checked = 0;
    let mut failures = Vec::new();
    for case in document["cases"].as_array().expect("cases") {
        let id = case["id"].as_str().expect("case id");
        if only.as_deref().is_some_and(|only| only != id) {
            continue;
        }
        for (index, step) in case["steps"].as_array().expect("steps").iter().enumerate() {
            checked += 1;
            let expected = native_step(step);
            match run_step(case, step) {
                Ok(actual) if actual == expected => {}
                Ok(actual) => failures.push(format!(
                    "{id} step {index}:\n  native {}\n  rust   {}",
                    serde_json::to_string(&expected).unwrap(),
                    serde_json::to_string(&actual).unwrap()
                )),
                Err(error) => failures.push(format!("{id} step {index}: {error}")),
            }
        }
    }
    assert!(checked > 0, "no step ran");
    assert!(
        failures.is_empty(),
        "{} of {checked} steps differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
