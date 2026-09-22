pub use crate::fs_trace::{array, hex, text, unhex};
use serde_json::{json, Value};
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use tsr_vfs::{
    iofs::{FileMode, IoError, Time},
    os::{self, native},
    Error,
};
pub fn int(a: &Value, k: &str) -> Result<i64, String> {
    a[k].as_i64().ok_or_else(|| format!("missing integer {k}"))
}
pub fn chmod(p: &[u8], mode: u32) -> std::io::Result<()> {
    std::fs::set_permissions(native::path(p), std::fs::Permissions::from_mode(mode))
}
pub fn mkdir(p: &[u8], mode: u32) -> std::io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(mode)
        .create(native::path(p))
}
pub fn write(p: &[u8], data: &[u8], mode: u32) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(native::path(p))?
        .write_all(data)
}
pub fn remove(p: &[u8]) -> std::io::Result<()> {
    let p = native::path(p);
    if std::fs::symlink_metadata(&p)?.is_dir() {
        std::fs::remove_dir(p)
    } else {
        std::fs::remove_file(p)
    }
}
pub fn errno(e: &IoError) -> &'static str {
    if let IoError::Os {
        code: Some(code), ..
    } = e
    {
        if *code == rustix::io::Errno::BADF.raw_os_error() {
            return "bad_file_descriptor";
        }
        if *code == rustix::io::Errno::LOOP.raw_os_error() {
            return "too_many_links";
        }
    }
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotADirectory => "not_a_directory",
        K::IsADirectory => "is_a_directory",
        K::DirectoryNotEmpty => "directory_not_empty",
        K::InvalidFilename => "name_too_long",
        K::InvalidInput => "invalid_argument",
        K::NotFound => "not_exist",
        K::PermissionDenied => "permission_denied",
        K::AlreadyExists => "exists",
        _ => "other",
    }
}
pub fn io_error(e: Option<&IoError>) -> Value {
    match e {
        None => json!(["nil"]),
        Some(IoError::Path { op, source, .. }) => json!([
            "patherror",
            if ["openfdat", "unlinkat", "readdirnames", "RemoveAll"].contains(op) {
                "internal"
            } else {
                op
            },
            errno(source)
        ]),
        Some(e) if e.to_string().contains("too many links") => json!(["plain", "too_many_links"]),
        Some(e) => json!(["plain", errno(e)]),
    }
}
pub fn error(e: Option<Error>) -> Value {
    match e {
        None => io_error(None),
        Some(Error::Detailed(e)) => io_error(Some(&e)),
        Some(Error::Unsupported("probe callback refusal")) => {
            json!(["plain", "probe_callback_refusal"])
        }
        Some(e) => json!(["plain", e.to_string()]),
    }
}
pub fn sys<T>(op: &'static str, r: std::io::Result<T>) -> Value {
    match r {
        Ok(_) => json!(["nil"]),
        Err(e) => {
            let e = IoError::from(e);
            json!([
                if matches!(op, "symlink" | "link") {
                    "linkerror"
                } else {
                    "patherror"
                },
                op,
                errno(&e)
            ])
        }
    }
}
pub fn mode(m: FileMode) -> Value {
    let kind = if m.is_dir() {
        "dir"
    } else if m.is_symlink() {
        "symlink"
    } else if m.0 & (1 << 25) != 0 {
        "fifo"
    } else if m.0 & (1 << 24) != 0 {
        "socket"
    } else if m.0 & (1 << 26) != 0 {
        "device"
    } else if m.is_irregular() {
        "irregular"
    } else {
        "regular"
    };
    json!([
        format!("0o{:03o}", m.perm()),
        kind,
        m.is_regular(),
        m.is_dir()
    ])
}
pub fn size(m: FileMode, n: u64) -> Value {
    if m.is_regular() {
        json!(["exact", n])
    } else {
        json!(["host_sized", n > 0])
    }
}
pub fn stamp(a: &Value, key: &str) -> Result<Time, String> {
    let s = text(a, key)?;
    if s == "zero" {
        Ok(Time::ZERO)
    } else {
        Time::parse_rfc3339(s).ok_or_else(|| format!("invalid timestamp {key}"))
    }
}
pub fn times(p: &[u8]) -> std::io::Result<([i64; 2], [i64; 2])> {
    let m = std::fs::metadata(native::path(p))?;
    Ok(([m.atime(), m.atime_nsec()], [m.mtime(), m.mtime_nsec()]))
}
pub struct Env {
    pub root: Vec<u8>,
    real: Vec<u8>,
}
impl Env {
    pub fn new() -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "phase1-osvfs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&p)
            .map_err(|e| e.to_string())?;
        let root = native::bytes(&p);
        let real = native::realpath(&root).unwrap_or_else(|_| root.clone());
        Ok(Self { root, real })
    }
    pub fn path(&self, rel: &str) -> Vec<u8> {
        assert!(
            !Path::new(rel).is_absolute() && !rel.split('/').any(|p| p == ".."),
            "fixture path escapes root"
        );
        if rel.is_empty() {
            self.root.clone()
        } else {
            [self.root.as_slice(), b"/", rel.as_bytes()].concat()
        }
    }
    pub fn place(&self, p: &[u8]) -> String {
        if p.is_empty() {
            return "<empty>".into();
        }
        for (root, label) in [(&self.root, "<root>"), (&self.real, "<realroot>")] {
            if p == root {
                return label.into();
            }
            if let Some(rest) = p
                .strip_prefix(root.as_slice())
                .and_then(|s| s.strip_prefix(b"/"))
            {
                return format!("{label}/{}", String::from_utf8_lossy(rest));
            }
        }
        if p.starts_with(b"/") {
            "<other-absolute>".into()
        } else {
            format!("literal:{}", String::from_utf8_lossy(p))
        }
    }
    pub fn name(&self, rel: &str, name: &[u8]) -> Value {
        if rel.is_empty() {
            json!(["<root-basename>", !name.is_empty(), !name.contains(&b'/')])
        } else if name == rel.rsplit('/').next().unwrap().as_bytes() {
            json!([String::from_utf8_lossy(name), true, true])
        } else {
            json!([self.place(name), false, !name.contains(&b'/')])
        }
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        fn clean(p: &Path) {
            let Ok(m) = std::fs::symlink_metadata(p) else {
                return;
            };
            if m.is_dir() {
                let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700));
                if let Ok(entries) = std::fs::read_dir(p) {
                    for e in entries.flatten() {
                        clean(&e.path());
                    }
                }
            }
        }
        let p = native::path(&self.root);
        clean(&p);
        let _ = std::fs::remove_dir_all(p);
    }
}
pub fn in_dir<T>(root: &[u8], f: impl FnOnce() -> T) -> T {
    struct Restore(PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.0).expect("restore fixture cwd");
        }
    }
    let _restore = Restore(std::env::current_dir().expect("get fixture cwd"));
    std::env::set_current_dir(native::path(root)).expect("enter fixture cwd");
    f()
}
pub fn guarded(op: &str, f: impl FnOnce() -> Result<Value, String>) -> Result<Value, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => Ok(json!({"op":op,"result":v?,"panic":""})),
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| p.downcast_ref::<&str>().copied())
                .unwrap_or("non_error_panic");
            Ok(json!({"op":op,"result":null,"panic":format!("other:{msg}")}))
        }
    }
}
pub fn write_observe(e: &Env, rel: &str, failure: Option<Error>) -> Value {
    let p = e.path(rel);
    let mut out = vec![error(failure)];
    match std::fs::read(native::path(&p)) {
        Ok(b) => out.extend([json!("content"), json!(hex(&b)), json!(b.len())]),
        Err(err) => {
            let op = if std::fs::File::open(native::path(&p)).is_ok() {
                "read"
            } else {
                "open"
            };
            out.extend([json!("unreadable"), sys::<()>(op, Err(err)), json!(0)]);
        }
    }
    match std::fs::symlink_metadata(native::path(&p)) {
        Ok(m) => out.extend([mode(os::mode(&m)), size(os::mode(&m), m.len())]),
        Err(_) => out.extend([json!(["absent"]), json!(["absent"])]),
    }
    out.push(json!("parent"));
    out.push(
        std::fs::metadata(native::path(&p).parent().unwrap())
            .map_or(json!(["absent"]), |m| mode(os::mode(&m))),
    );
    json!(out)
}
