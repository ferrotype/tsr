//! `testrunner/test_case_parser.go`: a test file's `// @name: value`
//! directives, its `// @Filename:` units, `// @link:` and `// @symlink:`
//! lines, and the tsconfig unit parsed through the production config parser.
//!
//! Contents are bytes (Go strings); directive names are lowercased ASCII.
//! The pin's two regular expressions are matched by hand over bytes with the
//! same leftmost-first semantics (`option_match`, `link_match`), including
//! the RE2 classes: `\s` is `[\t\n\f\r ]` and may cross a line break, `\w`
//! is `[0-9A-Za-z_]`.
use crate::harness::config_host::Host;
use crate::harness_options::get_config_name_from_file_name;
use crate::Stop;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;
use tsr_core::{CompilerOptions, Tristate};
use tsr_jsstring::wtf8::{decode_utf8, RUNE_ERROR};
use tsr_jsstring::{helpers::to_lower_go, JsString, SourceText};
use tsr_tsoptions::{ConfigValue, ParsedCommandLine, TsConfigSourceFile};

/// `rawCompilerSettings`: a setting's value as written, by lowercased name
/// (`@target: esnext, es2015` maps `target` to `esnext, es2015`).
pub type RawCompilerSettings = BTreeMap<String, String>;

/// A unit of a multi-file test.
// source: tsc/internal/testrunner/test_case_parser.go:testUnit
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestUnit {
    pub name: Vec<u8>,
    pub content: Vec<u8>,
}

/// Everything needed to turn a multi-file test into units for compilation.
// source: tsc/internal/testrunner/test_case_parser.go:testCaseContent
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

// source: tsc/internal/testrunner/test_case_parser.go:ParseTestFilesOptions
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

/// File-specific directives used by fourslash tests.
// source: tsc/internal/testrunner/test_case_parser.go:fourslashDirectives
const FOURSLASH_DIRECTIVES: [&str; 2] = ["emitthisfile", "noopen"];

/// The pin's panic when a compiler test has code before its first unit.
pub const CONTENT_BEFORE_FIRST_FILENAME: &str =
    "Non-comment test content appears before the first '// @Filename' directive";

/// Splits a test into named units and parses its tsconfig unit, if any,
/// with `parse_json_source_file_config_file_content` over a case-sensitive
/// in-memory host of all units (`tsoptionstest.NewVFSParseConfigHostWithSymlinks`),
/// relative to the `@currentDirectory` or [`crate::SRC_FOLDER`]. A top-level
/// `@runExternalCode: true` is passed as the existing options so content
/// mappers register. The pin's panics are `Stop::Fatal`.
///
/// The pin builds the host before looking for a config unit; here it is
/// built only when one is found, its only reader.
// port: tsc/internal/testrunner/test_case_parser.go:makeUnitsFromTest
pub fn make_units_from_test(code: &[u8], file_name: &[u8]) -> Result<TestCaseContent, Stop> {
    let ParsedTestFiles {
        units: mut test_units,
        symlinks,
        current_directory,
        global_options,
    } = parse_test_files_and_symlinks(
        code,
        file_name,
        |name, content, _| {
            Ok(TestUnit {
                name: name.to_vec(),
                content: content.to_vec(),
            })
        },
        ParseTestFilesOptions::default(),
    )?;

    let current_directory = if current_directory.is_empty() {
        crate::SRC_FOLDER.to_vec()
    } else {
        current_directory
    };

    // Content mappers are gated behind --runExternalCode, a command-line-only
    // option; a test opts in with a top-level `// @runExternalCode: true`.
    let existing_options = if global_options
        .get("runexternalcode")
        .is_some_and(|value| value == "true")
    {
        CompilerOptions {
            run_external_code: Tristate::TRUE,
            ..CompilerOptions::default()
        }
    } else {
        CompilerOptions::default()
    };

    // check if project has tsconfig.json in the list of files
    let mut ts_config = None;
    let mut ts_config_unit = None;
    if let Some(index) = test_units
        .iter()
        .position(|unit| !get_config_name_from_file_name(&unit.name).is_empty())
    {
        let host = parse_config_host(&test_units, &symlinks, &current_directory);
        let data = test_units.remove(index);
        let config_file_name = tsr_tspath::absolute(&data.name, &current_directory);
        let path = tsr_tspath::to_path(&data.name, host.cwd.as_bytes(), true);
        let source = TsConfigSourceFile::parse(
            JsString::from_bytes(config_file_name.clone()),
            path,
            SourceText::from_loaded_bytes(data.content.clone()),
        );
        let config_dir = tsr_tspath::directory(&config_file_name);
        let parsed = tsr_tsoptions::parse_json_source_file_config_file_content(
            source,
            &host,
            &config_dir,
            &existing_options,
            &ConfigValue::Null,
            &config_file_name,
        )
        .map_err(|error| {
            Stop::fatal(format!(
                "Could not parse {}: {error}",
                String::from_utf8_lossy(&config_file_name)
            ))
        })?;
        ts_config = Some(parsed);
        ts_config_unit = Some(data);
    }

    Ok(TestCaseContent {
        units: test_units,
        ts_config,
        ts_config_unit,
        symlinks,
    })
}

