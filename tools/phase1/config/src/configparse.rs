//! The config-parsing group: `internal/tsoptions/tsconfigparsing.go`,
//! `parsinghelpers.go` and `wildcarddirectories.go`.
//!
//! Both source-file and raw-JSON entry points, including wildcard directory
//! calculation and shared extended-config caching, execute the production port.
//! This driver only assembles hosts and translates the declared probe values.

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
pub(super) struct Host {
    pub(super) fs: Arc<dyn FileSystem>,
    pub(super) cwd: JsString,
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
        "strings" => Ok(ConfigValue::StringArray(Some(
            payload
                .and_then(Value::as_array)
                .ok_or("strings payload")?
                .iter()
                .map(|s| {
                    s.as_str()
                        .map(|s| JsString::from_bytes(s.as_bytes()))
                        .ok_or_else(|| "string element".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?,
        ))),
        "map" => {
            let mut entries = std::collections::HashMap::new();
            for pair in payload.and_then(Value::as_array).ok_or("map payload")? {
                entries.insert(
                    JsString::from_bytes(pair[0].as_str().ok_or("map key")?.as_bytes()),
                    decode_value(&pair[1])?,
                );
            }
            Ok(ConfigValue::UnorderedObject(entries))
        }
        other => Err(format!("unknown config value tag: {other:?}")),
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
        ConfigValue::StringArray(values) => render_strings(values.as_ref()),
        ConfigValue::UnorderedObject(_) => {
            panic!("unordered values must be normalized before rendering")
        }
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
    cache: Option<&tsr_tsoptions::ExtendedConfigCache<'_>>,
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
    if let Some(cache) = cache {
        return cache.parse_source_file(
            source,
            &base_of(request),
            &CompilerOptions::default(),
            &ConfigValue::Null,
            name,
        );
    }
    parse_json_source_file_config_file_content(
        source,
        host,
        &base_of(request),
        &CompilerOptions::default(),
        &ConfigValue::Null,
        name,
    )
}

pub fn observe(request: &Value) -> Option<Outcome> {
    if subject(request) != "configParse" {
        return None;
    }
    Some(answer(request).unwrap_or_else(Outcome::Failed))
}

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
            let host = build_host(request);
            let cache =
                flag(request, "useCache").then(|| tsr_tsoptions::ExtendedConfigCache::new(&host));
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
                    cache.as_ref(),
                ))?;
            }
            let source_text = text_of(request, "jsonText").as_bytes().to_vec();
            let mut parsed = failed(parse_source(
                request,
                &host,
                &name,
                &source_text,
                cache.as_ref(),
            ))?;
            if flag(request, "parseTwice") {
                parsed = failed(parse_source(
                    request,
                    &host,
                    &name,
                    &source_text,
                    cache.as_ref(),
                ))?;
            }
            Ok(Outcome::Observed(Value::Object(describe_parsed(
                request, &parsed,
            )?)))
        }
        "read_config_file" => {
            let host = build_host(request);
            let cache =
                flag(request, "useCache").then(|| tsr_tsoptions::ExtendedConfigCache::new(&host));
            let read = |name: &[u8]| match &cache {
                Some(cache) => {
                    cache.read_config_file(name, &CompilerOptions::default(), &ConfigValue::Null)
                }
                None => get_parsed_command_line_of_config_file(
                    name,
                    &CompilerOptions::default(),
                    &ConfigValue::Null,
                    &host,
                ),
            };
            let also = text_of(request, "parseAlso");
            if !also.is_empty() {
                failed(read(also.as_bytes()))?;
            }
            let result = failed(read(text_of(request, "configFileName").as_bytes()))?;
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
            let host = build_host(request);
            let name = text_of(request, "configFileName").as_bytes();
            let raw = if action == "parse_json_api_value" {
                decode_value(&request["value"])?
            } else {
                let path =
                    tsr_tspath::to_path(name, &base_of(request), flag(request, "caseSensitive"));
                parse_config_file_text_to_json(
                    JsString::from_bytes(name),
                    path,
                    SourceText::from_loaded_bytes(text_of(request, "jsonText").as_bytes().to_vec()),
                )
                .value
            };
            let parsed = failed(tsr_tsoptions::parse_json_config_file_content(
                raw,
                &host,
                &base_of(request),
                &CompilerOptions::default(),
                name,
                &[],
            ))?;
            Ok(Outcome::Observed(Value::Object(describe_parsed(
                request, &parsed,
            )?)))
        }
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
        "convert_to_object" => {
            let source = source_from_request(request);
            let (raw, errors) = tsr_tsoptions::convert_to_object(&source);
            Ok(Outcome::Observed(
                json!({"value":render_value(&raw),"errors":render_diagnostics(&errors)}),
            ))
        }
        "extended_config" => {
            let host = build_host(request);
            let name = text_of(request, "configFileName").as_bytes();
            let path = tsr_tspath::to_path(
                name,
                host.current_directory(),
                host.fs().use_case_sensitive_file_names(),
            );
            let entry = failed(tsr_tsoptions::parse_extended_config(name, path, &[], &host))?;
            Ok(Outcome::Observed(
                json!({"has_entry":true,"extended_file_names":entry.extended_file_names().iter().map(|s|text(s.as_bytes())).collect::<Vec<_>>()}),
            ))
        }
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
            "array" => {
                let values = tsr_tsoptions::substituted_strings(
                    &strings_of(request, "specs"),
                    &base_of(request),
                );
                let rendered = values.as_ref().map_or_else(
                    || json!(["nilarray"]),
                    |values| {
                        json!([
                            "strings",
                            values
                                .iter()
                                .map(|value| text(value.as_bytes()))
                                .collect::<Vec<_>>()
                        ])
                    },
                );
                Ok(Outcome::Observed(
                    json!({"substituted":values.is_some(),"array":rendered}),
                ))
            }
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
        "parser_diagnostics" | "option_parser_parse_option" => {
            let kind = text_of(request, "parserKind");
            let mut parser: Box<dyn tsr_tsoptions::OptionParser> = match kind {
                "compiler" => Box::new(CompilerOptions::default()),
                "watch" => Box::new(tsr_core::WatchOptions::default()),
                "build" => Box::new(tsr_core::BuildOptions::default()),
                "typeAcquisition" => Box::new(tsr_tsoptions::TypeAcquisition::default()),
                _ => return Err(format!("no pinned parser named {kind:?}")),
            };
            if action == "parser_diagnostics" {
                Ok(Outcome::Observed(
                    json!({"kind":kind,"unknown_option_code":parser.unknown_option().code,"unknown_did_you_mean_code":parser.unknown_did_you_mean().code}),
                ))
            } else {
                let mut codes = Vec::new();
                for (key, value) in parser_entries(request)? {
                    codes.extend(
                        parser
                            .parse_option(key.as_bytes(), &value)
                            .iter()
                            .map(|d| d.code),
                    );
                }
                Ok(Outcome::Observed(
                    json!({"kind":kind,"diagnostic_codes":codes}),
                ))
            }
        }
        "convert_map_to_options" => {
            let mut options = CompilerOptions::default();
            tsr_tsoptions::convert_map_to_options(&parser_entries(request)?, &mut options);
            Ok(Outcome::Observed(
                json!({"options":render_options(&options,&list_of(request,"optionNames"))?}),
            ))
        }
        "content_mapper_diagnostic_location" => {
            let host = build_host(request);
            let name = tsr_tspath::combine(
                &base_of(request),
                &[text_of(request, "configFileName").as_bytes()],
            );
            let parsed = failed(parse_source(
                request,
                &host,
                &name,
                text_of(request, "jsonText").as_bytes(),
                None,
            ))?;
            let mappers = parsed.content_mappers.as_deref().unwrap_or_default();
            let path = request["optionPath"]
                .as_array()
                .ok_or("optionPath")?
                .iter()
                .map(|segment| {
                    if flag(segment, "isIndex") {
                        config_mappers::OptionPathSegment::Index(
                            segment["index"].as_i64().unwrap_or_default() as isize,
                        )
                    } else {
                        config_mappers::OptionPathSegment::Property(JsString::from_bytes(
                            text_of(segment, "name").as_bytes(),
                        ))
                    }
                })
                .collect::<Vec<_>>();
            let location = mappers.first().and_then(|mapper| {
                config_mappers::option_diagnostic_location(&parsed, mapper, &path)
            });
            Ok(Outcome::Observed(
                json!({"mappers":mappers.len(),"has_file":location.is_some(),"pos":location.map_or(-1,|(_,loc)|loc.pos()),"end":location.map_or(-1,|(_,loc)|loc.end())}),
            ))
        }
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
                "project_reference" => {
                    let result = tsr_tsoptions::parse_project_reference(&value);
                    Ok(Outcome::Observed(
                        json!({"present":result.is_some(),"path":result.as_ref().map_or_else(String::new,|r|text(r.reference.path.as_bytes())),"circular":result.as_ref().is_some_and(|r|r.reference.circular),"has_path":result.as_ref().is_some_and(|r|r.has_path),"path_valid":result.as_ref().is_some_and(|r|r.path_valid),"has_circular":result.as_ref().is_some_and(|r|r.has_circular),"circular_valid":result.as_ref().is_some_and(|r|r.circular_valid)}),
                    ))
                }
                "is_string_value" => Ok(Outcome::Observed(
                    json!({"result": value.as_string().is_some()}),
                )),
                "normalize_json_value" => Ok(Outcome::Observed(
                    json!({"result": render_value(&tsr_tsoptions::normalize_json_value(value))}),
                )),
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
                let mut types = tsr_tsoptions::TypeAcquisition::for_config(
                    text_of(request, "configFileName").as_bytes(),
                );
                types.parse_option(text_of(request, "key").as_bytes(), &value);
                Ok(Outcome::Observed(json!({
                    "type_acquisition": render_type_acquisition(Some(&types)),
                    "errors": Value::Array(vec![]),
                })))
            }
            "watch" => {
                let value = decode_value(request.get("value").ok_or("request has no value")?)?;
                let mut options = tsr_core::WatchOptions::default();
                tsr_tsoptions::parse_watch_options(
                    text_of(request, "key").as_bytes(),
                    &value,
                    &mut options,
                );
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
                tsr_tsoptions::parse_build_options(
                    text_of(request, "key").as_bytes(),
                    &value,
                    &mut options,
                );
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
            let mut result = Map::new();
            let name = text_of(request, "configFileName").as_bytes();
            if wants(request, "options") {
                result.insert(
                    "options".into(),
                    render_options(
                        &tsr_tsoptions::default_compiler_options(name),
                        &list_of(request, "optionNames"),
                    )?,
                );
            }
            if wants(request, "type_acquisition") {
                result.insert(
                    "type_acquisition".into(),
                    render_type_acquisition(Some(&tsr_tsoptions::TypeAcquisition::for_config(
                        name,
                    ))),
                );
            }
            Ok(Outcome::Observed(Value::Object(result)))
        }
        "option_absolute_path" => match helper {
            "one" => {
                let value = decode_value(&request["value"])?;
                let converted = tsr_tsoptions::convert_option_to_absolute_path(
                    text_of(request, "name").as_bytes(),
                    &value,
                    tsr_tsoptions::compiler_option_name_map(),
                    text_of(request, "currentDirectory").as_bytes(),
                );
                Ok(Outcome::Observed(
                    json!({"converted":converted.is_some(),"result":render_value(converted.as_ref().unwrap_or(&ConfigValue::Null))}),
                ))
            }
            "all" => {
                let mut values = option_entries(request)?
                    .into_iter()
                    .map(|(key, value)| (JsString::from_bytes(key), value))
                    .collect();
                tsr_tsoptions::convert_options_with_absolute_paths(
                    &mut values,
                    tsr_tsoptions::compiler_option_name_map(),
                    text_of(request, "currentDirectory").as_bytes(),
                );
                Ok(Outcome::Observed(
                    json!({"result":render_value(&ConfigValue::Object(values))}),
                ))
            }
            other => Err(format!("unknown option_absolute_path helper {other:?}")),
        },
        "option_name_map" => {
            let map = tsr_tsoptions::compiler_option_name_map();
            match helper {
                "get" | "spelling" => {
                    let rows = list_of(request, "query")
                        .iter()
                        .map(|name| {
                            let found = if helper == "get" {
                                map.get(name)
                            } else {
                                map.spelling_suggestion(name)
                            };
                            json!([text(name), found.map_or("", |option| option.name)])
                        })
                        .collect::<Vec<_>>();
                    let key = if helper == "get" {
                        "resolved"
                    } else {
                        "suggested"
                    };
                    Ok(Outcome::Observed(json!({key:rows})))
                }
                "build_map" => {
                    let declarations = list_of(request, "query")
                        .iter()
                        .map(|name| {
                            crate::commandlineops::worker_declaration(
                                &json!({"name":text(name),"kind":"string"}),
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| "bad name-map declaration".to_owned())?;
                    let map = tsr_tsoptions::CommandLineOptionNameMap::new(&declarations);
                    Ok(Outcome::Observed(
                        json!({"keys":map.keys().map(text).collect::<Vec<_>>()}),
                    ))
                }
                other => Err(format!("unknown option_name_map helper {other:?}")),
            }
        }
        "wildcard_directories" => {
            let include = list_of(request, "include")
                .into_iter()
                .map(JsString::from_bytes)
                .collect::<Vec<_>>();
            let exclude = list_of(request, "exclude")
                .into_iter()
                .map(JsString::from_bytes)
                .collect::<Vec<_>>();
            let directories = tsr_tsoptions::wildcard_directories(
                &include,
                &exclude,
                text_of(request, "currentDirectory").as_bytes(),
                flag(request, "caseSensitive"),
            );
            let mut rows = directories.as_ref().map_or(Vec::new(), |m| {
                m.iter()
                    .map(|(p, r)| (text(p.as_bytes()), *r))
                    .collect::<Vec<_>>()
            });
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(Outcome::Observed(
                json!({"directories":rows,"nil_result":directories.is_none()}),
            ))
        }
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
                "extension_priority" => {
                    let groups = request["extensions"]
                        .as_array()
                        .ok_or("extension groups")?
                        .iter()
                        .map(|row| {
                            row.as_array()
                                .ok_or("extension group")?
                                .iter()
                                .map(|ext| {
                                    ext.as_str()
                                        .map(|s| JsString::from_bytes(s.as_bytes()))
                                        .ok_or("extension")
                                })
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let mut present: tsr_core::collections::OrderedMap<_, _> =
                        strings_of(request, "present")
                            .into_iter()
                            .map(|s| (s.clone(), s))
                            .collect();
                    let spec = text_of(request, "spec").as_bytes();
                    let higher = tsr_tsoptions::has_file_with_higher_priority_extension(
                        spec,
                        &groups,
                        |key| present.get(key).is_some(),
                    );
                    tsr_tsoptions::remove_wildcard_files_with_lower_priority_extension(
                        spec,
                        &mut present,
                        &groups,
                        true,
                    );
                    Ok(Outcome::Observed(
                        json!({"higher_priority":higher,"survivors":render_strings(Some(&present.into_iter().map(|(_,v)|v).collect()))}),
                    ))
                }
                other => Err(format!("unknown config_specs helper {other:?}")),
            }
        }
        "syntax_element" => {
            let name = text_of(request, "configFileName").as_bytes();
            let config = TsConfigSourceFile::parse(
                JsString::from_bytes(name),
                tsr_tspath::to_path(name, &base_of(request), flag(request, "caseSensitive")),
                SourceText::from_loaded_bytes(text_of(request, "jsonText").as_bytes().to_vec()),
            );
            let key = text_of(request, "key").as_bytes();
            let value = text_of(request, "spec").as_bytes();
            let describe = |node: Option<tsr_ast::NodeId>| {
                node.map_or(Value::Null, |node| {
                    let read = config.file.view().node(node).expect("config node");
                    json!([read.kind().raw(), read.pos(), read.end()])
                })
            };
            match helper {
                "prop_array_element" | "double_quoted" => {
                    let node = tsr_tsoptions::config_prop_array_element_value(&config, key, value);
                    if helper == "double_quoted" {
                        Ok(Outcome::Observed(node.map_or_else(||json!({"found":false}),|node|json!({"found":true,"double_quoted":tsr_tsoptions::is_double_quoted_string(&config,node)}))))
                    } else {
                        Ok(Outcome::Observed(node.map_or_else(||json!({"node":null}),|node|json!({"node":describe(Some(node)),"text":text(config.file.view().node_text(node).expect("config text").as_bytes())}))))
                    }
                }
                "options_syntax" => {
                    let object = config.object();
                    let node = object.and_then(|object| {
                        tsr_tsoptions::options_syntax_by_array_element_value(
                            &config, object, key, value,
                        )
                    });
                    Ok(Outcome::Observed(
                        json!({"node":describe(node),"has_object":object.is_some()}),
                    ))
                }
                other => Err(format!("unknown syntax_element helper {other:?}")),
            }
        }
        "reference_syntax" => {
            let host = build_host(request);
            let parsed = failed(parse_source(
                request,
                &host,
                text_of(request, "configFileName").as_bytes(),
                text_of(request, "jsonText").as_bytes(),
                None,
            ))?;
            let index = request["index"].as_i64().ok_or("missing index")?;
            let diagnostic = tsr_tsoptions::diagnostic_at_reference_syntax(
                &parsed,
                isize::try_from(index).map_err(|_| "invalid index")?,
                tsr_diagnostics::Compiler_option_0_cannot_be_given_an_empty_string,
                vec![JsString::from_bytes(b"reference.path".as_slice())],
            );
            Ok(Outcome::Observed(diagnostic.as_ref().map_or_else(
                || json!({"reported":false}),
                |diagnostic| json!({"reported":true,"diagnostic":render_diagnostic(diagnostic)}),
            )))
        }
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

fn parser_entries(
    request: &Value,
) -> Result<tsr_core::collections::OrderedMap<JsString, ConfigValue>, String> {
    request["parserEntries"]
        .as_array()
        .ok_or("parserEntries must be an array")?
        .iter()
        .map(|entry| {
            Ok((
                JsString::from_bytes(entry["key"].as_str().ok_or("parser key")?.as_bytes()),
                decode_value(&entry["value"])?,
            ))
        })
        .collect()
}

fn source_from_request(request: &Value) -> TsConfigSourceFile {
    let name = text_of(request, "configFileName").as_bytes();
    TsConfigSourceFile::parse(
        JsString::from_bytes(name),
        tsr_tspath::to_path(name, &base_of(request), flag(request, "caseSensitive")),
        SourceText::from_loaded_bytes(text_of(request, "jsonText").as_bytes().to_vec()),
    )
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
