//! Which tests exist: `harnessutil.EnumerateFiles` over `tests/cases/<suite>`,
//! the compiler runner's skip list, and the variants (test × configuration)
//! with their configured names, which are the baseline file stems.
use crate::configurations::{compiler_vary_by, get_file_based_test_configurations};
use crate::harness_options::NamedTestConfiguration;
use crate::test_case_parser::extract_compiler_settings;
use crate::{Stop, TestData};
use std::path::{Path, PathBuf};
use tsr_jsstring::SourceText;

// source: tsc/internal/testrunner/compiler_runner.go:CompilerTestType
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
// source: tsc/internal/testrunner/compiler_runner.go:skippedTests
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
/// Paths are normalized with forward slashes. A relative `folder` stays
/// relative (the pin resolves it against the test data directory). An
/// entry is a directory only if it is one itself, not a symlink to one
/// (Go's `DirEntry.IsDir`).
// port: tsc/internal/testutil/harnessutil/harnessutil.go:EnumerateFiles
pub fn enumerate_files(
    folder: &Path,
    matches: &dyn Fn(&str) -> bool,
    recursive: bool,
) -> Result<Vec<PathBuf>, Stop> {
    let files = list_files(path_text(folder)?, matches, recursive)?;
    // `listFilesWorker` already normalized every path, slashes included.
    Ok(files.into_iter().map(PathBuf::from).collect())
}

// port: tsc/internal/testutil/harnessutil/harnessutil.go:listFiles
fn list_files(
    path: &str,
    spec: &dyn Fn(&str) -> bool,
    recursive: bool,
) -> Result<Vec<String>, Stop> {
    list_files_worker(spec, recursive, path)
}

// port: tsc/internal/testutil/harnessutil/harnessutil.go:listFilesWorker
fn list_files_worker(
    spec: &dyn Fn(&str) -> bool,
    recursive: bool,
    folder: &str,
) -> Result<Vec<String>, Stop> {
    let folder = normalized(&tsr_tspath::absolute(folder.as_bytes(), b""));
    let read_error = |error: std::io::Error| Stop::fatal(format!("open {folder}: {error}"));
    let mut entries = std::fs::read_dir(&folder)
        .map_err(read_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(read_error)?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut paths = Vec::new();
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            Stop::fatal(format!(
                "{folder}: a file name is not UTF-8: {}",
                name.display()
            ))
        })?;
        let path = normalized(&tsr_tspath::normalize(
            format!("{folder}/{name}").as_bytes(),
        ));
        if !entry.file_type().map_err(read_error)?.is_dir() {
            if spec(&path) {
                paths.push(path);
            }
        } else if recursive {
            paths.extend(list_files_worker(spec, recursive, &path)?);
        }
    }
    Ok(paths)
}

/// A path the enumeration produced from UTF-8 parts.
fn normalized(path: &[u8]) -> String {
    String::from_utf8(path.to_vec()).expect("normalizing a UTF-8 path keeps it UTF-8")
}

fn path_text(path: &Path) -> Result<&str, Stop> {
    path.to_str()
        .ok_or_else(|| Stop::fatal(format!("{}: the path is not UTF-8", path.display())))
}

/// `tspath.GetBaseFileName` of a UTF-8 path.
fn base_name(path: &str) -> &str {
    std::str::from_utf8(tsr_tspath::base_name(path.as_bytes()))
        .expect("a base name ends at an ASCII separator")
}

/// `compilerBaselineRegex`: `\.tsx?$`.
#[allow(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "the pin's expression is case-sensitive: `foo.TS` is not a test"
)]
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
    let files = enumerate_files(&testdata.cases(kind.suite_name()), &is_compiler_test, true)
        .map_err(|stop| match stop {
            Stop::Fatal(message) => {
                Stop::fatal(format!("Could not read compiler test files: {message}"))
            }
            skip @ Stop::Skip(_) => skip,
        })?;
    Ok(files
        .into_iter()
        .filter(|file| {
            file.to_str()
                .is_none_or(|file| !SKIPPED_TESTS.contains(&base_name(file)))
        })
        .collect())
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
    let settings = extract_compiler_settings(content);
    let configurations = get_file_based_test_configurations(&settings, compiler_vary_by())?;
    let basename = base_name(path_text(file)?).to_string();
    let variant = |configuration: Option<NamedTestConfiguration>| {
        let mut test_name = basename.clone();
        if let Some(configuration) = &configuration {
            if !configuration.name.is_empty() {
                test_name.push(' ');
                test_name.push_str(&configuration.name);
            }
        }
        Variant {
            suite: kind.suite_name(),
            file: file.to_path_buf(),
            basename: basename.clone(),
            test_name,
            configured_name: configured_name(&basename, configuration.as_ref()),
            configuration,
        }
    };
    if configurations.is_empty() {
        return Ok(vec![variant(None)]);
    }
    Ok(configurations.into_iter().map(Some).map(variant).collect())
}

