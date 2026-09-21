use super::helpers::{
    array, error, in_dir, int, io_error, mkdir, mode, stamp, sys, text, times, write,
    write_observe, Env,
};
use serde_json::{json, Value};
use std::os::unix::fs::MetadataExt;
use tsr_vfs::{
    iofs::Time,
    os::{self, native, WriteMode},
    FileSystem,
};
fn flag(a: &Value) -> Result<WriteMode, String> {
    Ok(match text(a, "flag")? {
        "wronly_create_trunc" => WriteMode::Truncate,
        "wronly_create_append" => WriteMode::Append,
        "wronly_create_excl" => WriteMode::CreateNew,
        "rdonly" => WriteMode::ReadOnly,
        "wronly" => WriteMode::WriteOnly,
        s => return Err(format!("unknown open flag {s}")),
    })
}
pub fn action(e: &Env, a: &Value) -> Result<Value, String> {
    let op = text(a, "op")?;
    let fs = os::fs();
    let path = || text(a, "path").map(|s| e.path(s));
    Ok(match op {
        "write" | "append" | "write_with_flag" | "write_ensuring_dir" => {
            let rel = text(a, "path")?;
            let p = e.path(rel);
            let b = text(a, "content")?.as_bytes();
            let r = match op {
                "write" => fs.write_file(&p, b),
                "append" => fs.append_file(&p, b),
                "write_with_flag" => fs.write_with_mode(&p, b, flag(a)?),
                _ => fs.write_ensuring_directory(&p, b, flag(a)?),
            };
            write_observe(e, rel, r.err())
        }
        "write_literal" | "append_literal" | "write_ensuring_dir_literal" => {
            let p = text(a, "literal")?.as_bytes();
            let b = text(a, "content")?.as_bytes();
            let r = match op {
                "write_literal" => fs.write_file(p, b),
                "append_literal" => fs.append_file(p, b),
                _ => fs.write_ensuring_directory(p, b, flag(a)?),
            };
            json!([error(r.err())])
        }
        "write_with_flag_literal" => {
            let p = text(a, "literal")?.as_bytes();
            let b = text(a, "content")?.as_bytes();
            let mode = flag(a)?;
            in_dir(&e.root, || {
                let r = fs.write_with_mode(p, b, mode);
                json!([error(r.err()), std::fs::metadata(native::path(p)).is_ok()])
            })
        }
        "inspect" => write_observe(e, text(a, "path")?, None),
        "truncate_externally" => json!([sys(
            "truncate",
            std::fs::OpenOptions::new()
                .write(true)
                .open(native::path(&path()?))
                .and_then(|f| f.set_len(0))
        )]),
        "parent_absent" => {
            let m = std::fs::metadata(native::path(&path()?));
            json!([m.is_ok(), sys("stat", m)])
        }
        "lstat_kind" => match std::fs::symlink_metadata(native::path(&path()?)) {
            Ok(m) => json!(["present", mode(os::mode(&m))]),
            Err(err) => json!(["absent", sys::<()>("lstat", Err(err))]),
        },
        "ensure_directory" => {
            let p = path()?;
            let err = fs.ensure_directory(&p).err();
            match std::fs::symlink_metadata(native::path(&p)) {
                Ok(m) => json!([error(err), "present", mode(os::mode(&m))]),
                Err(e) => json!([error(err), "absent", sys::<()>("lstat", Err(e))]),
            }
        }
        "ensure_directory_relative" => {
            let p = text(a, "path")?.as_bytes();
            in_dir(&e.root, || {
                json!([
                    error(fs.ensure_directory(p).err()),
                    std::fs::metadata(native::path(p)).is_ok()
                ])
            })
        }
        "directory_snapshot" => {
            let p = path()?;
            match std::fs::read_dir(native::path(&p)) {
                Err(err) => json!(["unreadable", sys::<()>("open", Err(err))]),
                Ok(entries) => {
                    let mut names = entries
                        .map(|r| r.map(|e| e.file_name().to_string_lossy().into_owned()))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| e.to_string())?;
                    names.sort();
                    json!([
                        "entries",
                        names,
                        std::fs::metadata(native::path(&p))
                            .map_or(json!(["absent"]), |m| mode(os::mode(&m)))
                    ])
                }
            }
        }
        "mkdir" => json!([sys("mkdir", mkdir(&path()?, int(a, "mode")? as u32))]),
        "write_file_raw" => json!([sys(
            "open",
            write(
                &path()?,
                text(a, "content")?.as_bytes(),
                int(a, "mode")? as u32
            )
        )]),
        "remove" => {
            let p = path()?;
            let err = fs.remove(&p).err();
            let alive = array(a, "survivors")?
                .iter()
                .map(|n| {
                    let s = n.as_str().ok_or("invalid survivor")?;
                    Ok(json!([
                        s,
                        std::fs::symlink_metadata(native::path(&e.path(s))).is_ok()
                    ]))
                })
                .collect::<Result<Vec<_>, String>>()?;
            json!([
                error(err),
                std::fs::symlink_metadata(native::path(&p)).is_ok(),
                alive
            ])
        }
        "remove_empty_path" => json!([error(fs.remove(b"").err())]),
        "times_class" => match times(&path()?) {
            Ok((a, m)) => json!(["ok", a[0] > 0, m[0] > 0]),
            Err(err) => json!(["error", io_error(Some(&err.into()))]),
        },
        "chtimes" => {
            let p = path()?;
            let err = fs
                .change_times(&p, stamp(a, "atime")?, stamp(a, "mtime")?)
                .err();
            match times(&p) {
                Ok((a, m)) => json!([error(err), "times", a, m, a == m]),
                Err(e) => json!([error(err), "unstattable", io_error(Some(&e.into()))]),
            }
        }
        "chtimes_literal" => {
            let p = text(a, "literal")?.as_bytes();
            let at = stamp(a, "atime")?;
            let mt = stamp(a, "mtime")?;
            in_dir(&e.root, || json!([error(fs.change_times(p, at, mt).err())]))
        }
        "precision" => {
            let p = path()?;
            let n = int(a, "nanoseconds")?;
            let stamp = Time::from_unix(1_000_000_000, n as u32);
            let err = fs.change_times(&p, stamp, stamp).err();
            match times(&p) {
                Ok((_, m)) => json!([error(err), n, m[1], m[1] == n]),
                Err(_) => json!([error(err), "unstattable"]),
            }
        }
        "link_times" => {
            let l = std::fs::symlink_metadata(native::path(&e.path(text(a, "link")?)));
            match l {
                Err(e) => json!(["lstat_error", io_error(Some(&e.into()))]),
                Ok(l) => match times(&e.path(text(a, "target")?)) {
                    Err(e) => json!(["stat_error", io_error(Some(&e.into()))]),
                    Ok((a, m)) => json!(["ok", a, m, ([l.mtime(), l.mtime_nsec()] == m)]),
                },
            }
        }
        _ => return Err(format!("unknown OS write action {op}")),
    })
}
