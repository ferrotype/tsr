//! The native transform probes (`tools/phase3/probe`, recorded by
//! `scripts/phase3_probe.py`): each fixture under
//! `tests/fixtures/phase3/transforms/` holds small programs, the chains of
//! pinned transformers the probe ran over each of their files, and the text
//! the pin printed. This suite loads the same programs, runs the same chains
//! through the ported transformers with the checker's emit resolver, prints
//! with the emitter's printer options and requires the same bytes.
//!
//! A fixture is added with its transformer: every case of every committed
//! fixture must match. `PHASE3_PROBE=<fixture>` and `PHASE3_PROBE_CASE=<id>`
//! narrow a run during development.
#[path = "../../../tools/s08/p4/executor.rs"]
#[allow(dead_code)]
mod executor;

use serde_json::{json, Value};
use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::rc::Rc;
use tsr_arena::NodeId;
use tsr_ast::AstBuilder;
use tsr_checker::{CheckerRequest, Operation};
use tsr_compiler::{emitter, CheckedProgram, FileCache, Program};
use tsr_core::NewLineKind;
use tsr_jsstring::SourceText;
use tsr_printer::{EmitContext, EmitTextWriter, Printer, PrinterOptions, TextWriter};
use tsr_transformers::{Failure, Transformer};

fn fixtures() -> Vec<(String, Value)> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/phase3/transforms");
    let only = std::env::var("PHASE3_PROBE").ok();
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&directory).expect("fixture directory") {
        let path = entry.expect("fixture entry").path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".native.json") else {
            continue;
        };
        if only.as_deref().is_some_and(|only| only != stem) {
            continue;
        }
        let document: Value =
            serde_json::from_slice(&std::fs::read(&path).expect("fixture")).expect("fixture JSON");
        assert_eq!(document["version"], 1, "{name}: unknown fixture version");
        found.push((stem.to_string(), document));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// The emitter's printer options for a JavaScript file (`emitJSFile`), with
/// no source map.
fn printer_options(program: &Program) -> PrinterOptions {
    let options = program.options();
    PrinterOptions {
        remove_comments: options.remove_comments.is_true(),
        new_line: options.new_line,
        no_emit_helpers: options.no_emit_helpers.is_true(),
        target: options.target,
        ..PrinterOptions::default()
    }
}

fn new_line(kind: NewLineKind) -> &'static [u8] {
    if kind == NewLineKind::CRLF {
        b"\r\n"
    } else {
        b"\n"
    }
}

/// One chain over one file, as the probe's `Phase3Transform`.
fn transform(
    program: &Program,
    op: &mut Operation<'_>,
    source: NodeId,
    chain: &[&str],
) -> Result<Vec<u8>, String> {
    let counters = tsr_arena::Counters::new();
    let context = EmitContext::new();
    let mut output =
        AstBuilder::with_hooks(SourceText::default(), &counters, context.factory_hooks());
    // A transform may read any file its resolver answers with.
    for file in program.files() {
        output.retain_completed(file.bound());
    }
    let failure = Failure::default();
    let resolver: tsr_transformers::SharedEmitResolver<'_> = Rc::new(RefCell::new(op));
    let opts =
        emitter::script_transform_options(&context, program, resolver, source, &counters, failure)
            .map_err(|error| format!("{error:?}"))?;
    let chosen: Vec<Transformer<'_>> = if chain == ["script"] {
        emitter::get_script_transformers(program, &opts, source)
            .map_err(|error| format!("{error:?}"))?
    } else {
        let mut chosen = Vec::new();
        for name in chain {
            let transformer = emitter::transformer_by_name(name, &opts)
                .ok_or_else(|| format!("unknown transformer: {name}"))?;
            chosen.extend(transformer);
        }
        chosen
    };
    let mut file = source;
    for transformer in &chosen {
        file = transformer
            .transform_source_file(&mut output, file)
            .map_err(|error| error.to_string())?;
    }
    drop(chosen);
    drop(opts);
    let mut writer = TextWriter::new(new_line(program.options().new_line), 0);
    Printer::new(printer_options(program), &context)
        .write(output.view(), file, Some(file), &mut writer, None)
        .map_err(|error| format!("{error:?}"))?;
    Ok(writer.text().to_vec())
}