/// `foo.ts` with configuration `target=es2015` is `foo(target=es2015).ts`.
// port: tsc/internal/testrunner/compiler_runner.go:newCompilerTest
pub fn configured_name(basename: &str, configuration: Option<&NamedTestConfiguration>) -> String {
    match configuration {
        Some(configuration) if !configuration.name.is_empty() => {
            let extname =
                tsr_tspath::any_extension_from_path::<&[u8]>(basename.as_bytes(), &[], false);
            let (extensionless_basename, extname) =
                basename.split_at(basename.len() - extname.len());
            format!("{extensionless_basename}({}){extname}", configuration.name)
        }
        _ => basename.to_string(),
    }
}

/// The configured name without its parenthesized configuration:
/// `foo(target=es2015).ts` is `foo.ts`, `foo(sourceMap=true)` is `foo`.
fn unconfigured_name(configured: &str) -> String {
    match (configured.find('('), configured.rfind(')')) {
        (Some(open), Some(close)) if open < close => {
            format!("{}{}", &configured[..open], &configured[close + 1..])
        }
        _ => configured.to_string(),
    }
}

/// A test file's text as the pin's runners read it, `osvfs.FS().ReadFile`:
/// a UTF-16 file (by its byte order mark) is decoded to UTF-8 and a UTF-8
/// byte order mark is dropped. `extract_compiler_settings`,
/// `compiler_variants` and `make_units_from_test` take this text, not the
/// raw bytes: 839 compiler and conformance tests start with a byte order
/// mark (7 of them UTF-16), which hides a first-line directive.
pub fn read_test_file(file: &Path) -> Result<Vec<u8>, Stop> {
    read_file(file, "test file")
}

fn read_file(file: &Path, what: &str) -> Result<Vec<u8>, Stop> {
    let bytes = std::fs::read(file).map_err(|error| {
        Stop::fatal(format!(
            "Could not read {what}: {}: {error}",
            file.display()
        ))
    })?;
    Ok(SourceText::from_bytes(bytes).as_bytes().to_vec())
}

