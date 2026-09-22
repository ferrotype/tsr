//! Exact generated AST operations; all results come from production factories
//! and visitors. The requests select inputs, never expected observations.
use std::cell::RefCell;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};

use crate::api::{subject, Outcome};
use serde_json::{json, Value};
use tsr_arena::Counters;
use tsr_ast::{
    AstBuilder, AstView, ChildVisitor, Factory, FactoryHooks, FactoryMethods, NodeId, NodeListId,
    NodeSlice, NodeVisitor, NodeVisitorHooks, RuntimeFactory, SyntaxKind,
};
use tsr_core::TextRange;
use tsr_jsstring::{JsString, SourceText};

#[derive(Default)]
struct Hooks(Mutex<Vec<Value>>);
impl Hooks {
    fn take(&self) -> Vec<Value> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
    fn record(&self, stage: &str, factory: &dyn Factory, id: NodeId) {
        let n = factory.node(id);
        self.0
            .lock()
            .unwrap()
            .push(json!([stage, n.kind().raw(), n.flags(), n.pos(), n.end()]));
    }
}
impl FactoryHooks for Hooks {
    fn on_create(&self, f: &mut dyn Factory, id: NodeId) {
        self.record("create", f, id);
    }
    fn on_update(&self, f: &mut dyn Factory, id: NodeId, _: NodeId) {
        self.record("update", f, id);
    }
    fn on_clone(&self, f: &mut dyn Factory, id: NodeId, _: NodeId) {
        self.record("clone", f, id);
    }
}

