//! Group `positions`: names, type and expression positions, access kinds.
//! Go: `tools/phase1/tables/go/positions_columns.go`; spec:
//! `data/phase1/tables/positions.json`.
use super::{parse_source, text, Column};
use serde_json::{json, Value};

pub const COLUMNS: &[&str] = &["ast.IsDeclarationName"];

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "ast.IsDeclarationName" => is_declaration_name(input),
        _ => return None,
    })
}

/// The document-order indices where the operation is true.
fn is_declaration_name(input: &Value) -> Result<Column, String> {
    let parsed = parse_source(input)?;
    Ok(Box::new(move || {
        let view = parsed.view();
        let mut out = Vec::new();
        for (index, id) in parsed.nodes.iter().enumerate() {
            if tsr_ast::utilities_positions::is_declaration_name(view, *id).map_err(text)? {
                out.push(json!(index));
            }
        }
        Ok(Value::Array(out))
    }))
}
