//! Group `core`: core helpers, collections, link store, stack.
//! Go: `tools/phase1/tables/go/core_columns.go`; spec:
//! `data/phase1/tables/core.json`.
use super::{decode, Column};
use serde::Deserialize;
use serde_json::{json, Value};

pub const COLUMNS: &[&str] = &["core.Splice"];

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "core.Splice" => splice(input),
        _ => return None,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpliceCase {
    start: i64,
    count: i64,
    items: Vec<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpliceInput {
    s: Vec<i64>,
    cases: Vec<SpliceCase>,
}

/// The result of every `(start, deleteCount, items)` case on one slice.
fn splice(input: &Value) -> Result<Column, String> {
    let input: SpliceInput = decode(input)?;
    Ok(Box::new(move || {
        Ok(Value::Array(
            input
                .cases
                .iter()
                .map(|case| {
                    json!(tsr_core::slices_ext::splice(
                        &input.s,
                        case.start,
                        case.count,
                        &case.items
                    ))
                })
                .collect(),
        ))
    }))
}
