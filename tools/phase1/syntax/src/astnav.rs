//! `astnav` group (F4a plan task 6). Each request is one source file and an
//! ordered list of navigation actions, answered by the production
//! `tsr_astnav::Navigator` over a file parsed by the production parser. The
//! answers use the probe's shapes: a sweep is run-length encoded over every
//! byte offset from 0 to the text length inclusive, a node is [kind, pos, end,
//! ordinal] with the ordinal assigned on first sight so identity is compared,
//! and an upstream panic is ["panic", message].

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use serde_json::{json, Value};
use tsr_arena::NodeId;
use tsr_ast::{AstView, ChildVisitor, NodeListId, NodeSlice, SourceFileParseOptions, SyntaxKind};
use tsr_astnav::{ChildVisit, Error as NavError, Navigator};
use tsr_jsstring::SourceText;

use crate::api::{subject, Outcome};

const SUBJECT: &str = "astnav";

pub fn observe(request: &Value) -> Option<Outcome> {
    (subject(request) == SUBJECT).then(|| match run(request) {
        Ok(outcome) => outcome,
        Err(error) => Outcome::Failed(error),
    })
}

fn actions(request: &Value) -> Vec<&Value> {
    request
        .get("actions")
        .and_then(Value::as_array)
        .map(|actions| actions.iter().collect())
        .unwrap_or_default()
}

fn op(action: &Value) -> &str {
    action.get("op").and_then(Value::as_str).unwrap_or_default()
}

struct Answers<'n, 'a, 'p> {
    nav: &'n mut Navigator<'a, 'p>,
    view: AstView<'a>,
    ordinals: HashMap<NodeId, usize>,
}

impl Answers<'_, '_, '_> {
    fn node(&mut self, id: Option<NodeId>) -> Result<Value, tsr_arena::Error> {
        let Some(id) = id else {
            return Ok(Value::Null);
        };
        let next = self.ordinals.len();
        let ordinal = *self.ordinals.entry(id).or_insert(next);
        let node = self.view.node(id)?;
        Ok(json!([node.kind().raw(), node.pos(), node.end(), ordinal]))
    }

    /// One question: an upstream assertion becomes its pinned panic text; a
    /// storage error is a harness failure; a Rust panic is kept as a
    /// difference, never swallowed.
    fn answer(
        &mut self,
        question: impl FnOnce(&mut Self) -> Result<Value, NavError>,
    ) -> Result<Value, String> {
        match catch_unwind(AssertUnwindSafe(|| question(self))) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(NavError::Assertion(message))) => Ok(json!(["panic", message])),
            Ok(Err(NavError::Storage(error))) => Err(format!("storage error: {error:?}")),
            Err(payload) => Ok(json!([
                "rust_panic",
                payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_default()
            ])),
        }
    }

    fn sweep(
        &mut self,
        length: i64,
        mut question: impl FnMut(&mut Self, i64) -> Result<Value, NavError>,
    ) -> Result<Value, String> {
        let mut runs = Vec::new();
        let mut start = 0;
        let mut previous = Value::Null;
        for position in 0..=length {
            let value = self.answer(|this| question(this, position))?;
            if position > 0 && value == previous {
                continue;
            }
            if position > 0 {
                runs.push(json!([start, position - 1, previous]));
            }
            start = position;
            previous = value;
        }
        runs.push(json!([start, length, previous]));
        Ok(Value::Array(runs))
    }

    fn token(&mut self, position: i64) -> Result<NodeId, NavError> {
        self.nav.get_token_at_position(position)
    }

    fn found(&mut self, id: Option<NodeId>) -> Result<Value, NavError> {
        Ok(self.node(id)?)
    }
}

