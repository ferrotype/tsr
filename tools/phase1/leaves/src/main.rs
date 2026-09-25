//! Phase 1 F1a: the Rust side of the foundation-leaf request schedule.
//!
//! A private harness, following tools/s09/format-harness. It calls production
//! APIs where they exist and emits an explicit `not_implemented` row naming the
//! missing operation where they do not. It never emulates a missing algorithm
//! to make a comparison run, and it never reads an expected result.
//!
//! Ordered results are emitted under `ordered` as an array. The comparison
//! canonicalises with sorted keys, so an ordered map rendered as a JSON object
//! would lose exactly the property these cases exist to test.

use std::collections::BTreeSet;
use std::error::Error;

use serde_json::{json, Map, Value};

use phase1_harness as api;
mod bundled;
mod collections;
mod core;
mod diagnostics;
mod helpers;
mod json_contract;
mod locale;
mod options;
mod text;

use api::Outcome;

/// A group module's entry point: claims a request or declines it.
type GroupHandler = fn(&Value) -> Option<Outcome>;

/// Group modules, tried in order. The first to claim a request answers it.
const GROUPS: &[(&str, GroupHandler)] = &[
    ("collections", collections::observe),
    ("core", core::observe),
    ("options", options::observe),
    ("helpers", helpers::observe),
    ("json", json_contract::observe),
    ("text", text::observe),
    ("locale", locale::observe),
    ("diagnostics", diagnostics::observe),
    ("bundled", bundled::observe),
];

fn observe(request: &Value) -> Map<String, Value> {
    let needs_actions = !matches!(
        api::subject(request),
        "BundledIndex"
            | "BundledLibPath"
            | "BundledWrapper"
            | "BundledSourceDir"
            | "diagnostics.roster"
    );
    let claimed = if (needs_actions && request.get("actions").is_none())
        || request.get("actions").is_some_and(|actions| {
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
    let claimed = match claimed {
        Some(Outcome::Observed(value)) => {
            let unsupported = value
                .get("ordered")
                .and_then(Value::as_array)
                .and_then(|rows| {
                    rows.iter().find(|row| {
                        row.get("unsupported_action").is_some()
                            || row.as_array().is_some_and(|items| {
                                items.first().and_then(Value::as_str) == Some("unsupported_action")
                            })
                    })
                });
            if let Some(action) = unsupported {
                Some(Outcome::Failed(format!("unsupported action: {action}")))
            } else {
                Some(Outcome::Observed(value))
            }
        }
        other => other,
    };
    let outcome = claimed.unwrap_or_else(|| {
        let subject = api::subject(request);
        let case = request
            .get("case")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Outcome::Failed(format!(
            "no leaf group claimed subject {subject:?} for case {case:?}"
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
        return Err("usage: phase1_leaves requests.json observations.json".into());
    }
    run(&args[0], &args[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_dispatch_is_observed_through_the_production_walk() {
        // F2b ported wrappedFS.WalkDir, so the wrapper row is no longer a gap.
        let row = observe(
            &json!({"case":"identity-control", "subject":"BundledWrapper",
            "operation":"tsc/internal/bundled/embed.go:wrapFS"}),
        );
        assert_eq!(row["result"], "observed", "{row:?}");
        assert!(row.get("missing_operation").is_none(), "{row:?}");
    }

    #[test]
    fn group_gap_refuses_an_unreviewed_operation() {
        for subject in ["OrderedMap", "locale.parse", "diagnostics.format"] {
            let row = observe(&json!({"case":"identity-control", "subject":subject,
                "operation":"tsc/internal/core/text.go:NewTextRange", "actions":[{"op":"probe"}]}));
            assert_eq!(row["result"], "harness_failed", "{row:?}");
        }
    }

    #[test]
    fn leaves_gaps_are_pinned_and_belong_to_the_case() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/phase1");
        let cases: Value =
            serde_json::from_slice(&std::fs::read(root.join("cases.json")).unwrap()).unwrap();
        let scope: Value =
            serde_json::from_slice(&std::fs::read(root.join("scope.json")).unwrap()).unwrap();
        for entry in std::fs::read_dir(root.join("requests")).unwrap() {
            let path = entry.unwrap().path();
            if !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("leaves-")
            {
                continue;
            }
            let document: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            for request in document["requests"].as_array().unwrap() {
                let row = observe(request);
                assert_ne!(row["result"], "harness_failed", "{row:?}");
                if row["result"] != "not_implemented" {
                    continue;
                }
                let missing = &row["missing_operation"]["operation"];
                let case = cases["cases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|c| c["id"] == row["case"])
                    .unwrap();
                assert!(
                    scope["operations"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|op| op["id"] == *missing),
                    "{row:?}"
                );
                assert!(
                    case["operations"].as_array().unwrap().contains(missing),
                    "{row:?}"
                );
                // The old snapshot copied wrapFS from the request. Preserve it
                // as historical evidence; the specific test above pins the correction.
                if row["case"] != "leaves/bundled/wrapper-dispatch-surface" {
                    assert!(
                        case["missing_operations"]
                            .as_array()
                            .unwrap()
                            .contains(missing),
                        "{row:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn unknown_and_malformed_actions_are_harness_failures() {
        for actions in [
            json!([{"op": "typo_new"}]),
            json!({"op": "new"}),
            json!([]),
            json!([null]),
            json!([{}]),
        ] {
            let result = observe(&json!({"case": "bad", "operation": "NewTextRange",
                "subject": "core.TextRange", "actions": actions}));
            assert_eq!(result["result"], "harness_failed");
        }
        let result = observe(&json!({"case": "missing", "operation": "NewTextRange",
            "subject": "core.TextRange"}));
        assert_eq!(result["result"], "harness_failed");
    }
}
