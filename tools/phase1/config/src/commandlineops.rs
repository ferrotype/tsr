//! The command-line and result half of `internal/tsoptions`: the 65 pending
//! operations of `parsedcommandline.go`, `commandlineparser.go`, `errors.go`,
//! `commandlineoption.go`, `namemap.go`, `wildcarddirectories.go`,
//! `enummaps.go`, `diagnostics.go` and `parsedbuildcommandline.go`.
//!
//! This group is deliberately mixed, because the surface is. Three things are
//! true of the port at once, and a single undifferentiated answer would hide
//! two of them:
//!
//! 1. **There is no argument-vector parser.** `crates/tsr_tsoptions` has no
//!    `parse_command_line` and no build-option mode. The closest thing,
//!    `fixture_options::apply_fixture_settings`, takes already-split
//!    `name`/`value` pairs and explicitly refuses a switch or a response-file
//!    argument (crates/tsr_tsoptions/src/fixture_options.rs:270-275). So every
//!    `parse_command_line` and `parse_build_command_line` action is a gap, and
//!    so is everything only that parser reaches.
//!
//! 2. **The option DECLARATION semantics are present and comparable.** The
//!    generated table carries `element`, `enum_values` and `deprecated_keys`
//!    (crates/tsr_tsoptions/src/option_declarations.rs:38-40), and
//!    `disallow_null` (:47), `value_type_name` (:50) and `enum_names` (:62) are
//!    named functions. Those actions are really compared, not recorded as gaps.
//!
//! 3. **The config-parse result object is partly present.** `ParsedCommandLine`
//!    in the port (crates/tsr_tsoptions/src/lib.rs:196-212) carries the fields
//!    the pinned accessors read, but only four accessors exist as operations:
//!    `config_name` (config_specs.rs:72), `config_file_parsing_diagnostics`
//!    (lib.rs:217), `matched_file_spec` (config_specs.rs:87) and
//!    `matched_include_spec` (config_specs.rs:98). The other twenty are gaps,
//!    each named separately so the record says which.
//!
//! Preparation records a gap; it never emulates the missing algorithm to make a
//! comparison run, and it never reads an expected value.

use crate::api::{action_op, action_str, actions, ordered, subject, Outcome};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use tsr_core::{CompilerOptions, ScriptTarget};
use tsr_jsstring::{JsString, SourceText};
use tsr_tsoptions::{
    config_mappers::MapperResolution, default_lib_file_name, find_declaration, lib_file_name,
    parse_json_source_file_config_file_content, parse_list_type_option, ConfigValue,
    OptionDeclaration, ParseConfigHost, ParsedCommandLine, TsConfigSourceFile, BUILD_OPTIONS,
    COMPILER_OPTIONS, ROOT_OPTIONS, WATCH_OPTIONS,
};
use tsr_vfs::{FileSystem, MemoryBuilder};

// ---------------------------------------------------------------------------
// The gaps, one reviewed record each.
// ---------------------------------------------------------------------------

/// Shared by every action the absent argument-vector parser would have to run.
const NO_PARSER: &str = "crates/tsr_tsoptions/src/parse_options.rs interprets option VALUES and \
     crates/tsr_tsoptions/src/fixture_options.rs interprets already-split name/value pairs; \
     neither takes an argv, and no file in crates/tsr_tsoptions defines one (absent)";

/// Shared by the `ParsedCommandLine` accessors the port does not carry.
const NO_ACCESSOR: &str = "crates/tsr_tsoptions/src/lib.rs:196-212 declares ParsedCommandLine and \
     crates/tsr_tsoptions/src/config_specs.rs:70-117 carries its only accessor block; neither \
     defines this operation (absent)";

/// `(operation, go_authority, intended_signature, production_home)`.
type Gap = (&'static str, &'static str, &'static str, &'static str);

