//! The config-parsing group: `internal/tsoptions/tsconfigparsing.go`,
//! `parsinghelpers.go` and `wildcarddirectories.go`.
//!
//! Both source-file and raw-JSON entry points, including wildcard directory
//! calculation, execute the production `tsr_tsoptions` port. Remaining gaps
//! (extended-config caching, watch/build options and private helper entry
//! points) are reported at the handler that encounters them; this driver
//! never implements a missing compiler operation.

use crate::api::{subject, Outcome};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use tsr_core::CompilerOptions;
use tsr_jsstring::{JsString, SourceText};
use tsr_tsoptions::{
    config_mappers, file_names_from_specs, get_parsed_command_line_of_config_file, is_option_value,
    option_declaration, parse_config_file_text_to_json, parse_json_source_file_config_file_content,
    parse_number, parse_string, parse_string_array, parse_string_map, parse_tristate,
    spec_diagnostic, starts_with_config_dir, substitute_path, ConfigFileSpecs, ConfigValue,
    ParseConfigHost, ParsedCommandLine, TsConfigSourceFile,
};
use tsr_vfs::{Error, FileSystem, MemoryBuilder};

// --- host --------------------------------------------------------------------

/// The same assembly every other caller of the Rust config parse performs;
/// `tools/s07/config/host.rs:27` is the existing one. The pinned
/// `tsoptionstest` factory that does this in Go has no Rust counterpart, which
/// `tools/phase1/config/src/parseconfighost.rs` already records as its own gap.
struct Host {
    fs: Arc<dyn FileSystem>,
    cwd: JsString,
}
#[allow(
    clippy::needless_pass_by_value,
    reason = "this conversion consumes the error supplied by Result::map_err"
)]
fn module_error(error: tsr_module::Error) -> Error {
    match error {
        tsr_module::Error::Host(error) => error,
        tsr_module::Error::MutableHost => {
            Error::Unsupported("config resolver requires an immutable host")
        }
        tsr_module::Error::Unsupported(reason) => Error::Unsupported(reason),
        tsr_module::Error::MalformedPackageJson(_) => Error::Unsupported("malformed package JSON"),
    }
}
impl ParseConfigHost for Host {
    fn fs(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
    fn current_directory(&self) -> &[u8] {
        self.cwd.as_bytes()
    }
    fn resolve_config(&self, name: &[u8], containing: &[u8]) -> Result<Option<JsString>, Error> {
        let result =
            tsr_module::resolve_config(name, containing, self.fs.clone(), self.cwd.as_bytes())
                .map_err(module_error)?;
        Ok((!result.resolved_file_name.is_empty()).then_some(result.resolved_file_name))
    }
    fn resolve_content_mapper(
        &self,
        containing: &[u8],
        package: &[u8],
    ) -> Result<config_mappers::MapperResolution, Error> {
        tsr_module::resolve_content_mapper_manifest(
            &self.fs,
            self.cwd.as_bytes(),
            containing,
            package,
        )
        .map_err(module_error)
    }
}

// --- request accessors -------------------------------------------------------

fn text_of<'a>(request: &'a Value, key: &str) -> &'a str {
    request.get(key).and_then(Value::as_str).unwrap_or_default()
}
fn flag(request: &Value, key: &str) -> bool {
    request.get(key).and_then(Value::as_bool).unwrap_or(false)
}
fn list_of(request: &Value, key: &str) -> Vec<Vec<u8>> {
    request
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().as_bytes().to_vec())
                .collect()
        })
        .unwrap_or_default()
}
fn strings_of(request: &Value, key: &str) -> Vec<JsString> {
    list_of(request, key)
        .into_iter()
        .map(JsString::from_bytes)
        .collect()
}
fn base_of(request: &Value) -> Vec<u8> {
    let declared = text_of(request, "basePath");
    if declared.is_empty() {
        text_of(request, "currentDirectory").as_bytes().to_vec()
    } else {
        declared.as_bytes().to_vec()
    }
}
/// A request with no `report` list asks for every section the action offers.
fn wants(request: &Value, section: &str) -> bool {
    match request.get("report").and_then(Value::as_array) {
        None => true,
        Some(sections) if sections.is_empty() => true,
        Some(sections) => sections.iter().any(|name| name.as_str() == Some(section)),
    }
}
fn build_host(request: &Value) -> Host {
    let cwd = text_of(request, "currentDirectory").as_bytes().to_vec();
    let case_sensitive = flag(request, "caseSensitive");
    let mut builder = MemoryBuilder::new(&cwd, case_sensitive);
    if let Some(files) = request.get("files").and_then(Value::as_object) {
        for (path, content) in files {
            builder.insert_physical(
                path.as_bytes(),
                content.as_str().unwrap_or_default().as_bytes().to_vec(),
            );
        }
    }
    Host {
        fs: Arc::new(builder.finish()),
        cwd: JsString::from_bytes(cwd),
    }
}

// --- the tagged value encoding the probe and this module share ---------------

fn decode_value(value: &Value) -> Result<ConfigValue, String> {
    let Some(parts) = value.as_array() else {
        return Err("a tagged value must be a JSON array".into());
    };
    let Some(tag) = parts.first().and_then(Value::as_str) else {
        return Err("a tagged value's first element must be the tag".into());
    };
    let payload = parts.get(1);
    match tag {
        "null" => Ok(ConfigValue::Null),
        "emptyStruct" => Ok(ConfigValue::EmptyStruct),
        "bool" => Ok(ConfigValue::Boolean(
            payload.and_then(Value::as_bool).ok_or("bool payload")?,
        )),
        "number" => Ok(ConfigValue::Number(
            payload.and_then(Value::as_f64).ok_or("number payload")?,
        )),
        "int" => Ok(ConfigValue::Integer(
            payload.and_then(Value::as_i64).ok_or("int payload")?,
        )),
        "string" => Ok(ConfigValue::String(JsString::from_bytes(
            payload
                .and_then(Value::as_str)
                .ok_or("string payload")?
                .as_bytes(),
        ))),
        "nilarray" => Ok(ConfigValue::Array(None)),
        "array" => {
            let items = payload.and_then(Value::as_array).ok_or("array payload")?;
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(decode_value(item)?);
            }
            Ok(ConfigValue::Array(Some(values)))
        }
        "object" => {
            let items = payload.and_then(Value::as_array).ok_or("object payload")?;
            let mut entries = tsr_core::collections::OrderedMap::with_capacity(items.len());
            for item in items {
                let pair = item.as_array().ok_or("object entry")?;
                let key = pair.first().and_then(Value::as_str).ok_or("entry key")?;
                let nested = decode_value(pair.get(1).ok_or("entry value")?)?;
                entries.insert(JsString::from_bytes(key.as_bytes()), nested);
            }
            Ok(ConfigValue::Object(entries))
        }
        // `strings` and `map` name Go's []string and map[string]any. They exist
        // in the encoding because which concrete Go type an `any` holds is part
        // of the pinned contract, and ConfigValue can express neither: it has
        // no untyped string slice and no UNORDERED map. That is not an
        // oversight in the encoding, it is the subject of
        // `tsconfigparsing.go:normalizeJsonValue` (:882-901), whose whole job
        // is to turn a Go `map[string]any` into a key-sorted OrderedMap. The
        // port has nothing to normalize, because its config values are ordered
        // the moment they are parsed.
        //
        // So a request carrying one of these tags is a GAP, not a harness
        // failure: the caller turns this error into Outcome::missing naming the
        // pinned operation. An earlier revision of this comment claimed no case
        // reaches here, and one does.
        other => Err(format!("{GO_ONLY_VALUE}: {other:?}")),
    }
}