/// The message of a caught panic.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_owned();
    }
    "<non-string panic>".to_owned()
}

fn load(case: &Value) -> Result<CheckedProgram, String> {
    let request = json!({"id": case["id"], "loading": case["loading"], "mode": "single"});
    executor::load_fresh_checked(&request, &mut FileCache::new())
        .map_err(|failure| failure.to_string())
}

fn expected(chain: &Value) -> Option<Vec<u8>> {
    if let Some(text) = chain["text"].as_str() {
        return Some(text.as_bytes().to_vec());
    }
    let hex = chain["text_hex"].as_str()?;
    Some(
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
            .collect(),
    )
}

#[test]
fn ported_transformers_print_what_the_pinned_ones_print() {
    let only_case = std::env::var("PHASE3_PROBE_CASE").ok();
    let mut failures = Vec::new();
    let mut compared = 0usize;
    for (fixture, document) in fixtures() {
        for case in document["cases"].as_array().expect("cases") {
            let id = case["id"].as_str().expect("case id");
            if only_case.as_deref().is_some_and(|only| only != id) {
                continue;
            }
            let checked = match load(case) {
                Ok(checked) => checked,
                Err(error) => {
                    failures.push(format!("{fixture}/{id}: program did not load: {error}"));
                    continue;
                }
            };
            let program = checked.program().clone();
            for file in case["files"].as_array().expect("files") {
                let name = file["name"].as_str().expect("file name");
                let Some(source) = program
                    .source_file(name.as_bytes())
                    .map(tsr_compiler::ProgramFile::source)
                else {
                    failures.push(format!("{fixture}/{id}: the program has no file {name}"));
                    continue;
                };
                for native in file["chains"].as_array().expect("chains") {
                    let chain: Vec<&str> = native["chain"]
                        .as_array()
                        .expect("chain")
                        .iter()
                        .map(|name| name.as_str().expect("transformer name"))
                        .collect();
                    if chain == ["declarations"] {
                        // Declaration emit is witnessed by its own suite (T7).
                        continue;
                    }
                    let mut result = None;
                    let served = checked.with_type_checker_for_file(
                        &CheckerRequest::default(),
                        source,
                        &mut |op| {
                            // A panic inside one chain is that chain's refusal.
                            let outcome = catch_unwind(AssertUnwindSafe(|| {
                                transform(&program, op, source, &chain)
                            }));
                            result = Some(outcome.unwrap_or_else(|payload| {
                                Err(format!("panicked: {}", panic_text(&*payload)))
                            }));
                            Ok(())
                        },
                    );
                    let result = match served {
                        Ok(()) => result.expect("the task ran"),
                        Err(error) => Err(format!("{error:?}")),
                    };
                    compared += 1;
                    let label = format!("{fixture}/{id} {name} {chain:?}");
                    match (expected(native), result) {
                        (Some(expected), Ok(actual)) if expected == actual => {}
                        (Some(expected), Ok(actual)) => failures.push(format!(
                            "{label}: different\n--- pinned\n{}\n--- ported\n{}",
                            String::from_utf8_lossy(&expected),
                            String::from_utf8_lossy(&actual)
                        )),
                        (Some(_), Err(error)) => failures.push(format!("{label}: failed: {error}")),
                        // The pin panicked: the port must refuse too.
                        (None, Err(_)) => {}
                        (None, Ok(actual)) => failures.push(format!(
                            "{label}: the pin panics ({}), the port printed\n{}",
                            native["panic"],
                            String::from_utf8_lossy(&actual)
                        )),
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {compared} probe chains differ:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