/// `NewVFSParseConfigHostWithSymlinks(allFiles, symlinks, currentDirectory,
/// true)`: every unit at its normalized absolute name (a later unit of the
/// same name wins, as in the pin's map), then every symlink.
fn parse_config_host(
    units: &[TestUnit],
    symlinks: &BTreeMap<Vec<u8>, Vec<u8>>,
    current_directory: &[u8],
) -> Host {
    let mut fs = tsr_vfs::MemoryBuilder::new(current_directory, true);
    for unit in units {
        fs.insert_physical(
            &tsr_tspath::absolute(&unit.name, current_directory),
            unit.content.clone(),
        );
    }
    for (link, target) in symlinks {
        fs.insert_symlink(
            &tsr_tspath::absolute(link, current_directory),
            &tsr_tspath::absolute(target, current_directory),
        );
    }
    Host {
        fs: Arc::new(fs.finish()),
        cwd: JsString::from_bytes(current_directory),
    }
}

/// The line-by-line directive parser. `parse_file(name, content, file_options)`
/// builds one unit; the fourslash-only file options are `emitthisfile` and
/// `noopen`. Lines are split on `\r?\n`; a unit's content joins its lines
/// with `\n`, dropping leading blank lines unless `allow_implicit_first_file`.
///
/// The pin returns the units parsed so far together with a `parse_file`
/// error; here the error alone is returned.
// port: tsc/internal/testrunner/test_case_parser.go:ParseTestFilesAndSymlinks
// port: tsc/internal/testrunner/test_case_parser.go:ParseTestFilesAndSymlinksWithOptions
pub fn parse_test_files_and_symlinks<T>(
    code: &[u8],
    file_name: &[u8],
    mut parse_file: impl FnMut(&[u8], &[u8], &BTreeMap<String, String>) -> Result<T, Stop>,
    options: ParseTestFilesOptions,
) -> Result<ParsedTestFiles<T>, Stop> {
    // List of all the subfiles we've parsed out
    let mut test_units = Vec::new();

    // Stuff related to the subfile we're parsing
    let mut current_file_content: Vec<u8> = Vec::new();
    let mut current_file_name: Vec<u8> = Vec::new();
    let mut seen_content_line = false;
    let mut has_seen_file = false;
    if options.allow_implicit_first_file {
        // Content before the first @Filename directive goes into an implicit first file.
        current_file_name = file_name.to_vec();
    }
    let mut current_directory = Vec::new();
    let mut current_file_options = BTreeMap::new();
    let mut symlinks = BTreeMap::new();
    let mut global_options = BTreeMap::new();

    for line in split_lines(code) {
        if parse_symlink_from_test(line, &mut symlinks) {
            continue;
        }
        if let Some(found) = option_matches(line).next() {
            // Comment line, check for global/file @options and record them
            let meta_data_name = directive_name(&line[found.name]);
            let meta_data_value = trim_space(&line[found.value]);
            if meta_data_name == "currentdirectory" {
                current_directory = meta_data_value.to_vec();
            }
            if meta_data_name != "filename" {
                if meta_data_name == "symlink" && !current_file_name.is_empty() {
                    for link in meta_data_value.split(|&byte| byte == b',') {
                        let link = trim_space(link);
                        if !link.is_empty() {
                            symlinks.insert(link.to_vec(), current_file_name.clone());
                        }
                    }
                } else if FOURSLASH_DIRECTIVES.contains(&meta_data_name.as_str()) {
                    // File-specific option
                    current_file_options.insert(meta_data_name, setting_text(meta_data_value));
                } else {
                    // Global option. The pin leaves a conflicting duplicate
                    // unreported: the later value wins.
                    global_options.insert(meta_data_name, setting_text(meta_data_value));
                }
                continue;
            }

            // New metadata statement after having collected some code to go with the previous metadata
            if current_file_name.is_empty() {
                // First metadata marker in the file
                let has_content_before_first_filename = !current_file_content.is_empty()
                    && tsr_scanner::skip_trivia(&current_file_content, 0)
                        != current_file_content.len() as i64;
                if has_content_before_first_filename && !options.allow_implicit_first_file {
                    return Err(Stop::fatal(CONTENT_BEFORE_FIRST_FILENAME));
                }

                // Content before the first @Filename with AllowImplicitFirstFile
                // is saved as an implicit first file before starting the new
                // file. (At the pin this needs a nonempty current file name,
                // which this branch never has.)
                if has_content_before_first_filename
                    && options.allow_implicit_first_file
                    && !current_file_name.is_empty()
                {
                    has_seen_file = true;
                    test_units.push(parse_file(
                        &current_file_name,
                        &current_file_content,
                        &current_file_options,
                    )?);
                }

                // Reset for the new file
                current_file_content.clear();
                seen_content_line = false;
                // The pin trims the captured value again: `meta_data_value`.
                current_file_name = meta_data_value.to_vec();
                current_file_options = BTreeMap::new();
            } else {
                // Store result file - always save for regular tests, but skip
                // an empty implicit first file for fourslash
                let should_save_file = !options.allow_implicit_first_file
                    || !current_file_content.is_empty()
                    || has_seen_file;
                if should_save_file {
                    has_seen_file = true;
                    test_units.push(parse_file(
                        &current_file_name,
                        &current_file_content,
                        &current_file_options,
                    )?);
                }

                // Reset local data
                current_file_content.clear();
                seen_content_line = false;
                current_file_name = meta_data_value.to_vec();
                current_file_options = BTreeMap::new();
            }
        } else {
            // Subfile content line. Fourslash tests keep leading blank lines;
            // compiler tests drop them (the content is still empty).
            if options.allow_implicit_first_file {
                if seen_content_line {
                    current_file_content.push(b'\n');
                }
                seen_content_line = true;
            } else if !current_file_content.is_empty() {
                current_file_content.push(b'\n');
            }
            current_file_content.extend_from_slice(line);
        }
    }

    // normalize the fileName for the single file case
    if test_units.is_empty() && current_file_name.is_empty() {
        current_file_name = tsr_tspath::base_name(file_name).to_vec();
    }

    // EOF, push whatever remains
    test_units.push(parse_file(
        &current_file_name,
        &current_file_content,
        &current_file_options,
    )?);

    Ok(ParsedTestFiles {
        units: test_units,
        symlinks,
        current_directory,
        global_options,
    })
}

