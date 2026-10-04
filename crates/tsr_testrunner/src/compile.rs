//! `harnessutil.go`, the compilation half: `CompileFiles` / `CompileFilesEx`
//! build the in-memory file system (units, symlinks, the `/.lib` fixtures
//! from `tests/lib` when referenced), finalize the options, open the
//! content-mapper host when `runExternalCode` is on, and run
//! `compileFilesWithHost`: a pre-emit program collected for diagnostics, a
//! fresh post-emit program that emits through the output recorder before
//! its diagnostics are collected, and the count-mismatch rule;
//! `newCompilationResult` then orders the outputs by the inputs.
//!
//! The Rust pieces behind this exist in `crate::harness` (the recorder,
//! `create_program` with the incremental wrapper, `new_compilation_result`,
//! `ProgramFacts`); this module owns the flow over Rust inputs.
//!
//! The pin's harness host is `compiler.NewCompilerHost` over the wrapped
//! test file system with a `TracerForBaselining`; here the host is the
//! loader's `ProgramOptions` over the same file system, the Rust program
//! records its own resolution trace, and `Compilation::trace` renders it
//! through the ported sanitizer. The pin's process-wide `sourceFileCache`
//! is one `FileCache` per compilation, shared by the pre- and post-emit
//! programs, so both read the same parsed files.
use crate::harness::baselines::{self, Failure, SourceMapEmitResult, TestFile as FileView};
use crate::harness::incremental::{create_program, HarnessProgram};
use crate::harness::program_view::ProgramFacts;
use crate::harness::recorder::{Output, Recorder};
use crate::harness_options::{set_options_from_test_config, HarnessOptions, TestConfiguration};
use crate::{Mode, Stop, TestData, TEST_LIB_FOLDER};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use tsr_ast::{Diagnostic, NodeId};
use tsr_checker::CheckerRequest;
use tsr_compiler::{
    CheckedProgram, EmitOptions, EmitResult, FileCache, Program, ProgramLike, ProgramOptions,
    WriteFileData,
};
use tsr_core::{CompilerOptions, NewLineKind, TextRange, Tristate};
use tsr_jsstring::JsString;
use tsr_tsoptions::ParsedCommandLine;
use tsr_tspath::{absolute, file_extension_is, EXTENSION_JSON, EXTENSION_TS_BUILD_INFO};

/// A unit or output name and its bytes (Go's `harnessutil.TestFile`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestFile {
    pub unit_name: Vec<u8>,
    pub content: Vec<u8>,
}

impl TestFile {
    pub fn view(&self) -> FileView<'_> {
        FileView {
            unit_name: &self.unit_name,
            content: &self.content,
        }
    }
}

/// Why a compilation did not produce a result: the pin's `t.Fatalf` on the
/// way, a production refusal or error of the loader or checker, or a harness
/// defect in this crate's own plumbing.
#[derive(Debug)]
pub enum CompileError {
    Stop(Stop),
    Compiler(tsr_compiler::Error),
    Harness(Failure),
}

impl From<Stop> for CompileError {
    fn from(stop: Stop) -> Self {
        CompileError::Stop(stop)
    }
}

impl From<tsr_compiler::Error> for CompileError {
    fn from(error: tsr_compiler::Error) -> Self {
        CompileError::Compiler(error)
    }
}

impl From<Failure> for CompileError {
    fn from(failure: Failure) -> Self {
        CompileError::Harness(failure)
    }
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Stop(stop) => write!(f, "{stop}"),
            CompileError::Compiler(error) => write!(f, "compiler: {error:?}"),
            CompileError::Harness(failure) => write!(f, "harness: {failure}"),
        }
    }
}

/// The inputs of one `CompileFilesEx`, kept so `Repeat` can run the same
/// files again under another configuration.
///
/// In the pin, `Repeat` closes over the runner's own `*TestFile` pointers,
/// so the content-mapper fix-up `newCompilerTest` applies to its test files
/// after compiling (`file.Content = sf.Text()`) is what a later `Repeat`
/// compiles. A caller that applies that fix-up to its copies applies it to
/// `input_files` and `other_files` here as well.
#[derive(Clone, Debug)]
pub struct Inputs {
    pub input_files: Vec<TestFile>,
    pub other_files: Vec<TestFile>,
    pub harness_options: HarnessOptions,
    /// After `compile_files_ex`, the options with the paths it made
    /// absolute, as the pin's `CompileFilesEx` rewrites the options its
    /// `Repeat` clones.
    pub compiler_options: CompilerOptions,
    pub current_directory: Vec<u8>,
    pub symlinks: BTreeMap<Vec<u8>, Vec<u8>>,
    /// The tsconfig `CompileFilesEx` reads: its `config_file`,
    /// `config_dependencies` (the extended configs' syntax owners), `errors`,
    /// `content_mappers` and `config_specs()`; never its options or files.
    pub tsconfig: Option<ParsedCommandLine>,
    pub mode: Mode,
}

