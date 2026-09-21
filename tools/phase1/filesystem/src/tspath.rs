//! The `internal/tspath` group: path algebra, roots, separators, components,
//! comparison and extension handling.
//!
//! Every action drives the production `tsr_tspath` entry point and reports the
//! value it returned. Nothing here emulates an operation, and nothing here reads
//! an expected value. A pinned operation that panics by design is observed
//! through `guarded`, which records the panic class the native probe records.
//!
//! Byte payloads travel as hex in both directions: an argument is UTF-8 text
//! under its own key or hex under `<key>_hex`, exactly one of the two, and a
//! byte result is always hex. A missing argument is a harness failure rather
//! than a defaulted observation.

use serde_json::{json, Value};

use crate::api::{action_op, actions, ordered, subject, Outcome};

pub fn observe(request: &Value) -> Option<Outcome> {
    if subject(request) != "tspath" {
        return None;
    }
    Some(match replay(actions(request)) {
        Ok(rows) => Outcome::Observed(ordered(rows)),
        Err(problem) => Outcome::Failed(problem),
    })
}

fn replay(trace: &[Value]) -> Result<Vec<Value>, String> {
    trace.iter().map(row).collect()
}

fn row(action: &Value) -> Result<Value, String> {
    let op = action_op(action);
    let observed = match op {
        "encoded_root_length" => {
            let path = bytes(action, "path")?;
            json!({
                "op": op,
                "encoded": tsr_tspath::encoded_root_length(&path) as i64,
                "root": tsr_tspath::root_length(&path) as i64,
            })
        }
        "normalize_slashes" => text(op, &tsr_tspath::normalize_slashes(&bytes(action, "path")?)),
        "combine" => {
            let first = bytes(action, "first")?;
            let paths = list(action, "paths")?;
            text(op, &tsr_tspath::combine(&first, &borrow(&paths)))
        }
        "directory_path" => text(op, &tsr_tspath::directory(&bytes(action, "path")?)),
        "normalize_path" => text(op, &tsr_tspath::normalize(&bytes(action, "path")?)),
        "resolve_path" => {
            let path = bytes(action, "path")?;
            let paths = list(action, "paths")?;
            text(op, &tsr_tspath::resolve(&path, &borrow(&paths)))
        }
        "normalized_absolute_path" => {
            let (path, cwd) = (bytes(action, "path")?, bytes(action, "cwd")?);
            text(op, &tsr_tspath::absolute(&path, &cwd))
        }
        "to_file_name_lower_case" => text(
            op,
            &tsr_tspath::file_name_lower_case(&bytes(action, "path")?),
        ),
        "canonical_file_name" => {
            let path = bytes(action, "path")?;
            let sensitive = flag(action, "use_case_sensitive_file_names")?;
            text(op, &tsr_tspath::canonical(&path, sensitive))
        }
        "to_path" => {
            let (file, base) = (bytes(action, "file")?, bytes(action, "base")?);
            let sensitive = flag(action, "case_sensitive")?;
            text(op, tsr_tspath::to_path(&file, &base, sensitive).as_bytes())
        }
        "base_file_name" => text(op, tsr_tspath::base_name(&bytes(action, "path")?)),
        "has_extension" => {
            json!({ "op": op, "result": tsr_tspath::has_extension(&bytes(action, "path")?) })
        }
        "is_declaration_file_name" => {
            let path = bytes(action, "path")?;
            json!({ "op": op, "result": tsr_tspath::is_declaration_file_name(&path) })
        }
        "remove_file_extension" => text(
            op,
            tsr_tspath::remove_file_extension(&bytes(action, "path")?),
        ),
        "relative_to_directory_or_url" => {
            let (from, to) = (bytes(action, "from")?, bytes(action, "to")?);
            let cwd = bytes(action, "cwd")?;
            let as_url = flag(action, "absolute_path_as_url")?;
            let sensitive = flag(action, "case_sensitive")?;
            text(
                op,
                &tsr_tspath::relative_to_directory_or_url(&from, &to, as_url, &cwd, sensitive),
            )
        }
        "ancestor_walk" => {
            let visited: Vec<Value> = tsr_tspath::ancestors(&bytes(action, "path")?)
                .iter()
                .map(|directory| Value::String(hex(directory)))
                .collect();
            json!({ "op": op, "visited": visited })
        }

        "normalized_absolute_path_without_root" => {
            let (path, cwd) = (bytes(action, "path")?, bytes(action, "cwd")?);
            text(
                op,
                &tsr_tspath::normalized_absolute_path_without_root(&path, &cwd),
            )
        }
        "resolve_tripleslash_reference" => {
            let (name, file) = (
                bytes(action, "module_name")?,
                bytes(action, "containing_file")?,
            );
            text(op, &tsr_tspath::resolve_tripleslash_reference(&name, &file))
        }
        "convert_to_relative_path" => {
            let (path, cwd) = (bytes(action, "path")?, bytes(action, "cwd")?);
            let sensitive = flag(action, "use_case_sensitive_file_names")?;
            text(
                op,
                &tsr_tspath::convert_to_relative_path(&path, &cwd, sensitive),
            )
        }
        "root_predicates" => {
            let path = bytes(action, "path")?;
            json!({
                "op": op,
                "path_is_absolute": tsr_tspath::path_is_absolute(&path),
                "is_rooted_disk_path": tsr_tspath::is_rooted_disk_path(&path),
                "is_url": tsr_tspath::is_url(&path),
                "is_disk_path_root": tsr_tspath::is_disk_path_root(&path),
                "is_dynamic_file_name": tsr_tspath::is_dynamic_file_name(&path),
            })
        }
        "byte_predicates" => {
            let byte = u8::try_from(number(action, "byte")?)
                .map_err(|_| "byte argument out of range".to_string())?;
            json!({
                "op": op,
                "is_volume_character": tsr_tspath::is_volume_character(byte),
                "is_any_directory_separator": tsr_tspath::is_any_directory_separator(byte),
            })
        }
        "file_url_volume_separator_end" => {
            let url = bytes(action, "url")?;
            let start = usize::try_from(number(action, "start")?)
                .map_err(|_| "negative start".to_string())?;
            let end =
                tsr_tspath::file_url_volume_separator_end(&url, start).map_or(-1, |end| end as i64);
            json!({ "op": op, "result": end })
        }
        "split_volume_path" => {
            let path = bytes(action, "path")?;
            let (volume, rest, ok) = tsr_tspath::split_volume_path(&path);
            json!({ "op": op, "result": [hex(&volume), hex(rest), ok] })
        }
        "module_name_shape" => {
            let path = bytes(action, "path")?;
            json!({
                "op": op,
                "ensure_non_module_name": hex(&tsr_tspath::ensure_path_is_non_module_name(&path)),
                "is_external_module_name_relative":
                    tsr_tspath::is_external_module_name_relative(&path),
            })
        }
        "trailing_separator_family" => {
            let path = bytes(action, "path")?;
            let typed = tsr_tspath::Path::from_bytes(path.clone());
            json!({
                "op": op,
                "has": tsr_tspath::has_trailing_directory_separator(&path),
                "remove": hex(tsr_tspath::remove_trailing_directory_separator(&path)),
                "remove_all": hex(tsr_tspath::remove_trailing_directory_separators(&path)),
                "ensure": hex(&tsr_tspath::ensure_trailing_directory_separator(&path)),
                "path_remove": hex(typed.remove_trailing_directory_separator().as_bytes()),
                "path_ensure": hex(typed.ensure_trailing_directory_separator().as_bytes()),
            })
        }
        "contains_ignored_path" => {
            json!({ "op": op, "result": tsr_tspath::contains_ignored_path(&bytes(action, "path")?) })
        }
        "starts_with_directory" => {
            let (file, directory) = (bytes(action, "file")?, bytes(action, "directory")?);
            let sensitive = flag(action, "case_sensitive")?;
            json!({ "op": op, "result": tsr_tspath::starts_with_directory(&file, &directory, sensitive) })
        }
        "path_components" => {
            let (path, cwd) = (bytes(action, "path")?, bytes(action, "cwd")?);
            json!({ "op": op, "components": hex_all(&tsr_tspath::path_components(&path, &cwd)), "panic": "" })
        }
        "path_components_split" => {
            let path = bytes(action, "path")?;
            let root = usize::try_from(number(action, "root_length")?)
                .map_err(|_| "negative root length".to_string())?;
            let (value, panicked) =
                guarded(|| hex_all(&tsr_tspath::split_path_components(&path, root)));
            json!({ "op": op, "components": value, "panic": panicked })
        }
        "reduce_path_components" => {
            let components = list(action, "components")?;
            json!({ "op": op, "components": hex_all(&tsr_tspath::reduce_path_components(&components)) })
        }
        "normalized_components_from_combined" => {
            let path = bytes(action, "path")?;
            json!({ "op": op, "components": hex_all(&tsr_tspath::normalized_components_from_combined(&path)) })
        }
        "path_components_relative_to" => {
            let (from, to, cwd) = (
                bytes(action, "from")?,
                bytes(action, "to")?,
                bytes(action, "cwd")?,
            );
            let sensitive = flag(action, "case_sensitive")?;
            let components = tsr_tspath::path_components_relative_to(&from, &to, &cwd, sensitive);
            json!({ "op": op, "components": hex_all(&components) })
        }
        "simple_normalize_path" => {
            let path = bytes(action, "path")?;
            let result = tsr_tspath::simple_normalize_path(&path);
            json!({ "op": op, "ok": result.is_some(), "result": hex(&result.unwrap_or_default()) })
        }
        "has_relative_path_segment" => {
            json!({ "op": op, "result": tsr_tspath::has_relative_path_segment(&bytes(action, "path")?) })
        }
        "trim_rune_count" => {
            let text_bytes = bytes(action, "s")?;
            let count = isize::try_from(number(action, "rune_count")?)
                .map_err(|_| "rune count out of range".to_string())?;
            text(op, tsr_tspath::trim_rune_count(&text_bytes, count))
        }
        "common_parents" => {
            let paths = list(action, "paths")?;
            let (cwd, min) = (
                bytes(action, "cwd")?,
                number(action, "min_components")? as isize,
            );
            let sensitive = flag(action, "use_case_sensitive_file_names")?;
            let (value, panicked) = guarded(|| {
                let (parents, ignored) = tsr_tspath::common_parents(
                    &borrow(&paths),
                    min,
                    tsr_tspath::path_components,
                    &cwd,
                    sensitive,
                );
                let ignored: Vec<Vec<u8>> = ignored.into_iter().collect();
                json!([hex_all(&parents), hex_all(&ignored)])
            });
            if panicked.is_empty() {
                json!({ "op": op, "parents": value[0], "ignored": value[1], "panic": "" })
            } else {
                json!({ "op": op, "parents": null, "ignored": null, "panic": panicked })
            }
        }
        "common_parents_worker" => {
            let groups = lists(action, "groups")?;
            let (cwd, min) = (
                bytes(action, "cwd")?,
                number(action, "min_components")? as isize,
            );
            let sensitive = flag(action, "use_case_sensitive_file_names")?;
            let rendered: Vec<Value> =
                tsr_tspath::common_parents_worker(&groups, min, &cwd, sensitive)
                    .iter()
                    .map(|group| hex_all(group))
                    .collect();
            json!({ "op": op, "groups": rendered })
        }
        "compare_paths_wrappers" => {
            let (a, b, cwd) = (
                bytes(action, "a")?,
                bytes(action, "b")?,
                bytes(action, "cwd")?,
            );
            json!({
                "op": op,
                "sensitive": tsr_tspath::compare_paths_case_sensitive(&a, &b, &cwd) as i8,
                "insensitive": tsr_tspath::compare_paths_case_insensitive(&a, &b, &cwd) as i8,
            })
        }
        "path_comparer" => {
            let compare = tsr_tspath::path_comparer(flag(action, "use_case_sensitive_file_names")?);
            json!({ "op": op, "result": compare(&bytes(action, "left")?, &bytes(action, "right")?) as i8 })
        }
        "path_equality_comparer" => {
            let equal =
                tsr_tspath::path_equality_comparer(flag(action, "use_case_sensitive_file_names")?);
            json!({ "op": op, "result": equal(&bytes(action, "left")?, &bytes(action, "right")?) })
        }
        "path_comparer_sort" => {
            let compare = tsr_tspath::path_comparer(flag(action, "use_case_sensitive_file_names")?);
            let mut items = list(action, "items")?;
            items.sort_by(|a, b| compare(a, b));
            json!({ "op": op, "items": hex_all(&items) })
        }
        "compare_number_of_directory_separators" => {
            let (left, right) = (bytes(action, "left")?, bytes(action, "right")?);
            json!({ "op": op, "result": tsr_tspath::compare_number_of_directory_separators(&left, &right) as i8 })
        }
        "typed_path_contains" => {
            let (parent, child) = (bytes(action, "parent")?, bytes(action, "child")?);
            let typed = tsr_tspath::Path::from_bytes(parent.clone())
                .contains_path(&tsr_tspath::Path::from_bytes(child.clone()));
            json!({
                "op": op,
                "typed": typed,
                "free_case_sensitive": tsr_tspath::contains_path(&parent, &child, b"", true),
                "free_case_insensitive": tsr_tspath::contains_path(&parent, &child, b"", false),
            })
        }
        "typed_path_directory" => {
            let path = bytes(action, "path")?;
            let typed = tsr_tspath::Path::from_bytes(path.clone()).directory_path();
            json!({ "op": op, "typed": hex(typed.as_bytes()), "free": hex(&tsr_tspath::directory(&path)) })
        }
        "ancestor_walk_stop_at" => {
            let (path, stop) = (bytes(action, "path")?, optional(action, "stop_at")?);
            let mut visited = Vec::new();
            let returned = tsr_tspath::for_each_ancestor_directory(&path, |directory| {
                visited.push(Value::String(hex(directory)));
                (carried(directory), stop.as_deref() == Some(directory))
            });
            json!({ "op": op, "visited": visited, "ok": returned.is_some(),
                    "returned": hex(&returned.unwrap_or_default()) })
        }
        "ancestor_walk_path" => {
            if bytes(action, "dialect")? != b"path" {
                return Err("the Path-typed walk refuses another dialect".into());
            }
            let (path, stop) = (bytes(action, "path")?, optional(action, "stop_at")?);
            let mut visited = Vec::new();
            let start = tsr_tspath::Path::from_bytes(path);
            let returned = tsr_tspath::for_each_ancestor_directory_path(&start, |directory| {
                visited.push(Value::String(hex(directory.as_bytes())));
                (
                    carried(directory.as_bytes()),
                    stop.as_deref() == Some(directory.as_bytes()),
                )
            });
            json!({ "op": op, "visited": visited, "ok": returned.is_some(),
                    "returned": hex(&returned.unwrap_or_default()) })
        }
        "ancestor_walk_stopping_at_global_cache" => {
            let (cache, path) = (bytes(action, "global_cache")?, bytes(action, "path")?);
            let stop = optional(action, "stop_at")?;
            let mut visited = Vec::new();
            let returned = tsr_tspath::for_each_ancestor_directory_stopping_at_global_cache(
                &cache,
                &path,
                |directory| {
                    visited.push(Value::String(hex(directory)));
                    (carried(directory), stop.as_deref() == Some(directory))
                },
            );
            json!({ "op": op, "visited": visited, "returned": hex(&returned.unwrap_or_default()) })
        }
        "extension_is_ts" => {
            json!({ "op": op, "result": tsr_tspath::extension_is_ts(&bytes(action, "ext")?) })
        }
        "extension_is_one_of" => {
            let extensions = list(action, "extensions")?;
            json!({ "op": op, "result": tsr_tspath::extension_is_one_of(&bytes(action, "ext")?, &extensions) })
        }
        "file_extension_is" => {
            let (path, extension) = (bytes(action, "path")?, bytes(action, "extension")?);
            json!({ "op": op, "result": tsr_tspath::file_extension_is(&path, &extension) })
        }
        "file_extension_is_one_of" => {
            let extensions = list(action, "extensions")?;
            json!({ "op": op, "result": tsr_tspath::file_extension_is_one_of(&bytes(action, "path")?, &extensions) })
        }
        "extension_family_predicates" => {
            let path = bytes(action, "path")?;
            json!({
                "op": op,
                "ts": tsr_tspath::has_ts_file_extension(&path),
                "js": tsr_tspath::has_js_file_extension(&path),
                "json": tsr_tspath::has_json_file_extension(&path),
                "implementation_ts": tsr_tspath::has_implementation_ts_file_extension(&path),
            })
        }
        "extension_extract_tables" => {
            let path = bytes(action, "path")?;
            json!({
                "op": op,
                "try_get_extension_from_path": hex(tsr_tspath::try_get_extension_from_path(&path)),
                "try_extract_ts_extension": hex(tsr_tspath::try_extract_ts_extension(&path)),
                "declaration_file_extension": hex(tsr_tspath::declaration_file_extension(&path)),
                "declaration_emit_extension": hex(&tsr_tspath::declaration_emit_extension_for_path(&path)),
                "possible_original_input_extensions":
                    hex_all(&tsr_tspath::possible_original_input_extensions(&path)),
            })
        }
        "any_extension_from_path" => {
            let (path, extensions) = (bytes(action, "path")?, list(action, "extensions")?);
            let ignore = flag(action, "ignore_case")?;
            text(
                op,
                tsr_tspath::any_extension_from_path(&path, &extensions, ignore),
            )
        }
        "longest_extension_from_path" => {
            let (path, extensions) = (bytes(action, "path")?, list(action, "extensions")?);
            let ignore = flag(action, "ignore_case")?;
            text(
                op,
                tsr_tspath::longest_extension_from_path(&path, &extensions, ignore),
            )
        }
        "any_extension_worker" => {
            let (path, extensions) = (bytes(action, "path")?, list(action, "extensions")?);
            let equal = tsr_jsstring::compare::equality_comparer(flag(action, "ignore_case")?);
            text(
                op,
                tsr_tspath::any_extension_from_path_worker(&path, &extensions, equal),
            )
        }
        "try_get_extension_from_path" => {
            let (path, extension) = (bytes(action, "path")?, bytes(action, "extension")?);
            let equal = tsr_jsstring::compare::equality_comparer(flag(action, "ignore_case")?);
            text(
                op,
                tsr_tspath::try_get_extension_from_path_with(&path, &extension, equal),
            )
        }
        "change_extension" => {
            let (path, ext) = (bytes(action, "path")?, bytes(action, "ext")?);
            text(op, &tsr_tspath::change_extension(&path, &ext))
        }
        "change_any_extension" => {
            let (path, ext) = (bytes(action, "path")?, bytes(action, "ext")?);
            let extensions = list(action, "extensions")?;
            let ignore = flag(action, "ignore_case")?;
            text(
                op,
                &tsr_tspath::change_any_extension(&path, &ext, &extensions, ignore),
            )
        }
        "change_full_extension" => {
            let (path, ext) = (bytes(action, "path")?, bytes(action, "ext")?);
            text(op, &tsr_tspath::change_full_extension(&path, &ext))
        }
        "remove_extension" => {
            let (path, extension) = (bytes(action, "path")?, bytes(action, "extension")?);
            let (value, panicked) =
                guarded(|| Value::String(hex(tsr_tspath::remove_extension(&path, &extension))));
            json!({ "op": op, "result": value, "panic": panicked })
        }
        "remove_any_file_extension" => text(
            op,
            tsr_tspath::remove_any_file_extension(&bytes(action, "path")?),
        ),
        _ => {
            return Err(format!(
                "unsupported action {op:?}: an unknown action is a harness failure, never an \
                 observation"
            ))
        }
    };
    Ok(observed)
}

