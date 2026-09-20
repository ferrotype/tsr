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

mod api;
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
    let mut row = Map::new();
    let case = request.get("case").and_then(Value::as_str).unwrap_or("");
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("");
    row.insert("case".into(), Value::String(case.to_owned()));
    row.insert("operation".into(), Value::String(operation.to_owned()));

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
    match claimed {
        Some(Outcome::Observed(value)) => {
            row.insert("result".into(), Value::String("observed".into()));
            row.insert("observation".into(), value);
        }
        Some(Outcome::NotImplemented {
            go_authority,
            intended_signature,
            production_home,
        }) => {
            row.insert("result".into(), Value::String("not_implemented".into()));
            row.insert(
                "missing_operation".into(),
                json!({
                    "operation": operation,
                    "go_authority": go_authority,
                    "intended_signature": intended_signature,
                    "production_home": production_home,
                }),
            );
        }
        Some(Outcome::Failed(error)) => {
            row.insert("result".into(), Value::String("harness_failed".into()));
            row.insert("error".into(), Value::String(error));
        }
        None => {
            let subject = api::subject(request);
            row.insert("result".into(), Value::String("harness_failed".into()));
            row.insert(
                "error".into(),
                Value::String(format!(
                    "no leaf group claimed subject {subject:?} for case {case:?}"
                )),
            );
        }
    }
    row
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