/// `harnessutil.CompilationResult`: the post-emit program, its emit, the
/// recorded outputs, the diagnostics `compileFilesWithHost` settles on, the
/// resolution trace and everything the writers read.
pub struct Compilation {
    /// The post-emit program (`createProgram`'s, incremental when asked).
    pub program: HarnessProgram,
    /// `Program.Emit`'s result; `None` is Go's nil.
    pub emit: Option<EmitResult>,
    /// `OutputRecorderFS` outputs in written order: real path and text.
    pub recorded: Vec<(Vec<u8>, Vec<u8>)>,
    /// The post-emit diagnostics, or the shorter list plus the ad hoc
    /// count-mismatch diagnostic when pre- and post-emit counts differ.
    pub diagnostics: Vec<Diagnostic>,
    /// `program.Options()`.
    pub options: CompilerOptions,
    pub harness_options: HarnessOptions,
    pub symlinks: BTreeMap<Vec<u8>, Vec<u8>>,
    /// `TracerForBaselining.String()` after the compilation.
    pub trace: Vec<u8>,
    pub inputs: Inputs,
    /// The writers' view of `program`, which `result()` borrows.
    facts: ProgramFacts,
    /// The pre-emit program, retained only when the counts differ: its
    /// diagnostics may then be among `diagnostics`, and a diagnostic names
    /// its file by an identity that does not retain the file (Go's
    /// `*SourceFile` pointer does). The two programs share every file the
    /// cache could reuse, so this keeps alive only what the pre-emit
    /// program loaded on its own.
    pre_program: Option<HarnessProgram>,
}

impl Compilation {
    /// `result.Repeat(testConfig)`: the same inputs compiled again with
    /// `testConfig` applied on top of clones of the options.
    // port: tsc/internal/testutil/harnessutil/harnessutil.go:CompileFilesEx
    pub fn repeat(
        &self,
        test_config: &TestConfiguration,
        testdata: &TestData,
    ) -> Result<Compilation, CompileError> {
        let mut inputs = self.inputs.clone();
        set_options_from_test_config(
            test_config,
            &mut inputs.compiler_options,
            &mut inputs.harness_options,
            &inputs.current_directory,
            false,
        )?;
        compile_files_ex(inputs, testdata)
    }

    /// The writers' view (`newCompilationResult` over the recorded outputs):
    /// JS, DTS and map outputs ordered by the inputs, then by name.
    // port: tsc/internal/testutil/harnessutil/harnessutil.go:newCompilationResult
    pub fn result(&self) -> Result<baselines::CompilationResult<'_>, Failure> {
        let recorded: Vec<FileView<'_>> = self
            .recorded
            .iter()
            .map(|(unit_name, content)| FileView { unit_name, content })
            .collect();
        baselines::new_compilation_result(
            &self.facts,
            &recorded,
            self.diagnostics.len(),
            source_maps(self.emit.as_ref()),
        )
    }

    /// `GetSourceMapRecord`.
    // port: tsc/internal/testutil/harnessutil/harnessutil.go:CompilationResult.GetSourceMapRecord
    pub fn source_map_record(&self) -> Result<Vec<u8>, Failure> {
        baselines::sourcemap_record::get_source_map_record(&self.result()?)
    }

    /// The writers' `ProgramView` of the post-emit program.
    pub fn facts(&self) -> &ProgramFacts {
        &self.facts
    }

    /// The pre-emit program when its diagnostic count differed from the
    /// post-emit one (see `diagnostics`), so a renderer can resolve the
    /// files of the pre-emit diagnostics it reports.
    pub fn pre_program(&self) -> Option<&HarnessProgram> {
        self.pre_program.as_ref()
    }
}

/// `EmitResult.SourceMaps` as the writers read them; `None` is a nil
/// `result.Result`.
fn source_maps(result: Option<&EmitResult>) -> Option<Vec<SourceMapEmitResult>> {
    result.map(|result| {
        result
            .source_maps
            .iter()
            .map(|map| SourceMapEmitResult {
                input_source_file_names: map.input_source_file_names.clone(),
                source_map: map.source_map.clone(),
                generated_file: map.generated_file.as_bytes().to_vec(),
            })
            .collect()
    })
}

/// The defaults `CompileFiles` applies before the test configuration: the
/// tsconfig's options or none, `newLine` CRLF unless set,
/// `skipDefaultLibCheck` true unless set, `noErrorTruncation` true, and
/// harness options with case-sensitive names and `current_directory`; then
/// `SetOptionsFromTestConfig`. Shared with `list`, which decides skips from
/// these options without compiling: nothing here reads the file system.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:CompileFiles
pub fn resolve_options(
    test_config: Option<&TestConfiguration>,
    tsconfig: Option<&ParsedCommandLine>,
    current_directory: &[u8],
) -> Result<(CompilerOptions, HarnessOptions), Stop> {
    let mut compiler_options = tsconfig.map_or_else(CompilerOptions::default, |tsconfig| {
        tsconfig.options.clone()
    });
    // Set default options for tests
    if compiler_options.new_line == NewLineKind::NONE {
        compiler_options.new_line = NewLineKind::CRLF;
    }
    if compiler_options.skip_default_lib_check == Tristate::UNKNOWN {
        compiler_options.skip_default_lib_check = Tristate::TRUE;
    }
    compiler_options.no_error_truncation = Tristate::TRUE;
    let mut harness_options = HarnessOptions {
        use_case_sensitive_file_names: true,
        current_directory: current_directory.to_vec(),
        ..HarnessOptions::default()
    };

    // Parse harness and compiler options from the test configuration
    if let Some(test_config) = test_config {
        set_options_from_test_config(
            test_config,
            &mut compiler_options,
            &mut harness_options,
            current_directory,
            false,
        )?;
    }
    Ok((compiler_options, harness_options))
}

/// `CompileFiles`: `resolve_options`, then `compile_files_ex`.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:CompileFiles
#[allow(
    clippy::too_many_arguments,
    reason = "CompileFiles' parameters, plus the test data and the program mode"
)]
pub fn compile_files(
    input_files: Vec<TestFile>,
    other_files: Vec<TestFile>,
    test_config: Option<&TestConfiguration>,
    tsconfig: Option<ParsedCommandLine>,
    current_directory: &[u8],
    symlinks: BTreeMap<Vec<u8>, Vec<u8>>,
    testdata: &TestData,
    mode: Mode,
) -> Result<Compilation, CompileError> {
    let (compiler_options, harness_options) =
        resolve_options(test_config, tsconfig.as_ref(), current_directory)?;
    compile_files_ex(
        Inputs {
            input_files,
            other_files,
            harness_options,
            compiler_options,
            current_directory: current_directory.to_vec(),
            symlinks,
            tsconfig,
            mode,
        },
        testdata,
    )
}

