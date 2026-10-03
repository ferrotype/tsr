//! Which tests exist: `harnessutil.EnumerateFiles` over `tests/cases/<suite>`,
//! the compiler runner's skip list, and the variants (test × configuration)
//! with their configured names, which are the baseline file stems.
use crate::harness_options::NamedTestConfiguration;
use crate::{Stop, TestData};
use std::path::{Path, PathBuf};

// port: tsc/internal/testrunner/compiler_runner.go:CompilerTestType
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompilerTestType {
    Conformance,
    Regression,
}

impl CompilerTestType {
    /// `compiler` for regression tests, `conformance` otherwise: the suite
    /// name under `tests/cases` and `baselines/reference`.
    // port: tsc/internal/testrunner/compiler_runner.go:CompilerTestType.String
    pub fn suite_name(self) -> &'static str {
        match self {
            CompilerTestType::Regression => "compiler",
            CompilerTestType::Conformance => "conformance",
        }
    }

    pub fn from_suite_name(name: &str) -> Option<Self> {
        match name {
            "compiler" => Some(CompilerTestType::Regression),
            "conformance" => Some(CompilerTestType::Conformance),
            _ => None,
        }
    }
}

/// Tests the pin's runner never runs.
// port: tsc/internal/testrunner/compiler_runner.go:skippedTests
pub const SKIPPED_TESTS: &[&str] = &[
    "APILibCheck.ts",
    "APISample_Watch.ts",
    "APISample_WatchWithDefaults.ts",
    "APISample_WatchWithOwnWatchHost.ts",
    "APISample_compile.ts",
    "APISample_jsdoc.ts",
    "APISample_linter.ts",
    "APISample_parseConfig.ts",
    "APISample_transform.ts",
    "APISample_watcher.ts",
    "preserveUnusedImports.ts",
    "noCrashWithVerbatimModuleSyntaxAndImportsNotUsedAsValues.ts",
    "verbatimModuleSyntaxCompat.ts",
    "verbatimModuleSyntaxCompat2.ts",
    "verbatimModuleSyntaxCompat3.ts",
    "verbatimModuleSyntaxCompat4.ts",
    "preserveValueImports.ts",
    "preserveValueImports_importsNotUsedAsValues.ts",
    "preserveValueImports_errors.ts",
    "preserveValueImports_mixedImports.ts",
    "preserveValueImports_module.ts",
    "importsNotUsedAsValues_error.ts",
    "alwaysStrictNoImplicitUseStrict.ts",
    "nonPrimitiveIndexingWithForInSupressError.ts",
    "parameterInitializerBeforeDestructuringEmit.ts",
    "mappedTypeUnionConstraintInferences.ts",
    "lateBoundConstraintTypeChecksCorrectly.ts",
    "keyofDoesntContainSymbols.ts",
    "noStrictGenericChecks.ts",
    "noImplicitUseStrict_umd.ts",
    "noImplicitUseStrict_system.ts",
    "noImplicitUseStrict_es6.ts",
    "noImplicitUseStrict_commonjs.ts",
    "noImplicitAnyIndexingSuppressed.ts",
    "excessPropertyErrorsSuppressed.ts",
    "moduleNoneDynamicImport.ts",
    "moduleNoneErrors.ts",
    "noErrorUsingImportExportModuleAugmentationInDeclarationFile1.ts",
    "noErrorUsingImportExportModuleAugmentationInDeclarationFile2.ts",
    "noErrorUsingImportExportModuleAugmentationInDeclarationFile3.ts",
    "requireOfJsonFileWithModuleEmitNone.ts",
    "requireOfJsonFileWithModuleNodeResolutionEmitNone.ts",
];

/// One test × configuration: what one `run --id` process executes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    /// `compiler`, `conformance` or `transpile`.
    pub suite: &'static str,
    /// The test file, absolute.
    pub file: PathBuf,
    /// `tspath.GetBaseFileName(filename)`.
    pub basename: String,
    /// `basename` plus ` <configuration name>` when configured: the subtest
    /// name of `runTest`.
    pub test_name: String,
    /// `basename` with `(<configuration name>)` before the extension when
    /// configured: the baseline stem (`foo(target=es2015).ts`).
    pub configured_name: String,
    pub configuration: Option<NamedTestConfiguration>,
}

impl Variant {
    /// `<suite>/<configured name>`: the id `parity.py` sees.
    pub fn id(&self) -> String {
        crate::result::variant_id(self.suite, &self.configured_name)
    }
}

/// Files under `folder` whose normalized path matches `matches`, in the
/// pin's order (`os.ReadDir` sorts entries by name), recursively when asked.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:EnumerateFiles
// port: tsc/internal/testutil/harnessutil/harnessutil.go:listFiles
// port: tsc/internal/testutil/harnessutil/harnessutil.go:listFilesWorker
pub fn enumerate_files(
    folder: &Path,
    matches: &dyn Fn(&str) -> bool,
    recursive: bool,
) -> Result<Vec<PathBuf>, Stop> {
    let _ = (folder, matches, recursive);
    todo!("port EnumerateFiles")
}

/// `compilerBaselineRegex`: `\.tsx?$`.
pub fn is_compiler_test(path: &str) -> bool {
    path.ends_with(".ts") || path.ends_with(".tsx")
}

/// The test files of a compiler suite the pin's `RunTests` runs: every
/// `.ts`/`.tsx` under `tests/cases/<suite>`, minus `SKIPPED_TESTS`.
// port: tsc/internal/testrunner/compiler_runner.go:CompilerBaselineRunner.EnumerateTestFiles
// port: tsc/internal/testrunner/compiler_runner.go:CompilerBaselineRunner.RunTests
pub fn compiler_test_files(
    testdata: &TestData,
    kind: CompilerTestType,
) -> Result<Vec<PathBuf>, Stop> {
    let _ = (testdata, kind);
    todo!("EnumerateTestFiles minus skippedTests")
}

/// The variants of one compiler test file: `getCompilerFileBasedTest`'s
/// configurations, each named as `runTest` and `newCompilerTest` name them;
/// a file without configurations is one variant named after itself.
// port: tsc/internal/testrunner/compiler_runner.go:getCompilerFileBasedTest
// port: tsc/internal/testrunner/compiler_runner.go:CompilerBaselineRunner.runTest
pub fn compiler_variants(
    file: &Path,
    kind: CompilerTestType,
    content: &[u8],
) -> Result<Vec<Variant>, Stop> {
    let _ = (file, kind, content);
    todo!("configurations of one test file as variants")
}

/// `foo.ts` with configuration `target=es2015` is `foo(target=es2015).ts`.
// port: tsc/internal/testrunner/compiler_runner.go:newCompilerTest
pub fn configured_name(basename: &str, configuration: Option<&NamedTestConfiguration>) -> String {
    let _ = (basename, configuration);
    todo!("the configured baseline name")
}

/// Finds the variant with this id among a suite's files, reading only the
/// file the id names (its basename is the id's configured name without the
/// parenthesized configuration).
pub fn find_variant(testdata: &TestData, id: &str) -> Result<Option<Variant>, Stop> {
    let _ = (testdata, id);
    todo!("locate one variant by id")
}