const PARSE_COMMAND_LINE: Gap = (
    "tsc/internal/tsoptions/commandlineparser.go:ParseCommandLine",
    "tsc/internal/tsoptions/commandlineparser.go:43, which runs parseCommandLineWorker (:115), \
     parseStrings (:134), getInputOptionName (:164), parseOptionValue (:237) and, for an \
     unrecognised switch, commandLineParser.createUnknownOptionError (errors.go:39)",
    "pub fn parse_command_line(command_line: &[JsString], host: &dyn ParseConfigHost) -> \
     ParsedCommandLine, carrying the raw option map in argument order, the file names and the \
     diagnostics each step raised",
    NO_PARSER,
);

const PARSE_BUILD_COMMAND_LINE: Gap = (
    "tsc/internal/tsoptions/commandlineparser.go:ParseBuildCommandLine",
    "tsc/internal/tsoptions/commandlineparser.go:64, a distinct parsing mode over BuildOpts that \
     also cross-checks CompilerNameMap (:75), defaults an empty project list to \".\" (:95) and \
     raises the four nonsensical-combination diagnostics (:99-110)",
    "pub fn parse_build_command_line(command_line: &[JsString], host: &dyn ParseConfigHost) -> \
     ParsedBuildCommandLine, carrying the build options, the compiler options it still accepts, \
     the project list and the build-only diagnostics",
    "crates/tsr_tsoptions/src/option_declarations_generated.rs carries BUILD_OPTIONS, so the \
     declarations exist; the build parsing MODE does not (absent)",
);

const INPUT_OPTION_NAME: Gap = (
    "tsc/internal/tsoptions/commandlineparser.go:getInputOptionName",
    "tsc/internal/tsoptions/commandlineparser.go:164, which strips at most two leading '-' from \
     an argument before the name map is consulted (:146-147)",
    "pub fn input_option_name(argument: &[u8]) -> &[u8]",
    NO_PARSER,
);

const INVALID_ENUM_TYPE_DIAGNOSTIC: Gap = (
    "tsc/internal/tsoptions/errors.go:createDiagnosticForInvalidEnumType",
    "tsc/internal/tsoptions/errors.go:14, which collects the option's enum keys, formats them \
     through formatEnumTypeKeys (:21) and builds Argument_for_0_option_must_be_Colon_1",
    "pub fn invalid_enum_type_diagnostic(option: &OptionDeclaration, syntax: OptionSyntax<'_>) -> \
     Diagnostic",
    "crates/tsr_tsoptions/src/option_declarations.rs:62 has enum_names, the string this \
     diagnostic carries, and crates/tsr_tsoptions/src/fixture_options.rs:57 builds the same \
     message for the fixture bridge; neither is a shared operation the config path can call, \
     and fixture_options::enum_error is private to that module (absent as a named operation)",
);

const EXTRA_KEY_DIAGNOSTICS: Gap = (
    "tsc/internal/tsoptions/errors.go:extraKeyDiagnostics",
    "tsc/internal/tsoptions/errors.go:103 and its did-you-mean sibling at :118, which map a \
     parent option name -- compilerOptions, watchOptions, typeAcquisition, buildOptions -- to \
     the unknown-key message pair, and answer nil for anything else",
    "pub fn extra_key_diagnostics(parent: &[u8]) -> Option<(&'static Message, &'static Message)>",
    "crates/tsr_tsoptions/src/config_parse.rs:144-156 inlines the same choice inside `unknown`, \
     for compilerOptions and typeAcquisition only, with no watchOptions or buildOptions arm and \
     no nil answer for an unrecognised parent (absent as an operation, and partial where it is \
     inlined)",
);

const WORKER_DIAGNOSTICS: Gap = (
    "tsc/internal/tsoptions/diagnostics.go:getParseCommandLineWorkerDiagnostics",
    "tsc/internal/tsoptions/diagnostics.go:30, which builds the compiler-mode \
     ParseCommandLineWorkerDiagnostics -- the alternate mode pointing at BuildNameMap, the \
     unknown and did-you-mean messages, and the option-type mismatch message -- over a \
     caller-supplied declaration list",
    "pub fn parse_command_line_worker_diagnostics(declarations: &'static [OptionDeclaration]) -> \
     ParseCommandLineWorkerDiagnostics",
    "no Rust home: crates/tsr_tsoptions has no worker-diagnostics value at all; \
     fixture_options.rs:276-302 hard-codes the compiler-mode choices inline instead (absent)",
);