/// `CompileFilesEx`: program file names (inputs minus `.json` and
/// `.tsbuildinfo`, plus `/.lib/<libFiles>`), the `/.lib` folder when an
/// input mentions it, absolute `outDir`/`project`/`rootDir`/
/// `tsBuildInfoFile`/`baseUrl`/`declarationDir`/`rootDirs`/`typeRoots`, the
/// in-memory file system under `BundledFs`, the content-mapper host when
/// `runExternalCode` and mappers are present, and `compileFilesWithHost`.
/// The content-mapper project and host close when this returns, as the
/// pin's deferred closes run.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:CompileFilesEx
// port: tsc/internal/testutil/harnessutil/harnessutil.go:createCompilerHost
pub fn compile_files_ex(
    mut inputs: Inputs,
    testdata: &TestData,
) -> Result<Compilation, CompileError> {
    let current_directory = inputs.current_directory.clone();
    let cwd = current_directory.as_slice();
    let mut program_file_names = Vec::new();
    for file in &inputs.input_files {
        let file_name = absolute(&file.unit_name, cwd);
        if !file_extension_is(&file_name, EXTENSION_JSON)
            && !file_extension_is(&file_name, EXTENSION_TS_BUILD_INFO)
        {
            program_file_names.push(JsString::from_bytes(file_name));
        }
    }

    // Performance optimization; avoid copying in the /.lib folder if the test doesn't need it.
    let lib_folder_prefix = [TEST_LIB_FOLDER, b"/"].concat();
    let mut include_lib_dir = inputs
        .input_files
        .iter()
        .any(|file| contains(&file.content, &lib_folder_prefix));

    // Files from testdata\lib that are requested by "@libFiles"
    for lib_file in &inputs.harness_options.lib_files {
        if lib_file.as_slice() == b"lib.d.ts" && inputs.compiler_options.no_lib != Tristate::TRUE {
            // We used to override lib with a custom lib.d.ts for some reason. Skip this unless it becomes necessary.
            continue;
        }
        program_file_names.push(JsString::from_bytes(tsr_tspath::combine(
            TEST_LIB_FOLDER,
            &[lib_file],
        )));
        include_lib_dir = true;
    }

    absolutize_option_paths(&mut inputs.compiler_options, cwd);

    // Create fake FS for testing
    let mut fs =
        tsr_vfs::MemoryBuilder::new(cwd, inputs.harness_options.use_case_sensitive_file_names);
    for file in inputs.input_files.iter().chain(&inputs.other_files) {
        fs.insert_physical(&absolute(&file.unit_name, cwd), file.content.clone());
    }
    for (src, target) in &inputs.symlinks {
        fs.insert_symlink(&absolute(src, cwd), &absolute(target, cwd));
    }
    if include_lib_dir {
        let lib_folder = test_lib_folder_map(testdata)?;
        for (name, content) in &*lib_folder {
            fs.insert_physical(name, content.clone());
        }
    }
    let host: Arc<dyn tsr_vfs::FileSystem> =
        Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish())));

    let mut config = ParsedCommandLine::new(inputs.compiler_options.clone(), program_file_names);
    let mut specs = None;
    if let Some(tsconfig) = &inputs.tsconfig {
        config.config_file.clone_from(&tsconfig.config_file);
        // Go reaches the extended configs through the config file; Rust
        // keeps their syntax owners beside it.
        config
            .config_dependencies
            .clone_from(&tsconfig.config_dependencies);
        config.errors.clone_from(&tsconfig.errors);
        config.content_mappers.clone_from(&tsconfig.content_mappers);
        // Go stores the include/exclude specifications on the config file;
        // Rust keeps them beside it in ParsedCommandLine.
        specs = tsconfig.config_specs().cloned();
    }
    // CompileFilesEx constructs a fresh ParsedCommandLine without
    // comparePathsOptions: an empty directory and case-insensitive matching.
    config.set_config_specs(specs, JsString::default(), false);

    // Content mappers, when trusted, are served in-process by the test mapper (see contentmappertest).
    // The host is shared by the pre- and post-emit programs and torn down when this compilation finishes.
    let scope = ContentMapperScope::open(&config);
    let host = HarnessHost {
        fs: host,
        current_directory: JsString::from_bytes(cwd),
        content_mapper_project: scope.project.clone(),
        mode: inputs.mode,
    };
    let compiled = compile_files_with_host(&host, &config, &inputs.harness_options)?;
    let trace = crate::harness::subtests::trace_text(compiled.program.program());
    let compilation = Compilation {
        options: compiled.program.program().options().clone(),
        facts: compiled.facts,
        program: compiled.program,
        pre_program: compiled.pre_program,
        emit: compiled.emit,
        recorded: compiled.recorded,
        diagnostics: compiled.diagnostics,
        harness_options: inputs.harness_options.clone(),
        symlinks: inputs.symlinks.clone(),
        trace,
        inputs,
    };
    drop(scope);
    Ok(compilation)
}