/// Every `// @name: value` of the file by lowercased name, values trimmed
/// and stripped of a trailing `;`. A later directive of the same name wins.
// port: tsc/internal/testrunner/test_case_parser.go:extractCompilerSettings
pub fn extract_compiler_settings(content: &[u8]) -> RawCompilerSettings {
    let mut opts = BTreeMap::new();
    for found in option_matches(content) {
        let value = trim_space(&content[found.value]);
        let value = value.strip_suffix(b";").unwrap_or(value);
        opts.insert(directive_name(&content[found.name]), setting_text(value));
    }
    opts
}

/// `// @link: <target> -> <link>` adds `link → target`; returns whether the
/// line was one.
// port: tsc/internal/testrunner/test_case_parser.go:parseSymlinkFromTest
pub fn parse_symlink_from_test(line: &[u8], symlinks: &mut BTreeMap<Vec<u8>, Vec<u8>>) -> bool {
    let Some((target, link)) = line_starts(line).find_map(|start| link_match(line, start)) else {
        return false;
    };
    symlinks.insert(
        trim_space(&line[link]).to_vec(),
        trim_space(&line[target]).to_vec(),
    );
    true
}

/// `lineDelimiter.Split(code, -1)` with `lineDelimiter = \r?\n`.
fn split_lines(code: &[u8]) -> impl Iterator<Item = &[u8]> {
    code.split(|&byte| byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
}

/// The submatches of one `optionRegex` match.
struct OptionMatch {
    name: Range<usize>,
    value: Range<usize>,
}

/// `optionRegex.FindAllSubmatch`: successive non-overlapping matches of
/// `(?m)^\/{2}\s*@(\w+)\s*:\s*([^\r\n]*)`. Each search starts where the
/// previous match ended, so the next candidate is the first line start at
/// or after that end.
fn option_matches(text: &[u8]) -> impl Iterator<Item = OptionMatch> + '_ {
    let mut position = Some(0);
    std::iter::from_fn(move || {
        while let Some(start) = position {
            if let Some((found, end)) = option_match(text, start) {
                // A match spans at least `//@x:`, so the search moves on.
                position = line_start_at_or_after(text, end);
                return Some(found);
            }
            position = line_start_at_or_after(text, start + 1);
        }
        None
    })
}

