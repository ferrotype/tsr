//! Facts rows: per-node subtree facts after a parse of an S06 primary request,
//! the Rust side of `tools/phase1/mutation/go/facts/main.go`.
//!
//! The `subtree_facts` stage walks every node in document order (a preorder
//! walk of the production child visitor from the source file, as Go's
//! `(*ast.Node).ForEachChild`), then records each walked node's subtree facts
//! in that order through the public production API (`AstView::subtree_facts`,
//! the port of `(*ast.Node).SubtreeFacts`). As on the Go side, the parse and
//! the `subtree_facts` calls are production and the walk observes. The single
//! observation that follows, `[[kind, facts], ...]` (the pairs computed before
//! a panic when the stage panics), is digested as `sha256(canonical(list))`.
use crate::jobs::{enter, Stage};
use crate::protocol::{unhex, Session};
use serde_json::{json, Value};
use std::ops::ControlFlow;
use tsr_ast::{
    AstView, ChildVisitor, ExternalModuleIndicatorOptions, JsString, NodeId, NodeListId, NodeSlice,
    SourceFileParseOptions,
};
use tsr_jsstring::SourceText;

/// One node's immediate children, lists flattened in order.
struct Children<'a> {
    view: AstView<'a>,
    nodes: Vec<NodeId>,
    error: Option<String>,
}

impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.nodes.push(node);
        ControlFlow::Continue(())
    }

    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        match self.view.list(list) {
            Ok(list) => self.visit_node_slice(list.nodes()),
            Err(error) => {
                self.error = Some(error.to_string());
                ControlFlow::Break(())
            }
        }
    }

    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        match self.view.node_slice(nodes) {
            Ok(nodes) => {
                self.nodes.extend(nodes.iter().flatten());
                ControlFlow::Continue(())
            }
            Err(error) => {
                self.error = Some(error.to_string());
                ControlFlow::Break(())
            }
        }
    }
}

/// Every node under `root` with its kind, in preorder.
fn walk(view: AstView<'_>, root: NodeId) -> Result<Vec<(NodeId, i16)>, String> {
    let mut nodes = Vec::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        let node = view.node(id).map_err(|error| error.to_string())?;
        nodes.push((id, node.kind().raw()));
        let mut children = Children {
            view,
            nodes: Vec::new(),
            error: None,
        };
        let _ = node.for_each_child(&mut children);
        if let Some(error) = children.error {
            return Err(error);
        }
        pending.extend(children.nodes.into_iter().rev());
    }
    Ok(nodes)
}

/// Runs one row; the caller is already on the parser worker.
pub fn run(s: &Session, r: &Value) {
    let mut parsed = None;
    if !s.stage("parse", || {
        let source = SourceText::from_loaded_bytes(unhex(&r["source_hex"])?);
        let options = SourceFileParseOptions {
            file_name: JsString::from_bytes(r["filename"].as_str().ok_or("filename")?.as_bytes()),
            path: JsString::from_bytes(r["path"].as_str().ok_or("path")?.as_bytes()),
            external_module_indicator_options: ExternalModuleIndicatorOptions {
                jsx: r["jsx"].as_bool().ok_or("jsx")?,
                force: r["force"].as_bool().ok_or("force")?,
            },
        };
        let kind = r["script_kind"].as_i64().ok_or("script_kind")?;
        parsed = Some(tsr_parser::parse_source_file(
            source,
            tsr_core::ScriptKind(i32::try_from(kind).map_err(|error| error.to_string())?),
            options,
        ));
        Ok(())
    }) {
        return;
    }
    let parsed = parsed.expect("successful parser stage");
    let view = parsed.view();
    let mut nodes = Vec::new();
    let mut facts = Vec::new();
    s.stage("subtree_facts", || {
        enter(Stage::Observe);
        nodes = walk(view, parsed.root())?;
        enter(Stage::Production);
        facts.reserve(nodes.len());
        for (id, _) in &nodes {
            facts.push(view.subtree_facts(*id));
        }
        Ok(())
    });
    let list = nodes
        .iter()
        .zip(&facts)
        .map(|((_, kind), facts)| json!([kind, facts]))
        .collect();
    s.observe("subtree_facts", "facts", Value::Array(list));
}
