use super::{api, inner_filesystem, utf8, Outcome};
use serde_json::{json, Value};
use std::sync::Arc;
use tsr_vfs::{Error, FileSystem, WalkControl as C};
const SENTINEL: Error = Error::Unsupported("phase1 walk sentinel");
fn error(value: Option<&Error>) -> &'static str {
    match value {
        None => "nil",
        Some(Error::Unsupported("phase1 walk sentinel")) => "phase1 walk sentinel",
        Some(Error::Io(std::io::ErrorKind::NotFound)) => "not-exist",
        _ => "other",
    }
}
pub(super) fn walk(request: &Value) -> Outcome {
    let fs = tsr_bundled::BundledFs::new(Arc::new(inner_filesystem(true)));
    let mut rows = Vec::new();
    for action in api::actions(request) {
        let op = api::action_op(action);
        let root = api::action_str(action, "root");
        let stop = api::action_str(action, "stop");
        if op != "walk" || !matches!(stop, "none" | "skip_all_at_2" | "error_at_2") {
            return Outcome::Failed("unknown bundled walk action".into());
        }
        let mut count = 0;
        let mut first = Vec::new();
        let mut callback = "nil";
        let result = fs.walk_dir(root.as_bytes(), &mut |path, entry, err| {
            if let Some(err) = err {
                if callback == "nil" {
                    callback = error(Some(&err));
                }
                return Err(err);
            }
            let entry = entry.expect("successful callback has entry");
            count += 1;
            if first.len() < 64 {
                first.push(json!([
                    utf8(path),
                    utf8(entry.name.as_bytes()),
                    entry.info.directory
                ]));
            }
            if count == 2 {
                match stop {
                    "skip_all_at_2" => return Ok(C::SkipAll),
                    "error_at_2" => return Err(SENTINEL),
                    _ => {}
                }
            }
            Ok(C::Continue)
        });
        rows.push(json!({"op":op,"root":root,"stop":stop,"error":error(result.as_ref().err()),"callback_error":callback,"count":count,"first":first}));
    }
    Outcome::Observed(api::ordered(rows))
}
pub(super) fn source_dir() -> Outcome {
    let path = tsr_bundled::testing_lib_path();
    Outcome::Observed(
        json!({"embedded":true,"lib_path":utf8(tsr_bundled::LIB_PATH),"source_lib_dir_base":path.file_name().unwrap().to_str().unwrap(),"source_lib_dir_parent_base":path.parent().unwrap().file_name().unwrap().to_str().unwrap(),"source_lib_dir_absolute":path.is_absolute(),"source_lib_dir_exists":path.is_dir(),"source_lib_dir_entry_count":std::fs::read_dir(&path).map_or(-1,|entries|entries.count() as i64),"source_lib_d_ts_exists":path.join("lib.d.ts").exists()}),
    )
}
pub(super) fn wrapper() -> Outcome {
    fn run() -> Result<Value, Error> {
        let fs = super::filesystem();
        let mut rows = Vec::new();
        for path in [
            "bundled:///",
            "bundled:///libs",
            "bundled:///libs/",
            "bundled:///LIBS",
            "bundled:///libs/../libs",
            "bundled:////libs",
            "bundled:///libs/lib.d.ts",
            "bundled:///libs/nope.d.ts",
        ] {
            let bytes = path.as_bytes();
            let entries = fs.entries(bytes)?;
            let entry = fs.entry(bytes)?;
            rows.push(json!([
                "path",
                path,
                fs.directory_exists(bytes)?,
                fs.file_exists(bytes)?,
                entries.files.as_ref().map_or(0, Vec::len),
                entries
                    .directories
                    .iter()
                    .flatten()
                    .map(|v| utf8(v.as_bytes()))
                    .collect::<Vec<_>>(),
                entry
                    .as_ref()
                    .map_or("nil", |e| if e.info.directory { "dir" } else { "file" }),
                entry.as_ref().map_or("", |e| utf8(e.name.as_bytes())),
                entry.as_ref().map_or(-1, |e| e.info.size as i64),
                utf8(fs.realpath(bytes)?.as_bytes())
            ]));
        }
        for (label, root, decision) in [
            ("root-dispatch", "bundled:///", 0),
            ("root-skip-dir-on-the-libs-entry", "bundled:///", 1),
            ("libs-complete", "bundled:///libs", 0),
            ("libs-skip-all-at-the-first-entry", "bundled:///libs", 2),
            ("libs-skip-dir-at-every-file-entry", "bundled:///libs", 1),
            ("libs-error-at-the-first-entry", "bundled:///libs", 3),
            ("unknown-bundled-directory", "bundled:///nope", 0),
        ] {
            let mut count = 0;
            let mut first = Vec::new();
            let result = fs.walk_dir(root.as_bytes(), &mut |path, entry, err| {
                if let Some(err) = err {
                    return Err(err);
                }
                let entry = entry.unwrap();
                count += 1;
                if first.len() < 2 {
                    first.push(json!([
                        utf8(path),
                        utf8(entry.name.as_bytes()),
                        entry.info.directory
                    ]));
                }
                match decision {
                    1 => Ok(C::SkipDir),
                    2 => Ok(C::SkipAll),
                    3 => Err(SENTINEL),
                    _ => Ok(C::Continue),
                }
            });
            rows.push(json!([
                "walk",
                label,
                root,
                error(result.as_ref().err()),
                count,
                first
            ]));
        }
        for method in [
            "WriteFile",
            "AppendFile",
            "Remove",
            "Chtimes",
            "Remove-an-absent-asset",
            "WriteFile-the-scheme-root",
        ] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match method {
                "WriteFile" => fs.write_file(b"bundled:///libs/lib.d.ts", b"x"),
                "AppendFile" => fs.append_file(b"bundled:///libs/lib.d.ts", b"x"),
                "Remove" => fs.remove(b"bundled:///libs/lib.d.ts"),
                "Chtimes" => fs.change_times(
                    b"bundled:///libs/lib.d.ts",
                    tsr_vfs::iofs::Time::ZERO,
                    tsr_vfs::iofs::Time::ZERO,
                ),
                "Remove-an-absent-asset" => fs.remove(b"bundled:///libs/nope.d.ts"),
                _ => fs.write_file(b"bundled:///", b"x"),
            }));
            rows.push(match result {
                Ok(value) => json!(["mutate", method, "returned", error(value.as_ref().err())]),
                Err(value) => json!([
                    "mutate",
                    method,
                    "panic",
                    crate::diagnostics::panic_text(value.as_ref())
                ]),
            });
        }
        Ok(api::ordered(rows))
    }
    match run() {
        Ok(value) => Outcome::Observed(value),
        Err(error) => Outcome::Failed(error.to_string()),
    }
}
