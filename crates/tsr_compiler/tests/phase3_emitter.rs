//! The emitter witness (docs/PHASE3-plan.md, T8): corpus rows whose emit the
//! pin's harness observed (`tests/fixtures/phase3/emitter/emitter.json`,
//! frozen by its `regenerate.py` from a `phase3_native.py capture --texts`).
//! Each row's program is loaded as the harness loads its post-emit program
//! and emitted with an in-memory write callback, which records what the
//! harness's `OutputRecorderFS` records. A row compares `EmitSkipped`,
//! `EmittedFiles` (in order), the emit diagnostics, the number of source maps
//! and every written file by name and digest.
//!
//! - `CheckedProgram::emit` must reproduce every row;
//! - the rows marked `seam` are also emitted through the emitter with a
//!   caller-supplied chain, the transformers of the emitter's chain that were
//!   ported first, in its order (the others leave these rows' inputs
//!   unchanged: scripts with no enum, namespace, module syntax, parameter
//!   property, class field or decorator), and must match in both
//!   test-program modes;
//! - three declaration rows emit their JavaScript half through the same chain
//!   with `EmitOnly::Js`, which runs `emitJSFile` as `EmitAll` does.
//!
//! The write-failure, `EmitOnly::Dts` and trace tests are contracts read from
//! the pinned `emitter.go` and `Program.Emit`, not native observations.
#[path = "../../../tools/s08/p4/executor.rs"]
#[allow(dead_code)]
mod executor;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::Mutex;
use tsr_arena::Counters;
use tsr_arena::NodeId;
use tsr_checker::CheckerRequest;
use tsr_checker::{MemoryTraceSink, TraceSink};
use tsr_compiler::{
    emitter, CheckedProgram, EmitOnly, EmitOptions, EmitResult, FileCache, Program, WriteFileData,
};
use tsr_core::{LanguageVariant, ScriptTarget};
use tsr_transformers::{TransformOptions, Transformer};

fn fixture() -> Value {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/phase3/emitter/emitter.json");
    serde_json::from_slice(&std::fs::read(path).expect("emitter fixture")).expect("fixture JSON")
}

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}

fn hex(raw: &[u8]) -> String {
    use std::fmt::Write;
    raw.iter().fold(String::new(), |mut text, byte| {
        write!(text, "{byte:02x}").expect("writing to a string");
        text
    })
}

fn sha256(raw: &[u8]) -> String {
    hex(&Sha256::digest(raw))
}

/// The downlevel transformers of `estransforms.GetESTransformer`'s chain for
/// `target` that the port has, in the chain's order.
fn ported_es_components(target: ScriptTarget) -> &'static [&'static str] {
    match target {
        ScriptTarget::ESNEXT
        | ScriptTarget::ES2025
        | ScriptTarget::ES2024
        | ScriptTarget::ES2023
        | ScriptTarget::ES2022
        | ScriptTarget::ES2021 => &[],
        ScriptTarget::ES2020 => &["logicalassignment"],
        ScriptTarget::ES2019 => &["logicalassignment", "nullishcoalescing", "optionalchain"],
        ScriptTarget::ES2018 => &[
            "logicalassignment",
            "nullishcoalescing",
            "optionalchain",
            "optionalcatch",
        ],
        ScriptTarget::ES2017 => &[
            "logicalassignment",
            "nullishcoalescing",
            "optionalchain",
            "optionalcatch",
            "forawait",
            "taggedtemplate",
        ],
        ScriptTarget::ES2016 => &[
            "logicalassignment",
            "nullishcoalescing",
            "optionalchain",
            "optionalcatch",
            "forawait",
            "taggedtemplate",
            "async",
        ],
        _ => &[
            "logicalassignment",
            "nullishcoalescing",
            "optionalchain",
            "optionalcatch",
            "forawait",
            "taggedtemplate",
            "async",
            "exponentiation",
        ],
    }
}