/// Go's encoding/json writes 1.0 as `1` and a Rust f64 writes `1.0`, and the
/// comparison canonicalises the parsed documents, so an agreeing pair would
/// differ on nothing but the spelling. The text is the shared representation.
fn render_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}
fn text(value: &[u8]) -> String {
    String::from_utf8_lossy(value).into_owned()
}
fn render_value(value: &ConfigValue) -> Value {
    match value {
        ConfigValue::Null => json!(["null"]),
        ConfigValue::EmptyStruct => json!(["emptyStruct"]),
        ConfigValue::Boolean(value) => json!(["bool", value]),
        ConfigValue::Number(value) => json!(["number", render_number(*value)]),
        ConfigValue::Integer(value) => json!(["int", value]),
        ConfigValue::Enum(value) => json!(["enum", value]),
        ConfigValue::String(value) => json!(["string", text(value.as_bytes())]),
        ConfigValue::Array(None) => json!(["nilarray"]),
        ConfigValue::Array(Some(values)) => {
            json!(["array", values.iter().map(render_value).collect::<Vec<_>>()])
        }
        ConfigValue::Object(entries) => json!([
            "object",
            entries
                .iter()
                .map(|(key, value)| json!([text(key.as_bytes()), render_value(value)]))
                .collect::<Vec<_>>()
        ]),
    }
}
/// Go's `[]string` is nil or a slice; the Rust counterpart is `Option<Vec<_>>`.
fn render_strings(values: Option<&Vec<JsString>>) -> Value {
    match values {
        None => json!(["nilarray"]),
        Some(values) => json!([
            "strings",
            values
                .iter()
                .map(|value| text(value.as_bytes()))
                .collect::<Vec<_>>()
        ]),
    }
}
fn render_diagnostic(diagnostic: &tsr_ast::Diagnostic) -> Value {
    json!({
        "code": diagnostic.code,
        "args": diagnostic
            .message_args
            .iter()
            .map(|arg| text(arg.as_bytes()))
            .collect::<Vec<_>>(),
        "pos": diagnostic.loc.pos(),
        "end": diagnostic.loc.end(),
        "has_file": diagnostic.file.is_some(),
    })
}
fn render_diagnostics(diagnostics: &[tsr_ast::Diagnostic]) -> Value {
    Value::Array(diagnostics.iter().map(render_diagnostic).collect())
}

fn render_options(options: &CompilerOptions, names: &[Vec<u8>]) -> Result<Value, String> {
    let mut rows = Vec::with_capacity(names.len());
    for name in names {
        let string = |value: &JsString| json!(["string", text(value.as_bytes())]);
        let tristate = |value: tsr_core::Tristate| json!(["tristate", value.0]);
        let rendered = match name.as_slice() {
            b"outDir" => string(&options.out_dir),
            b"outFile" => string(&options.out_file),
            b"rootDir" => string(&options.root_dir),
            b"declarationDir" => string(&options.declaration_dir),
            b"baseUrl" => string(&options.base_url),
            b"tsBuildInfoFile" => string(&options.ts_build_info_file),
            b"generateCpuProfile" => string(&options.generate_cpu_profile),
            b"generateTrace" => string(&options.generate_trace),
            b"configFilePath" => string(&options.config_file_path),
            b"pathsBasePath" => string(&options.paths_base_path),
            b"rootDirs" => render_strings(options.root_dirs.as_ref()),
            b"typeRoots" => render_strings(options.type_roots.as_ref()),
            b"types" => render_strings(options.types.as_ref()),
            b"lib" => render_strings(options.lib.as_ref()),
            b"moduleSuffixes" => render_strings(options.module_suffixes.as_ref()),
            b"customConditions" => render_strings(options.custom_conditions.as_ref()),
            b"paths" => match &options.paths {
                None => json!(["null"]),
                Some(paths) => json!([
                    "object",
                    paths
                        .iter()
                        .map(|(key, values)| json!([
                            text(key.as_bytes()),
                            render_strings(values.as_ref())
                        ]))
                        .collect::<Vec<_>>()
                ]),
            },
            b"strict" => tristate(options.strict),
            b"allowJs" => tristate(options.allow_js),
            b"noEmit" => tristate(options.no_emit),
            b"skipLibCheck" => tristate(options.skip_lib_check),
            b"composite" => tristate(options.composite),
            b"declaration" => tristate(options.declaration),
            b"resolveJsonModule" => tristate(options.resolve_json_module),
            b"runExternalCode" => tristate(options.run_external_code),
            b"maxNodeModuleJsDepth" => match options.max_node_module_js_depth {
                None => json!(["null"]),
                Some(depth) => json!(["int", depth]),
            },
            b"target" => json!(["int", options.target.0]),
            b"module" => json!(["int", options.module.0]),
            b"moduleResolution" => json!(["int", options.module_resolution.0]),
            b"moduleDetection" => json!(["int", options.module_detection.0]),
            b"jsx" => json!(["int", options.jsx.0]),
            b"newLine" => json!(["int", options.new_line.0]),
            other => {
                return Err(format!(
                    "no projection for compiler option {:?}",
                    text(other)
                ))
            }
        };
        rows.push(json!([text(name), rendered]));
    }
    Ok(Value::Array(rows))
}

fn render_type_acquisition(types: Option<&tsr_tsoptions::TypeAcquisition>) -> Value {
    match types {
        None => Value::Null,
        Some(types) => json!([
            ["enable", json!(["tristate", types.enable.0])],
            ["include", render_strings(types.include.as_ref())],
            ["exclude", render_strings(types.exclude.as_ref())],
            [
                "disableFilenameBasedTypeAcquisition",
                json!(["tristate", types.disable_filename_based_type_acquisition.0])
            ],
        ]),
    }
}

// --- the shared projection of one parsed command line ------------------------

