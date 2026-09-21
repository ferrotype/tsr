//! Phase 1 F2a: the Rust side of the filesystem, path and matching schedule.
//!
//! A private harness, following tools/phase1/leaves. It calls production APIs
//! where they exist and emits an explicit `not_implemented` row naming the
//! missing operation where they do not. It never emulates a missing algorithm
//! to make a comparison run, and it never reads an expected result.
//!
//! Ordered results are emitted under `ordered` as an array. The comparison
//! canonicalises with sorted keys, so a trace rendered as a JSON object would
//! lose exactly the property these cases exist to test.

use std::collections::BTreeSet;
use std::error::Error;

use serde_json::{json, Map, Value};

// The group modules land one per adapter surface. The trace helpers in `api`
// still have no caller: the carried matchFiles group answers whole baselines
// rather than action traces, and the adapter groups that will use them are not
// written yet.
#[allow(dead_code)]
use phase1_harness as api;
mod cachedvfs;
mod glob;
mod iovfs;
mod matchfiles;
mod osvfs;
mod symlinks;
mod tspath;
mod vfsmatch;
mod vfsmock;
mod vfstest;
mod wrapvfs;

use api::Outcome;

/// A group module's entry point: claims a request or declines it.
type GroupHandler = fn(&Value) -> Option<Outcome>;

/// Group modules, tried in order. The first to claim a request answers it.
const GROUPS: &[(&str, GroupHandler)] = &[
    ("cachedvfs", cachedvfs::observe),
    ("glob", glob::observe),
    ("iovfs", iovfs::observe),
    ("matchfiles", matchfiles::observe),
    ("osvfs", osvfs::observe),
    ("symlinks", symlinks::observe),
    ("tspath", tspath::observe),
    ("vfsmatch", vfsmatch::observe),
    ("vfsmock", vfsmock::observe),
    ("vfstest", vfstest::observe),
    ("wrapvfs", wrapvfs::observe),
];

fn observe(request: &Value) -> Map<String, Value> {
    let claimed = if request.get("actions").is_some_and(|actions| {
        actions.as_array().is_none_or(|actions| {
            actions.is_empty()
                || actions.iter().any(|action| {
                    !action.is_object()
                        || action
                            .get("op")
                            .and_then(Value::as_str)
                            .is_none_or(str::is_empty)
                })
        })
    }) {
        Some(Outcome::Failed(
            "actions must be a nonempty array of named operations".into(),
        ))
    } else {
        GROUPS.iter().find_map(|(_, handler)| handler(request))
    };
    let outcome = claimed.unwrap_or_else(|| {
        let subject = api::subject(request);
        let case = request
            .get("case")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Outcome::Failed(format!(
            "no filesystem group claimed subject {subject:?} for case {case:?}"
        ))
    });
    api::response(request, outcome)
}

fn run(input: &str, output: &str) -> Result<(), Box<dyn Error>> {
    let raw = std::fs::read(input)?;
    let document: Value = serde_json::from_slice(&raw)?;
    let requests = document
        .get("requests")
        .and_then(Value::as_array)
        .ok_or("request document has no `requests` array")?;
    let mut seen = BTreeSet::new();
    let mut rows = Vec::with_capacity(requests.len());
    for request in requests {
        let case = request.get("case").and_then(Value::as_str).unwrap_or("");
        if !seen.insert(case.to_owned()) {
            return Err(format!("duplicate case id {case:?} in the request document").into());
        }
        rows.push(Value::Object(observe(request)));
    }
    let observations = json!({
        "version": 1,
        "family": document.get("family").cloned().unwrap_or(Value::Null),
        "observations": rows,
    });
    let mut bytes = serde_json::to_vec_pretty(&observations)?;
    bytes.push(b'\n');
    std::fs::write(output, bytes)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: phase1_filesystem requests.json observations.json".into());
    }
    run(&args[0], &args[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_gap_cannot_claim_an_implemented_operation_is_absent() {
        let row = observe(&json!({
            "case": "identity-control", "subject": "vfsmatch.Usage", "dialect": "vfsmatch",
            "operation": "tsc/internal/vfs/vfsmatch/vfsmatch.go:IsImplicitGlob",
        }));
        assert_eq!(row["result"], "not_implemented");
        assert_eq!(
            row["missing_operation"]["operation"],
            "tsc/internal/vfs/vfsmatch/stringer_generated.go:Usage.String"
        );
    }

    #[test]
    fn group_gap_refuses_an_identity_outside_its_reviewed_entry_points() {
        let row = observe(&json!({
            "case": "identity-control", "subject": "trackingvfs.FS",
            "operation": "tsc/internal/vfs/vfsmatch/vfsmatch.go:IsImplicitGlob",
        }));
        assert_eq!(row["result"], "harness_failed");
        assert!(row.get("missing_operation").is_none());
    }

    #[test]
    fn cached_gap_uses_the_owning_source_not_the_requested_prefix() {
        let row = observe(&json!({
            "case": "filesystem/cachedvfs/identity-control", "subject": "CachedFS",
            "operation": "arbitrary/source.go:From",
        }));
        assert_eq!(
            row["missing_operation"]["operation"],
            "tsc/internal/vfs/cachedvfs/cachedvfs.go:From"
        );
    }

    #[test]
    fn every_frozen_filesystem_gap_has_a_handler_owned_identity() {
        // Exercise each dispatch branch over the real schedule, including the
        // case-specific and multi-operation groups. No native children or
        // production corpus is run by this adapter identity check.
        let directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/phase1/requests");
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("filesystem-")
            {
                continue;
            }
            let document: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            for request in document["requests"].as_array().unwrap() {
                let row = observe(request);
                assert_ne!(row["result"], "harness_failed", "{row:?}");
                if row["result"] == "not_implemented" {
                    assert_eq!(
                        row["missing_operation"]["operation"], request["operation"],
                        "{row:?}"
                    );
                }
            }
        }
    }
}