/// The paths `CompileFilesEx` makes absolute against the current directory
/// (the pin's partial `convertToOptionsWithAbsolutePaths`).
// port: tsc/internal/testutil/harnessutil/harnessutil.go:CompileFilesEx
fn absolutize_option_paths(options: &mut CompilerOptions, cwd: &[u8]) {
    let absolute_in_place = |value: &mut JsString| {
        if !value.is_empty() {
            *value = JsString::from_bytes(absolute(value.as_bytes(), cwd));
        }
    };
    absolute_in_place(&mut options.out_dir);
    absolute_in_place(&mut options.project);
    absolute_in_place(&mut options.root_dir);
    absolute_in_place(&mut options.ts_build_info_file);
    absolute_in_place(&mut options.base_url);
    absolute_in_place(&mut options.declaration_dir);
    for root_dir in options.root_dirs.iter_mut().flatten() {
        *root_dir = JsString::from_bytes(absolute(root_dir.as_bytes(), cwd));
    }
    for type_root in options.type_roots.iter_mut().flatten() {
        *type_root = JsString::from_bytes(absolute(type_root.as_bytes(), cwd));
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// One `/.lib` file: its name under the test file system and its bytes.
type LibFile = (Vec<u8>, Arc<[u8]>);

/// `testLibFolderMap`: every file under `tests/lib` as `/.lib/<relative
/// path>`, read once per process (the pin's `sync.OnceValue`), here once per
/// test-data directory. A failed walk is the pin's panic.
// source: tsc/internal/testutil/harnessutil/harnessutil.go:testLibFolderMap
fn test_lib_folder_map(testdata: &TestData) -> Result<Arc<[LibFile]>, Stop> {
    static FOLDERS: OnceLock<Mutex<HashMap<PathBuf, Arc<[LibFile]>>>> = OnceLock::new();
    let root = testdata.lib();
    let folders = FOLDERS.get_or_init(Mutex::default);
    if let Some(files) = folders
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&root)
    {
        return Ok(files.clone());
    }
    let mut files = Vec::new();
    walk_lib_folder(&root, b"", &mut files)
        .map_err(|error| Stop::fatal(format!("Failed to read lib dir: {error}")))?;
    let files: Arc<[LibFile]> = files.into();
    folders
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(root, files.clone());
    Ok(files)
}

/// `fs.WalkDir` over `directory`: directories are descended, every other
/// entry is read (a symbolic link is followed, a link to a directory fails).
fn walk_lib_folder(
    directory: &Path,
    relative: &[u8],
    files: &mut Vec<LibFile>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let mut path = relative.to_vec();
        if !path.is_empty() {
            path.push(b'/');
        }
        path.extend_from_slice(entry.file_name().as_encoded_bytes());
        if entry.file_type()?.is_dir() {
            walk_lib_folder(&entry.path(), &path, files)?;
            continue;
        }
        let content = std::fs::read(entry.path())?;
        files.push(([TEST_LIB_FOLDER, b"/", &path].concat(), content.into()));
    }
    Ok(())
}

/// The content-mapper host and project one `CompileFilesEx` opens when the
/// options trust external code and the configuration declares mappers, the
/// test mappers served in process. Dropping the scope closes the project,
/// then the host, in the order the pin's deferred closes run.
struct ContentMapperScope {
    host: Option<tsr_contentmapper::HostImpl>,
    project: Option<Arc<dyn tsr_contentmapper::Project>>,
}

impl ContentMapperScope {
    // port: tsc/internal/testutil/harnessutil/harnessutil.go:CompileFilesEx
    fn open(config: &ParsedCommandLine) -> Self {
        let mappers = config.content_mappers.as_deref().unwrap_or_default();
        if !config.options.run_external_code.is_true() || mappers.is_empty() {
            return Self {
                host: None,
                project: None,
            };
        }
        let host = tsr_contentmapper::new_host(
            &tsr_ipc::Context::background(),
            tsr_contentmappertest::new_spawner(),
            tsr_locale::Locale::default(),
        );
        let project = tsr_contentmapper::Host::project(
            &host,
            tsr_contentmapper::ProjectSpec {
                config_file_name: config.config_name(),
                mappers: Arc::from(mappers.to_vec()),
                compiler_options: Arc::new(config.options.clone()),
            },
        );
        Self {
            host: Some(host),
            project,
        }
    }
}

impl Drop for ContentMapperScope {
    fn drop(&mut self) {
        if let Some(project) = self.project.take() {
            let _ = project.close();
        }
        if let Some(host) = self.host.take() {
            let _ = tsr_contentmapper::Host::close(&host);
        }
    }
}

/// What `createCompilerHost` gives each program of one compilation: the
/// wrapped test file system, the current directory, the content-mapper
/// project, and the test-program mode `createProgram` reads.
struct HarnessHost {
    fs: Arc<dyn tsr_vfs::FileSystem>,
    current_directory: JsString,
    content_mapper_project: Option<Arc<dyn tsr_contentmapper::Project>>,
    mode: Mode,
}

impl HarnessHost {
    /// `createProgram`'s `compiler.NewProgram` over this host (the default
    /// library path is `bundled.LibPath()`), then the harness's wrapper.
    // port: tsc/internal/testutil/harnessutil/harnessutil.go:createProgram
    fn create_program(
        &self,
        config: ParsedCommandLine,
        cache: &mut FileCache,
        counters: &tsr_arena::Counters,
    ) -> Result<HarnessProgram, tsr_compiler::Error> {
        let options = ProgramOptions {
            config,
            host: self.fs.clone(),
            current_directory: self.current_directory.clone(),
            default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
            skip_module_resolution: false,
            single_threaded: self.mode.single_threaded(),
        };
        let program = Program::load_with_content_mapper_project(
            options,
            self.content_mapper_project.clone(),
            cache,
            counters,
        )?;
        create_program(Arc::new(CheckedProgram::new(
            Arc::new(program),
            counters,
            None,
        )))
    }
}

/// What `compileFilesWithHost` hands `newCompilationResult`.
struct HostCompilation {
    program: HarnessProgram,
    facts: ProgramFacts,
    pre_program: Option<HarnessProgram>,
    emit: Option<EmitResult>,
    recorded: Vec<Output>,
    diagnostics: Vec<Diagnostic>,
}

