//! Phase 3 C1 witness of the ported emit baseline writers
//! (`tools/phase3/harness/baselines.rs`, `scripts/phase3_baselines.py`): for
//! each row of a native emit capture taken with `--texts`, the writers compose
//! the `.js`, `.js.map` and `.sourcemap.txt` baselines from the pin's own
//! outputs, and the row records how they compare with the pin's baselines.
//!
//!     phase3_baselines REQUESTS.ndjson OUTPUT.ndjson
//!
//! A request line is `{"id", "configured_name", "subfolder", "mode",
//! "loading", "error_inputs", "native"}`: the native row and the first
//! compilation's loading request (`scripts/phase3_corpus.py`). The Rust
//! program of that request answers the writers' program queries; nothing
//! the Rust emitter computes is used. What the capture does not record is
//! reconstructed and checked here:
//!
//! - the harness's output ordering (`newCompilationResult`), recomputed from
//!   the native outputs and compared with the native order (`order`);
//! - the emit result's source maps (`EmitResult.SourceMaps`): one per emitted
//!   JavaScript or declaration file that carries a map, in `EmittedFiles`
//!   order, its raw map read back from the `.map` output or the inline data
//!   URL, its input names matched against the program's files through the
//!   generator's relative naming; the count must equal the native one;
//! - the `noCheck` repeat, which needs an emitter: its outputs are the
//!   original ones, so a native baseline with a repeat block is
//!   `unverifiable` and only its text before the block is compared;
//! - the declaration re-compilation, which runs on the Rust program loader
//!   and checker and renders through the ported error writer.
#[path = "../../../tools/phase3/harness/baselines.rs"]
mod baselines;
#[path = "../../../tools/phase3/harness/declaration_program.rs"]
mod declaration_program;
#[path = "../../../tools/s08/p5/errors.rs"]
#[allow(dead_code)]
mod errors;
#[path = "../../../tools/s08/p4/executor.rs"]
#[allow(dead_code)]
mod executor;
#[path = "../../../tools/s08/p5/paths.rs"]
mod paths;
#[path = "../../../tools/phase3/harness/program_view.rs"]
mod program_view;

use baselines::{
    Baseline, CompilationResult, DeclarationCompilationResult, Failure, JsEmitInput, OrderedFiles,
    ProgramView, RepeatOutputs, SourceMapEmitResult, SourcemapInput, SourcemapRecordInput,
    TestFile, NO_CONTENT,
};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Mutex, PoisonError};
use tsr_compiler::FileCache;
use tsr_jsstring::JsString;

static PANIC_LOCATION: Mutex<Option<String>> = Mutex::new(None);

