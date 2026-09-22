//! Direct native action traces for command-line parsing and parsed config.
//! Operations call production parsers, result accessors and diagnostic policies,
//! including response files and build mode.

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
        ConfigValue::StringArray(values) => strings(values.as_deref().unwrap_or_default()),
        ConfigValue::UnorderedObject(_) => {
            config_value(&tsr_tsoptions::normalize_json_value(value.clone()))
        }
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
fn host(request: &Value) -> Host {
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
    Host {
        fs: Arc::new(builder.finish()),
        cwd: JsString::from_bytes(current),
    }
}

fn config_parse(request: &Value) -> Result<ParsedCommandLine, String> {
    let host = host(request);
    let current = host.current_directory();
    let case_sensitive = host.fs().use_case_sensitive_file_names();
    let name = tsr_tspath::combine(current, &[b"tsconfig.json"]);
    let source = TsConfigSourceFile::parse(
        JsString::from_bytes(name.clone()),
        tsr_tspath::to_path(&name, current, case_sensitive),
        SourceText::from_loaded_bytes(action_str(request, "jsonText").as_bytes().to_vec()),
    );
    parse_json_source_file_config_file_content(
        source,
        &host,
        current,
        &CompilerOptions::default(),
        &ConfigValue::Null,
        &name,
    )
    .map_err(|error| format!("config parse: {error:?}"))
}

// ---------------------------------------------------------------------------
// Actions.
// ---------------------------------------------------------------------------