// port: tsc/internal/testutil/harnessutil/harnessutil.go:compileFilesWithHost
fn compile_files_with_host(
    host: &HarnessHost,
    config: &ParsedCommandLine,
    harness_options: &HarnessOptions,
) -> Result<HostCompilation, CompileError> {
    let mut cache = FileCache::new();
    let counters = tsr_arena::Counters::new();

    let mut pre_config = config.clone();
    pre_config.options.trace_resolution = Tristate::FALSE;
    crate::trace_phase("pre.load.begin");
    let pre_program = host.create_program(pre_config, &mut cache, &counters)?;
    crate::trace_phase("pre.diagnostics.begin");
    let pre_errors = harness_diagnostics(
        pre_program.program_like(),
        harness_options.capture_suggestions,
    )?;

    crate::trace_phase("post.load.begin");
    let post_program = host.create_program(config.clone(), &mut cache, &counters)?;
    crate::trace_phase("post.emit.begin");
    let (emit_result, recorded) = emit(&post_program)?;
    crate::trace_phase("post.diagnostics.begin");
    let post_errors = harness_diagnostics(
        post_program.program_like(),
        harness_options.capture_suggestions,
    )?;

    crate::trace_phase("diagnostics.settle.begin");
    let counts_match = pre_errors.len() == post_errors.len();
    let errors = settle_diagnostics(
        pre_errors,
        post_errors,
        &[
            post_program.program().as_ref(),
            pre_program.program().as_ref(),
        ],
    )
    .map_err(tsr_compiler::Error::from)?;

    crate::trace_phase("program.facts.begin");
    let facts = ProgramFacts::new(post_program.program().clone())?;
    crate::trace_phase("compile.complete");
    Ok(HostCompilation {
        program: post_program,
        facts,
        pre_program: (!counts_match).then_some(pre_program),
        emit: emit_result,
        recorded,
        diagnostics: errors,
    })
}

/// One collection of `compileFilesWithHost`, sorted and deduplicated: the
/// config file parsing, program, syntactic, semantic and global
/// diagnostics, the declaration diagnostics when declarations are emitted,
/// and the suggestion diagnostics when the test captures them.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:compileFilesWithHost
pub fn harness_diagnostics(
    program_like: &dyn ProgramLike,
    capture_suggestions: bool,
) -> Result<Vec<Diagnostic>, tsr_compiler::Error> {
    let program = program_like.checked_program().program();
    let request = CheckerRequest::default();
    crate::trace_phase("diagnostics.config-program-syntactic");
    let mut values = program_like.config_file_parsing_diagnostics();
    values.extend(program_like.program_diagnostics()?);
    values.extend(program_like.syntactic_diagnostics(&request, None)?);
    crate::trace_phase("diagnostics.semantic");
    values.extend(program_like.semantic_diagnostics(&request, None)?);
    crate::trace_phase("diagnostics.global");
    values.extend(program_like.global_diagnostics(&request)?);
    if program_like.options().emit_declarations() {
        crate::trace_phase("diagnostics.declaration");
        values.extend(program_like.declaration_diagnostics(&request, None)?);
    }
    if capture_suggestions {
        crate::trace_phase("diagnostics.suggestion");
        values.extend(program_like.suggestion_diagnostics(&request, None)?);
    }
    crate::trace_phase("diagnostics.sort");
    program.sort_and_deduplicate_diagnostics(&values)
}

/// `postProgram.Emit(ctx, compiler.EmitOptions{})` with the output recorder
/// as the file system's `WriteFile`; an incremental program writes its build
/// info through it as well.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:compileFilesWithHost
fn emit(
    created: &HarnessProgram,
) -> Result<(Option<EmitResult>, Vec<Output>), tsr_compiler::Error> {
    let recorder = Recorder::new(created.program().host());
    let write_file = |name: &[u8], text: &[u8], _: &mut WriteFileData| {
        recorder.write_file(name, text);
        Ok(())
    };
    let options = EmitOptions {
        write_file: Some(&write_file),
        ..EmitOptions::default()
    };
    let result = created
        .program_like()
        .emit(&CheckerRequest::default(), &options)?;
    Ok((result, recorder.outputs()))
}

/// The count-mismatch rule: the post-emit diagnostics when the counts agree;
/// otherwise the shorter list followed by an ad hoc compiler diagnostic
/// whose related information is "The excess diagnostics are:" and every
/// diagnostic of the longer list that `CompareDiagnostics` matches with none
/// of the shorter. `programs` resolve the diagnostics' files by name.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:compileFilesWithHost
fn settle_diagnostics(
    pre_errors: Vec<Diagnostic>,
    post_errors: Vec<Diagnostic>,
    programs: &[&Program],
) -> Result<Vec<Diagnostic>, tsr_arena::Error> {
    if post_errors.len() == pre_errors.len() {
        return Ok(post_errors);
    }
    let message = format!(
        "Pre-emit ({}) and post-emit ({}) diagnostic counts do not match! This can indicate that a semantic _error_ was added by the emit resolver - such an error may not be reflected on the command line or in the editor, but may be captured in a baseline here!",
        pre_errors.len(),
        post_errors.len()
    );
    let (longer_errors, mut shorter_errors) = if pre_errors.len() > post_errors.len() {
        (pre_errors, post_errors)
    } else {
        (post_errors, pre_errors)
    };
    let mut diag = ad_hoc_compiler_diagnostic(message.as_bytes());
    diag.related_information
        .push(Arc::new(ad_hoc_compiler_diagnostic(
            b"The excess diagnostics are:",
        )));
    let file_name = |id: NodeId| diagnostic_file_name(programs, id);
    for d in &longer_errors {
        let mut matched = false;
        for d2 in &shorter_errors {
            if tsr_ast::compare_diagnostics(d, d2, &file_name)? == Ordering::Equal {
                matched = true;
                break;
            }
        }
        if !matched {
            diag.related_information.push(Arc::new(d.clone()));
        }
    }
    shorter_errors.push(diag);
    Ok(shorter_errors)
}

