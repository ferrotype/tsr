//! Request decoding and observation formatting shared by filesystem adapters.
//! These helpers construct fixtures and record calls; filesystem behavior stays
//! in tsr_vfs. No expected observations are read here.
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc};
use tsr_vfs::{
    iofs::{FileMode, MapFile, Time},
    vfstest::{self, Clock, InputFile},
    Entries, Error, FileSystem,
};

pub struct FixedClock(pub Time);
impl Clock for FixedClock {
    fn now(&self) -> Time {
        self.0
    }
}
pub fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string {key}"))
}
pub fn flag(value: &Value, key: &str) -> Result<bool, String> {
    value
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("missing boolean {key}"))
}
pub fn array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("missing array {key}"))
}
pub fn instant(value: &Value, key: &str) -> Result<Time, String> {
    Time::parse_rfc3339(text(value, key)?).ok_or_else(|| format!("invalid instant {key}"))
}
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            write!(out, "{b:02x}").expect("write String");
            out
        })
}
pub fn unhex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("odd hex length".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|c| {
            let digit = |b: u8| {
                char::from(b)
                    .to_digit(16)
                    .map(|v| v as u8)
                    .ok_or_else(|| "bad hex digit".to_owned())
            };
            Ok(digit(c[0])? * 16 + digit(c[1])?)
        })
        .collect()
}
pub fn error(failure: Option<Error>) -> &'static str {
    match failure {
        None => "none",
        Some(Error::Unsupported("phase1 sentinel")) => "sentinel",
        Some(Error::Detailed(e)) => error(Some(Error::Io(e.kind()))),
        Some(Error::Io(std::io::ErrorKind::NotFound)) => "not_exist",
        Some(Error::Io(std::io::ErrorKind::PermissionDenied)) => "permission",
        Some(Error::Io(std::io::ErrorKind::InvalidInput)) => "invalid",
        Some(Error::Io(std::io::ErrorKind::AlreadyExists)) => "exist",
        Some(_) => "other",
    }
}
pub fn strings(values: &[tsr_jsstring::JsString]) -> Vec<String> {
    values
        .iter()
        .map(|v| String::from_utf8(v.as_bytes().to_vec()).expect("fixture names are UTF-8"))
        .collect()
}
pub fn entries(row: &mut Value, entries: Entries) {
    row["files"] = json!(strings(entries.files.as_deref().unwrap_or_default()));
    row["directories"] = json!(strings(entries.directories.as_deref().unwrap_or_default()));
    row["symlinks_present"] = json!(entries.symlinks.is_some());
    row["symlinks"] = json!(strings(
        &entries
            .symlinks
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>()
    ));
}
pub fn map_files(files: &[Value]) -> Result<BTreeMap<Vec<u8>, InputFile>, String> {
    files
        .iter()
        .map(|file| {
            let data = match text(file, "kind")? {
                "file" => MapFile {
                    data: text(file, "content")?.as_bytes().into(),
                    ..MapFile::default()
                },
                "dir" => MapFile {
                    mode: FileMode::DIR | FileMode(0o777),
                    ..MapFile::default()
                },
                "symlink" => vfstest::symlink(text(file, "target")?.as_bytes()),
                kind => return Err(format!("unknown file kind {kind}")),
            };
            Ok((
                text(file, "path")?.as_bytes().to_vec(),
                InputFile::File(data),
            ))
        })
        .collect()
}
pub fn filesystem(
    files: &[Value],
    sensitive: bool,
    now: Time,
) -> Result<Arc<dyn FileSystem>, String> {
    Ok(Arc::new(
        vfstest::from_map_with_clock(&map_files(files)?, sensitive, Arc::new(FixedClock(now)))
            .into_vfs(),
    ))
}
pub fn guarded(
    op: &str,
    run: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<Value, String> {
    let mut row = json!({"op":op});
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&mut row))) {
        Ok(result) => result?,
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-error panic");
            row = json!({"op":op,"panic":format!("other:{message}")});
        }
    }
    Ok(row)
}
