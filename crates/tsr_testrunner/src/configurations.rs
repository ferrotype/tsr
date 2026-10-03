//! `harnessutil.go`, the configuration half: which options a test may vary
//! (`// @strict: true, false`), the expansion into named configurations,
//! and the pin's skip rules for options the Go compiler does not support.
use crate::harness_options::{NamedTestConfiguration, TestConfiguration};
use crate::test_case_parser::RawCompilerSettings;
use crate::Stop;
use std::collections::BTreeSet;
use tsr_core::CompilerOptions;

/// The lowercased names of the options a compiler test may vary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VaryBy {
    pub options: BTreeSet<String>,
}

/// `getCompilerVaryByMap`: every non-command-line boolean or enum option
/// that affects program structure, emit, module resolution, bind or
/// semantic diagnostics, source files, declaration paths or build info,
/// plus `noEmit` and `isolatedModules`. The Rust option tables do not carry
/// all of those `Affects*` flags, so the set is a constant taken from the
/// pin's `tsoptions/declscompiler.go`, with a unit test that re-derives it
/// from that Go source text.
// port: tsc/internal/testrunner/compiler_runner.go:getCompilerVaryByMap
pub fn compiler_vary_by() -> &'static VaryBy {
    todo!("the constant vary-by set of the pin")
}

/// Expands `settings` into configurations: options in `vary_by` with more
/// than one value multiply (at most 25 variations, else fatal); the rest are
/// shared. With nothing varying but some settings, one unnamed configuration;
/// with no settings, none.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:GetFileBasedTestConfigurations
pub fn get_file_based_test_configurations(
    settings: &RawCompilerSettings,
    vary_by: &VaryBy,
) -> Result<Vec<NamedTestConfiguration>, Stop> {
    let _ = (settings, vary_by);
    todo!("port GetFileBasedTestConfigurations")
}

/// `key=value,key=value` over the sorted keys, values lowercased.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:getFileBasedTestConfigurationDescription
pub fn description(config: &TestConfiguration) -> String {
    let _ = config;
    todo!("port getFileBasedTestConfigurationDescription")
}

/// `esnext, es2015, es6` → the distinct values by normalized option value,
/// `*` → every value, `-x` / `!x` → exclusions (unknown exclusions are
/// skipped). An empty result is a panic in the pin, `Stop::Fatal` here.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:splitOptionValues
pub fn split_option_values(value: &str, option: &str) -> Result<Vec<String>, Stop> {
    let _ = (value, option);
    todo!("port splitOptionValues")
}

/// The cross product of the varying options' values, in the pin's order.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:computeFileBasedTestConfigurationVariations
pub fn compute_variations(option_entries: &[(String, Vec<String>)]) -> Vec<TestConfiguration> {
    let _ = option_entries;
    todo!("port computeFileBasedTestConfigurationVariations")
}

/// Fatal for `module: amd` and `outFile`; skip for `module: umd|system`,
/// `moduleResolution: node10|classic`, `esModuleInterop: false`,
/// `allowSyntheticDefaultImports: false`, a `baseUrl`, `target: es5` and
/// `alwaysStrict: false`.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:SkipUnsupportedCompilerOptions
// port: tsc/internal/testutil/harnessutil/harnessutil.go:failOnUnsupportedCompilerOptions
pub fn skip_unsupported_compiler_options(options: &CompilerOptions) -> Result<(), Stop> {
    let _ = options;
    todo!("port SkipUnsupportedCompilerOptions")
}
