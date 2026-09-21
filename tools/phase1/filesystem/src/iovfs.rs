//! The `vfs/iovfs` group: the compiler filesystem over an `io/fs` backing, and
//! the root dispatcher beneath it.
//!
//! Every action drives `tsr_vfs::iovfs`. The native probe hands the adapter
//! five kinds of backing to show which capability selects which behaviour; this
//! replay builds the same five over the production `MapFs` and `TestFs`, with
//! the spy recording what reached it. Nothing here emulates the adapter.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tsr_vfs::{
    iofs::{self, FileMode, Fs, Handle, IoError, MapFile, MapFs, Time, WalkError},
    iovfs::{self, Backing, IoVfs, RealpathFs, WritableFs},
    vfstest::{self, Clock, InputFile, TestFs},
    Entries,
};

use crate::api::{action_op, actions, ordered, subject, Outcome};

const SUBJECTS: &[&str] = &[
    "iovfs.From",
    "iovfs.Read",
    "iovfs.Entries",
    "iovfs.Realpath",
    "iovfs.Walk",
    "iovfs.Mutate",
    "iovfs.Identity",
];
const WALK_STOP: &str = "phase1: walk callback stop";
const SPY_MKDIR_REFUSED: &str = "phase1: spy refused the mkdir";

pub fn observe(request: &Value) -> Option<Outcome> {
    if !SUBJECTS.contains(&subject(request)) {
        return None;
    }
    let mut state = None;
    let rows: Result<Vec<Value>, String> = actions(request)
        .iter()
        .map(|a| row(&mut state, a))
        .collect();
    Some(match rows {
        Ok(rows) => Outcome::Observed(ordered(rows)),
        Err(problem) => Outcome::Failed(problem),
    })
}

/// 2023-11-14T22:13:20Z plus one second per reading.
struct FixedClock(Mutex<i64>);
impl Clock for FixedClock {
    fn now(&self) -> Time {
        let mut ticks = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *ticks += 1;
        Time::from_unix(1_700_000_000 + *ticks, 0)
    }
}

