//! The bundled leaf group: packaged library access.
//!
//! Every observed row comes from a production `tsr_bundled` item --
//! `LIBRARIES`, `library`, `is_bundled` and `LIB_PATH`. This module renders
//! what they return; it never reimplements a lookup, a path predicate or an
//! index.
//!
//! Two parts of the group's Go authority have no answer here. Both say so
//! through a recorded gap, which is what the capture reads; this comment only
//! explains the gaps, it does not stand in for them.
//!
//! `TestingLibPath` has no Rust counterpart by construction.
//!
//! `wrappedFS`, the dispatch `WrapFS` installs, is exercised on the Go side
//! only. `tsr_bundled::BundledFs` is its counterpart, and a close one for the
//! methods it implements, but `tsr_bundled` imports `tsr_vfs::FileSystem`
//! privately, so its methods cannot be named without a direct `tsr_vfs`
//! dependency that this harness does not declare. Two frozen cases still carry
//! a `wrappedFS` operation id -- `asset-index-complete` records
//! `wrappedFS.WalkDir` and `asset-name-resolution` records
//! `wrappedFS.ReadFile` -- because that is the pinned entry point the native
//! probe calls. What the Rust half of those two cases witnesses is the asset
//! index and the bare-name lookup underneath the wrapper, not the wrapper
//! itself, and each case's `discriminates` says so. The wrapper's own surface
//! is `leaves/bundled/wrapper-dispatch-surface`, whose Rust row is this
//! module's second named gap.

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

/// One production lookup per requested name, answered by `tsr_bundled::library`.
fn lookup(request: &Value) -> Outcome {
    let rows: Vec<Value> = api::actions(request)
        .iter()
        .map(|action| match api::action_op(action) {
            "lookup" => {
                let name = api::action_str(action, "name");
                match tsr_bundled::library(name.as_bytes()) {
                    Some(bytes) => json!([name, true, bytes.len(), digest(bytes)]),
                    None => json!([name, false, -1, ""]),
                }
            }
            other => json!(["unsupported_action", other]),
        })
        .collect();
    Outcome::Observed(api::ordered(rows))
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
        "BundledPath" => Some(paths(request)),
        "BundledLibPath" => Some(lib_path()),
        // Reported as a gap rather than answered. `tsr_bundled::BundledFs` is
        // the counterpart of the pinned wrapper, but its methods are
        // `tsr_vfs::FileSystem` trait methods and `tsr_bundled` imports that
        // trait privately, so nothing can name them without a direct `tsr_vfs`
        // dependency. Two parts of this case would stay gaps even with that
        // dependency added, which is why the record names them.
        "BundledWrapper" => Some(Outcome::missing(
            "tsc/internal/bundled/embed.go:wrapFS",
            "a reachable tsr_bundled::BundledFs: the type exists and implements read_file, \
             stat, directory_exists, entries and realpath over the bundled:/// prefix, but \
             tsr_vfs::FileSystem is a private import of crates/tsr_bundled/src/lib.rs, so a \
             caller needs a direct tsr_vfs dependency to invoke any of them. Beyond reach, \
             two observations in this case have no counterpart to compare even then: \
             tsr_vfs::FileSystem declares no walk method, and BundledFs overrides none of \
             the trait's mutating methods, so it inherits defaults that answer \
             Error::Unsupported for every path instead of refusing a bundled path and \
             delegating the rest",
            "crates/tsr_bundled/src/lib.rs (BundledFs; reachable from this harness once \
             tools/phase1/leaves declares tsr_vfs, which is not this group's file to change)",
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
