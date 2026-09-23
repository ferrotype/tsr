//! Phase 1 F4a: the Rust side of the syntax schedule and of the syntax, AST,
//! navigation and evaluator requests.
//!
//! A private harness, following tools/phase1/config. It calls production APIs
//! where they exist and emits an explicit `not_implemented` row naming the
//! missing operation where they do not. It never emulates a missing algorithm
//! to make a comparison run, and it never reads an expected result.
//!
//! `phase1_syntax <requests.json> <observations.json>` answers the family's
//! request schedule; `phase1_syntax --schedule <probe-requests.json>
//! <rows.jsonl>` answers the corpus syntax schedule, one program per row.

use std::collections::BTreeSet;
use std::error::Error;

use serde_json::{json, Map, Value};

use phase1_harness as api;
use tsr_compiler as ts_compiler_error;
use tsr_compiler::{FileCache, Program, ProgramOptions};

mod astnav;
mod debug;
mod diagnostics;
mod evaluator;
#[path = "../ast-generated/probe.rs"]
mod generated_ast;
mod parse_outputs;
mod scanner_ast;
mod schedule;

// The corpus and embedding consumers share this host and config preparation.
// Declared at the crate root so every group reaches its `pub(super)` items;
// its full loader observer is exercised by S07 program parity, not here.
#[allow(dead_code)]
#[path = "../../../s07/program/rust_observation.rs"]
mod observation;

use api::Outcome;

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// A group module's entry point: claims a request or declines it.
type GroupHandler = fn(&Value) -> Option<Outcome>;

/// Group modules, tried in order. The first to claim a request answers it.
const GROUPS: &[(&str, GroupHandler)] = &[
    ("generatedAst", generated_ast::observe),
    ("scannerAst", scanner_ast::observe),
    ("diagnostics", diagnostics::observe),
    ("astnav", astnav::observe),
    ("evaluator", evaluator::observe),
    ("parseOutputs", parse_outputs::observe),
    ("debug", debug::observe),
];

fn observe(request: &Value) -> Map<String, Value> {
    let outcome = GROUPS
        .iter()
        .find_map(|(_, handler)| handler(request))
        .unwrap_or_else(|| {
            Outcome::Failed(format!(
                "no syntax group claimed subject {:?} for case {:?}",
                api::subject(request),
                request
                    .get("case")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ))
        });
    api::response(request, outcome)
}

fn run(input: &str, output: &str) -> Result<(), Box<dyn Error>> {
    let document: Value = serde_json::from_slice(&std::fs::read(input)?)?;
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
    let args: Vec<String> = std::env::args().collect();
    match args.as_slice() {
        [_, flag, input, output] if flag == "--schedule" => schedule::run(input, output),
        [_, input, output] => run(input, output),
        _ => Err(
            "usage: phase1_syntax <requests.json> <observations.json> | \
                  phase1_syntax --schedule <probe-requests.json> <rows.jsonl>"
                .into(),
        ),
    }
}