// Production declarations have static metadata. This short-lived probe process
// retains request-owned synthetic names for that lifetime; no production parser
// or compiler options acquire a leaking dynamic-declaration API.
pub(super) fn worker_declaration(value: &Value) -> Result<OptionDeclaration, Outcome> {
    use tsr_tsoptions::OptionKind;
    let kind = match action_str(value, "kind") {
        "string" => OptionKind::String,
        "boolean" => OptionKind::Boolean,
        "number" => OptionKind::Number,
        "object" => OptionKind::Object,
        "enum" => OptionKind::Enum,
        other => {
            return Err(Outcome::Failed(format!(
                "unsupported synthetic declaration kind {other:?}"
            )))
        }
    };
    Ok(OptionDeclaration {
        name: Box::leak(action_str(value, "name").to_owned().into_boxed_str()),
        short_name: Box::leak(action_str(value, "shortName").to_owned().into_boxed_str()),
        kind,
        is_file_path: value["isFilePath"].as_bool().unwrap_or(false),
        is_tsconfig_only: value["isTSConfigOnly"].as_bool().unwrap_or(false),
        is_command_line_only: value["isCommandLineOnly"].as_bool().unwrap_or(false),
        enum_values: &[],
        deprecated_keys: &[],
        element: None,
        extra_validation: "",
        min_value: 0,
        allow_config_dir_template: false,
        preserve_falsy: false,
    })
}

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

        "parse_command_line" | "parse_build_command_line" => {
            let args: Vec<_> = action
                .get("args")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|value| {
                    value
                        .as_str()
                        .map(|value| JsString::from_bytes(value.as_bytes()))
                        .ok_or_else(|| Outcome::Failed("argv member is not a string".into()))
                })
                .collect::<Result<_, _>>()?;
            let host = host(request);
            row.insert("args".into(), strings(&args));
            let (options, raw, errors) = if action_op(action) == "parse_command_line" {
                let parsed = tsr_tsoptions::parse_command_line(&args, &host);
                row.insert("file_names".into(), strings(&parsed.root_file_names));
                row.insert("current_directory".into(), text(parsed.current_directory()));
                row.insert(
                    "use_case_sensitive_file_names".into(),
                    json!(parsed.use_case_sensitive_file_names()),
                );
                (parsed.options, parsed.raw, parsed.errors)
            } else {
                let parsed = tsr_tsoptions::parse_build_command_line(&args, &host);
                row.insert("projects".into(), strings(&parsed.projects));
                row.insert(
                    "resolvedProjects".into(),
                    strings(parsed.resolved_project_paths()),
                );
                row.insert(
                    "locale_is_default".into(),
                    json!(parsed.locale().is_default()),
                );
                (parsed.compiler_options, parsed.raw, parsed.errors)
            };
            row.insert("errors".into(), diagnostic_rows!(errors));
            row.insert("raw".into(), config_value(&raw));
            let values = tsr_tsoptions::compiler_options_value(&options);
            let selected: Vec<_> = action
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|name| {
                    let name = name
                        .as_str()
                        .ok_or_else(|| Outcome::Failed("option selector is not a string".into()))?;
                    let value = values.get(name.as_bytes());
                    Ok(json!([name, value.map(config_value), value.is_some()]))
                })
                .collect::<Result<_, Outcome>>()?;
            row.insert("compiler_options".into(), json!(selected));
        }
        "input_option_name" => {
            let input = action_str(action, "value");
            row.insert("input".into(), json!(input));
            row.insert(
                "option_name".into(),
                text(tsr_tsoptions::input_option_name(input.as_bytes())),
            );
        }
        "invalid_enum_type_diagnostic" => {
            let option = declaration(action).map_err(Outcome::Failed)?;
            let diagnostic = tsr_tsoptions::invalid_enum_type_diagnostic(
                option,
                tsr_tsoptions::OptionSyntax::default(),
            );
            row.insert("name".into(), json!(option.name));
            row.insert(
                "diagnostic".into(),
                diagnostic_rows!([diagnostic])[0].clone(),
            );
        }
        "extra_key_diagnostics" => {
            let parent = action_str(action, "value");
            let messages = tsr_tsoptions::extra_key_diagnostics(parent.as_bytes());
            row.insert("parent".into(), json!(parent));
            row.insert(
                "unknown_code".into(),
                messages.map_or(Value::Null, |messages| json!(messages.0.code)),
            );
            row.insert(
                "did_you_mean_code".into(),
                messages.map_or(Value::Null, |messages| json!(messages.1.code)),
            );
        }
        "worker_diagnostics" => {
            let declarations = action
                .get("declarations")
                .and_then(Value::as_array)
                .ok_or_else(|| Outcome::Failed("missing declarations".into()))?
                .iter()
                .map(worker_declaration)
                .collect::<Result<Vec<_>, _>>()?;
            let policy = tsr_tsoptions::parse_command_line_worker_diagnostics(&declarations);
            row.insert(
                "option_type_mismatch_code".into(),
                json!(policy.mismatch.code),
            );
            row.insert("unknown_option_code".into(), json!(policy.unknown.code));
            row.insert(
                "unknown_did_you_mean_code".into(),
                json!(policy.did_you_mean.code),
            );
            row.insert(
                "alternate_mode_code".into(),
                policy
                    .alternate
                    .map_or(Value::Null, |alternate| json!(alternate.diagnostic.code)),
            );
            row.insert(
                "alternate_mode_has_name_map".into(),
                json!(policy.alternate.is_some()),
            );
            row.insert("declaration_count".into(), json!(policy.declarations.len()));
        }
        "canonical_key" => {
            let input = action_str(action, "value");
            row.insert("input".into(), json!(input));
            row.insert(
                "key".into(),
                text(
                    tsr_tsoptions::canonical_key(
                        input.as_bytes(),
                        request["caseSensitive"].as_bool().unwrap_or(false),
                    )
                    .as_bytes(),
                ),
            );
        }
        "wildcard_directory_from_spec" => {
            let spec = action_str(action, "value");
            let found = tsr_tsoptions::wildcard_directory_from_spec(
                spec.as_bytes(),
                request["caseSensitive"].as_bool().unwrap_or(false),
            );
            row.insert("spec".into(), json!(spec));
            row.insert("matched".into(), json!(found.is_some()));
            row.insert(
                "key".into(),
                text(found.as_ref().map_or(b"", |v| v.key.as_bytes())),
            );
            row.insert(
                "path".into(),
                text(found.as_ref().map_or(b"", |v| v.path.as_bytes())),
            );
            row.insert(
                "recursive".into(),
                json!(found.is_some_and(|v| v.recursive)),
            );
        }
        "wildcard_directories" => {
            let list = |key: &str| {
                action[key]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|v| JsString::from_bytes(v.as_str().expect("spec string").as_bytes()))
                    .collect::<Vec<_>>()
            };
            let include = list("include");
            let exclude = list("exclude");
            let directories = tsr_tsoptions::wildcard_directories(
                &include,
                &exclude,
                action_str(request, "currentDirectory").as_bytes(),
                request["caseSensitive"].as_bool().unwrap_or(false),
            );
            row.insert("include".into(), strings(&include));
            row.insert("exclude".into(), strings(&exclude));
            row.insert("is_nil".into(), json!(directories.is_none()));
            row.insert("directories".into(), directory_rows(directories.as_ref()));
        }

        other => {
            return Err(Outcome::Failed(format!(
                "unknown command-line action {other:?}"
            )))
        }
    }
    Ok(Value::Object(row))
}

fn directory_rows(
    directories: Option<&tsr_core::collections::OrderedMap<JsString, bool>>,
) -> Value {
    let mut entries = directories
        .into_iter()
        .flat_map(|m| m.iter())
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    json!(entries
        .into_iter()
        .map(|(p, r)| json!([text(p.as_bytes()), r]))
        .collect::<Vec<_>>())
}

