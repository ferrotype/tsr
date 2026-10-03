//! The pinned compiler runner's emit baseline writers (docs/PHASE3-plan.md
//! T0): the `.js` baseline (`DoJSEmitBaseline`), the `.js.map` baseline
//! (`DoSourcemapBaseline`) and the `.sourcemap.txt` baseline
//! (`DoSourcemapRecordBaseline` over `CompilationResult.GetSourceMapRecord`),
//! with the harness's output ordering (`newCompilationResult`) they read.
//!
//! The writers read a compilation through plain inputs, so the harness can
//! hand them the Rust emitter's outputs and the witness the pin's own: the
//! emitted files in the harness's order, the program facts they query
//! ([`ProgramView`]), the source-map emit results and the option values. The
//! two compilations the runner starts from inside the writer, the declaration
//! re-compilation and the `noCheck` repeat, are the caller's
//! ([`DeclarationCompilationResult`], [`RepeatOutputs`]).
//!
//! Texts are bytes throughout. A `t.Fatal` or panic of the pinned writer is a
//! [`Failure`] with the pin's message, never baseline text.
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;
use tsr_tspath as path;

#[path = "patience.rs"]
pub mod patience;
#[path = "sourcemap_record.rs"]
pub mod sourcemap_record;

/// The pinned `baseline.NoContent`: what a writer hands `baseline.Run` when
/// there is nothing to baseline.
pub const NO_CONTENT: &[u8] = b"<no content>";

/// Go's `harnessutil.TestFile`: a unit or output name and its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TestFile<'a> {
    pub unit_name: &'a [u8],
    pub content: &'a [u8],
}

/// Why a writer composed no baseline: where the pinned sub-test stops.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The pinned writer's `t.Fatal` or explicit `panic`, with its message.
    Assertion(String),
    /// A runtime fault the pin hits on the same input (an index out of range,
    /// a nil dereference, a failed `json.Unmarshal`).
    Runtime(String),
    /// The caller's inputs do not describe one compilation consistently: a
    /// harness defect, never a compiler outcome.
    Input(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Assertion(message) => write!(f, "assertion: {message}"),
            Failure::Runtime(message) => write!(f, "runtime: {message}"),
            Failure::Input(message) => write!(f, "input: {message}"),
        }
    }
}

/// What a writer hands `baseline.Run`: the baseline path and the text, which
/// is [`NO_CONTENT`] when there is nothing to baseline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Baseline {
    pub path: Vec<u8>,
    pub actual: Vec<u8>,
}

/// One source file of the program, as the writers read it.
#[derive(Clone, Copy, Debug)]
pub struct SourceFileView<'p> {
    /// `SourceFile.FileName()`.
    pub file_name: &'p [u8],
    /// `SourceFile.Path()`: the file's identity in the program.
    pub path: &'p [u8],
    /// `SourceFile.Text()`.
    pub text: &'p [u8],
    /// `SourceFile.OriginalText()`: the text before content mapping.
    pub original_text: &'p [u8],
    /// `SourceFile.ContentMapper()`: empty when the file is not mapped.
    pub content_mapper: &'p [u8],
}

/// The program queries the writers make (`result.Program`, `result.Host`).
pub trait ProgramView {
    /// `Program.GetSourceFiles()`, in program order.
    fn source_files(&self) -> Vec<SourceFileView<'_>>;
    /// `Program.GetSourceFile(fileName)`; `None` is Go's nil.
    fn source_file(&self, file_name: &[u8]) -> Option<SourceFileView<'_>>;
    /// `Program.Options()`.
    fn options(&self) -> &CompilerOptions;
    /// `Program.CommonSourceDirectory()`.
    fn common_source_directory(&self) -> Result<Vec<u8>, Failure>;
    /// `Host.GetCurrentDirectory()`.
    fn current_directory(&self) -> &[u8];
    /// `Host.FS().UseCaseSensitiveFileNames()`.
    fn use_case_sensitive_file_names(&self) -> bool;
    /// `Program.ContentMapperExtensions()`, which
    /// `outputpaths.ChangeToDeclarationExtension` reads.
    fn content_mapper_extensions(&self) -> Vec<JsString>;
}

/// Go's `collections.OrderedMap[string, *TestFile]` of the harness's
/// outputs: insertion order, one entry per name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OrderedFiles<'a> {
    files: Vec<TestFile<'a>>,
}

