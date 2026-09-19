//! Frozen factory witness scripts, calling production Rust APIs only.
use std::fmt::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use ts_arena::Counters;
use ts_ast::{
    clone_node, deep_clone_node, deep_clone_reparse, get_binary_operator_precedence, node_flags,
    AstBuilder, CheckJsDirective, CommentRange, Factory, FactoryHooks, FactoryMethods, JsString,
    NodeId, NodeKind, NodeSlice, NodeVisitor, NodeVisitorHooks, SourceFileParseOptions, SourceHash,
    SyntaxKind,
};
use ts_core::{ScriptKind, TextRange};
use ts_jsstring::SourceText;

#[derive(Debug)]
pub struct Witness {
    pub label: &'static str,
    pub ints: Vec<i64>,
    pub bools: Vec<bool>,
    pub strings: Vec<String>,
}
pub type Sink = Arc<dyn Fn(Witness) + Send + Sync>;
fn emit(s: &Sink, label: &'static str, ints: Vec<i64>, bools: Vec<bool>, strings: Vec<String>) {
    s(Witness {
        label,
        ints,
        bools,
        strings,
    });
}
fn b() -> AstBuilder {
    AstBuilder::new(SourceText::default(), &Counters::new())
}
fn ident(b: &mut AstBuilder, text: &[u8]) -> NodeId {
    b.new_identifier(JsString::from_bytes(text))
}
fn i(value: usize) -> i64 {
    i64::try_from(value).expect("fixture count fits Go int")
}
fn capture(action: impl FnOnce()) -> (String, String) {
    match catch_unwind(AssertUnwindSafe(action)) {
        Ok(()) => ("ok".into(), String::new()),
        Err(error) => {
            let message = error
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| error.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic".into());
            ("panic".into(), message)
        }
    }
}
fn identity(_: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
    node
}
fn options(name: &[u8]) -> SourceFileParseOptions {
    SourceFileParseOptions {
        file_name: JsString::from_bytes(name),
        ..SourceFileParseOptions::default()
    }
}
struct Hooks {
    sink: Sink,
    ids: Mutex<Vec<NodeId>>,
    nested: Mutex<bool>,
}
impl Hooks {
    fn id(&self, node: NodeId) -> i64 {
        i(self
            .ids
            .lock()
            .unwrap()
            .iter()
            .position(|&id| id == node)
            .expect("allocated fixture node")
            + 1)
    }
    fn event(&self, name: &'static str, f: &dyn Factory, node: NodeId, original: Option<NodeId>) {
        let n = f.node(node);
        emit(
            &self.sink,
            name,
            vec![
                self.id(node),
                original.map_or(0, |id| self.id(id)),
                i64::from(n.kind().raw()),
                i64::from(n.flags()),
                i64::from(n.pos()),
                i64::from(n.end()),
                f.node_count(),
                f.text_count(),
            ],
            vec![],
            vec![],
        );
    }
}
impl FactoryHooks for Hooks {
    fn on_create(&self, f: &mut dyn Factory, node: NodeId) {
        self.ids.lock().unwrap().push(node);
        self.event("create", f, node, None);
        f.set_node_flags(node, u32::MAX);
        let recurse = {
            let mut nested = self.nested.lock().unwrap();
            if !*nested && f.node(node).kind() == SyntaxKind::Identifier {
                *nested = true;
                true
            } else {
                false
            }
        };
        if recurse {
            f.new_token(SyntaxKind::Unknown.into());
        }
    }
    fn on_update(&self, f: &mut dyn Factory, node: NodeId, original: NodeId) {
        self.event("update", f, node, Some(original));
        f.node_mut(original).set_range(TextRange::new(20, 21));
    }
    fn on_clone(&self, f: &mut dyn Factory, node: NodeId, original: NodeId) {
        self.event("clone", f, node, Some(original));
    }
}

