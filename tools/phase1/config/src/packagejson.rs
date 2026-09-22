//! Package field, dependency and cache probes over the production APIs.
//! The owner-approved immutable-content exception remains an observed difference.

use std::sync::Arc;

use serde_json::{json, Value};
use tsr_jsstring::JsString;
use tsr_module::package_json::{self, Fields};
use tsr_module::{PackageJson, Resolver};
use tsr_vfs::MemoryBuilder;

use crate::api::{action_op, action_str, actions, ordered, subject, Outcome};

/// The request subject this group claims.
const SUBJECT: &str = "packageJson";

/// The ordered field vocabulary the `Expected` cases address, identical to the
/// probe's `phase1FieldNames`. Both sides walk this list, so a port that grew
/// or lost a field shows up as a length difference rather than as a silently
/// skipped row.
const FIELD_NAMES: &[&str] = &[
    "name",
    "version",
    "type",
    "tsconfig",
    "main",
    "types",
    "typings",
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
    "contentMapper",
    "contentMapper.exec",
    "contentMapper.compilerOptions",
    "contentMapper.dynamicConfig",
];

fn text(value: &JsString) -> String {
    String::from_utf8_lossy(value.as_bytes()).into_owned()
}

/// The validity the `TypeValidatedField` view reports, read off the port's own
/// parsed state. A field the parse never saw has no entry and is not valid,
/// which is the pin's zero-valued `Expected` answering false.
fn field_valid(fields: &Fields, name: &str) -> bool {
    if name == "contentMapper" {
        return fields.content_mapper.state.valid;
    }
    if let Some(inner) = name.strip_prefix("contentMapper.") {
        return fields
            .content_mapper
            .value
            .field(inner)
            .is_some_and(|field| field.state.valid);
    }
    fields.field(name).is_some_and(|field| field.state.valid)
}

/// Build the package through the production resolver, which is the only way to
/// reach a `tsr_module::PackageJson`: `Resolver::package_json` is what
/// constructs `PackageContents` and the version-paths cache that lives on it.
/// The snapshot is in memory, so the case is host independent.
fn load_package(source: &str) -> Result<Arc<PackageJson>, String> {
    let mut files = MemoryBuilder::new(b"/repo", true);
    files.insert_loaded(b"/repo/pkg/package.json", source.as_bytes().to_vec());
    let mut resolver = Resolver::new(
        Arc::new(files.finish()),
        Arc::new(tsr_core::CompilerOptions::default()),
        b"/repo",
    )
    .map_err(|error| format!("resolver construction failed: {error:?}"))?;
    resolver
        .package_json(b"/repo/pkg")
        .map_err(|error| format!("reading /repo/pkg/package.json failed: {error:?}"))?
        .ok_or_else(|| "the memory snapshot did not surface /repo/pkg/package.json".to_owned())
}

fn same_mappings(
    first: Option<&tsr_core::PathMappings>,
    second: Option<&tsr_core::PathMappings>,
) -> bool {
    match (first, second) {
        (Some(first), Some(second)) => std::ptr::eq(first, second),
        (None, None) => true,
        _ => false,
    }
}

fn render_mappings(paths: &tsr_core::PathMappings) -> Vec<Value> {
    paths
        .iter()
        .map(|(key, values)| match values {
            Some(values) => json!([text(key), values.iter().map(text).collect::<Vec<_>>()]),
            // The pin cannot produce this: GetPaths always stores a slice. It
            // is rendered rather than flattened so a port that lost the values
            // is visible instead of looking empty.
            None => json!([text(key), Value::Null]),
        })
        .collect()
}

/// The `Expected` validity case. Every action is either a parse or a read of
/// the parsed state, so the whole trace runs on `package_json::parse` and the
/// public field readers.
fn expected_validity(request: &Value) -> Outcome {
    let mut fields: Option<Fields> = None;
    let mut rows = Vec::new();
    for action in actions(request) {
        match action_op(action) {
            "load" => {
                let parsed = package_json::parse(action_str(action, "source").as_bytes());
                let parseable = parsed.parseable;
                fields = Some(parsed.fields);
                rows.push(json!({"op": "load", "parsed": parseable}));
            }
            "expected_is_valid" => {
                let Some(fields) = fields.as_ref() else {
                    return Outcome::Failed(
                        "expected_is_valid ran before a load action parsed a document".to_owned(),
                    );
                };
                let result: Vec<Value> = FIELD_NAMES
                    .iter()
                    .map(|name| json!([name, field_valid(fields, name)]))
                    .collect();
                rows.push(json!({"op": "expected_is_valid", "result": result}));
            }
            other => {
                return Outcome::Failed(format!(
                    "the expected-validity case sent unsupported action {other:?}"
                ))
            }
        }
    }
    Outcome::Observed(ordered(rows))
}