const TO_CANONICAL_KEY: Gap = (
    "tsc/internal/tsoptions/wildcarddirectories.go:toCanonicalKey",
    "tsc/internal/tsoptions/wildcarddirectories.go:85, the case-folding a wildcard directory key \
     is stored under",
    "pub fn to_canonical_key(path: &[u8], use_case_sensitive_file_names: bool) -> Cow<'_, [u8]>",
    "no Rust home: a grep for `wildcard` over crates/ finds only diagnostic message names, \
     tsr_module type references and semver helpers; crates/tsr_tsoptions has no \
     wildcard-directory file (absent)",
);

const WILDCARD_DIRECTORY_FROM_SPEC: Gap = (
    "tsc/internal/tsoptions/wildcarddirectories.go:getWildcardDirectoryFromSpec",
    "tsc/internal/tsoptions/wildcarddirectories.go:99, which decides from one include spec which \
     directory is watched and whether it is watched recursively",
    "pub fn wildcard_directory_from_spec(spec: &[u8], use_case_sensitive_file_names: bool) -> \
     Option<WildcardDirectoryMatch>",
    "no Rust home: crates/tsr_tsoptions has no wildcard-directory file (absent)",
);

const WILDCARD_DIRECTORIES: Gap = (
    "tsc/internal/tsoptions/wildcarddirectories.go:getWildcardDirectories",
    "tsc/internal/tsoptions/wildcarddirectories.go:10, the whole calculation: exclude matching, \
     per-spec directory selection, canonical-key collision handling and the removal of subpaths \
     under an already recursive watch",
    "pub fn wildcard_directories(include: &[JsString], exclude: &[JsString], base: &[u8], \
     use_case_sensitive_file_names: bool) -> Vec<(JsString, bool)>, carrying the include-order \
     insertion sequence the TypeScript object had",
    "no Rust home: crates/tsr_tsoptions has no wildcard-directory file, and \
     crates/tsr_tsoptions/src/config_specs.rs carries only the spec matchers (absent)",
);