fn describe_parsed(
    request: &Value,
    parsed: &ParsedCommandLine,
) -> Result<Map<String, Value>, String> {
    let mut observation = Map::new();
    if wants(request, "file_names") {
        observation.insert(
            "file_names".into(),
            Value::Array(
                parsed
                    .root_file_names
                    .iter()
                    .map(|name| Value::String(text(name.as_bytes())))
                    .collect(),
            ),
        );
    }
    if wants(request, "errors") {
        observation.insert("errors".into(), render_diagnostics(&parsed.errors));
    }
    if wants(request, "error_codes") {
        observation.insert(
            "error_codes".into(),
            Value::Array(
                parsed
                    .errors
                    .iter()
                    .map(|diagnostic| json!(diagnostic.code))
                    .collect(),
            ),
        );
    }
    if wants(request, "options") {
        observation.insert(
            "options".into(),
            render_options(&parsed.options, &list_of(request, "optionNames"))?,
        );
    }
    if wants(request, "type_acquisition") {
        observation.insert(
            "type_acquisition".into(),
            render_type_acquisition(parsed.type_acquisition.as_ref()),
        );
    }
    if wants(request, "raw") {
        observation.insert("raw".into(), render_value(&parsed.raw));
    }
    if wants(request, "raw_keys") {
        observation.insert(
            "raw_keys".into(),
            Value::Array(
                parsed
                    .raw
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(key, _)| Value::String(text(key.as_bytes())))
                    .collect(),
            ),
        );
    }
    if wants(request, "ordered") {
        // The same top-level key list as `raw_keys`, under the name an
        // order-sensitive request must use: the comparison canonicalises with
        // sorted keys, so JSON member order survives only inside an array.
        observation.insert(
            "ordered".into(),
            Value::Array(
                parsed
                    .raw
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(key, _)| Value::String(text(key.as_bytes())))
                    .collect(),
            ),
        );
    }
    if wants(request, "syntax_diagnostics") {
        // The config source file's OWN parser diagnostics, which
        // config_file_parsing_diagnostics reports alongside parsed.errors.
        let syntax = parsed.config_file.as_ref().map_or_else(Vec::new, |config| {
            config
                .file
                .view()
                .source_file(config.root)
                .expect("config source")
                .diagnostics
                .clone()
        });
        observation.insert("syntax_diagnostics".into(), render_diagnostics(&syntax));
    }
    if wants(request, "compile_on_save") {
        observation.insert(
            "compile_on_save".into(),
            parsed.compile_on_save.map_or(Value::Null, Value::Bool),
        );
    }
    if wants(request, "extended_source_files") {
        observation.insert(
            "extended_source_files".into(),
            Value::Array(
                parsed
                    .config_file
                    .as_ref()
                    .map(|config| config.extended_source_files.as_slice())
                    .unwrap_or_default()
                    .iter()
                    .map(|name| Value::String(text(name.as_bytes())))
                    .collect(),
            ),
        );
    }
    if wants(request, "project_references") {
        observation.insert(
            "project_references".into(),
            Value::Array(
                parsed
                    .project_references
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|reference| {
                        json!([
                            text(reference.path.as_bytes()),
                            text(reference.original_path.as_bytes()),
                            reference.circular
                        ])
                    })
                    .collect(),
            ),
        );
    }
    if wants(request, "content_mappers") {
        observation.insert(
            "content_mappers".into(),
            Value::Array(
                parsed
                    .content_mappers
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|mapper| {
                        json!([
                            text(mapper.package.as_bytes()),
                            mapper
                                .extensions
                                .iter()
                                .map(|extension| text(extension.as_bytes()))
                                .collect::<Vec<_>>(),
                            mapper.options.as_deref().map(text).unwrap_or_default()
                        ])
                    })
                    .collect(),
            ),
        );
    }
    Ok(observation)
}

fn parse_source(
    request: &Value,
    host: &Host,
    name: &[u8],
    source_text: &[u8],
) -> Result<ParsedCommandLine, Error> {
    let path = tsr_tspath::to_path(
        name,
        text_of(request, "currentDirectory").as_bytes(),
        flag(request, "caseSensitive"),
    );
    let source = TsConfigSourceFile::parse(
        JsString::from_bytes(name),
        path,
        SourceText::from_loaded_bytes(source_text.to_vec()),
    );
    parse_json_source_file_config_file_content(
        source,
        host,
        &base_of(request),
        &CompilerOptions::default(),
        &ConfigValue::Null,
        name,
    )
}

// --- the reviewed gap records -------------------------------------------------

type Gap = (&'static str, &'static str, &'static str, &'static str);

const CONVERT_TO_OBJECT: Gap = (
    "tsc/internal/tsoptions/tsconfigparsing.go:convertToObject",
    "tsc/internal/tsoptions/tsconfigparsing.go:918-925, the circularity branch's converter: unlike \
     convertConfigFileToObject it calls convertToJson directly and so does NOT report \
     The_root_value_of_a_0_file_must_be_an_object for a non-object root",
    "pub fn convert_to_object(config: &TsConfigSourceFile) -> (ConfigValue, Vec<Diagnostic>)",
    "crates/tsr_tsoptions/src/config_text.rs, which exports convert_config_file_to_object (:98) \
     -- the root-checking variant -- and nothing that skips the check (absent)",
);
const NORMALIZE_JSON_VALUE: Gap = (
    "tsc/internal/tsoptions/tsconfigparsing.go:normalizeJsonValue",
    "tsc/internal/tsoptions/tsconfigparsing.go:882-916, which turns a map[string]any into a \
     key-SORTED OrderedMap, leaves an existing OrderedMap's order alone, rewrites any slice or \
     array through reflection and maps a nil slice to nil",
    "pub fn normalize_json_value(value: &ConfigValue) -> ConfigValue",
    "no Rust home: ConfigValue (crates/tsr_tsoptions/src/config_value.rs:6) has no unordered-map \
     or typed-slice variant to normalize, because the value-mode API that needs one is absent",
);
const EXTENDED_CONFIG: Gap = (
    "tsc/internal/tsoptions/tsconfigparsing.go:ParseExtendedConfig",
    "tsc/internal/tsoptions/tsconfigparsing.go:1052-1078, which reads the extended config through \
     readJsonConfigFile, returns an ExtendedConfigCacheEntry carrying the source file, the parsed \
     config and the errors, and stops at the first of read errors or parse diagnostics",
    "pub fn parse_extended_config(name: &[u8], path: JsString, stack: &[JsString], \
     host: &dyn ParseConfigHost, cache: Option<&dyn ExtendedConfigCache>) \
     -> Result<ExtendedConfigCacheEntry, Error>",
    "crates/tsr_tsoptions/src/config_parse.rs:440-509, which reads, parses and merges each \
     extended config INLINE inside parse_config; there is no separable entry point, no cache \
     entry type and no ExtendedConfigCache trait anywhere in crates/",
);
const GET_EXTENDED_CONFIG: Gap = (
    "tsc/internal/tsoptions/tsconfigparsing.go:getExtendedConfig",
    "tsc/internal/tsoptions/tsconfigparsing.go:1015-1050, which consults the supplied \
     ExtendedConfigCache -- bypassing it on a resolution-stack cycle -- and re-emits the cached \
     entry's errors on every hit, so a second parse of the same base still reports them",
    "pub fn get_extended_config(config: Option<&TsConfigSourceFile>, name: &[u8], \
     host: &dyn ParseConfigHost, stack: &[JsString], cache: Option<&dyn ExtendedConfigCache>) \
     -> Result<(Option<Parsed>, Vec<Diagnostic>), Error>",
    "crates/tsr_tsoptions/src/config_parse.rs:440-509: the extended read is inline and there is \
     no cache, so a case that supplies one has nothing to supply it to (absent)",
);