struct Children<'a> {
    view: AstView<'a>,
    values: Vec<Value>,
    stop: usize,
}
impl Children<'_> {
    fn push(&mut self, node: Option<NodeId>) -> ControlFlow<()> {
        self.values
            .push(node.map_or(Value::Null, |n| json!(self.view.node(n).unwrap().pos())));
        if self.stop != 0 && self.values.len() == self.stop {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}
impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.push(Some(node))
    }
    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        self.visit_node_slice(self.view.list(list).unwrap().nodes())
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        for node in self.view.node_slice(nodes).unwrap().iter() {
            self.push(node)?;
        }
        ControlFlow::Continue(())
    }
}
fn children(f: &AstBuilder, id: NodeId, stop: usize) -> Value {
    let mut visit = Children {
        view: f.view(),
        values: vec![],
        stop,
    };
    let stopped = f.node(id).for_each_child(&mut visit).is_break();
    json!([visit.values, stopped])
}
fn list(f: &AstBuilder, id: NodeId, shape: &str) -> Option<NodeListId> {
    let n = f.node(id);
    match shape {
        "Block" => n.as_block().unwrap().statements(),
        "JSDocParameterOrPropertyTag" => n.as_js_doc_parameter_or_property_tag().unwrap().comment(),
        _ => None,
    }
}
fn snapshot(f: &AstBuilder, id: NodeId, shape: &str) -> Value {
    let n = f.node(id);
    let list = list(f, id, shape).map(|l| {
        let read = f.view().list(l).unwrap();
        let nodes = f.view().node_slice(read.nodes()).unwrap();
        json!([
            read.loc().pos(),
            read.loc().end(),
            read.nodes().is_nil(),
            nodes
                .iter()
                .map(|n| n.map(|n| f.node(n).pos()))
                .collect::<Vec<_>>()
        ])
    });
    let fields = match shape {
        "QualifiedName" => {
            let d = n.as_qualified_name().unwrap();
            json!([
                d.left().map(|n| f.node(n).pos()),
                d.right().map(|n| f.node(n).pos())
            ])
        }
        "Block" => json!([n.as_block().unwrap().multi_line()]),
        "JSDocParameterOrPropertyTag" => {
            let d = n.as_js_doc_parameter_or_property_tag().unwrap();
            json!([
                d.tag_name().map(|n| f.node(n).pos()),
                d.name().map(|n| f.node(n).pos()),
                d.is_bracketed(),
                d.type_expression().map(|n| f.node(n).pos()),
                d.is_name_first()
            ])
        }
        _ => unreachable!(),
    };
    json!([
        n.kind().raw(),
        n.flags(),
        n.pos(),
        n.end(),
        fields,
        list,
        children(f, id, 0)
    ])
}
fn update(
    f: &mut AstBuilder,
    id: NodeId,
    shape: &str,
    changed: bool,
    replacement: NodeId,
) -> NodeId {
    let n = f.node(id);
    match shape {
        "QualifiedName" => {
            let d = n.as_qualified_name().unwrap();
            let (left, right) = (d.left(), d.right());
            drop(n);
            f.update_qualified_name(id, if changed { Some(replacement) } else { left }, right)
        }
        "Block" => {
            let d = n.as_block().unwrap();
            let (list, multi) = (d.statements(), d.multi_line());
            drop(n);
            f.update_block(id, list, if changed { !multi } else { multi })
        }
        "JSDocParameterOrPropertyTag" => {
            let d = n.as_js_doc_parameter_or_property_tag().unwrap();
            let args = (
                d.tag_name(),
                d.name(),
                d.is_bracketed(),
                d.type_expression(),
                d.is_name_first(),
                d.comment(),
            );
            drop(n);
            f.update_js_doc_parameter_or_property_tag(
                id,
                args.0,
                if changed { Some(replacement) } else { args.1 },
                args.2,
                args.3,
                args.4,
                args.5,
            )
        }
        _ => unreachable!(),
    }
}
fn validate_actions(request: &Value, shape: &str) -> Result<(), String> {
    let mut expected = vec![
        "new",
        "children-stop",
        "update-same",
        "update-changed",
        "clone",
        "visit-same",
        "visit-replace",
    ];
    if shape == "QualifiedName" || shape == "Block" {
        expected.push("facts");
    }
    expected.push("counts");
    let expected = expected
        .into_iter()
        .map(|op| json!({"op": op}))
        .collect::<Vec<_>>();
    if request["actions"].as_array() != Some(&expected) {
        return Err("generated AST action schedule differs from executable trace".into());
    }
    Ok(())
}
fn run(request: &Value) -> Result<Value, String> {
    let shape = request["shape"].as_str().ok_or("missing shape")?;
    if !matches!(
        shape,
        "QualifiedName" | "Block" | "JSDocParameterOrPropertyTag"
    ) {
        return Err(format!("unknown generated shape {shape}"));
    }
    if request["operation"].as_str()
        != Some(format!("tsc/internal/ast/ast_generated.go:NodeFactory.New{shape}").as_str())
    {
        return Err("generated AST operation does not match selected factory".into());
    }
    validate_actions(request, shape)?;
    let mode = request["list"].as_str().ok_or("missing list mode")?;
    let absent = request["absent"].as_bool().ok_or("missing absent")?;
    let name_first = request["name_first"]
        .as_bool()
        .ok_or("missing name_first")?;
    let hooks = Arc::new(Hooks::default());
    let mut f = AstBuilder::with_hooks(
        SourceText::from_loaded_bytes(&b""[..]),
        &Counters::new(),
        hooks.clone(),
    );
    let mut sentinels = Vec::new();
    for pos in 1..=5 {
        let n = if pos == 2 {
            let this = f.new_keyword_expression(SyntaxKind::ThisKeyword.into());
            f.new_await_expression(Some(this))
        } else {
            f.new_identifier(JsString::from_bytes(format!("n{pos}").as_bytes()))
        };
        f.node_mut(n)
            .unwrap()
            .set_range(TextRange::new(pos, pos + 1));
        sentinels.push(n);
    }
    let selected_list = match mode {
        "nil" => None,
        "empty" => {
            let nodes = f.alloc_nodes(Vec::new());
            Some(f.alloc_list(TextRange::new(8, 12), nodes))
        }
        "nodes" | "nil-element" => {
            let nodes = f.alloc_nodes(vec![
                Some(sentinels[3]),
                if mode == "nil-element" {
                    None
                } else {
                    Some(sentinels[1])
                },
            ]);
            Some(f.alloc_list(TextRange::new(8, 12), nodes))
        }
        _ => return Err(format!("unknown list mode {mode}")),
    };
    hooks.take();
    let edge = |i: usize| if absent { None } else { Some(sentinels[i]) };
    let root = match shape {
        "QualifiedName" => f.new_qualified_name(edge(0), edge(1)),
        "Block" => f.new_block(selected_list, true),
        "JSDocParameterOrPropertyTag" => f.new_js_doc_parameter_or_property_tag(
            SyntaxKind::JSDocParameterTag.into(),
            edge(0),
            edge(1),
            true,
            edge(2),
            name_first,
            selected_list,
        ),
        _ => unreachable!(),
    };
    let mut out = vec![json!(["new", snapshot(&f, root, shape), hooks.take()])];
    f.node_mut(root).unwrap().set_flags(128);
    f.node_mut(root).unwrap().set_range(TextRange::new(17, 29));
    out.push(json!(["children-stop", children(&f, root, 1)]));
    let same = update(&mut f, root, shape, false, sentinels[4]);
    out.push(json!([
        "update-same",
        same == root,
        snapshot(&f, same, shape),
        hooks.take()
    ]));
    let changed = update(&mut f, root, shape, true, sentinels[4]);
    out.push(json!([
        "update-changed",
        changed == root,
        snapshot(&f, changed, shape),
        hooks.take()
    ]));
    let cloned = f
        .clone_node_generated(root)
        .ok_or("clone dispatch missing")?;
    out.push(json!([
        "clone",
        cloned == root,
        list(&f, cloned, shape) == list(&f, root, shape),
        snapshot(&f, cloned, shape),
        hooks.take()
    ]));
    for replace in [false, true] {
        let calls = RefCell::new(Vec::<Value>::new());
        let callback = |v: &mut NodeVisitor<'_>, n: Option<NodeId>| {
            calls
                .borrow_mut()
                .push(n.map_or(Value::Null, |n| json!(v.factory().node(n).pos())));
            if replace && n == Some(sentinels[1]) {
                Some(sentinels[4])
            } else {
                n
            }
        };
        let visited = NodeVisitor::new(Some(&callback), Some(&mut f), NodeVisitorHooks::default())
            .visit_each_child(Some(root))
            .ok_or("visitor removed root")?;
        out.push(json!([
            if replace {
                "visit-replace"
            } else {
                "visit-same"
            },
            visited == root,
            list(&f, visited, shape) == list(&f, root, shape),
            calls.into_inner(),
            snapshot(&f, visited, shape),
            hooks.take()
        ]));
    }
    if shape == "QualifiedName" || shape == "Block" {
        out.push(json!(["facts", f.view().subtree_facts(root)]));
    }
    out.push(json!(["counts", f.node_count(), f.text_count()]));
    Ok(json!({"ordered":out}))
}
#[path = "predicates.rs"]
mod predicates;
#[path = "shapes_probe.rs"]
mod shapes_probe;