/// Native version selection, retrieval-local mappings and replayed traces.
fn version_paths(request: &Value) -> Outcome {
    let mut package: Option<Arc<PackageJson>> = None;
    let mut rows = Vec::new();
    for action in actions(request) {
        match action_op(action) {
            "load" => match load_package(action_str(action, "source")) {
                Ok(loaded) => {
                    let parseable = loaded.parseable;
                    package = Some(loaded);
                    rows.push(json!({"op": "load", "parsed": parseable}));
                }
                Err(error) => return Outcome::Failed(error),
            },
            "get_version_paths" | "get_version_paths_traced" => {
                let Some(package) = package.as_ref() else {
                    return Outcome::Failed(
                        "get_version_paths ran before a load action built the package".to_owned(),
                    );
                };
                let mut traces = Vec::new();
                let paths = package.version_paths_traced(|message| traces.push(message.clone()));
                let row = if action_op(action) == "get_version_paths_traced" {
                    let traces: Vec<_> = traces
                        .iter()
                        .map(|message| {
                            let args: Vec<_> = message
                                .args
                                .iter()
                                .map(|arg| match arg {
                                    tsr_module::TraceArg::Text(value) => text(value),
                                    tsr_module::TraceArg::Bool(value) => value.to_string(),
                                })
                                .collect();
                            json!([message.message.code, args])
                        })
                        .collect();
                    json!({"op": action_op(action), "exists": paths.exists(), "traces": traces})
                } else {
                    json!({"op": action_op(action), "result": [paths.exists()]})
                };
                rows.push(row);
            }
            "version_paths_mappings" => {
                let Some(package) = package.as_ref() else {
                    return Outcome::Failed(
                        "version_paths_mappings ran before a load action built the package"
                            .to_owned(),
                    );
                };
                let result = package
                    .version_paths()
                    .paths()
                    .map_or(Value::Null, |paths| Value::Array(render_mappings(paths)));
                rows.push(json!({"op": "version_paths_mappings", "result": result}));
            }
            "version_paths_identity" => {
                let Some(package) = package.as_ref() else {
                    return Outcome::Failed(
                        "version_paths_identity ran before a load action built the package"
                            .to_owned(),
                    );
                };
                let retrieval = package.version_paths();
                let other = package.version_paths();
                let first = retrieval.paths();
                let second = retrieval.paths();
                let third = other.paths();
                let sizes = match (first, third) {
                    (Some(first), Some(third)) => first.len() == third.len(),
                    (None, None) => true,
                    _ => false,
                };
                rows.push(json!({
                    "op": "version_paths_identity",
                    "result": [same_mappings(first, second), same_mappings(first, third)],
                    "shape": [first.is_some(), second.is_some(), third.is_some(), sizes],
                }));
            }
            other => {
                return Outcome::Failed(format!(
                    "the version-paths cases sent unsupported action {other:?}"
                ))
            }
        }
    }
    Outcome::Observed(ordered(rows))
}

fn expected_of<T: package_json::DeclaredJsonType>(value: T) -> Value {
    let field = package_json::Expected::of(value);
    json!([
        field.state.valid,
        field.state.null,
        field.state.actual_type,
        package_json::Expected::<T>::expected_json_type(),
        field.state.actual_type == package_json::Expected::<T>::expected_json_type()
    ])
}
fn declared_type(kind: &str) -> Result<&'static str, String> {
    use package_json::Expected;
    Ok(match kind {
        "string" | "empty_string" => Expected::<String>::expected_json_type(),
        "bool" => Expected::<bool>::expected_json_type(),
        "string_slice" => Expected::<Vec<String>>::expected_json_type(),
        "string_map" => {
            Expected::<std::collections::BTreeMap<String, String>>::expected_json_type()
        }
        "content_mapper_fields" => Expected::<package_json::ContentMapper>::expected_json_type(),
        _ => return Err(format!("unknown Expected type {kind:?}")),
    })
}
fn entry(kind: &str) -> Result<Option<Arc<tsr_module::InfoCacheEntry>>, String> {
    if kind == "nil" {
        return Ok(None);
    }
    if !matches!(
        kind,
        "empty_contents" | "with_contents" | "with_contents_absent_directory" | "empty_directory"
    ) {
        return Err(format!("unknown entry receiver {kind:?}"));
    }
    Ok(Some(Arc::new(tsr_module::InfoCacheEntry {
        package_directory: JsString::from_bytes(if kind == "empty_directory" {
            b""
        } else {
            b"/repo/pkg".as_slice()
        }),
        directory_exists: matches!(kind, "empty_contents" | "with_contents"),
        contents: if kind == "empty_contents" {
            None
        } else {
            Some(load_package("!")?)
        },
    })))
}
fn same_contents(a: &tsr_module::InfoCacheEntry, b: &tsr_module::InfoCacheEntry) -> bool {
    match (&a.contents, &b.contents) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}