/// The first position at or after `index` where `(?m)^` holds: the start
/// of the text or a position after a `\n`.
fn line_start_at_or_after(text: &[u8], index: usize) -> Option<usize> {
    if index > text.len() {
        return None;
    }
    if index == 0 || text[index - 1] == b'\n' {
        return Some(index);
    }
    text[index..]
        .iter()
        .position(|&byte| byte == b'\n')
        .map(|offset| index + offset + 1)
}

/// Every position where `(?m)^` holds: the start of the text and each
/// position after a `\n`.
fn line_starts(text: &[u8]) -> impl Iterator<Item = usize> + '_ {
    std::iter::once(0).chain(
        text.iter()
            .enumerate()
            .filter(|(_, &byte)| byte == b'\n')
            .map(|(index, _)| index + 1),
    )
}

/// `optionRegex` anchored at `start` (a line start): the submatches and the
/// match end. Every quantifier but the last `\s*` is followed by a class it
/// excludes, so the greedy run is the only candidate; the last `\s*` is
/// greedy and `([^\r\n]*)` then always matches.
// source: tsc/internal/testrunner/test_case_parser.go:optionRegex
fn option_match(text: &[u8], start: usize) -> Option<(OptionMatch, usize)> {
    let mut index = start;
    if !text[index..].starts_with(b"//") {
        return None;
    }
    index = skip_regex_space(text, index + 2);
    if text.get(index) != Some(&b'@') {
        return None;
    }
    let name_start = index + 1;
    index = name_start;
    while text.get(index).is_some_and(|&byte| is_regex_word(byte)) {
        index += 1;
    }
    if index == name_start {
        return None;
    }
    let name = name_start..index;
    index = skip_regex_space(text, index);
    if text.get(index) != Some(&b':') {
        return None;
    }
    let value_start = skip_regex_space(text, index + 1);
    let value_end = line_content_end(text, value_start);
    Some((
        OptionMatch {
            name,
            value: value_start..value_end,
        },
        value_end,
    ))
}

/// `linkRegex` anchored at `start`:
/// `^\/{2}\s*@link\s*:\s*([^\r\n]*)\s*->\s*([^\r\n]*)`. The first group is
/// greedy, so it ends at the last position (before any `\r` or `\n`) from
/// which optional whitespace and `->` follow.
// source: tsc/internal/testrunner/test_case_parser.go:linkRegex
fn link_match(text: &[u8], start: usize) -> Option<(Range<usize>, Range<usize>)> {
    if !text[start..].starts_with(b"//") {
        return None;
    }
    let mut index = skip_regex_space(text, start + 2);
    if !text[index..].starts_with(b"@link") {
        return None;
    }
    index = skip_regex_space(text, index + b"@link".len());
    if text.get(index) != Some(&b':') {
        return None;
    }
    let target_start = skip_regex_space(text, index + 1);
    let target_limit = line_content_end(text, target_start);
    (target_start..=target_limit).rev().find_map(|target_end| {
        let arrow = skip_regex_space(text, target_end);
        if !text[arrow..].starts_with(b"->") {
            return None;
        }
        let link_start = skip_regex_space(text, arrow + 2);
        Some((
            target_start..target_end,
            link_start..line_content_end(text, link_start),
        ))
    })
}

