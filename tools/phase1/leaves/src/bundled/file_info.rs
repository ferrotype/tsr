//! The `fileInfo` members a bundled entry reports: name, size, directory
//! flag, mode, modification time and `Sys`, plus the `fs.DirEntry` half.
//!
//! The port has no `DirEntry` type. `BundledFs::stat` returns the entry's
//! `FileInfo` value, and a walk entry carries that value as `info`, so the
//! Go `Info()` of either is that same value and Go's `Type()` is the type bits
//! of its mode. Every value rendered here is read from what the production
//! `BundledFs` returned.

use super::{api, filesystem, utf8, Outcome};
use serde_json::{json, Value};
use tsr_vfs::iofs::Sys;
use tsr_vfs::{FileInfo, FileSystem, WalkControl};

/// The probe's `phase1InfoRow`, in the same order.
fn info_row(info: &FileInfo) -> Value {
    let (seconds, nanos) = info.mod_time.unix();
    json!([
        utf8(info.name.as_bytes()),
        info.size,
        info.directory,
        info.mode.0,
        info.mod_time.is_zero(),
        seconds,
        nanos,
        matches!(info.sys, Sys::Nil),
    ])
}

pub(super) fn observe(request: &Value) -> Outcome {
    fn run(request: &Value) -> Result<Value, tsr_vfs::Error> {
        let fs = filesystem();
        let mut rows = Vec::new();
        for action in api::actions(request) {
            if api::action_op(action) == "stat" {
                let path = api::action_str(action, "path");
                rows.push(match fs.stat(path.as_bytes())? {
                    None => json!(["stat", path, false]),
                    Some(info) => json!([
                        "stat",
                        path,
                        true,
                        info_row(&info),
                        info.mode.file_type().0,
                        info_row(&info)
                    ]),
                });
            } else {
                // One row per action: the callback sequence travels as a list.
                let root = api::action_str(action, "root");
                let mut entries = Vec::new();
                fs.walk_dir(root.as_bytes(), &mut |path, entry, error| {
                    if let Some(error) = error {
                        return Err(error);
                    }
                    let entry = entry.expect("a successful walk callback carries its entry");
                    entries.push(json!([
                        utf8(path),
                        utf8(entry.name.as_bytes()),
                        entry.info.directory,
                        entry.info.mode.file_type().0,
                        info_row(&entry.info)
                    ]));
                    Ok(WalkControl::Continue)
                })?;
                rows.push(json!(["walk", root, entries]));
            }
        }
        Ok(api::ordered(rows))
    }
    if api::actions(request)
        .iter()
        .any(|action| !matches!(api::action_op(action), "stat" | "walk"))
    {
        return Outcome::Failed("unsupported bundled file-info action".into());
    }
    match run(request) {
        Ok(value) => Outcome::Observed(value),
        Err(error) => Outcome::Failed(error.to_string()),
    }
}
