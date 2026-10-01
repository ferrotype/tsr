//! The corpus row's emit (docs/PHASE3-plan.md T8): what the pin's compiler
//! runner computes for its `output`, `sourcemap` and `sourcemap record`
//! sub-tests, on the Rust program loader, checker and emitter.
//!
//! - The first compilation (`harnessutil.compileFilesWithHost`): the
//!   pre-emit program's diagnostics, then a fresh post-emit program that
//!   emits (`Program.Emit` with an in-memory write callback recording what
//!   the harness's `OutputRecorderFS` records) before its diagnostics are
//!   collected, and the count-mismatch rule. Both programs are the harness's
//!   `createProgram`'s: with `incremental`, the incremental program, whose
//!   emit ends with the build info.
//! - `newCompilationResult` over the recorded outputs, and the three writers
//!   of `baselines.rs` over it.
//! - The `.js` baseline's two compilations: the declaration re-compilation
//!   (`compileDeclarationFiles`, its `DtsFileErrors` rendered by the ported
//!   error writer) and the `noCheck` repeat (`result.Repeat`: the same
//!   inputs, as the runner's test files hold them after the first
//!   compilation, compiled again with `noCheck` and emitted).
//!
//! Each compilation opens its own content-mapper scope, as each
//! `CompileFilesEx` opens and closes its own host. A production refusal
//! (`Error::Unsupported`) is a named failure of class `unsupported`; a
//! writer's assertion or runtime fault is the pin's failing sub-test on the
//! Rust outputs (`assertion`, `runtime`); a harness defect is class `harness`.
use super::baselines::{
    self, Baseline, CompilationResult, DeclarationCompilationContext, DeclarationCompilationResult,
    Failure, JsEmitInput, OrderedFiles, RepeatOutputs, SourcemapInput, SourcemapRecordInput,
    TestFile, NO_CONTENT,
};
use super::declaration_program::{self, create_program, CompileError, HarnessProgram};
use super::program_view::ProgramFacts;
use super::{errors, executor, reprint};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use tsr_checker::CheckerRequest;
use tsr_compiler::{CheckedProgram, EmitOptions, EmitResult, FileCache, Program, WriteFileData};

/// A recorded output: its real path and text.
pub type Output = (Vec<u8>, Vec<u8>);

/// The harness's `OutputRecorderFS`: every write in written order, one entry
/// per real path; a later write of the same path replaces the text in place.
/// The written file exists afterwards, so on a case-insensitive file system
/// a second spelling of it is the first one.
// source: tsc/internal/testutil/harnessutil/recorderfs.go:OutputRecorderFS.WriteFile
struct Recorder<'p> {
    host: &'p dyn tsr_vfs::FileSystem,
    outputs: Mutex<Recorded>,
}

/// The outputs in written order and each real path's position.
#[derive(Default)]
struct Recorded {
    outputs: Vec<Output>,
    index: HashMap<Vec<u8>, usize>,
}

impl<'p> Recorder<'p> {
    fn new(host: &'p dyn tsr_vfs::FileSystem) -> Self {
        Self {
            host,
            outputs: Mutex::new(Recorded::default()),
        }
    }

    fn write(&self, name: &[u8], text: &[u8]) {
        let real = self
            .host
            .realpath(name)
            .map_or_else(|_| name.to_vec(), |path| path.as_bytes().to_vec());
        let key = tsr_tspath::to_path(&real, b"", self.host.use_case_sensitive_file_names());
        let mut guard = self
            .outputs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let recorded = &mut *guard;
        if let Some(&at) = recorded.index.get(key.as_bytes()) {
            recorded.outputs[at].1 = text.to_vec();
        } else {
            recorded
                .index
                .insert(key.as_bytes().to_vec(), recorded.outputs.len());
            recorded.outputs.push((real, text.to_vec()));
        }
    }

    fn into_outputs(self) -> Vec<Output> {
        self.outputs
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .outputs
    }
}

/// One `Program.Emit(ctx, EmitOptions{})` with the recorder as the file
/// system's `WriteFile`; the incremental program writes its build info
/// through it as well.
pub struct Emitted {
    /// `None` is a nil emit result.
    pub result: Option<EmitResult>,
    pub recorded: Vec<Output>,
}