impl<'a> OrderedFiles<'a> {
    /// Files already in the harness's order with distinct names.
    pub fn from_ordered(files: Vec<TestFile<'a>>) -> Result<Self, Failure> {
        let mut result = Self::default();
        for file in files {
            if result.get(file.unit_name).is_some() {
                return Err(Failure::Input(format!(
                    "output {} listed twice",
                    String::from_utf8_lossy(file.unit_name)
                )));
            }
            result.files.push(file);
        }
        Ok(result)
    }
    /// `OrderedMap.Set`: an existing name keeps its position.
    fn set(&mut self, file: TestFile<'a>) {
        if let Some(slot) = self
            .files
            .iter_mut()
            .find(|f| f.unit_name == file.unit_name)
        {
            *slot = file;
        } else {
            self.files.push(file);
        }
    }
    fn delete(&mut self, name: &[u8]) {
        self.files.retain(|f| f.unit_name != name);
    }
    /// `OrderedMap.GetOrZero`.
    pub fn get(&self, name: &[u8]) -> Option<&TestFile<'a>> {
        self.files.iter().find(|f| f.unit_name == name)
    }
    /// `OrderedMap.Values()` / `Entries()`, in order.
    pub fn files(&self) -> &[TestFile<'a>] {
        &self.files
    }
    /// `OrderedMap.Size()`.
    pub fn len(&self) -> usize {
        self.files.len()
    }
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// One entry of `EmitResult.SourceMaps`: what the source-map record reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceMapEmitResult {
    /// The generator's raw sources, 1:1 with `source_map.sources`.
    pub input_source_file_names: Vec<JsString>,
    pub source_map: tsr_sourcemap::RawSourceMap,
    pub generated_file: Vec<u8>,
}

/// The harness's `CompilationResult` as the writers read it.
pub struct CompilationResult<'a> {
    /// `len(result.Diagnostics)`.
    pub diagnostics: usize,
    /// `result.JS`, `result.DTS` and `result.Maps`.
    pub js: OrderedFiles<'a>,
    pub dts: OrderedFiles<'a>,
    pub maps: OrderedFiles<'a>,
    /// `result.Inputs()`: each program source file's name and `Text()`.
    pub inputs: Vec<TestFile<'a>>,
    /// `result.Outputs()`: the outputs paired with an input, in input order.
    pub outputs: Vec<TestFile<'a>>,
    /// `result.Result.SourceMaps`; `None` is a nil `result.Result`.
    pub source_maps: Option<Vec<SourceMapEmitResult>>,
    pub program: &'a dyn ProgramView,
}

impl CompilationResult<'_> {
    // source: tsc/internal/testutil/harnessutil/harnessutil.go:CompilationResult.GetNumberOfJSFiles
    pub fn number_of_js_files(&self, include_json: bool) -> usize {
        if include_json {
            return self.js.len();
        }
        self.js
            .files()
            .iter()
            .filter(|file| !path::file_extension_is(file.unit_name, path::EXTENSION_JSON))
            .count()
    }
}