#[derive(Default)]
struct Spy {
    calls: Vec<Value>,
    write_attempts: i64,
    fail_writes: i64,
    fail_mkdir: bool,
}
/// One backing type for the five native kinds: what it serves, and which of the
/// two optional capabilities it declares.
struct Driven {
    files: Served,
    resolves: bool,
    spy: Option<Mutex<Spy>>,
}
enum Served {
    Plain(MapFs),
    Test(TestFs),
}
impl Fs for Driven {
    fn open(&self, name: &[u8]) -> Result<Handle, IoError> {
        match &self.files {
            Served::Plain(fs) => fs.open(name),
            Served::Test(fs) => fs.open(name),
        }
    }
}
impl Backing for Driven {
    fn as_realpath(&self) -> Option<&dyn RealpathFs> {
        (self.resolves || matches!(self.files, Served::Test(_))).then_some(self as &dyn RealpathFs)
    }
    fn as_writable(&self) -> Option<&dyn WritableFs> {
        (self.spy.is_some() || matches!(self.files, Served::Test(_)))
            .then_some(self as &dyn WritableFs)
    }
}
impl RealpathFs for Driven {
    fn realpath(&self, p: &[u8]) -> Result<Vec<u8>, IoError> {
        if let Served::Test(fs) = &self.files {
            return fs.realpath(p);
        }
        if p.windows(4).any(|window| window == b"boom") {
            return Err(IoError::NotExist);
        }
        Ok([b"RP<".as_slice(), p, b">"].concat())
    }
}
impl Driven {
    fn spy(&self) -> std::sync::MutexGuard<'_, Spy> {
        self.spy
            .as_ref()
            .expect("a spy backing")
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn refuse_write(&self) -> Result<(), IoError> {
        let mut spy = self.spy();
        spy.write_attempts += 1;
        if spy.fail_writes > 0 {
            spy.fail_writes -= 1;
            return Err(IoError::message(format!(
                "phase1: spy refused write {}: phase1: spy refused the write",
                spy.write_attempts
            )));
        }
        Ok(())
    }
    fn record(&self, row: Value) {
        self.spy().calls.push(row);
    }
}
impl WritableFs for Driven {
    fn write_file(&self, p: &[u8], data: &[u8], perm: u32) -> Result<(), IoError> {
        if let Served::Test(fs) = &self.files {
            return fs.write_file(p, data, perm);
        }
        let result = self.refuse_write();
        self.record(json!([
            "WriteFile",
            text(p),
            hex(data),
            perm_octal(perm),
            error_class(result.as_ref().err())
        ]));
        result
    }
    fn append_file(&self, p: &[u8], data: &[u8], perm: u32) -> Result<(), IoError> {
        if let Served::Test(fs) = &self.files {
            return fs.append_file(p, data, perm);
        }
        let result = self.refuse_write();
        self.record(json!([
            "AppendFile",
            text(p),
            hex(data),
            perm_octal(perm),
            error_class(result.as_ref().err())
        ]));
        result
    }
    fn mkdir_all(&self, p: &[u8], perm: u32) -> Result<(), IoError> {
        if let Served::Test(fs) = &self.files {
            return fs.mkdir_all(p, perm);
        }
        let result = if self.spy().fail_mkdir {
            Err(IoError::message(SPY_MKDIR_REFUSED))
        } else {
            Ok(())
        };
        self.record(json!([
            "MkdirAll",
            text(p),
            "",
            perm_octal(perm),
            error_class(result.as_ref().err())
        ]));
        result
    }
    fn remove(&self, p: &[u8]) -> Result<(), IoError> {
        if let Served::Test(fs) = &self.files {
            return fs.remove(p);
        }
        self.record(json!(["Remove", text(p), "", "", ""]));
        Ok(())
    }
    fn chtimes(&self, p: &[u8], a_time: Time, m_time: Time) -> Result<(), IoError> {
        if let Served::Test(fs) = &self.files {
            return fs.chtimes(p, a_time, m_time);
        }
        self.record(json!([
            "Chtimes",
            text(p),
            a_time.format_rfc3339_nano(),
            m_time.format_rfc3339_nano(),
            ""
        ]));
        Ok(())
    }
}

struct State {
    kind: String,
    adapter: Arc<IoVfs<Driven>>,
    handed: Arc<Driven>,
    pointer_backing: bool,
}

fn build(a: &Value) -> Result<State, String> {
    let kind = string(a, "backing")?;
    let sensitive = a
        .get("case_sensitive")
        .and_then(Value::as_bool)
        .ok_or("new_fs requires case_sensitive")?;
    let files = a
        .get("files")
        .and_then(Value::as_array)
        .ok_or("new_fs requires files")?;
    let spy = || {
        Some(Mutex::new(Spy {
            fail_writes: a.get("fail_writes").and_then(Value::as_i64).unwrap_or(0),
            fail_mkdir: a
                .get("fail_mkdir")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            ..Spy::default()
        }))
    };
    let driven = match kind.as_str() {
        "fstest_map" => Driven {
            files: Served::Plain(plain_map(files)?),
            resolves: false,
            spy: None,
        },
        "realpath_only" => Driven {
            files: Served::Plain(plain_map(files)?),
            resolves: true,
            spy: None,
        },
        "writable_only" => Driven {
            files: Served::Plain(plain_map(files)?),
            resolves: false,
            spy: spy(),
        },
        "spy_realpath" => Driven {
            files: Served::Plain(plain_map(files)?),
            resolves: true,
            spy: spy(),
        },
        "vfstest_map" => {
            let mut seeded = std::collections::BTreeMap::new();
            for entry in files {
                let (path, file) = seed_file(entry)?;
                seeded.insert(path, InputFile::File(file));
            }
            let fs = vfstest::from_map_with_clock(
                &seeded,
                sensitive,
                Arc::new(FixedClock(Mutex::new(0))),
            );
            Driven {
                files: Served::Test(fs),
                resolves: true,
                spy: None,
            }
        }
        other => return Err(format!("unsupported backing: {other}")),
    };
    let handed = Arc::new(driven);
    Ok(State {
        pointer_backing: kind != "fstest_map",
        kind,
        adapter: Arc::new(iovfs::from(handed.clone(), sensitive)),
        handed,
    })
}