#[allow(clippy::too_many_lines, reason = "one arm per declared package action")]
fn package_actions(request: &Value) -> Result<Outcome, String> {
    let mut fields = None;
    let mut cache = None;
    let mut rows = Vec::new();
    for action in actions(request) {
        let op = action_op(action);
        let mut row = json!({"op":op});
        match op {
            "load" => {
                let parsed = package_json::parse(action_str(action, "source").as_bytes());
                row["parsed"] = json!(parsed.parseable);
                fields = Some(parsed.fields);
            }
            "expected_json_type" => {
                let fields = fields.as_ref().ok_or("load must precede field query")?;
                row["result"] = json!(FIELD_NAMES
                    .iter()
                    .map(|name| json!([
                        name,
                        fields
                            .validated_field(name)
                            .expect("known field")
                            .expected_json_type
                    ]))
                    .collect::<Vec<_>>());
            }
            "expected_of" | "nil_receiver_expected_json_type" => {
                let kind = action_str(action, "kind");
                row["kind"] = json!(kind);
                row["result"] = if op == "nil_receiver_expected_json_type" {
                    json!(["", declared_type(kind)?])
                } else {
                    match kind {
                        "string" => expected_of("sample".to_owned()),
                        "empty_string" => expected_of(String::new()),
                        "bool" => expected_of(false),
                        "string_map" => {
                            expected_of(None::<std::collections::BTreeMap<String, String>>)
                        }
                        "string_slice" => expected_of(None::<Vec<String>>),
                        "content_mapper_fields" => {
                            expected_of(package_json::ContentMapper::default())
                        }
                        _ => return Err(format!("unknown ExpectedOf type {kind:?}")),
                    }
                };
            }
            "json_value_type_string" => {
                let value = action["value"].as_i64().ok_or("value must be integer")?;
                row["value"] = json!(value);
                row["result"] = json!(package_json::JsonValueType(
                    i8::try_from(value).map_err(|_| "value outside int8")?
                )
                .to_string());
            }
            "json_value_is_present" | "json_value_as_string" => {
                let name = action_str(action, "field");
                row["field"] = json!(name);
                let value = fields
                    .as_ref()
                    .ok_or("load must precede field query")?
                    .json_value(name);
                row["result"] = if op == "json_value_is_present" {
                    json!([value.is_present(), value.kind().0])
                } else {
                    match std::panic::catch_unwind(|| value.as_string()) {
                        Ok(s) => json!(["", s]),
                        Err(payload) => {
                            let message = payload
                                .downcast_ref::<String>()
                                .map(String::as_str)
                                .or_else(|| payload.downcast_ref::<&str>().copied())
                                .unwrap_or("");
                            if !message.starts_with("expected string, got ") {
                                return Err(format!("unexpected string-access panic: {message}"));
                            }
                            json!(["as_string_off_type", ""])
                        }
                    }
                };
            }
            "has_dependency" => {
                let name = action_str(action, "name");
                row["name"] = json!(name);
                row["result"] = json!(fields.as_ref().ok_or("load first")?.has_dependency(name));
            }
            "range_dependencies" => {
                let stop = action["stop_after"].as_i64().ok_or("stop_after")?;
                let mut sequence = Vec::new();
                let mut visits = Vec::new();
                fields
                    .as_ref()
                    .ok_or("load first")?
                    .range_dependencies(|name, version, field| {
                        let order = [
                            "dependencies",
                            "devDependencies",
                            "peerDependencies",
                            "optionalDependencies",
                        ]
                        .iter()
                        .position(|s| *s == field)
                        .expect("known dependency field");
                        if sequence.last().is_none_or(|last| *last != field) {
                            sequence.push(field);
                        }
                        visits.push((order, name.to_owned(), version.to_owned(), field));
                        stop < 0 || (visits.len() as i64) < stop
                    });
                visits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
                row["stop_after"] = json!(stop);
                row["count"] = json!(visits.len());
                row["field_sequence"] = json!(sequence);
                row["visits"] = json!(visits);
            }
            "runtime_dependency_names" => {
                let names = fields
                    .as_ref()
                    .ok_or("load first")?
                    .runtime_dependency_names();
                row["result"] = json!([true, names.len(), names]);
            }
            "entry_readers" => {
                let kind = action_str(action, "receiver");
                let value = entry(kind)?;
                row["receiver"] = json!(kind);
                row["result"] = json!([
                    value.as_ref().is_some_and(|v| v.exists()),
                    value.as_ref().is_none_or(|v| v.contents.is_none()),
                    value
                        .as_ref()
                        .map_or_else(String::new, |v| text(&v.package_directory))
                ]);
            }
            "with_package_directory" => {
                let kind = action_str(action, "receiver");
                let directory = action_str(action, "directory");
                row["receiver"] = json!(kind);
                row["directory"] = json!(directory);
                // A nil Go receiver cannot be borrowed as &Arc in Rust. Record
                // this precondition at the representation boundary, not by
                // manufacturing a crash in an unrelated production operation.
                row["result"] = if let Some(original) = entry(kind)? {
                    let result = original.with_package_directory(directory.as_bytes());
                    json!([
                        "",
                        Arc::ptr_eq(&original, &result),
                        same_contents(&original, &result),
                        text(&result.package_directory),
                        result.directory_exists
                    ])
                } else {
                    json!(["nil_pointer_dereference"])
                };
            }
            "with_package_directory_shares_contents" => {
                let original = entry("with_contents")?.expect("present entry");
                let copied = original.with_package_directory(b"/repo/other");
                let before = original.contents.as_ref().expect("contents").parseable;
                row["result"] = json!([
                    Arc::ptr_eq(&original, &copied),
                    same_contents(&original, &copied),
                    before,
                    original.contents.as_ref().expect("contents").parseable
                ]);
                row["mutation_performed"] = json!(false); // approved immutable publication
            }
            "new_info_cache" => {
                let dir = action_str(action, "directory");
                let sensitive = action["case_sensitive"].as_bool().ok_or("case_sensitive")?;
                cache = Some(tsr_module::InfoCache::new(dir.as_bytes(), sensitive));
                row["result"] = json!([dir, sensitive]);
            }
            "cache_set" => {
                let path = action_str(action, "path");
                let label = action_str(action, "label");
                let exists = action["exists"].as_bool().ok_or("exists")?;
                let value = Arc::new(tsr_module::InfoCacheEntry {
                    package_directory: JsString::from_bytes(label.as_bytes()),
                    directory_exists: exists,
                    contents: Some(load_package("!")?),
                });
                let result = cache
                    .as_ref()
                    .ok_or("new_info_cache first")?
                    .set(path.as_bytes(), value.clone());
                row["path"] = json!(path);
                row["label"] = json!(label);
                row["result"] = json!([
                    true,
                    Arc::ptr_eq(&result, &value),
                    text(&result.package_directory)
                ]);
            }
            "cache_get" => {
                let path = action_str(action, "path");
                let value = cache
                    .as_ref()
                    .ok_or("new_info_cache first")?
                    .get(path.as_bytes());
                row["path"] = json!(path);
                row["result"] = json!([
                    value.is_some(),
                    value
                        .as_ref()
                        .map_or_else(String::new, |v| text(&v.package_directory))
                ]);
            }
            "cache_range" => {
                let stop = action["stop_after"].as_i64().ok_or("stop_after")?;
                let mut labels = Vec::new();
                let mut count = 0;
                cache
                    .as_ref()
                    .ok_or("new_info_cache first")?
                    .range(|_, value| {
                        count += 1;
                        labels.push(text(&value.package_directory));
                        stop < 0 || count < stop
                    });
                if stop >= 0 {
                    labels.clear();
                }
                labels.sort();
                row["stop_after"] = json!(stop);
                row["count"] = json!(count);
                row["complete"] = json!(stop < 0);
                row["labels"] = json!(labels);
            }
            _ => return Err(format!("unknown package action {op:?}")),
        }
        rows.push(row);
    }
    Ok(Outcome::Observed(ordered(rows)))
}
pub fn observe(request: &Value) -> Option<Outcome> {
    if subject(request) != SUBJECT {
        return None;
    }
    Some(
        if actions(request).iter().any(|action| {
            action_op(action).starts_with("get_version_paths")
                || action_op(action).starts_with("version_paths_")
        }) {
            version_paths(request)
        } else if actions(request)
            .iter()
            .any(|action| action_op(action) == "expected_is_valid")
        {
            expected_validity(request)
        } else {
            package_actions(request).unwrap_or_else(Outcome::Failed)
        },
    )
}