fn emit(created: &HarnessProgram) -> Result<Emitted, tsr_compiler::Error> {
    let program = created.program();
    let recorder = Recorder::new(program.host());
    let write_file = |name: &[u8], text: &[u8], _: &mut WriteFileData| {
        recorder.write(name, text);
        Ok(())
    };
    let options = EmitOptions {
        write_file: Some(&write_file),
        ..EmitOptions::default()
    };
    let result = created
        .program_like()
        .emit(&CheckerRequest::default(), &options)?;
    Ok(Emitted {
        result,
        recorded: recorder.into_outputs(),
    })
}

/// A failure object of the row: `{"state":"failed","class","reason"}`.
fn failed(class: &str, reason: impl std::fmt::Display) -> Value {
    executor::failure(reason, class)
}

fn writer_failure(failure: &Failure) -> Value {
    match failure {
        Failure::Assertion(message) => failed("assertion", message),
        Failure::Runtime(message) => failed("runtime", message),
        Failure::Input(message) => failed("harness", message),
    }
}

/// A panic as a failure: its message and the location the panic hook saw
/// (`null` when it saw none).
pub fn panic_failure(payload: &(dyn std::any::Any + Send), location: Option<&str>) -> Value {
    let reason = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic payload");
    json!({"state":"failed","class":"panic","reason":reason,"location":location})
}

/// `body` with a panic recorded as a failure with its location.
pub fn guarded(last_panic: &dyn Fn() -> Option<String>, body: impl FnOnce() -> Value) -> Value {
    catch_unwind(AssertUnwindSafe(body))
        .unwrap_or_else(|payload| panic_failure(payload.as_ref(), last_panic().as_deref()))
}

/// The diagnostic as the native oracle's `phase3Diagnostics` records it.
fn diagnostic(program: &Program, d: &tsr_ast::Diagnostic) -> Value {
    let file = d.file.map(|id| {
        let config = program.config();
        if let Some(config) = config
            .config_file
            .iter()
            .chain(&config.config_dependencies)
            .find(|c| c.root == id)
        {
            return config.file.view().source_file(config.root).map_or_else(
                |_| "?".to_owned(),
                |source| reprint::hex(source.parse_options().file_name.as_bytes()),
            );
        }
        program
            .files()
            .iter()
            .find(|f| f.source() == id)
            .and_then(|f| f.bound().view().source_file().ok())
            .map_or_else(
                || "?".to_owned(),
                |source| reprint::hex(source.parse_options().file_name.as_bytes()),
            )
    });
    json!({"file_hex":file,"pos":d.loc.pos(),"end":d.loc.end(),"code":d.code,"category":d.category,
        "key_hex":reprint::hex(d.message_key.as_bytes()),"text_hex":reprint::hex(d.message_text.as_bytes()),
        "args_hex":d.message_args.iter().map(|a| reprint::hex(a.as_bytes())).collect::<Vec<_>>(),
        "chain":d.message_chain.iter().map(|c| diagnostic(program, c)).collect::<Vec<_>>(),
        "related":d.related_information.iter().map(|r| diagnostic(program, r)).collect::<Vec<_>>()})
}

fn files_json(files: &OrderedFiles<'_>, texts: bool) -> Vec<Value> {
    files
        .files()
        .iter()
        .map(|file| {
            let mut entry = json!({"name_hex":reprint::hex(file.unit_name),
                "sha256":reprint::sha256(file.content),"bytes":file.content.len()});
            if texts {
                entry["text_hex"] = json!(reprint::hex(file.content));
            }
            entry
        })
        .collect()
}

/// One `compileFilesWithHost`: the post-emit program, what it emitted, and
/// the harness's diagnostic counts.
pub struct Compiled {
    pub post: HarnessProgram,
    pub emitted: Emitted,
    pub pre_diagnostics: usize,
    pub post_diagnostics: usize,
    /// `len(result.Diagnostics)`: the post-emit count, or the shorter count
    /// and the mismatch diagnostic when the two differ.
    pub diagnostics: usize,
}

impl Compiled {
    /// The row's record of one compilation's counts, in the order the pin's
    /// row runs its compilations (`name`: first, declaration, repeat).
    pub fn counts(&self, name: &str) -> Value {
        json!({"compilation":name,"pre_diagnostics":self.pre_diagnostics,
            "post_diagnostics":self.post_diagnostics})
    }
}