/// `getScriptTransformers` without the transformers the port does not have.
fn ported_script_transformers<'t>(
    program: &Program,
    opts: &TransformOptions<'t>,
    source_file: NodeId,
) -> Result<Vec<Transformer<'t>>, tsr_arena::Error> {
    let options = &opts.compiler_options;
    let file = program
        .files()
        .iter()
        .find(|file| file.source() == source_file)
        .ok_or(tsr_arena::Error::WrongOwner)?;
    let view = file.bound().view();
    let node = view.ast().node(source_file)?;
    let in_js_file = tsr_ast::utilities::is_in_js_file(Some(&node));
    let language_variant = view.source_file()?.language_variant;
    let mut names = Vec::new();
    if options.emit_decorator_metadata.is_true() {
        names.push("metadata");
    }
    names.push("typeeraser");
    if !options.verbatim_module_syntax.is_true() && !in_js_file {
        names.push("importelision");
    }
    if options.jsx_transform_enabled() && language_variant == LanguageVariant::JSX {
        names.push("jsx");
    }
    names.extend(ported_es_components(options.emit_script_target()));
    names.push("usestrict");
    if !options.isolated_modules() {
        names.push("constenum");
    }
    Ok(names
        .into_iter()
        .filter_map(|name| emitter::transformer_by_name(name, opts).expect("a known transformer"))
        .collect())
}

/// What one emit produced: its result or failure, and the files written.
struct Observed {
    result: Result<Option<EmitResult>, String>,
    written: Vec<(Vec<u8>, Vec<u8>)>,
}

/// How a test emits a row: the program mode, the chain, the `EmitOnly`
/// mode, and the written names the write callback refuses.
#[derive(Clone, Copy)]
struct Setup<'s> {
    mode: &'s str,
    seam: bool,
    emit_only: EmitOnly,
    refuse: &'s [&'s [u8]],
    tracing: Option<&'s std::sync::Arc<MemoryTraceSink>>,
}

impl Setup<'_> {
    fn new(mode: &str, seam: bool) -> Setup<'_> {
        Setup {
            mode,
            seam,
            emit_only: EmitOnly::All,
            refuse: &[],
            tracing: None,
        }
    }
}

fn emit(row: &Value, mode: &str, seam: bool) -> Result<Observed, String> {
    emit_with(row, Setup::new(mode, seam))
}

fn emit_with(row: &Value, setup: Setup<'_>) -> Result<Observed, String> {
    let mut request = row["request"].clone();
    request["mode"] = json!(setup.mode);
    let mut checked = executor::load_fresh_checked(&request, &mut FileCache::new())
        .map_err(|failure| format!("the program did not load: {failure}"))?;
    if let Some(sink) = setup.tracing {
        let sink: std::sync::Arc<dyn TraceSink> = sink.clone();
        checked = CheckedProgram::new(checked.program().clone(), &Counters::new(), Some(sink));
    }
    let written = Mutex::new(Vec::new());
    let write_file = |name: &[u8], text: &[u8], _: &mut WriteFileData| {
        if setup.refuse.contains(&name) {
            return Err(tsr_vfs::Error::Io(std::io::ErrorKind::PermissionDenied));
        }
        written
            .lock()
            .expect("recorder")
            .push((name.to_vec(), text.to_vec()));
        Ok(())
    };
    let options = EmitOptions {
        write_file: Some(&write_file),
        emit_only: setup.emit_only,
        ..EmitOptions::default()
    };
    let request = CheckerRequest::default();
    let result = catch_unwind(AssertUnwindSafe(|| {
        if setup.seam {
            checked.emit_with_script_transformers(&request, &options, &ported_script_transformers)
        } else {
            checked.emit(&request, &options)
        }
    }));
    let result = match result {
        Ok(result) => result.map_err(|error| match error {
            tsr_compiler::Error::Unsupported(name) => format!("unsupported: {name}"),
            error => format!("{error:?}"),
        }),
        Err(payload) => Err(format!(
            "panicked: {}",
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned()))
                .unwrap_or_default()
        )),
    };
    Ok(Observed {
        result,
        written: written.into_inner().expect("recorder"),
    })
}

fn row(id: &str) -> Value {
    fixture()["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("no fixture row {id}"))
        .clone()
}

/// The harness's output groups of `name` (`newCompilationResult`).
fn group(name: &[u8]) -> Option<&'static str> {
    if tsr_tspath::has_js_file_extension(name) || tsr_tspath::has_json_file_extension(name) {
        Some("js")
    } else if tsr_tspath::is_declaration_file_name(name) {
        Some("dts")
    } else if name.ends_with(b".map") {
        Some("maps")
    } else {
        None
    }
}

