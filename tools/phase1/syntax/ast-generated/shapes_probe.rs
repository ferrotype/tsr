//! Shared mechanics for schema-generated calls; no expected AST behavior.
use super::{children, Hooks};
use serde_json::{json, Value};
use std::{cell::RefCell, sync::Arc};
use tsr_arena::Counters;
use tsr_ast::{
    AstBuilder, Factory, FactoryMethods, NodeId, NodeKind, NodeListId, NodeSlice, NodeVisitor,
    NodeVisitorHooks, RuntimeFactory, SyntaxKind, TextSlice,
};
use tsr_core::TextRange;
use tsr_jsstring::{JsString, SourceText};

#[path = "shapes.rs"]
mod dispatch;
struct Inputs {
    absent: bool,
    nodes: Vec<NodeId>,
    replacements: Vec<NodeId>,
    lists: Vec<Option<NodeListId>>,
    mods: Vec<Option<NodeListId>>,
    changed_lists: Vec<NodeListId>,
    changed_mods: Vec<NodeListId>,
    raw: Vec<NodeSlice>,
    changed_raw: Vec<NodeSlice>,
    text: Vec<TextSlice>,
    changed_text: Vec<TextSlice>,
}
impl Inputs {
    fn new(f: &mut AstBuilder, mode: &str) -> Self {
        let mut b = Self {
            absent: mode == "nil",
            nodes: vec![],
            replacements: vec![],
            lists: vec![],
            mods: vec![],
            changed_lists: vec![],
            changed_mods: vec![],
            raw: vec![],
            changed_raw: vec![],
            text: vec![],
            changed_text: vec![],
        };
        for i in 0..16 {
            let node = if i % 2 == 0 {
                let this = f.new_keyword_expression(SyntaxKind::ThisKeyword.into());
                f.new_await_expression(Some(this))
            } else {
                f.new_identifier(JsString::from_bytes(format!("node{i}").as_bytes()))
            };
            let replacement =
                f.new_identifier(JsString::from_bytes(format!("replacement{i}").as_bytes()));
            let modifier = f.new_token(SyntaxKind::ExportKeyword.into());
            let changed_modifier = f.new_token(SyntaxKind::AsyncKeyword.into());
            for (n, pos) in [
                (node, 10 * i + 1),
                (replacement, 1000 + 10 * i + 1),
                (modifier, 10 * i + 2),
                (changed_modifier, 1000 + 10 * i + 2),
            ] {
                f.node_mut(n)
                    .unwrap()
                    .set_range(TextRange::new(pos, pos + 1));
            }
            b.nodes.push(node);
            b.replacements.push(replacement);
            let raw = match mode {
                "nil" => NodeSlice::empty(),
                "empty" => f.alloc_nodes(vec![]),
                "nodes" => f.alloc_nodes(vec![Some(node), Some(replacement)]),
                "nil-element" => f.alloc_nodes(vec![Some(node), None]),
                _ => unreachable!(),
            };
            let changed_raw = f.alloc_nodes(vec![Some(replacement)]);
            let list = if mode == "nil" {
                None
            } else {
                Some(f.alloc_list(TextRange::new(10 * i, 10 * i + 9), raw))
            };
            let changed_list = f.alloc_list(
                TextRange::new(1000 + 10 * i, 1000 + 10 * i + 9),
                changed_raw,
            );
            let mods = if mode == "nil" {
                None
            } else {
                let nodes = if mode == "empty" {
                    f.alloc_nodes(vec![])
                } else {
                    f.alloc_nodes(vec![Some(modifier)])
                };
                let id = f.new_modifier_list(nodes);
                f.set_list_location(id, TextRange::new(10 * i, 10 * i + 9))
                    .unwrap();
                Some(id)
            };
            let changed_nodes = f.alloc_nodes(vec![Some(changed_modifier)]);
            let changed_mods = f.new_modifier_list(changed_nodes);
            f.set_list_location(
                changed_mods,
                TextRange::new(1000 + 10 * i, 1000 + 10 * i + 9),
            )
            .unwrap();
            let text = if mode == "nil" {
                TextSlice::empty()
            } else {
                f.text_slice(if mode == "empty" {
                    vec![]
                } else {
                    vec![JsString::from_bytes(format!("text{i}").as_bytes())]
                })
                .unwrap()
            };
            let changed_text = f
                .text_slice(vec![JsString::from_bytes(format!("changed{i}").as_bytes())])
                .unwrap();
            b.raw.push(raw);
            b.changed_raw.push(changed_raw);
            b.lists.push(list);
            b.changed_lists.push(changed_list);
            b.mods.push(mods);
            b.changed_mods.push(changed_mods);
            b.text.push(text);
            b.changed_text.push(changed_text);
        }
        b
    }
    fn node(&self, i: usize, changed: bool) -> Option<NodeId> {
        if changed {
            Some(self.replacements[i])
        } else if self.absent {
            None
        } else {
            Some(self.nodes[i])
        }
    }
    fn list(&self, i: usize, modifier: bool, changed: bool) -> Option<NodeListId> {
        match (modifier, changed) {
            (true, true) => Some(self.changed_mods[i]),
            (true, false) => self.mods[i],
            (false, true) => Some(self.changed_lists[i]),
            (false, false) => self.lists[i],
        }
    }
    fn nodes(&self, i: usize, changed: bool) -> NodeSlice {
        if changed {
            self.changed_raw[i]
        } else {
            self.raw[i]
        }
    }
    fn texts(&self, i: usize, changed: bool) -> TextSlice {
        if changed {
            self.changed_text[i]
        } else {
            self.text[i]
        }
    }
    fn text(i: usize, changed: bool) -> JsString {
        JsString::from_bytes(format!("field{i}-{changed}").as_bytes())
    }
    fn boolean(i: usize, changed: bool) -> bool {
        i.is_multiple_of(2) ^ changed
    }
    fn number(i: usize, changed: bool) -> u32 {
        (1 << (i % 5 + 1)) + u32::from(changed)
    }
    fn kind(kind: NodeKind, changed: bool) -> NodeKind {
        if changed {
            SyntaxKind::MinusToken.into()
        } else {
            kind
        }
    }
}
fn pos(f: &AstBuilder, n: Option<NodeId>) -> Value {
    n.map_or(Value::Null, |n| json!(f.node(n).pos()))
}
fn nodes_snapshot(f: &AstBuilder, nodes: NodeSlice) -> Value {
    json!([
        nodes.is_nil(),
        f.view()
            .node_slice(nodes)
            .unwrap()
            .iter()
            .map(|n| pos(f, n))
            .collect::<Vec<_>>()
    ])
}
fn list_snapshot(f: &AstBuilder, list: Option<NodeListId>) -> Value {
    list.map_or(Value::Null, |id| {
        let l = f.view().list(id).unwrap();
        json!([l.loc().pos(), l.loc().end(), nodes_snapshot(f, l.nodes())])
    })
}
fn texts_snapshot(f: &AstBuilder, texts: TextSlice) -> Value {
    json!([
        texts.is_nil(),
        f.view()
            .text_slice(texts)
            .unwrap()
            .iter()
            .map(|s| String::from_utf8(s.as_bytes().to_vec()).unwrap())
            .collect::<Vec<_>>()
    ])
}
fn snapshot(f: &AstBuilder, id: NodeId, shape: &str) -> Value {
    let n = f.node(id);
    json!([
        n.kind().raw(),
        n.flags(),
        n.pos(),
        n.end(),
        dispatch::fields(f, id, shape)
    ])
}