fn predicate(request: &Value) -> Result<Value, String> {
    let name = request["predicate"].as_str().ok_or("missing predicate")?;
    if request["operation"].as_str()
        != Some(format!("tsc/internal/ast/ast_generated.go:{name}").as_str())
    {
        return Err("generated predicate operation does not match selected predicate".into());
    }
    let first = i16::try_from(request["first_kind"].as_i64().ok_or("missing first_kind")?)
        .map_err(|e| e.to_string())?;
    let last = i16::try_from(request["last_kind"].as_i64().ok_or("missing last_kind")?)
        .map_err(|e| e.to_string())?;
    if first > last {
        return Err("reversed kind interval".into());
    }
    let extra = request["extra_kinds"]
        .as_array()
        .ok_or("missing extra_kinds")?;
    let mut kinds = (first..=last).collect::<Vec<_>>();
    for raw in extra {
        kinds.push(
            i16::try_from(raw.as_i64().ok_or("invalid extra kind")?).map_err(|e| e.to_string())?,
        );
    }
    let mut f = AstBuilder::new(SourceText::from_loaded_bytes(&b""[..]), &Counters::new());
    let mut out = vec![];
    for kind in kinds {
        let id = f.new_token(tsr_ast::NodeKind::from_raw(kind));
        let value = predicates::call(name, &f.node(id))
            .ok_or_else(|| format!("unknown generated predicate {name}"))?;
        out.push(json!([kind, value]));
    }
    Ok(json!({"ordered":out}))
}
pub fn observe(request: &Value) -> Option<Outcome> {
    let result = match subject(request) {
        "generatedAst" => run(request),
        "generatedPredicate" => predicate(request),
        "generatedShape" => shapes_probe::run(request),
        "generatedSpecial" => shapes_probe::special(request),
        _ => return None,
    };
    Some(match result {
        Ok(v) => Outcome::Observed(v),
        Err(e) => Outcome::Failed(e),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_an_action_schedule_that_does_not_execute() {
        let mut request = json!({"actions": [
            {"op":"new"}, {"op":"children-stop"}, {"op":"update-same"},
            {"op":"update-changed"}, {"op":"clone"}, {"op":"visit-same"},
            {"op":"visit-replace"}, {"op":"facts"}, {"op":"counts"}
        ]});
        assert!(validate_actions(&request, "QualifiedName").is_ok());
        request["actions"][0]["op"] = json!("not-executed");
        assert!(validate_actions(&request, "QualifiedName").is_err());
        assert!(validate_actions(&json!({}), "QualifiedName").is_err());
        let wrong = json!({"shape":"QualifiedName", "operation":"wrong"});
        assert_eq!(
            run(&wrong).unwrap_err(),
            "generated AST operation does not match selected factory"
        );
        let wrong = json!({"predicate":"IsAwaitExpression", "operation":"wrong"});
        assert_eq!(
            predicate(&wrong).unwrap_err(),
            "generated predicate operation does not match selected predicate"
        );
    }
}