/// `ast.NewCompilerDiagnostic(diagnostics.NewAdHocMessage(text))`: no file,
/// the undefined range, the ad hoc message's code (-1), category (error) and
/// key ("-1"), and no arguments.
fn ad_hoc_compiler_diagnostic(text: &[u8]) -> Diagnostic {
    let message = tsr_diagnostics::AdHocMessage::new(text);
    let mut diagnostic = Diagnostic::from_text(
        None,
        TextRange::new(-1, -1),
        message.code(),
        message.category() as i32,
        text,
        Vec::new(),
        Vec::new(),
        false,
        false,
    );
    diagnostic.message_key = JsString::from_bytes(message.key().as_bytes());
    diagnostic
}

/// `getDiagnosticPath`'s file name for a diagnostic of any of `programs`:
/// a configuration file's, or a loaded file's. The pre- and post-emit
/// programs share the files their cache reused; a file only one of them
/// loaded resolves in that one.
fn diagnostic_file_name<'p>(
    programs: &[&'p Program],
    id: NodeId,
) -> Result<&'p [u8], tsr_arena::Error> {
    for &program in programs {
        if let Some(config) = program.config_source(id) {
            return Ok(config.file.view().source_file(config.root)?.file_name());
        }
        if let Some(file) = program.file_of_node(id) {
            return Ok(file.bound().view().ast().source_file(id)?.file_name());
        }
    }
    Err(tsr_arena::Error::WrongOwner)
}

#[cfg(test)]
mod tests {
    use super::{
        ad_hoc_compiler_diagnostic, compile_files, compile_files_ex, diagnostic_file_name,
        resolve_options, settle_diagnostics, HarnessHost, Inputs, TestFile,
    };
    use crate::harness::baselines::TestFile as FileView;
    use crate::harness_options::TestConfiguration;
    use crate::{Mode, TestData};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tsr_compiler::FileCache;
    use tsr_core::{CompilerOptions, ModuleKind, NewLineKind, Tristate};
    use tsr_jsstring::{JsString, SourceText};
    use tsr_tsoptions::{ConfigValue, ParseConfigHost, ParsedCommandLine, TsConfigSourceFile};

    const A: &str = "export const a = 1;\n";
    const B: &str = "import { a } from './a';\nexport const b = a + 1;\n";