/// The `ParsedCommandLine` accessors with no Rust counterpart, by probe name.
const ACCESSOR_GAPS: &[(&str, Gap)] = &[
    (
        "file_names_by_path",
        (
            "tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.FileNamesByPath",
            "tsc/internal/tsoptions/parsedcommandline.go:334, the once-built path -> file-name \
             index over the parse's own file names",
            "pub fn file_names_by_path(&self) -> &BTreeMap<Path, JsString>",
            NO_ACCESSOR,
        ),
    ),
    (
        "current_directory",
        (
            "tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.GetCurrentDirectory",
            "tsc/internal/tsoptions/parsedcommandline.go:189, which reads the comparePathsOptions \
             the result was built with, and :193 for the case-sensitivity flag beside it",
            "pub fn current_directory(&self) -> &[u8]",
            "crates/tsr_tsoptions/src/lib.rs:205-206 carries config_base_path and \
             config_case_sensitive as public fields, but the pinned pair is comparePathsOptions, \
             which a command-line parse fills from the host rather than from a config's base \
             path; no accessor of that name exists (absent)",
        ),
    ),
    (
        "wildcard_directories",
        (
            "tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.WildcardDirectories",
            "tsc/internal/tsoptions/parsedcommandline.go:258, the once-only accessor that is the \
             only pinned caller of getWildcardDirectories (wildcarddirectories.go:10)",
            "pub fn wildcard_directories(&self) -> &[(JsString, bool)]",
            NO_ACCESSOR,
        ),
    ),
    (
        "wildcard_directory_globs",
        (
            "tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.fileGlobPatterns",
            "tsc/internal/tsoptions/parsedcommandline.go:31, which augments the built-in include \
             glob with the extensions the config's content mappers registered; its only caller is \
             WildcardDirectoryGlobs (:284)",
            "fn file_glob_patterns(&self) -> (Vec<u8>, Vec<u8>)",
            NO_ACCESSOR,
        ),
    ),
    (
        "extended_source_files",
        (
            "tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.ExtendedSourceFiles",
            "tsc/internal/tsoptions/parsedcommandline.go:386, which reads the extends chain the \
             parse recorded on the config source file, and answers nil when there is no config",
            "pub fn extended_source_files(&self) -> &[JsString]",
            "crates/tsr_tsoptions/src/config_syntax.rs:11 carries extended_source_files on \
             TsConfigSourceFile, so the state exists; no accessor on ParsedCommandLine reaches \
             it, and the nil-when-no-config contract has no home (absent)",
        ),
    ),
    (
        "project_references",
        (
            "tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.ResolvedProjectReferencePaths",
            "tsc/internal/tsoptions/parsedcommandline.go:379, the once-only resolution of every \
             project reference path, over ProjectReferences (:345)",
            "pub fn resolved_project_reference_paths(&self) -> &[JsString]",
            "crates/tsr_tsoptions/src/lib.rs:209 carries project_references as a public field, so \
             the references themselves exist; core.ResolveProjectReferencePath has no Rust \
             counterpart reachable from ParsedCommandLine (absent)",
        ),
    ),
    (
        "common_source_directory",
        (
            "tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.CommonSourceDirectory",
            "tsc/internal/tsoptions/parsedcommandline.go:157, which filters the file names and \
             hands outputpaths.GetCommonSourceDirectory the checkSourceFilesBelongToPath callback \
             at :176 -- the callback that appends File_0_is_not_under_rootDir_1 to Errors",
            "pub fn common_source_directory(&mut self) -> &[u8]",
            NO_ACCESSOR,
        ),
    ),
    (
        "build_info_file_name",
        (
            "tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.GetBuildInfoFileName",
            "tsc/internal/tsoptions/parsedcommandline.go:253, which forwards the compiler options \
             and comparePathsOptions to outputpaths.GetBuildInfoFileName",
            "pub fn build_info_file_name(&self) -> JsString",
            NO_ACCESSOR,
        ),
    ),
    (
        "input_output_names",
        (
            "tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.ParseInputOutputNames",
            "tsc/internal/tsoptions/parsedcommandline.go:135, which walks \
             getOutputDeclarationAndSourceFileNames (:197) once and fills both \
             SourceToProjectReference (:127) and OutputDtsToProjectReference (:131)",
            "pub fn parse_input_output_names(&mut self), plus the two path-keyed maps it fills",
            NO_ACCESSOR,
        ),
    ),
    (
        "content_mappers",
        (
            "tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.GetContentMapperForFileName",
            "tsc/internal/tsoptions/parsedcommandline.go:366, which picks the configured mapper \
             whose extensions match a file name, over ContentMapperExtensions (:358) and \
             ContentMappers (:349)",
            "pub fn content_mapper_for_file_name(&self, file_name: &[u8]) -> \
             Option<&ContentMapper>",
            "crates/tsr_tsoptions/src/lib.rs:211 carries content_mappers as a public field and \
             crates/tsr_tsoptions/src/config_mappers.rs validates them, but no accessor selects a \
             mapper by file name and nothing flattens the extension list (absent)",
        ),
    ),
    (
        "type_acquisition",
        (
            "tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.SetTypeAcquisition",
            "tsc/internal/tsoptions/parsedcommandline.go:321, and the accessor at :325 that reads \
             back what it set",
            "pub fn set_type_acquisition(&mut self, acquisition: TypeAcquisition)",
            "crates/tsr_tsoptions/src/lib.rs:208 carries type_acquisition as a public field, so \
             the state is reachable; no named operation sets or reads it, and writing the field \
             from this harness would be the harness doing the port's job (absent)",
        ),
    ),
    (
        "set_compiler_options",
        (
            "tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.SetCompilerOptions",
            "tsc/internal/tsoptions/parsedcommandline.go:310, which replaces the compiler options \
             without disturbing the rest of ParsedConfig, and Locale (:489), the once-only \
             locale.Parse over whatever options are in place when it is first asked",
            "pub fn set_compiler_options(&mut self, options: CompilerOptions) and pub fn \
             locale(&self) -> Locale",
            "crates/tsr_tsoptions/src/lib.rs:198 carries options as a public field; there is no \
             setter operation, and no locale parsing anywhere in crates/tsr_tsoptions (absent)",
        ),
    ),
    (
        "set_parsed_options",
        (
            "tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.SetParsedOptions",
            "tsc/internal/tsoptions/parsedcommandline.go:306, which replaces the whole \
             ParsedOptions block -- compiler options, watch options, type acquisition, file \
             names, project references and content mappers -- in one call",
            "pub fn set_parsed_options(&mut self, parsed: ParsedOptions)",
            "crates/tsr_tsoptions/src/lib.rs:196-212 flattens ParsedOptions into \
             ParsedCommandLine's own fields, so there is no block to replace and no setter \
             (absent)",
        ),
    ),
];

