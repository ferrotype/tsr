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
                "files": entries.files.iter().map(|name| std::str::from_utf8(name.as_bytes()).expect("embedded library names are UTF-8")).collect::<Vec<_>>(),
                "directories": entries.directories.iter().map(|name| std::str::from_utf8(name.as_bytes()).expect("embedded library names are UTF-8")).collect::<Vec<_>>(),
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
        // Reads are exercised separately. This mixed trace also needs the
        // absent walk API and Go-compatible mutation refusal/delegation.
        "BundledWrapper" => Some(Outcome::missing(
            "tsc/internal/bundled/embed.go:wrappedFS.WalkDir and mutating methods",
            "a FileSystem walk operation and bundled mutation refusal/delegation; reads are already exercised by BundledReads and BundledLookup",
            "crates/tsr_bundled/src/lib.rs (FileSystem has no walk method; bundled mutations currently inherit Unsupported defaults)",
        )),
        // Reported as a gap rather than answered. The Go accessor resolves the
        // package's own source directory through runtime.Caller(0); the Rust
        // crate compiles its assets in from a crate-local copy, so there is no
        // checkout path for a counterpart to return. Recording that as a named
        // missing operation is the accurate classification, not an admission
        // that packaged access is unimplemented: the asset-index case is where
        // checkout-independent access is actually witnessed.
        "BundledSourceDir" => Some(Outcome::missing(
            "tsc/internal/bundled/bundled.go:TestingLibPath",
            "none by construction: the Go accessor returns its own source directory from \
             runtime.Caller(0), while crates/tsr_bundled embeds bundled/libs with \
             include_bytes! and ships it through the Cargo `include` list, so a packaged \
             consumer has no source directory to locate",
            "crates/tsr_bundled/src/lib.rs (LIBRARIES; no source-directory accessor intended)",
        )),
        _ => None,
    }
}