/// `compileFilesWithHost` over the request's program: the pre-emit program's
/// diagnostics (`pre`, or a fresh load), then a fresh post-emit program that
/// emits before its diagnostics are collected.
// source: tsc/internal/testutil/harnessutil/harnessutil.go:compileFilesWithHost
pub fn compile_files(
    request: &Value,
    pre: Option<Arc<CheckedProgram>>,
    capture_suggestions: bool,
    cache: &mut FileCache,
) -> Result<Compiled, Value> {
    let pre = match pre {
        Some(pre) => pre,
        None => Arc::new(executor::load_fresh_checked(request, cache)?),
    };
    let pre = create_program(pre).map_err(executor::compiler_failure)?;
    let pre_diagnostics =
        declaration_program::harness_diagnostics(pre.program_like(), capture_suggestions)
            .map_err(executor::compiler_failure)?
            .len();
    let post = create_program(Arc::new(executor::load_fresh_checked(request, cache)?))
        .map_err(executor::compiler_failure)?;
    let emitted = emit(&post).map_err(executor::compiler_failure)?;
    let post_diagnostics =
        declaration_program::harness_diagnostics(post.program_like(), capture_suggestions)
            .map_err(executor::compiler_failure)?
            .len();
    let diagnostics = if pre_diagnostics == post_diagnostics {
        post_diagnostics
    } else {
        pre_diagnostics.min(post_diagnostics) + 1
    };
    Ok(Compiled {
        post,
        emitted,
        pre_diagnostics,
        post_diagnostics,
        diagnostics,
    })
}

fn views(outputs: &[Output]) -> Vec<TestFile<'_>> {
    outputs
        .iter()
        .map(|(name, content)| TestFile {
            unit_name: name,
            content,
        })
        .collect()
}

fn source_maps(result: Option<&EmitResult>) -> Option<Vec<baselines::SourceMapEmitResult>> {
    result.map(|result| {
        result
            .source_maps
            .iter()
            .map(|map| baselines::SourceMapEmitResult {
                input_source_file_names: map.input_source_file_names.clone(),
                source_map: map.source_map.clone(),
                generated_file: map.generated_file.as_bytes().to_vec(),
            })
            .collect()
    })
}

/// The row's `emit` observation: the emit result as the native row records
/// it, the counts, and the harness's outputs in `newCompilationResult` order.
pub fn emit_json(first: &Compiled, result: &CompilationResult<'_>, texts: bool) -> Value {
    let program = first.post.program();
    let emit_result = first.emitted.result.as_ref().map(|emit| {
        json!({"emit_skipped":emit.emit_skipped,
            "emitted_files_hex":emit.emitted_files.iter().map(|name| reprint::hex(name.as_bytes())).collect::<Vec<_>>(),
            "diagnostics":emit.diagnostics.iter().map(|d| diagnostic(program, d)).collect::<Vec<_>>(),
            "source_maps":emit.source_maps.len()})
    });
    json!({"state":"executed","result":emit_result,"pre_diagnostics":first.pre_diagnostics,
        "post_diagnostics":first.post_diagnostics,"diagnostics":first.diagnostics,
        "outputs":{"js":files_json(&result.js, texts),"dts":files_json(&result.dts, texts),
            "maps":files_json(&result.maps, texts)}})
}

/// The request's writer inputs, decoded: the native baseline inputs (the
/// header and the runner's three file groups), the configured name, the
/// suite and the harness options the writers read.
pub struct RequestInputs {
    configured_name: String,
    subfolder: String,
    header: Vec<u8>,
    full_emit_paths: bool,
    harness_current_directory: String,
    capture_suggestions: bool,
    ts_config_files: Vec<Output>,
    to_be_compiled: Vec<Output>,
    other_files: Vec<Output>,
}

