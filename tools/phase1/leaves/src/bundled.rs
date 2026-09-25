//! The bundled leaf group: packaged library access.
//!
//! Every observed row comes from a production `tsr_bundled` item --
//! `LIBRARIES`, `BundledFs`, `is_bundled` and `LIB_PATH`. This module renders
//! what they return; it never reimplements a lookup, a path predicate or an
//! index.
//!
//! Asset-index evidence is distinct from wrapper evidence: enumerating
//! `LIBRARIES` cannot certify a filesystem walk. Read/path cases call
//! `BundledFs` through its production `FileSystem` interface.

use std::sync::Arc;
use tsr_vfs::{FileSystem, MemoryBuilder};

use serde_json::{json, Value};

use crate::api::{self, Outcome};

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a/64 over asset bytes, as 16 lowercase hex digits.
///
/// The Go probe uses `hash/fnv`, which is the same construction. A digest is
/// how both sides carry 2.3MB of asset bytes into a comparable row; the Rust
/// harness has no hash crate in its dependency set, so this is the one
/// construction both can produce. It renders the observation and is not itself
/// behavior under test.
fn digest(bytes: &[u8]) -> String {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

/// The complete asset index, in the order `LIBRARIES` holds it.
///
/// That order is not presentation: `tsr_bundled::library` resolves a name with
/// a binary search over this list, so a list that is not byte-sorted would make
/// lookups miss silently. The case is order-sensitive for that reason.
fn index() -> Outcome {
    let rows: Vec<Value> = tsr_bundled::LIBRARIES
        .iter()
        .map(|(name, bytes)| json!([*name, bytes.len(), digest(bytes)]))
        .collect();
    Outcome::Observed(json!({ "count": rows.len(), "ordered": rows }))
}

fn filesystem() -> tsr_bundled::BundledFs {
    tsr_bundled::BundledFs::new(Arc::new(MemoryBuilder::new(b"/", true).finish()))
}

/// The inner filesystem the delegation cases wrap, matching the map the Go
/// probe hands to `vfstest.FromMap`. Both hosts are in-memory and both are
/// asked only for normalized, rooted, posix paths, which is the domain the two
/// agree on; what these cases pin is the wrapper's dispatch, not either host.
fn inner_filesystem(case_sensitive: bool) -> tsr_vfs::MemorySnapshot {
    let mut builder = MemoryBuilder::new(b"/", case_sensitive);
    builder.insert_physical(b"/inner/known.txt", b"phase1 inner contents\n".to_vec());
    builder.insert_physical(b"/inner/sub/deep.txt", b"deep\n".to_vec());
    builder.finish()
}

/// Names inside this group are ASCII: the embedded library names and the two
/// inner paths this file writes are the only values that reach it.
fn utf8(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("phase1 bundled paths and entry names are UTF-8")
}

/// The delegating half of the wrapper: a path that is not under the scheme is
/// forwarded to the inner filesystem, and a bundled path is not, even when the
/// inner filesystem could have answered it.
fn delegation(request: &Value) -> Outcome {
    fn run(request: &Value) -> Result<Value, tsr_vfs::Error> {
        let fs = tsr_bundled::BundledFs::new(Arc::new(inner_filesystem(true)));
        let mut rows = Vec::new();
        for action in api::actions(request) {
            let path = api::action_str(action, "path");
            let bytes = path.as_bytes();
            let entries = fs.entries(bytes)?;
            let stat = fs
                .stat(bytes)?
                .map(|info| json!([info.directory, info.size]));
            // A miss is (false, -1, ""), so it stays distinct from an empty
            // file; no real read can report a negative length.
            let read = match fs.read_file(bytes)? {
                Some(content) => json!([true, content.raw.len(), digest(&content.raw)]),
                None => json!([false, -1, ""]),
            };
            let realpath = fs.realpath(bytes)?;
            rows.push(json!({
                "op": "delegate_path",
                "path": path,
                "directory_exists": fs.directory_exists(bytes)?,
                "file_exists": fs.file_exists(bytes)?,
                "files": entries.files.iter().flatten().map(|name| utf8(name.as_bytes())).collect::<Vec<_>>(),
                "directories": entries.directories.iter().flatten().map(|name| utf8(name.as_bytes())).collect::<Vec<_>>(),
                "stat": stat,
                "realpath": utf8(realpath.as_bytes()),
                "read": read,
            }));
        }
        Ok(api::ordered(rows))
    }
    if api::actions(request)
        .iter()
        .any(|action| api::action_op(action) != "delegate_path")
    {
        return Outcome::Failed("unsupported bundled delegation action".into());
    }
    match run(request) {
        Ok(value) => Outcome::Observed(value),
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

/// Whether the wrapper answers for itself or forwards. One inner filesystem per
/// setting is what makes the two answers discriminable; the asset probes in the
/// same row show the embedded half does not follow the setting either way.
fn case_sensitivity(request: &Value) -> Outcome {
    fn run(request: &Value) -> Result<Value, tsr_vfs::Error> {
        let mut rows = Vec::new();
        for action in api::actions(request) {
            // Checked by the guard below, so the value is present here.
            let requested = action
                .get("case_sensitive")
                .and_then(Value::as_bool)
                .unwrap_or_default();
            let inner = inner_filesystem(requested);
            let inner_answer = inner.use_case_sensitive_file_names();
            let fs = tsr_bundled::BundledFs::new(Arc::new(inner));
            let exact = [tsr_bundled::LIB_PATH, b"/lib.d.ts"].concat();
            let upper = [tsr_bundled::LIB_PATH, b"/LIB.D.TS"].concat();
            rows.push(json!({
                "op": "use_case_sensitive_file_names",
                "requested": requested,
                "inner": inner_answer,
                "wrapper": fs.use_case_sensitive_file_names(),
                "exact_case_asset_exists": fs.file_exists(&exact)?,
                "upper_case_asset_exists": fs.file_exists(&upper)?,
                "inner_upper_case_file_exists": fs.file_exists(b"/inner/KNOWN.TXT")?,
            }));
        }
        Ok(api::ordered(rows))
    }
    if api::actions(request)
        .iter()
        .any(|action| api::action_op(action) != "use_case_sensitive_file_names")
    {
        return Outcome::Failed("unsupported bundled case-sensitivity action".into());
    }
    // Absence is a schedule defect, not false. Defaulting would let both sides
    // agree on a zero value without either having exercised the operation, so
    // the key is required here and by a nil check in the Go probe.
    if api::actions(request).iter().any(|action| {
        action
            .get("case_sensitive")
            .and_then(Value::as_bool)
            .is_none()
    }) {
        return Outcome::Failed(
            "use_case_sensitive_file_names needs an explicit case_sensitive".into(),
        );
    }
    match run(request) {
        Ok(value) => Outcome::Observed(value),
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

/// Ask the same wrapper path on both sides, including malformed asset names.
fn lookup(request: &Value) -> Outcome {
    let fs = filesystem();
    let mut rows = Vec::new();
    for action in api::actions(request) {
        if api::action_op(action) != "lookup" {
            return Outcome::Failed(format!("unsupported bundled lookup action: {action}"));
        }
        let name = api::action_str(action, "name");
        let path = [tsr_bundled::LIB_PATH, b"/", name.as_bytes()].concat();
        let content = match fs.read_file(&path) {
            Ok(content) => content,
            Err(error) => return Outcome::Failed(error.to_string()),
        };
        rows.push(match content {
            Some(content) => json!([name, true, content.raw.len(), digest(&content.raw)]),
            None => json!([name, false, -1, ""]),
        });
    }
    Outcome::Observed(api::ordered(rows))
}

fn reads(request: &Value) -> Outcome {
    fn run(request: &Value) -> Result<Value, tsr_vfs::Error> {
        let fs = filesystem();
        let mut rows = Vec::new();
        for action in api::actions(request) {
            let path = api::action_str(action, "path");
            let entries = fs.entries(path.as_bytes())?;
            let stat = fs
                .stat(path.as_bytes())?
                .map(|info| json!([info.directory, info.size]));
            rows.push(json!({
                "op": "read_path", "path": path,
                "directory_exists": fs.directory_exists(path.as_bytes())?,
                "file_exists": fs.file_exists(path.as_bytes())?,
                "files": entries.files.iter().flatten().map(|name| std::str::from_utf8(name.as_bytes()).expect("embedded library names are UTF-8")).collect::<Vec<_>>(),
                "directories": entries.directories.iter().flatten().map(|name| std::str::from_utf8(name.as_bytes()).expect("embedded library names are UTF-8")).collect::<Vec<_>>(),
                "stat": stat,
                "realpath": std::str::from_utf8(fs.realpath(path.as_bytes())?.as_bytes()).expect("request paths are UTF-8"),
            }));
        }
        Ok(api::ordered(rows))
    }
    if api::actions(request)
        .iter()
        .any(|action| api::action_op(action) != "read_path")
    {
        return Outcome::Failed("unsupported bundled path action".into());
    }
    match run(request) {
        Ok(value) => Outcome::Observed(value),
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

/// The `bundled:///` prefix predicate, answered by `tsr_bundled::is_bundled`.
fn paths(request: &Value) -> Outcome {
    let rows: Vec<Value> = api::actions(request)
        .iter()
        .map(|action| match api::action_op(action) {
            "is_bundled" => {
                let path = api::action_str(action, "path");
                json!([path, tsr_bundled::is_bundled(path.as_bytes())])
            }
            other => json!(["unsupported_action", other]),
        })
        .collect();
    Outcome::Observed(api::ordered(rows))
}

/// The root consumers join asset names onto, and whether it is itself bundled.
fn lib_path() -> Outcome {
    match std::str::from_utf8(tsr_bundled::LIB_PATH) {
        Ok(text) => Outcome::Observed(json!({
            "lib_path": text,
            "lib_path_is_bundled": tsr_bundled::is_bundled(tsr_bundled::LIB_PATH),
        })),
        Err(error) => Outcome::Failed(format!("tsr_bundled::LIB_PATH is not UTF-8: {error}")),
    }
}

pub fn observe(request: &Value) -> Option<Outcome> {
    match api::subject(request) {
        "BundledIndex" => Some(index()),
        "BundledLookup" => Some(lookup(request)),
        "BundledReads" => Some(reads(request)),
        "BundledPath" => Some(paths(request)),
        "BundledLibPath" => Some(lib_path()),
        "BundledDelegation" => Some(delegation(request)),
        "BundledCaseSensitivity" => Some(case_sensitivity(request)),
        "BundledWalk" => Some(extra::walk(request)),
        "BundledWrapper" => Some(extra::wrapper()),
        "BundledSourceDir" => Some(extra::source_dir()),
        "BundledFileInfo" => Some(file_info::observe(request)),
        _ => None,
    }
}
mod extra;
mod file_info;
