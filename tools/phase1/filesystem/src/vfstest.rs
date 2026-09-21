//! The `vfs/vfstest` group: the in-memory test filesystem.
//!
//! Every action drives `tsr_vfs::vfstest` and `tsr_vfs::iofs`. The native probe
//! reaches the pinned internals by name, so this replay does too: the public
//! operations, the symbolic-link worker, the raw setters and the open-handle
//! behaviour each have their production counterpart. Nothing here emulates the
//! filesystem, and nothing reads an expected value.
//!
//! Two native shapes have no production form and are reported as themselves
//! rather than imitated. An input whose Go value is of a foreign dynamic type
//! cannot be written down against a typed input enum, so that one guarded row
//! reports `unrepresentable_input`. A view published by `get_file_info` never
//! changes under its holder, because stored entries are immutable here, where
//! the pin assigns a modification time through its stored pointer.

use std::{collections::BTreeMap, sync::Arc};

use serde_json::{json, Value};
use tsr_vfs::{
    iofs::{FileMode, Fs, Handle, IoError, MapFile, MapFs, Sys, Time},
    vfstest::{self, Clock, InputFile, TestFs},
};

use crate::api::{action_op, actions, ordered, subject, Outcome};

const CASE_PREFIX: &str = "filesystem/vfstest/";
const SYS_VALUE: i64 = 1234;

pub fn observe(request: &Value) -> Option<Outcome> {
    let case = request.get("case").and_then(Value::as_str).unwrap_or("");
    if !case.starts_with(CASE_PREFIX) {
        return None;
    }
    let rows = match subject(request) {
        "MapFS" => replay(actions(request)),
        "MapFSSnapshot" => snapshot_replay(actions(request)),
        other => Err(format!("case {case} has unknown subject {other}")),
    };
    Some(match rows {
        Ok(rows) => Outcome::Observed(ordered(rows)),
        Err(problem) => Outcome::Failed(problem),
    })
}

/// Starts at 2020-01-01T00:00:00Z and advances one second per reading.
struct StepClock(std::sync::Mutex<Time>);
impl StepClock {
    fn new() -> Arc<Self> {
        Arc::new(Self(std::sync::Mutex::new(Time::from_unix(
            1_577_836_800,
            0,
        ))))
    }
}
impl Clock for StepClock {
    fn now(&self) -> Time {
        let mut at = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *at = at.add_seconds(1);
        *at
    }
}

#[derive(Default)]
struct World {
    fs: Option<Arc<TestFs>>,
    handle: Option<(Handle, String)>,
}
impl World {
    fn fs(&self, op: &str) -> Result<Arc<TestFs>, String> {
        self.fs
            .clone()
            .ok_or_else(|| format!("action {op} ran before a filesystem was built"))
    }
}

fn replay(trace: &[Value]) -> Result<Vec<Value>, String> {
    let mut world = World::default();
    trace.iter().map(|action| row(&mut world, action)).collect()
}