/// The harness's output ordering: the recorder's outputs split into JS, DTS
/// and maps, each paired with its input in program order, the rest sorted by
/// name. `recorded` is the output recorder's list (`OutputRecorderFS.
/// Outputs()`): written order, one entry per real path.
// source: tsc/internal/testutil/harnessutil/harnessutil.go:newCompilationResult
pub fn new_compilation_result<'a>(
    program: &'a dyn ProgramView,
    recorded: &[TestFile<'a>],
    diagnostics: usize,
    source_maps: Option<Vec<SourceMapEmitResult>>,
) -> Result<CompilationResult<'a>, Failure> {
    let mut c = CompilationResult {
        diagnostics,
        js: OrderedFiles::default(),
        dts: OrderedFiles::default(),
        maps: OrderedFiles::default(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        source_maps,
        program,
    };
    let options = program.options();
    // Corsa, unlike Strada, can use multiple threads for emit. As a result, the order of outputs is non-deterministic.
    // To make the order deterministic, we sort the outputs by the order of the inputs.
    let (mut js, mut dts, mut maps) = (
        OrderedFiles::default(),
        OrderedFiles::default(),
        OrderedFiles::default(),
    );
    for &document in recorded {
        if path::has_js_file_extension(document.unit_name)
            || path::has_json_file_extension(document.unit_name)
        {
            js.set(document);
        } else if path::is_declaration_file_name(document.unit_name) {
            dts.set(document);
        } else if path::file_extension_is(document.unit_name, b".map") {
            maps.set(document);
        }
    }

    // using the order from the inputs, populate the outputs
    for source_file in program.source_files() {
        c.inputs.push(TestFile {
            unit_name: source_file.file_name,
            content: source_file.text,
        });
        if !path::is_declaration_file_name(source_file.file_name) {
            let extname = tsr_tsoptions::output_paths::get_output_extension(
                source_file.file_name,
                options.jsx,
            );
            let js_name = output_path(program, source_file.file_name, extname)?;
            let dts_name = output_path(
                program,
                source_file.file_name,
                &path::declaration_emit_extension_for_path(source_file.file_name),
            )?;
            let map_name = output_path(
                program,
                source_file.file_name,
                &[extname, b".map".as_slice()].concat(),
            )?;
            for (pending, collected, name) in [
                (&mut js, &mut c.js, js_name),
                (&mut dts, &mut c.dts, dts_name),
                (&mut maps, &mut c.maps, map_name),
            ] {
                if let Some(&output) = pending.get(&name) {
                    collected.set(output);
                    pending.delete(output.unit_name);
                    c.outputs.push(output);
                }
            }
        }
    }

    // add any unhandled outputs, ordered by unit name
    for (pending, collected) in [(js, &mut c.js), (dts, &mut c.dts), (maps, &mut c.maps)] {
        let mut rest = pending.files;
        rest.sort_by(|a, b| a.unit_name.cmp(b.unit_name));
        for document in rest {
            collected.set(document);
        }
    }
    Ok(c)
}

// source: tsc/internal/testutil/harnessutil/harnessutil.go:CompilationResult.getOutputPath
pub fn output_path(
    program: &dyn ProgramView,
    file_name: &[u8],
    ext: &[u8],
) -> Result<Vec<u8>, Failure> {
    let options = program.options();
    let cwd = program.current_directory();
    let mut file = path::resolve(cwd, &[file_name]);
    let declaration = ext == b".d.ts"
        || ext == b".d.mts"
        || ext == b".d.cts"
        || (ext.ends_with(b".ts") && ext.windows(3).any(|w| w == b".d."));
    let out_dir = if declaration && !options.declaration_dir.is_empty() {
        options.declaration_dir.as_bytes()
    } else {
        options.out_dir.as_bytes()
    };
    if !out_dir.is_empty() {
        let common = program.common_source_directory()?;
        if !common.is_empty() {
            file = path::relative_from_directory(
                &common,
                &file,
                cwd,
                program.use_case_sensitive_file_names(),
            );
            // The pin combines with OutDir here, not with the declaration directory.
            file = path::combine(&path::resolve(cwd, &[options.out_dir.as_bytes()]), &[&file]);
        }
    }
    if ext == path::declaration_emit_extension_for_path(&file).as_slice() {
        return Ok(tsr_tsoptions::output_paths::declaration_extension(
            &file,
            &program.content_mapper_extensions(),
        ));
    }
    Ok(path::change_extension(&file, ext))
}

/// Test-path prefixes replaced as the pin's `removeTestPathPrefixes` does
/// (one `strings.Replacer` pass; at one position the earliest-listed pair
/// that matches wins).
// source: tsc/internal/testutil/tsbaseline/util.go:removeTestPathPrefixes
pub fn remove_test_path_prefixes(
    text: &[u8],
    retain_trailing_directory_separator: bool,
) -> Vec<u8> {
    const PREFIXES: [&[u8]; 7] = [
        b"/.ts/",
        b"/.lib/",
        b"/.src/",
        b"bundled:///libs/",
        b"file:///./ts/",
        b"file:///./lib/",
        b"file:///./src/",
    ];
    const REPLACEMENTS: [&[u8]; 7] = [b"", b"", b"", b"", b"file:///", b"file:///", b"file:///"];
    const RETAINED: [&[u8]; 7] = [
        b"/",
        b"/",
        b"/",
        b"/",
        b"file:///",
        b"file:///",
        b"file:///",
    ];
    let replacements = if retain_trailing_directory_separator {
        RETAINED
    } else {
        REPLACEMENTS
    };
    let mut output = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if let Some(index) = PREFIXES
            .iter()
            .position(|prefix| text[i..].starts_with(prefix))
        {
            output.extend_from_slice(replacements[index]);
            i += PREFIXES[index].len();
        } else {
            output.push(text[i]);
            i += 1;
        }
    }
    output
}

