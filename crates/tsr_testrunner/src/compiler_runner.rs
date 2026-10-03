//! `testrunner/compiler_runner.go`: one compiler test variant from its units
//! to its nine sub-tests, each compared with the committed reference through
//! `crate::baseline`.
//!
//! Sub-test names are the pin's `t.Run` names: `error`, `content mapper`,
//! `output`, `sourcemap`, `sourcemap record`, `type`, `symbol`, `module
//! resolution`, `union ordering`, `source file parent pointers`. A sub-test
//! that panics is a failing sub-test naming the panic (`RecoverAndFail`).
use crate::baseline::Roots;
use crate::compile::{Compilation, TestFile};
use crate::enumerate::Variant;
use crate::harness_options::HarnessOptions;
use crate::result::Report;
use crate::test_case_parser::TestCaseContent;
use crate::{Mode, Stop, TestData};
use tsr_core::CompilerOptions;

/// What `newCompilerTest` settles before compiling: the units split into
/// the files passed on the command line and the other files on the file
/// system, the tsconfig unit, the current directory, and the options that
/// `SkipUnsupportedCompilerOptions` reads. `list` stops here.
pub struct Prepared {
    pub variant: Variant,
    pub content: TestCaseContent,
    pub current_directory: Vec<u8>,
    pub ts_config_files: Vec<TestFile>,
    pub to_be_compiled: Vec<TestFile>,
    pub other_files: Vec<TestFile>,
    pub has_non_dts_files: bool,
    pub options: CompilerOptions,
    pub harness_options: HarnessOptions,
}

/// `compilerTest` after `newCompilerTest`.
// port: tsc/internal/testrunner/compiler_runner.go:compilerTest
pub struct CompilerTest {
    pub prepared: Prepared,
    pub result: Compilation,
}

/// `makeUnitsFromTest`, the input split of `newCompilerTest` (a tsconfig
/// decides the roots; otherwise the last unit alone is compiled when
/// `noImplicitReferences` is set or it uses `require(` or a
/// `reference path`, else every unit), and `resolve_options`. Returns
/// `Stop::Skip` when `SkipUnsupportedCompilerOptions` would skip the test
/// and `Stop::Fatal` for its fatal cases and the parser's panics.
// port: tsc/internal/testrunner/compiler_runner.go:newCompilerTest
// port: tsc/internal/testrunner/compiler_runner.go:createHarnessTestFile
pub fn prepare(variant: Variant, content: &[u8]) -> Result<Prepared, Stop> {
    let _ = (variant, content);
    todo!("port the input assembly of newCompilerTest")
}

/// `CompileFiles` over a prepared variant, then the content-mapped units'
/// text is replaced by what the compiler parsed (`sf.Text()`).
// port: tsc/internal/testrunner/compiler_runner.go:newCompilerTest
pub fn compile(prepared: Prepared, testdata: &TestData, mode: Mode) -> Result<CompilerTest, Stop> {
    let _ = (prepared, testdata, mode);
    todo!("CompileFiles and the content-mapper text fix-up")
}

/// `runSingleConfigTest`: compile, then the nine `verify*` sub-tests in the
/// pin's order, each reported under the variant's id. A compile failure
/// fails every sub-test the pin would have run with the same reason.
// port: tsc/internal/testrunner/compiler_runner.go:CompilerBaselineRunner.runSingleConfigTest
pub fn run_single_config_test(
    prepared: Prepared,
    testdata: &TestData,
    roots: &Roots,
    mode: Mode,
    report: &mut Report,
) {
    let _ = (prepared, testdata, roots, mode, report);
    todo!("compile and run the verify* sub-tests")
}

