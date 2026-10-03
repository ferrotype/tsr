//! `testrunner/transpile_runner.go`: the transpile suite. The per-unit
//! `transpileModule` / `transpileDeclaration` runs and the baseline text are
//! `crate::harness::transpile` (ported in Phase 3); this module owns the
//! enumeration, the configurations and the comparison.
use crate::baseline::Roots;
use crate::enumerate::Variant;
use crate::result::Report;
use crate::{Stop, TestData};
use std::path::{Path, PathBuf};

/// `transpileBaselineRegex`: `\.[cm]?[tj]sx?$`.
pub fn is_transpile_test(path: &str) -> bool {
    let _ = path;
    todo!("the transpile test file pattern")
}

/// `transpileVaryBy`: `declarationmap`, `sourcemap`, `inlinesourcemap`.
pub fn transpile_vary_by() -> &'static crate::configurations::VaryBy {
    todo!("the transpile vary-by set")
}

/// Every transpile test file under `tests/cases/transpile`.
// port: tsc/internal/testrunner/transpile_runner.go:TranspileBaselineRunner.EnumerateTestFiles
pub fn transpile_test_files(testdata: &TestData) -> Result<Vec<PathBuf>, Stop> {
    let _ = testdata;
    todo!("EnumerateFiles over tests/cases/transpile")
}

/// The variants of one transpile test: its configurations (or one unnamed
/// configuration of all settings), named `<name without extension>` plus
/// `(<formatted configuration>)`; the suite is `transpile`.
// port: tsc/internal/testrunner/transpile_runner.go:TranspileBaselineRunner.runTest
// port: tsc/internal/testrunner/transpile_runner.go:formatTranspileConfigurationName
pub fn transpile_variants(file: &Path, content: &[u8]) -> Result<Vec<Variant>, Stop> {
    let _ = (file, content);
    todo!("configurations of one transpile test as variants")
}

/// Runs one variant: `SetOptionsFromTestConfig` on empty options, then the
/// module kind unless `emitDeclarationOnly` and the declaration kind when
/// `declaration`, each compared with `transpile/<configured name>.js` /
/// `.d.ts` through `crate::baseline::run`. The sub-test names are `module`
/// and `declaration`.
// port: tsc/internal/testrunner/transpile_runner.go:TranspileBaselineRunner.runTest
// port: tsc/internal/testrunner/transpile_runner.go:TranspileBaselineRunner.runKind
pub fn run_transpile_test(variant: &Variant, content: &[u8], roots: &Roots, report: &mut Report) {
    let _ = (variant, content, roots, report);
    todo!("run the transpile kinds and compare")
}
