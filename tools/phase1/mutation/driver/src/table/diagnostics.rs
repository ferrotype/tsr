//! Group `diagnostics`: AST diagnostics.
//! Go: `tools/phase1/tables/go/diagnostics_columns.go`; spec:
//! `data/phase1/tables/diagnostics.json`.
use super::Column;
use serde_json::Value;

pub const COLUMNS: &[&str] = &[];

pub fn build(column: &str, _input: &Value) -> Option<Result<Column, String>> {
    COLUMNS
        .contains(&column)
        .then(|| Err(format!("column {column} has no Rust builder")))
}