/// The value every walk callback carries out, so a stopped walk and an
/// exhausted one are told apart by more than the stop flag.
fn carried(directory: &[u8]) -> Vec<u8> {
    [b"v:".as_slice(), directory].concat()
}

fn hex_all<T: AsRef<[u8]>>(values: &[T]) -> Value {
    Value::Array(
        values
            .iter()
            .map(|value| Value::String(hex(value.as_ref())))
            .collect(),
    )
}

/// Runs an operation whose pinned form panics by design and records the class
/// of panic the native probe records. The default hook is silenced for the call
/// so an expected panic does not write to the driver's diagnostics.
fn guarded(operation: impl FnOnce() -> Value + std::panic::UnwindSafe) -> (Value, String) {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(operation);
    std::panic::set_hook(hook);
    match outcome {
        Ok(value) => (value, String::new()),
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_string())
                })
                .unwrap_or_default();
            let class = if message.contains("out of range") {
                "index_out_of_range".to_string()
            } else {
                format!("other:{message}")
            };
            (Value::Null, class)
        }
    }
}

fn number(action: &Value, key: &str) -> Result<i64, String> {
    action
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("action key {key:?} is missing or is not an integer"))
}

/// A byte argument whose key must be present but whose value may be null.
fn optional(action: &Value, key: &str) -> Result<Option<Vec<u8>>, String> {
    match action.get(key) {
        None => Err(format!("action key {key:?} is missing")),
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_bytes().to_vec())),
        Some(_) => Err(format!("action key {key:?} is not a string or null")),
    }
}