const CONVERT_OPTION_TO_ABSOLUTE_PATH: Gap = (
    "tsc/internal/tsoptions/parsinghelpers.go:ConvertOptionToAbsolutePath",
    "tsc/internal/tsoptions/parsinghelpers.go:711-738, which resolves the option declaration, \
     absolutizes a list option's elements when the ELEMENT is a file path and a scalar option's \
     value when the option itself is, and reports whether it converted anything",
    "pub fn convert_option_to_absolute_path(name: &[u8], value: &ConfigValue, cwd: &[u8]) \
     -> Option<ConfigValue>",
    "crates/tsr_tsoptions/src/convert_options.rs:161-177 absolutizes a file-path option DURING \
     conversion from JSON, against the config's base path; the pinned post-hoc conversion of an \
     already-parsed option map against an arbitrary cwd has no entry point (absent)",
);
const CONVERT_TO_OPTIONS_WITH_ABSOLUTE_PATHS: Gap = (
    "tsc/internal/tsoptions/parsinghelpers.go:convertToOptionsWithAbsolutePaths",
    "tsc/internal/tsoptions/parsinghelpers.go:696-709, which walks an option map in place and \
     replaces every entry ConvertOptionToAbsolutePath converted",
    "pub fn convert_to_options_with_absolute_paths(options: &mut ConfigValue, cwd: &[u8])",
    "crates/tsr_tsoptions/src/convert_options.rs (absent); its single caller in the pin is \
     commandlineparser.go:51, and the port has no argument-vector parser either",
);
const COMMAND_LINE_OPTIONS_TO_MAP: Gap = (
    "tsc/internal/tsoptions/tsconfigparsing.go:commandLineOptionsToMap",
    "tsc/internal/tsoptions/tsconfigparsing.go:615-622, which stores EVERY declaration twice, \
     under its exact name and under its lowercased name, which is what makes \
     CommandLineOptionNameMap.Get's two-step lookup work",
    "pub fn option_name_map(options: &[OptionDeclaration]) -> BTreeMap<Vec<u8>, &OptionDeclaration>",
    "crates/tsr_tsoptions/src/option_declarations.rs:83-104 replaces the doubled map with a \
     case-insensitive linear scan over the declaration slice, so no map is built and the \
     operation has no counterpart to observe (fused into find_declaration)",
);
const GET_SPELLING_SUGGESTION: Gap = (
    "tsc/internal/tsoptions/tsconfigparsing.go:CommandLineOptionNameMap.GetSpellingSuggestion",
    "tsc/internal/tsoptions/tsconfigparsing.go:606-613, core.GetSpellingSuggestion over the \
     declarations, tie-broken by option name",
    "pub fn spelling_suggestion(name: &[u8]) -> Option<&'static OptionDeclaration>",
    "IMPLEMENTED BUT UNREACHABLE: crates/tsr_tsoptions/src/config_parse.rs:158-165 and \
     convert_options.rs:304-319 both run tsr_scanner::get_spelling_suggestion_for_strings over \
     COMPILER_OPTIONS, but neither `unknown` nor `unknown_option` is exported from the crate \
     (lib.rs:273-276 re-exports neither), so no caller outside tsr_tsoptions can reach it",
);
const HAS_FILE_WITH_HIGHER_PRIORITY_EXTENSION: Gap = (
    "tsc/internal/tsoptions/tsconfigparsing.go:hasFileWithHigherPriorityExtension",
    "tsc/internal/tsoptions/tsconfigparsing.go:1875-1903, the extension-priority test, including \
     the legacy exemption that lets a .d.ts sit beside its .js or .jsx counterpart",
    "pub fn has_file_with_higher_priority_extension(file: &[u8], extensions: &[Vec<JsString>], \
     has_file: &dyn Fn(&[u8]) -> bool) -> bool",
    "IMPLEMENTED BUT UNREACHABLE: crates/tsr_tsoptions/src/config_files.rs:39 is the port, and it \
     is a private fn in a private module; lib.rs:286-287 re-exports only file_names_from_specs, \
     so the operation can only be observed through the whole expansion",
);
const PROP_ARRAY_ELEMENT: Gap = (
    "tsc/internal/tsoptions/tsconfigparsing.go:GetTsConfigPropArrayElementValue",
    "tsc/internal/tsoptions/tsconfigparsing.go:1613-1621, which finds the string literal with the \
     given text inside the named top-level array property, and is what attaches a spec diagnostic \
     to the exact element",
    "pub fn prop_array_element(config: &TsConfigSourceFile, key: &[u8], value: &[u8]) \
     -> Option<NodeId>",
    "IMPLEMENTED BUT UNREACHABLE: crates/tsr_tsoptions/src/config_parse.rs:105-143 (`array_string`) \
     is the port and is a private fn; lib.rs re-exports find_property and \
     find_property_in_object but not this one, so a caller can find the PROPERTY and not the \
     element",
);
const OPTIONS_SYNTAX_BY_ARRAY_ELEMENT_VALUE: Gap = (
    "tsc/internal/tsoptions/tsconfigparsing.go:GetOptionsSyntaxByArrayElementValue",
    "tsc/internal/tsoptions/tsconfigparsing.go:1674-1676, the same element search over an \
     ARBITRARY object literal rather than the config root, which is how the program attributes a \
     types or lib diagnostic",
    "pub fn options_syntax_by_array_element_value(config: &TsConfigSourceFile, object: NodeId, \
     key: &[u8], value: &[u8]) -> Option<NodeId>",
    "crates/tsr_tsoptions/src/config_parse.rs:105-143 searches only from the config ROOT object \
     and is private; nothing in crates/ takes an arbitrary object literal (absent)",
);
const IS_DOUBLE_QUOTED_STRING: Gap = (
    "tsc/internal/tsoptions/tsconfigparsing.go:isDoubleQuotedString",
    "tsc/internal/tsoptions/tsconfigparsing.go:814-816, ast.IsStringLiteral under another name; \
     its only caller is the KindStringLiteral arm of convertPropertyValueToJson (:827-831), where \
     it is therefore always true and the guarded diagnostic unreachable",
    "pub fn is_double_quoted_string(config: &TsConfigSourceFile, node: NodeId) -> bool",
    "crates/tsr_tsoptions/src/config_text.rs:165-170 omits the predicate and the unreachable \
     diagnostic with it; there is no entry point to call (absent, deliberately)",
);
const PARSE_PROJECT_REFERENCE: Gap = (
    "tsc/internal/tsoptions/parsinghelpers.go:parseProjectReference",
    "tsc/internal/tsoptions/parsinghelpers.go:83-103, which reports separately whether `path` and \
     `circular` were PRESENT and whether each was of the right type, so the caller can tell a \
     missing property from a wrongly typed one",
    "pub fn parse_project_reference(value: &ConfigValue) -> Option<ProjectReferenceParse>",
    "crates/tsr_tsoptions/src/config_parse.rs:629-687 (`references`) fuses the parse into the \
     validation loop and never materialises the four present/valid flags; nothing in crates/ \
     parses one reference object on its own (fused)",
);
const IS_STRING_VALUE: Gap = (
    "tsc/internal/tsoptions/tsconfigparsing.go:isStringValue",
    "tsc/internal/tsoptions/tsconfigparsing.go:1226-1229, the element validator \
     parseJsonConfigFileContentWorker passes to getPropFromRaw for files, include and exclude",
    "pub fn is_string_value(value: &ConfigValue) -> bool",
    "crates/tsr_tsoptions/src/config_parse.rs:577-598 filters spec arrays with \
     ConfigValue::as_string inline and never names the predicate; the pinned per-element \
     validation callback has no counterpart (fused)",
);
const SUBSTITUTED_STRING_ARRAY: Gap = (
    "tsc/internal/tsoptions/tsconfigparsing.go:getSubstitutedStringArrayWithConfigDirTemplate",
    "tsc/internal/tsoptions/tsconfigparsing.go:1807-1821, which clones the list ONLY when some \
     element starts with the template and returns nil otherwise, so a caller can tell \
     'nothing to substitute' from 'substituted to the same text'",
    "pub fn substitute_string_array(values: &[JsString], base: &[u8]) -> Option<Vec<JsString>>",
    "IMPLEMENTED BUT UNREACHABLE: crates/tsr_tsoptions/src/config_substitution.rs:21-27 \
     (`substitute_strings`) is the port -- in place, with no nil signal -- and lib.rs:288-289 \
     re-exports only starts_with_config_dir, substitute_options and substitute_path, so no caller \
     outside the crate can reach it",
);
const CREATE_DIAGNOSTIC_AT_REFERENCE_SYNTAX: Gap = (
    "tsc/internal/tsoptions/tsconfigparsing.go:CreateDiagnosticAtReferenceSyntax",
    "tsc/internal/tsoptions/tsconfigparsing.go:1630-1640, which attributes a diagnostic to the \
     index-th element of the config's `references` array, or returns nil when the config has no \
     such element",
    "pub fn diagnostic_at_reference_syntax(config: &ParsedCommandLine, index: usize, \
     message: &'static Message, args: Vec<JsString>) -> Option<Diagnostic>",
    "crates/tsr_tsoptions/src/config_parse.rs:96-104 (`array_element`) can reach the element and \
     is private; no public function builds a diagnostic at a project reference (absent). Its \
     pinned caller is compiler/program.go:1368",
);