impl RequestInputs {
    pub fn parse(request: &Value) -> Result<Self, Value> {
        let harness = |reason: &str| failed("harness", reason);
        let text = |value: &Value, what: &str| -> Result<String, Value> {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| harness(&format!("request without {what}")))
        };
        let inputs = &request["baseline_inputs"];
        let group = |name: &str| -> Result<Vec<Output>, Value> {
            inputs[name]
                .as_array()
                .ok_or_else(|| harness(&format!("baseline inputs without {name}")))?
                .iter()
                .map(|item| {
                    let name = unhex(item["name_hex"].as_str().unwrap_or("?"))
                        .map_err(|error| harness(&error))?;
                    let content = unhex(item["content_hex"].as_str().unwrap_or("?"))
                        .map_err(|error| harness(&error))?;
                    Ok((name, content))
                })
                .collect()
        };
        let options = &request["harness_options"];
        Ok(Self {
            configured_name: text(&request["configured_name"], "a configured name")?,
            subfolder: text(&request["suite"], "a suite")?,
            header: unhex(&text(&inputs["header_hex"], "a baseline header")?)
                .map_err(|error| harness(&error))?,
            full_emit_paths: options["FullEmitPaths"] == true,
            harness_current_directory: text(&options["CurrentDirectory"], "a harness directory")?,
            capture_suggestions: options["CaptureSuggestions"] == true,
            ts_config_files: group("ts_config_files")?,
            to_be_compiled: group("to_be_compiled")?,
            other_files: group("other_files")?,
        })
    }

    pub fn capture_suggestions(&self) -> bool {
        self.capture_suggestions
    }

    pub fn writer_inputs(&self) -> WriterInputs<'_> {
        WriterInputs {
            configured_name: self.configured_name.as_bytes(),
            subfolder: &self.subfolder,
            header: &self.header,
            full_emit_paths: self.full_emit_paths,
            harness_current_directory: self.harness_current_directory.as_bytes(),
            capture_suggestions: self.capture_suggestions,
            ts_config_files: views(&self.ts_config_files),
            to_be_compiled: views(&self.to_be_compiled),
            other_files: views(&self.other_files),
        }
    }
}

/// What the writers read besides the compilation result.
pub struct WriterInputs<'a> {
    pub configured_name: &'a [u8],
    pub subfolder: &'a str,
    pub header: &'a [u8],
    pub full_emit_paths: bool,
    pub harness_current_directory: &'a [u8],
    pub capture_suggestions: bool,
    pub ts_config_files: Vec<TestFile<'a>>,
    pub to_be_compiled: Vec<TestFile<'a>>,
    pub other_files: Vec<TestFile<'a>>,
}

/// A composed baseline as the row records it.
fn baseline_json(subfolder: &str, baseline: Option<Baseline>, texts: bool) -> Value {
    let Some(baseline) = baseline else {
        return json!({"state":"not_baselined"});
    };
    let name = format!("{subfolder}/{}", String::from_utf8_lossy(&baseline.path));
    if baseline.actual == NO_CONTENT {
        return json!({"state":"no_content","name":name});
    }
    let mut value = json!({"state":"content","name":name,"sha256":reprint::sha256(&baseline.actual),
        "bytes":baseline.actual.len()});
    if texts {
        value["text_hex"] = json!(reprint::hex(&baseline.actual));
    }
    value
}

fn composed(subfolder: &str, result: Result<Option<Baseline>, Failure>, texts: bool) -> Value {
    match result {
        Ok(baseline) => baseline_json(subfolder, baseline, texts),
        Err(failure) => writer_failure(&failure),
    }
}

/// The JSON-output branch of `DoJSEmitBaseline` (an emitted `.json` file
/// whose re-parse has diagnostics, with no program diagnostic) renders an
/// error baseline over a file outside any program, which the ported error
/// writer cannot address. No native row takes it; a Rust row that does is
/// a harness gap, never a silent difference.
struct JsonErrors;
impl baselines::JsonErrorBaseline for JsonErrors {
    fn render(
        &self,
        file: &TestFile<'_>,
        _parsed: &tsr_ast::ParsedFile,
        _diagnostics: &[tsr_ast::Diagnostic],
    ) -> Result<Vec<u8>, Failure> {
        Err(Failure::Input(format!(
            "the harness renders no error baseline for the JSON output {}",
            String::from_utf8_lossy(file.unit_name)
        )))
    }
}

fn compile_error(error: CompileError) -> Value {
    match error {
        CompileError::Harness(failure) => writer_failure(&failure),
        CompileError::Load(error) | CompileError::Diagnostics(error) => {
            executor::compiler_failure(error)
        }
    }
}

