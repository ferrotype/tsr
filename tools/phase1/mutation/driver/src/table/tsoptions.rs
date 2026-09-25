//! Group `tsoptions`: tsoptions ports (and `computeFn` through the bridge in
//! `tools/phase1/tables/go/bridges/tsoptions`).
//! Go: `tools/phase1/tables/go/tsoptions_columns.go`; spec:
//! `data/phase1/tables/tsoptions.json`.
use super::{config_host, decode, parse_config, Column};
use crate::protocol::hex;
use serde_json::{json, Value};
use tsr_jsstring::JsString;

pub const COLUMNS: &[&str] = &[
    "tsoptions.ParsedCommandLine.PossiblyMatchesDirectoryName",
    "tsoptions.ParsedCommandLine.LiteralFileNames",
    "tsoptions.ParsedCommandLine.WithFileNames",
    "tsoptions.ParsedCommandLine.GetOutputFileNames",
    "tsoptions.ParsedCommandLine.PossiblyMatchesFileName",
    "tsoptions.ParsedCommandLine.WildcardDirectoryGlobs",
    "tsoptions.ParsedBuildCommandLine.ResolvedProjectPaths",
    "tsoptions.TargetToLibMap",
    "tsoptions.CompilerOptionsAffectEmit",
    "tsoptions.CompilerOptionsAffectDeclarationPath",
    "tsoptions.CompilerOptionsAffectSemanticDiagnostics",
    "tsoptions.ForEachCompilerOptionValue",
    "tsoptions.ConvertToTSConfig",
    "tsoptions.computeFn",
    "tsoptions.ExtendedConfigCacheEntry.ExtendedFileNames",
    "tsoptions.ParsedCommandLine.ReloadFileNamesOfParsedCommandLine",
    "core.ResolveProjectReferencePath",
];

/// Go's `hexes`.
fn hexes(texts: Option<&[JsString]>) -> Value {
    texts.map_or(Value::Null, |texts| {
        json!(texts
            .iter()
            .map(|text| hex(text.as_bytes()))
            .collect::<Vec<_>>())
    })
}

/// Go's `configColumn`.
fn config_column<A: serde::de::DeserializeOwned + 'static>(
    input: &Value,
    f: fn(&mut tsr_tsoptions::ParsedCommandLine, &A) -> Result<Value, String>,
) -> Result<Column, String> {
    let (mut parsed, args) = parse_config(input)?;
    let args: A = decode(&args)?;
    Ok(Box::new(move || f(&mut parsed, &args)))
}

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "tsoptions.ParsedCommandLine.PossiblyMatchesDirectoryName" => {
            possibly_matches_directory_name(input)
        }
        "tsoptions.ParsedCommandLine.LiteralFileNames" => {
            config_column::<Value>(input, |parsed, _| Ok(hexes(parsed.literal_file_names())))
        }
        "tsoptions.ParsedCommandLine.WithFileNames" => {
            config_column::<Option<Vec<String>>>(input, |parsed, args| {
                let names = args.as_ref().map(|names| {
                    names
                        .iter()
                        .map(|name| JsString::from_bytes(name.as_bytes()))
                        .collect()
                });
                let copied = parsed.with_file_names(names);
                Ok(json!([
                    hexes(Some(&copied.root_file_names)),
                    hexes(copied.literal_file_names()),
                    hexes(Some(&parsed.root_file_names))
                ]))
            })
        }
        "tsoptions.ParsedCommandLine.GetOutputFileNames" => {
            config_column::<Value>(input, |parsed, _| {
                Ok(hexes(Some(&parsed.output_file_names())))
            })
        }
        "tsoptions.ParsedCommandLine.PossiblyMatchesFileName" => {
            config_column::<Vec<String>>(input, |parsed, args| {
                Ok(json!(args
                    .iter()
                    .map(|name| parsed.possibly_matches_file_name(name.as_bytes()))
                    .collect::<Vec<_>>()))
            })
        }
        "tsoptions.ParsedCommandLine.WildcardDirectoryGlobs" => {
            config_column::<Vec<String>>(input, |parsed, args| {
                let globs = parsed.wildcard_directory_globs().unwrap_or_default();
                let mut out: Vec<Value> = args
                    .iter()
                    .map(|name| json!(globs.iter().any(|glob| glob.matches(name.as_bytes()))))
                    .collect();
                out.push(json!(globs.len()));
                Ok(Value::Array(out))
            })
        }
        "tsoptions.ParsedBuildCommandLine.ResolvedProjectPaths" => resolved_project_paths(input),
        "tsoptions.TargetToLibMap" => {
            let _: Value = match decode(input) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            Ok(Box::new(|| {
                let mut entries: Vec<(&str, i32)> = tsr_tsoptions::enum_maps::target_to_lib_map()
                    .iter()
                    .map(|(target, lib)| (*lib, target.0))
                    .collect();
                entries.sort_unstable();
                Ok(json!(entries
                    .into_iter()
                    .map(|(lib, target)| json!([hex(lib.as_bytes()), target]))
                    .collect::<Vec<_>>()))
            }))
        }
        "core.ResolveProjectReferencePath" => resolve_project_reference_path(input),
        "tsoptions.CompilerOptionsAffectEmit" => {
            affect_column(input, tsr_tsoptions::affects::compiler_options_affect_emit)
        }
        "tsoptions.CompilerOptionsAffectDeclarationPath" => affect_column(
            input,
            tsr_tsoptions::affects::compiler_options_affect_declaration_path,
        ),
        "tsoptions.CompilerOptionsAffectSemanticDiagnostics" => affect_column(
            input,
            tsr_tsoptions::affects::compiler_options_affect_semantic_diagnostics,
        ),
        "tsoptions.ForEachCompilerOptionValue" => for_each_compiler_option_value(input),
        "tsoptions.ConvertToTSConfig" => config_column::<String>(input, |parsed, name| {
            let config = tsr_tsoptions::show_config::convert_to_ts_config(parsed, name.as_bytes());
            let options: Vec<Value> = config
                .compiler_options
                .iter()
                .map(|(key, value)| json!([hex(key.as_bytes()), show_value(value)]))
                .collect();
            let references = config
                .references
                .as_ref()
                .map_or(Value::Null, |references| {
                    json!(references
                        .iter()
                        .map(|(path, circular)| json!([hex(path.as_bytes()), circular]))
                        .collect::<Vec<_>>())
                });
            Ok(json!([
                options,
                references,
                hexes(config.files.as_deref()),
                hexes(config.include.as_deref()),
                hexes(config.exclude.as_deref()),
                config.compile_on_save
            ]))
        }),
        "tsoptions.computeFn" => compute_fn_column(input),
        "tsoptions.ExtendedConfigCacheEntry.ExtendedFileNames" => extended_file_names(input),
        "tsoptions.ParsedCommandLine.ReloadFileNamesOfParsedCommandLine" => {
            reload_file_names(input)
        }
        _ => return None,
    })
}