pub fn run(request: &Value) -> Result<Value, String> {
    let shape = request["shape"].as_str().ok_or("missing shape")?;
    let mode = request["mode"].as_str().ok_or("missing mode")?;
    if !matches!(mode, "nil" | "empty" | "nodes" | "nil-element") {
        return Err("unknown shape input mode".into());
    }
    if request["operation"] != format!("tsc/internal/ast/ast_generated.go:NodeFactory.New{shape}") {
        return Err("shape operation identity mismatch".into());
    }
    let labels = dispatch::action_labels(shape);
    let expected = labels
        .iter()
        .map(|(op, _)| json!({"op":op}))
        .collect::<Vec<_>>();
    if request["actions"].as_array() != Some(&expected) {
        return Err("shape action schedule mismatch".into());
    }
    let hooks = Arc::new(Hooks::default());
    let mut f = AstBuilder::with_hooks(
        SourceText::from_loaded_bytes(&b""[..]),
        &Counters::new(),
        hooks.clone(),
    );
    let inputs = Inputs::new(&mut f, mode);
    hooks.take();
    let root = dispatch::make(&mut f, &inputs, shape);
    let mut out = vec![json!(["new", snapshot(&f, root, shape), hooks.take()])];
    f.node_mut(root)
        .unwrap()
        .set_range(TextRange::new(333, 444));
    let flags = f.node(root).flags();
    f.node_mut(root).unwrap().set_flags(flags | 128);
    for &(label, field) in &labels[1..] {
        let row = match label {
            "cast" => json!([label, dispatch::fields(&f, root, shape)]),
            "name" => json!([label, pos(&f, f.node(root).name())]),
            "children-stop" => json!([label, children(&f, root, 0), children(&f, root, 1)]),
            "clone" => {
                let cloned = f
                    .clone_node_generated(root)
                    .ok_or("missing clone dispatch")?;
                json!([
                    label,
                    cloned == root,
                    snapshot(&f, cloned, shape),
                    hooks.take()
                ])
            }
            "visit-same" | "visit-replace" => {
                let calls = RefCell::new(Vec::<Value>::new());
                let replacement = label == "visit-replace";
                let callback = |v: &mut NodeVisitor<'_>, node: Option<NodeId>| {
                    calls
                        .borrow_mut()
                        .push(node.map_or(Value::Null, |n| json!(v.factory().node(n).pos())));
                    if replacement {
                        node.and_then(|n| {
                            inputs
                                .nodes
                                .iter()
                                .position(|&original| original == n)
                                .map(|i| inputs.replacements[i])
                                .or(Some(n))
                        })
                    } else {
                        node
                    }
                };
                let visited =
                    NodeVisitor::new(Some(&callback), Some(&mut f), NodeVisitorHooks::default())
                        .visit_each_child(Some(root))
                        .ok_or("visitor removed root")?;
                json!([
                    label,
                    visited == root,
                    calls.into_inner(),
                    snapshot(&f, visited, shape),
                    hooks.take()
                ])
            }
            "facts" => json!([label, f.view().subtree_facts(root)]),
            "counts" => json!([label, f.node_count(), f.text_count()]),
            _ if label.starts_with("update-") => {
                let updated = dispatch::update(&mut f, &inputs, root, shape, field);
                json!([
                    label,
                    updated == root,
                    snapshot(&f, updated, shape),
                    hooks.take()
                ])
            }
            _ => return Err("unhandled generated shape action".into()),
        };
        out.push(row);
    }
    Ok(json!({"ordered":out}))
}