/// `compileDeclarationFiles` on the Rust program loader and checker, its
/// diagnostics rendered by the ported error writer. A declaration program
/// writes nothing its diagnostics depend on, so its pre- and post-emit
/// counts are the one collection's (`declaration_program`).
fn compile_declarations<'a>(
    request: &Value,
    inputs: &WriterInputs<'a>,
    context: &DeclarationCompilationContext<'a>,
    cache: &mut FileCache,
    compilations: &mut Vec<Value>,
) -> Result<DeclarationCompilationResult<'a>, Value> {
    let loading = declaration_program::loading_request(
        &request["loading"],
        context,
        &request["harness_options"],
    )
    .map_err(|failure| writer_failure(&failure))?;
    // CompileFilesEx opens its own content-mapper host for the configuration.
    let _scope = executor::content_mapper_scope(request, tsr_contentmappertest::new_spawner())?;
    let compiled = declaration_program::compile_checked(
        request,
        &loading,
        inputs.capture_suggestions,
        executor::content_mapper_project(),
        cache,
    )
    .map_err(compile_error)?;
    compilations.push(
        json!({"compilation":"declaration","pre_diagnostics":compiled.diagnostics.len(),
        "post_diagnostics":compiled.diagnostics.len()}),
    );
    let mut result = DeclarationCompilationResult {
        decl_input_files: context.decl_input_files.clone(),
        decl_other_files: context.decl_other_files.clone(),
        diagnostics: compiled.diagnostics.len(),
        error_baseline: Vec::new(),
    };
    if !compiled.diagnostics.is_empty() {
        let files = baselines::dts_file_error_inputs(&inputs.ts_config_files, &result);
        let rendered = errors::render(
            compiled.checked.program(),
            &files
                .iter()
                .map(|file| errors::InputFile {
                    name: file.unit_name,
                    content: file.content,
                })
                .collect::<Vec<_>>(),
            &compiled.diagnostics,
            false,
        )
        .map_err(|error| failed("error_baseline", error))?;
        result.error_baseline = match rendered["text_hex"].as_str() {
            Some(text) => unhex(text).map_err(|error| failed("harness", error))?,
            None => {
                return Err(failed(
                    "harness",
                    "the DtsFileErrors baseline rendered no text",
                ))
            }
        };
    }
    Ok(result)
}

fn unhex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err("odd hex length".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// The repeat's request: the first compilation's, with `noCheck` set and the
/// files holding the runner's test files as they are after the first
/// compilation (a content-mapped unit holds its mapped text).
// source: tsc/internal/testutil/harnessutil/harnessutil.go:CompileFilesEx (result.Repeat)
fn repeat_request(request: &Value) -> Result<Value, Value> {
    let mut repeat = request.clone();
    let loading = &mut repeat["loading"];
    loading["options"]["noCheck"] = json!(true);
    let cwd = loading["cwd"]
        .as_str()
        .ok_or_else(|| failed("harness", "loading request without a directory"))?
        .as_bytes()
        .to_vec();
    let inputs = &request["baseline_inputs"];
    for group in ["to_be_compiled", "other_files"] {
        for item in inputs[group].as_array().into_iter().flatten() {
            let name = unhex(item["name_hex"].as_str().unwrap_or_default())
                .map_err(|error| failed("harness", error))?;
            let name = String::from_utf8(tsr_tspath::absolute(&name, &cwd))
                .map_err(|_| failed("harness", "a test file name is not UTF-8"))?;
            loading["files"][name] = item["content_hex"].clone();
        }
    }
    Ok(repeat)
}

/// `result.Repeat({"noCheck": "true"})`'s JavaScript and declaration outputs
/// in `newCompilationResult` order, and its diagnostic counts.
fn no_check_repeat(
    request: &Value,
    capture_suggestions: bool,
    cache: &mut FileCache,
) -> Result<(Vec<Output>, Vec<Output>, Value), Value> {
    let repeat = repeat_request(request)?;
    let _scope = executor::content_mapper_scope(&repeat, tsr_contentmappertest::new_spawner())?;
    let compiled = compile_files(&repeat, None, capture_suggestions, cache)?;
    let facts =
        ProgramFacts::new(compiled.post.program().clone()).map_err(|f| writer_failure(&f))?;
    let recorded = views(&compiled.emitted.recorded);
    let result = baselines::new_compilation_result(&facts, &recorded, compiled.diagnostics, None)
        .map_err(|f| writer_failure(&f))?;
    let owned = |files: &OrderedFiles<'_>| -> Vec<Output> {
        files
            .files()
            .iter()
            .map(|file| (file.unit_name.to_vec(), file.content.to_vec()))
            .collect()
    };
    Ok((
        owned(&result.js),
        owned(&result.dts),
        compiled.counts("repeat"),
    ))
}

