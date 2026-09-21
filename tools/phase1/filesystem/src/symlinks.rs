//! The `internal/symlinks` known-symlink cache group.
//!
//! Every action drives `tsr_module::symlinks::KnownSymlinks`. What the group is
//! about is the bookkeeping: which index a single call writes, that both
//! reverse indexes grow only on a symlink key's first insertion while the
//! forward maps are overwritten every time, and what the directory walk
//! consumes before it stops. A replay holds named caches, exactly as the native
//! probe does, and renders each index sorted because the pinned maps are
//! unordered. Nothing here emulates the cache and nothing reads an expected
//! value.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use tsr_module::symlinks::{KnownDirectoryLink, KnownSymlinks, SymlinkSet};
use tsr_tspath::Path;

use crate::api::{action_op, actions, ordered, Outcome};

pub fn observe(request: &Value) -> Option<Outcome> {
    if crate::api::subject(request) != "symlinks.KnownSymlinks" {
        return None;
    }
    let mut caches = BTreeMap::new();
    let rows: Result<Vec<Value>, String> = actions(request)
        .iter()
        .map(|action| row(&mut caches, action))
        .collect();
    Some(match rows {
        Ok(rows) => Outcome::Observed(ordered(rows)),
        Err(problem) => Outcome::Failed(problem),
    })
}

type Caches = BTreeMap<Vec<u8>, KnownSymlinks>;

fn row(caches: &mut Caches, action: &Value) -> Result<Value, String> {
    let op = action_op(action);
    if op == "new" {
        let name = bytes(action, "target")?;
        let cache = KnownSymlinks::new(&bytes(action, "cwd")?, flag(action, "case_sensitive")?);
        // Read back off the cache, not off the request.
        let row = json!({ "op": op, "not_nil": true, "cwd": hex(cache.current_directory()),
                          "case_sensitive": cache.use_case_sensitive_file_names() });
        caches.insert(name, cache);
        return Ok(row);
    }
    let name = bytes(action, "target")?;
    let cache = caches
        .get(&name)
        .ok_or_else(|| format!("no cache named {}", hex(&name)))?;
    Ok(match op {
        "dump" => json!({
            "op": op,
            "directories": directories(cache),
            "directories_by_realpath": by_realpath(cache.directories_by_realpath()),
            "files": files(cache),
            "files_by_realpath": by_realpath(cache.files_by_realpath()),
        }),
        "has_directory" => {
            let path = Path::from_bytes(bytes(action, "path")?);
            json!({ "op": op, "result": cache.has_directory(&path) })
        }
        "set_directory" => {
            cache.set_directory(
                &bytes(action, "symlink")?,
                Path::from_bytes(bytes(action, "symlink_path")?),
                link(action)?,
            );
            json!({ "op": op, "sizes": sizes(cache) })
        }
        "set_file" => {
            cache.set_file(
                &bytes(action, "symlink")?,
                Path::from_bytes(bytes(action, "symlink_path")?),
                &bytes(action, "realpath")?,
            );
            json!({ "op": op, "sizes": sizes(cache) })
        }
        "process_resolution" => {
            cache.process_resolution(&bytes(action, "original")?, &bytes(action, "resolved")?);
            json!({ "op": op, "sizes": sizes(cache) })
        }
        "set_symlinks_from_resolutions" => {
            let modules = pairs(action, "modules")?;
            let type_references = pairs(action, "type_references")?;
            let calls = std::cell::RefCell::new(Vec::new());
            // The pin hands each driver a nil source file.
            let drive = |entered: &str,
                         each: &str,
                         list: &[Resolution],
                         callback: &mut dyn FnMut(&[u8], &[u8])| {
                calls.borrow_mut().push(json!([entered, true, list.len()]));
                for (original, resolved) in list {
                    calls
                        .borrow_mut()
                        .push(json!([each, hex(original), hex(resolved)]));
                    callback(original, resolved);
                }
            };
            cache.set_symlinks_from_resolutions(
                |callback| drive("drive_modules", "module", &modules, callback),
                |callback| {
                    drive(
                        "drive_type_references",
                        "type_reference",
                        &type_references,
                        callback,
                    );
                },
            );
            json!({ "op": op, "calls": calls.into_inner(), "sizes": sizes(cache) })
        }
        "store_through_directories" => {
            cache.directories().store(
                Path::from_bytes(bytes(action, "symlink_path")?),
                link(action)?,
            );
            json!({ "op": op, "sizes": sizes(cache) })
        }
        "store_through_files" => {
            cache.files().store(
                Path::from_bytes(bytes(action, "symlink_path")?),
                bytes(action, "realpath")?,
            );
            json!({ "op": op, "sizes": sizes(cache) })
        }
        "store_through_by_realpath" => {
            let key = Path::from_bytes(bytes(action, "realpath")?);
            let index = match bytes(action, "index")?.as_slice() {
                b"directories" => cache.directories_by_realpath(),
                b"files" => cache.files_by_realpath(),
                _ => return Err("unknown reverse index".into()),
            };
            let (set, _) = index.load_or_store(key, SymlinkSet::default());
            set.insert(bytes(action, "symlink")?);
            json!({ "op": op, "sizes": sizes(cache) })
        }
        "guess_directory_symlink" => {
            let (resolved, original) = cache
                .guess_directory_symlink(
                    &bytes(action, "a")?,
                    &bytes(action, "b")?,
                    &bytes(action, "cwd")?,
                )
                .unwrap_or_default();
            json!({ "op": op, "common_resolved": hex(&resolved), "common_original": hex(&original) })
        }
        "is_node_modules_or_scoped_package_directory" => {
            let name = bytes(action, "name")?;
            json!({ "op": op, "result": cache.is_node_modules_or_scoped_package_directory(&name) })
        }
        other => return Err(format!("unsupported action {other:?}")),
    })
}