pub fn run(scenario: &str, s: &Sink) {
    let mut f = b();
    match scenario {
        "source-file-hooks" => source_file_hooks(s),
        "hooks-counts-update-clone" => {
            let hooks = Arc::new(Hooks {
                sink: s.clone(),
                ids: Mutex::new(vec![]),
                nested: Mutex::new(false),
            });
            let mut f =
                AstBuilder::with_hooks(SourceText::default(), &Counters::new(), hooks.clone());
            let name = ident(&mut f, b"x");
            f.node_mut(name).unwrap().set_range(TextRange::new(1, 2));
            f.set_node_flags(name, 3);
            let dot = f.new_token(SyntaxKind::QuestionDotToken.into());
            let access = f.new_property_access_expression(
                Some(name),
                Some(dot),
                Some(name),
                node_flags::OPTIONAL_CHAIN,
            );
            f.node_mut(access).unwrap().set_range(TextRange::new(3, 9));
            f.set_node_flags(access, 7);
            emit(
                s,
                "constructed",
                vec![
                    hooks.id(name),
                    hooks.id(access),
                    i64::from(f.node(access).flags()),
                    f.node_count(),
                    f.text_count(),
                ],
                vec![],
                vec![],
            );
            let flags = f.node(access).flags();
            let same = f.update_property_access_expression(
                access,
                Some(name),
                Some(dot),
                Some(name),
                flags,
            );
            let cloned = clone_node(&mut f, access);
            emit(
                s,
                "result",
                vec![
                    hooks.id(cloned),
                    i64::from(f.node(cloned).pos()),
                    i64::from(f.node(cloned).end()),
                    i64::from(f.node(access).pos()),
                    i64::from(f.node(access).end()),
                    f.node_count(),
                    f.text_count(),
                ],
                vec![same == access],
                vec![],
            );
        }
        "raw-slice-same" => {
            let a = ident(&mut f, b"a");
            let raw = f.node_slice(vec![Some(a), None]).unwrap();
            let original = f.new_syntax_list(raw);
            let same = f.update_syntax_list(original, raw);
            let copy = f.node_slice(vec![Some(a), None]).unwrap();
            let copied = f.update_syntax_list(original, copy);
            let empty_raw = f.node_slice(vec![]).unwrap();
            let empty = f.new_syntax_list(empty_raw);
            let same_empty = f.update_syntax_list(empty, NodeSlice::empty());
            emit(
                s,
                "same",
                vec![f.node_count(), f.text_count()],
                vec![
                    same == original,
                    copied == original,
                    same_empty == empty,
                    f.node(empty)
                        .data_source()
                        .as_syntax_list()
                        .unwrap()
                        .children()
                        .is_nil(),
                ],
                vec![],
            );
        }
        "visitor-nil-flatten-disable" => {
            let a = ident(&mut f, b"a");
            let c = ident(&mut f, b"c");
            let expansion = f.node_slice(vec![Some(c), None]).unwrap();
            let syntax = f.new_syntax_list(expansion);
            let input = f.node_slice(vec![None, Some(a), Some(a), Some(c)]).unwrap();
            let calls = std::cell::Cell::new(0);
            let visit = |v: &mut NodeVisitor<'_>, node: Option<NodeId>| {
                calls.set(calls.get() + 1);
                if calls.get() == 3 {
                    v.visit = None;
                }
                if node == Some(a) {
                    Some(syntax)
                } else {
                    node
                }
            };
            let mut v = NodeVisitor::new(Some(&visit), Some(&mut f), NodeVisitorHooks::default());
            let (nodes, changed) = v.visit_slice(input);
            let mut values = vec![calls.get(), i(nodes.len())];
            values.extend(v.factory().read_nodes(nodes).iter().map(|n| {
                if n.is_none() {
                    0
                } else if n == Some(c) {
                    2
                } else {
                    1
                }
            }));
            emit(s, "slice", values, vec![changed, nodes.is_nil()], vec![]);
            let (nodes, changed) = v.visit_slice(NodeSlice::empty());
            emit(
                s,
                "nil",
                vec![i(nodes.len())],
                vec![changed, nodes.is_nil()],
                vec![],
            );
        }
        "visitor-lift-contracts" => {
            let a = ident(&mut f, b"a");
            let one = f.node_slice(vec![Some(a)]).unwrap();
            let syntax = f.new_syntax_list(one);
            let input = f.node_slice(vec![Some(syntax)]).unwrap();
            let mut v =
                NodeVisitor::new(Some(&identity), Some(&mut f), NodeVisitorHooks::default());
            let (result, changed) = v.visit_slice(input);
            let lifted = v.visit_node(Some(syntax));
            emit(
                s,
                "unchanged",
                vec![i(result.len())],
                vec![
                    changed,
                    v.factory().read_nodes(result).at(0) == Some(syntax),
                    lifted == Some(a),
                ],
                vec![],
            );
            for (name, nodes) in [
                ("empty", vec![]),
                ("nil_child", vec![None]),
                ("nested", vec![Some(syntax)]),
            ] {
                let nodes = if name == "empty" {
                    NodeSlice::empty()
                } else {
                    v.factory_mut().alloc_nodes(nodes)
                };
                let wrapper = v.new_syntax_list(nodes);
                let mut out = None;
                let (outcome, message) = capture(|| {
                    out = v.visit_node(Some(wrapper));
                });
                emit(s, name, vec![], vec![out.is_none()], vec![outcome, message]);
            }
        }
        "visitor-role-hooks" => {
            let token = f.new_token(SyntaxKind::ThisKeyword.into());
            let expr = ident(&mut f, b"x");
            let loop_node = f.new_while_statement(Some(expr), Some(token));
            let remove = |_: &mut NodeVisitor<'_>, _: Option<NodeId>| None;
            let hooks = NodeVisitorHooks {
                visit_node: Some(&remove),
                ..NodeVisitorHooks::default()
            };
            let mut v = NodeVisitor::new(Some(&identity), Some(&mut f), hooks);
            let direct = v.visit_embedded_statement(Some(token));
            let updated = v.visit_each_child(Some(loop_node)).unwrap();
            let node = v.node(updated);
            let data = node.data_source().as_while_statement().unwrap();
            let statement = data.statement().unwrap();
            let expression = data.expression();
            drop(node);
            let list = v
                .node(statement)
                .data_source()
                .as_block()
                .unwrap()
                .statements()
                .unwrap();
            let len = v.factory().read_list(list).nodes().len();
            emit(
                s,
                "embedded",
                vec![i64::from(v.node(statement).kind().raw()), i(len)],
                vec![direct == Some(token), expression.is_none()],
                vec![],
            );
            let access = v.new_property_access_expression(Some(expr), Some(token), Some(expr), 0);
            let rewritten = v.visit_each_child(Some(access)).unwrap();
            let node = v.node(rewritten);
            let data = node.data_source().as_property_access_expression().unwrap();
            emit(
                s,
                "token",
                vec![],
                vec![
                    data.question_dot_token() == Some(token),
                    data.expression().is_none(),
                    data.name().is_none(),
                ],
                vec![],
            );
        }
        "deep-clone-locations-and-parents" => {
            let child = ident(&mut f, b"child");
            f.node_mut(child).unwrap().set_range(TextRange::new(1, 6));
            let edges = f.node_slice(vec![Some(child)]).unwrap();
            let list = f.new_list(TextRange::new(0, 7), edges).unwrap();
            let root = f.new_array_literal_expression(Some(list), false);
            f.node_mut(root).unwrap().set_range(TextRange::new(0, 8));
            // Construct the empty source node before publishing the source owner.
            let empty_list = f
                .new_list(TextRange::new(-1, -1), NodeSlice::empty())
                .unwrap();
            let empty = f.new_array_literal_expression(Some(empty_list), false);
            let source = f.complete(root).unwrap().publish_unbound();
            let mut destination = b();
            destination.retain_file(source);
            let cloned = deep_clone_node(&mut destination, Some(root)).unwrap();
            let cl = destination
                .node(cloned)
                .data_source()
                .as_array_literal_expression()
                .unwrap()
                .elements()
                .unwrap();
            let cc = destination
                .view()
                .node_slice(destination.view().list(cl).unwrap().nodes())
                .unwrap()
                .at(0)
                .unwrap();
            emit(
                s,
                "synthetic",
                vec![
                    i64::from(destination.node(cloned).pos()),
                    i64::from(destination.node(cloned).end()),
                    destination.view().list(cl).unwrap().loc().pos(),
                    destination.view().list(cl).unwrap().loc().end(),
                    i64::from(destination.node(cc).pos()),
                    i64::from(destination.node(cc).end()),
                    i64::from(destination.node(child).pos()),
                    i64::from(destination.node(child).end()),
                ],
                vec![
                    cloned == root,
                    cc == child,
                    destination.view().list_has_trailing_comma(cl).unwrap(),
                    destination.node(cc).parent().is_none(),
                ],
                vec![],
            );
            let reparsed = deep_clone_reparse(&mut destination, Some(root)).unwrap();
            let list = destination
                .node(reparsed)
                .data_source()
                .as_array_literal_expression()
                .unwrap()
                .elements()
                .unwrap();
            let rc = destination
                .view()
                .node_slice(destination.view().list(list).unwrap().nodes())
                .unwrap()
                .at(0)
                .unwrap();
            emit(
                s,
                "reparse",
                vec![
                    i64::from(destination.node(reparsed).pos()),
                    i64::from(destination.node(reparsed).end()),
                    i64::from(destination.node(rc).pos()),
                    i64::from(destination.node(rc).end()),
                    i64::from(destination.node(reparsed).flags()),
                ],
                vec![
                    destination.node(rc).parent() == Some(reparsed),
                    destination.node(reparsed).parent().is_none(),
                ],
                vec![],
            );
            let empty_clone = deep_clone_node(&mut destination, Some(empty)).unwrap();
            let cloned_list = destination
                .node(empty_clone)
                .data_source()
                .as_array_literal_expression()
                .unwrap()
                .elements()
                .unwrap();
            emit(
                s,
                "empty",
                vec![],
                vec![
                    empty_clone != empty,
                    cloned_list != empty_list,
                    destination
                        .view()
                        .list(cloned_list)
                        .unwrap()
                        .nodes()
                        .is_nil(),
                ],
                vec![],
            );
        }
        "subtree-cache-and-exclusions" => {
            let this = f.new_keyword_expression(SyntaxKind::ThisKeyword.into());
            let identifier = ident(&mut f, b"x");
            let outer = f.new_computed_property_name(Some(this));
            let before = f.view().subtree_facts(outer);
            if let ts_ast::NodeData::ComputedPropertyName(data) =
                f.node_mut(outer).unwrap().data_mut()
            {
                data.expression = Some(identifier);
            }
            let after = f.view().subtree_facts(outer);
            let cloned = clone_node(&mut f, outer);
            emit(
                s,
                "cache",
                vec![
                    i64::from(before),
                    i64::from(after),
                    i64::from(f.view().subtree_facts(cloned)),
                ],
                vec![],
                vec![],
            );
            let awaited = f.new_await_expression(Some(this));
            let function =
                f.new_function_expression(None, None, None, None, None, None, None, Some(awaited));
            let arrow = f.new_arrow_function(None, None, None, None, None, None, Some(awaited));
            let parent_function = f.new_computed_property_name(Some(function));
            let parent_arrow = f.new_computed_property_name(Some(arrow));
            emit(
                s,
                "scope",
                vec![
                    i64::from(f.view().subtree_facts(function)),
                    i64::from(f.view().subtree_facts(parent_function)),
                    i64::from(f.view().subtree_facts(arrow)),
                    i64::from(f.view().subtree_facts(parent_arrow)),
                ],
                vec![],
                vec![],
            );
            let malformed = f.new_call_expression(None, None, None, None, 0);
            let declaration = f.new_variable_declaration(None, None, Some(malformed), None);
            emit(
                s,
                "erased",
                vec![i64::from(f.view().subtree_facts(declaration))],
                vec![],
                vec![],
            );
            let (outcome, message) = capture(|| {
                f.view().subtree_facts(malformed);
            });
            let class = if outcome == "panic" && message == "nil node in subtree kind predicate" {
                "nil-dereference".to_owned()
            } else {
                message
            };
            emit(s, "invalid", vec![], vec![], vec![outcome, class]);
        }
        "token-subtree-and-precedence" => {
            for raw in [-32768, -1]
                .into_iter()
                .chain(0..=i16::try_from(SyntaxKind::COUNT).unwrap())
                .chain([32767])
            {
                let kind = NodeKind::from_raw(raw);
                let token = f.new_token(kind);
                emit(
                    s,
                    "token",
                    vec![
                        i64::from(raw),
                        i64::from(f.view().subtree_facts(token)),
                        i64::from(get_binary_operator_precedence(kind)),
                    ],
                    vec![],
                    vec![],
                );
            }
        }
        "source-file-clone-omissions" => {
            let eof = f.new_token(SyntaxKind::EndOfFile.into());
            let node = f.new_source_file(
                options(b"/fixture.ts"),
                SourceText::from_loaded_bytes(&b"x\xff"[..]),
                None,
                Some(eof),
            );
            f.node_mut(node).unwrap().set_range(TextRange::new(0, 2));
            f.set_node_flags(node, 7);
            let sf = f.source_file_mut(node).unwrap();
            sf.script_kind = ScriptKind::TS;
            sf.identifier_count = 3;
            sf.node_count = 7;
            sf.text_count = 11;
            sf.hash = SourceHash { hi: 13, lo: 17 };
            sf.check_js_directive = Some(CheckJsDirective {
                enabled: true,
                range: CommentRange::default(),
            });
            sf.external_module_indicator = Some(node);
            sf.has_lazy_jsdoc = true;
            let doc = f.new_token(SyntaxKind::Unknown.into());
            let eof_flags = f.node(eof).flags() | node_flags::HAS_JS_DOC;
            f.set_node_flags(eof, eof_flags);
            f.seed_source_jsdoc(node, eof, vec![doc]).unwrap();
            let unchanged = f.update_source_file(node, None, Some(eof));
            let cloned = clone_node(&mut f, node);
            let cn = f.node(cloned);
            let sf = f.view().source_file(cloned).unwrap();
            emit(
                s,
                "source",
                vec![
                    i64::from(cn.kind().raw()),
                    i64::from(cn.flags()),
                    i64::from(cn.pos()),
                    i64::from(cn.end()),
                    i64::from(sf.script_kind.0),
                    sf.identifier_count,
                    sf.node_count,
                    sf.text_count,
                    sf.hash.hi as i64,
                    sf.hash.lo as i64,
                ],
                vec![
                    unchanged == node,
                    sf.check_js_directive.is_none(),
                    sf.external_module_indicator == Some(node),
                    cn.data_source()
                        .as_source_file()
                        .unwrap()
                        .end_of_file_token()
                        == Some(eof),
                ],
                vec![
                    hex(sf.text().as_bytes()),
                    String::from_utf8(sf.parse_options().file_name.as_bytes().to_vec()).unwrap(),
                ],
            );

            let original_docs = f.view().source_eager_jsdoc(node, eof).unwrap();
            let cloned_docs = f.view().source_eager_jsdoc(cloned, eof).unwrap();
            emit(
                s,
                "source-jsdoc-cache",
                vec![
                    original_docs.as_ref().map_or(0, |docs| i(docs.len())),
                    cloned_docs.as_ref().map_or(0, |docs| i(docs.len())),
                ],
                vec![original_docs
                    .as_ref()
                    .is_some_and(|docs| docs.len() == 1 && docs[0] == doc)],
                vec![],
            );
        }
        "node-index-nil-after-sort" => {
            let a = f.new_token(SyntaxKind::Unknown.into());
            let b = f.new_token(SyntaxKind::Unknown.into());
            let c = f.new_token(SyntaxKind::Unknown.into());
            let table =
                ts_ast::NodeIndexCache::new(vec![None, Some(a), Some(b), Some(a), Some(c), None]);
            let (outcome, message) = capture(|| {
                table.get_index(f.view(), None).unwrap();
            });
            eprintln!("factory node-index nil witness: {message}");
            let class = if outcome == "panic" && message == "nil node in NodeIndexTable.GetIndex" {
                "nil-query-after-sort".to_owned()
            } else {
                message
            };
            emit(s, "nil-query", vec![], vec![], vec![outcome, class]);
            let ai = ts_ast::runtime_node_id(&f.node(a));
            let bi = ts_ast::runtime_node_id(&f.node(b));
            let ci = ts_ast::runtime_node_id(&f.node(c));
            emit(
                s,
                "after-sort",
                vec![
                    i64::from(table.get_index(f.view(), Some(&f.node(a))).unwrap()),
                    i64::from(table.get_index(f.view(), Some(&f.node(b))).unwrap()),
                    i64::from(table.get_index(f.view(), Some(&f.node(c))).unwrap()),
                ],
                vec![ai < bi, ai < ci, bi < ci],
                vec![],
            );
            let absent = f.new_token(SyntaxKind::Unknown.into());
            emit(
                s,
                "absent",
                vec![i64::from(
                    table.get_index(f.view(), Some(&f.node(absent))).unwrap(),
                )],
                vec![],
                vec![],
            );
        }

        "source-cache-panic-once" => {
            let sf = f.new_source_file(options(b"/cache.ts"), SourceText::default(), None, None);
            let calls = std::cell::Cell::new(0);
            let (outcome, message) = capture(|| {
                f.view().source_file(sf).unwrap().node_index_cache(|| {
                    calls.set(calls.get() + 1);
                    panic!("source initializer failed")
                });
            });
            emit(
                s,
                "first",
                vec![calls.get()],
                vec![],
                vec![outcome, message],
            );
            let state = f.view().source_file(sf).unwrap();
            let result = state.node_index_cache(|| {
                calls.set(calls.get() + 1);
                ts_ast::NodeIndexCache::new(vec![])
            });
            emit(
                s,
                "second",
                vec![calls.get()],
                vec![result.is_none()],
                vec![],
            );
        }
        _ => panic!("unknown frozen factory scenario"),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(result, "{byte:02x}").expect("writing into String");
    }
    result
}

