//! Group `modules`: modules, imports, type-only forms, augmentations, symbol names.
//! Go: `tools/phase1/tables/go/modules_columns.go`; spec:
//! `data/phase1/tables/modules.json`.
use super::Column;
use serde_json::Value;

pub const COLUMNS: &[&str] = &[];

pub fn build(column: &str, _input: &Value) -> Option<Result<Column, String>> {
    COLUMNS
        .contains(&column)
        .then(|| Err(format!("column {column} has no Rust builder")))
}