/// The entry counts of the four indexes: directories, directories by realpath,
/// files, files by realpath.
fn sizes(cache: &KnownSymlinks) -> Value {
    json!([
        cache.directories().len(),
        cache.directories_by_realpath().len(),
        cache.files().len(),
        cache.files_by_realpath().len(),
    ])
}
fn directories(cache: &KnownSymlinks) -> Value {
    let mut entries = Vec::new();
    cache.directories().range(|key, value| {
        let row = match value {
            Some(link) => json!([
                hex(key.as_bytes()),
                true,
                hex(&link.real),
                hex(link.real_path.as_bytes())
            ]),
            None => json!([hex(key.as_bytes()), false, "", ""]),
        };
        entries.push((key.as_bytes().to_vec(), row));
        true
    });
    sorted(entries)
}
fn files(cache: &KnownSymlinks) -> Value {
    let mut entries = Vec::new();
    cache.files().range(|key, value| {
        entries.push((
            key.as_bytes().to_vec(),
            json!([hex(key.as_bytes()), hex(&value)]),
        ));
        true
    });
    sorted(entries)
}
fn by_realpath(index: &tsr_core::collections::SyncMap<Path, SymlinkSet>) -> Value {
    let mut entries = Vec::new();
    index.range(|key, set| {
        let mut members = set.to_vec();
        members.sort();
        let members: Vec<String> = members.iter().map(|member| hex(member)).collect();
        entries.push((
            key.as_bytes().to_vec(),
            json!([hex(key.as_bytes()), members]),
        ));
        true
    });
    sorted(entries)
}
fn sorted(mut entries: Vec<(Vec<u8>, Value)>) -> Value {
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Value::Array(entries.into_iter().map(|(_, row)| row).collect())
}
/// `link_present: false` is the absent link and must carry empty strings.
fn link(action: &Value) -> Result<Option<KnownDirectoryLink>, String> {
    let (real, real_path) = (bytes(action, "real")?, bytes(action, "real_path")?);
    if !flag(action, "link_present")? {
        if !real.is_empty() || !real_path.is_empty() {
            return Err("an absent directory link must carry empty real and real_path".into());
        }
        return Ok(None);
    }
    Ok(Some(KnownDirectoryLink {
        real,
        real_path: Path::from_bytes(real_path),
    }))
}
/// An original path and the file name it resolved to.
type Resolution = (Vec<u8>, Vec<u8>);

fn pairs(action: &Value, key: &str) -> Result<Vec<Resolution>, String> {
    action
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("action key {key:?} is not an array"))?
        .iter()
        .map(|pair| {
            let text = |index: usize| {
                pair.get(index)
                    .and_then(Value::as_str)
                    .map(|value| value.as_bytes().to_vec())
                    .ok_or_else(|| format!("action key {key:?} holds a malformed pair"))
            };
            Ok((text(0)?, text(1)?))
        })
        .collect()
}
fn flag(action: &Value, key: &str) -> Result<bool, String> {
    action
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("missing boolean {key:?}"))
}
/// UTF-8 text under `key`, or hex under `key_hex`; exactly one of the two.
fn bytes(action: &Value, key: &str) -> Result<Vec<u8>, String> {
    match (action.get(key), action.get(format!("{key}_hex"))) {
        (Some(value), None) => value
            .as_str()
            .map(|value| value.as_bytes().to_vec())
            .ok_or_else(|| format!("action key {key:?} is not a string")),
        (None, Some(value)) => {
            let text = value
                .as_str()
                .ok_or_else(|| format!("action key {key}_hex is not a string"))?;
            (0..text.len())
                .step_by(2)
                .map(|i| {
                    u8::from_str_radix(text.get(i..i + 2).unwrap_or(""), 16)
                        .map_err(|_| format!("{key}_hex is malformed"))
                })
                .collect()
        }
        _ => Err(format!(
            "an action must carry exactly one of {key:?} and {key}_hex"
        )),
    }
}
fn hex(value: &[u8]) -> String {
    use std::fmt::Write;
    value.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").expect("writing to a String is infallible");
        out
    })
}
