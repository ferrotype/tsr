//! Group `runtime`: the harness's own columns, which claim no operation. They
//! show that the Rust setups agree with Go's (the canonical encoding, both
//! document-order walks of a parsed file, the symbol keys of a bound one), so
//! a column that differs is the column's difference.
//! Go: `tools/phase1/tables/go/runtime_columns.go`; spec:
//! `data/phase1/tables/runtime.json`.
use super::{
    bind_source, bind_source_jsdoc, parse_source, parse_source_jsdoc, text, Column, Parsed,
};
use serde_json::{json, Value};

pub const COLUMNS: &[&str] = &[
    "runtime.values",
    "runtime.walk",
    "runtime.walk_jsdoc",
    "runtime.symbols",
    "runtime.symbols_jsdoc",
];

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "runtime.values" => values(input),
        "runtime.walk" => parse_source(input).and_then(|parsed| walk(&parsed)),
        "runtime.walk_jsdoc" => parse_source_jsdoc(input).and_then(|parsed| walk(&parsed)),
        "runtime.symbols" => bind_source(input).and_then(|parsed| symbols(&parsed)),
        "runtime.symbols_jsdoc" => bind_source_jsdoc(input).and_then(|parsed| symbols(&parsed)),
        _ => return None,
    })
}

/// The input's `value`, returned as decoded: the canonical encoding is the
/// only thing compared.
fn values(input: &Value) -> Result<Column, String> {
    crate::protocol::fields(input, "value")?;
    let value = input["value"].clone();
    Ok(Box::new(move || Ok(value)))
}

/// `[kind, pos, end, parent]` of every node of the input's walk, read in
/// setup.
fn walk(parsed: &Parsed) -> Result<Column, String> {
    let view = parsed.view();
    let mut rows = Vec::with_capacity(parsed.nodes.len());
    for id in &parsed.nodes {
        let node = view.node(*id).map_err(text)?;
        rows.push(json!([
            node.kind().raw(),
            node.pos(),
            node.end(),
            parsed.node_ref(node.parent())?
        ]));
    }
    Ok(Box::new(move || Ok(Value::Array(rows))))
}

/// `[node, symbol key, symbol flags]` of every node with a symbol, read in
/// setup.
fn symbols(parsed: &Parsed) -> Result<Column, String> {
    let bound = parsed.bound().ok_or("the input is not bound")?;
    let mut rows = Vec::new();
    for (at, id) in parsed.nodes.iter().enumerate() {
        if let Some(symbol) = parsed.node_symbol(*id)? {
            let flags = bound.symbol(symbol).map_err(text)?.flags();
            rows.push(json!([at, parsed.symbol_key(Some(symbol))?, flags]));
        }
    }
    Ok(Box::new(move || Ok(Value::Array(rows))))
}
