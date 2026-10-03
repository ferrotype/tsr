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
//! `create_program` with the incremental wrapper, `harness_diagnostics`,
//! `new_compilation_result`, `ProgramFacts`); this module owns the flow
//! over Rust inputs instead of a Go-observed request.
use crate::harness::baselines::{self, Failure, TestFile as FileView};
use crate::harness::incremental::HarnessProgram;
use crate::harness_options::{HarnessOptions, TestConfiguration};
use crate::{Mode, Stop, TestData};
use std::collections::BTreeMap;
use tsr_ast::Diagnostic;
use tsr_compiler::EmitResult;
use tsr_core::CompilerOptions;
use tsr_tsoptions::ParsedCommandLine;

/// A unit or output name and its bytes.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:TestFile
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
#[derive(Clone, Debug)]
pub struct Inputs {
    pub input_files: Vec<TestFile>,
    pub other_files: Vec<TestFile>,
    pub harness_options: HarnessOptions,
    pub compiler_options: CompilerOptions,
    pub current_directory: Vec<u8>,
    pub symlinks: BTreeMap<Vec<u8>, Vec<u8>>,
    pub tsconfig: Option<ParsedCommandLine>,
    pub mode: Mode,
}

/// `harnessutil.CompilationResult`: the post-emit program, its emit, the
/// recorded outputs, the diagnostics `compileFilesWithHost` settles on, the
/// resolution trace and everything the writers read.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:CompilationResult
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
}

impl Compilation {
    /// `result.Repeat(testConfig)`: the same inputs compiled again with
    /// `testConfig` applied on top of the options.
    // port: tsc/internal/testutil/harnessutil/harnessutil.go:CompileFilesEx
    pub fn repeat(
        &self,
        test_config: &TestConfiguration,
        testdata: &TestData,
    ) -> Result<Compilation, CompileError> {
        let _ = (self, test_config, testdata);
        todo!("Repeat: SetOptionsFromTestConfig on clones of the options, then CompileFilesEx")
    }

    /// The writers' view (`newCompilationResult` over the recorded outputs):
    /// JS, DTS and map outputs ordered by the inputs, then by name.
    // port: tsc/internal/testutil/harnessutil/harnessutil.go:newCompilationResult
    pub fn result(&self) -> Result<baselines::CompilationResult<'_>, Failure> {
        todo!("new_compilation_result over ProgramFacts and the recorded outputs")
    }

    /// `GetSourceMapRecord`.
    pub fn source_map_record(&self) -> Result<Vec<u8>, Failure> {
        todo!("harness::sourcemap_record::get_source_map_record over result()")
    }
}

/// The defaults `CompileFiles` applies before the test configuration: the
/// tsconfig's options or none, `newLine` CRLF unless set,
/// `skipDefaultLibCheck` true unless set, `noErrorTruncation` true, and
/// harness options with case-sensitive names and `current_directory`; then
/// `SetOptionsFromTestConfig`. Shared with `list`, which decides skips from
/// these options without compiling.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:CompileFiles
pub fn resolve_options(
    test_config: Option<&TestConfiguration>,
    tsconfig: Option<&ParsedCommandLine>,
    current_directory: &[u8],
) -> Result<(CompilerOptions, HarnessOptions), Stop> {
    let _ = (test_config, tsconfig, current_directory);
    todo!("port the option half of CompileFiles")
}

/// `CompileFiles`: `resolve_options`, then `compile_files_ex`.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:CompileFiles
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
    let _ = (
        input_files,
        other_files,
        test_config,
        tsconfig,
        current_directory,
        symlinks,
        testdata,
        mode,
    );
    todo!("port CompileFiles")
}

/// `CompileFilesEx`: program file names (inputs minus `.json` and
/// `.tsbuildinfo`, plus `/.lib/<libFiles>`), the `/.lib` folder when an
/// input mentions it, absolute `outDir`/`project`/`rootDir`/
/// `tsBuildInfoFile`/`baseUrl`/`declarationDir`/`rootDirs`/`typeRoots`, the
/// in-memory file system under `BundledFs`, the content-mapper host when
/// `runExternalCode` and mappers are present, and `compileFilesWithHost`.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:CompileFilesEx
// port: tsc/internal/testutil/harnessutil/harnessutil.go:testLibFolderMap
// port: tsc/internal/testutil/harnessutil/harnessutil.go:createCompilerHost
// port: tsc/internal/testutil/harnessutil/harnessutil.go:compileFilesWithHost
pub fn compile_files_ex(inputs: Inputs, testdata: &TestData) -> Result<Compilation, CompileError> {
    let _ = (inputs, testdata);
    todo!("port CompileFilesEx and compileFilesWithHost")
}