// source: tsc/internal/testutil/tsbaseline/js_emit_baseline.go:fileOutput
pub fn file_output(file: &TestFile<'_>, full_emit_paths: bool) -> Vec<u8> {
    let file_name = if full_emit_paths {
        remove_test_path_prefixes(
            file.unit_name,
            false, /*retainTrailingDirectorySeparator*/
        )
    } else {
        path::base_name(file.unit_name).to_vec()
    };
    [b"//// [".as_slice(), &file_name, b"]\r\n", file.content].concat()
}

/// `tspath.FileExtensionIsOneOf(baselinePath, {".ts", ".tsx"})` then
/// `ChangeExtension(baselinePath, extension)`.
fn baseline_path(configured_name: &[u8], extension: &[u8]) -> Vec<u8> {
    if path::file_extension_is_one_of(configured_name, &[path::EXTENSION_TS, path::EXTENSION_TSX]) {
        path::change_extension(configured_name, extension)
    } else {
        configured_name.to_vec()
    }
}

/// The declaration re-compilation the `.js` baseline asks for: which files
/// become its inputs. The options, harness settings and configuration are
/// the first compilation's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclarationCompilationContext<'a> {
    pub decl_input_files: Vec<TestFile<'a>>,
    pub decl_other_files: Vec<TestFile<'a>>,
    /// The runner passes no directory, so this is the harness's.
    pub current_directory: &'a [u8],
}

/// What `compileDeclarationFiles` returned for the context
/// [`prepare_declaration_compilation_context`] computed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclarationCompilationResult<'a> {
    pub decl_input_files: Vec<TestFile<'a>>,
    pub decl_other_files: Vec<TestFile<'a>>,
    /// `len(declResult.Diagnostics)`.
    pub diagnostics: usize,
    /// When `diagnostics > 0`: `GetErrorBaseline` over
    /// [`dts_file_error_inputs`] and the declaration program's diagnostics,
    /// not pretty. Ignored otherwise.
    pub error_baseline: Vec<u8>,
}

/// The files the `DtsFileErrors` block renders, in the pin's order.
pub fn dts_file_error_inputs<'a>(
    ts_config_files: &[TestFile<'a>],
    result: &DeclarationCompilationResult<'a>,
) -> Vec<TestFile<'a>> {
    let mut files = ts_config_files.to_vec();
    files.extend_from_slice(&result.decl_input_files);
    files.extend_from_slice(&result.decl_other_files);
    files
}

/// `result.Repeat({"noCheck": "true"})`'s outputs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepeatOutputs<'a> {
    pub js: OrderedFiles<'a>,
    pub dts: OrderedFiles<'a>,
}

/// Renders the error baseline of a JSON output whose re-parse has
/// diagnostics (`GetErrorBaseline([file], diagnostics, pretty=false)`).
pub trait JsonErrorBaseline {
    fn render(
        &self,
        file: &TestFile<'_>,
        parsed: &tsr_ast::ParsedFile,
        diagnostics: &[tsr_ast::Diagnostic],
    ) -> Result<Vec<u8>, Failure>;
}

/// What `DoJSEmitBaseline` reads.
pub struct JsEmitInput<'a> {
    /// The runner's configured name (`c.configuredName`).
    pub configured_name: &'a [u8],
    pub header: &'a [u8],
    /// The harness's compiler options (`c.options`).
    pub options: &'a CompilerOptions,
    /// `harnessSettings.FullEmitPaths`.
    pub full_emit_paths: bool,
    /// `harnessSettings.CurrentDirectory`.
    pub harness_current_directory: &'a [u8],
    pub to_be_compiled: &'a [TestFile<'a>],
    pub other_files: &'a [TestFile<'a>],
    pub result: &'a CompilationResult<'a>,
    /// The declaration re-compilation; required exactly when
    /// [`prepare_declaration_compilation_context`] returns a context.
    pub declaration: Option<&'a DeclarationCompilationResult<'a>>,
    /// The `noCheck` repeat's outputs; required when the repeat runs (neither
    /// `noCheck` nor `noEmit`).
    pub no_check_repeat: Option<&'a RepeatOutputs<'a>>,
    pub json_errors: &'a dyn JsonErrorBaseline,
}