#[allow(
    clippy::too_many_lines,
    reason = "one arm per pinned probe action, in the probe's order"
)]
fn row(world: &mut World, a: &Value) -> Result<Value, String> {
    let op = action_op(a);
    let mut out = json!({ "op": op });
    match op {
        "from_map" | "from_map_with_clock" => {
            let files =
                input(a)?.map_err(|kind| format!("input kind {kind} outside a guarded action"))?;
            let sensitive = flag(a, "case_sensitive")?;
            let built = if op == "from_map" {
                vfstest::from_map(&files, sensitive)
            } else {
                vfstest::from_map_with_clock(&files, sensitive, StepClock::new())
            };
            world.fs = Some(Arc::new(built));
            out["result"] = json!(["built", files_len(a)]);
        }
        "from_empty_map" => {
            world.fs = Some(Arc::new(vfstest::from_map(
                &BTreeMap::new(),
                flag(a, "case_sensitive")?,
            )));
            out["result"] = json!(["built", 0]);
        }
        "from_map_guarded" => {
            let sensitive = flag(a, "case_sensitive")?;
            let panicked = match input(a)? {
                Ok(files) => {
                    guarded(move || {
                        vfstest::from_map(&files, sensitive);
                        Value::Null
                    })
                    .1
                }
                Err(_) => "unrepresentable_input".to_string(),
            };
            out["result"] = json!(["guarded", panicked]);
        }
        "convert_map_fs" => {
            let built = vfstest::convert_map_fs(
                &raw_input(a)?,
                flag(a, "case_sensitive")?,
                Some(StepClock::new()),
            );
            world.fs = Some(Arc::new(built));
            out["result"] = json!(["built", files_len(a)]);
        }
        "convert_map_fs_guarded" => {
            let (raw, sensitive) = (raw_input(a)?, flag(a, "case_sensitive")?);
            let (_, panicked) = guarded(move || {
                vfstest::convert_map_fs(&raw, sensitive, Some(StepClock::new()));
                Value::Null
            });
            out["result"] = json!(["guarded", panicked]);
        }
        "entries" | "entries_with_times" | "entries_break" => {
            let fs = world.fs(op)?;
            let stop = if op == "entries_break" {
                Some(int(a, "stop")?)
            } else {
                None
            };
            let with_times = op == "entries_with_times";
            let (value, panicked) =
                guarded(move || json!(["entries", entries(&fs, stop, with_times)]));
            out["result"] = value;
            out["panic"] = json!(panicked);
        }
        "get_file_info" => {
            let path = need(a, "path")?;
            out["result"] = match world.fs(op)?.get_file_info(path.as_bytes()) {
                None => json!(["absent"]),
                Some(file) => json!([
                    "info",
                    mode(file.mode),
                    file.data.len(),
                    text(&file.data),
                    stored(&file)
                ]),
            };
            out["path"] = json!(path);
        }
        "get_mod_time" => {
            let path = need(a, "path")?;
            let stamp = world.fs(op)?.get_mod_time(path.as_bytes());
            out["result"] = if stamp.is_zero() {
                json!(["zero", ""])
            } else {
                json!(["stamp", stamp.format_rfc3339_nano()])
            };
            out["path"] = json!(path);
        }
        "mod_time_relation" => {
            let (path, other) = (need(a, "path")?, need(a, "other")?);
            let fs = world.fs(op)?;
            let (left, right) = (
                fs.get_mod_time(path.as_bytes()),
                fs.get_mod_time(other.as_bytes()),
            );
            let relation = if left.is_zero() || right.is_zero() {
                "missing"
            } else if left.after(right) {
                "after"
            } else {
                "not_after"
            };
            out["result"] = json!(["relation", relation]);
            out["path"] = json!(path);
            out["other"] = json!(other);
        }
        "get_target_of_symlink" => {
            let path = need(a, "path")?;
            let target = world.fs(op)?.get_target_of_symlink(path.as_bytes());
            out["result"] = json!([
                "target",
                text(&target.clone().unwrap_or_default()),
                target.is_some()
            ]);
            out["path"] = json!(path);
        }
        "realpath" => {
            let path = need(a, "path")?;
            out["result"] = match world.fs(op)?.realpath(key(&path)) {
                Ok(value) => json!(["ok", text(&value)]),
                Err(error) => prefixed("err", &error),
            };
            out["path"] = json!(path);
        }
        "resolve" | "resolve_worker" | "is_broken_symlink" => {
            let path = need(a, "path")?;
            let fs = world.fs(op)?;
            let canonical = fs.canonical(key(&path));
            let resolved = if op == "resolve_worker" {
                fs.get_following_symlinks_worker(
                    &canonical,
                    field(a, "from").as_bytes(),
                    field(a, "to").as_bytes(),
                )
            } else {
                fs.get_following_symlinks(&canonical)
            };
            out["result"] = if op == "is_broken_symlink" {
                let error = resolved.err().map(|(_, error)| error);
                json!([
                    "broken",
                    error.as_ref().is_some_and(IoError::is_broken_symlink),
                    error.as_ref().is_some_and(IoError::is_not_exist)
                ])
            } else {
                match resolved {
                    Ok((file, canonical)) => {
                        json!(["ok", text(&canonical), mode(file.mode), text(&file.data)])
                    }
                    Err((canonical, error)) => {
                        let mut row = vec![json!("err"), json!(text(&canonical))];
                        row.extend(err(&error));
                        Value::Array(row)
                    }
                }
            };
            out["path"] = json!(path);
            if op == "resolve_worker" {
                out["from"] = json!(field(a, "from"));
                out["to"] = json!(field(a, "to"));
            }
        }
        "canonical" => {
            let path = field(a, "path");
            out["result"] = json!(["canonical", text(&world.fs(op)?.canonical(path.as_bytes()))]);
            out["path"] = json!(path);
        }
        "compare_paths" => {
            let (left, right) = (need(a, "path")?, need(a, "other")?);
            let by_parts = vfstest::compare_paths_by_parts(left.as_bytes(), right.as_bytes()) as i8;
            out["result"] = json!([
                "compare",
                by_parts,
                left.as_bytes().cmp(right.as_bytes()) as i8
            ]);
            out["path"] = json!(left);
            out["other"] = json!(right);
        }
        "split_path" => {
            let (path, offset) = (field(a, "path"), int(a, "offset")?);
            let at = usize::try_from(offset).map_err(|_| "negative offset".to_string())?;
            let (before, after) = vfstest::split_path(path.as_bytes(), at);
            out["result"] = json!(["split", text(before), text(after)]);
            out["path"] = json!(path);
            out["offset"] = json!(offset);
        }
        "dir_name" | "base_name" => {
            let path = field(a, "path");
            let value = if op == "dir_name" {
                vfstest::dir_name(path.as_bytes())
            } else {
                vfstest::base_name(path.as_bytes())
            };
            out["result"] = json!([op, text(value)]);
            out["path"] = json!(path);
        }
        "symlink_value" => {
            let target = need(a, "target")?;
            let value = vfstest::symlink(target.as_bytes());
            out["result"] = json!([
                "symlink",
                mode(value.mode),
                text(&value.data),
                value.mod_time.is_zero(),
                sys(&value.sys)
            ]);
            out["target"] = json!(target);
        }
        "write_file" | "append_file" => {
            let (path, perm) = (need(a, "path")?, perm(a)?);
            let fs = world.fs(op)?;
            let data = field(a, "data");
            let result = if op == "write_file" {
                fs.write_file(key(&path), data.as_bytes(), perm)
            } else {
                fs.append_file(key(&path), data.as_bytes(), perm)
            };
            out["result"] = Value::Array(result.err().map_or_else(none, |error| err(&error)));
            out["path"] = json!(path);
        }
        "remove" | "remove_inner" => {
            let path = need(a, "path")?;
            let fs = world.fs(op)?;
            let result = if op == "remove" {
                fs.remove(key(&path))
            } else {
                fs.remove_canonical(key(&path));
                Ok(())
            };
            out["result"] = Value::Array(result.err().map_or_else(none, |error| err(&error)));
            out["path"] = json!(path);
        }
        "mkdir_all" | "mkdir_all_inner" => {
            let (path, perm) = (need(a, "path")?, perm(a)?);
            let fs = world.fs(op)?;
            let target = key(&path).to_vec();
            let (value, panicked) = guarded(move || {
                Value::Array(
                    fs.mkdir_all(&target, perm)
                        .err()
                        .map_or_else(none, |error| err(&error)),
                )
            });
            out["result"] = value;
            out["panic"] = json!(panicked);
            out["path"] = json!(path);
        }
        "chtimes" => {
            let path = need(a, "path")?;
            let result =
                world
                    .fs(op)?
                    .chtimes(path.as_bytes(), time(a, "a_time")?, time(a, "m_time")?);
            out["result"] = Value::Array(result.err().map_or_else(none, |error| err(&error)));
            out["path"] = json!(path);
        }
        "add_symlink" => {
            let (path, target) = (need(a, "path")?, need(a, "target")?);
            let fs = world.fs(op)?;
            let (link, to) = (key(&path).to_vec(), target.clone().into_bytes());
            let (_, panicked) = guarded(move || {
                fs.add_symlink(&link, &to);
                Value::Null
            });
            out["result"] = json!(["guarded", panicked]);
            out["path"] = json!(path);
            out["target"] = json!(target);
        }
        "set_entry" => {
            let (realpath, canonical) = (field(a, "realpath"), field(a, "canonical"));
            let mut file = raw_file(&need(a, "kind")?, field(a, "data").as_bytes())?;
            if flag(a, "has_sys")? {
                file.sys = Sys::Int(SYS_VALUE);
            }
            let fs = world.fs(op)?;
            let (spelled, key) = (
                realpath.clone().into_bytes(),
                canonical.clone().into_bytes(),
            );
            let (_, panicked) = guarded(move || {
                fs.set_entry_public(&spelled, &key, file);
                Value::Null
            });
            out["result"] = json!(["guarded", panicked]);
            out["realpath"] = json!(realpath);
            out["canonical"] = json!(canonical);
        }
        "set_raw" => {
            let canonical = need(a, "canonical")?;
            let file = raw_file(&need(a, "kind")?, field(a, "data").as_bytes())?;
            world.fs(op)?.set(canonical.as_bytes(), file);
            out["result"] = json!(["set", canonical]);
            out["canonical"] = json!(canonical);
        }
        "open" => {
            let path = need(a, "path")?;
            let fs = world.fs(op)?;
            let name = key(&path).to_vec();
            let slot = Arc::new(std::sync::Mutex::new(None));
            let keep = slot.clone();
            let (value, panicked) = guarded(move || match fs.open(&name) {
                Err(error) => prefixed("err", &error),
                Ok(handle) => {
                    let info = handle.info.clone();
                    let directory = handle.is_read_dir_file();
                    *keep
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handle);
                    json!([
                        "open",
                        text(&info.name),
                        info.is_dir(),
                        info.size,
                        sys(&info.sys),
                        directory
                    ])
                }
            });
            if let Some(handle) = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                world.handle = Some((handle, path.clone()));
            }
            out["result"] = value;
            out["panic"] = json!(panicked);
            out["path"] = json!(path);
        }
        "open_inner" => {
            let canonical = need(a, "canonical")?;
            let fs = world.fs(op)?;
            let name = canonical.clone().into_bytes();
            let (value, panicked) = guarded(move || match fs.open_canonical(&name) {
                Err(error) => prefixed("err", &error),
                Ok(handle) => json!([
                    "open",
                    text(&handle.info.name),
                    handle.info.is_dir(),
                    vfstest::convert_info(&handle.info).is_some()
                ]),
            });
            out["result"] = value;
            out["panic"] = json!(panicked);
            out["canonical"] = json!(canonical);
        }
        "handle_stat" => {
            let (handle, path) = world
                .handle
                .as_ref()
                .ok_or("action handle_stat ran with no open handle")?;
            let (first, second) = (handle.info.clone(), handle.info.clone());
            out["result"] = json!([
                "stat",
                text(&first.name),
                first.is_dir(),
                sys(&first.sys),
                Arc::ptr_eq(&first, &second)
            ]);
            out["panic"] = json!("");
            out["path"] = json!(path);
        }
        "read_dir" => {
            let count = int(a, "n")?;
            let (handle, _) = world
                .handle
                .as_mut()
                .ok_or("action read_dir ran with no open handle")?;
            if !handle.is_read_dir_file() {
                return Err("read_dir on a handle that is not a directory".into());
            }
            let count_arg = isize::try_from(count).map_err(|_| "n out of range")?;
            let (value, panicked) = guarded(|| match handle.read_dir(count_arg) {
                Err(error) => prefixed("err", &error),
                Ok(list) => {
                    let names: Vec<Value> = list
                        .iter()
                        .map(|entry| json!([text(&entry.name), entry.is_dir(), sys(&entry.sys)]))
                        .collect();
                    json!(["entries", names])
                }
            });
            out["result"] = value;
            out["panic"] = json!(panicked);
            out["n"] = json!(count);
        }
        "convert_info_raw" => {
            let path = need(a, "path")?;
            let raw = raw_input(a)?;
            let name = key(&path).to_vec();
            let (value, panicked) = guarded(move || match raw.open(&name) {
                Err(error) => prefixed("err", &error),
                Ok(handle) => json!([
                    "convert",
                    text(&handle.info.name),
                    handle.info.is_dir(),
                    vfstest::convert_info(&handle.info).is_some()
                ]),
            });
            out["result"] = value;
            out["panic"] = json!(panicked);
            out["path"] = json!(path);
        }
        other => return Err(format!("unsupported action {other:?}")),
    }
    Ok(out)
}