/// One bool per directory path of `args`; the wildcard directories are
/// computed in setup.
fn possibly_matches_directory_name(input: &Value) -> Result<Column, String> {
    let (parsed, args) = parse_config(input)?;
    let paths: Vec<String> = decode(&args)?;
    let _ = parsed.wildcard_directories();
    Ok(Box::new(move || {
        Ok(Value::Array(
            paths
                .iter()
                .map(|path| {
                    json!(
                        parsed.possibly_matches_directory_name(&tsr_tspath::Path::from_bytes(
                            path.as_bytes().to_vec()
                        ))
                    )
                })
                .collect(),
        ))
    }))
}

/// ParseBuildCommandLine over the config input's files and args.
fn resolved_project_paths(input: &Value) -> Result<Column, String> {
    let (host, args) = config_host(input)?;
    let args: Vec<String> = decode(&args)?;
    let args: Vec<JsString> = args
        .iter()
        .map(|arg| JsString::from_bytes(arg.as_bytes()))
        .collect();
    let parsed = tsr_tsoptions::parse_build_command_line(&args, &host);
    Ok(Box::new(move || {
        Ok(hexes(Some(parsed.resolved_project_paths())))
    }))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Paths {
    paths: Vec<String>,
}

fn resolve_project_reference_path(input: &Value) -> Result<Column, String> {
    let paths: Paths = decode(input)?;
    Ok(Box::new(move || {
        Ok(json!(paths
            .paths
            .iter()
            .map(|path| {
                let reference = tsr_tsoptions::ProjectReference {
                    path: JsString::from_bytes(path.as_bytes()),
                    original_path: JsString::default(),
                    circular: false,
                };
                json!([
                    hex(
                        tsr_tsoptions::resolve_config_file_name_of_project_reference(
                            path.as_bytes()
                        )
                        .as_bytes()
                    ),
                    hex(tsr_tsoptions::resolve_project_reference_path(&reference).as_bytes())
                ])
            })
            .collect::<Vec<_>>()))
    }))
}

/// Go's `optionsOf`.
fn options_of(text: Option<&str>) -> Result<Option<tsr_core::CompilerOptions>, String> {
    let Some(text) = text else {
        return Ok(None);
    };
    let input = json!({"files": {}, "currentDirectory": "/p", "caseSensitive": true, "jsonText": text, "args": null});
    Ok(Some(parse_config(&input)?.0.options))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AffectCase {
    #[serde(default)]
    old: Option<String>,
    #[serde(default)]
    new: Option<String>,
    #[serde(default)]
    same: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AffectCases {
    cases: Vec<AffectCase>,
}

type Affect = fn(Option<&tsr_core::CompilerOptions>, Option<&tsr_core::CompilerOptions>) -> bool;

/// Go's `affectColumn`.
fn affect_column(input: &Value, affect: Affect) -> Result<Column, String> {
    let cases: AffectCases = decode(input)?;
    let options = cases
        .cases
        .iter()
        .map(|case| {
            Ok((
                options_of(case.old.as_deref())?,
                options_of(case.new.as_deref())?,
                case.same,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Box::new(move || {
        Ok(json!(options
            .iter()
            .map(|(old, new, same)| {
                let new = if *same { old.as_ref() } else { new.as_ref() };
                affect(old.as_ref(), new)
            })
            .collect::<Vec<_>>()))
    }))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ForEachInput {
    configs: Vec<String>,
    stop_at: String,
}

fn for_each_compiler_option_value(input: &Value) -> Result<Column, String> {
    let input: ForEachInput = decode(input)?;
    let configs = input
        .configs
        .iter()
        .map(|text| options_of(Some(text)).map(Option::unwrap_or_default))
        .collect::<Result<Vec<_>, String>>()?;
    let stop_at = input.stop_at;
    Ok(Box::new(move || {
        let mut out = Vec::new();
        for options in &configs {
            let mut visited = Vec::new();
            let stopped = tsr_tsoptions::affects::for_each_compiler_option_value(
                options,
                |field| field.affects_semantic_diagnostics,
                &mut |field, value, index| {
                    visited.push(json!([
                        hex(field.declaration.as_bytes()),
                        value.is_none(),
                        index
                    ]));
                    field.declaration == stop_at
                },
            );
            out.push(json!([stopped, visited]));
        }
        Ok(Value::Array(out))
    }))
}

/// Go's `showValue`.
fn show_value(value: &tsr_tsoptions::ConfigValue) -> Value {
    use tsr_tsoptions::ConfigValue as V;
    match value {
        V::Boolean(value) => json!(value),
        V::String(text) => json!(hex(text.as_bytes())),
        V::Integer(value) => json!(value),
        #[allow(clippy::cast_possible_truncation)]
        V::Number(value) => json!(*value as i64),
        V::Array(None) => Value::Null,
        V::Array(Some(items)) => Value::Array(items.iter().map(show_value).collect()),
        V::Object(map) => Value::Array(
            map.iter()
                .map(|(key, value)| json!([hex(key.as_bytes()), show_value(value)]))
                .collect(),
        ),
        other => json!(format!("unsupported {other:?}")),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Configs {
    configs: Vec<String>,
}

fn compute_fn_column(input: &Value) -> Result<Column, String> {
    let configs: Configs = decode(input)?;
    let options = configs
        .configs
        .iter()
        .map(|text| options_of(Some(text)).map(Option::unwrap_or_default))
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Box::new(move || {
        Ok(json!(options
            .iter()
            .map(|options| {
                let module_kind =
                    tsr_tsoptions::show_config::compute_fn(options.emit_module_kind().0);
                let allow_js = tsr_tsoptions::show_config::compute_fn(options.allow_js());
                json!([module_kind.1, allow_js.1 != 0])
            })
            .collect::<Vec<_>>()))
    }))
}

fn extended_file_names(input: &Value) -> Result<Column, String> {
    let (host, args) = config_host(input)?;
    let names: Vec<String> = decode(&args)?;
    let current = input["currentDirectory"]
        .as_str()
        .ok_or("currentDirectory")?
        .as_bytes()
        .to_vec();
    let case_sensitive = input["caseSensitive"].as_bool().ok_or("caseSensitive")?;
    let entries = names
        .iter()
        .map(|name| {
            let path = tsr_tspath::to_path(name.as_bytes(), &current, case_sensitive);
            tsr_tsoptions::parse_extended_config(
                name.as_bytes(),
                JsString::from_bytes(path.as_bytes()),
                &[],
                &host,
            )
            .map_err(|error| format!("{error:?}"))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Box::new(move || {
        Ok(json!(entries
            .iter()
            .map(|entry| {
                let names = entry.extended_file_names();
                if names.is_empty() {
                    Value::Null
                } else {
                    hexes(Some(names))
                }
            })
            .collect::<Vec<_>>()))
    }))
}

fn reload_file_names(input: &Value) -> Result<Column, String> {
    use tsr_tsoptions::ParseConfigHost;
    let (parsed, args) = parse_config(input)?;
    let extra: Vec<String> = decode(&args)?;
    let mut merged = input.clone();
    for name in &extra {
        merged["files"][name.as_str()] = json!("");
    }
    let (host, _) = config_host(&merged)?;
    Ok(Box::new(move || {
        let reloaded = parsed
            .reload_file_names_of_parsed_command_line(host.fs())
            .map_err(|error| format!("{error:?}"))?;
        Ok(json!([
            hexes(Some(&reloaded.root_file_names)),
            hexes(reloaded.literal_file_names()),
            hexes(Some(&parsed.root_file_names))
        ]))
    }))
}