/// RE2's `\s*`: `[\t\n\f\r ]`, without `\v`.
fn skip_regex_space(text: &[u8], mut index: usize) -> usize {
    while text
        .get(index)
        .is_some_and(|byte| matches!(byte, b'\t' | b'\n' | 0x0c | b'\r' | b' '))
    {
        index += 1;
    }
    index
}

/// RE2's `\w`.
fn is_regex_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The end of `[^\r\n]*` from `index`.
fn line_content_end(text: &[u8], index: usize) -> usize {
    text[index..]
        .iter()
        .position(|&byte| byte == b'\r' || byte == b'\n')
        .map_or(text.len(), |offset| index + offset)
}

/// `strings.ToLower` of a `\w+` capture, which is ASCII.
fn directive_name(name: &[u8]) -> String {
    name.iter()
        .map(|&byte| char::from(byte.to_ascii_lowercase()))
        .collect()
}

/// A directive value as this crate's `String`-typed settings carry it. The
/// pin keeps Go strings, which may hold any bytes; every directive value in
/// the pin's test data is valid UTF-8, and a malformed one is replaced
/// rather than dropped.
pub(crate) fn setting_text(value: &[u8]) -> String {
    String::from_utf8_lossy(value).into_owned()
}

/// Go's `unicode.IsSpace`.
fn is_go_space(rune: i32) -> bool {
    matches!(
        rune,
        0x09..=0x0d | 0x20 | 0x85 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f
            | 0x205f | 0x3000
    )
}

/// Go's `strings.TrimSpace` over bytes: Unicode white space from both ends,
/// decoding with Go's standard decoder (malformed bytes are not space).
pub(crate) fn trim_space(mut bytes: &[u8]) -> &[u8] {
    while !bytes.is_empty() {
        let (rune, width) = decode_utf8(bytes);
        if !is_go_space(rune) {
            break;
        }
        bytes = &bytes[width..];
    }
    while !bytes.is_empty() {
        let (rune, width) = decode_last_rune(bytes);
        if !is_go_space(rune) {
            break;
        }
        bytes = &bytes[..bytes.len() - width];
    }
    bytes
}

/// Go's `utf8.DecodeLastRune` over a nonempty slice.
fn decode_last_rune(bytes: &[u8]) -> (i32, usize) {
    let end = bytes.len();
    let last = bytes[end - 1];
    if last < 0x80 {
        return (i32::from(last), 1);
    }
    let limit = end.saturating_sub(4);
    let mut start = end - 1;
    while start > limit {
        start -= 1;
        if bytes[start] & 0xc0 != 0x80 {
            break;
        }
    }
    let (rune, width) = decode_utf8(&bytes[start..]);
    if start + width == end {
        (rune, width)
    } else {
        (RUNE_ERROR, 1)
    }
}