/// The sources block that opens a `.js` baseline: the header, then every
/// source in `otherFiles + toBeCompiled` order under its base file name.
// source: tsc/internal/testutil/tsbaseline/js_emit_baseline.go:DoJSEmitBaseline
pub fn ts_code(
    header: &[u8],
    other_files: &[TestFile<'_>],
    to_be_compiled: &[TestFile<'_>],
) -> Vec<u8> {
    let mut ts_code = Vec::new();
    let ts_sources: Vec<&TestFile<'_>> = other_files.iter().chain(to_be_compiled).collect();
    ts_code.extend_from_slice(b"//// [");
    ts_code.extend_from_slice(header);
    ts_code.extend_from_slice(b"] ////\r\n\r\n");
    for (i, file) in ts_sources.iter().enumerate() {
        ts_code.extend_from_slice(b"//// [");
        ts_code.extend_from_slice(path::base_name(file.unit_name));
        ts_code.extend_from_slice(b"]\r\n");
        ts_code.extend_from_slice(file.content);
        if i < ts_sources.len() - 1 {
            ts_code.extend_from_slice(b"\r\n");
        }
    }
    ts_code
}

// source: tsc/internal/testutil/tsbaseline/js_emit_baseline.go:DoJSEmitBaseline
pub fn js_emit_baseline(input: &JsEmitInput<'_>) -> Result<Baseline, Failure> {
    let options = input.options;
    let result = input.result;
    if !options.no_emit.is_true()
        && !options.emit_declaration_only.is_true()
        && result.js.is_empty()
        && result.diagnostics == 0
    {
        return Err(Failure::Assertion(
            "Expected at least one js file to be emitted or at least one error to be created."
                .into(),
        ));
    }

    // check js output
    let ts_code = ts_code(input.header, input.other_files, input.to_be_compiled);

    let mut js_code = Vec::new();
    for file in result.js.files() {
        if !js_code.is_empty() && !js_code.ends_with(b"\n") {
            js_code.extend_from_slice(b"\r\n");
        }
        if result.diagnostics == 0 && file.unit_name.ends_with(path::EXTENSION_JSON) {
            let parsed = tsr_parser::parse_source_file(
                tsr_jsstring::SourceText::from_loaded_bytes(file.content),
                tsr_core::ScriptKind::JSON,
                tsr_ast::SourceFileParseOptions {
                    file_name: JsString::from_bytes(file.unit_name),
                    path: JsString::from_bytes(file.unit_name),
                    ..Default::default()
                },
            );
            let diagnostics = parsed
                .view()
                .source_file(parsed.root())
                .map_err(|error| Failure::Runtime(format!("JSON output parse: {error:?}")))?
                .diagnostics
                .clone();
            if !diagnostics.is_empty() {
                js_code.extend(input.json_errors.render(file, &parsed, &diagnostics)?);
                continue;
            }
        }
        js_code.extend(file_output(file, input.full_emit_paths));
    }

    if !result.dts.is_empty() {
        js_code.extend_from_slice(b"\r\n\r\n");
        for decl_file in result.dts.files() {
            js_code.extend(file_output(decl_file, input.full_emit_paths));
        }
    }

    let context = prepare_declaration_compilation_context(
        input.to_be_compiled,
        input.other_files,
        result,
        options,
        input.harness_current_directory,
    )?;
    let decl_file_compilation_result = compile_declaration_files(context, input.declaration)?;

    if let Some(compiled) = decl_file_compilation_result {
        if compiled.diagnostics > 0 {
            js_code.extend_from_slice(b"\r\n\r\n//// [DtsFileErrors]\r\n");
            js_code.extend_from_slice(b"\r\n\r\n");
            js_code.extend_from_slice(&compiled.error_baseline);
        }
    }

    if !options.no_check.is_true() && !options.no_emit.is_true() {
        let without_checking = input.no_check_repeat.ok_or_else(|| {
            Failure::Input("the noCheck repeat runs but no repeat outputs were supplied".into())
        })?;
        let mut compare_result_file_sets = |a: &OrderedFiles<'_>, b: &OrderedFiles<'_>| {
            for doc in a.files() {
                match b.get(doc.unit_name) {
                    None => {
                        js_code.extend_from_slice(b"\r\n\r\n!!!! File ");
                        js_code.extend(remove_test_path_prefixes(doc.unit_name, false));
                        js_code.extend_from_slice(
                            b" missing from original emit, but present in noCheck emit\r\n",
                        );
                        js_code.extend(file_output(doc, input.full_emit_paths));
                    }
                    Some(original) if original.content != doc.content => {
                        js_code.extend_from_slice(b"\r\n\r\n!!!! File ");
                        js_code.extend(remove_test_path_prefixes(doc.unit_name, false));
                        js_code
                            .extend_from_slice(b" differs from original emit in noCheck emit\r\n");
                        let file_name = if input.full_emit_paths {
                            remove_test_path_prefixes(doc.unit_name, false)
                        } else {
                            path::base_name(doc.unit_name).to_vec()
                        };
                        js_code.extend_from_slice(b"//// [");
                        js_code.extend(file_name);
                        js_code.extend_from_slice(b"]\r\n");
                        js_code.extend(patience::diff_text(
                            b"Expected\tThe full check baseline",
                            b"Actual\twith noCheck set",
                            original.content,
                            doc.content,
                        ));
                    }
                    Some(_) => {}
                }
            }
        };
        compare_result_file_sets(&without_checking.dts, &result.dts);
        compare_result_file_sets(&without_checking.js, &result.js);
    }

    let path = baseline_path(input.configured_name, path::EXTENSION_JS);
    let actual = if js_code.is_empty() {
        NO_CONTENT.to_vec()
    } else {
        [ts_code.as_slice(), b"\r\n\r\n", &js_code].concat()
    };
    Ok(Baseline { path, actual })
}

/// `compileDeclarationFiles` is the caller's: this checks that the result it
/// supplied belongs to the context the writer prepared.
// source: tsc/internal/testutil/tsbaseline/js_emit_baseline.go:compileDeclarationFiles
fn compile_declaration_files<'r, 'a>(
    context: Option<DeclarationCompilationContext<'_>>,
    supplied: Option<&'r DeclarationCompilationResult<'a>>,
) -> Result<Option<&'r DeclarationCompilationResult<'a>>, Failure> {
    match (context, supplied) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err(Failure::Input(
            "a declaration re-compilation was supplied but the writer prepares none".into(),
        )),
        (Some(_), None) => Err(Failure::Input(
            "the writer prepares a declaration re-compilation but none was supplied".into(),
        )),
        (Some(context), Some(result)) => {
            if context.decl_input_files != result.decl_input_files
                || context.decl_other_files != result.decl_other_files
            {
                return Err(Failure::Input(
                    "the supplied declaration re-compilation compiled other files than the prepared context"
                        .into(),
                ));
            }
            Ok(Some(result))
        }
    }
}

/// Which emitted declaration files the runner re-compiles, and the harness
/// assertions it makes on the way. `None` when there is no re-compilation.
// source: tsc/internal/testutil/tsbaseline/js_emit_baseline.go:prepareDeclarationCompilationContext
pub fn prepare_declaration_compilation_context<'a>(
    input_files: &[TestFile<'a>],
    other_files: &[TestFile<'a>],
    result: &CompilationResult<'a>,
    options: &CompilerOptions,
    harness_current_directory: &'a [u8],
) -> Result<Option<DeclarationCompilationContext<'a>>, Failure> {
    let program = result.program;
    if options.declaration.is_true() && result.diagnostics == 0 {
        if options.emit_declaration_only.is_true() {
            if !result.js.is_empty() {
                return Err(Failure::Assertion(
                    "Only declaration files should be generated when emitDeclarationOnly:true"
                        .into(),
                ));
            }
            if result.dts.is_empty() && !options.no_emit.is_true() {
                return Err(Failure::Assertion("Expected at least one declaration file to be emitted when emitDeclarationOnly:true and no errors were generated".into()));
            }
        } else if !program
            .source_files()
            .iter()
            .any(|file| !file.content_mapper.is_empty())
            && result.dts.len() != result.number_of_js_files(false /*includeJson*/)
        {
            return Err(Failure::Assertion(
                "There were no errors and declFiles generated did not match number of js files generated"
                    .into(),
            ));
        }
    }

    let find_unit = |file_name: &[u8], units: &[TestFile<'a>]| {
        units.iter().any(|unit| unit.unit_name == file_name)
    };

    let find_result_code_file = |file_name: &[u8]| -> Result<Option<TestFile<'a>>, Failure> {
        let Some(source_file) = program.source_file(file_name) else {
            return Err(Failure::Assertion(format!(
                "Program has no source file with name '{}'",
                String::from_utf8_lossy(file_name)
            )));
        };
        // Is this file going to be emitted separately
        let source_file_name = if options.out_dir.is_empty() {
            source_file.file_name.to_vec()
        } else {
            let mut source_file_path =
                path::absolute(source_file.file_name, program.current_directory());
            source_file_path =
                replace_first(&source_file_path, &program.common_source_directory()?);
            path::combine(options.out_dir.as_bytes(), &[&source_file_path])
        };
        let d_ts_file_name = tsr_tsoptions::output_paths::declaration_extension(
            &source_file_name,
            &program.content_mapper_extensions(),
        );
        Ok(result.dts.get(&d_ts_file_name).copied())
    };

    let add_dts_file = |file: &TestFile<'a>,
                        dts_files: &mut Vec<TestFile<'a>>,
                        other: &[TestFile<'a>]|
     -> Result<(), Failure> {
        if path::is_declaration_file_name(file.unit_name)
            || path::has_json_file_extension(file.unit_name)
        {
            dts_files.push(*file);
        } else if let Some(source_file) = program.source_file(file.unit_name) {
            if path::has_ts_file_extension(file.unit_name)
                || (path::has_js_file_extension(file.unit_name) && options.allow_js())
                || !source_file.content_mapper.is_empty()
            {
                if let Some(decl_file) = find_result_code_file(file.unit_name)? {
                    if !find_unit(decl_file.unit_name, dts_files)
                        && !find_unit(decl_file.unit_name, other)
                    {
                        dts_files.push(TestFile {
                            unit_name: decl_file.unit_name,
                            content: decl_file
                                .content
                                .strip_prefix(b"\xef\xbb\xbf")
                                .unwrap_or(decl_file.content),
                        });
                    }
                }
            }
        }
        Ok(())
    };

    // if the .d.ts is non-empty, confirm it compiles correctly as well
    if options.declaration.is_true() && result.diagnostics == 0 && !result.dts.is_empty() {
        let mut decl_input_files = Vec::new();
        let mut decl_other_files = Vec::new();
        for file in input_files {
            add_dts_file(file, &mut decl_input_files, &decl_other_files)?;
        }
        for file in other_files {
            add_dts_file(file, &mut decl_other_files, &decl_input_files)?;
        }
        return Ok(Some(DeclarationCompilationContext {
            decl_input_files,
            decl_other_files,
            current_directory: harness_current_directory,
        }));
    }
    Ok(None)
}

/// Go's `strings.Replace(s, old, "", 1)`.
fn replace_first(text: &[u8], old: &[u8]) -> Vec<u8> {
    if old.is_empty() {
        return text.to_vec();
    }
    match text.windows(old.len()).position(|window| window == old) {
        Some(at) => [&text[..at], &text[at + old.len()..]].concat(),
        None => text.to_vec(),
    }
}

/// What `DoSourcemapBaseline` reads.
pub struct SourcemapInput<'a> {
    pub configured_name: &'a [u8],
    pub options: &'a CompilerOptions,
    pub full_emit_paths: bool,
    pub result: &'a CompilationResult<'a>,
}