/// The differences between a row's native emit and `result` with `written`.
fn differences(row: &Value, result: &EmitResult, written: &[(Vec<u8>, Vec<u8>)]) -> Vec<String> {
    let mut found = Vec::new();
    if row["emit_skipped"] != result.emit_skipped {
        found.push(format!(
            "EmitSkipped: pinned {}, ported {}",
            row["emit_skipped"], result.emit_skipped
        ));
    }
    let pinned: Vec<Vec<u8>> = row["emitted_files_hex"]
        .as_array()
        .expect("emitted files")
        .iter()
        .map(|name| bytes(name.as_str().expect("name")))
        .collect();
    let ported: Vec<Vec<u8>> = result
        .emitted_files
        .iter()
        .map(|name| name.as_bytes().to_vec())
        .collect();
    if pinned != ported {
        found.push(format!(
            "EmittedFiles: pinned {:?}, ported {:?}",
            pinned
                .iter()
                .map(|n| String::from_utf8_lossy(n))
                .collect::<Vec<_>>(),
            ported
                .iter()
                .map(|n| String::from_utf8_lossy(n))
                .collect::<Vec<_>>()
        ));
    }
    let diagnostics: Vec<Value> = result
        .diagnostics
        .iter()
        .map(|diagnostic| {
            json!([
                diagnostic.code,
                diagnostic
                    .message_args
                    .iter()
                    .map(|arg| hex(arg.as_bytes()))
                    .collect::<Vec<_>>()
            ])
        })
        .collect();
    if row["diagnostics"] != json!(diagnostics) {
        found.push(format!(
            "diagnostics: pinned {}, ported {}",
            row["diagnostics"],
            json!(diagnostics)
        ));
    }
    if row["source_maps"] != result.source_maps.len() {
        found.push(format!(
            "SourceMaps: pinned {}, ported {}",
            row["source_maps"],
            result.source_maps.len()
        ));
    }
    for kind in ["js", "dts", "maps"] {
        let mut pinned: Vec<(Vec<u8>, String, Vec<u8>)> = row["outputs"][kind]
            .as_array()
            .expect("outputs")
            .iter()
            .map(|file| {
                (
                    bytes(file["name_hex"].as_str().expect("name")),
                    file["sha256"].as_str().expect("digest").to_owned(),
                    bytes(file["text_hex"].as_str().expect("text")),
                )
            })
            .collect();
        let mut ported: Vec<(Vec<u8>, String, Vec<u8>)> = written
            .iter()
            .filter(|(name, _)| group(name) == Some(kind))
            .map(|(name, text)| (name.clone(), sha256(text), text.clone()))
            .collect();
        pinned.sort();
        ported.sort();
        let names = |files: &[(Vec<u8>, String, Vec<u8>)]| {
            files
                .iter()
                .map(|(name, _, _)| String::from_utf8_lossy(name).into_owned())
                .collect::<Vec<_>>()
        };
        if names(&pinned) != names(&ported) {
            found.push(format!(
                "{kind} files: pinned {:?}, ported {:?}",
                names(&pinned),
                names(&ported)
            ));
            continue;
        }
        for (pinned, ported) in pinned.iter().zip(&ported) {
            if pinned.1 != ported.1 {
                found.push(format!(
                    "{}: different\n--- pinned\n{}\n--- ported\n{}",
                    String::from_utf8_lossy(&pinned.0),
                    String::from_utf8_lossy(&pinned.2),
                    String::from_utf8_lossy(&ported.2)
                ));
            }
        }
    }
    found
}