fn row(state: &mut Option<State>, a: &Value) -> Result<Value, String> {
    let op = action_op(a);
    if op == "new_fs" {
        let built = build(a)?;
        let result = json!([
            built.kind,
            built.handed.as_realpath().is_some(),
            built.handed.as_writable().is_some(),
            built.adapter.use_case_sensitive_file_names()
        ]);
        *state = Some(built);
        return Ok(json!({ "op": op, "result": result }));
    }
    let s = state
        .as_ref()
        .ok_or_else(|| format!("action {op} ran before new_fs"))?;
    let adapter = s.adapter.clone();
    let mut out = json!({ "op": op });
    let path = a.get("path").and_then(Value::as_str).map(str::to_owned);
    let need_path = || {
        path.clone()
            .ok_or_else(|| format!("action {op} requires path"))
    };
    let (value, panicked) = match op {
        "use_case_sensitive_file_names" => {
            guarded(move || json!([adapter.use_case_sensitive_file_names()]))
        }
        "file_exists" => {
            let p = need_path()?;
            guarded(move || json!([adapter.file_exists(p.as_bytes())]))
        }
        "directory_exists" => {
            let p = need_path()?;
            guarded(move || json!([adapter.directory_exists(p.as_bytes())]))
        }
        "stat" => {
            let p = need_path()?;
            guarded(move || match adapter.stat(p.as_bytes()) {
                None => json!([false]),
                Some(info) => json!([
                    true,
                    text(&info.name),
                    info.size,
                    info.is_dir(),
                    mode_class(info.mode),
                    perm_octal(info.mode.perm()),
                    info.mod_time.format_rfc3339_nano()
                ]),
            })
        }
        "read_file" => {
            let p = need_path()?;
            guarded(move || {
                let contents = adapter.read_file(p.as_bytes());
                let bytes = contents.clone().unwrap_or_default();
                json!([contents.is_some(), hex(&bytes), bytes.len()])
            })
        }
        "entries" => {
            let p = need_path()?;
            guarded(move || entries_row(&adapter.get_accessible_entries(p.as_bytes())))
        }
        "realpath" => {
            let p = need_path()?;
            guarded(move || json!([text(&adapter.realpath(p.as_bytes()))]))
        }
        "walk" => {
            let root = string(a, "root")?;
            let control = walk_decisions(a)?;
            out["root"] = json!(root);
            guarded(move || walk_once(&adapter, root.as_bytes(), &control))
        }
        "write_file" | "append_file" => {
            let (p, content) = (need_path()?, string(a, "content")?);
            let append = op == "append_file";
            guarded(move || {
                let result = if append {
                    adapter.append_file(p.as_bytes(), content.as_bytes())
                } else {
                    adapter.write_file(p.as_bytes(), content.as_bytes())
                };
                json!([error_class(result.as_ref().err())])
            })
        }
        "remove" => {
            let p = need_path()?;
            guarded(move || json!([error_class(adapter.remove(p.as_bytes()).as_ref().err())]))
        }
        "chtimes" => {
            let p = need_path()?;
            let (a_time, m_time) = (time(a, "atime")?, time(a, "mtime")?);
            guarded(move || {
                json!([error_class(
                    adapter.chtimes(p.as_bytes(), a_time, m_time).as_ref().err()
                )])
            })
        }
        // The backing handed to `from` is never itself an adapter.
        "fsys_kind" => (json!([s.kind, false]), Value::Null),
        "fsys_identity" => {
            let p = need_path()?;
            let (handed, comparable) = (s.handed.clone(), s.pointer_backing);
            guarded(move || {
                let got = adapter.fsys().clone();
                let reached = match iofs::stat(&*got, p.as_bytes()) {
                    Ok(info) => json!(["", text(&info.name), info.size, mode_class(info.mode)]),
                    Err(error) => json!([error_class(Some(&error))]),
                };
                if comparable {
                    json!(["comparable", Arc::ptr_eq(&got, &handed), reached])
                } else {
                    json!(["not_comparable", reached])
                }
            })
        }
        "fsys_read" => {
            let p = need_path()?;
            guarded(
                move || match iofs::read_file(&**adapter.fsys(), p.as_bytes()) {
                    Ok(data) => json!(["", hex(&data), data.len()]),
                    Err(error) => json!([error_class(Some(&error)), "", 0]),
                },
            )
        }
        "backing_mod_time" => {
            let p = need_path()?;
            let Served::Test(fs) = &s.handed.files else {
                return Err(format!(
                    "action {op} needs a vfstest backing, got {}",
                    s.kind
                ));
            };
            (
                json!([fs.get_mod_time(p.as_bytes()).format_rfc3339_nano()]),
                Value::Null,
            )
        }
        "spy_log" => {
            if s.handed.spy.is_none() {
                return Err(format!("action {op} needs a spy backing, got {}", s.kind));
            }
            let calls = std::mem::take(&mut s.handed.spy().calls);
            return Ok(json!({ "op": op, "result": calls }));
        }
        other => return Err(format!("unsupported action: {other}")),
    };
    if let Some(p) = path.filter(|_| op != "walk") {
        out["path"] = json!(p);
    }
    out["result"] = value;
    out["panic"] = panicked;
    Ok(out)
}

