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