/// `CheckedProgram::emit` on every row.
#[test]
fn program_emit_matches_the_pin() {
    let document = fixture();
    let mut failures = Vec::new();
    let rows = document["rows"].as_array().expect("rows");
    for row in rows {
        let id = row["id"].as_str().expect("id");
        let observed = match emit(row, "single", false) {
            Ok(observed) => observed,
            Err(error) => {
                failures.push(format!("{id}: {error}"));
                continue;
            }
        };
        match &observed.result {
            Ok(Some(result)) => {
                let found = differences(row, result, &observed.written);
                if !found.is_empty() {
                    failures.push(format!("{id}:\n{}", found.join("\n")));
                }
            }
            Ok(None) => failures.push(format!("{id}: no emit result")),
            Err(error) => failures.push(format!("{id}: failed: {error}")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} rows differ:\n\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n\n")
    );
    assert!(rows.len() >= 57, "only {} rows in the fixture", rows.len());
}

/// The emitter with the ported transformers on the `seam` rows, in both
/// test-program modes.
#[test]
fn emitter_with_the_ported_transformers_writes_what_the_pin_writes() {
    let document = fixture();
    let only = std::env::var("PHASE3_EMITTER_CASE").ok();
    let mut failures = Vec::new();
    let mut compared = 0usize;
    for row in document["rows"].as_array().expect("rows") {
        let id = row["id"].as_str().expect("id");
        if row["seam"] != true || only.as_deref().is_some_and(|only| only != id) {
            continue;
        }
        for mode in ["single", "concurrent"] {
            compared += 1;
            let observed = match emit(row, mode, true) {
                Ok(observed) => observed,
                Err(error) => {
                    failures.push(format!("{id} ({mode}): {error}"));
                    continue;
                }
            };
            match &observed.result {
                Ok(Some(result)) => {
                    let found = differences(row, result, &observed.written);
                    if !found.is_empty() {
                        failures.push(format!("{id} ({mode}):\n{}", found.join("\n")));
                    }
                }
                Ok(None) => failures.push(format!("{id} ({mode}): no emit result")),
                Err(error) => failures.push(format!("{id} ({mode}): failed: {error}")),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {compared} emits differ:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    assert!(compared >= 60, "only {compared} seam emits compared");
}

/// The JavaScript half of a row's native emit: its `js` and `maps` files and
/// its emitted files other than declarations, against `result` with
/// `written`. `EmitOnly::Js` runs `emitJSFile` exactly as `EmitAll` does and
/// skips `emitDeclarationFile`, so these are the pin's for that mode.
fn javascript_differences(
    row: &Value,
    result: &EmitResult,
    written: &[(Vec<u8>, Vec<u8>)],
) -> Vec<String> {
    let mut javascript_row = row.clone();
    let emitted: Vec<Value> = row["emitted_files_hex"]
        .as_array()
        .expect("emitted files")
        .iter()
        .filter(|name| !tsr_tspath::is_declaration_file_name(&bytes(name.as_str().expect("name"))))
        .cloned()
        .collect();
    javascript_row["emitted_files_hex"] = json!(emitted);
    javascript_row["outputs"]["dts"] = json!([]);
    // The declaration half is what blocks or skips these rows' emit.
    javascript_row["emit_skipped"] = json!(false);
    differences(&javascript_row, result, written)
}

/// Declaration rows with a JavaScript output the ported transformers produce
/// (`emitBOM` with a source map and CRLF, accessors with a source map, a
/// blocked declaration output beside a written script): `EmitOnly::Js`
/// writes the pin's JavaScript and map files in both modes.
#[test]
fn emit_only_js_writes_the_javascript_of_declaration_rows() {
    let mut failures = Vec::new();
    for id in [
        "compiler/emitBOM.ts#configuration=0",
        "compiler/properties.ts#configuration=0",
        "compiler/declarationFileOverwriteError.ts#configuration=0",
    ] {
        let row = row(id);
        for mode in ["single", "concurrent"] {
            let observed = emit_with(
                &row,
                Setup {
                    emit_only: EmitOnly::Js,
                    ..Setup::new(mode, true)
                },
            )
            .expect("the program loads");
            match &observed.result {
                Ok(Some(result)) => {
                    let found = javascript_differences(&row, result, &observed.written);
                    if !found.is_empty() {
                        failures.push(format!("{id} ({mode}):\n{}", found.join("\n")));
                    }
                }
                other => failures.push(format!("{id} ({mode}): {other:?}")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// `EmitOnly::Dts` on a row without declaration outputs writes nothing and
/// skips nothing: `emitJSFile` returns for the mode and
/// `emitDeclarationFile` for the empty declaration path.
#[test]
fn emit_only_dts_without_declarations_writes_nothing() {
    let row = row("compiler/sourceMap-Comment1.ts#configuration=0");
    let observed = emit_with(
        &row,
        Setup {
            emit_only: EmitOnly::Dts,
            ..Setup::new("single", true)
        },
    )
    .expect("the program loads");
    let result = observed.result.expect("emit").expect("a result");
    assert!(!result.emit_skipped);
    assert!(result.emitted_files.is_empty());
    assert!(result.source_maps.is_empty());
    assert!(result.diagnostics.is_empty());
    assert!(observed.written.is_empty());
}

/// `printSourceFile`'s write failures: a refused map write reports
/// `Could_not_write_file_0_Colon_1` with the JavaScript path (the pin passes
/// `jsFilePath` there too) and leaves the map out of `EmittedFiles`; the
/// script is still written with its `sourceMappingURL`. A refused script
/// write reports the same diagnostic and leaves the script out.
#[test]
fn write_failures_become_could_not_write_file_diagnostics() {
    const JS: &[u8] = b"/.src/sourceMap-Comment1.js";
    const MAP: &[u8] = b"/.src/sourceMap-Comment1.js.map";
    let row = row("compiler/sourceMap-Comment1.ts#configuration=0");
    let refused = tsr_vfs::Error::Io(std::io::ErrorKind::PermissionDenied).to_string();
    let could_not_write = |result: &EmitResult| -> Vec<(i32, Vec<Vec<u8>>)> {
        result
            .diagnostics
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.code,
                    diagnostic
                        .message_args
                        .iter()
                        .map(|arg| arg.as_bytes().to_vec())
                        .collect(),
                )
            })
            .collect()
    };
    let expected = vec![(5033, vec![JS.to_vec(), refused.as_bytes().to_vec()])];

    let observed = emit_with(
        &row,
        Setup {
            refuse: &[MAP],
            ..Setup::new("single", true)
        },
    )
    .expect("the program loads");
    let result = observed.result.expect("emit").expect("a result");
    assert_eq!(could_not_write(&result), expected);
    assert_eq!(
        result.emitted_files,
        vec![tsr_jsstring::JsString::from_bytes(JS)]
    );
    assert_eq!(result.source_maps.len(), 1);
    assert!(!result.emit_skipped);
    let names: Vec<&[u8]> = observed.written.iter().map(|(name, _)| &name[..]).collect();
    assert_eq!(names, vec![JS]);
    assert!(observed.written[0]
        .1
        .ends_with(b"//# sourceMappingURL=sourceMap-Comment1.js.map"));

    let observed = emit_with(
        &row,
        Setup {
            refuse: &[JS],
            ..Setup::new("single", true)
        },
    )
    .expect("the program loads");
    let result = observed.result.expect("emit").expect("a result");
    assert_eq!(could_not_write(&result), expected);
    assert_eq!(
        result.emitted_files,
        vec![tsr_jsstring::JsString::from_bytes(MAP)]
    );
    let names: Vec<&[u8]> = observed.written.iter().map(|(name, _)| &name[..]).collect();
    assert_eq!(names, vec![MAP]);
}

/// The emit's trace events (`tracing.PhaseEmit`): `Program.Emit`'s `emit`,
/// the file's `emit` with its path, `emitJsFileOrBundle` with the script's
/// path, each with separate begin and end events, and the sampled
/// `transformNodes` inside them, which a deterministic session drops.
#[test]
fn emit_pushes_its_trace_events() {
    let row = row("compiler/sourceMap-Comment1.ts#configuration=0");
    let trace = |deterministic: bool| {
        let sink = std::sync::Arc::new(MemoryTraceSink::new(deterministic));
        let observed = emit_with(
            &row,
            Setup {
                tracing: Some(&sink),
                ..Setup::new("single", true)
            },
        )
        .expect("the program loads");
        observed.result.expect("emit").expect("a result");
        sink.events()
            .into_iter()
            .filter(|event| event.phase == tsr_checker::TracePhase::Emit)
            .map(|event| {
                let args: Vec<String> = event
                    .args
                    .iter()
                    .map(|(key, value)| format!("{key}={value:?}"))
                    .collect();
                format!("{} {} {}", event.ph, event.name, args.join(","))
            })
            .collect::<Vec<_>>()
    };
    let path = r#"path=Str("/.src/sourceMap-Comment1.ts")"#;
    let js = r#"jsFilePath=Str("/.src/sourceMap-Comment1.js")"#;
    let begin = [
        "B emit ".to_owned(),
        format!("B emit {path}"),
        format!("B emitJsFileOrBundle {js}"),
    ];
    let end = [
        format!("E emitJsFileOrBundle {js}"),
        format!("E emit {path}"),
        "E emit ".to_owned(),
    ];
    let deterministic: Vec<String> = begin.iter().chain(&end).cloned().collect();
    assert_eq!(trace(true), deterministic);
    let sampled: Vec<String> = begin
        .iter()
        .cloned()
        .chain([format!("X transformNodes {path}")])
        .chain(end.iter().cloned())
        .collect();
    assert_eq!(trace(false), sampled);
}