pub fn special(request: &Value) -> Result<Value, String> {
    let shape = request["shape"].as_str().ok_or("missing special shape")?;
    let mode = request["mode"].as_str().ok_or("missing special mode")?;
    let labels = match shape {
        "SourceFile" => vec![
            "new",
            "cast",
            "children-stop",
            "visit-same",
            "visit-replace",
        ],
        "SyntheticExpression" => vec!["cast", "children-stop"],
        "ModifierList" => vec!["new", "clone"],
        _ => return Err("unknown special shape".into()),
    };
    let operation = match shape {
        "SourceFile" => "tsc/internal/ast/ast.go:NodeFactory.NewSourceFile",
        "SyntheticExpression" => "tsc/internal/ast/ast_generated.go:Node.AsSyntheticExpression",
        _ => "tsc/internal/ast/ast.go:NodeFactory.NewModifierList",
    };
    if request["operation"] != operation
        || request["actions"] != json!(labels.iter().map(|op| json!({"op":op})).collect::<Vec<_>>())
    {
        return Err("special shape request identity or actions changed".into());
    }
    if !matches!(mode, "nil" | "empty" | "nodes" | "nil-element") {
        return Err("unknown special mode".into());
    }
    let mut f = AstBuilder::new(SourceText::from_loaded_bytes(&b""[..]), &Counters::new());
    if shape == "ModifierList" {
        return modifier_list_special(&mut f, mode);
    }
    let inputs = Inputs::new(&mut f, mode);
    if shape == "SourceFile" {
        let source = f.new_source_file(
            tsr_ast::SourceFileParseOptions {
                file_name: JsString::from_bytes(&b"/generated.ts"[..]),
                path: JsString::from_bytes(&b"/canonical/generated.ts"[..]),
                external_module_indicator_options: tsr_ast::ExternalModuleIndicatorOptions {
                    jsx: true,
                    force: false,
                },
            },
            SourceText::from_loaded_bytes(&b"let text = 'source';\n"[..]),
            inputs.list(0, false, false),
            inputs.node(1, false),
        );
        let fields = |f: &AstBuilder, id: NodeId| -> Result<Value, String> {
            let d = f.node(id);
            let syntax = d.as_source_file().ok_or("SourceFile payload")?;
            let state = f.view().source_file(id).map_err(|e| e.to_string())?;
            let options = state.parse_options();
            Ok(json!([
                String::from_utf8(state.file_name().to_vec()).unwrap(),
                String::from_utf8(options.path.as_bytes().to_vec()).unwrap(),
                options.external_module_indicator_options.jsx,
                options.external_module_indicator_options.force,
                String::from_utf8(state.text().as_bytes().to_vec()).unwrap(),
                list_snapshot(f, syntax.statements()),
                pos(f, syntax.end_of_file_token())
            ]))
        };
        let kind = f.node(source).kind().raw();
        let base = fields(&f, source)?;
        let mut out = vec![
            json!(["new", kind, base.clone()]),
            json!(["cast", base]),
            json!([
                "children-stop",
                children(&f, source, 0),
                children(&f, source, 1)
            ]),
        ];
        for label in ["visit-same", "visit-replace"] {
            // The visitor enters through VisitSourceFile; the callback descends
            // once, into the file's own VisitEachChild, and records or replaces
            // what that visit hands it, as the shape runs do for their root.
            let calls = RefCell::new(Vec::<Value>::new());
            let replacement = label == "visit-replace";
            let callback = |v: &mut NodeVisitor<'_>, node: Option<NodeId>| {
                calls
                    .borrow_mut()
                    .push(node.map_or(Value::Null, |n| json!(v.factory().node(n).pos())));
                if node == Some(source) {
                    return v.visit_each_child(node);
                }
                if replacement {
                    node.and_then(|n| {
                        inputs
                            .nodes
                            .iter()
                            .position(|&original| original == n)
                            .map(|i| inputs.replacements[i])
                            .or(Some(n))
                    })
                } else {
                    node
                }
            };
            let visited =
                NodeVisitor::new(Some(&callback), Some(&mut f), NodeVisitorHooks::default())
                    .visit_source_file(source);
            let n = f.node(visited);
            let snapshot = json!([
                n.kind().raw(),
                n.flags(),
                n.pos(),
                n.end(),
                fields(&f, visited)?
            ]);
            out.push(json!([
                label,
                visited == source,
                calls.into_inner(),
                snapshot
            ]));
        }
        Ok(json!({"ordered": out}))
    } else {
        let root = f.new_node(
            SyntaxKind::SyntheticExpression.into(),
            tsr_ast::SyntheticExpressionData {
                is_spread: mode != "nil",
                tuple_name_source: inputs.node(0, false),
            }
            .into(),
        );
        let n = f.node(root);
        let d = n.as_synthetic_expression().unwrap();
        Ok(
            json!({"ordered":[["cast",d.is_spread(),pos(&f,d.tuple_name_source())],["children-stop",children(&f,root,0),children(&f,root,1)]]}),
        )
    }
}