// ---------------------------------------------------------------------------
// Rendering.
// ---------------------------------------------------------------------------

fn text(value: &[u8]) -> Value {
    Value::String(String::from_utf8_lossy(value).into_owned())
}

/// A diagnostic as its numeric code, its text range and then its arguments,
/// matching the probe's rendering. A macro rather than a function because `tsr_ast` is not a
/// direct dependency of this harness, so the type cannot be named.
macro_rules! diagnostic_rows {
    ($diagnostics:expr) => {
        Value::Array(
            $diagnostics
                .iter()
                .map(|diagnostic| {
                    let mut row = vec![
                        json!(diagnostic.code),
                        json!(diagnostic.loc.pos()),
                        json!(diagnostic.loc.end()),
                    ];
                    row.extend(
                        diagnostic
                            .message_args
                            .iter()
                            .map(|argument| text(argument.as_bytes())),
                    );
                    Value::Array(row)
                })
                .collect::<Vec<Value>>(),
        )
    };
}

/// Render a value that crossed the port's config `any` boundary in the shape
/// the probe renders the pinned one: scalars as themselves, arrays as arrays,
/// objects as entry arrays so no ordered data becomes a JSON object.
fn config_value(value: &ConfigValue) -> Value {
    match value {
        ConfigValue::Null => Value::Null,
        ConfigValue::EmptyStruct => Value::String("empty-struct".into()),
        ConfigValue::Boolean(value) => json!(value),
        ConfigValue::Number(value) => json!(value),
        ConfigValue::Integer(value) => json!(value),
        ConfigValue::Enum(value) => json!(value),
        ConfigValue::String(value) => text(value.as_bytes()),
        ConfigValue::Array(values) => Value::Array(
            values
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(config_value)
                .collect(),
        ),
        ConfigValue::Object(entries) => Value::Array(
            entries
                .iter()
                .map(|(name, value)| json!([text(name.as_bytes()), config_value(value)]))
                .collect(),
        ),
    }
}

fn strings(values: &[JsString]) -> Value {
    Value::Array(values.iter().map(|value| text(value.as_bytes())).collect())
}

// ---------------------------------------------------------------------------
// Declaration tables.
// ---------------------------------------------------------------------------

fn table(name: &str) -> Option<&'static [OptionDeclaration]> {
    match name {
        "compiler" => Some(COMPILER_OPTIONS),
        "watch" => Some(WATCH_OPTIONS),
        "build" => Some(BUILD_OPTIONS),
        "root" => Some(ROOT_OPTIONS),
        _ => None,
    }
}

/// The port's counterpart of `NameMap.Get`: `find_declaration` with short names
/// disabled (crates/tsr_tsoptions/src/option_declarations.rs:82-104), whose
/// `.rev()` reproduces the pin's last-declaration-wins collision rule
/// (namemap.go:19).
fn declaration(action: &Value) -> Result<&'static OptionDeclaration, String> {
    let name = action_str(action, "table");
    let options = table(name).ok_or_else(|| format!("unknown declaration table {name:?}"))?;
    let option = action_str(action, "name");
    find_declaration(options, option.as_bytes(), false)
        .ok_or_else(|| format!("no option named {option:?} in table {name:?}"))
}

// ---------------------------------------------------------------------------
// The config-parse host.
// ---------------------------------------------------------------------------

struct Host {
    fs: Arc<dyn FileSystem>,
    cwd: JsString,
}

