//! Replay the recording-wrapper trace against production delegates and logs.
use crate::{
    api::{ordered, Outcome},
    fs_trace as t, wrapvfs,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tsr_vfs::{
    iofs::{Sys, Time},
    recording::{Operation, RecordingFs},
    Error, FileSystem, WalkControl,
};
const SENTINEL: &[u8] = b"\0phase1-walk-sentinel";
fn string(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("UTF-8 fixture")
}
fn error(e: Option<Error>) -> &'static str {
    match t::error(e) {
        "none" => "ok",
        "not_exist" => "not_exist",
        _ => "error",
    }
}
fn backing(a: &Value, key: &str) -> Result<Arc<dyn FileSystem>, String> {
    t::filesystem(
        t::array(a, key)?,
        t::flag(a, "case_sensitive")?,
        Time::now(),
    )
}
#[derive(Default)]
struct Walk {
    trace: Vec<Value>,
    probed: bool,
    handed: Vec<Value>,
}
pub fn observe(request: &Value) -> Option<Outcome> {
    if crate::api::subject(request) != "vfsmock.FSMock" {
        return None;
    }
    Some(match replay(request) {
        Ok(rows) => Outcome::Observed(ordered(rows)),
        Err(e) => Outcome::Failed(e),
    })
}
fn replay(request: &Value) -> Result<Vec<Value>, String> {
    let mut inner: Option<Arc<dyn FileSystem>> = None;
    let mut mock: Option<RecordingFs> = None;
    let mut walks: Vec<Arc<Mutex<Walk>>> = Vec::new();
    let mut rows = Vec::new();
    let actions = t::array(request, "actions")?;
    if actions.is_empty() {
        return Err("empty trace".into());
    }
    for a in actions {
        let op = t::text(a, "op")?;
        let mut row = json!({"op":op});
        match op {
            "wrap" | "rebind_source" => {
                let first = backing(a, "files")?;
                mock = Some(RecordingFs::new(first.clone()));
                inner = Some(first);
                walks.clear();
                if op == "wrap" {
                    row["case_sensitive"] = json!(t::flag(a, "case_sensitive")?);
                    fields(&mut row, mock.as_ref().unwrap());
                } else {
                    let _new_source = backing(a, "files_other")?;
                    let r = mock
                        .as_ref()
                        .unwrap()
                        .read_file_result(t::text(a, "path")?.as_bytes())
                        .map_err(|e| e.to_string())?;
                    row["result"] = json!([string(&r.content.raw), r.found]);
                }
            }
            "zero_mock_fields" => fields(&mut row, &RecordingFs::default()),
            "unconfigured_append_file" | "panicking_append_file" => {
                let path = t::text(a, "path")?;
                let data = t::text(a, "data")?;
                let mut recorder = RecordingFs::default();
                if op == "panicking_append_file" {
                    recorder.delegates.append_file = Some(Arc::new(|_, _| {
                        panic!("phase1: delegate panicked on purpose")
                    }));
                }
                let (_, panic) = wrapvfs::guarded(|| {
                    json!(error(
                        recorder.append_file(path.as_bytes(), data.as_bytes()).err()
                    ))
                });
                row["panic"] = json!(if panic.contains("method is nil but FS.") {
                    String::from("unwired_method")
                } else if panic == "other:phase1: delegate panicked on purpose" {
                    String::from("delegate_panicked")
                } else {
                    panic
                });
                let calls = recorder.calls(Operation::AppendFile);
                row["log_length"] = json!(calls.len());
                if op == "panicking_append_file" {
                    row["log"] = json!(calls
                        .iter()
                        .map(|c| json!([
                            string(c.path.as_ref().unwrap().as_bytes()),
                            string(c.data.as_ref().unwrap().as_bytes())
                        ]))
                        .collect::<Vec<_>>());
                }
            }
            _ => {
                let fs = mock.as_ref().ok_or("action before wrap")?;
                let backing = inner.as_ref().ok_or("missing inner")?;
                match op {
                    "interface_methods" => {
                        fields(&mut row, fs);
                        row["count"] = json!(Operation::ALL.len());
                    }
                    "use_case_sensitive_file_names" => {
                        row["result"] = json!(fs.use_case_sensitive_file_names());
                    }
                    "read_use_case_sensitive_file_names_calls" => {
                        row["length"] = json!(fs.calls(Operation::UseCaseSensitiveFileNames).len());
                    }
                    "read_walk_dir_calls" => {
                        let calls = fs.retained_walks();
                        if calls.len() != walks.len() {
                            return Err("retained callback count differs".into());
                        }
                        let mut log = Vec::new();
                        for ((root, callback), record) in calls.iter().zip(&walks) {
                            let returned = match callback(
                                SENTINEL,
                                None,
                                Some(Error::Io(std::io::ErrorKind::NotFound)),
                            ) {
                                Ok(WalkControl::Continue) => "ok",
                                Ok(WalkControl::SkipDir) => "skip_dir",
                                Ok(WalkControl::SkipAll) => "skip_all",
                                Err(_) => "error",
                            };
                            let record = record.lock().unwrap();
                            log.push(json!([
                                string(root.as_bytes()),
                                record.probed,
                                record.handed,
                                returned
                            ]));
                        }
                        row["length"] = json!(calls.len());
                        row["log"] = json!(log);
                    }
                    "walk_dir" => {
                        let root = t::text(a, "root")?;
                        let stop = t::text(a, "stop_kind")?.to_owned();
                        let at = t::text(a, "stop_at")?.to_owned();
                        if !matches!(stop.as_str(), "none" | "skip_dir" | "skip_all") {
                            return Err("invalid stop_kind".into());
                        }
                        let record = Arc::new(Mutex::new(Walk::default()));
                        walks.push(record.clone());
                        let retained = record.clone();
                        let result = fs.walk_dir_owned(
                            root.as_bytes(),
                            Arc::new(move |path, entry, err| {
                                let mut record = retained.lock().unwrap();
                                if path == SENTINEL {
                                    record.probed = true;
                                    record.handed = vec![json!(entry.is_none()), json!(error(err))];
                                    return Ok(WalkControl::SkipAll);
                                }
                                record.trace.push(json!([
                                    string(path),
                                    entry.is_some(),
                                    entry.is_some_and(|e| e.info.directory),
                                    error(err)
                                ]));
                                Ok(if path == at.as_bytes() {
                                    match stop.as_str() {
                                        "skip_dir" => WalkControl::SkipDir,
                                        "skip_all" => WalkControl::SkipAll,
                                        _ => WalkControl::Continue,
                                    }
                                } else {
                                    WalkControl::Continue
                                })
                            }),
                        );
                        row["root"] = json!(root);
                        row["returned"] = json!(error(result.err()));
                        row["trace"] = json!(record.lock().unwrap().trace);
                    }
                    "inner_entry_names" => {
                        let root = t::text(a, "root")?;
                        let mut names = Vec::new();
                        let _ = backing.walk_dir(root.as_bytes(), &mut |p, _, _| {
                            names.push(string(p));
                            Ok(WalkControl::Continue)
                        });
                        names.sort();
                        row["root"] = json!(root);
                        row["result"] = json!(names);
                    }
                    op if op.starts_with("read_") && op.ends_with("_calls") => {
                        let method = match op {
                            "read_append_file_calls" => Operation::AppendFile,
                            "read_chtimes_calls" => Operation::Chtimes,
                            "read_directory_exists_calls" => Operation::DirectoryExists,
                            "read_file_exists_calls" => Operation::FileExists,
                            "read_get_accessible_entries_calls" => Operation::GetAccessibleEntries,
                            "read_read_file_calls" => Operation::ReadFile,
                            "read_realpath_calls" => Operation::Realpath,
                            "read_remove_calls" => Operation::Remove,
                            "read_stat_calls" => Operation::Stat,
                            "read_write_file_calls" => Operation::WriteFile,
                            _ => return Err(format!("unknown log reader {op}")),
                        };
                        let calls = fs.calls(method);
                        let mut log = Vec::new();
                        for c in &calls {
                            let path = string(c.path.as_ref().unwrap().as_bytes());
                            log.push(if let Some(data) = &c.data {
                                json!([path, string(data.as_bytes())])
                            } else if let Some((a, m)) = c.times {
                                json!([path, a.format_rfc3339_nano(), m.format_rfc3339_nano()])
                            } else {
                                json!(path)
                            });
                        }
                        row["length"] = json!(calls.len());
                        row["log"] = json!(log);
                    }
                    _ => {
                        let path = t::text(a, "path")?;
                        if path.is_empty() {
                            return Err("empty path".into());
                        }
                        row["path"] = json!(path);
                        let p = path.as_bytes();
                        row["result"] = match op {
                            "file_exists" => json!(fs.file_exists(p).map_err(|e| e.to_string())?),
                            "directory_exists" => {
                                json!(fs.directory_exists(p).map_err(|e| e.to_string())?)
                            }
                            "read_file" | "inner_read_file" => {
                                let r = if op == "read_file" {
                                    fs.read_file_result(p)
                                } else {
                                    backing.read_file_result(p)
                                }
                                .map_err(|e| e.to_string())?;
                                json!([string(&r.content.raw), r.found])
                            }
                            "mutate_source" => {
                                let e = backing.write_file(p, t::text(a, "data")?.as_bytes()).err();
                                let r = fs.read_file_result(p).map_err(|e| e.to_string())?;
                                row.as_object_mut().unwrap().remove("path");
                                json!([error(e), string(&r.content.raw), r.found])
                            }
                            "stat" => {
                                fs.stat(p)
                                    .map_err(|e| e.to_string())?
                                    .map_or(json!([false]), |i| {
                                        json!([
                                            true,
                                            string(i.name.as_bytes()),
                                            i.size,
                                            i.directory,
                                            i.mod_time.is_zero(),
                                            i.sys != Sys::Nil,
                                            i.mode.to_string()
                                        ])
                                    })
                            }
                            "realpath" => json!(string(
                                fs.realpath(p).map_err(|e| e.to_string())?.as_bytes()
                            )),
                            "get_accessible_entries" => {
                                let e = fs.entries(p).map_err(|e| e.to_string())?;
                                let mut absent = Vec::new();
                                if e.files.is_none() {
                                    absent.push("files");
                                }
                                if e.directories.is_none() {
                                    absent.push("directories");
                                }
                                if e.symlinks.is_none() {
                                    absent.push("symlinks");
                                }
                                json!([
                                    t::strings(e.files.as_deref().unwrap_or_default()),
                                    t::strings(e.directories.as_deref().unwrap_or_default()),
                                    t::strings(
                                        &e.symlinks
                                            .unwrap_or_default()
                                            .into_iter()
                                            .collect::<Vec<_>>()
                                    ),
                                    absent
                                ])
                            }
                            "write_file" => json!(error(
                                fs.write_file(p, t::text(a, "data")?.as_bytes()).err()
                            )),
                            "append_file" => json!(error(
                                fs.append_file(p, t::text(a, "data")?.as_bytes()).err()
                            )),
                            "remove" => json!(error(fs.remove(p).err())),
                            "chtimes" => json!(error(
                                fs.change_times(
                                    p,
                                    t::instant(a, "atime")?,
                                    t::instant(a, "mtime")?
                                )
                                .err()
                            )),
                            "inner_modtime_class" => {
                                let a_time = t::instant(a, "atime")?;
                                let m_time = t::instant(a, "mtime")?;
                                json!(backing.stat(p).map_err(|e| e.to_string())?.map_or(
                                    "missing",
                                    |i| if i.mod_time == m_time {
                                        "equals_mtime"
                                    } else if i.mod_time == a_time {
                                        "equals_atime"
                                    } else {
                                        "equals_neither"
                                    }
                                ))
                            }
                            _ => return Err(format!("unknown mock action {op}")),
                        };
                    }
                }
            }
        }
        rows.push(row);
    }
    Ok(rows)
}
fn fields(row: &mut Value, fs: &RecordingFs) {
    let wired = fs.delegates.wired_count();
    row["wired_count"] = json!(wired);
    row["unwired_count"] = json!(Operation::ALL.len() - wired);
    row["every_member_wired"] = json!(wired == Operation::ALL.len());
}
