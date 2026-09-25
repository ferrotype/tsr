//! Generic column shapes shared by the groups; the Go twins are in
//! `tools/phase1/tables/go/columns.go` and produce the same value layout.
use super::{
    bind_source, bind_source_jsdoc, decode, parse_source, parse_source_jsdoc, text, Column, Parsed,
};
use crate::protocol::unhex;
use serde_json::{json, Value};
use tsr_arena::Error;
use tsr_ast::{AstView, NodeId, SyntaxKind};

/// The setup of a parsed input kind.
pub fn parsed_for(kind: &str, input: &Value) -> Result<Parsed, String> {
    match kind {
        "source" => parse_source(input),
        "bound" => bind_source(input),
        "source_jsdoc" => parse_source_jsdoc(input),
        "bound_jsdoc" => bind_source_jsdoc(input),
        other => Err(format!("input kind {other} is not parsed")),
    }
}

/// Admits every node (Go's `All`). A `Filter`, so it returns a `Result`.
#[allow(clippy::unnecessary_wraps)]
pub fn all(_: AstView<'_>, _: NodeId) -> Result<bool, Error> {
    Ok(true)
}

/// Whether a node has one of `kinds` (Go's `Kinds`).
pub fn is_kind(view: AstView<'_>, node: NodeId, kinds: &[SyntaxKind]) -> Result<bool, Error> {
    let kind = view.node(node)?.kind();
    Ok(kinds.iter().any(|candidate| kind == *candidate))
}

type Filter = fn(AstView<'_>, NodeId) -> Result<bool, Error>;

/// Go's `NodePredicate`: the document-order indices, among the nodes
/// `filter` admits, where `pred` is true.
pub fn node_predicate(
    kind: &'static str,
    input: &Value,
    filter: Filter,
    pred: fn(&Parsed, AstView<'_>, NodeId) -> Result<bool, Error>,
) -> Result<Column, String> {
    let parsed = parsed_for(kind, input)?;
    Ok(Box::new(move || {
        let view = parsed.view();
        let mut out = Vec::new();
        for (at, id) in parsed.nodes.iter().enumerate() {
            if filter(view, *id).map_err(text)? && pred(&parsed, view, *id).map_err(text)? {
                out.push(json!(at));
            }
        }
        Ok(Value::Array(out))
    }))
}

/// Go's `NodeMap`: `[index, f(node)]` for every node `filter` admits where
/// `f` is not null.
pub fn node_map(
    kind: &'static str,
    input: &Value,
    filter: Filter,
    f: fn(&Parsed, AstView<'_>, NodeId) -> Result<Value, String>,
) -> Result<Column, String> {
    let parsed = parsed_for(kind, input)?;
    Ok(Box::new(move || {
        let view = parsed.view();
        let mut out = Vec::new();
        for (at, id) in parsed.nodes.iter().enumerate() {
            if !filter(view, *id).map_err(text)? {
                continue;
            }
            let value = f(&parsed, view, *id)?;
            if !value.is_null() {
                out.push(json!([at, value]));
            }
        }
        Ok(Value::Array(out))
    }))
}

/// Go's `RefOf`.
pub fn ref_of(parsed: &Parsed, node: Option<NodeId>) -> Result<Value, String> {
    parsed.node_ref(node)
}

/// Go's `RefsOf`: null for an empty list.
pub fn refs_of(parsed: &Parsed, nodes: Vec<NodeId>) -> Result<Value, String> {
    if nodes.is_empty() {
        return Ok(Value::Null);
    }
    parsed.refs(nodes)
}

/// Go's `Int`: null for zero.
pub fn int(value: i64) -> Value {
    if value == 0 {
        Value::Null
    } else {
        json!(value)
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Names {
    names_hex: Vec<Value>,
}

/// Go's `ValuesMap`: `[f(name)]` over the input's synthetic byte strings.
pub fn values_map(input: &Value, f: fn(&[u8]) -> Result<Value, String>) -> Result<Column, String> {
    let names = decode::<Names>(input)?
        .names_hex
        .iter()
        .map(unhex)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Box::new(move || {
        names
            .iter()
            .map(|name| f(name))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array)
    }))
}