fn missing(gap: Gap) -> Outcome {
    let (operation, authority, signature, home) = gap;
    Outcome::missing(operation, authority, signature, home)
}

// --- the group ---------------------------------------------------------------

#[allow(clippy::too_many_lines, reason = "one arm per reviewed pinned action")]
pub fn observe(request: &Value) -> Option<Outcome> {
    if subject(request) != "configParse" {
        return None;
    }
    Some(answer(request).unwrap_or_else(|error| {
        if error.starts_with(GO_ONLY_VALUE) {
            // The pinned value has a Go type the port's ConfigValue cannot
            // hold, which is exactly what the pinned operation exists to
            // normalize away. Record the gap; do not fail the capture.
            return Outcome::missing(
                "tsc/internal/tsoptions/tsconfigparsing.go:normalizeJsonValue",
                "tsc/internal/tsoptions/tsconfigparsing.go:normalizeJsonValue (:882-901), which \
                 turns a Go map[string]any into a key-sorted OrderedMap, walks an existing \
                 OrderedMap in place, rewrites a typed slice through reflection and maps a nil \
                 slice to nil",
                "pub fn normalize_json_value(value: ConfigValue) -> ConfigValue -- absent, and \
                 absent for a reason: ConfigValue has no unordered map and no untyped string \
                 slice, so there is no unordered input for it to sort. Closing this gap means \
                 deciding whether the port needs that shape at all, not writing the function",
                "crates/tsr_tsoptions/src/config_value.rs (no unordered map variant exists)",
            );
        }
        Outcome::Failed(error)
    }))
}

/// Marks the one failure that is a recorded gap rather than a broken harness.
const GO_ONLY_VALUE: &str = "go-only config value";

fn failed<T>(result: Result<T, Error>) -> Result<T, String> {
    result.map_err(|error| format!("filesystem: {error}"))
}