struct SourceFileHooks {
    sink: Sink,
    phase: Mutex<i64>,
    original: Mutex<Option<NodeId>>,
}
impl SourceFileHooks {
    fn observe(&self, label: &'static str, factory: &dyn Factory, node: NodeId) {
        let state = factory.read_source_file(node).unwrap();
        let header = factory.node(node);
        emit(
            &self.sink,
            label,
            vec![
                *self.phase.lock().unwrap(),
                i64::from(state.script_kind.0),
                state.identifier_count,
                state.node_count,
                state.text_count,
                i64::try_from(state.hash.hi).unwrap(),
                i64::try_from(state.hash.lo).unwrap(),
                i(state.diagnostics.len()),
                i64::from(header.flags()),
                i64::from(header.pos()),
                i64::from(header.end()),
                factory.node_count(),
                factory.text_count(),
            ],
            vec![],
            vec![hex(state.file_name()), hex(state.text().as_bytes())],
        );
    }
}
impl FactoryHooks for SourceFileHooks {
    fn on_create(&self, factory: &mut dyn Factory, node: NodeId) {
        if factory.node(node).kind() != SyntaxKind::SourceFile {
            return;
        }
        let phase = {
            let mut phase = self.phase.lock().unwrap();
            *phase += 1;
            *phase
        };
        self.observe("source-create", factory, node);
        let state = factory.mut_source_file(node).unwrap();
        state.script_kind = ScriptKind::JSX;
        state.identifier_count = phase * 100;
        state.node_count = phase * 10;
        state.text_count = phase * 20;
        state.hash = SourceHash {
            hi: u64::try_from(phase).unwrap(),
            lo: u64::try_from(phase + 1).unwrap(),
        };
        state.diagnostics = vec![ts_ast::Diagnostic::external(
            None,
            TextRange::default(),
            JsString::from_bytes(b"hook".as_slice()),
            1,
            42,
            JsString::from_bytes(b"hook".as_slice()),
        )];
        let original = *self.original.lock().unwrap();
        if let Some(original) = original {
            let state = factory.mut_source_file(original).unwrap();
            state.script_kind = ScriptKind(i32::try_from(phase * 2).unwrap());
            state.identifier_count = 999;
        }
    }
    fn on_update(&self, factory: &mut dyn Factory, node: NodeId, _: NodeId) {
        self.observe("source-update", factory, node);
    }
    fn on_clone(&self, factory: &mut dyn Factory, node: NodeId, _: NodeId) {
        self.observe("source-clone", factory, node);
    }
}
fn source_file_hooks(sink: &Sink) {
    let hooks = Arc::new(SourceFileHooks {
        sink: sink.clone(),
        phase: Mutex::new(0),
        original: Mutex::new(None),
    });
    let mut factory =
        AstBuilder::with_hooks(SourceText::default(), &Counters::new(), hooks.clone());
    let original = factory.new_source_file(
        options(b"/hooks.ts"),
        SourceText::from_loaded_bytes(b"source\xff".as_slice()),
        None,
        None,
    );
    *hooks.original.lock().unwrap() = Some(original);
    factory.set_node_flags(original, 7);
    factory
        .node_mut(original)
        .unwrap()
        .set_range(TextRange::new(4, 9));
    factory.source_file_mut(original).unwrap().script_kind = ScriptKind::JS;
    let cloned = factory.clone_source_file(original);
    let eof = factory.new_token(SyntaxKind::EndOfFile.into());
    let updated = factory.update_source_file(original, None, Some(eof));
    hooks.observe("source-original-result", &factory, original);
    hooks.observe("source-cloned-result", &factory, cloned);
    hooks.observe("source-updated-result", &factory, updated);
}
