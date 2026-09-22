//! Replay cachedvfs and Common through the production wrappers and root adapter.
use crate::{
    api::{actions, ordered, subject, Outcome},
    fs_trace as trace,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tsr_vfs::{
    cached::CachedFs,
    recording::{Operation, RecordingFs},
    Error, FileSystem, WalkControl,
};
mod common;

pub fn observe(request: &Value) -> Option<Outcome> {
    if !request["case"]
        .as_str()
        .unwrap_or_default()
        .starts_with("filesystem/cachedvfs/")
    {
        return None;
    }
    let result = match subject(request) {
        "CachedFS" => cached(actions(request)),
        "vfs.Common" => common::replay(actions(request)),
        "vfs.RootLength" | "vfs.SplitPath" => paths(subject(request), actions(request)),
        other => Err(format!("unknown cachedvfs subject {other}")),
    };
    Some(match result {
        Ok(rows) => Outcome::Observed(ordered(rows)),
        Err(error) => Outcome::Failed(error),
    })
}
fn paths(subject: &str, actions: &[Value]) -> Result<Vec<Value>, String> {
    actions
        .iter()
        .map(|action| {
            let op = trace::text(action, "op")?;
            let path = trace::text(action, "path")?;
            let mut row = trace::guarded(op, |row| {
                match (subject, op) {
                    ("vfs.RootLength", "root_length") => {
                        row["length"] = json!(tsr_vfs::iovfs::root_length(path.as_bytes()));
                    }
                    ("vfs.SplitPath", "split_path") => {
                        let (root, rest) = tsr_vfs::iovfs::split_path(path.as_bytes());
                        row["root_name"] = json!(String::from_utf8(root).unwrap());
                        row["rest"] = json!(String::from_utf8(rest).unwrap());
                    }
                    _ => return Err(format!("unsupported {subject} action {op}")),
                }
                Ok(())
            })?;
            row["path"] = json!(path);
            Ok(row)
        })
        .collect()
}
fn counts(mock: Option<&RecordingFs>) -> Vec<usize> {
    [
        Operation::DirectoryExists,
        Operation::FileExists,
        Operation::GetAccessibleEntries,
        Operation::Realpath,
        Operation::Stat,
        Operation::ReadFile,
        Operation::UseCaseSensitiveFileNames,
        Operation::WalkDir,
        Operation::Remove,
        Operation::Chtimes,
        Operation::WriteFile,
        Operation::AppendFile,
    ]
    .iter()
    .map(|op| mock.map_or(0, |mock| mock.calls(*op).len()))
    .collect()
}
fn cached(actions: &[Value]) -> Result<Vec<Value>, String> {
    if actions.is_empty() {
        return Err("empty cached trace".into());
    }
    let mut state: Option<(Arc<RecordingFs>, CachedFs)> = None;
    let mut rows = Vec::new();
    for action in actions {
        let op = trace::text(action, "op")?;
        let mut row = trace::guarded(op, |row| {
            if op == "from" {
                let inner = trace::filesystem(
                    trace::array(action, "files")?,
                    trace::flag(action, "case_sensitive")?,
                    trace::instant(action, "now")?,
                )?;
                let mock = Arc::new(RecordingFs::new(inner));
                let cached = CachedFs::new(mock.clone());
                state = Some((mock, cached));
                return Ok(());
            }
            let (mock, fs) = state.as_ref().ok_or("cached filesystem used before from")?;
            match op {
                "enable" => fs.enable(),
                "clear_cache" => fs.clear_cache(),
                "disable_and_clear" => fs.disable_and_clear_cache(),
                "directory_exists" => {
                    row["value"] = json!(fs
                        .directory_exists(trace::text(action, "path")?.as_bytes())
                        .map_err(|e| e.to_string())?);
                }
                "file_exists" => {
                    row["value"] = json!(fs
                        .file_exists(trace::text(action, "path")?.as_bytes())
                        .map_err(|e| e.to_string())?);
                }
                "use_case_sensitive_file_names" => {
                    row["value"] = json!(fs.use_case_sensitive_file_names());
                }
                "read_file" => {
                    let read = fs
                        .read_file_result(trace::text(action, "path")?.as_bytes())
                        .map_err(|e| e.to_string())?;
                    row["contents_hex"] = json!(trace::hex(read.content.text.as_bytes()));
                    row["ok"] = json!(read.found);
                }
                "realpath" => {
                    row["value"] = json!(String::from_utf8(
                        fs.realpath(trace::text(action, "path")?.as_bytes())
                            .map_err(|e| e.to_string())?
                            .as_bytes()
                            .to_vec()
                    )
                    .unwrap());
                }
                "entries" => trace::entries(
                    row,
                    fs.entries(trace::text(action, "path")?.as_bytes())
                        .map_err(|e| e.to_string())?,
                ),
                "stat" => {
                    let info = fs
                        .stat(trace::text(action, "path")?.as_bytes())
                        .map_err(|e| e.to_string())?;
                    row["stat"] = info.map_or(Value::Null, |info| {
                        json!([
                            String::from_utf8(info.name.as_bytes().to_vec()).unwrap(),
                            info.size,
                            info.directory,
                            if action["mod_time"] == "class" {
                                "aliased".to_owned()
                            } else {
                                info.mod_time.format_rfc3339_nano()
                            }
                        ])
                    });
                }
                "write_file" | "append_file" => {
                    let path = trace::text(action, "path")?.as_bytes();
                    let data = trace::unhex(trace::text(action, "data_hex")?)?;
                    let operation = if op == "write_file" {
                        Operation::WriteFile
                    } else {
                        Operation::AppendFile
                    };
                    let result = if op == "write_file" {
                        fs.write_file(path, &data)
                    } else {
                        fs.append_file(path, &data)
                    };
                    row["error"] = json!(trace::error(result.err()));
                    let call = mock
                        .calls(operation)
                        .pop()
                        .ok_or("missing forwarded write")?;
                    row["forwarded"] = json!([
                        String::from_utf8(call.path.unwrap().as_bytes().to_vec()).unwrap(),
                        trace::hex(call.data.unwrap().as_bytes())
                    ]);
                }
                "remove" | "chtimes" => {
                    let path = trace::text(action, "path")?.as_bytes();
                    let (result, operation) = if op == "remove" {
                        (fs.remove(path), Operation::Remove)
                    } else {
                        (
                            fs.change_times(
                                path,
                                trace::instant(action, "a_time")?,
                                trace::instant(action, "m_time")?,
                            ),
                            Operation::Chtimes,
                        )
                    };
                    row["error"] = json!(trace::error(result.err()));
                    let call = mock
                        .calls(operation)
                        .pop()
                        .ok_or("missing forwarded mutation")?;
                    let mut fields = vec![json!(String::from_utf8(
                        call.path.unwrap().as_bytes().to_vec()
                    )
                    .unwrap())];
                    if let Some((at, mt)) = call.times {
                        fields.extend([
                            json!(at.format_rfc3339_nano()),
                            json!(mt.format_rfc3339_nano()),
                        ]);
                    }
                    row["forwarded"] = json!(fields);
                }
                "walk" => {
                    let root = trace::text(action, "root")?;
                    let mut visited = Vec::new();
                    let result = fs.walk_dir(root.as_bytes(), &mut |path, entry, error| {
                        let path = std::str::from_utf8(path).unwrap();
                        let decision = walk_decision(action, path)?;
                        visited.push(json!([
                            path,
                            entry.is_some(),
                            entry.map_or("", |e| std::str::from_utf8(e.name.as_bytes()).unwrap()),
                            entry.is_some_and(|e| e.info.directory),
                            trace::error(error),
                            decision
                        ]));
                        walk_result(decision)
                    });
                    row["visited"] = json!(visited);
                    row["error"] = json!(trace::error(result.err()));
                }
                other => return Err(format!("unsupported cached action {other}")),
            }
            Ok(())
        })?;
        for key in ["path", "root"] {
            if let Some(value) = action.get(key) {
                row[key] = value.clone();
            }
        }
        row["counts"] = json!(counts(state.as_ref().map(|(mock, _)| &**mock)));
        rows.push(row);
    }
    Ok(rows)
}
fn walk_decision<'a>(action: &Value, path: &str) -> Result<&'a str, Error> {
    for (key, decision) in [
        ("skip_dir_at", "skip_dir"),
        ("skip_all_at", "skip_all"),
        ("fail_at", "sentinel"),
    ] {
        let target = action[key]
            .as_str()
            .ok_or(Error::Unsupported("malformed walk request"))?;
        if path == target {
            return Ok(decision);
        }
    }
    Ok("continue")
}
fn walk_result(decision: &str) -> Result<WalkControl, Error> {
    match decision {
        "skip_dir" => Ok(WalkControl::SkipDir),
        "skip_all" => Ok(WalkControl::SkipAll),
        "sentinel" => Err(Error::Unsupported("phase1 sentinel")),
        _ => Ok(WalkControl::Continue),
    }
}