/// `Ok(None)` when the runner calls no `baseline.Run`.
// source: tsc/internal/testutil/tsbaseline/sourcemap_baseline.go:DoSourcemapBaseline
pub fn sourcemap_baseline(input: &SourcemapInput<'_>) -> Result<Option<Baseline>, Failure> {
    let options = input.options;
    let result = input.result;
    let decl_maps = options.declaration_maps_enabled();
    if options.inline_source_map.is_true() {
        if !result.maps.is_empty() && !decl_maps {
            return Err(Failure::Assertion(
                "No sourcemap files should be generated if inlineSourceMaps was set.".into(),
            ));
        }
        return Ok(None);
    }
    if !(options.source_map.is_true() || decl_maps) {
        return Ok(None);
    }
    let mut expected_map_count = 0;
    if options.source_map.is_true() {
        expected_map_count += result.number_of_js_files(false /*includeJSON*/);
    }
    if decl_maps {
        expected_map_count += result.dts.len();
    }
    if result.maps.len() != expected_map_count {
        return Err(Failure::Assertion(
            "Number of sourcemap files should be same as js files.".into(),
        ));
    }

    let source_map_code = if options.no_emit_on_error.is_true() && result.diagnostics != 0
        || result.maps.is_empty()
    {
        NO_CONTENT.to_vec()
    } else {
        let mut builder = Vec::new();
        for source_map in result.maps.files() {
            if !builder.is_empty() {
                builder.extend_from_slice(b"\r\n");
            }
            builder.extend(file_output(source_map, input.full_emit_paths));
            if !options.inline_source_map.is_true() {
                builder.extend(create_source_map_preview_link(source_map, result)?);
            }
        }
        builder
    };

    let path = baseline_path(input.configured_name, b".js.map");
    Ok(Some(Baseline {
        path,
        actual: source_map_code,
    }))
}

