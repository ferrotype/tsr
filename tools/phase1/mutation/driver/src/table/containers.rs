//! Group `containers`: containers, function flags, precedence, reparse
//! identity, source-file tables, outer expressions.
//! Go: `tools/phase1/tables/go/containers_columns.go`; spec:
//! `data/phase1/tables/containers.json`.
use super::{parse_source, text, Column, Parsed};
use serde_json::{json, Value};
use tsr_ast::{NodeId, SyntaxKind as K};

pub const COLUMNS: &[&str] = &["binder.FindUseStrictPrologue"];

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "binder.FindUseStrictPrologue" => find_use_strict_prologue(input),
        _ => return None,
    })
}

/// Statement containers: document-order index and statement list.
type Containers = Vec<(usize, Vec<Option<NodeId>>)>;

/// The statement containers (Go's `Node.CanHaveStatements`, ast.go:604) with
/// their statement lists, taken in setup.
fn statement_containers(parsed: &Parsed) -> Result<Containers, String> {
    let view = parsed.view();
    let mut found = Vec::new();
    for (at, id) in parsed.nodes.iter().enumerate() {
        let node = view.node(*id).map_err(text)?;
        if matches!(
            node.kind().known(),
            Some(K::SourceFile | K::Block | K::ModuleBlock | K::CaseClause | K::DefaultClause)
        ) {
            let statements = view
                .node_slice(node.statements(view).map_err(text)?)
                .map_err(text)?
                .iter()
                .collect();
            found.push((at, statements));
        }
    }
    Ok(found)
}

/// `[container, prologue]` of every container whose result is not nil.
fn find_use_strict_prologue(input: &Value) -> Result<Column, String> {
    let parsed = parse_source(input)?;
    let containers = statement_containers(&parsed)?;
    Ok(Box::new(move || {
        let view = parsed.view();
        let mut out = Vec::new();
        for (at, statements) in &containers {
            if let Some(found) =
                tsr_binder::find_use_strict_prologue(view, parsed.root(), statements)
            {
                out.push(json!([at, parsed.node_ref(Some(found))?]));
            }
        }
        Ok(Value::Array(out))
    }))
}