fn module_error(error: &tsr_module::Error) -> tsr_vfs::Error {
    match error {
        tsr_module::Error::Host(error) => error.clone(),
        tsr_module::Error::MutableHost => {
            tsr_vfs::Error::Unsupported("config resolver requires an immutable host")
        }
        tsr_module::Error::Unsupported(reason) => tsr_vfs::Error::Unsupported(reason),
        tsr_module::Error::MalformedPackageJson(_) => {
            tsr_vfs::Error::Unsupported("malformed package JSON")
        }
    }
}

impl ParseConfigHost for Host {
    fn fs(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
    fn current_directory(&self) -> &[u8] {
        self.cwd.as_bytes()
    }
    fn resolve_config(
        &self,
        name: &[u8],
        containing: &[u8],
    ) -> Result<Option<JsString>, tsr_vfs::Error> {
        let resolved =
            tsr_module::resolve_config(name, containing, self.fs.clone(), self.cwd.as_bytes())
                .map_err(|error| module_error(&error))?;
        Ok((!resolved.resolved_file_name.is_empty()).then_some(resolved.resolved_file_name))
    }
    fn resolve_content_mapper(
        &self,
        containing: &[u8],
        package: &[u8],
    ) -> Result<MapperResolution, tsr_vfs::Error> {
        tsr_module::resolve_content_mapper_manifest(
            &self.fs,
            self.cwd.as_bytes(),
            containing,
            package,
        )
        .map_err(|error| module_error(&error))
    }
}

/// Build the request's `ParsedCommandLine` through the port's own config entry
/// point, with the same file name and base path the pinned
/// `tsoptionstest.GetParsedCommandLine` derives
/// (tsoptionstest/parsedcommandline.go:11-13).
fn config_parse(request: &Value) -> Result<ParsedCommandLine, String> {
    let current = action_str(request, "currentDirectory").as_bytes().to_vec();
    let case_sensitive = request
        .get("caseSensitive")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut builder = MemoryBuilder::new(&current, case_sensitive);
    if let Some(files) = request.get("files").and_then(Value::as_object) {
        for (path, content) in files {
            builder.insert_physical(
                path.as_bytes(),
                content.as_str().unwrap_or_default().as_bytes().to_vec(),
            );
        }
    }
    let host = Host {
        fs: Arc::new(builder.finish()),
        cwd: JsString::from_bytes(current.clone()),
    };
    let name = tsr_tspath::combine(&current, &[b"tsconfig.json"]);
    let source = TsConfigSourceFile::parse(
        JsString::from_bytes(name.clone()),
        tsr_tspath::to_path(&name, &current, case_sensitive),
        SourceText::from_loaded_bytes(action_str(request, "jsonText").as_bytes().to_vec()),
    );
    parse_json_source_file_config_file_content(
        source,
        &host,
        &current,
        &CompilerOptions::default(),
        &ConfigValue::Null,
        &name,
    )
    .map_err(|error| format!("config parse: {error:?}"))
}

// ---------------------------------------------------------------------------
// Actions.
// ---------------------------------------------------------------------------

/// Answer one action, or report which pinned operation the port is missing.
#[allow(clippy::too_many_lines)]
fn run(
    request: &Value,
    action: &Value,
    parsed: &mut Option<ParsedCommandLine>,
) -> Result<Value, Outcome> {
    let op = action_op(action);
    let mut row = Map::new();
    row.insert("op".into(), Value::String(op.to_owned()));
    match op {
        "option_declaration" => {
            let option = declaration(action).map_err(Outcome::Failed)?;
            row.insert("name".into(), Value::String(option.name.into()));
            row.insert("short_name".into(), Value::String(option.short_name.into()));
            row.insert("kind".into(), Value::String(option.kind.as_str().into()));
            row.insert("is_file_path".into(), json!(option.is_file_path));
            row.insert("is_tsconfig_only".into(), json!(option.is_tsconfig_only));
            row.insert(
                "is_command_line_only".into(),
                json!(option.is_command_line_only),
            );
            row.insert(
                "disallow_null_or_undefined".into(),
                json!(option.disallow_null()),
            );
            match option.element {
                None => {
                    row.insert("element_name".into(), Value::Null);
                    row.insert("element_kind".into(), Value::Null);
                    row.insert("element_is_file_path".into(), Value::Null);
                }
                Some(element) => {
                    row.insert("element_name".into(), Value::String(element.name.into()));
                    row.insert(
                        "element_kind".into(),
                        Value::String(element.kind.as_str().into()),
                    );
                    row.insert("element_is_file_path".into(), json!(element.is_file_path));
                }
            }
            row.insert(
                "enum_entries".into(),
                Value::Array(
                    option
                        .enum_values
                        .iter()
                        .map(|(name, value)| match value {
                            tsr_tsoptions::EnumValue::String(value) => json!([name, value]),
                            tsr_tsoptions::EnumValue::Number(value) => json!([name, value]),
                        })
                        .collect(),
                ),
            );
            let mut deprecated: Vec<&str> = option.deprecated_keys.to_vec();
            deprecated.sort_unstable();
            row.insert("deprecated_keys".into(), json!(deprecated));
        }

        "option_value_type_string" => {
            let option = declaration(action).map_err(Outcome::Failed)?;
            row.insert("name".into(), Value::String(option.name.into()));
            row.insert(
                "value_type_string".into(),
                Value::String(option.value_type_name()),
            );
        }

        "format_enum_type_keys" => {
            let option = declaration(action).map_err(Outcome::Failed)?;
            row.insert("name".into(), Value::String(option.name.into()));
            row.insert("formatted".into(), Value::String(option.enum_names()));
        }

        "name_map" => {
            let name = action_str(action, "table");
            let options = table(name)
                .ok_or_else(|| Outcome::Failed(format!("unknown declaration table {name:?}")))?;
            let lookup = action_str(action, "name");
            let allow_short = action
                .get("allowShort")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            row.insert("lookup".into(), Value::String(lookup.to_owned()));
            row.insert("allow_short".into(), json!(allow_short));
            row.insert(
                "get".into(),
                find_declaration(options, lookup.as_bytes(), false)
                    .map_or(Value::Null, |option| Value::String(option.name.into())),
            );
            row.insert(
                "get_option_declaration_from_name".into(),
                find_declaration(options, lookup.as_bytes(), allow_short)
                    .map_or(Value::Null, |option| Value::String(option.name.into())),
            );
        }

        "parse_list_type_option" => {
            let option = declaration(action).map_err(Outcome::Failed)?;
            let value = action_str(action, "value").as_bytes().to_vec();
            row.insert("name".into(), Value::String(option.name.into()));
            row.insert(
                "value".into(),
                Value::String(action_str(action, "value").to_owned()),
            );
            // The pinned function refuses a list whose elements are objects by
            // panicking (commandlineparser.go:378) and so does the port
            // (crates/tsr_tsoptions/src/fixture_options.rs:101). The FACT of the
            // refusal is recorded on both sides; neither wording is.
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            let caught =
                std::panic::catch_unwind(|| parse_list_type_option(option, value.as_slice()));
            std::panic::set_hook(previous);
            match caught {
                Err(_) => {
                    row.insert("refused".into(), json!(true));
                    row.insert("result".into(), Value::Null);
                    row.insert("errors".into(), Value::Null);
                }
                Ok((result, errors)) => {
                    row.insert("refused".into(), json!(false));
                    row.insert("result".into(), config_value(&result));
                    row.insert(
                        "result_is_nil".into(),
                        json!(matches!(result, ConfigValue::Array(None))),
                    );
                    row.insert("errors".into(), diagnostic_rows!(errors));
                }
            }
        }

        "lib_file_name" => {
            let input = action_str(action, "value");
            row.insert("input".into(), Value::String(input.to_owned()));
            match lib_file_name(input.as_bytes()) {
                None => {
                    row.insert("found".into(), json!(false));
                    row.insert("file_name".into(), Value::Null);
                }
                Some(found) => {
                    row.insert("found".into(), json!(true));
                    row.insert("file_name".into(), Value::String(found.into()));
                }
            }
        }

        "default_lib_file_name" => {
            let target = i32::try_from(
                action
                    .get("target")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
            )
            .map_err(|_| Outcome::Failed("script target does not fit an i32".into()))?;
            row.insert("target".into(), json!(target));
            let options = CompilerOptions {
                target: ScriptTarget(target),
                ..CompilerOptions::default()
            };
            row.insert(
                "file_name".into(),
                Value::String(default_lib_file_name(&options).into()),
            );
        }

        "parsed_config" => return config_probe(request, action, parsed),

        "parse_command_line" => return Err(gap(PARSE_COMMAND_LINE)),
        "parse_build_command_line" => return Err(gap(PARSE_BUILD_COMMAND_LINE)),
        "input_option_name" => return Err(gap(INPUT_OPTION_NAME)),
        "invalid_enum_type_diagnostic" => return Err(gap(INVALID_ENUM_TYPE_DIAGNOSTIC)),
        "extra_key_diagnostics" => return Err(gap(EXTRA_KEY_DIAGNOSTICS)),
        "worker_diagnostics" => return Err(gap(WORKER_DIAGNOSTICS)),
        "canonical_key" => return Err(gap(TO_CANONICAL_KEY)),
        "wildcard_directory_from_spec" => return Err(gap(WILDCARD_DIRECTORY_FROM_SPEC)),
        "wildcard_directories" => return Err(gap(WILDCARD_DIRECTORIES)),

        other => {
            return Err(Outcome::Failed(format!(
                "unknown command-line action {other:?}"
            )))
        }
    }
    Ok(Value::Object(row))
}

fn gap((operation, authority, signature, home): Gap) -> Outcome {
    Outcome::missing(operation, authority, signature, home)
}

/// Drive one `ParsedCommandLine` accessor, or report it missing.
fn config_probe(
    request: &Value,
    action: &Value,
    parsed: &mut Option<ParsedCommandLine>,
) -> Result<Value, Outcome> {
    let requested = action_str(action, "probe");
    let (probe, argument) = requested.split_once(':').unwrap_or((requested, ""));
    if let Some((_, record)) = ACCESSOR_GAPS.iter().find(|(name, _)| *name == probe) {
        return Err(gap(*record));
    }
    if parsed.is_none() {
        *parsed = Some(config_parse(request).map_err(Outcome::Failed)?);
    }
    let parsed = parsed.as_ref().expect("config parse");
    let mut row = Map::new();
    row.insert("op".into(), Value::String("parsed_config".into()));
    row.insert("probe".into(), Value::String(requested.to_owned()));
    match probe {
        "config_name" => {
            row.insert("config_name".into(), text(parsed.config_name().as_bytes()));
        }
        "file_names" => {
            row.insert("file_names".into(), strings(&parsed.root_file_names));
        }
        "config_file_parsing_diagnostics" => {
            let diagnostics = parsed.config_file_parsing_diagnostics();
            row.insert("diagnostics".into(), diagnostic_rows!(diagnostics));
        }
        "matched_file_spec" => {
            row.insert("file".into(), Value::String(argument.to_owned()));
            row.insert(
                "matched".into(),
                text(parsed.matched_file_spec(argument.as_bytes())),
            );
        }
        "matched_include_spec" => {
            let (spec, is_default) = parsed.matched_include_spec(argument.as_bytes());
            row.insert("file".into(), Value::String(argument.to_owned()));
            row.insert("matched".into(), text(spec));
            row.insert("is_default_include".into(), json!(is_default));
        }
        other => {
            return Err(Outcome::Failed(format!(
                "unknown parsed-config probe {other:?}"
            )))
        }
    }
    Ok(Value::Object(row))
}

// ---------------------------------------------------------------------------
// Entry point.
// ---------------------------------------------------------------------------

pub fn observe(request: &Value) -> Option<Outcome> {
    if subject(request) != "commandLine" {
        return None;
    }
    let mut parsed: Option<ParsedCommandLine> = None;
    let mut trace = Vec::new();
    for action in actions(request) {
        match run(request, action, &mut parsed) {
            Ok(row) => trace.push(row),
            // The first action the port cannot answer decides the whole case:
            // a request carries one result, and an unported step invalidates
            // every step after it.
            Err(outcome) => return Some(outcome),
        }
    }
    Some(Outcome::Observed(ordered(trace)))
}