/// The `output` sub-test: `DoJSEmitBaseline`, running the declaration
/// re-compilation and the repeat only where the pinned writer reaches them;
/// each compilation it runs appends its counts to `compilations`.
pub fn output(
    request: &Value,
    inputs: &WriterInputs<'_>,
    result: &CompilationResult<'_>,
    cache: &mut FileCache,
    compilations: &mut Vec<Value>,
    texts: bool,
) -> Value {
    let options = result.program.options();
    let mut run = || -> Result<Result<Baseline, Failure>, Value> {
        let first_assertion = !options.no_emit.is_true()
            && !options.emit_declaration_only.is_true()
            && result.js.is_empty()
            && result.diagnostics == 0;
        let mut declaration = None;
        let mut repeat = None;
        if !first_assertion {
            let context = baselines::prepare_declaration_compilation_context(
                &inputs.to_be_compiled,
                &inputs.other_files,
                result,
                options,
                inputs.harness_current_directory,
            );
            // A failed assertion stops the pinned writer before both compilations.
            if let Ok(context) = context {
                if let Some(context) = context {
                    declaration = Some(compile_declarations(
                        request,
                        inputs,
                        &context,
                        cache,
                        compilations,
                    )?);
                }
                if !options.no_check.is_true() && !options.no_emit.is_true() {
                    let (js, dts, counts) =
                        no_check_repeat(request, inputs.capture_suggestions, cache)?;
                    compilations.push(counts);
                    repeat = Some((js, dts));
                }
            }
        }
        let repeat_outputs = repeat
            .as_ref()
            .map(|(js, dts)| -> Result<RepeatOutputs<'_>, Failure> {
                Ok(RepeatOutputs {
                    js: OrderedFiles::from_ordered(views(js))?,
                    dts: OrderedFiles::from_ordered(views(dts))?,
                })
            })
            .transpose();
        let repeat_outputs = match repeat_outputs {
            Ok(outputs) => outputs,
            Err(failure) => return Ok(Err(failure)),
        };
        Ok(baselines::js_emit_baseline(&JsEmitInput {
            configured_name: inputs.configured_name,
            header: inputs.header,
            options,
            full_emit_paths: inputs.full_emit_paths,
            harness_current_directory: inputs.harness_current_directory,
            to_be_compiled: &inputs.to_be_compiled,
            other_files: &inputs.other_files,
            result,
            declaration: declaration.as_ref(),
            no_check_repeat: repeat_outputs.as_ref(),
            json_errors: &JsonErrors,
        }))
    };
    match run() {
        Ok(composed_output) => composed(inputs.subfolder, composed_output.map(Some), texts),
        Err(failure) => failure,
    }
}

/// The `sourcemap` sub-test: `DoSourcemapBaseline`.
pub fn sourcemap(inputs: &WriterInputs<'_>, result: &CompilationResult<'_>, texts: bool) -> Value {
    composed(
        inputs.subfolder,
        baselines::sourcemap_baseline(&SourcemapInput {
            configured_name: inputs.configured_name,
            options: result.program.options(),
            full_emit_paths: inputs.full_emit_paths,
            result,
        }),
        texts,
    )
}

/// The `sourcemap record` sub-test: `DoSourcemapRecordBaseline`.
pub fn sourcemap_record(
    inputs: &WriterInputs<'_>,
    result: &CompilationResult<'_>,
    texts: bool,
) -> Value {
    composed(
        inputs.subfolder,
        baselines::sourcemap_record_baseline(&SourcemapRecordInput {
            configured_name: inputs.configured_name,
            options: result.program.options(),
            result,
        })
        .map(Some),
        texts,
    )
}

/// `newCompilationResult` over the first compilation's recorded outputs.
pub fn compilation_result<'a>(
    facts: &'a ProgramFacts,
    first: &'a Compiled,
    recorded: &'a [TestFile<'a>],
) -> Result<CompilationResult<'a>, Failure> {
    baselines::new_compilation_result(
        facts,
        recorded,
        first.diagnostics,
        source_maps(first.emitted.result.as_ref()),
    )
}

/// The recorded outputs as the writers' test files.
pub fn recorded(first: &Compiled) -> Vec<TestFile<'_>> {
    views(&first.emitted.recorded)
}