/// A view is the entries `get_file_info` handed back, held by the caller, with
/// the stamp each had when it was published.
struct View {
    paths: Vec<String>,
    held: BTreeMap<String, Option<Arc<MapFile>>>,
    stamps: BTreeMap<String, Value>,
}
fn stamp(file: Option<&Arc<MapFile>>) -> Value {
    match file {
        None => json!(["absent", "", false, ""]),
        Some(file) => json!([
            "present",
            text(&file.data),
            file.mode.is_dir(),
            file.mod_time.format_rfc3339_nano()
        ]),
    }
}
fn snapshot_replay(trace: &[Value]) -> Result<Vec<Value>, String> {
    let mut fs: Option<TestFs> = None;
    let mut views: BTreeMap<String, View> = BTreeMap::new();
    let mut rows = Vec::new();
    for a in trace {
        let op = action_op(a);
        let mut out = json!({ "op": op });
        let live = |fs: &Option<TestFs>| {
            fs.as_ref()
                .map(|_| ())
                .ok_or_else(|| format!("{op} ran before build"))
        };
        match op {
            "build" => {
                let files =
                    input(a)?.map_err(|kind| format!("input kind {kind} in a snapshot build"))?;
                fs = Some(vfstest::from_map_with_clock(
                    &files,
                    flag(a, "case_sensitive")?,
                    StepClock::new(),
                ));
                out["result"] = json!(["built", files_len(a)]);
            }
            "publish" => {
                live(&fs)?;
                let (name, paths) = (need(a, "name")?, strings(a, "paths")?);
                if paths.is_empty() {
                    return Err("publish needs the paths it publishes".into());
                }
                let mut view = View {
                    paths: paths.clone(),
                    held: BTreeMap::new(),
                    stamps: BTreeMap::new(),
                };
                let mut contents = Vec::new();
                for path in &paths {
                    let file = fs.as_ref().and_then(|fs| fs.get_file_info(path.as_bytes()));
                    contents.push(match &file {
                        None => json!([path, "absent", ""]),
                        Some(file) => json!([path, "present", text(&file.data)]),
                    });
                    view.stamps.insert(path.clone(), stamp(file.as_ref()));
                    view.held.insert(path.clone(), file);
                }
                views.insert(name.clone(), view);
                out["result"] = json!(["published", name, contents]);
                out["name"] = json!(name);
            }
            "mutate_write" => {
                live(&fs)?;
                let (path, perm) = (need(a, "path")?, perm(a)?);
                let result = fs
                    .as_ref()
                    .map(|fs| fs.write_file(key(&path), field(a, "data").as_bytes(), perm));
                out["result"] = Value::Array(
                    result
                        .and_then(Result::err)
                        .map_or_else(none, |error| err(&error)),
                );
                out["path"] = json!(path);
            }
            "mutate_chtimes" => {
                live(&fs)?;
                let path = need(a, "path")?;
                let (a_time, m_time) = (time(a, "a_time")?, time(a, "m_time")?);
                let result = fs
                    .as_ref()
                    .map(|fs| fs.chtimes(path.as_bytes(), a_time, m_time));
                out["result"] = Value::Array(
                    result
                        .and_then(Result::err)
                        .map_or_else(none, |error| err(&error)),
                );
                out["path"] = json!(path);
            }
            "read_view" => {
                let (name, path) = (need(a, "name")?, need(a, "path")?);
                let view = views
                    .get(&name)
                    .ok_or_else(|| format!("unpublished view {name}"))?;
                let file = view
                    .held
                    .get(&path)
                    .ok_or_else(|| format!("view never published {path}"))?;
                out["result"] = file.as_ref().map_or_else(
                    || json!(["absent", ""]),
                    |file| json!(["bytes", text(&file.data)]),
                );
                out["name"] = json!(name);
                out["path"] = json!(path);
            }
            "read_live" => {
                live(&fs)?;
                let path = need(a, "path")?;
                let file = fs.as_ref().and_then(|fs| fs.get_file_info(path.as_bytes()));
                out["result"] = file.map_or_else(
                    || json!(["absent", ""]),
                    |file| json!(["bytes", text(&file.data)]),
                );
                out["path"] = json!(path);
            }
            "view_unchanged" => {
                let name = need(a, "name")?;
                let view = views
                    .get(&name)
                    .ok_or_else(|| format!("unpublished view {name}"))?;
                let unchanged = view
                    .paths
                    .iter()
                    .all(|path| stamp(view.held[path].as_ref()) == view.stamps[path]);
                out["result"] = json!(["unchanged", unchanged]);
                out["name"] = json!(name);
            }
            other => return Err(format!("unsupported action {other:?}")),
        }
        rows.push(out);
    }
    Ok(rows)
}

