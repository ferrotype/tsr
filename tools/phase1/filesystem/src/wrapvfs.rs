//! Access-only replay over the production replacement and tracking adapters.
use crate::{
    api::{ordered, Outcome},
    fs_trace as t,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tsr_jsstring::JsString;
use tsr_vfs::{
    self as vfs,
    iofs::{FileMode, MapFile, MapFs, Sys},
    recording::{Call, Operation, RecordingFs},
    tracking::TrackingFs,
    wrapped::{Replacements, WrappedFs},
    Entries, Error, FileContent, FileInfo, FileSystem, ReadResult, WalkControl, WalkEntry,
};
type Log = Arc<Mutex<Vec<Value>>>;
const STUB: &str = "phase1 replacement sentinel";
const CALLBACK: &str = "phase1 callback sentinel";
pub fn observe(request: &Value) -> Option<Outcome> {
    let subject = crate::api::subject(request);
    if !matches!(subject, "wrapvfs.Wrap" | "trackingvfs.FS") {
        return None;
    }
    Some(match replay(subject, request) {
        Ok(rows) => Outcome::Observed(ordered(rows)),
        Err(e) => Outcome::Failed(e),
    })
}
fn js(s: &str) -> JsString {
    JsString::from_bytes(s.as_bytes())
}
fn string(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("UTF-8 fixture")
}
pub(super) fn error(error: Option<Error>) -> String {
    match error {
        None => String::new(),
        Some(Error::Unsupported(STUB)) => "probe_stub_sentinel".into(),
        Some(Error::Unsupported(CALLBACK)) => "probe_callback_sentinel".into(),
        Some(error) => match t::error(Some(error.clone())) {
            "other" => format!("other:{error}"),
            kind => kind.into(),
        },
    }
}
fn returned(value: &Result<WalkControl, Error>) -> String {
    match value {
        Ok(WalkControl::Continue) => String::new(),
        Ok(WalkControl::SkipDir) => "skip_dir".into(),
        Ok(WalkControl::SkipAll) => "skip_all".into(),
        Err(e) => error(Some(e.clone())),
    }
}
pub(super) fn info(info: Option<FileInfo>) -> Value {
    info.map_or(json!("nil"), |i| {
        json!([
            string(i.name.as_bytes()),
            i.size,
            i.directory,
            i.mode.to_string(),
            i.mod_time.format_rfc3339_nano(),
            i.sys != Sys::Nil
        ])
    })
}
pub(super) fn entries(entries: Entries) -> Value {
    json!([
        t::strings(entries.files.as_deref().unwrap_or_default()),
        t::strings(entries.directories.as_deref().unwrap_or_default()),
        entries.symlinks.map_or(json!("nil"), |v| json!(t::strings(
            &v.into_iter().collect::<Vec<_>>()
        )))
    ])
}
fn note(log: &Log, row: Value) {
    log.lock().unwrap().push(row);
}
fn drain(log: &Log) -> Vec<Value> {
    std::mem::take(&mut *log.lock().unwrap())
}
fn names(value: &Value, key: &str) -> Result<Vec<JsString>, String> {
    t::array(value, key)?
        .iter()
        .map(|v| {
            v.as_str()
                .map(js)
                .ok_or_else(|| format!("{key} must contain strings"))
        })
        .collect()
}
fn object_files(action: &Value) -> Result<BTreeMap<Vec<u8>, vfs::vfstest::InputFile>, String> {
    let mut files = BTreeMap::new();
    for (p, v) in action
        .get("files")
        .and_then(Value::as_object)
        .ok_or("missing files object")?
    {
        files.insert(
            p.as_bytes().to_vec(),
            vfs::vfstest::InputFile::File(MapFile {
                data: v.as_str().ok_or("non-string file")?.as_bytes().into(),
                ..MapFile::default()
            }),
        );
    }
    for (p, v) in action
        .get("symlinks")
        .and_then(Value::as_object)
        .ok_or("missing symlinks object")?
    {
        files.insert(
            p.as_bytes().to_vec(),
            vfs::vfstest::InputFile::File(vfs::vfstest::symlink(
                v.as_str().ok_or("non-string symlink")?.as_bytes(),
            )),
        );
    }
    Ok(files)
}
fn backing(action: &Value) -> Result<Arc<dyn FileSystem>, String> {
    let files = object_files(action)?;
    let sensitive = t::flag(action, "case_sensitive")?;
    Ok(match t::text(action, "inner")? {
        "mock_over_mapfs" => Arc::new(
            vfs::vfstest::from_map_with_clock(
                &files,
                sensitive,
                Arc::new(t::FixedClock(t::instant(action, "clock_start")?)),
            )
            .into_vfs(),
        ),
        "mock_over_readonly_iofs" => {
            if !action["symlinks"].as_object().unwrap().is_empty()
                || action
                    .get("clock_start")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
            {
                return Err("readonly inner has no symlinks or clock".into());
            }
            let files = files
                .into_iter()
                .map(|(p, f)| {
                    let vfs::vfstest::InputFile::File(f) = f else {
                        unreachable!()
                    };
                    (p.strip_prefix(b"/").unwrap_or(&p).to_vec(), Arc::new(f))
                })
                .collect();
            Arc::new(vfs::iovfs::from(Arc::new(MapFs(files)), sensitive))
        }
        other => return Err(format!("unknown inner {other}")),
    })
}
fn silence(table: &mut Replacements, method: &str) -> Result<(), String> {
    match method {
        "UseCaseSensitiveFileNames" => table.use_case_sensitive_file_names = None,
        "FileExists" => table.file_exists = None,
        "ReadFile" => table.read_file_result = None,
        "WriteFile" => table.write_file = None,
        "AppendFile" => table.append_file = None,
        "Remove" => table.remove = None,
        "Chtimes" => table.change_times = None,
        "DirectoryExists" => table.directory_exists = None,
        "GetAccessibleEntries" => table.entries = None,
        "Stat" => table.stat = None,
        "WalkDir" => {
            table.walk_dir = None;
            table.walk_dir_owned = None;
        }
        "Realpath" => table.realpath = None,
        _ => return Err(format!("unknown silent method {method}")),
    }
    Ok(())
}
fn install(table: &mut Replacements, stub: &Value, log: &Log) -> Result<(), String> {
    let method = t::text(stub, "method")?.to_owned();
    let behavior = t::text(stub, "behavior")?;
    let log = log.clone();
    match (method.as_str(), behavior) {
        ("UseCaseSensitiveFileNames", "constant_bool") => {
            let value = t::flag(stub, "flag")?;
            table.use_case_sensitive_file_names = Some(Arc::new(move || {
                note(&log, json!([method]));
                value
            }));
        }
        ("FileExists" | "DirectoryExists", "true_for") => {
            let paths = names(stub, "paths")?;
            let directory = method == "DirectoryExists";
            let f: vfs::wrapped::FileExistsReplacement = Arc::new(move |path| {
                note(&log, json!([method, string(path)]));
                Ok(paths.iter().any(|p| p.as_bytes() == path))
            });
            if directory {
                table.directory_exists = Some(f);
            } else {
                table.file_exists = Some(f);
            }
        }
        ("ReadFile", "read_table") => {
            let reads: Vec<_> = t::array(stub, "reads")?
                .iter()
                .map(|r| {
                    Ok((
                        t::text(r, "path")?.to_owned(),
                        t::text(r, "contents")?.as_bytes().to_vec(),
                        t::flag(r, "ok")?,
                    ))
                })
                .collect::<Result<_, String>>()?;
            table.read_file_result = Some(Arc::new(move |path| {
                note(&log, json!([method, string(path)]));
                let found = reads.iter().find(|(p, _, _)| p.as_bytes() == path);
                Ok(ReadResult {
                    content: FileContent::loaded(
                        found.map_or_else(Vec::new, |(_, c, _)| c.clone()),
                    ),
                    found: found.is_some_and(|(_, _, ok)| *ok),
                })
            }));
        }
        ("WriteFile" | "AppendFile", "record_and_succeed" | "record_and_fail") => {
            let fail = behavior == "record_and_fail";
            let append = method == "AppendFile";
            let f: vfs::wrapped::WriteFileReplacement = Arc::new(move |path, data| {
                note(&log, json!([method, string(path), string(data)]));
                if fail {
                    Err(Error::Unsupported(STUB))
                } else {
                    Ok(())
                }
            });
            if append {
                table.append_file = Some(f);
            } else {
                table.write_file = Some(f);
            }
        }
        ("Remove", "record_and_succeed" | "record_and_fail") => {
            let fail = behavior == "record_and_fail";
            table.remove = Some(Arc::new(move |path| {
                note(&log, json!([method, string(path)]));
                if fail {
                    Err(Error::Unsupported(STUB))
                } else {
                    Ok(())
                }
            }));
        }
        ("Chtimes", "record_and_succeed" | "record_and_fail") => {
            let fail = behavior == "record_and_fail";
            table.change_times = Some(Arc::new(move |path, a, m| {
                note(
                    &log,
                    json!([
                        method,
                        string(path),
                        a.format_rfc3339_nano(),
                        m.format_rfc3339_nano()
                    ]),
                );
                if fail {
                    Err(Error::Unsupported(STUB))
                } else {
                    Ok(())
                }
            }));
        }
        ("GetAccessibleEntries", "constant_entries") => {
            let symlinks = names(stub, "symlinks")?;
            let answer = Entries {
                files: Some(names(stub, "files")?),
                directories: Some(names(stub, "directories")?),
                symlinks: (!t::flag(stub, "nil_symlinks")?).then(|| symlinks.into_iter().collect()),
            };
            table.entries = Some(Arc::new(move |path| {
                note(&log, json!([method, string(path)]));
                Ok(answer.clone())
            }));
        }
        ("Stat", "constant_info" | "nil_info") => {
            let answer = if behavior == "nil_info" {
                None
            } else {
                let directory = t::flag(stub, "dir")?;
                let perm = stub["perm"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or("invalid perm")?;
                Some(FileInfo {
                    name: js(t::text(stub, "name")?),
                    size: stub["size"].as_u64().ok_or("invalid size")?,
                    directory,
                    mode: FileMode(perm)
                        | if directory {
                            FileMode::DIR
                        } else {
                            FileMode::default()
                        },
                    mod_time: t::instant(stub, "mod_time")?,
                    sys: Sys::Int(1),
                })
            };
            table.stat = Some(Arc::new(move |path| {
                note(&log, json!([method, string(path)]));
                Ok(answer.clone())
            }));
        }
        ("Realpath", "constant_text") => {
            let answer = js(t::text(stub, "text")?);
            table.realpath = Some(Arc::new(move |path| {
                note(&log, json!([method, string(path)]));
                Ok(answer.clone())
            }));
        }
        ("WalkDir", "replay_arrivals" | "replay_arrivals_then_fail") => {
            let fail = behavior == "replay_arrivals_then_fail";
            let arrivals = t::array(stub, "arrivals")?
                .iter()
                .map(|a| {
                    let directory = t::flag(a, "is_dir")?;
                    let mut info = FileInfo::basic(directory, 0);
                    info.name = js(t::text(a, "name")?);
                    info.mode = if directory {
                        FileMode::DIR | FileMode(0o755)
                    } else {
                        FileMode(0o644)
                    };
                    Ok((
                        t::text(a, "path")?.to_owned(),
                        WalkEntry {
                            name: info.name.clone(),
                            info,
                            symlink: false,
                        },
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            table.walk_dir_owned = None;
            table.walk_dir = Some(Arc::new(move |path, visit| {
                note(&log, json!([method, string(path)]));
                for (path, entry) in &arrivals {
                    let result = visit(path.as_bytes(), Some(entry), None);
                    note(
                        &log,
                        json!(["WalkDirCallbackReturned", path, returned(&result)]),
                    );
                }
                if fail {
                    Err(Error::Unsupported(STUB))
                } else {
                    Ok(())
                }
            }));
        }
        _ => return Err(format!("unsupported replacement {method}: {behavior}")),
    }
    Ok(())
}
fn fill(table: &mut Replacements, action: &Value, log: &Log) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for stub in t::array(action, "replace")? {
        install(table, stub, log)?;
        names.push(t::text(stub, "method")?.into());
    }
    Ok(names)
}
pub(super) fn call(operation: Operation, call: &Call) -> Value {
    let mut args = vec![json!(format!("{operation:?}"))];
    if let Some(path) = &call.path {
        args.push(json!(string(path.as_bytes())));
    }
    if let Some(data) = &call.data {
        args.push(json!(string(data.as_bytes())));
    }
    if let Some((a, m)) = call.times {
        args.push(json!(a.format_rfc3339_nano()));
        args.push(json!(m.format_rfc3339_nano()));
    }
    json!(args)
}
pub(super) fn guarded(run: impl FnOnce() -> Value) -> (Value, String) {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(v) => (v, String::new()),
        Err(p) => {
            let message = p
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| p.downcast_ref::<&str>().copied())
                .unwrap_or("non-error panic");
            (Value::Null, format!("other:{message}"))
        }
    }
}
fn replay(subject: &str, request: &Value) -> Result<Vec<Value>, String> {
    let actions = t::array(request, "actions")?;
    if actions.is_empty() {
        return Err("empty trace".into());
    }
    let mut fs: Option<Arc<dyn FileSystem>> = None;
    let mut mock: Option<Arc<RecordingFs>> = None;
    let mut tracker: Option<Arc<TrackingFs>> = None;
    let mut table = Replacements::default();
    let log = Log::default();
    let mut cursor = 0;
    let mut rows = Vec::new();
    for a in actions {
        let op = t::text(a, "op")?;
        let mut row = json!({"op":op});
        match op {
            "wrap" | "new_tracking" => {
                if (op == "wrap") != (subject == "wrapvfs.Wrap") {
                    return Err("wrong constructor for subject".into());
                }
                let mut recorder = RecordingFs::new(backing(a)?);
                for m in t::array(a, "silent_methods")? {
                    silence(
                        &mut recorder.delegates,
                        m.as_str().ok_or("invalid silent method")?,
                    )?;
                }
                let recorder = Arc::new(recorder);
                mock = Some(recorder.clone());
                cursor = 0;
                if op == "wrap" {
                    table = Replacements::default();
                    row["replaced"] = json!(fill(&mut table, a, &log)?);
                    fs = Some(Arc::new(WrappedFs::new(recorder, table.clone())));
                } else {
                    if a.get("replace").is_some() {
                        return Err("tracking has no replacement table".into());
                    }
                    let tracked = Arc::new(TrackingFs::new(recorder));
                    fs = Some(tracked.clone());
                    tracker = Some(tracked);
                }
                row["inner"] = a["inner"].clone();
            }
            "mutate_replacements" => {
                if fs.is_none() {
                    return Err("mutation before constructor".into());
                }
                row["replaced"] = json!(fill(&mut table, a, &log)?);
            }
            _ => {
                let fs = fs.as_ref().ok_or("call before constructor")?;
                let path = if op == "use_case_sensitive_file_names" {
                    ""
                } else {
                    let p = t::text(a, "path")?;
                    if p.is_empty() {
                        return Err("empty path".into());
                    }
                    row["path"] = json!(p);
                    p
                };
                let mut key = "result";
                let mut arrivals = Vec::new();
                // Validate request-only parameters before the production panic guard.
                let data = if matches!(op, "write_file" | "append_file") {
                    let d = t::text(a, "data")?;
                    row["data"] = json!(d);
                    Some(d)
                } else {
                    None
                };
                let times = if op == "chtimes" {
                    let pair = (t::instant(a, "a_time")?, t::instant(a, "m_time")?);
                    row["times"] =
                        json!([pair.0.format_rfc3339_nano(), pair.1.format_rfc3339_nano()]);
                    Some(pair)
                } else {
                    None
                };
                let callback = if op == "walk_dir" {
                    let c = t::text(a, "callback")?;
                    let at = a.get("at").and_then(Value::as_str).unwrap_or("");
                    if !matches!(c, "collect" | "skip_dir_at" | "skip_all_at" | "error_at")
                        || (c == "collect") != at.is_empty()
                    {
                        return Err("invalid walk callback".into());
                    }
                    Some((c, at))
                } else {
                    None
                };
                if !matches!(
                    op,
                    "use_case_sensitive_file_names"
                        | "file_exists"
                        | "directory_exists"
                        | "read_file"
                        | "entries"
                        | "stat"
                        | "realpath"
                        | "write_file"
                        | "append_file"
                        | "remove"
                        | "chtimes"
                        | "walk_dir"
                ) {
                    return Err(format!("unsupported action {op}"));
                }
                let (value, panic) = guarded(|| match op {
                    "use_case_sensitive_file_names" => json!(fs.use_case_sensitive_file_names()),
                    "file_exists" => {
                        json!(fs.file_exists(path.as_bytes()).expect("predicate adapter"))
                    }
                    "directory_exists" => json!(fs
                        .directory_exists(path.as_bytes())
                        .expect("predicate adapter")),
                    "read_file" => {
                        let r = fs.read_file_result(path.as_bytes()).expect("read adapter");
                        json!([string(r.content.raw.as_ref()), r.found])
                    }
                    "entries" => entries(fs.entries(path.as_bytes()).expect("entries adapter")),
                    "stat" => info(fs.stat(path.as_bytes()).expect("stat adapter")),
                    "realpath" => json!(string(
                        fs.realpath(path.as_bytes())
                            .expect("realpath adapter")
                            .as_bytes()
                    )),
                    "write_file" => {
                        key = "error";
                        json!(error(
                            fs.write_file(path.as_bytes(), data.unwrap().as_bytes())
                                .err()
                        ))
                    }
                    "append_file" => {
                        key = "error";
                        json!(error(
                            fs.append_file(path.as_bytes(), data.unwrap().as_bytes())
                                .err()
                        ))
                    }
                    "remove" => {
                        key = "error";
                        json!(error(fs.remove(path.as_bytes()).err()))
                    }
                    "chtimes" => {
                        key = "error";
                        let (a, m) = times.unwrap();
                        json!(error(fs.change_times(path.as_bytes(), a, m).err()))
                    }
                    "walk_dir" => {
                        key = "error";
                        let (c, at) = callback.unwrap();
                        json!(error(
                            fs.walk_dir(path.as_bytes(), &mut |p, e, err| {
                                let result = if p == at.as_bytes() {
                                    match c {
                                        "skip_dir_at" => Ok(WalkControl::SkipDir),
                                        "skip_all_at" => Ok(WalkControl::SkipAll),
                                        "error_at" => Err(Error::Unsupported(CALLBACK)),
                                        _ => Ok(WalkControl::Continue),
                                    }
                                } else {
                                    Ok(WalkControl::Continue)
                                };
                                arrivals.push(json!([
                                    string(p),
                                    e.map_or(String::new(), |e| string(e.name.as_bytes())),
                                    e.is_some_and(|e| e.info.directory),
                                    error(err),
                                    returned(&result)
                                ]));
                                result
                            })
                            .err()
                        ))
                    }
                    _ => unreachable!(),
                });
                row[key] = value;
                row["panic"] = json!(panic);
                if let Some((c, _)) = callback {
                    row["callback"] = json!(c);
                    row["arrivals"] = json!(arrivals);
                }
            }
        }
        let calls = mock.as_ref().ok_or("missing recorder")?.all_calls();
        row["inner_calls"] = json!(calls[cursor..]
            .iter()
            .map(|(op, c)| call(*op, c))
            .collect::<Vec<_>>());
        cursor = calls.len();
        if subject == "wrapvfs.Wrap" {
            row["stub_calls"] = json!(drain(&log));
        }
        if let Some(tracker) = &tracker {
            let mut seen = tracker.seen_files.to_vec();
            seen.sort();
            row["seen"] = json!(t::strings(&seen));
            row["seen_size"] = json!(tracker.seen_files.len());
        }
        rows.push(row);
    }
    Ok(rows)
}