fn lists(action: &Value, key: &str) -> Result<Vec<Vec<Vec<u8>>>, String> {
    action
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("action key {key:?} is not an array"))?
        .iter()
        .map(|group| {
            group
                .as_array()
                .ok_or_else(|| format!("action key {key:?} holds a non-array entry"))?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(|item| item.as_bytes().to_vec())
                        .ok_or_else(|| format!("action key {key:?} holds a non-string entry"))
                })
                .collect()
        })
        .collect()
}

/// One byte result, hex encoded under the same key both sides use.
fn text(op: &str, value: &[u8]) -> Value {
    json!({ "op": op, "result": hex(value) })
}

fn borrow(paths: &[Vec<u8>]) -> Vec<&[u8]> {
    paths.iter().map(Vec::as_slice).collect()
}

/// A byte argument: UTF-8 text under `key`, or hex under `key_hex`. Exactly
/// one of the two must be present, because a defaulted argument would produce
/// a row that ran nothing.
fn bytes(action: &Value, key: &str) -> Result<Vec<u8>, String> {
    let plain = action.get(key);
    let encoded = action.get(format!("{key}_hex"));
    match (plain, encoded) {
        (Some(_), Some(_)) | (None, None) => Err(format!(
            "an action must carry exactly one of {key:?} and {key}_hex"
        )),
        (Some(value), None) => value
            .as_str()
            .map(|value| value.as_bytes().to_vec())
            .ok_or_else(|| format!("action key {key:?} is not a string")),
        (None, Some(value)) => {
            let encoded = value
                .as_str()
                .ok_or_else(|| format!("action key {key}_hex is not a string"))?;
            unhex(encoded).ok_or_else(|| format!("action key {key}_hex is malformed: {encoded:?}"))
        }
    }
}

fn flag(action: &Value, key: &str) -> Result<bool, String> {
    action
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("action key {key:?} is missing or is not a boolean"))
}

fn list(action: &Value, key: &str) -> Result<Vec<Vec<u8>>, String> {
    let items = action
        .get(key)
        .ok_or_else(|| format!("action key {key:?} is missing"))?;
    if items.is_null() {
        return Ok(Vec::new());
    }
    items
        .as_array()
        .ok_or_else(|| format!("action key {key:?} is not an array"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(|item| item.as_bytes().to_vec())
                .ok_or_else(|| format!("action key {key:?} holds a non-string entry"))
        })
        .collect()
}

fn hex(value: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(value.len() * 2);
    for byte in value {
        write!(out, "{byte:02x}").expect("writing to a String is infallible");
    }
    out
}

fn unhex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}
