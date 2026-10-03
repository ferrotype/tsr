//! `testrunner/transpile_runner.go`: the transpile suite. The per-unit
//! `transpileModule` / `transpileDeclaration` runs and the baseline text are
//! `crate::harness::transpile` (ported in Phase 3); this module owns the
//! enumeration, the configurations and the comparison.
//!
//! The pin runs both kinds of a configuration inside one `t.Run`; here they
//! are the sub-tests `module` and `declaration` of the configuration's
//! variant. A kind that stops (the pin's `t.Fatal`, or a panic) fails, and
//! so does the declaration kind it keeps from running.
use crate::baseline::{self, Roots};
use crate::compiler_runner::panic_message;
use crate::configurations::VaryBy;
use crate::enumerate::Variant;
use crate::harness::transpile;
use crate::harness_options::{HarnessOptions, NamedTestConfiguration, TestConfiguration};
use crate::result::{Outcome, Report};
use crate::{Stop, TestData, SRC_FOLDER};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tsr_core::CompilerOptions;

/// The sub-test of the module kind.
const MODULE: &str = "module";
/// The sub-test of the declaration kind.
const DECLARATION: &str = "declaration";
/// Where a variant stops before its kinds are known.
const TEST: &str = "test";

/// `transpileBaselineRegex`: `\.[cm]?[tj]sx?$`.
pub fn is_transpile_test(path: &str) -> bool {
    let path = path.as_bytes();
    // `$` anchors the match, so a trailing `x` can only be the optional one.
    let path = path.strip_suffix(b"x").unwrap_or(path);
    let Some(path) = path.strip_suffix(b"s") else {
        return false;
    };
    let Some((b't' | b'j', path)) = path.split_last() else {
        return false;
    };
    match path.split_last() {
        Some((b'.', _)) => true,
        Some((b'c' | b'm', path)) => path.last() == Some(&b'.'),
        _ => false,
    }
}

/// `transpileVaryBy`: `declarationmap`, `sourcemap`, `inlinesourcemap`.
pub fn transpile_vary_by() -> &'static VaryBy {
    static VARY_BY: OnceLock<VaryBy> = OnceLock::new();
    VARY_BY.get_or_init(|| VaryBy {
        options: ["declarationmap", "sourcemap", "inlinesourcemap"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    })
}

/// Every transpile test file under `tests/cases/transpile`.
// port: tsc/internal/testrunner/transpile_runner.go:TranspileBaselineRunner.EnumerateTestFiles
pub fn transpile_test_files(testdata: &TestData) -> Result<Vec<PathBuf>, Stop> {
    crate::enumerate::enumerate_files(&testdata.cases("transpile"), &is_transpile_test, true)
}

/// The variants of one transpile test: its configurations (or one unnamed
/// configuration of all settings), named `<name without extension>` plus
/// `(<formatted configuration>)`; the suite is `transpile`.
// port: tsc/internal/testrunner/transpile_runner.go:TranspileBaselineRunner.runTest
// port: tsc/internal/testrunner/transpile_runner.go:formatTranspileConfigurationName
pub fn transpile_variants(file: &Path, content: &[u8]) -> Result<Vec<Variant>, Stop> {
    let settings = crate::test_case_parser::extract_compiler_settings(content);
    let mut configurations =
        crate::configurations::get_file_based_test_configurations(&settings, transpile_vary_by())?;
    if configurations.is_empty() {
        configurations = vec![NamedTestConfiguration {
            name: String::new(),
            config: settings,
        }];
    }
    let file_name = path_bytes(file);
    let base_name = String::from_utf8_lossy(tsr_tspath::base_name(&file_name)).into_owned();
    Ok(configurations
        .into_iter()
        .map(|configuration| {
            let configured_name = configured_name(&file_name, &configuration.name);
            Variant {
                suite: "transpile",
                file: file.to_path_buf(),
                basename: base_name.clone(),
                test_name: configured_name.clone(),
                configured_name,
                configuration: Some(configuration),
            }
        })
        .collect())
}

/// `runTest`'s configured name: the base name without its extension, plus
/// the formatted configuration name in parentheses when it has one.
fn configured_name(file_name: &[u8], configuration_name: &str) -> String {
    let (configured_name, _) = transpile::configured_name(file_name, configuration_name.as_bytes());
    String::from_utf8_lossy(&configured_name).into_owned()
}