fn entries(fs: &TestFs, stop: Option<i64>, with_times: bool) -> Vec<Value> {
    let mut out = Vec::new();
    for (path, file) in fs.entries() {
        if stop.is_some_and(|stop| stop >= 0 && out.len() as i64 >= stop) {
            break;
        }
        let mut row = vec![
            json!(text(&path)),
            mode(file.mode),
            json!(file.data.len()),
            json!(text(&file.data)),
        ];
        if with_times {
            row.push(json!(file.mod_time.format_rfc3339_nano()));
        }
        out.push(Value::Array(row));
    }
    out
}
fn mode(mode: FileMode) -> Value {
    json!([
        mode.is_dir(),
        mode.is_symlink(),
        mode.is_regular(),
        format!("{:04o}", mode.perm())
    ])
}
fn sys(value: &Sys) -> Value {
    match value {
        Sys::Nil => json!(["nil", 0]),
        Sys::Int(value) => json!(["int", value]),
        Sys::Wrapper { .. } => json!(["wrapper", 0]),
    }
}
fn stored(file: &MapFile) -> Value {
    match &file.sys {
        Sys::Wrapper { original, realpath } => json!(["wrapped", text(realpath), sys(original)]),
        other => json!(["bare", "", sys(other)]),
    }
}
fn none() -> Vec<Value> {
    vec![json!("none"), json!("")]
}
/// The probe's error vocabulary, in its order of tests.
fn err(error: &IoError) -> Vec<Value> {
    if error.is_broken_symlink() {
        return vec![json!("broken_symlink"), json!(error.to_string())];
    }
    if let IoError::Path { op, path, source } = error {
        let inner = if source.is_not_exist() {
            "not_exist"
        } else {
            "other"
        };
        return vec![
            json!("path_error"),
            json!(format!("{op} {}", text(path))),
            json!(inner),
        ];
    }
    if error.is_not_exist() {
        return vec![json!("not_exist"), json!(sentence(&error.to_string()))];
    }
    if *error == IoError::Eof {
        return vec![json!("eof"), json!("")];
    }
    vec![json!("pinned"), json!(sentence(&error.to_string()))]
}
fn prefixed(tag: &str, error: &IoError) -> Value {
    let mut row = vec![json!(tag)];
    row.extend(err(error));
    Value::Array(row)
}
fn sentence(text: &str) -> String {
    text.replace("file does not exist", "<ErrNotExist>")
}
/// The filesystem's locks recover from poisoning, so observing it after a
/// guarded panic is sound.
fn guarded(operation: impl FnOnce() -> Value) -> (Value, String) {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation));
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
            } else if message.contains("interface conversion") {
                "interface_conversion".to_string()
            } else if message.contains("stack overflow") {
                "stack_overflow".to_string()
            } else {
                format!("pinned:{}", sentence(&message))
            };
            (Value::Null, class)
        }
    }
}
/// The map a caller hands `from_map`. `Err` names a Go value kind that a typed
/// input cannot carry.
fn input(a: &Value) -> Result<Result<BTreeMap<Vec<u8>, InputFile>, String>, String> {
    let mut out = BTreeMap::new();
    for (path, kind, data) in file_rows(a)? {
        let file = match kind.as_str() {
            "string" => InputFile::Text(data.into_bytes()),
            "bytes" => InputFile::Bytes(data.into_bytes()),
            "mapfile" => InputFile::File(MapFile {
                data: data.as_bytes().into(),
                ..MapFile::default()
            }),
            "mapfile_sys" => InputFile::File(MapFile {
                data: data.as_bytes().into(),
                sys: Sys::Int(SYS_VALUE),
                ..MapFile::default()
            }),
            "symlink" => InputFile::File(vfstest::symlink(data.as_bytes())),
            "invalid" => return Ok(Err(kind)),
            other => return Err(format!("unknown file kind: {other}")),
        };
        out.insert(path.into_bytes(), file);
    }
    Ok(Ok(out))
}
fn raw_input(a: &Value) -> Result<MapFs, String> {
    let mut out = BTreeMap::new();
    for (path, kind, data) in file_rows(a)? {
        out.insert(
            path.into_bytes(),
            Arc::new(raw_file(&kind, data.as_bytes())?),
        );
    }
    Ok(MapFs(out))
}
fn raw_file(kind: &str, data: &[u8]) -> Result<MapFile, String> {
    Ok(match kind {
        "file" => MapFile {
            data: data.into(),
            ..MapFile::default()
        },
        "file_sys" => MapFile {
            data: data.into(),
            sys: Sys::Int(SYS_VALUE),
            ..MapFile::default()
        },
        "symlink" => vfstest::symlink(data),
        "dir" => MapFile {
            mode: FileMode::DIR | FileMode(0o755),
            ..MapFile::default()
        },
        other => return Err(format!("unknown raw file kind: {other}")),
    })
}
fn file_rows(a: &Value) -> Result<Vec<(String, String, String)>, String> {
    let rows = a
        .get("files")
        .and_then(Value::as_array)
        .filter(|rows| !rows.is_empty())
        .ok_or("action needs a files list")?;
    rows.iter()
        .map(|row| {
            let cell = |index: usize| {
                row.get(index)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or("files rows are [path, kind, data]")
            };
            Ok((cell(0)?, cell(1)?, cell(2)?))
        })
        .collect::<Result<_, &str>>()
        .map_err(str::to_owned)
}
fn files_len(a: &Value) -> usize {
    a.get("files").and_then(Value::as_array).map_or(0, Vec::len)
}
fn key(path: &str) -> &[u8] {
    path.strip_prefix('/').unwrap_or(path).as_bytes()
}
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
fn field(a: &Value, name: &str) -> String {
    a.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
fn need(a: &Value, name: &str) -> Result<String, String> {
    Some(field(a, name))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("action needs a non-empty {name}"))
}
fn flag(a: &Value, name: &str) -> Result<bool, String> {
    a.get(name)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("action needs {name}"))
}
fn int(a: &Value, name: &str) -> Result<i64, String> {
    a.get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("action needs {name}"))
}
fn strings(a: &Value, name: &str) -> Result<Vec<String>, String> {
    a.get(name)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .ok_or_else(|| format!("action needs {name}"))
}
fn perm(a: &Value) -> Result<u32, String> {
    u32::from_str_radix(&need(a, "perm")?, 8).map_err(|_| "unreadable octal perm".to_string())
}
fn time(a: &Value, name: &str) -> Result<Time, String> {
    Time::parse_rfc3339(&need(a, name)?).ok_or_else(|| format!("unreadable {name}"))
}