#[allow(clippy::too_many_lines, reason = "one arm per reviewed pinned action")]
fn answer(request: &Value) -> Result<Outcome, String> {
    let action = text_of(request, "action");
    let helper = text_of(request, "helper");
    let target = text_of(request, "target");
    match action {
        "parse_source_file" => {
            if flag(request, "useCache") {
                // The request asks for a shared ExtendedConfigCache. The Rust
                // parse has nothing to give one to, so the honest answer is the
                // gap, not a cacheless run dressed up as agreement.
                return Ok(missing(GET_EXTENDED_CONFIG));
            }
            let host = build_host(request);
            let name = text_of(request, "configFileName").as_bytes().to_vec();
            let also = text_of(request, "parseAlso");
            if !also.is_empty() {
                let Some(content) = failed(host.fs().read_file(also.as_bytes()))? else {
                    return Err(format!("parseAlso names {also:?}, which is not in files"));
                };
                failed(parse_source(
                    request,
                    &host,
                    also.as_bytes(),
                    content.raw.as_ref(),
                ))?;
            }
            let source_text = text_of(request, "jsonText").as_bytes().to_vec();
            let mut parsed = failed(parse_source(request, &host, &name, &source_text))?;
            if flag(request, "parseTwice") {
                parsed = failed(parse_source(request, &host, &name, &source_text))?;
            }
            Ok(Outcome::Observed(Value::Object(describe_parsed(
                request, &parsed,
            )?)))
        }
        "read_config_file" => {
            if flag(request, "useCache") {
                return Ok(missing(GET_EXTENDED_CONFIG));
            }
            let host = build_host(request);
            let also = text_of(request, "parseAlso");
            if !also.is_empty() {
                failed(get_parsed_command_line_of_config_file(
                    also.as_bytes(),
                    &CompilerOptions::default(),
                    &ConfigValue::Null,
                    &host,
                ))?;
            }
            let result = failed(get_parsed_command_line_of_config_file(
                text_of(request, "configFileName").as_bytes(),
                &CompilerOptions::default(),
                &ConfigValue::Null,
                &host,
            ))?;
            let Some(parsed) = result.command_line else {
                return Ok(Outcome::Observed(json!({
                    "read_failed": true,
                    "errors": render_diagnostics(&result.read_errors),
                })));
            };
            let mut observation = describe_parsed(request, &parsed)?;
            observation.insert("read_failed".into(), Value::Bool(false));
            Ok(Outcome::Observed(Value::Object(observation)))
        }
        "parse_json_api" | "parse_json_api_value" => {
            let host=build_host(request);
            let name=text_of(request,"configFileName").as_bytes();
            let raw=if action=="parse_json_api_value" { decode_value(&request["value"])? } else {
                let path=tsr_tspath::to_path(name,&base_of(request),flag(request,"caseSensitive"));
                parse_config_file_text_to_json(JsString::from_bytes(name),path,SourceText::from_loaded_bytes(text_of(request,"jsonText").as_bytes().to_vec())).value
            };
            let parsed=failed(tsr_tsoptions::parse_json_config_file_content(raw,&host,&base_of(request),&CompilerOptions::default(),name,&[]))?;
            Ok(Outcome::Observed(Value::Object(describe_parsed(request,&parsed)?)))
        },
        "parse_config_text" => {
            let name = text_of(request, "configFileName").as_bytes().to_vec();
            let path =
                tsr_tspath::to_path(&name, &base_of(request), flag(request, "caseSensitive"));
            let parsed = parse_config_file_text_to_json(
                JsString::from_bytes(name),
                path,
                SourceText::from_loaded_bytes(text_of(request, "jsonText").as_bytes().to_vec()),
            );
            Ok(Outcome::Observed(json!({
                "value": render_value(&parsed.value),
                "errors": render_diagnostics(&parsed.diagnostics),
            })))
        }
        "convert_to_object" => Ok(missing(CONVERT_TO_OBJECT)),
        "extended_config" => Ok(missing(EXTENDED_CONFIG)),
        "spec_diagnostic" => {
            let message = spec_diagnostic(
                text_of(request, "spec").as_bytes(),
                flag(request, "disallow"),
            );
            Ok(Outcome::Observed(json!({
                "code": message.map_or(0, |message| message.code),
                "reported": message.is_some(),
            })))
        }
        "config_dir" => match helper {
            "value" => {
                let base = base_of(request);
                let per_value: Vec<Value> = strings_of(request, "specs")
                    .iter()
                    .map(|value| {
                        json!([
                            text(value.as_bytes()),
                            starts_with_config_dir(value.as_bytes()),
                            text(substitute_path(value.as_bytes(), &base).as_bytes()),
                        ])
                    })
                    .collect();
                Ok(Outcome::Observed(json!({ "per_value": per_value })))
            }
            "array" => Ok(missing(SUBSTITUTED_STRING_ARRAY)),
            other => Err(format!("unknown config_dir helper {other:?}")),
        },
        "supported_extensions" => {
            let mut options = CompilerOptions::default();
            for (key, value) in option_entries(request)? {
                tsr_tsoptions::parse_compiler_options(&key, &value, &mut options);
            }
            let extra = strings_of(request, "extra");
            let render = |groups: Vec<Vec<JsString>>| {
                Value::Array(
                    groups
                        .into_iter()
                        .map(|group| {
                            Value::Array(
                                group
                                    .into_iter()
                                    .map(|extension| Value::String(text(extension.as_bytes())))
                                    .collect(),
                            )
                        })
                        .collect(),
                )
            };
            Ok(Outcome::Observed(json!({
                "supported": render(tsr_tsoptions::supported_extensions(&options, &extra)),
                "with_json": render(tsr_tsoptions::supported_extensions_with_json(&options, &extra)),
            })))
        }
        // The four unexported option parsers and the generic that dispatches
        // through them. `tsr_tsoptions` has no parser INTERFACE: it has one
        // `parse_compiler_options(key, value, &mut options)` free function
        // (parse_options.rs:57) and nothing at all for watch, type-acquisition
        // or build options, so there is no per-kind dispatch to compare and no
        // per-kind unknown-option message to read back.
        "parser_diagnostics" | "option_parser_parse_option" => {
            let kind = text_of(request, "parserKind");
            let parser = match kind {
                "compiler" => "compilerOptionsParser",
                "watch" => "watchOptionsParser",
                "typeAcquisition" => "typeAcquisitionParser",
                "build" => "buildOptionsParser",
                other => return Err(format!("no pinned parser named {other:?}")),
            };
            // The identity must be one the CASE claims, not just one on the
            // same interface: `record` refuses an unclaimed identity, and it is
            // right to -- a gap record naming a neighbour would attribute the
            // absence to the wrong operation. The two actions ask about
            // different methods of the same parser, so they name different ones.
            let method = if action == "parser_diagnostics" {
                "UnknownOptionDiagnostic"
            } else {
                "ParseOption"
            };
            Ok(Outcome::missing(
                format!("tsc/internal/tsoptions/parsinghelpers.go:{parser}.{method}"),
                "tsc/internal/tsoptions/parsinghelpers.go:198-266, the optionParser interface and \
                 its four implementations, each answering ParseOption plus its own \
                 UnknownOptionDiagnostic and UnknownDidYouMeanDiagnostic",
                "a per-kind option parser -- pub trait OptionParser { fn parse_option(&mut self, \
                 key: &[u8], value: &ConfigValue) -> Vec<Diagnostic>; fn unknown_option(&self) -> \
                 &'static Message; fn unknown_did_you_mean(&self) -> &'static Message } with an \
                 implementation for each of compiler, watch, type-acquisition and build options",
                "crates/tsr_tsoptions/src/parse_options.rs has one free                  parse_compiler_options(key, value, &mut options) at :57 and no watch,                  type-acquisition or build parser at all",
            ))
        }
        "convert_map_to_options" => Ok(Outcome::missing(
            "tsc/internal/tsoptions/tsconfigparsing.go:convertMapToOptions",
            "tsc/internal/tsoptions/tsconfigparsing.go:626-632, the generic that walks an \
             OrderedMap in order and dispatches each entry through optionParser.ParseOption",
            "pub fn convert_map_to_options<P: OptionParser>(entries: &[(JsString, ConfigValue)], \
             parser: &mut P) -- needs the parser trait above before it can exist",
            "crates/tsr_tsoptions/src/convert_options.rs (no interface-dispatching walk exists)",
        )),
        "content_mapper_diagnostic_location" => Ok(Outcome::missing(
            "tsc/internal/tsoptions/tsconfigparsing.go:GetContentMapperOptionDiagnosticLocation",
            "tsc/internal/tsoptions/tsconfigparsing.go:1706-1740, which locates the syntax node \
             for a content mapper's option path so a diagnostic can point at it",
            "pub fn content_mapper_option_diagnostic_location(config: &ParsedCommandLine, mapper: \
             &ContentMapper, path: &[OptionPathSegment]) -> Option<(AstFile, TextRange)>",
            "crates/tsr_tsoptions/src/config_mappers.rs carries mapper validation but no              option-path syntax lookup",
        )),
        "parse_value" => {
            let value = decode_value(request.get("value").ok_or("request has no value")?)?;
            match helper {
                "tristate" => Ok(Outcome::Observed(
                    json!({ "result": ["tristate", parse_tristate(&value).0] }),
                )),
                "string" => Ok(Outcome::Observed(
                    json!({ "result": ["string", text(parse_string(&value).as_bytes())] }),
                )),
                "string_array" => Ok(Outcome::Observed(
                    json!({ "result": render_strings(parse_string_array(&value).as_ref()) }),
                )),
                "string_map" => {
                    let result = parse_string_map(&value).map_or(json!(["null"]), |entries| {
                        json!([
                            "object",
                            entries
                                .iter()
                                .map(|(key, values)| json!([
                                    text(key.as_bytes()),
                                    render_strings(values.as_ref())
                                ]))
                                .collect::<Vec<_>>()
                        ])
                    });
                    Ok(Outcome::Observed(json!({ "result": result })))
                }
                "number" => Ok(Outcome::Observed(json!({
                    "result": parse_number(&value).map_or(json!(["null"]), |n| json!(["int", n])),
                }))),
                "string_array_strict" => {
                    let result = config_mappers::parse_string_array_strict(&value);
                    Ok(Outcome::Observed(json!({
                        "result": render_strings(result.as_ref()),
                        "accepted": result.is_some(),
                    })))
                }
                "is_option_value" => {
                    let option = option_declaration(text_of(request, "name").as_bytes(), false);
                    Ok(Outcome::Observed(json!({
                        "result": option.is_some_and(|option| is_option_value(option, &value)),
                        "known_option": option.is_some(),
                    })))
                }
                "content_mapper" => {
                    let (mapper, errors) = config_mappers::parse_content_mapper(&value);
                    Ok(Outcome::Observed(json!({
                        "present": mapper.is_some(),
                        "package": mapper.as_ref().map_or_else(String::new, |mapper| text(mapper.package.as_bytes())),
                        "extensions": render_strings(mapper.as_ref().map(|mapper| &mapper.extensions)),
                        "options": mapper
                            .as_ref()
                            .and_then(|mapper| mapper.options.as_deref())
                            .map(text)
                            .unwrap_or_default(),
                        "errors": render_diagnostics(&errors),
                    })))
                }
                "project_reference" => Ok(missing(PARSE_PROJECT_REFERENCE)),
                "is_string_value" => Ok(missing(IS_STRING_VALUE)),
                "normalize_json_value" => Ok(missing(NORMALIZE_JSON_VALUE)),
                other => Err(format!("unknown parse_value helper {other:?}")),
            }
        }
        "parse_option" => match target {
            "compiler" => {
                let value = decode_value(request.get("value").ok_or("request has no value")?)?;
                let mut options = tsr_tsoptions::default_compiler_options(
                    text_of(request, "configFileName").as_bytes(),
                );
                tsr_tsoptions::parse_compiler_options(
                    text_of(request, "key").as_bytes(),
                    &value,
                    &mut options,
                );
                Ok(Outcome::Observed(json!({
                    "options": render_options(&options, &list_of(request, "optionNames"))?,
                    // ParseCompilerOptions returns no diagnostics at the pin.
                    "errors": Value::Array(vec![]),
                })))
            }
            "type_acquisition" => {
                let value = decode_value(request.get("value").ok_or("request has no value")?)?;
                // TypeAcquisition::for_config is private, so the jsconfig.json
                // default cannot be requested here; a case that needs it uses a
                // whole parse instead. Default::default() is the tsconfig.json
                // default, which is what these cases ask for.
                let mut types = tsr_tsoptions::TypeAcquisition::default();
                types.parse_option(text_of(request, "key").as_bytes(), &value);
                Ok(Outcome::Observed(json!({
                    "type_acquisition": render_type_acquisition(Some(&types)),
                    "errors": Value::Array(vec![]),
                })))
            }
            "watch" => {
                let value = decode_value(request.get("value").ok_or("request has no value")?)?;
                let mut options = tsr_core::WatchOptions::default();
                tsr_tsoptions::parse_watch_options(text_of(request, "key").as_bytes(), &value, &mut options);
                Ok(Outcome::Observed(json!({
                    "watch": [
                        ["watchInterval", options.interval.map_or_else(|| json!(["null"]), |v| json!(["int",v]))],
                        ["watchFile", ["int", options.file_kind.0]],
                        ["watchDirectory", ["int", options.directory_kind.0]],
                        ["fallbackPolling", ["int", options.fallback_polling.0]],
                        ["synchronousWatchDirectory", ["tristate", options.sync_watch_dir.0]],
                        ["excludeDirectories", render_strings(options.exclude_dir.as_ref())],
                        ["excludeFiles", render_strings(options.exclude_files.as_ref())],
                    ], "errors": []
                })))
            }
            "build" => {
                let value = decode_value(request.get("value").ok_or("request has no value")?)?;
                let mut options = tsr_core::BuildOptions::default();
                tsr_tsoptions::parse_build_options(text_of(request, "key").as_bytes(), &value, &mut options);
                Ok(Outcome::Observed(json!({
                    "build": [
                        ["clean", ["tristate", options.clean.0]],
                        ["dry", ["tristate", options.dry.0]],
                        ["force", ["tristate", options.force.0]],
                        ["builders", options.builders.map_or_else(|| json!(["null"]), |v| json!(["int",v]))],
                        ["stopBuildOnErrors", ["tristate", options.stop_build_on_errors.0]],
                        ["verbose", ["tristate", options.verbose.0]],
                    ], "errors": []
                })))
            }
            other => Err(format!("unknown parse_option target {other:?}")),
        },
        "default_options" => {
            if wants(request, "type_acquisition") {
                return Ok(missing((
                    "tsc/internal/tsoptions/tsconfigparsing.go:getDefaultTypeAcquisition",
                    "tsc/internal/tsoptions/tsconfigparsing.go:941-947, which enables type \
                     acquisition when and only when the config's base name is jsconfig.json",
                    "pub fn default_type_acquisition(config_file_name: &[u8]) -> TypeAcquisition",
                    "IMPLEMENTED BUT UNREACHABLE: crates/tsr_tsoptions/src/config_parse.rs:39-48 \
                     (`TypeAcquisition::for_config`) is the port and is a private associated \
                     function; lib.rs:295-297 exports the type but not the constructor, so it can \
                     only be observed through a whole config parse",
                )));
            }
            let options = tsr_tsoptions::default_compiler_options(
                text_of(request, "configFileName").as_bytes(),
            );
            Ok(Outcome::Observed(json!({
                "options": render_options(&options, &list_of(request, "optionNames"))?,
            })))
        }
        "option_absolute_path" => match helper {
            "one" => Ok(missing(CONVERT_OPTION_TO_ABSOLUTE_PATH)),
            "all" => Ok(missing(CONVERT_TO_OPTIONS_WITH_ABSOLUTE_PATHS)),
            other => Err(format!("unknown option_absolute_path helper {other:?}")),
        },
        "option_name_map" => match helper {
            "get" => {
                let rows: Vec<Value> = list_of(request, "query")
                    .into_iter()
                    .map(|name| {
                        let found = option_declaration(&name, false)
                            .map_or_else(String::new, |option| option.name.to_owned());
                        json!([text(&name), found])
                    })
                    .collect();
                Ok(Outcome::Observed(json!({ "resolved": rows })))
            }
            "spelling" => Ok(missing(GET_SPELLING_SUGGESTION)),
            "build_map" => Ok(missing(COMMAND_LINE_OPTIONS_TO_MAP)),
            other => Err(format!("unknown option_name_map helper {other:?}")),
        },
        "wildcard_directories" => {
            let include=list_of(request,"include").into_iter().map(JsString::from_bytes).collect::<Vec<_>>();let exclude=list_of(request,"exclude").into_iter().map(JsString::from_bytes).collect::<Vec<_>>();
            let directories=tsr_tsoptions::wildcard_directories(&include,&exclude,text_of(request,"currentDirectory").as_bytes(),flag(request,"caseSensitive"));
            let mut rows=directories.as_ref().map_or(Vec::new(),|m|m.iter().map(|(p,r)| (text(p.as_bytes()),*r)).collect::<Vec<_>>());rows.sort_by(|a,b|a.0.cmp(&b.0));
            Ok(Outcome::Observed(json!({"directories":rows,"nil_result":directories.is_none()})))
        },
        "config_specs" => {
            let specs = config_specs(request);
            match helper {
                "file_names" => {
                    let mut options = CompilerOptions::default();
                    for (key, value) in option_entries(request)? {
                        tsr_tsoptions::parse_compiler_options(&key, &value, &mut options);
                    }
                    let host = build_host(request);
                    let (names, literal) = failed(file_names_from_specs(
                        &specs,
                        &base_of(request),
                        &options,
                        host.fs(),
                        &strings_of(request, "extra"),
                    ))?;
                    Ok(Outcome::Observed(json!({
                        "file_names": names
                            .iter()
                            .map(|name| text(name.as_bytes()))
                            .collect::<Vec<_>>(),
                        "literal_len": literal,
                    })))
                }
                "match" => {
                    let cwd = text_of(request, "currentDirectory").as_bytes().to_vec();
                    let case_sensitive = flag(request, "caseSensitive");
                    let rows: Vec<Value> = list_of(request, "query")
                        .into_iter()
                        .map(|name| {
                            json!([
                                text(&name),
                                specs.matches_exclude(&name, &cwd, case_sensitive),
                                text(specs.matched_include_spec(&name, &cwd, case_sensitive)),
                                text(specs.matched_file_spec(&name, &cwd, case_sensitive)),
                            ])
                        })
                        .collect();
                    Ok(Outcome::Observed(json!({ "matches": rows })))
                }
                "extension_priority" => Ok(missing(HAS_FILE_WITH_HIGHER_PRIORITY_EXTENSION)),
                other => Err(format!("unknown config_specs helper {other:?}")),
            }
        }
        "syntax_element" => match helper {
            "prop_array_element" => Ok(missing(PROP_ARRAY_ELEMENT)),
            "options_syntax" => Ok(missing(OPTIONS_SYNTAX_BY_ARRAY_ELEMENT_VALUE)),
            "double_quoted" => Ok(missing(IS_DOUBLE_QUOTED_STRING)),
            other => Err(format!("unknown syntax_element helper {other:?}")),
        },
        "reference_syntax" => Ok(missing(CREATE_DIAGNOSTIC_AT_REFERENCE_SYNTAX)),
        other => Err(format!("unknown configParse action {other:?}")),
    }
}

