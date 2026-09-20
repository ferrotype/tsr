//! The five callback operations project the pinned api/callbackfs.go contract.
//! Fallback is always the injected snapshot, never the process filesystem.
use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{json, Value};
use tsr_vfs::{FileSystem, MemoryBuilder, MemorySnapshot};

pub(crate) const OPERATIONS: [&str; 5] = [
    "readFile",
    "fileExists",
    "directoryExists",
    "getAccessibleEntries",
    "realpath",
];

pub(crate) struct Host {
    pub case_sensitive: bool,
    callbacks: BTreeSet<String>,
    snapshot: MemorySnapshot,
}

impl Host {
    pub fn new(
        case_sensitive: bool,
        base: &BTreeMap<String, String>,
        symlinks: BTreeMap<String, String>,
        callbacks: &[String],
    ) -> Result<Self, String> {
        let enabled: BTreeSet<_> = callbacks.iter().cloned().collect();
        if enabled.len() != callbacks.len()
            || enabled
                .iter()
                .any(|name| !OPERATIONS.contains(&name.as_str()))
        {
            return Err("callbacks must name unique supported filesystem operations".into());
        }
        let mut canonical = BTreeSet::new();
        for path in base.keys().chain(symlinks.keys()) {
            valid_base_path(path)?;
            let key = tsr_tspath::to_path(path.as_bytes(), b"/", case_sensitive)
                .as_bytes()
                .to_vec();
            if !canonical.insert(key) {
                return Err("injected paths collide after normalization/case folding".into());
            }
        }
        for key in &canonical {
            for ancestor in tsr_tspath::ancestors(&tsr_tspath::directory(key.as_slice())) {
                if canonical.contains(ancestor.as_slice()) {
                    return Err("injected file or symlink is an ancestor of another entry".into());
                }
            }
        }
        let mut builder = MemoryBuilder::new(b"/", case_sensitive);
        builder.insert_directory(b"/");
        for (path, content) in base {
            valid_base_path(path)?;
            builder.insert_physical(path.as_bytes(), content.as_bytes());
        }
        for (path, target) in symlinks {
            valid_base_path(&path)?;
            valid_base_path(&target)?;
            if base.contains_key(&path) {
                return Err("base file and symlink share a path".into());
            }
            builder.insert_symlink(path.as_bytes(), target.as_bytes());
        }
        Ok(Self {
            case_sensitive,
            callbacks: enabled,
            snapshot: builder.finish(),
        })
    }

    pub fn enabled(&self, operation: &str) -> bool {
        self.callbacks.contains(operation)
    }

    pub fn complete(&self, operation: &str, path: &str, result: Value) -> Result<Value, String> {
        if result.is_null() {
            return self.base(operation, path);
        }
        match operation {
            "readFile" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Read {
                    content: Option<String>,
                }
                // Go decodes both omitted content and explicit null as missing.
                let read: Read = serde_json::from_value(result).map_err(|e| e.to_string())?;
                Ok(json!({"content":read.content}))
            }
            "fileExists" | "directoryExists" if result.is_boolean() => Ok(result),
            "realpath" if result.is_string() => Ok(result),
            "getAccessibleEntries" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Entries {
                    files: Vec<String>,
                    directories: Vec<String>,
                }
                let entries: Entries = serde_json::from_value(result).map_err(|e| e.to_string())?;
                Ok(json!({"files":entries.files,"directories":entries.directories}))
            }
            _ => Err(format!("invalid {operation} callback result")),
        }
    }

    fn base(&self, operation: &str, path: &str) -> Result<Value, String> {
        let bytes = path.as_bytes();
        let io = |error: tsr_vfs::Error| error.to_string();
        match operation {
            "readFile" => {
                let content = self.snapshot.read_file(bytes).map_err(io)?;
                let content = content
                    .as_ref()
                    .map(|file| utf8(file.text.as_bytes()))
                    .transpose()?;
                Ok(json!({"content":content}))
            }
            "fileExists" => self
                .snapshot
                .file_exists(bytes)
                .map(Value::Bool)
                .map_err(io),
            "directoryExists" => self
                .snapshot
                .directory_exists(bytes)
                .map(Value::Bool)
                .map_err(io),
            "realpath" => {
                // Go vfstest.Realpath returns the original input when resolution
                // fails, including missing paths containing dot segments.
                if self.snapshot.stat(bytes).map_err(io)?.is_none() {
                    return Ok(json!(path));
                }
                Ok(json!(utf8(
                    self.snapshot.realpath(bytes).map_err(io)?.as_bytes()
                )?))
            }
            "getAccessibleEntries" => {
                let entries = self.snapshot.entries(bytes).map_err(io)?;
                let files: Result<Vec<_>, _> =
                    entries.files.iter().map(|s| utf8(s.as_bytes())).collect();
                let directories: Result<Vec<_>, _> = entries
                    .directories
                    .iter()
                    .map(|s| utf8(s.as_bytes()))
                    .collect();
                Ok(json!({"files":files?,"directories":directories?}))
            }
            _ => Err("unknown filesystem operation".into()),
        }
    }
}

fn utf8(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes).map_err(|_| "injected filesystem produced non-UTF8 text".into())
}

pub(crate) fn valid_base_path(path: &str) -> Result<(), String> {
    if path.starts_with('/') && !path.contains('\0') {
        Ok(())
    } else {
        Err("injected base paths must be slash-rooted and contain no NUL".into())
    }
}