/// Tests whose `output` sub-test the pin skips, with its reason.
// port: tsc/internal/testrunner/compiler_runner.go:skippedEmitTests
pub const SKIPPED_EMIT_TESTS: &[(&str, &str)] = &[
    (
        "filesEmittingIntoSameOutput.ts",
        "Output order nondeterministic due to collision on filename during parallel emit.",
    ),
    (
        "jsFileCompilationWithJsEmitPathSameAsInput.ts",
        "Output order nondeterministic due to collision on filename during parallel emit.",
    ),
    (
        "grammarErrors.ts",
        "Output order nondeterministic due to collision on filename during parallel emit.",
    ),
    (
        "jsFileCompilationEmitBlockedCorrectly.ts",
        "Output order nondeterministic due to collision on filename during parallel emit.",
    ),
    (
        "jsDeclarationsReexportAliasesEsModuleInterop.ts",
        "cls.d.ts is missing statements when run concurrently.",
    ),
    (
        "jsFileCompilationWithoutJsExtensions.ts",
        "No files are emitted.",
    ),
    (
        "typeOnlyMerge2.ts",
        "Nondeterministic contents when run concurrently.",
    ),
    (
        "typeOnlyMerge3.ts",
        "Nondeterministic contents when run concurrently.",
    ),
];

impl CompilerTest {
    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifyDiagnostics
    pub fn verify_diagnostics(&self, roots: &Roots, report: &mut Report) {
        let _ = (self, roots, report);
        todo!("DoErrorBaseline over tsConfigFiles + toBeCompiled + otherFiles minus content-mapped units")
    }

    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifyContentMapper
    pub fn verify_content_mapper(&self, roots: &Roots, report: &mut Report) {
        let _ = (self, roots, report);
        todo!("DoContentMapperBaseline")
    }

    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.contentMappedFileNames
    pub fn content_mapped_file_names(&self) -> Vec<Vec<u8>> {
        todo!("files produced by a content mapper")
    }

    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifyJavaScriptOutput
    pub fn verify_javascript_output(
        &self,
        roots: &Roots,
        testdata: &TestData,
        report: &mut Report,
    ) {
        let _ = (self, roots, testdata, report);
        todo!("DoJSEmitBaseline when the test has non-.d.ts files, skipping SKIPPED_EMIT_TESTS")
    }

    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifySourceMapOutput
    pub fn verify_source_map_output(
        &self,
        roots: &Roots,
        testdata: &TestData,
        report: &mut Report,
    ) {
        let _ = (self, roots, testdata, report);
        todo!("DoSourcemapBaseline")
    }

    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifySourceMapRecord
    pub fn verify_source_map_record(
        &self,
        roots: &Roots,
        testdata: &TestData,
        report: &mut Report,
    ) {
        let _ = (self, roots, testdata, report);
        todo!("DoSourcemapRecordBaseline")
    }

    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifyTypesAndSymbols
    pub fn verify_types_and_symbols(
        &self,
        roots: &Roots,
        testdata: &TestData,
        report: &mut Report,
    ) {
        let _ = (self, roots, testdata, report);
        todo!("DoTypeAndSymbolBaseline unless noTypesAndSymbols: the `type` and `symbol` sub-tests")
    }

    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifyModuleResolution
    pub fn verify_module_resolution(&self, roots: &Roots, report: &mut Report) {
        let _ = (self, roots, report);
        todo!("DoModuleResolutionBaseline when traceResolution")
    }

    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifyUnionOrdering
    pub fn verify_union_ordering(&self, report: &mut Report) {
        let _ = (self, report);
        todo!("harness::subtests::union_ordering over every checker of the program")
    }

    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifyParentPointers
    pub fn verify_parent_pointers(&self, report: &mut Report) {
        let _ = (self, report);
        todo!("harness::subtests::parent_pointers over every non-library source file")
    }
}

/// `tspath.GetPathComponentsRelativeTo(repo.TestDataPath(), filename)` joined
/// back: the header the emit, source-map and type baselines print
/// (`tests/cases/compiler/foo.ts`).
pub fn baseline_header(testdata: &TestData, file: &std::path::Path) -> Vec<u8> {
    let _ = (testdata, file);
    todo!("the test path relative to testdata")
}