/// `strings.ToLower` of a directive value, which `setting_text` made UTF-8.
pub(crate) fn to_lower(value: &str) -> String {
    setting_text(&to_lower_go(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::{
        extract_compiler_settings, make_units_from_test, parse_symlink_from_test,
        parse_test_files_and_symlinks, trim_space, ParseTestFilesOptions, TestUnit,
        CONTENT_BEFORE_FIRST_FILENAME,
    };
    use crate::Stop;
    use std::collections::BTreeMap;
    use tsr_core::Tristate;
    use tsr_jsstring::JsString;

    fn units(code: &[u8], options: ParseTestFilesOptions) -> Result<Vec<TestUnit>, Stop> {
        parse_test_files_and_symlinks(
            code,
            b"tests/cases/compiler/sample.ts",
            |name, content, _| {
                Ok(TestUnit {
                    name: name.to_vec(),
                    content: content.to_vec(),
                })
            },
            options,
        )
        .map(|parsed| parsed.units)
    }

    fn unit(name: &str, content: &str) -> TestUnit {
        TestUnit {
            name: name.as_bytes().to_vec(),
            content: content.as_bytes().to_vec(),
        }
    }

    #[test]
    fn multi_file_tests_split_into_named_units_and_global_options() {
        let code = b"// @target: es2015\r\n// @Filename: a.ts\r\nexport const a = 1;\r\n// @filename:   b.ts  \nimport { a } from \"./a\";\n";
        let parsed = parse_test_files_and_symlinks(
            code,
            b"sample.ts",
            |name, content, _| {
                Ok(TestUnit {
                    name: name.to_vec(),
                    content: content.to_vec(),
                })
            },
            ParseTestFilesOptions::default(),
        )
        .expect("parses");
        assert_eq!(
            parsed.units,
            vec![
                unit("a.ts", "export const a = 1;"),
                // The trailing empty line is kept: content joins lines with `\n`.
                unit("b.ts", "import { a } from \"./a\";\n"),
            ]
        );
        assert_eq!(
            parsed.global_options,
            BTreeMap::from([("target".to_string(), "es2015".to_string())])
        );
        assert!(parsed.current_directory.is_empty());
    }

    #[test]
    fn leading_blank_lines_are_dropped_unless_an_implicit_first_file_is_allowed() {
        let code = b"// @Filename: a.ts\n\n\nconst x = 1;\n\nconst y = 2;";
        assert_eq!(
            units(code, ParseTestFilesOptions::default()).expect("parses"),
            vec![unit("a.ts", "const x = 1;\n\nconst y = 2;")]
        );
        assert_eq!(
            units(
                code,
                ParseTestFilesOptions {
                    allow_implicit_first_file: true
                }
            )
            .expect("parses"),
            vec![unit("a.ts", "\n\nconst x = 1;\n\nconst y = 2;")]
        );
    }

    #[test]
    fn a_test_without_filename_directives_is_one_unit_named_after_the_file() {
        assert_eq!(
            units(
                b"// @strict: true\nconst x = 1;\n",
                ParseTestFilesOptions::default()
            )
            .expect("parses"),
            vec![unit("sample.ts", "const x = 1;\n")]
        );
    }

    #[test]
    fn code_before_the_first_filename_directive_is_fatal_but_comments_are_not() {
        assert_eq!(
            units(
                b"const x = 1;\n// @Filename: a.ts\nexport {}",
                ParseTestFilesOptions::default()
            ),
            Err(Stop::fatal(CONTENT_BEFORE_FIRST_FILENAME))
        );
        assert_eq!(
            units(
                b"// a leading comment\n/* and a block */\n// @Filename: a.ts\nexport {}",
                ParseTestFilesOptions::default()
            )
            .expect("comments are trivia"),
            vec![unit("a.ts", "export {}")]
        );
    }

    #[test]
    fn link_and_symlink_directives_record_links() {
        let code = b"// @Filename: /packages/a/index.ts\n// @symlink: /node_modules/a, ,/node_modules/b\nexport {}\n// @link: /packages/c -> /node_modules/c\n// @link: x -> y -> z\n";
        let parsed = parse_test_files_and_symlinks(
            code,
            b"sample.ts",
            |name, content, _| Ok((name.to_vec(), content.to_vec())),
            ParseTestFilesOptions::default(),
        )
        .expect("parses");
        let expected: BTreeMap<Vec<u8>, Vec<u8>> = [
            ("/node_modules/a", "/packages/a/index.ts"),
            ("/node_modules/b", "/packages/a/index.ts"),
            ("/node_modules/c", "/packages/c"),
            // The target group is greedy: the last `->` separates.
            ("z", "x -> y"),
        ]
        .into_iter()
        .map(|(link, target)| (link.as_bytes().to_vec(), target.as_bytes().to_vec()))
        .collect();
        assert_eq!(parsed.symlinks, expected);
        assert_eq!(
            parsed.units,
            vec![(b"/packages/a/index.ts".to_vec(), b"export {}\n".to_vec())]
        );
        // `@link` lines are not global options, and a `@symlink` before any
        // file is one.
        let parsed = parse_test_files_and_symlinks(
            b"// @symlink: /a\n// @Link: b -> c\nexport {}",
            b"sample.ts",
            |_, _, _| Ok(()),
            ParseTestFilesOptions::default(),
        )
        .expect("parses");
        assert!(parsed.symlinks.is_empty());
        assert_eq!(
            parsed.global_options,
            BTreeMap::from([
                ("link".to_string(), "b -> c".to_string()),
                ("symlink".to_string(), "/a".to_string()),
            ])
        );
        let mut symlinks = BTreeMap::new();
        assert!(!parse_symlink_from_test(
            b"// @link: no arrow",
            &mut symlinks
        ));
        assert!(symlinks.is_empty());
    }

    #[test]
    fn fourslash_directives_are_file_options() {
        let parsed = parse_test_files_and_symlinks(
            b"// @Filename: a.ts\n// @emitThisFile: true\nexport {}\n// @Filename: b.ts\n// @noOpen: true\n",
            b"sample.ts",
            |name, _, options| Ok((name.to_vec(), options.clone())),
            ParseTestFilesOptions::default(),
        )
        .expect("parses");
        assert_eq!(
            parsed.units,
            vec![
                (
                    b"a.ts".to_vec(),
                    BTreeMap::from([("emitthisfile".to_string(), "true".to_string())])
                ),
                (
                    b"b.ts".to_vec(),
                    BTreeMap::from([("noopen".to_string(), "true".to_string())])
                ),
            ]
        );
        assert!(parsed.global_options.is_empty());
    }

    #[test]
    fn current_directory_roots_the_tsconfig_unit() {
        let code = b"// @currentDirectory: /home/src/project\n// @runExternalCode: true\n// @Filename: tsconfig.json\n{ \"compilerOptions\": { \"strict\": true }, \"files\": [\"a.ts\"] }\n// @Filename: a.ts\nexport {}\n// @Filename: b.ts\nexport {}\n";
        let content =
            make_units_from_test(code, b"tests/cases/compiler/sample.ts").expect("parses");
        assert_eq!(
            content.units,
            vec![unit("a.ts", "export {}"), unit("b.ts", "export {}\n")]
        );
        assert_eq!(
            content.ts_config_unit.expect("config unit").name,
            b"tsconfig.json"
        );
        let config = content.ts_config.expect("config");
        assert!(config.errors.is_empty(), "{:?}", config.errors);
        assert_eq!(config.options.strict, Tristate::TRUE);
        assert_eq!(config.options.run_external_code, Tristate::TRUE);
        assert_eq!(
            config.options.config_file_path.as_bytes(),
            b"/home/src/project/tsconfig.json"
        );
        let names: Vec<&[u8]> = config
            .root_file_names
            .iter()
            .map(JsString::as_bytes)
            .collect();
        assert_eq!(names, vec![b"/home/src/project/a.ts".as_slice()]);
    }

    #[test]
    fn the_tsconfig_unit_defaults_to_the_source_folder() {
        let code = b"// @Filename: /.src/jsconfig.json\n{}\n// @Filename: a.js\nexport {}\n";
        let content = make_units_from_test(code, b"sample.ts").expect("parses");
        let config = content.ts_config.expect("config");
        assert_eq!(
            config.options.config_file_path.as_bytes(),
            b"/.src/jsconfig.json"
        );
        assert_eq!(config.options.run_external_code, Tristate::UNKNOWN);
        assert_eq!(content.units, vec![unit("a.js", "export {}\n")]);
    }

    #[test]
    fn settings_follow_the_pins_regular_expression() {
        let settings = extract_compiler_settings(
            b"// @Target: ES2015;\n//@strict:true\n  // @indented: no\n/// @triple: no\n// @target: esnext\n// @empty:\n// @next: value\n",
        );
        assert_eq!(
            settings,
            BTreeMap::from([
                // `\s*` after the colon crosses the line break, so an empty
                // value captures the next line.
                ("empty".to_string(), "// @next: value".to_string()),
                ("strict".to_string(), "true".to_string()),
                ("target".to_string(), "esnext".to_string()),
            ])
        );
    }

    #[test]
    fn trim_space_trims_go_unicode_space_only() {
        assert_eq!(trim_space(b" \t\x0b a b \xc2\xa0\xe3\x80\x80"), b"a b");
        assert_eq!(trim_space(b"\xc2 a \xa0"), b"\xc2 a \xa0");
        assert_eq!(trim_space(b"   "), b"");
    }

    #[test]
    fn a_real_test_file_parses() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../upstream/tsc/testdata/tests/cases/compiler/moduleResolutionWithSymlinks.ts"
        );
        let code = std::fs::read(path).expect("pinned test file");
        let content =
            make_units_from_test(&code, b"moduleResolutionWithSymlinks.ts").expect("parses");
        assert!(content.ts_config.is_none());
        assert!(!content.symlinks.is_empty());
        assert!(content.units.len() > 1);
    }
}