// source: tsc/internal/testutil/tsbaseline/sourcemap_baseline.go:createSourceMapPreviewLink
fn create_source_map_preview_link(
    source_map: &TestFile<'_>,
    result: &CompilationResult<'_>,
) -> Result<Vec<u8>, Failure> {
    let mut sourcemap_json = tsr_sourcemap::RawSourceMap::default();
    tsr_json::unmarshal(
        source_map.content,
        &mut sourcemap_json,
        tsr_json::Options::default(),
    )
    .map_err(|error| Failure::Runtime(format!("json.Unmarshal of a source map: {error}")))?;

    let Some(output_js_file) = result
        .outputs
        .iter()
        .find(|td| td.unit_name.ends_with(sourcemap_json.file.as_bytes()))
    else {
        return Ok(Vec::new());
    };

    let mut source_tds = Vec::with_capacity(sourcemap_json.sources.len());
    for source in &sourcemap_json.sources {
        let source_file = result
            .inputs
            .iter()
            .find(|td| td.unit_name.ends_with(source.as_bytes()));
        let td = match source_file {
            Some(source_file) => match result.program.source_file(source_file.unit_name) {
                Some(program_source) => Some(TestFile {
                    unit_name: source_file.unit_name,
                    content: program_source.original_text,
                }),
                None => Some(*source_file),
            },
            None => None,
        };
        source_tds.push(td);
    }
    if source_tds.contains(&None) {
        return Ok(Vec::new());
    }

    let mut hash = Vec::new();
    hash.extend_from_slice(b"\n//// https://sokra.github.io/source-map-visualization#base64,");
    hash.extend(base64_encode_chunk(output_js_file.content));
    hash.push(b',');
    hash.extend(base64_encode_chunk(source_map.content));
    for td in source_tds.into_iter().flatten() {
        hash.push(b',');
        hash.extend(base64_encode_chunk(td.content));
    }
    hash.push(b'\n');
    Ok(hash)
}

