//! Group `class`: classes, heritage, decorators, modifiers.
//! Go: `tools/phase1/tables/go/class_columns.go`; spec:
//! `data/phase1/tables/class.json`.
use super::{parse_source, text, Column};
use serde_json::{json, Value};

pub const COLUMNS: &[&str] = &["ast.Node.Decorators"];

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "ast.Node.Decorators" => decorators(input),
        _ => return None,
    })
}

/// `[node, [decorators]]` of every node with a nonempty result.
///
/// `Node.Decorators` is ported by PB04 (the s06 accessor fixture); until that
/// port exists this column filters the modifiers here, so it shows parity but
/// credits nothing, and TC1 re-points it to the port.
fn decorators(input: &Value) -> Result<Column, String> {
    let parsed = parse_source(input)?;
    Ok(Box::new(move || {
        let view = parsed.view();
        let mut out = Vec::new();
        for (at, id) in parsed.nodes.iter().enumerate() {
            let node = view.node(*id).map_err(text)?;
            let Some(modifiers) = node.modifiers() else {
                continue;
            };
            let mut found = Vec::new();
            for modifier in view
                .node_slice(view.list(modifiers).map_err(text)?.nodes())
                .map_err(text)?
                .iter()
                .flatten()
            {
                if tsr_ast::is_decorator(&view.node(modifier).map_err(text)?) {
                    found.push(modifier);
                }
            }
            if !found.is_empty() {
                out.push(json!([at, parsed.refs(found)?]));
            }
        }
        Ok(Value::Array(out))
    }))
}