    fn testdata() -> TestData {
        TestData::in_repository(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
    }

    fn unit(name: &str, content: &str) -> TestFile {
        TestFile {
            unit_name: name.as_bytes().to_vec(),
            content: content.as_bytes().to_vec(),
        }
    }

    fn names(files: &[FileView<'_>]) -> Vec<String> {
        files
            .iter()
            .map(|file| String::from_utf8_lossy(file.unit_name).into_owned())
            .collect()
    }

    fn inputs(input_files: Vec<TestFile>, compiler_options: CompilerOptions) -> Inputs {
        let (_, harness_options) = resolve_options(None, None, b"/.src").unwrap();
        Inputs {
            input_files,
            other_files: Vec::new(),
            harness_options,
            compiler_options,
            current_directory: b"/.src".to_vec(),
            symlinks: BTreeMap::new(),
            tsconfig: None,
            mode: Mode::Single,
        }
    }

    fn esnext() -> CompilerOptions {
        let (mut options, _) = resolve_options(None, None, b"/.src").unwrap();
        options.module = ModuleKind::ESNEXT;
        options
    }

    #[test]
    fn resolve_options_applies_the_harness_defaults() {
        let (options, harness) = resolve_options(None, None, b"/.src").unwrap();
        assert_eq!(options.new_line, NewLineKind::CRLF);
        assert_eq!(options.skip_default_lib_check, Tristate::TRUE);
        assert_eq!(options.no_error_truncation, Tristate::TRUE);
        assert!(harness.use_case_sensitive_file_names);
        assert_eq!(harness.current_directory, b"/.src");

        // A tsconfig's settings win over the defaults it sets.
        let mut tsconfig = ParsedCommandLine::new(CompilerOptions::default(), Vec::new());
        tsconfig.options.new_line = NewLineKind::LF;
        tsconfig.options.skip_default_lib_check = Tristate::FALSE;
        tsconfig.options.no_error_truncation = Tristate::FALSE;
        let (options, _) = resolve_options(None, Some(&tsconfig), b"/.src").unwrap();
        assert_eq!(options.new_line, NewLineKind::LF);
        assert_eq!(options.skip_default_lib_check, Tristate::FALSE);
        assert_eq!(options.no_error_truncation, Tristate::TRUE);
    }

    #[test]
    fn compiles_two_units_and_orders_outputs_by_the_inputs() {
        let mut compilation = compile_files_ex(
            inputs(vec![unit("/.src/a.ts", A), unit("/.src/b.ts", B)], esnext()),
            &testdata(),
        )
        .unwrap();
        assert!(compilation.diagnostics.is_empty());
        assert!(compilation.pre_program().is_none());
        assert_eq!(compilation.options.module, ModuleKind::ESNEXT);
        let mut recorded: Vec<&[u8]> = compilation
            .recorded
            .iter()
            .map(|(name, _)| name.as_slice())
            .collect();
        recorded.sort_unstable();
        assert_eq!(recorded, [b"/.src/a.js".as_slice(), b"/.src/b.js"]);

        let result = compilation.result().unwrap();
        // The JS outputs follow the program's source files, whatever order
        // the emitter wrote them in.
        let expected: Vec<String> = result
            .inputs
            .iter()
            .filter(|input| input.unit_name.starts_with(b"/.src/"))
            .map(|input| {
                String::from_utf8_lossy(&tsr_tspath::change_extension(input.unit_name, b".js"))
                    .into_owned()
            })
            .collect();
        assert_eq!(names(result.js.files()), expected);
        assert_eq!(names(&result.outputs), expected);
        let b_js = result.js.get(b"/.src/b.js").unwrap().content;
        assert!(b_js.starts_with(b"import { a } from './a';\r\n"));
        assert!(result.dts.is_empty() && result.maps.is_empty());
        assert_eq!(result.diagnostics, 0);
        assert!(compilation.trace.is_empty());
        assert_eq!(compilation.source_map_record().unwrap(), b"");

        // Written in the other order (as a concurrent emit may), the outputs
        // still follow the inputs.
        compilation.recorded.reverse();
        let result = compilation.result().unwrap();
        assert_eq!(names(result.js.files()), expected);
    }

    #[test]
    fn traces_the_post_emit_program_and_records_its_source_maps() {
        let mut options = esnext();
        options.trace_resolution = Tristate::TRUE;
        options.source_map = Tristate::TRUE;
        let compilation = compile_files_ex(
            inputs(vec![unit("/.src/a.ts", A), unit("/.src/b.ts", B)], options),
            &testdata(),
        )
        .unwrap();
        let trace = String::from_utf8(compilation.trace.clone()).unwrap();
        assert!(
            trace.contains("======== Resolving module './a' from '/.src/b.ts'. ========\n"),
            "{trace}"
        );
        let result = compilation.result().unwrap();
        assert_eq!(
            names(result.maps.files()),
            ["/.src/a.js.map", "/.src/b.js.map"]
        );
        let record = String::from_utf8(compilation.source_map_record().unwrap()).unwrap();
        assert!(record.contains("JsFile: b.js"), "{record}");
    }

    #[test]
    fn compile_files_applies_the_test_configuration() {
        let config: TestConfiguration = [("module".to_owned(), "esnext".to_owned())]
            .into_iter()
            .collect();
        let compilation = compile_files(
            vec![unit("/.src/a.ts", A), unit("/.src/b.ts", B)],
            Vec::new(),
            Some(&config),
            None,
            b"/.src",
            BTreeMap::new(),
            &testdata(),
            Mode::Single,
        )
        .unwrap();
        assert_eq!(compilation.options.module, ModuleKind::ESNEXT);
        assert!(compilation.diagnostics.is_empty());

        // `Repeat` compiles the same inputs with another configuration on top.
        let repeat: TestConfiguration = [("noemit".to_owned(), "true".to_owned())]
            .into_iter()
            .collect();
        let repeated = compilation.repeat(&repeat, &testdata()).unwrap();
        assert_eq!(repeated.options.module, ModuleKind::ESNEXT);
        assert_eq!(repeated.options.no_emit, Tristate::TRUE);
        assert!(repeated.recorded.is_empty());
    }

    /// A parse host over the test's files with no package resolution.
    struct ConfigHost {
        fs: Arc<dyn tsr_vfs::FileSystem>,
    }

    impl ParseConfigHost for ConfigHost {
        fn fs(&self) -> &dyn tsr_vfs::FileSystem {
            self.fs.as_ref()
        }
        fn current_directory(&self) -> &[u8] {
            b"/.src"
        }
        fn resolve_config(
            &self,
            _name: &[u8],
            _containing: &[u8],
        ) -> Result<Option<JsString>, tsr_vfs::Error> {
            Ok(None)
        }
        fn resolve_content_mapper(
            &self,
            _containing: &[u8],
            _package: &[u8],
        ) -> Result<tsr_tsoptions::config_mappers::MapperResolution, tsr_vfs::Error> {
            Err(tsr_vfs::Error::Unsupported(
                "no content mappers in this test",
            ))
        }
    }

    #[test]
    fn a_tsconfig_supplies_its_options_and_config_file() {
        let config_text = br#"{ "compilerOptions": { "module": "esnext", "outDir": "out", "declaration": true } }"#;
        let mut fs = tsr_vfs::MemoryBuilder::new(b"/.src", true);
        fs.insert_physical(b"/.src/tsconfig.json", config_text.to_vec());
        fs.insert_physical(b"/.src/a.ts", A.as_bytes().to_vec());
        fs.insert_physical(b"/.src/b.ts", B.as_bytes().to_vec());
        let host = ConfigHost {
            fs: Arc::new(fs.finish()),
        };
        let source = TsConfigSourceFile::parse(
            JsString::from_bytes(b"/.src/tsconfig.json".as_slice()),
            tsr_tspath::to_path(b"/.src/tsconfig.json", b"/.src", true),
            SourceText::from_loaded_bytes(config_text.to_vec()),
        );
        let tsconfig = tsr_tsoptions::parse_json_source_file_config_file_content(
            source,
            &host,
            b"/.src",
            &CompilerOptions::default(),
            &ConfigValue::Null,
            b"/.src/tsconfig.json",
        )
        .unwrap();
        assert!(tsconfig.errors.is_empty());

        let compilation = compile_files(
            vec![unit("/.src/a.ts", A), unit("/.src/b.ts", B)],
            Vec::new(),
            None,
            Some(tsconfig),
            b"/.src",
            BTreeMap::new(),
            &testdata(),
            Mode::Single,
        )
        .unwrap();
        assert!(compilation.diagnostics.is_empty());
        assert_eq!(compilation.options.out_dir.as_bytes(), b"/.src/out");
        assert_eq!(compilation.options.new_line, NewLineKind::CRLF);
        assert!(compilation.program.program().config().config_file.is_some());
        let result = compilation.result().unwrap();
        assert_eq!(
            names(result.js.files()),
            ["/.src/out/a.js", "/.src/out/b.js"]
        );
        assert_eq!(
            names(result.dts.files()),
            ["/.src/out/a.d.ts", "/.src/out/b.d.ts"]
        );
        assert_eq!(
            names(&result.outputs),
            [
                "/.src/out/a.js",
                "/.src/out/a.d.ts",
                "/.src/out/b.js",
                "/.src/out/b.d.ts"
            ]
        );
    }

    #[test]
    fn the_lib_folder_is_copied_in_when_an_input_mentions_it() {
        let mut lib_files = inputs(
            vec![unit(
                "/.src/a.ts",
                "/// <reference path=\"/.lib/react.d.ts\" />\nexport const a = 1;\n",
            )],
            esnext(),
        );
        let compilation = compile_files_ex(lib_files.clone(), &testdata()).unwrap();
        let program = compilation.program.program();
        assert!(program.source_file(b"/.lib/react.d.ts").is_some());
        assert!(program.source_file(b"/.lib/react16.d.ts").is_none());

        // `libFiles` adds roots under /.lib and copies the folder in.
        lib_files.input_files = vec![unit("/.src/a.ts", A)];
        lib_files.harness_options.lib_files = vec![b"react16.d.ts".to_vec(), b"lib.d.ts".to_vec()];
        let compilation = compile_files_ex(lib_files, &testdata()).unwrap();
        let program = compilation.program.program();
        assert!(program.source_file(b"/.lib/react16.d.ts").is_some());
        // `lib.d.ts` is no root without `noLib`: a root would be missing.
        assert!(program.source_file(b"/.lib/lib.d.ts").is_none());
        assert!(
            compilation.diagnostics.is_empty(),
            "{:?}",
            compilation.diagnostics
        );
    }

    /// The pre- and post-emit programs read the same parsed files, as the
    /// pin's do through `sourceFileCache`: a diagnostic of either names a
    /// file the other program resolves.
    #[test]
    fn the_pre_and_post_emit_programs_share_their_files() {
        let mut fs = tsr_vfs::MemoryBuilder::new(b"/.src", true);
        fs.insert_physical(b"/.src/a.ts", A.as_bytes().to_vec());
        fs.insert_physical(b"/.src/b.ts", B.as_bytes().to_vec());
        let host = HarnessHost {
            fs: Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish()))),
            current_directory: JsString::from_bytes(b"/.src".as_slice()),
            content_mapper_project: None,
            mode: Mode::Single,
        };
        let config = ParsedCommandLine::new(
            esnext(),
            vec![
                JsString::from_bytes(b"/.src/a.ts".as_slice()),
                JsString::from_bytes(b"/.src/b.ts".as_slice()),
            ],
        );
        let mut pre_config = config.clone();
        pre_config.options.trace_resolution = Tristate::FALSE;
        let mut cache = FileCache::new();
        let counters = tsr_arena::Counters::new();
        let pre = host
            .create_program(pre_config, &mut cache, &counters)
            .unwrap();
        let post = host.create_program(config, &mut cache, &counters).unwrap();
        let (pre, post) = (pre.program(), post.program());
        assert_eq!(pre.files().len(), post.files().len());
        for (pre_file, post_file) in pre.files().iter().zip(post.files()) {
            assert!(Arc::ptr_eq(pre_file, post_file));
            let source = pre_file.source();
            assert_eq!(
                diagnostic_file_name(&[post.as_ref()], source).unwrap(),
                diagnostic_file_name(&[pre.as_ref()], source).unwrap()
            );
        }
    }

    #[test]
    fn a_count_mismatch_keeps_the_shorter_list_and_names_the_excess() {
        let first = ad_hoc_compiler_diagnostic(b"first");
        let second = ad_hoc_compiler_diagnostic(b"second");
        let text = |d: &tsr_ast::Diagnostic| d.ad_hoc_message.as_ref().unwrap().text().to_vec();

        // Equal counts keep the post-emit list, whatever it holds.
        let settled = settle_diagnostics(vec![first.clone()], vec![second.clone()], &[]).unwrap();
        assert_eq!(settled, std::slice::from_ref(&second));

        for (pre, post) in [
            (vec![first.clone(), second.clone()], vec![first.clone()]),
            (vec![first.clone()], vec![first.clone(), second.clone()]),
        ] {
            let (pre_count, post_count) = (pre.len(), post.len());
            let settled = settle_diagnostics(pre, post, &[]).unwrap();
            assert_eq!(settled.len(), 2);
            assert_eq!(settled[0], first);
            let diag = &settled[1];
            assert_eq!(
                String::from_utf8(text(diag)).unwrap(),
                format!(
                    "Pre-emit ({pre_count}) and post-emit ({post_count}) diagnostic counts do not match! This can indicate that a semantic _error_ was added by the emit resolver - such an error may not be reflected on the command line or in the editor, but may be captured in a baseline here!"
                )
            );
            assert_eq!((diag.code, diag.category), (-1, 1));
            assert_eq!((diag.loc.pos(), diag.loc.end()), (-1, -1));
            assert_eq!(diag.message_key.as_bytes(), b"-1");
            assert!(diag.file.is_none() && diag.message_args.is_empty());
            let related: Vec<Vec<u8>> = diag.related_information.iter().map(|d| text(d)).collect();
            assert_eq!(
                related,
                [b"The excess diagnostics are:".to_vec(), b"second".to_vec()]
            );
        }
    }
}