/// A modifier list built by the factory from the mode's tokens (a nil element
/// is not a modifier: `modifiers_to_flags` reads its kind), then its factory
/// clone: `[pos, end, flags, nodes]` of each list and whether the clone is the
/// same list.
fn modifier_list_special(f: &mut AstBuilder, mode: &str) -> Result<Value, String> {
    let raw = match mode {
        "nil" => NodeSlice::empty(),
        "empty" => f.alloc_nodes(vec![]),
        "nodes" => {
            let export = f.new_token(SyntaxKind::ExportKeyword.into());
            let r#async = f.new_token(SyntaxKind::AsyncKeyword.into());
            f.node_mut(export).unwrap().set_range(TextRange::new(2, 3));
            f.node_mut(r#async)
                .unwrap()
                .set_range(TextRange::new(12, 13));
            f.alloc_nodes(vec![Some(export), Some(r#async)])
        }
        _ => return Err("unsupported ModifierList mode".into()),
    };
    let list = f.new_modifier_list(raw);
    f.set_list_location(list, TextRange::new(5, 25)).unwrap();
    let snapshot = |f: &AstBuilder, id: NodeListId| {
        let l = f.view().list(id).unwrap();
        json!([
            l.loc().pos(),
            l.loc().end(),
            l.modifier_flags(),
            nodes_snapshot(f, l.nodes())
        ])
    };
    let cloned = f.clone_modifier_list_header(list);
    Ok(
        json!({"ordered": [["new", snapshot(f, list)], ["clone", cloned == list, snapshot(f, cloned)]]}),
    )
}
