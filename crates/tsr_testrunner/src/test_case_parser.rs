//! `testrunner/test_case_parser.go`: a test file's `// @name: value`
//! directives, its `// @Filename:` units, `// @link:` and `// @symlink:`
//! lines, and the tsconfig unit parsed through the production config parser.
//!
//! Contents are bytes (Go strings); directive names are lowercased ASCII.
use crate::Stop;
use std::collections::BTreeMap;
use tsr_tsoptions::ParsedCommandLine;

/// `rawCompilerSettings`: a setting's value as written, by lowercased name
/// (`@target: esnext, es2015` maps `target` to `esnext, es2015`).
pub type RawCompilerSettings = BTreeMap<String, String>;

/// A unit of a multi-file test.
// port: tsc/internal/testrunner/test_case_parser.go:testUnit
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestUnit {
    pub name: Vec<u8>,
    pub content: Vec<u8>,
}

/// Everything needed to turn a multi-file test into units for compilation.
// port: tsc/internal/testrunner/test_case_parser.go:testCaseContent
#[derive(Debug)]
pub struct TestCaseContent {
    /// The units, without the tsconfig unit.
    pub units: Vec<TestUnit>,
    /// The parsed `tsconfig.json` / `jsconfig.json` unit, when the test has one.
    pub ts_config: Option<ParsedCommandLine>,
    pub ts_config_unit: Option<TestUnit>,
    /// Symlink → target, as written (relative names are resolved by the caller).
    pub symlinks: BTreeMap<Vec<u8>, Vec<u8>>,
}

// port: tsc/internal/testrunner/test_case_parser.go:ParseTestFilesOptions
#[derive(Clone, Copy, Debug, Default)]
pub struct ParseTestFilesOptions {
    /// Content before the first `@Filename` goes into an implicit first file
    /// named after the test (the fourslash harness's behavior).
    pub allow_implicit_first_file: bool,
}

/// What `ParseTestFilesAndSymlinksWithOptions` returns besides the units.
#[derive(Debug)]
pub struct ParsedTestFiles<T> {
    pub units: Vec<T>,
    pub symlinks: BTreeMap<Vec<u8>, Vec<u8>>,
    /// The `@currentDirectory` directive, empty when absent.
    pub current_directory: Vec<u8>,
    /// Every global `@option` by lowercased name.
    pub global_options: BTreeMap<String, String>,
}

/// Splits a test into named units and parses its tsconfig unit, if any,
/// with `parse_json_source_file_config_file_content` over a case-sensitive
/// in-memory host of all units (`tsoptionstest.NewVFSParseConfigHostWithSymlinks`),
/// relative to the `@currentDirectory` or [`crate::SRC_FOLDER`]. A top-level
/// `@runExternalCode: true` is passed as the existing options so content
/// mappers register. The pin's panics are `Stop::Fatal`.
// port: tsc/internal/testrunner/test_case_parser.go:makeUnitsFromTest
pub fn make_units_from_test(code: &[u8], file_name: &[u8]) -> Result<TestCaseContent, Stop> {
    let _ = (code, file_name);
    todo!("port makeUnitsFromTest")
}

/// The line-by-line directive parser. `parse_file(name, content, file_options)`
/// builds one unit; the fourslash-only file options are `emitthisfile` and
/// `noopen`. Lines are split on `\r?\n`; a unit's content joins its lines
/// with `\n`, dropping leading blank lines unless `allow_implicit_first_file`.
// port: tsc/internal/testrunner/test_case_parser.go:ParseTestFilesAndSymlinks
// port: tsc/internal/testrunner/test_case_parser.go:ParseTestFilesAndSymlinksWithOptions
pub fn parse_test_files_and_symlinks<T>(
    code: &[u8],
    file_name: &[u8],
    parse_file: impl FnMut(&[u8], &[u8], &BTreeMap<String, String>) -> Result<T, Stop>,
    options: ParseTestFilesOptions,
) -> Result<ParsedTestFiles<T>, Stop> {
    let _ = (code, file_name, parse_file, options);
    todo!("port ParseTestFilesAndSymlinksWithOptions")
}

/// Every `// @name: value` of the file by lowercased name, values trimmed
/// and stripped of a trailing `;`.
// port: tsc/internal/testrunner/test_case_parser.go:extractCompilerSettings
pub fn extract_compiler_settings(content: &[u8]) -> RawCompilerSettings {
    let _ = content;
    todo!("port extractCompilerSettings")
}

/// `// @link: <target> -> <link>` adds `link → target`; returns whether the
/// line was one.
// port: tsc/internal/testrunner/test_case_parser.go:parseSymlinkFromTest
pub fn parse_symlink_from_test(line: &[u8], symlinks: &mut BTreeMap<Vec<u8>, Vec<u8>>) -> bool {
    let _ = (line, symlinks);
    todo!("port parseSymlinkFromTest")
}
