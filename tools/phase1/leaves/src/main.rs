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

/// A production entry point this phase still has to write.
struct Missing {
    subject: &'static str,
    go_authority: &'static str,
    intended_signature: &'static str,
    production_home: &'static str,
}

/// The leaf families whose Rust home does not exist yet. Keyed by request
/// subject so one entry covers a whole family of operations rather than
/// repeating itself per case.
const MISSING: &[Missing] = &[
    Missing {
        subject: "OrderedMap",
        go_authority: "tsc/internal/collections/ordered_map.go:OrderedMap",
        intended_signature:
            "pub struct OrderedMap<K, V> with set/get/has/delete/entry_at/keys/values/entries/clear/size/clone preserving insertion order",
        production_home: "crates/tsr_core/src/collections/ordered_map.rs (absent)",
    },
    Missing {
        subject: "OrderedSet",
        go_authority: "tsc/internal/collections/ordered_set.go:OrderedSet",
        intended_signature: "pub struct OrderedSet<T> with add/has/delete/values/clear/size/clone",
        production_home: "crates/tsr_core/src/collections/ordered_set.rs (absent)",
    },
    Missing {
        subject: "CopyOnWriteMap",
        go_authority: "tsc/internal/collections/copyonwrite.go:CopyOnWriteMap",
        intended_signature: "pub struct CopyOnWriteMap<K, V> with get/has/set/enter_scope",
        production_home: "crates/tsr_core/src/collections/cow.rs (absent)",
    },
    Missing {
        subject: "SyncMap",
        go_authority: "tsc/internal/collections/syncmap.go:SyncMap",
        intended_signature: "pub struct SyncMap<K, V> with load/store/load_or_store/range/delete",
        production_home: "crates/tsr_core/src/collections/sync_map.rs (absent)",
    },
];

fn observe(request: &Value) -> Map<String, Value> {
    let mut row = Map::new();
    let case = request.get("case").and_then(Value::as_str).unwrap_or("");
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("");
    let subject = request.get("subject").and_then(Value::as_str).unwrap_or("");
    row.insert("case".into(), Value::String(case.to_owned()));
    row.insert("operation".into(), Value::String(operation.to_owned()));

    if let Some(missing) = MISSING.iter().find(|m| m.subject == subject) {
        row.insert("result".into(), Value::String("not_implemented".into()));
        row.insert(
            "missing_operation".into(),
            json!({
                "operation": operation,
                "go_authority": missing.go_authority,
                "intended_signature": missing.intended_signature,
                "production_home": missing.production_home,
            }),
        );
        return row;
    }

    row.insert("result".into(), Value::String("harness_failed".into()));
    row.insert(
        "error".into(),
        Value::String(format!(
            "driver has no dispatch entry for subject {subject:?}"
        )),
    );
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