/// Finds the variant with this id among a suite's files, reading only the
/// file the id names (its basename is the id's configured name without the
/// parenthesized configuration; a name that itself has parentheses is also
/// tried as written). Compiler tests the pin skips by name have no
/// variants. A transpile id names a file by its name without extension.
pub fn find_variant(testdata: &TestData, id: &str) -> Result<Option<Variant>, Stop> {
    let Some((suite, configured)) = id.split_once('/') else {
        return Ok(None);
    };
    let unconfigured = unconfigured_name(configured);
    let names_file = |name: &str| name == unconfigured || name == configured;
    if suite == "transpile" {
        let files = enumerate_files(
            &testdata.cases(suite),
            &|path| {
                crate::transpile_runner::is_transpile_test(path) && {
                    let basename = base_name(path);
                    let extension = tsr_tspath::any_extension_from_path::<&[u8]>(
                        basename.as_bytes(),
                        &[],
                        false,
                    );
                    names_file(&basename[..basename.len() - extension.len()])
                }
            },
            true,
        )?;
        for file in files {
            let content = read_file(&file, "transpile test file")?;
            let variants = crate::transpile_runner::transpile_variants(&file, &content)?;
            if let Some(variant) = variants.into_iter().find(|variant| variant.id() == id) {
                return Ok(Some(variant));
            }
        }
        return Ok(None);
    }
    let Some(kind) = CompilerTestType::from_suite_name(suite) else {
        return Ok(None);
    };
    let files = enumerate_files(
        &testdata.cases(suite),
        &|path| {
            let basename = base_name(path);
            is_compiler_test(path) && names_file(basename) && !SKIPPED_TESTS.contains(&basename)
        },
        true,
    )?;
    for file in files {
        let content = read_test_file(&file)?;
        if let Some(variant) = compiler_variants(&file, kind, &content)?
            .into_iter()
            .find(|variant| variant.id() == id)
        {
            return Ok(Some(variant));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::{
        compiler_test_files, compiler_variants, configured_name, enumerate_files, find_variant,
        is_compiler_test, read_test_file, CompilerTestType, Variant,
    };
    use crate::harness_options::NamedTestConfiguration;
    use crate::TestData;
    use std::collections::BTreeMap;
    use std::path::Path;

    fn testdata() -> TestData {
        TestData::in_repository(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
    }

    fn named(name: &str) -> NamedTestConfiguration {
        NamedTestConfiguration {
            name: name.to_string(),
            config: BTreeMap::new(),
        }
    }

    #[test]
    fn configured_names_put_the_configuration_before_the_extension() {
        assert_eq!(configured_name("foo.ts", None), "foo.ts");
        assert_eq!(configured_name("foo.ts", Some(&named(""))), "foo.ts");
        assert_eq!(
            configured_name("foo.ts", Some(&named("target=es2015"))),
            "foo(target=es2015).ts"
        );
        assert_eq!(
            configured_name("foo.d.tsx", Some(&named("jsx=preserve,strict=true"))),
            "foo.d(jsx=preserve,strict=true).tsx"
        );
        assert_eq!(configured_name("noext", Some(&named("a=b"))), "noext(a=b)");
    }

    #[test]
    fn a_configured_test_has_one_variant_per_configuration() {
        let file = testdata()
            .cases("compiler")
            .join("abstractPropertyBasics.ts");
        let content = read_test_file(&file).expect("pinned test file");
        let variants: Vec<Variant> =
            compiler_variants(&file, CompilerTestType::Regression, &content).expect("variants");
        let names: Vec<(&str, &str, String)> = variants
            .iter()
            .map(|variant| {
                (
                    variant.test_name.as_str(),
                    variant.configured_name.as_str(),
                    variant.id(),
                )
            })
            .collect();
        assert_eq!(
            names,
            // `//@target: ES5, ES2015`: the names lowercase the values.
            vec![
                (
                    "abstractPropertyBasics.ts target=es5",
                    "abstractPropertyBasics(target=es5).ts",
                    "compiler/abstractPropertyBasics(target=es5).ts".to_string()
                ),
                (
                    "abstractPropertyBasics.ts target=es2015",
                    "abstractPropertyBasics(target=es2015).ts",
                    "compiler/abstractPropertyBasics(target=es2015).ts".to_string()
                ),
            ]
        );
        assert!(variants
            .iter()
            .all(|variant| variant.basename == "abstractPropertyBasics.ts"));
        assert_eq!(
            variants[0]
                .configuration
                .as_ref()
                .map(|c| c.config["target"].as_str()),
            Some("ES5")
        );
    }

    #[test]
    fn enumeration_is_sorted_normalized_and_filtered() {
        let folder = testdata().cases("conformance");
        let seen = std::cell::RefCell::new(Vec::new());
        let files = enumerate_files(
            &folder.join("./types/../types"),
            &|path| {
                seen.borrow_mut().push(path.to_string());
                Path::new(path).extension().is_some_and(|ext| ext == "tsx")
            },
            true,
        )
        .expect("enumerates");
        assert!(!files.is_empty());
        assert!(seen.borrow().len() > files.len());
        for file in &files {
            let text = file.to_str().expect("UTF-8");
            assert!(file.extension().is_some_and(|ext| ext == "tsx"));
            assert!(!text.contains("/./") && !text.contains("/../") && !text.contains("//"));
            assert!(seen.borrow().iter().any(|path| path == text));
        }
        let top = enumerate_files(&folder, &|_| true, false).expect("enumerates");
        assert!(top
            .iter()
            .any(|file| file.ends_with("conformance/simpleTest.ts")));
        assert!(top.iter().all(|file| file
            .parent()
            .is_some_and(|parent| parent.ends_with("cases/conformance"))));
        let mut sorted = top.clone();
        sorted.sort();
        assert_eq!(top, sorted);
        assert!(enumerate_files(&folder.join("missing"), &|_| true, true).is_err());
    }

    #[test]
    fn find_variant_locates_nested_configured_tests() {
        let testdata = testdata();
        let found = find_variant(&testdata, "compiler/abstractPropertyBasics(target=es5).ts")
            .expect("finds")
            .expect("variant");
        assert_eq!(
            found.configured_name,
            "abstractPropertyBasics(target=es5).ts"
        );
        let nested = find_variant(&testdata, "conformance/asyncAwaitIsolatedModules_es2017.ts")
            .expect("finds")
            .expect("nested variant");
        assert!(nested
            .file
            .to_str()
            .expect("UTF-8")
            .ends_with("/conformance/async/es2017/asyncAwaitIsolatedModules_es2017.ts"));
        assert!(nested.configuration.is_some());
        // Transpile names drop the extension.
        for id in [
            "transpile/declarationBasicSyntax(declarationMap=true)",
            "transpile/declarationAsyncAndGeneratorFunctions",
        ] {
            let found = find_variant(&testdata, id).expect("finds").expect(id);
            assert_eq!(found.id(), id);
            assert_eq!(found.suite, "transpile");
        }
        for missing in [
            "compiler/abstractPropertyBasics(target=es3).ts",
            "compiler/noSuchTest.ts",
            "conformance/abstractPropertyBasics(target=es5).ts",
            "compiler/APILibCheck.ts",
            "nosuite/abstractPropertyBasics.ts",
            "abstractPropertyBasics.ts",
            "transpile/declarationBasicSyntax.ts",
        ] {
            assert_eq!(find_variant(&testdata, missing), Ok(None), "{missing}");
        }
    }

    /// The variants of every compiler and conformance test, in the pin's
    /// enumeration order, against the Phase 2 inventory of the same pin
    /// (`data/phase2/inventory.json`), whose rows a Go producer enumerated:
    /// every row but the 52 `skippedTests` and the 2 `.js` files the
    /// enumeration never sees.
    #[test]
    fn variants_match_the_phase2_inventory() {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let inventory: serde_json::Value = serde_json::from_slice(
            &std::fs::read(repository.join("data/phase2/inventory.json")).expect("inventory"),
        )
        .expect("inventory JSON");
        let mut expected: Vec<(String, String)> = Vec::new();
        for row in inventory["rows"].as_array().expect("rows") {
            if matches!(
                row["informational_reason"].as_str(),
                Some("filename_skip" | "not_enumerated")
            ) {
                continue;
            }
            let path = row["path"].as_str().expect("path");
            let suite = row["suite"].as_str().expect("suite");
            let configured = row["configured_name"].as_str().expect("configured name");
            expected.push((path.to_string(), format!("{suite}/{configured}")));
        }

        let testdata = testdata();
        let upstream = repository.join("upstream");
        let upstream = upstream.canonicalize().expect("upstream");
        let mut actual: Vec<(String, String)> = Vec::new();
        for kind in [CompilerTestType::Regression, CompilerTestType::Conformance] {
            for file in compiler_test_files(&testdata, kind).expect("files") {
                let relative = file
                    .canonicalize()
                    .expect("test file")
                    .strip_prefix(&upstream)
                    .expect("under upstream")
                    .to_str()
                    .expect("UTF-8")
                    .to_string();
                assert!(is_compiler_test(&relative));
                let content = read_test_file(&file).expect("test file");
                let mut variants: Vec<String> = compiler_variants(&file, kind, &content)
                    .expect("variants")
                    .iter()
                    .map(Variant::id)
                    .collect();
                // The pin's configuration order is random; the inventory's is its own.
                variants.sort();
                actual.extend(variants.into_iter().map(|id| (relative.clone(), id)));
            }
        }
        let mut expected_by_file = expected.clone();
        expected_by_file.sort();
        let mut actual_by_file = actual.clone();
        actual_by_file.sort();
        assert_eq!(actual.len(), expected.len());
        assert_eq!(actual_by_file, expected_by_file);
        // The file order is the pin's: os.ReadDir order, depth first.
        let files = |rows: &[(String, String)]| {
            let mut files: Vec<String> = rows.iter().map(|(file, _)| file.clone()).collect();
            files.dedup();
            files
        };
        assert_eq!(files(&actual), files(&expected));
    }
}