/// `entries` carries `[key, tagged value]` pairs the caller feeds to the
/// pinned option parser.
fn option_entries(request: &Value) -> Result<Vec<(Vec<u8>, ConfigValue)>, String> {
    let mut result = Vec::new();
    for entry in request
        .get("entries")
        .and_then(Value::as_array)
        .map_or(&[][..], |items| items.as_slice())
    {
        let pair = entry.as_array().ok_or("an option entry is not a pair")?;
        let key = pair
            .first()
            .and_then(Value::as_str)
            .ok_or("an option entry key is not a string")?;
        let value = decode_value(pair.get(1).ok_or("an option entry has no value")?)?;
        result.push((key.as_bytes().to_vec(), value));
    }
    Ok(result)
}

/// Assemble the spec record from the components the request supplies. The
/// pinned counterpart is an unexported struct the probe's in-package companion
/// builds the same way; nothing here validates or substitutes, because those
/// are separate pinned steps with their own cases.
fn config_specs(request: &Value) -> ConfigFileSpecs {
    let array = |key: &str| {
        ConfigValue::Array(Some(
            strings_of(request, key)
                .into_iter()
                .map(ConfigValue::String)
                .collect(),
        ))
    };
    ConfigFileSpecs {
        files_specs: array("filesBefore"),
        include_specs: array("includesBefore"),
        exclude_specs: array("validatedExcludes"),
        validated_files: strings_of(request, "validatedFiles"),
        validated_includes: strings_of(request, "validatedIncludes"),
        validated_excludes: strings_of(request, "validatedExcludes"),
        files_before_substitution: strings_of(request, "filesBefore"),
        includes_before_substitution: strings_of(request, "includesBefore"),
        is_default_include: flag(request, "isDefaultInclude"),
    }
}

#[cfg(test)]
mod tests {
    use super::{observe, Outcome};
    use serde_json::json;

    #[test]
    fn raw_references_reports_both_pinned_validation_calls() {
        // getConfigFileSpecs and getProjectReferences each validate the raw
        // property at the pin. These cases are also captured from native Go.
        for (references, required) in [(json!(42), "Array"), (json!([42]), "object")] {
            let request = json!({
                "operation": "tsoptions.configParse", "subject": "configParse",
                "action": "parse_json_api", "currentDirectory": "/project",
                "configFileName": "/project/tsconfig.json", "caseSensitive": true,
                "files": {"/project/index.ts": "export {};\n"}, "report": ["errors"],
                "jsonText": json!({"files": ["index.ts"], "references": references}).to_string()
            });
            let Some(Outcome::Observed(result)) = observe(&request) else {
                panic!("raw config must execute");
            };
            let expected = json!({
                "code": 5024, "args": ["references", required],
                "pos": -1, "end": -1, "has_file": false
            });
            assert_eq!(result["errors"], json!([expected, expected]));
        }
    }
}