/// Runs one variant: `SetOptionsFromTestConfig` on empty options, then the
/// module kind unless `emitDeclarationOnly` and the declaration kind when
/// `declaration`, each compared with `transpile/<configured name>.js` /
/// `.d.ts` through `crate::baseline::run`. The sub-test names are `module`
/// and `declaration`; a failure before the kinds are known is reported
/// under `test`.
// port: tsc/internal/testrunner/transpile_runner.go:TranspileBaselineRunner.runTest
// port: tsc/internal/testrunner/transpile_runner.go:TranspileBaselineRunner.runKind
pub fn run_transpile_test(variant: &Variant, content: &[u8], roots: &Roots, report: &mut Report) {
    let id = variant.id();
    let file_name = path_bytes(&variant.file);
    let base_name = tsr_tspath::base_name(&file_name);
    let units = match crate::test_case_parser::make_units_from_test(content, base_name) {
        Ok(test_content) => test_content.units,
        Err(stop) => {
            report.subtest(&id, TEST, stop.into());
            return;
        }
    };
    let units: Vec<transpile::Unit> = units
        .into_iter()
        .map(|unit| transpile::Unit {
            name: unit.name,
            content: unit.content,
        })
        .collect();

    let mut options = CompilerOptions::default();
    let mut harness_options = HarnessOptions::default();
    let (name, config) = match &variant.configuration {
        Some(configuration) => (configuration.name.as_str(), configuration.config.clone()),
        None => ("", TestConfiguration::default()),
    };
    if let Err(stop) = crate::harness_options::set_options_from_test_config(
        &config,
        &mut options,
        &mut harness_options,
        SRC_FOLDER,
        false,
    ) {
        report.subtest(&id, TEST, stop.into());
        return;
    }

    let configuration = transpile::Configuration {
        file: &file_name,
        name: name.as_bytes(),
        units: &units,
        options: &options,
        report_diagnostics: harness_options.report_diagnostics,
    };
    let mut stopped: Option<String> = None;
    for declaration in transpile::kinds(&options) {
        let subtest = if declaration { DECLARATION } else { MODULE };
        if let Some(reason) = &stopped {
            report.subtest(
                &id,
                subtest,
                Outcome::fail(format!(
                    "not run: the module kind stopped the test: {reason}"
                )),
            );
            continue;
        }
        let run = catch_unwind(AssertUnwindSafe(|| {
            transpile::run_one(&configuration, declaration)
        }));
        let outcome = match run {
            // `baseline.Run(t, "transpile/"+baselineName, result, baseline.Options{})`.
            Ok(Ok(run)) => baseline::run(
                roots,
                &run.baseline_name,
                &run.baseline,
                baseline::Options::default(),
            ),
            Ok(Err(reason)) => {
                stopped = Some(reason.clone());
                Outcome::fail(reason)
            }
            Err(payload) => {
                let reason = format!(
                    "panic in the {subtest} kind: {}",
                    panic_message(payload.as_ref())
                );
                stopped = Some(reason.clone());
                Outcome::fail(reason)
            }
        };
        report.subtest(&id, subtest, outcome);
    }
}

/// A path's bytes with forward slashes (`tspath.NormalizeSlashes`).
fn path_bytes(path: &Path) -> Vec<u8> {
    tsr_tspath::normalize_slashes(path.as_os_str().as_encoded_bytes()).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{configured_name, is_transpile_test, transpile_vary_by};

    #[test]
    fn transpile_tests_match_the_pinned_pattern() {
        for path in [
            "/t/a.ts",
            "/t/a.tsx",
            "/t/a.js",
            "/t/a.jsx",
            "/t/a.mts",
            "/t/a.cts",
            "/t/a.mjs",
            "/t/a.cjs",
            "/t/a.d.ts",
            "/t/a.mtsx",
            "/t/a.cjsx",
        ] {
            assert!(is_transpile_test(path), "{path}");
        }
        for path in [
            "/t/a.json",
            "/t/a.txt",
            "/t/a.xts",
            "/t/a.tss",
            "/t/a.ts.map",
            "/t/ats",
            "/t/a.s",
            "/t/a.x",
            "/t/a.mmts",
            "ts",
            "",
        ] {
            assert!(!is_transpile_test(path), "{path}");
        }
    }

    #[test]
    fn configured_names_drop_the_extension_and_camel_case_the_varied_options() {
        assert_eq!(
            configured_name(b"/t/declarationBasicSyntax.ts", ""),
            "declarationBasicSyntax"
        );
        assert_eq!(
            configured_name(b"/t/declarationBasicSyntax.ts", "declarationmap=true"),
            "declarationBasicSyntax(declarationMap=true)"
        );
        assert_eq!(
            configured_name(
                b"/t/a.d.mts",
                "declarationmap=false,inlinesourcemap=true,sourcemap=false"
            ),
            "a.d(declarationMap=false,inlineSourceMap=true,sourceMap=false)"
        );
    }

    #[test]
    fn the_vary_by_set_is_the_three_map_options() {
        let options: Vec<&str> = transpile_vary_by()
            .options
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(options, ["declarationmap", "inlinesourcemap", "sourcemap"]);
    }
}