/// `url.QueryEscape` then `url.QueryUnescape` restore every byte string, so
/// this is standard padded base64 of the bytes.
// source: tsc/internal/testutil/tsbaseline/sourcemap_baseline.go:base64EncodeChunk
fn base64_encode_chunk(bytes: &[u8]) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63]);
        out.push(ALPHABET[(n >> 12) as usize & 63]);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63]
        } else {
            b'='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63]
        } else {
            b'='
        });
    }
    out
}

/// What `DoSourcemapRecordBaseline` reads.
pub struct SourcemapRecordInput<'a> {
    pub configured_name: &'a [u8],
    pub options: &'a CompilerOptions,
    pub result: &'a CompilationResult<'a>,
}

// source: tsc/internal/testutil/tsbaseline/sourcemap_record_baseline.go:DoSourcemapRecordBaseline
pub fn sourcemap_record_baseline(input: &SourcemapRecordInput<'_>) -> Result<Baseline, Failure> {
    let options = input.options;
    let mut actual = NO_CONTENT.to_vec();
    if options.source_map.is_true()
        || options.inline_source_map.is_true()
        || options.declaration_map.is_true()
    {
        let record = remove_test_path_prefixes(
            &sourcemap_record::get_source_map_record(input.result)?,
            false, /*retainTrailingDirectorySeparator*/
        );
        // !(NoEmitOnError && len(Diagnostics) > 0) && len(record) > 0
        if (!options.no_emit_on_error.is_true() || input.result.diagnostics == 0)
            && !record.is_empty()
        {
            actual = record;
        }
    }
    let path = baseline_path(input.configured_name, b".sourcemap.txt");
    Ok(Baseline { path, actual })
}