fn last_panic() -> Option<String> {
    PANIC_LOCATION
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

type Owned = (Vec<u8>, Vec<u8>);

fn unhex(text: &Value) -> Result<Vec<u8>, String> {
    let text = text.as_str().ok_or("expected a hex string")?;
    if !text.len().is_multiple_of(2) {
        return Err("odd hex length".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn inputs(group: &Value) -> Result<Vec<Owned>, String> {
    group
        .as_array()
        .ok_or("expected an input group")?
        .iter()
        .map(|item| Ok((unhex(&item["name_hex"])?, unhex(&item["content_hex"])?)))
        .collect()
}

fn outputs(group: &Value) -> Result<Vec<Owned>, String> {
    group
        .as_array()
        .ok_or("expected an output group")?
        .iter()
        .map(|item| {
            let text = item
                .get("text_hex")
                .ok_or("the native capture was taken without --texts")?;
            Ok((unhex(&item["name_hex"])?, unhex(text)?))
        })
        .collect()
}

fn views(files: &[Owned]) -> Vec<TestFile<'_>> {
    files
        .iter()
        .map(|(name, content)| TestFile {
            unit_name: name,
            content,
        })
        .collect()
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Standard padded base64, as the generator's data URL encodes it.
fn base64_decode(text: &[u8]) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u32> {
        Some(u32::from(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        }))
    }
    if !text.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for chunk in text.chunks(4) {
        let pad = chunk.iter().rev().take_while(|&&b| b == b'=').count();
        let mut n = 0u32;
        for &byte in &chunk[..4 - pad] {
            n = (n << 6) | value(byte)?;
        }
        n <<= 6 * pad as u32;
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

/// The directory the generator relativizes sources against
/// (`emitter.getSourceMapDirectory`); `mapOptions` carries the program's
/// `sourceRoot` and `mapRoot` for JavaScript and declaration maps alike.
fn source_map_directory(
    facts: &dyn ProgramView,
    generated_file: &[u8],
) -> Result<Vec<u8>, Failure> {
    let options = facts.options();
    if !options.source_root.is_empty() {
        return facts.common_source_directory();
    }
    if !options.map_root.is_empty() {
        let mut directory = tsr_tspath::normalize_slashes(options.map_root.as_bytes()).into_owned();
        let common = facts.common_source_directory()?;
        if let Some(source) = emitting_source(facts, generated_file)? {
            directory = tsr_tspath::directory(
                &tsr_tsoptions::output_paths::get_source_file_path_in_new_dir(
                    &source,
                    &directory,
                    facts.current_directory(),
                    &common,
                    facts.use_case_sensitive_file_names(),
                ),
            );
        }
        if tsr_tspath::root_length(&directory) == 0 {
            directory = tsr_tspath::combine(&common, &[&directory]);
        }
        return Ok(directory);
    }
    Ok(tsr_tspath::directory(&tsr_tspath::normalize(
        generated_file,
    )))
}

/// The program file whose JavaScript or declaration output is `generated`.
fn emitting_source(facts: &dyn ProgramView, generated: &[u8]) -> Result<Option<Vec<u8>>, Failure> {
    for file in facts.source_files() {
        if tsr_tspath::is_declaration_file_name(file.file_name) {
            continue;
        }
        let js =
            tsr_tsoptions::output_paths::get_output_extension(file.file_name, facts.options().jsx);
        let dts = tsr_tspath::declaration_emit_extension_for_path(file.file_name);
        if baselines::output_path(facts, file.file_name, js)? == generated
            || baselines::output_path(facts, file.file_name, &dts)? == generated
        {
            return Ok(Some(file.file_name.to_vec()));
        }
    }
    Ok(None)
}

/// The generator's raw sources, recovered from the map's relative ones.
fn input_source_file_names(
    facts: &dyn ProgramView,
    raw: &tsr_sourcemap::RawSourceMap,
    generated: &[u8],
) -> Result<Vec<JsString>, String> {
    let directory = source_map_directory(facts, generated).map_err(|e| e.to_string())?;
    let files = facts.source_files();
    raw.sources
        .iter()
        .map(|source| {
            let found: Vec<&[u8]> = files
                .iter()
                .map(|file| file.file_name)
                .filter(|name| {
                    tsr_tspath::relative_to_directory_or_url(
                        &directory,
                        name,
                        true,
                        facts.current_directory(),
                        facts.use_case_sensitive_file_names(),
                    ) == source.as_bytes()
                })
                .collect();
            match found.as_slice() {
                [name] => Ok(JsString::from_bytes(*name)),
                [] => Err(format!(
                    "no program file is the map source {} of {}",
                    lossy(source.as_bytes()),
                    lossy(generated)
                )),
                _ => Err(format!(
                    "map source {} is ambiguous",
                    lossy(source.as_bytes())
                )),
            }
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// A default library file, whose text no fixture carries.
fn library_file(name: &[u8]) -> bool {
    name.starts_with(b"bundled:///libs/") || name.starts_with(b"/.lib/")
}

/// What the writers read from the program and the emit result, for the
/// writers' fixtures (`scripts/phase3_baselines.py fixtures`): the program
/// view's answers (library texts omitted) and the reconstructed source maps.
fn facts_dump(
    facts: &dyn ProgramView,
    source_maps: Option<&[SourceMapEmitResult]>,
    map_texts: &[Vec<u8>],
) -> Value {
    let files: Vec<Value> = facts
        .source_files()
        .iter()
        .map(|file| {
            let mut item = json!({"file_name_hex":hex(file.file_name),"path_hex":hex(file.path),
                "content_mapper_hex":hex(file.content_mapper)});
            if library_file(file.file_name) {
                item["library"] = json!(true);
            } else {
                item["text_hex"] = json!(hex(file.text));
                if file.original_text != file.text {
                    item["original_text_hex"] = json!(hex(file.original_text));
                }
            }
            item
        })
        .collect();
    let common = match facts.common_source_directory() {
        Ok(directory) => json!({"ok_hex":hex(&directory)}),
        Err(failure) => json!({"error":failure.to_string()}),
    };
    let maps = source_maps.map(|maps| {
        maps.iter()
            .zip(map_texts)
            .map(|(map, text)| {
                json!({"generated_file_hex":hex(&map.generated_file),"map_text_hex":hex(text),
                    "input_source_file_names_hex":map.input_source_file_names.iter()
                        .map(|name| hex(name.as_bytes())).collect::<Vec<_>>()})
            })
            .collect::<Vec<_>>()
    });
    json!({"source_files":files,"current_directory_hex":hex(facts.current_directory()),
        "use_case_sensitive_file_names":facts.use_case_sensitive_file_names(),
        "common_source_directory":common,
        "content_mapper_extensions_hex":facts.content_mapper_extensions().iter()
            .map(|extension| hex(extension.as_bytes())).collect::<Vec<_>>(),
        "source_maps":maps})
}

/// One reconstructed `EmitResult.SourceMaps` entry and the map's JSON text.
type MapWithText = (SourceMapEmitResult, Vec<u8>);

/// `EmitResult.SourceMaps` reconstructed from `EmittedFiles` and the outputs.
fn source_maps(
    native: &Value,
    facts: &dyn ProgramView,
    js: &OrderedFiles<'_>,
    maps: &OrderedFiles<'_>,
) -> Result<Option<Vec<MapWithText>>, String> {
    let emit = &native["emit"];
    if emit["state"] != "executed" {
        return Ok(None);
    }
    let options = facts.options();
    let mut result = Vec::new();
    for name in emit["emitted_files_hex"]
        .as_array()
        .ok_or("emitted files missing")?
    {
        let name = unhex(name)?;
        let map_text = if tsr_tspath::is_declaration_file_name(&name) {
            if !options.declaration_map.is_true() {
                continue;
            }
            let map_name = [name.as_slice(), b".map"].concat();
            maps.get(&map_name)
                .ok_or_else(|| format!("no map output {}", lossy(&map_name)))?
                .content
                .to_vec()
        } else if tsr_tspath::has_js_file_extension(&name) {
            if options.inline_source_map.is_true() {
                let file = js.get(&name).ok_or("an emitted file is not an output")?;
                let starts = tsr_jsstring::line_map::compute_ecma_line_starts(file.content);
                let info = tsr_sourcemap::create_ecma_line_info(
                    JsString::from_bytes(file.content),
                    starts,
                );
                let url = tsr_sourcemap::try_get_source_mapping_url(Some(&info));
                let data = url
                    .strip_prefix(b"data:application/json;base64,")
                    .ok_or("an inline source map without a data URL")?;
                base64_decode(data).ok_or("an inline source map that is not base64")?
            } else if options.source_map.is_true() {
                let map_name = [name.as_slice(), b".map"].concat();
                maps.get(&map_name)
                    .ok_or_else(|| format!("no map output {}", lossy(&map_name)))?
                    .content
                    .to_vec()
            } else {
                continue;
            }
        } else {
            continue;
        };
        let mut raw = tsr_sourcemap::RawSourceMap::default();
        tsr_json::unmarshal(&map_text, &mut raw, tsr_json::Options::default())
            .map_err(|error| format!("source map JSON: {error}"))?;
        let names = input_source_file_names(facts, &raw, &name)?;
        result.push((
            SourceMapEmitResult {
                input_source_file_names: names,
                source_map: raw,
                generated_file: name,
            },
            map_text,
        ));
    }
    if Some(result.len() as u64) != emit["source_maps"].as_u64() {
        return Err(format!(
            "{} source maps reconstructed, the emit result has {}",
            result.len(),
            emit["source_maps"]
        ));
    }
    Ok(Some(result))
}

/// The witness's JSON-output error renderer: no corpus row re-parses a JSON
/// output with errors (every such row has diagnostics), so this branch is
/// reported, not rendered.
struct NoJsonErrorRenderer;
impl baselines::JsonErrorBaseline for NoJsonErrorRenderer {
    fn render(
        &self,
        file: &TestFile<'_>,
        _parsed: &tsr_ast::ParsedFile,
        _diagnostics: &[tsr_ast::Diagnostic],
    ) -> Result<Vec<u8>, Failure> {
        Err(Failure::Input(format!(
            "the witness renders no JSON output error baseline ({})",
            lossy(file.unit_name)
        )))
    }
}

/// What a writer composed for one domain.
enum Composed {
    NotBaselined,
    NoContent { name: String },
    Content { name: String, text: Vec<u8> },
    Failed { reason: String },
}

fn composed(subfolder: &str, result: Result<Option<Baseline>, Failure>) -> Composed {
    match result {
        Ok(None) => Composed::NotBaselined,
        Ok(Some(baseline)) => {
            let name = format!("{subfolder}/{}", lossy(&baseline.path));
            if baseline.actual == NO_CONTENT {
                Composed::NoContent { name }
            } else {
                Composed::Content {
                    name,
                    text: baseline.actual,
                }
            }
        }
        Err(failure) => Composed::Failed {
            reason: failure.to_string(),
        },
    }
}

/// The first differing line of two texts, for the report's buckets.
fn first_difference(native: &[u8], rust: &[u8]) -> Value {
    let native_lines: Vec<&[u8]> = native.split(|&b| b == b'\n').collect();
    let rust_lines: Vec<&[u8]> = rust.split(|&b| b == b'\n').collect();
    let line = native_lines
        .iter()
        .zip(&rust_lines)
        .position(|(a, b)| a != b)
        .unwrap_or(native_lines.len().min(rust_lines.len()));
    let excerpt = |lines: &[&[u8]]| {
        lines
            .get(line)
            .map(|text| lossy(&text[..text.len().min(240)]))
    };
    json!({"line":line + 1,"native":excerpt(&native_lines),"rust":excerpt(&rust_lines),
        "native_lines":native_lines.len(),"rust_lines":rust_lines.len()})
}

const REPEAT_BLOCK: &[u8] = b"\r\n\r\n!!!! File ";

/// What the writer composes for a native `.js` baseline that holds `noCheck`
/// repeat blocks, when the repeat's outputs equal the original ones.
enum WithoutRepeat<'t> {
    /// Nothing precedes the first block: `<no content>`.
    NoContent,
    /// The text before the first block.
    Content(&'t [u8]),
}

/// `None` when the native baseline holds no repeat block. Every block starts
/// with [`REPEAT_BLOCK`], and the text before the first one is the sources
/// block, `\r\n\r\n` and the JavaScript composed before the repeat
/// (`tsCode + "\r\n\r\n" + jsCode`); with no such JavaScript that text is the
/// sources block and the separator alone.
fn without_repeat<'t>(native: &'t [u8], ts_code: &[u8]) -> Option<WithoutRepeat<'t>> {
    let at = native
        .windows(REPEAT_BLOCK.len())
        .position(|window| window == REPEAT_BLOCK)?;
    let prefix = &native[..at];
    if prefix.len() == ts_code.len() + 4
        && prefix.starts_with(ts_code)
        && prefix.ends_with(b"\r\n\r\n")
    {
        return Some(WithoutRepeat::NoContent);
    }
    Some(WithoutRepeat::Content(prefix))
}

/// One domain's outcome against the native item. `ts_code` is the sources
/// block of the `.js` baseline, for the output domain.
fn compare(native: &Value, rust: Composed, ts_code: Option<&[u8]>) -> Value {
    let native_state = native["state"].as_str().unwrap_or("missing");
    if native_state == "disabled" {
        return json!({"outcome":"disabled","reason":native["reason"]});
    }
    let (rust_state, rust_name, rust_text) = match rust {
        Composed::NotBaselined => ("not_baselined", None, None),
        Composed::NoContent { name } => ("no_content", Some(name), None),
        Composed::Content { name, text } => ("content", Some(name), Some(text)),
        Composed::Failed { reason } => {
            return json!({"outcome":"failed","native_state":native_state,"reason":reason});
        }
    };
    let Ok(native_text) = native.get("text_hex").map(unhex).transpose() else {
        return json!({"outcome":"failed","reason":"malformed native text"});
    };
    let native_name = native["name"].as_str();
    if let (Some(text), Some(ts_code)) = (&native_text, ts_code) {
        if let Some(expected) = without_repeat(text, ts_code) {
            // The repeat's outputs need an emitter; the witness supplies the
            // original ones, so only the composition without the blocks is
            // checked.
            let without_repeat_matches = rust_name.as_deref() == native_name
                && match expected {
                    WithoutRepeat::NoContent => rust_state == "no_content",
                    WithoutRepeat::Content(prefix) => {
                        rust_state == "content" && rust_text.as_deref() == Some(prefix)
                    }
                };
            let mut detail = json!({"outcome":"unverifiable","reason":"the native baseline holds a noCheck repeat block",
                "without_repeat_matches":without_repeat_matches,"rust_state":rust_state,"rust_name":rust_name});
            if let (WithoutRepeat::Content(prefix), Some(rust_text)) = (expected, &rust_text) {
                detail["first_difference"] = first_difference(prefix, rust_text);
            }
            return detail;
        }
    }
    if native_state == rust_state && native_name == rust_name.as_deref() && native_text == rust_text
    {
        return json!({"outcome":"match","state":native_state});
    }
    let mut detail = json!({"outcome":"different","native_state":native_state,"rust_state":rust_state,
        "native_name":native_name,"rust_name":rust_name});
    if let (Some(native_text), Some(rust_text)) = (&native_text, &rust_text) {
        detail["first_difference"] = first_difference(native_text, rust_text);
    }
    detail
}

fn observe(request: &Value, cache: &mut FileCache) -> Result<Value, String> {
    let native = &request["native"];
    let id = &request["id"];
    if native["state"] != "executed" {
        return Ok(json!({"id":id,"state":"unexecuted","native_state":native["state"]}));
    }
    let subfolder = request["subfolder"].as_str().ok_or("subfolder missing")?;
    let configured_name = request["configured_name"]
        .as_str()
        .ok_or("configured name missing")?
        .as_bytes();
    let baseline_inputs = &native["baseline_inputs"];
    let header = unhex(&baseline_inputs["header_hex"])?;
    let ts_config_files = inputs(&baseline_inputs["ts_config_files"])?;
    let to_be_compiled = inputs(&baseline_inputs["to_be_compiled"])?;
    let other_files = inputs(&baseline_inputs["other_files"])?;
    let js = outputs(&native["outputs"]["js"])?;
    let dts = outputs(&native["outputs"]["dts"])?;
    let maps = outputs(&native["outputs"]["maps"])?;
    let options = tsr_tsoptions::raw::compiler_options(&native["options"])
        .map_err(|error| format!("native options: {error:?}"))?;
    let harness = &native["harness_options"];
    let full_emit_paths = harness["FullEmitPaths"] == true;
    let harness_cwd = harness["CurrentDirectory"]
        .as_str()
        .ok_or("harness current directory missing")?
        .as_bytes();
    let diagnostics = native["diagnostics"]
        .as_u64()
        .ok_or("diagnostic count missing")? as usize;

    // The first compilation's Rust program answers the program queries.
    let scope = executor::content_mapper_scope(request, tsr_contentmappertest::new_spawner())
        .map_err(|failure| format!("content mapper scope: {failure}"))?;
    let checked = executor::load_fresh_checked(request, cache)
        .map_err(|failure| format!("program load: {failure}"))?;
    let facts =
        program_view::ProgramFacts::new(checked.program().clone()).map_err(|e| e.to_string())?;

    let (js_views, dts_views, map_views) = (views(&js), views(&dts), views(&maps));
    let native_js = OrderedFiles::from_ordered(js_views.clone()).map_err(|e| e.to_string())?;
    let native_dts = OrderedFiles::from_ordered(dts_views.clone()).map_err(|e| e.to_string())?;
    let native_maps = OrderedFiles::from_ordered(map_views.clone()).map_err(|e| e.to_string())?;
    let maps_result = source_maps(native, &facts, &native_js, &native_maps);
    let mut row = json!({"id":id,"state":"executed"});
    let (source_map_results, map_texts) = match maps_result {
        Ok(value) => {
            row["source_maps"] =
                json!({"state":"reconstructed","count":value.as_ref().map(Vec::len)});
            match value {
                Some(value) => {
                    let (results, texts): (Vec<_>, Vec<_>) = value.into_iter().unzip();
                    (Some(results), texts)
                }
                None => (None, Vec::new()),
            }
        }
        Err(reason) => {
            row["source_maps"] = json!({"state":"failed","reason":reason});
            (None, Vec::new())
        }
    };
    if request["dump_facts"] == true {
        row["facts"] = facts_dump(&facts, source_map_results.as_deref(), &map_texts);
    }
    let recorded: Vec<TestFile<'_>> = js_views
        .iter()
        .chain(&dts_views)
        .chain(&map_views)
        .copied()
        .collect();
    let mut result =
        baselines::new_compilation_result(&facts, &recorded, diagnostics, source_map_results)
            .map_err(|e| format!("newCompilationResult: {e}"))?;
    let names = |files: &OrderedFiles<'_>| -> Vec<String> {
        files.files().iter().map(|f| lossy(f.unit_name)).collect()
    };
    row["order"] = if result.js == native_js
        && result.dts == native_dts
        && result.maps == native_maps
    {
        json!({"state":"match"})
    } else {
        json!({"state":"different","js":names(&result.js),"dts":names(&result.dts),"maps":names(&result.maps),
            "native_js":names(&native_js),"native_dts":names(&native_dts),"native_maps":names(&native_maps)})
    };
    // The writers read the pin's own order.
    result.js = native_js;
    result.dts = native_dts;
    result.maps = native_maps;
    let result: CompilationResult<'_> = result;

    let (ts_config_views, to_be_compiled_views, other_views) = (
        views(&ts_config_files),
        views(&to_be_compiled),
        views(&other_files),
    );

    // output
    let output = if native["output"]["state"] == "disabled" {
        json!({"outcome":"disabled","reason":native["output"]["reason"]})
    } else {
        let mut declaration_row = json!({"state":"none"});
        let declaration = match baselines::prepare_declaration_compilation_context(
            &to_be_compiled_views,
            &other_views,
            &result,
            &options,
            harness_cwd,
        ) {
            Ok(Some(context)) => {
                let compiled =
                    compile_declarations(request, harness, &context, &ts_config_views, cache);
                match compiled {
                    Ok(compiled) => {
                        declaration_row = json!({"state":"compiled","inputs":context.decl_input_files.len(),
                            "others":context.decl_other_files.len(),"diagnostics":compiled.diagnostics});
                        if request["dump_facts"] == true {
                            declaration_row["error_baseline_hex"] =
                                json!(hex(&compiled.error_baseline));
                        }
                        Some(compiled)
                    }
                    Err(failure) => {
                        declaration_row = json!({"state":"failed","reason":failure.to_string()});
                        None
                    }
                }
            }
            // No re-compilation; or a failed assertion, which the writer
            // reports where the pin stops.
            Ok(None) | Err(_) => None,
        };
        let repeat = RepeatOutputs {
            js: result.js.clone(),
            dts: result.dts.clone(),
        };
        let composed_output = if declaration_row["state"] == "failed" {
            Err(Failure::Input(format!(
                "declaration re-compilation: {}",
                declaration_row["reason"]
            )))
        } else {
            baselines::js_emit_baseline(&JsEmitInput {
                configured_name,
                header: &header,
                options: &options,
                full_emit_paths,
                harness_current_directory: harness_cwd,
                to_be_compiled: &to_be_compiled_views,
                other_files: &other_views,
                result: &result,
                declaration: declaration.as_ref(),
                no_check_repeat: Some(&repeat),
                json_errors: &NoJsonErrorRenderer,
            })
            .map(Some)
        };
        row["declaration"] = declaration_row;
        let ts_code = baselines::ts_code(&header, &other_views, &to_be_compiled_views);
        compare(
            &native["output"],
            composed(subfolder, composed_output),
            Some(&ts_code),
        )
    };
    row["output"] = output;

    let sourcemap = baselines::sourcemap_baseline(&SourcemapInput {
        configured_name,
        options: &options,
        full_emit_paths,
        result: &result,
    });
    row["sourcemap"] = compare(&native["sourcemap"], composed(subfolder, sourcemap), None);

    let record = if row["source_maps"]["state"] == "failed" {
        Err(Failure::Input(format!(
            "source maps not reconstructed: {}",
            row["source_maps"]["reason"]
        )))
    } else {
        baselines::sourcemap_record_baseline(&SourcemapRecordInput {
            configured_name,
            options: &options,
            result: &result,
        })
        .map(Some)
    };
    row["sourcemap_record"] = compare(
        &native["sourcemap_record"],
        composed(subfolder, record),
        None,
    );
    drop(scope);
    Ok(row)
}

/// `compileDeclarationFiles` on the Rust program loader and checker, its
/// diagnostics rendered by the ported error writer.
fn compile_declarations<'a>(
    request: &Value,
    harness: &Value,
    context: &baselines::DeclarationCompilationContext<'a>,
    ts_config_files: &[TestFile<'a>],
    cache: &mut FileCache,
) -> Result<DeclarationCompilationResult<'a>, Failure> {
    let loading = declaration_program::loading_request(&request["loading"], context, harness)?;
    let compiled = declaration_program::compile(
        request,
        &loading,
        harness["CaptureSuggestions"] == true,
        executor::content_mapper_project(),
        cache,
    )?;
    let mut result = DeclarationCompilationResult {
        decl_input_files: context.decl_input_files.clone(),
        decl_other_files: context.decl_other_files.clone(),
        diagnostics: compiled.diagnostics.len(),
        error_baseline: Vec::new(),
    };
    if !compiled.diagnostics.is_empty() {
        let files = baselines::dts_file_error_inputs(ts_config_files, &result);
        let inputs: Vec<errors::InputFile<'_>> = files
            .iter()
            .map(|file| errors::InputFile {
                name: file.unit_name,
                content: file.content,
            })
            .collect();
        let rendered = errors::render(
            compiled.checked.program(),
            &inputs,
            &compiled.diagnostics,
            false,
        )
        .map_err(|error| Failure::Input(format!("error baseline: {error}")))?;
        result.error_baseline = unhex(&rendered["text_hex"]).map_err(Failure::Input)?;
    }
    Ok(result)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::panic::set_hook(Box::new(|info| {
        *PANIC_LOCATION
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = info
            .location()
            .map(|at| format!("{}:{}", at.file(), at.line()));
        eprintln!("{info}");
    }));
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 {
        return Err("usage: phase3_baselines REQUESTS.ndjson OUTPUT.ndjson".into());
    }
    let input = std::io::BufReader::new(std::fs::File::open(&args[1])?);
    let mut output = std::io::BufWriter::new(std::fs::File::create(&args[2])?);
    let mut cache = FileCache::new();
    for line in input.lines() {
        let request: Value = serde_json::from_str(&line?)?;
        let row = match catch_unwind(AssertUnwindSafe(|| observe(&request, &mut cache))) {
            Ok(Ok(row)) => row,
            Ok(Err(reason)) => {
                json!({"id":request["id"],"state":"failed","class":"witness","reason":reason})
            }
            Err(payload) => {
                let reason = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("non-string panic payload");
                json!({"id":request["id"],"state":"failed","class":"panic","reason":reason,"location":last_panic()})
            }
        };
        serde_json::to_writer(&mut output, &row)?;
        output.write_all(b"\n")?;
    }
    output.flush()?;
    Ok(())
}
