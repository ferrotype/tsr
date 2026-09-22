//! Phase 1 F3a: the Rust side of the config, command-line, package and
//! module-resolution schedule.
//!
//! A private harness, following tools/phase1/leaves and tools/phase1/filesystem.
//! It calls production APIs
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

use phase1_harness as api;
mod commandline;
mod commandlineops;
mod configparse;
mod diagwriter;
mod module;
mod options_wire;
mod packagejson;
mod parseconfighost;
mod tsconfigparsing;

use api::Outcome;

/// A group module's entry point: claims a request or declines it.
type GroupHandler = fn(&Value) -> Option<Outcome>;

/// Group modules, tried in order. The first to claim a request answers it.
const GROUPS: &[(&str, GroupHandler)] = &[
    ("commandline", commandline::observe),
    ("commandlineops", commandlineops::observe),
    ("configparse", configparse::observe),
    ("module", module::observe),
    ("packagejson", packagejson::observe),
    ("diagwriter", diagwriter::observe),
    ("parseconfighost", parseconfighost::observe),
    ("tsconfigparsing", tsconfigparsing::observe),
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
            "no config group claimed subject {subject:?} for case {case:?}"
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
        return Err("usage: phase1_config requests.json observations.json".into());
    }
    run(&args[0], &args[1])
}
