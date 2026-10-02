//! The native declaration-emit probes (`tools/phase3/probe`, recorded by
//! `scripts/phase3_probe.py`): each `["declarations"]` chain of a fixture under
//! `tests/fixtures/phase3/transforms/` holds the declaration text the pin
//! printed for one file (its declaration transformers and the declaration
//! printer options, as `emitDeclarationFile` runs them) and the transform's
//! diagnostics. This suite loads the same programs, runs the ported
//! declaration transform with the checker operation as its resolver, prints
//! with the same options and requires the same text and diagnostics.
//!
//! `PHASE3_PROBE=<fixture>` and `PHASE3_PROBE_CASE=<id>` narrow a run during
//! development.
#[path = "../../../tools/s08/p4/executor.rs"]
#[allow(dead_code)]
mod executor;

use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use tsr_arena::NodeId;
use tsr_ast::AstBuilder;
use tsr_checker::{CheckerRequest, Operation};
use tsr_compiler::{CheckedProgram, FileCache, Program, ProgramDeclarationHost};
use tsr_core::NewLineKind;
use tsr_jsstring::{JsString, SourceText};
use tsr_printer::{EmitContext, EmitTextWriter, Printer, PrinterOptions, TextWriter};
use tsr_transformers::declarations::{
    transform_declarations, DeclarationOptions, SupplementalReferencesTransformer,
};

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

/// The declaration printer options of `emitDeclarationFile`, with no
/// declaration map.
fn printer_options(program: &Program) -> PrinterOptions {
    let options = program.options();
    PrinterOptions {
        remove_comments: options.remove_comments.is_true(),
        new_line: options.new_line,
        no_emit_helpers: true,
        target: options.emit_script_target(),
        only_print_js_doc_style: true,
        omit_brace_source_map_positions: true,
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

/// One file's declaration emit: the printed text and the transform's
/// diagnostics as (code, pos, end, message).
type Emitted = (Vec<u8>, Vec<(i64, i64, i64, String)>);

fn declarations(
    program: &Program,
    op: &mut Operation<'_>,
    source: NodeId,
) -> Result<Emitted, String> {
    let counters = tsr_arena::Counters::new();
    let mut context = EmitContext::new();
    let mut output =
        AstBuilder::with_hooks(SourceText::default(), &counters, context.factory_hooks());
    // The transform may read any file its resolver answers with.
    for file in program.files() {
        output.retain_completed(file.bound());
    }
    // emitDeclarationFile's declaration path, with declaration emit forced as
    // the probe computes it.
    let host = ProgramDeclarationHost::new(program);
    let declaration_file_path = host
        .declaration_file_path(source)
        .map_err(|error| format!("{error:?}"))?;
    let options = DeclarationOptions {
        isolated_declarations: program.options().isolated_declarations.is_true(),
        strip_internal: program.options().strip_internal.is_true(),
        declaration_file_path: JsString::from_bytes(declaration_file_path.clone()),
    };
    // runDeclarationTransformers: the declaration transformer, then the
    // supplemental references, each one's diagnostics in turn.
    let supplemental = SupplementalReferencesTransformer::new(
        &host,
        &output,
        source,
        JsString::from_bytes(declaration_file_path),
        false,
    )
    .map_err(|error| format!("{error:?}"))?;
    let transformed =
        transform_declarations(op, &host, &mut output, &mut context, source, &options)
            .map_err(|error| format!("{error:?}"))?;
    let root = supplemental
        .transform_source_file(&mut output, transformed.root)
        .map_err(|error| format!("{error:?}"))?;
    let mut all_diagnostics = transformed.diagnostics;
    all_diagnostics.extend(supplemental.get_diagnostics());
    let mut diagnostics = Vec::new();
    for diagnostic in &all_diagnostics {
        let view = diagnostic.file.and_then(|file| {
            program
                .files()
                .iter()
                .find(|candidate| candidate.source() == file)
                .map(|candidate| candidate.bound().view().ast())
        });
        let message = diagnostic
            .string(view)
            .map_err(|error| format!("{error:?}"))?;
        diagnostics.push((
            i64::from(diagnostic.code),
            diagnostic.loc.pos(),
            diagnostic.loc.end(),
            String::from_utf8_lossy(&message).into_owned(),
        ));
    }
    let mut writer = TextWriter::new(new_line(program.options().new_line), 0);
    let bound = program
        .files()
        .iter()
        .find(|file| file.source() == source)
        .expect("the file is the program's")
        .bound()
        .view();
    let mut printer = Printer::new(printer_options(program), &context);
    printer.bindings = Some(&bound);
    printer
        .write(output.view(), root, Some(root), &mut writer, None)
        .map_err(|error| format!("{error:?}"))?;
    Ok((writer.text().to_vec(), diagnostics))
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

fn expected_text(chain: &Value) -> Option<Vec<u8>> {
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

fn expected_diagnostics(chain: &Value) -> Vec<(i64, i64, i64, String)> {
    chain["diagnostics"]
        .as_array()
        .map(|diagnostics| {
            diagnostics
                .iter()
                .map(|d| {
                    (
                        d["code"].as_i64().expect("code"),
                        d["pos"].as_i64().expect("pos"),
                        d["end"].as_i64().expect("end"),
                        d["message"].as_str().expect("message").to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn ported_declaration_emit_prints_what_the_pin_prints() {
    let only_case = std::env::var("PHASE3_PROBE_CASE").ok();
    let mut failures = Vec::new();
    let mut compared = 0usize;
    for (fixture, document) in fixtures() {
        for case in document["cases"].as_array().expect("cases") {
            let id = case["id"].as_str().expect("case id");
            if only_case.as_deref().is_some_and(|only| only != id) {
                continue;
            }
            let has_declarations = case["files"].as_array().expect("files").iter().any(|file| {
                file["chains"]
                    .as_array()
                    .expect("chains")
                    .iter()
                    .any(|chain| chain["chain"] == json!(["declarations"]))
            });
            if !has_declarations {
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
                    if native["chain"] != json!(["declarations"]) {
                        continue;
                    }
                    let mut result = None;
                    let served = checked.with_type_checker_for_file(
                        &CheckerRequest::default(),
                        source,
                        &mut |op| {
                            // A panic inside the transform is its refusal.
                            let outcome = catch_unwind(AssertUnwindSafe(|| {
                                declarations(&program, op, source)
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
                    let label = format!("{fixture}/{id} {name}");
                    match (expected_text(native), result) {
                        (Some(expected), Ok((actual, diagnostics))) => {
                            if expected != actual {
                                failures.push(format!(
                                    "{label}: different\n--- pinned\n{}\n--- ported\n{}",
                                    String::from_utf8_lossy(&expected),
                                    String::from_utf8_lossy(&actual)
                                ));
                            }
                            let pinned = expected_diagnostics(native);
                            if pinned != diagnostics {
                                failures.push(format!(
                                    "{label}: diagnostics differ\n--- pinned\n{pinned:#?}\n--- ported\n{diagnostics:#?}"
                                ));
                            }
                        }
                        (Some(_), Err(error)) => failures.push(format!("{label}: failed: {error}")),
                        // The pin panicked: the port must refuse too.
                        (None, Err(_)) => {}
                        (None, Ok((actual, _))) => failures.push(format!(
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
        "{} of {compared} declaration chains differ:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