/// Every node `ForEachChild` reaches from the file, parents first.
pub(crate) fn preorder(view: AstView<'_>, root: NodeId) -> Result<Vec<NodeId>, String> {
    struct Children<'v> {
        view: AstView<'v>,
        out: Vec<NodeId>,
        error: Option<String>,
    }
    impl ChildVisitor for Children<'_> {
        fn visit_node(&mut self, node: NodeId) -> std::ops::ControlFlow<()> {
            self.out.push(node);
            std::ops::ControlFlow::Continue(())
        }
        fn visit_list(&mut self, list: NodeListId) -> std::ops::ControlFlow<()> {
            match self.view.list(list) {
                Ok(list) => self.visit_node_slice(list.nodes()),
                Err(error) => {
                    self.error = Some(format!("{error:?}"));
                    std::ops::ControlFlow::Break(())
                }
            }
        }
        fn visit_node_slice(&mut self, nodes: NodeSlice) -> std::ops::ControlFlow<()> {
            match self.view.node_slice(nodes) {
                Ok(nodes) => {
                    self.out.extend(nodes.iter().flatten());
                    std::ops::ControlFlow::Continue(())
                }
                Err(error) => {
                    self.error = Some(format!("{error:?}"));
                    std::ops::ControlFlow::Break(())
                }
            }
        }
    }
    let mut order = Vec::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        order.push(node);
        let mut children = Children {
            view,
            out: Vec::new(),
            error: None,
        };
        let _ = view
            .node(node)
            .map_err(|e| format!("{e:?}"))?
            .for_each_child(&mut children);
        if let Some(error) = children.error {
            return Err(error);
        }
        pending.extend(children.out.into_iter().rev());
    }
    Ok(order)
}

fn kinds_by_name() -> HashMap<String, SyntaxKind> {
    (0..=u16::from(u8::MAX) * 4)
        .filter_map(SyntaxKind::from_u16)
        .map(|kind| (format!("Kind{kind:?}"), kind))
        .collect()
}