fn walk_once(adapter: &IoVfs<Driven>, root: &[u8], control: &[(String, String)]) -> Value {
    let mut rows = Vec::new();
    let outcome = adapter.walk_dir(root, &mut |path, entry, walk_error| {
        let spelled = text(path);
        let decision = control
            .iter()
            .find(|(at, _)| *at == spelled)
            .map_or("continue", |(_, d)| d.as_str());
        let (name, directory, class) = entry.map_or((String::new(), false, "nil"), |entry| {
            (
                text(&entry.name),
                entry.is_dir(),
                mode_class(entry.mode.file_type()),
            )
        });
        rows.push(json!([
            spelled,
            name,
            directory,
            class,
            error_class(walk_error.as_ref()),
            decision
        ]));
        match decision {
            "skip_dir" => Err(WalkError::SkipDir),
            "skip_all" => Err(WalkError::SkipAll),
            "error" => Err(WalkError::Other(IoError::message(WALK_STOP))),
            "propagate" => walk_error.map_or(Ok(()), |error| Err(WalkError::Other(error))),
            _ => Ok(()),
        }
    });
    json!([rows, error_class(outcome.as_ref().err())])
}
fn walk_decisions(a: &Value) -> Result<Vec<(String, String)>, String> {
    let Some(control) = a.get("control").filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    control
        .as_array()
        .ok_or("control is not an array")?
        .iter()
        .map(|pair| {
            let cell = |index: usize| pair.get(index).and_then(Value::as_str).map(str::to_owned);
            match (cell(0), cell(1)) {
                (Some(at), Some(decision))
                    if ["continue", "skip_dir", "skip_all", "error", "propagate"]
                        .contains(&decision.as_str()) =>
                {
                    Ok((at, decision))
                }
                _ => Err("malformed walk control entry".to_string()),
            }
        })
        .collect()
}
fn entries_row(got: &Entries) -> Value {
    let names = |list: &[tsr_jsstring::JsString]| {
        list.iter()
            .map(|name| text(name.as_bytes()))
            .collect::<Vec<_>>()
    };
    let links: Vec<String> = got
        .symlinks
        .iter()
        .flatten()
        .map(|name| text(name.as_bytes()))
        .collect();
    json!([
        names(&got.files),
        names(&got.directories),
        links,
        got.symlinks.is_none()
    ])
}
fn error_class(error: Option<&IoError>) -> String {
    let Some(error) = error else {
        return String::new();
    };
    let sentence = error.to_string();
    if sentence.contains("parent path exists but is not a directory") {
        "parent_not_a_directory".into()
    } else if sentence.contains("path exists but is not a directory") {
        "not_a_directory".into()
    } else if sentence.contains("path exists but is not a regular file") {
        "not_a_regular_file".into()
    } else if sentence.contains("broken symlink") {
        "broken_symlink".into()
    } else if sentence == WALK_STOP {
        "walk_sentinel".into()
    } else if let Some(rest) = sentence.strip_prefix("phase1: spy refused write ") {
        format!("spy_refused_write:{}", rest.split(':').next().unwrap_or(""))
    } else if sentence == SPY_MKDIR_REFUSED {
        "spy_refused_mkdir".into()
    } else if error.is_not_exist() {
        "not_exist".into()
    } else if matches!(error, IoError::Invalid)
        || matches!(error, IoError::Path { source, .. } if **source == IoError::Invalid)
    {
        "invalid".into()
    } else {
        format!("other:{sentence}")
    }
}
fn mode_class(mode: FileMode) -> &'static str {
    if mode.is_dir() {
        "dir"
    } else if mode.is_regular() {
        "regular"
    } else if mode.is_symlink() {
        "symlink"
    } else if mode.is_irregular() {
        "irregular"
    } else {
        "other"
    }
}
fn perm_octal(perm: u32) -> String {
    format!("0o{:03o}", perm & 0o777)
}
fn seed_file(entry: &Value) -> Result<(Vec<u8>, MapFile), String> {
    let cell = |index: usize| {
        entry
            .get(index)
            .and_then(Value::as_str)
            .ok_or("file entries are [path, kind, content]")
    };
    let (path, kind, content) = (cell(0)?, cell(1)?, cell(2)?);
    let file = match kind {
        "file" => MapFile {
            data: content.as_bytes().into(),
            ..MapFile::default()
        },
        "hex" => MapFile {
            data: unhex(content)?.into(),
            ..MapFile::default()
        },
        "symlink" => vfstest::symlink(content.as_bytes()),
        "dir" => MapFile {
            mode: FileMode::DIR | FileMode(0o755),
            ..MapFile::default()
        },
        other => return Err(format!("unsupported file kind: {other}")),
    };
    Ok((path.as_bytes().to_vec(), file))
}
fn plain_map(files: &[Value]) -> Result<MapFs, String> {
    let mut out = std::collections::BTreeMap::new();
    for entry in files {
        let (path, file) = seed_file(entry)?;
        out.insert(
            path.strip_prefix(b"/").unwrap_or(&path).to_vec(),
            Arc::new(file),
        );
    }
    Ok(MapFs(out))
}
/// The probe's panic vocabulary: a class for runtime and sub-tree failures, the
/// literal text for a sentence the pinned source raises itself.
fn guarded(operation: impl FnOnce() -> Value) -> (Value, Value) {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation));
    std::panic::set_hook(hook);
    match outcome {
        Ok(value) => (value, Value::Null),
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
            const SUB_FAILED: &str = "vfs: failed to create sub file system for ";
            let class = if let Some(rest) = message.strip_prefix(SUB_FAILED) {
                match rest.find("\": ") {
                    Some(end) => json!(["class", format!("sub_failed:{}", &rest[..=end])]),
                    None => json!(["class", "sub_failed"]),
                }
            } else if message.contains("out of range") {
                json!(["class", "index_out_of_range"])
            } else if message.contains("interface conversion") {
                json!(["class", "interface_conversion"])
            } else {
                json!(["literal", message])
            };
            (Value::Null, class)
        }
    }
}
fn string(a: &Value, key: &str) -> Result<String, String> {
    a.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("action requires {key}"))
}
fn time(a: &Value, key: &str) -> Result<Time, String> {
    Time::parse_rfc3339(&string(a, key)?).ok_or_else(|| format!("unparsable {key}"))
}
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").expect("writing to a String is infallible");
        out
    })
}
fn unhex(value: &str) -> Result<Vec<u8>, String> {
    (0..value.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(value.get(i..i + 2).unwrap_or(""), 16)
                .map_err(|_| "unparsable hex".to_string())
        })
        .collect()
}
