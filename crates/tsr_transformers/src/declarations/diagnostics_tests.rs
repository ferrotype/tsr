use super::*;
use serde_json::{json, Value};
use std::ops::ControlFlow;
use ts_ast::{ChildVisitor, NodeListId, NodeSlice};

struct Children<'a> {
    view: AstView<'a>,
    nodes: Vec<NodeId>,
    error: Option<Error>,
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
                self.error = Some(error);
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
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
}
fn node_ref(view: AstView<'_>, node: Option<NodeId>) -> Result<Value, Error> {
    node.map(|node| {
        let read = view.node(node)?;
        Ok(json!({"kind":read.kind().raw(),"pos":read.pos(),"end":read.end()}))
    })
    .transpose()
    .map(|node| node.unwrap_or(Value::Null))
}

#[test]
fn selector_codes_and_locations_match_native_declaration_contexts(
) -> Result<(), Box<dyn std::error::Error>> {
    let source = include_bytes!("../../../../tools/s08/p4/declaration-selectors/source.ts");
    let expected: Value = serde_json::from_str(include_str!(
        "../../../../tools/s08/p4/declaration-selectors/observations.json"
    ))?;
    let report: Value = serde_json::from_str(include_str!(
        "../../../../tools/s08/p4/declaration-selectors/report.json"
    ))?;
    let upstream: Value = serde_json::from_str(include_str!("../../../../data/upstream.json"))?;
    assert_eq!(
        report["pin"], upstream["pin"],
        "selector oracle must be recaptured at the pin"
    );
    let parsed = ts_parser::parse_source_file(
        ts_jsstring::SourceText::from_loaded_bytes(source.as_slice()),
        ts_core::ScriptKind::TS,
        ts_ast::SourceFileParseOptions {
            file_name: ts_ast::JsString::from_bytes(b"/selectors.ts".as_slice()),
            path: ts_ast::JsString::from_bytes(b"/selectors.ts".as_slice()),
            ..Default::default()
        },
    );
    let view = parsed.view();
    let root = parsed.root();
    assert!(view.source_file(root)?.diagnostics().is_empty());
    let mut pending = vec![root];
    let mut actual = Vec::new();
    while let Some(node) = pending.pop() {
        if super::super::util::can_produce_diagnostics(&view.node(node)?) {
            for name_context in [false, true] {
                for variant in 0..4 {
                    let mut result = SymbolAccessibilityResult::accessible();
                    result.accessibility = if variant & 2 != 0 {
                        SymbolAccessibility::CannotBeNamed
                    } else {
                        SymbolAccessibility::NotAccessible
                    };
                    result.error_symbol_name = ts_ast::JsString::from_bytes(b"Hidden".as_slice());
                    if variant & 1 != 0 {
                        result.error_module_name =
                            ts_ast::JsString::from_bytes(b"module".as_slice());
                    }
                    let diagnostic = accessibility_diagnostic(view, node, name_context, &result)?;
                    actual.push(json!({
                        "node":node_ref(view,Some(node))?,"name_context":name_context,"variant":variant,
                        "code":diagnostic.map(|record|record.diagnostic_message.code),
                        "error_node":node_ref(view,diagnostic.and_then(|record|record.error_node))?,
                        "type_name":node_ref(view,diagnostic.and_then(|record|record.type_name))?,
                    }));
                }
            }
        }
        let mut children = Children {
            view,
            nodes: Vec::new(),
            error: None,
        };
        let _ = view.node(node)?.for_each_child(&mut children);
        if let Some(error) = children.error {
            return Err(error.into());
        }
        pending.extend(children.nodes.into_iter().rev());
    }
    assert_eq!(Value::Array(actual), expected);
    Ok(())
}