fn run(request: &Value) -> Result<Outcome, String> {
    if actions(request)
        .iter()
        .any(|action| op(action) == "visit_lists")
    {
        // The public visitor resolves each list to its members
        // (ChildVisit::List(Vec<NodeId>)), so a caller cannot read the list's
        // own range, which upstream hands its visitNodes hook; and it reports
        // no call for an absent child, where upstream calls visitNode(nil) and
        // visitNodes(nil) for every empty slot.
        return Ok(Outcome::missing(
            "tsc/internal/astnav/tokens.go:VisitEachChildAndJSDoc",
            "upstream/tsc/internal/astnav/tokens.go:311 passes the *ast.NodeList to visitNodes \
             (ls/utilities.go findContainingList reads its position), and ast/visitor.go passes \
             absent children through as nil hook calls",
            "Navigator::visit_each_child_and_jsdoc(&mut self, id: NodeId) -> Result<Vec<Visit>, Error>, \
             with Visit::List(NodeListId) keeping the list range and an absent-slot visit",
            "crates/tsr_astnav/src/lib.rs",
        ));
    }
    let file_spec = request.get("file").ok_or("request has no file")?;
    let name = file_spec["name"].as_str().ok_or("file has no name")?;
    let text = file_spec["text"].as_str().ok_or("file has no text")?;
    let kind = file_spec["script_kind"]
        .as_i64()
        .ok_or("file has no script kind")?;
    let file = tsr_parser::parse_source_file(
        SourceText::from_loaded_bytes(text.as_bytes().to_vec()),
        tsr_core::ScriptKind(i32::try_from(kind).map_err(|e| e.to_string())?),
        SourceFileParseOptions {
            file_name: tsr_ast::JsString::from_bytes(name.as_bytes()),
            path: tsr_ast::JsString::from_bytes(name.as_bytes()),
            ..Default::default()
        },
    )
    .publish_unbound();
    let root = file.root().ok_or("parsed file has no root")?;
    let view = file.view();
    let mut provider = tsr_parser::ParserJsDocProvider::default();
    let mut nav = Navigator::new(view, root, &mut provider);
    let mut answers = Answers {
        nav: &mut nav,
        view,
        ordinals: HashMap::new(),
    };
    let length = i64::try_from(text.len()).map_err(|e| e.to_string())?;
    let names = kinds_by_name();
    let mut ordered = Vec::new();
    for action in actions(request) {
        let value = match op(action) {
            "token_at" | "token_at_repeat" => answers.sweep(length, |a, p| {
                let token = a.token(p)?;
                a.found(Some(token))
            })?,
            "touching_property_name" => answers.sweep(length, |a, p| {
                let token = a.nav.get_touching_property_name(p)?;
                a.found(Some(token))
            })?,
            "touching_token" => answers.sweep(length, |a, p| {
                let token = a.nav.get_touching_token(p)?;
                a.found(Some(token))
            })?,
            "preceding" => answers.sweep(length, |a, p| {
                let token = a.nav.find_preceding_token(p)?;
                a.found(token)
            })?,
            "preceding_exclude_jsdoc" => answers.sweep(length, |a, p| {
                let token = a.nav.find_preceding_token_ex(p, None, true)?;
                a.found(token)
            })?,
            "next_in_file" => answers.sweep(length, |a, p| {
                let previous = a.token(p)?;
                let next = a.nav.find_next_token(previous, root)?;
                a.found(next)
            })?,
            "next_in_parent" => answers.sweep(length, |a, p| {
                let previous = a.token(p)?;
                let Some(parent) = a.view.node(previous)?.parent() else {
                    return Ok(json!(["no_parent"]));
                };
                let next = a.nav.find_next_token(previous, parent)?;
                a.found(next)
            })?,
            "start_of_token" => answers.sweep(length, |a, p| {
                let token = a.token(p)?;
                Ok(json!(a.nav.get_start_of_node(token, false)?))
            })?,
            "start_of_token_with_jsdoc" => answers.sweep(length, |a, p| {
                let token = a.token(p)?;
                Ok(json!(a.nav.get_start_of_node(token, true)?))
            })?,
            "child_of_kind" => {
                let wanted: Vec<&str> = action
                    .get("kinds")
                    .and_then(Value::as_array)
                    .map(|kinds| kinds.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let mut rows = Vec::new();
                for (index, container) in preorder(view, root)?.into_iter().enumerate() {
                    for name in &wanted {
                        let kind = *names
                            .get(*name)
                            .ok_or_else(|| format!("unknown kind {name}"))?;
                        let found = answers.answer(|a| {
                            let found = a.nav.find_child_of_kind(container, kind)?;
                            a.found(found)
                        })?;
                        rows.push(json!([index, name, found]));
                    }
                }
                Value::Array(rows)
            }
            "visit_nodes" => {
                let mut rows = Vec::new();
                for (index, node) in preorder(view, root)?.into_iter().enumerate() {
                    let visits = answers
                        .nav
                        .visit_each_child_and_jsdoc(node)
                        .map_err(|e| format!("{e:?}"))?;
                    let mut out = Vec::new();
                    for visit in visits {
                        out.push(match visit {
                            ChildVisit::Node(child) => json!([
                                "node",
                                answers.node(Some(child)).map_err(|e| format!("{e:?}"))?
                            ]),
                            ChildVisit::List(members) => {
                                let members = members
                                    .into_iter()
                                    .map(|child| {
                                        answers.node(Some(child)).map_err(|e| format!("{e:?}"))
                                    })
                                    .collect::<Result<Vec<_>, _>>()?;
                                json!(["list", members])
                            }
                        });
                    }
                    rows.push(json!([index, out]));
                }
                Value::Array(rows)
            }
            other => return Err(format!("unknown astnav action {other:?}")),
        };
        ordered.push(value);
    }
    let seen = answers.ordinals.len();
    Ok(Outcome::Observed(
        json!({"ordered": ordered, "nodes_seen": seen}),
    ))
}