/// Drive one `ParsedCommandLine` accessor, or report it missing.
fn config_probe(
    request: &Value,
    action: &Value,
    parsed: &mut Option<ParsedCommandLine>,
) -> Result<Value, Outcome> {
    let requested = action_str(action, "probe");
    let (probe, argument) = requested.split_once(':').unwrap_or((requested, ""));
    if parsed.is_none() {
        *parsed = Some(config_parse(request).map_err(Outcome::Failed)?);
    }
    let parsed = parsed.as_mut().expect("config parse");
    let mut row = Map::new();
    row.insert("op".into(), Value::String("parsed_config".into()));
    row.insert("probe".into(), Value::String(requested.to_owned()));
    match probe {
        "common_source_directory" => {
            let before = parsed.errors.len();
            row.insert(
                "common_source_directory".into(),
                text(parsed.common_source_directory()),
            );
            row.insert("errors_added".into(), json!(parsed.errors.len() - before));
            row.insert("errors".into(), diagnostic_rows!(&parsed.errors[before..]));
        }
        "build_info_file_name" => {
            row.insert(
                "build_info_file_name".into(),
                text(parsed.build_info_file_name().as_bytes()),
            );
        }
        "input_output_names" => {
            parsed.parse_input_output_names();
            row.insert(
                "source_to_project_reference".into(),
                json!(parsed
                    .source_to_project_reference()
                    .map(|(key, entry)| json!([
                        text(key.as_bytes()),
                        text(entry.names.source.as_bytes()),
                        text(entry.names.output_dts.as_bytes())
                    ]))
                    .collect::<Vec<_>>()),
            );
            row.insert(
                "output_dts_to_project_reference".into(),
                json!(parsed
                    .output_dts_to_project_reference()
                    .map(|(key, entry)| json!([
                        text(key.as_bytes()),
                        text(entry.names.source.as_bytes()),
                        text(entry.names.output_dts.as_bytes())
                    ]))
                    .collect::<Vec<_>>()),
            );
        }
        "file_names_by_path" => {
            row.insert(
                "file_names_by_path".into(),
                json!(parsed
                    .file_names_by_path()
                    .iter()
                    .map(|(key, value)| [text(key.as_bytes()), text(value.as_bytes())])
                    .collect::<Vec<_>>()),
            );
        }
        "wildcard_directory_globs" => {
            let mut patterns = parsed
                .wildcard_directory_globs()
                .unwrap_or_default()
                .iter()
                .map(tsr_glob::Glob::to_bytes)
                .collect::<Vec<_>>();
            patterns.sort();
            row.insert("glob_count".into(), json!(patterns.len()));
            row.insert(
                "patterns".into(),
                json!(patterns
                    .iter()
                    .map(|pattern| text(pattern))
                    .collect::<Vec<_>>()),
            );
        }
        "extended_source_files" => {
            row.insert(
                "extended_source_files".into(),
                strings(parsed.extended_source_files()),
            );
        }
        "project_references" => {
            row.insert(
                "project_references".into(),
                json!(parsed
                    .project_references
                    .iter()
                    .flatten()
                    .map(|reference| json!([
                        text(reference.path.as_bytes()),
                        text(reference.original_path.as_bytes()),
                        reference.circular
                    ]))
                    .collect::<Vec<_>>()),
            );
            row.insert(
                "resolved_paths".into(),
                strings(parsed.resolved_project_reference_paths()),
            );
        }
        "content_mappers" => {
            row.insert(
                "mapper_count".into(),
                json!(parsed.content_mappers.as_ref().map_or(0, Vec::len)),
            );
            row.insert(
                "extensions".into(),
                strings(&parsed.content_mapper_extensions()),
            );
            row.insert("mapper_for_file".into(), json!(argument));
            row.insert(
                "has_mapper_for_file".into(),
                json!(
                    !argument.is_empty()
                        && parsed
                            .content_mapper_for_file_name(argument.as_bytes())
                            .is_some()
                ),
            );
        }
        "type_acquisition" => {
            parsed.set_type_acquisition(Some(tsr_tsoptions::TypeAcquisition {
                enable: tsr_core::Tristate::TRUE,
                ..Default::default()
            }));
            row.insert(
                "set_then_read_enable".into(),
                json!(parsed
                    .type_acquisition
                    .as_ref()
                    .is_some_and(|value| value.enable.is_true())),
            );
        }
        "set_compiler_options" => {
            parsed.set_compiler_options(CompilerOptions {
                locale: JsString::from_bytes(argument.as_bytes()),
                ..Default::default()
            });
            row.insert("locale_input".into(), json!(argument));
            row.insert(
                "locale_is_default".into(),
                json!(parsed.locale().is_default()),
            );
            row.insert(
                "file_names_survived".into(),
                json!(parsed.root_file_names.len()),
            );
        }
        "set_parsed_options" => {
            parsed.set_parsed_options(tsr_tsoptions::ParsedOptions {
                file_names: vec![JsString::from_bytes(argument.as_bytes())],
                ..Default::default()
            });
            row.insert("file_names".into(), strings(&parsed.root_file_names));
        }
        "current_directory" => {
            row.insert("current_directory".into(), text(parsed.current_directory()));
            row.insert(
                "use_case_sensitive_file_names".into(),
                json!(parsed.use_case_sensitive_file_names()),
            );
        }
        "wildcard_directories" => {
            let directories = parsed.wildcard_directories();
            row.insert("is_nil".into(), json!(directories.is_none()));
            row.insert("directories".into(), directory_rows(directories));
        }
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
