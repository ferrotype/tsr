//! Compile-time boundaries of the public local binding capability.
//!
//! A local identity cannot escape the scope that selected its owner.
//! ```compile_fail
//! use ts_ast::{BindBuilder, local_bind::BindNode};
//! fn escape(builder: &mut BindBuilder<'_>) -> BindNode<'static> {
//!     builder.with_local_scope(|local| local.source()).unwrap()
//! }
//! ```
//!
//! Two simultaneously live files still receive distinct scopes.
//! ```compile_fail
//! use ts_ast::BindBuilder;
//! fn cross_owner(first: &mut BindBuilder<'_>, second: &mut BindBuilder<'_>) {
//!     first.with_local_scope(|left| {
//!         let node = left.source();
//!         second.with_local_scope(|right| { right.node(node); });
//!     });
//! }
//! ```
//!
//! Syntax and flow namespaces remain distinct even in the same scope.
//! ```compile_fail
//! use ts_ast::local_bind::{BindNode, LocalBind};
//! fn wrong_namespace<'scope>(local: &mut LocalBind<'scope, '_>, node: BindNode<'scope>) {
//!     local.set_flow(node, Some(node));
//! }
//! ```
//!
//! A typed row borrow excludes writes until its last read.
//! ```compile_fail
//! use ts_ast::local_bind::{BindNode, LocalBind};
//! fn overlapping<'scope>(local: &mut LocalBind<'scope, '_>, node: BindNode<'scope>) {
//!     let read = local.node(node).as_identifier().unwrap();
//!     local.set_flags(node, 1);
//!     assert_eq!(read.text(), b"x");
//! }
//! ```
//!
//! The same operations compile when the borrowed observation finishes first.
//! ```
//! use ts_ast::local_bind::{BindNode, LocalBind};
//! fn separate<'scope>(local: &mut LocalBind<'scope, '_>, node: BindNode<'scope>) {
//!     let is_x = local.node(node).as_identifier().unwrap().text() == b"x";
//!     local.set_flags(node, u32::from(is_x));
//! }
//! ```

use crate::local_bind::{BindList, BindNode, BindSlice, LocalBind, LocalChildVisitor};
use crate::*;
use std::{
    ops::ControlFlow,
    panic::{catch_unwind, AssertUnwindSafe},
};
use ts_arena::{Counters, Error};
use ts_core::TextRange;
use ts_jsstring::SourceText;

fn text(bytes: &[u8]) -> JsString {
    JsString::from_bytes(bytes)
}

fn finish(
    mut build: AstBuilder,
    source_text: SourceText,
    roots: &[NodeId],
) -> (ParsedFile, NodeId) {
    let nodes = build
        .node_slice(roots.iter().copied().map(Some).collect())
        .unwrap();
    let list = build.new_list(TextRange::new(0, 0), nodes).unwrap();
    let source = build.new_source_file(
        SourceFileParseOptions {
            file_name: text(b"/local.ts"),
            ..Default::default()
        },
        source_text,
        Some(list),
        None,
    );
    for &root in roots {
        build.node_mut(root).unwrap().set_parent(Some(source));
    }
    (build.complete(source).unwrap(), source)
}

fn simple(counters: &Counters) -> (ParsedFile, NodeId, NodeId) {
    let source_text = SourceText::from_loaded_bytes(b"x".as_slice());
    let mut build = AstBuilder::new(source_text.clone(), counters);
    let identifier = build.new_identifier(text(b"x"));
    build.set_node_range(identifier, TextRange::new(0, 1));
    let (parsed, source) = finish(build, source_text, &[identifier]);
    (parsed, source, identifier)
}

fn assert_empty_binding(binding: Option<NodeBinding>) {
    let binding = binding.expect("materialized empty binding remains present");
    assert_eq!(
        (
            binding.symbol,
            binding.local_symbol,
            binding.locals,
            binding.next_container,
            binding.flow_node,
            binding.return_flow_node,
            binding.end_flow_node,
            binding.fallthrough_flow_node
        ),
        (None, None, None, None, None, None, None, None),
    );
}

#[test]
fn typed_text_borrows_and_owned_values_share_raw_and_exceptional_backings() {
    let source_text = SourceText::from_loaded_bytes(b"raw \\u0061 #private".as_slice());
    let source_start = source_text.as_bytes().as_ptr();
    let mut build = AstBuilder::new(source_text.clone(), &Counters::new());
    let raw = build.new_identifier(text(b"raw"));
    build.set_node_range(raw, TextRange::new(0, 3));
    let escaped = build.new_identifier(text(b"a"));
    build.set_node_range(escaped, TextRange::new(4, 10));
    let synthetic = build.new_identifier(text(b"factory"));
    let wtf8 = build.new_identifier(text(b"\xed\xa0\x80"));
    let malformed = build.new_identifier(text(b"\xed\xa0\x80\xff"));
    let private = build.new_private_identifier(text(b"#private"));
    build.set_node_range(private, TextRange::new(11, 19));
    let roots = [raw, escaped, synthetic, wtf8, malformed, private];
    let (parsed, source) = finish(build, source_text, &roots);
    let mut retained = Vec::new();
    let completed = parsed
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|local| {
                    assert_eq!(local.node_id(local.source()), source);
                    for (id, bytes) in [
                        (raw, b"raw".as_slice()),
                        (escaped, b"a"),
                        (synthetic, b"factory"),
                        (wtf8, b"\xed\xa0\x80"),
                        (malformed, b"\xed\xa0\x80\xff"),
                    ] {
                        let node = local.import_node(id)?;
                        let read = local.node(node);
                        let identifier = read.as_identifier().unwrap();
                        assert_eq!(identifier.text(), bytes);
                        let owned = identifier.text_owned();
                        let checked = local.view().node_text(id)?.into_js_string();
                        assert_eq!(owned.as_bytes(), bytes);
                        assert_eq!(owned.validity(), checked.validity());
                        assert_eq!(owned.as_bytes().as_ptr(), identifier.text().as_ptr());
                        assert_eq!(owned.as_bytes().as_ptr(), checked.as_bytes().as_ptr());
                        if id == raw {
                            assert_eq!(owned.as_bytes().as_ptr(), source_start);
                        }
                        retained.push((owned, bytes));
                        assert!(read.as_private_identifier().is_none());
                        assert_eq!(read.parent().map(|id| local.node_id(id)), Some(source));
                        assert_eq!(read.kind(), SyntaxKind::Identifier);
                    }
                    assert_eq!(local.node(local.import_node(raw)?).pos(), 0);
                    assert_eq!(local.node(local.import_node(raw)?).end(), 3);
                    assert_eq!(local.node(local.import_node(synthetic)?).pos(), -1);
                    let private = local
                        .node(local.import_node(private)?)
                        .as_private_identifier()
                        .unwrap();
                    let owned = private.text_owned();
                    assert_eq!(owned.as_bytes(), b"#private");
                    assert_eq!(owned.as_bytes().as_ptr(), private.text().as_ptr());
                    retained.push((owned, b"#private".as_slice()));
                    Ok::<_, Error>(())
                })
                .expect("validated factory source enters local binding")?;
            Ok(())
        })
        .unwrap();
    assert!(completed.bound_in_place());
    assert_eq!(
        completed
            .view()
            .node(escaped)
            .unwrap()
            .as_identifier()
            .unwrap()
            .text(),
        b"a"
    );
    drop(completed);
    for (owned, bytes) in retained {
        assert_eq!(owned.as_bytes(), bytes);
        assert_eq!(owned.validity(), text(bytes).validity());
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Child {
    Node(NodeId),
    List {
        nil: bool,
        elements: Vec<Option<NodeId>>,
    },
    Slice {
        nil: bool,
        elements: Vec<Option<NodeId>>,
    },
}

struct GeneralChildren<'view> {
    view: AstView<'view>,
    children: Vec<Child>,
}
impl ChildVisitor for GeneralChildren<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.children.push(Child::Node(node));
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        let nodes = self.view.list(list).unwrap().nodes();
        self.children.push(Child::List {
            nil: nodes.is_nil(),
            elements: self.view.node_slice(nodes).unwrap().iter().collect(),
        });
        ControlFlow::Continue(())
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        self.children.push(Child::Slice {
            nil: nodes.is_nil(),
            elements: self.view.node_slice(nodes).unwrap().iter().collect(),
        });
        ControlFlow::Continue(())
    }
}

struct LocalChildren<'view, 'scope, 'owner> {
    local: &'view LocalBind<'scope, 'owner>,
    children: Vec<Child>,
}

struct BreakOnFirst(usize);
impl<'scope> LocalChildVisitor<'scope> for BreakOnFirst {
    fn visit_node(&mut self, _: BindNode<'scope>) -> ControlFlow<()> {
        self.0 += 1;
        ControlFlow::Break(())
    }
    fn visit_list(&mut self, _: BindList<'scope>) -> ControlFlow<()> {
        self.0 += 1;
        ControlFlow::Break(())
    }
    fn visit_node_slice(&mut self, _: BindSlice<'scope>) -> ControlFlow<()> {
        self.0 += 1;
        ControlFlow::Break(())
    }
}
impl<'scope> LocalChildVisitor<'scope> for LocalChildren<'_, 'scope, '_> {
    fn visit_node(&mut self, node: BindNode<'scope>) -> ControlFlow<()> {
        self.children.push(Child::Node(self.local.node_id(node)));
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, list: BindList<'scope>) -> ControlFlow<()> {
        let nodes = self.local.list(list);
        self.children.push(Child::List {
            nil: nodes.is_nil(),
            elements: self
                .local
                .slice(nodes)
                .iter()
                .map(|node| node.map(|node| self.local.node_id(node)))
                .collect(),
        });
        ControlFlow::Continue(())
    }
    fn visit_node_slice(&mut self, nodes: BindSlice<'scope>) -> ControlFlow<()> {
        self.children.push(Child::Slice {
            nil: nodes.is_nil(),
            elements: self
                .local
                .slice(nodes)
                .iter()
                .map(|node| node.map(|node| self.local.node_id(node)))
                .collect(),
        });
        ControlFlow::Continue(())
    }
}

fn general_children(view: AstView<'_>, node: NodeId) -> Vec<Child> {
    let mut children = GeneralChildren {
        view,
        children: Vec::new(),
    };
    assert_eq!(
        view.node(node).unwrap().for_each_child(&mut children),
        ControlFlow::Continue(())
    );
    children.children
}

#[test]
fn local_children_match_general_order_and_preserve_slice_nil_elements_and_subranges() {
    let source_text = SourceText::default();
    let mut build = AstBuilder::new(source_text.clone(), &Counters::new());
    let a = build.new_identifier(text(b"a"));
    let b = build.new_identifier(text(b"b"));
    let c = build.new_identifier(text(b"c"));
    let backing = build
        .node_slice(vec![Some(a), None, Some(b), Some(c)])
        .unwrap();
    let selected = backing.slice(1..3).unwrap();
    let list = build.new_list(TextRange::new(1, 3), selected).unwrap();
    let array = build.new_array_literal_expression(Some(list), false);
    let absent_list = build.new_array_literal_expression(None, false);
    let nil_list = build
        .new_list(TextRange::new(0, 0), NodeSlice::empty())
        .unwrap();
    let nil_array = build.new_array_literal_expression(Some(nil_list), false);
    let allocated_empty = build.node_slice(Vec::new()).unwrap();
    assert!(!allocated_empty.is_nil());
    let empty_list = build
        .new_list(TextRange::new(0, 0), allocated_empty)
        .unwrap();
    let empty_array = build.new_array_literal_expression(Some(empty_list), false);
    let syntax_list = build.new_syntax_list(selected);
    let nil_syntax = build.new_syntax_list(NodeSlice::empty());
    let empty_syntax = build.new_syntax_list(allocated_empty);
    let tag_name = build.new_identifier(text(b"param"));
    let type_expression = build.new_js_doc_type_expression(Some(c));
    let tags = [false, true].map(|name_first| {
        build.new_js_doc_parameter_or_property_tag(
            SyntaxKind::JSDocParameterTag.into(),
            Some(tag_name),
            Some(a),
            false,
            Some(type_expression),
            name_first,
            Some(list),
        )
    });
    let roots = [
        array,
        absent_list,
        nil_array,
        empty_array,
        syntax_list,
        nil_syntax,
        empty_syntax,
        tags[0],
        tags[1],
    ];
    let (parsed, source) = finish(build, source_text, &roots);
    let expected = roots
        .iter()
        .map(|&node| general_children(parsed.view(), node))
        .collect::<Vec<_>>();
    assert_eq!(
        expected[0],
        vec![Child::List {
            nil: false,
            elements: vec![None, Some(b)]
        }]
    );
    assert!(expected[1].is_empty());
    assert_eq!(
        expected[2],
        vec![Child::List {
            nil: true,
            elements: vec![]
        }]
    );
    assert_eq!(
        expected[3],
        vec![Child::List {
            nil: false,
            elements: vec![]
        }]
    );
    assert_eq!(
        &expected[7][..3],
        &[
            Child::Node(tag_name),
            Child::Node(type_expression),
            Child::Node(a)
        ]
    );
    assert_eq!(
        &expected[8][..3],
        &[
            Child::Node(tag_name),
            Child::Node(a),
            Child::Node(type_expression)
        ]
    );
    parsed
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|local| {
                    for (&root, expected) in roots.iter().zip(&expected) {
                        let mut children = LocalChildren {
                            local: &local,
                            children: Vec::new(),
                        };
                        assert_eq!(
                            local
                                .node(local.import_node(root)?)
                                .for_each_child(&mut children),
                            ControlFlow::Continue(())
                        );
                        assert_eq!(&children.children, expected);
                        if !expected.is_empty() {
                            let mut early = BreakOnFirst(0);
                            assert_eq!(
                                local
                                    .node(local.import_node(root)?)
                                    .for_each_child(&mut early),
                                ControlFlow::Break(()),
                            );
                            assert_eq!(early.0, 1, "visiting stops at the first break");
                        }
                    }
                    let source_read = local.node(local.source()).as_source_file().unwrap();
                    let nodes = local.slice(local.list(source_read.statements().unwrap()));
                    assert_eq!(nodes.len(), roots.len());
                    assert!(!nodes.is_empty());
                    assert_eq!(
                        nodes
                            .iter()
                            .rev()
                            .map(|node| local.node_id(node.unwrap()))
                            .collect::<Vec<_>>(),
                        roots.into_iter().rev().collect::<Vec<_>>()
                    );
                    assert_eq!(local.node_id(local.source()), source);
                    Ok::<_, Error>(())
                })
                .expect("local list fixture")?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn flow_writes_preserve_public_presence_and_flags_without_invalidating_syntax() {
    let source_text = SourceText::from_loaded_bytes(b"x".as_slice());
    let mut build = AstBuilder::new(source_text.clone(), &Counters::new());
    let identifier = build.new_identifier(text(b"x"));
    let function = build.new_function_declaration(None, None, None, None, None, None, None, None);
    // Concrete shape, rather than its caller-supplied syntax kind, controls fields.
    let token = build.new_token(SyntaxKind::Identifier.into());
    let (parsed, source) = finish(build, source_text, &[identifier, function, token]);
    let completed = parsed
        .bind_and_publish(|builder| {
            let flow = builder.flows_mut().push(FlowNode::new(flow_flags::START));
            builder.set_node_symbol(function, None)?;
            assert!(builder.binding(identifier)?.is_none());
            assert!(builder.binding(function)?.is_some());
            builder
                .with_local_scope(|mut local| {
                    let flow = local.import_flow(flow)?;
                    let identifier = local.import_node(identifier)?;
                    let function = local.import_node(function)?;
                    let token = local.import_node(token)?;
                    assert!(local.node(identifier).has_flow_node());
                    assert!(local.node(function).has_flow_node());
                    assert!(!local.node(token).has_flow_node());
                    assert!(!local.set_flow(token, Some(flow)));
                    assert!(local.set_flow(identifier, Some(flow)));
                    assert!(local.set_flow(function, Some(flow)));
                    local.set_flags(identifier, node_flags::AMBIENT | node_flags::UNREACHABLE);
                    assert_eq!(
                        local.flow_id(flow),
                        local.flow_id(local.import_flow(local.flow_id(flow))?)
                    );
                    assert_eq!(
                        local.node(identifier).parent().map(|id| local.node_id(id)),
                        Some(source)
                    );
                    Ok::<_, Error>(())
                })
                .expect("inline binding fields remain eligible")?;
            assert_eq!(builder.node_flow(identifier)?, Some(flow));
            assert_eq!(builder.node_flow(function)?, Some(flow));
            assert!(builder.binding(token)?.is_none());
            assert_eq!(builder.node(token)?.kind(), SyntaxKind::Identifier);
            assert_eq!(builder.node(token)?.flags(), 0);
            builder
                .with_local_scope(|mut local| {
                    assert!(local.set_flow(local.import_node(identifier)?, None));
                    assert!(local.set_flow(local.import_node(function)?, None));
                    Ok::<_, Error>(())
                })
                .expect("narrow writes preserve completed syntax proof")?;
            assert!(builder.binding(identifier)?.is_none());
            assert_empty_binding(builder.binding(function)?);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        completed.view().node(identifier).unwrap().flags(),
        node_flags::AMBIENT | node_flags::UNREACHABLE
    );
    assert!(completed.view().node_binding(identifier).unwrap().is_none());
    assert_empty_binding(completed.view().node_binding(function).unwrap());
}

#[test]
fn raw_imports_reject_foreign_owners_before_slots_in_each_namespace() {
    let counters = Counters::new();
    let (parsed, _, child) = simple(&counters);
    let (_foreign, _, foreign_child) = simple(&counters);
    let mut foreign_flows = FlowNodes::new(&counters);
    let foreign_flow = foreign_flows.push(FlowNode::new(flow_flags::START));
    parsed
        .bind_and_publish(|builder| {
            let flow = builder.flows_mut().push(FlowNode::new(flow_flags::START));
            builder
                .with_local_scope(|local| {
                    assert!(local.import_node(child).is_ok());
                    assert!(local.import_flow(flow).is_ok());
                    assert_eq!(local.import_node(foreign_child), Err(Error::WrongOwner));
                    assert_eq!(
                        local.import_node(
                            NodeId::from_parts(foreign_child.arena(), u32::MAX).unwrap()
                        ),
                        Err(Error::WrongOwner)
                    );
                    assert_eq!(
                        local.import_node(NodeId::from_parts(child.arena(), u32::MAX).unwrap()),
                        Err(Error::InvalidSlot)
                    );
                    assert_eq!(local.import_flow(foreign_flow), Err(Error::WrongOwner));
                    assert_eq!(
                        local.import_flow(
                            FlowId::from_parts(foreign_flow.arena(), u32::MAX).unwrap()
                        ),
                        Err(Error::WrongOwner)
                    );
                    assert_eq!(
                        local.import_flow(FlowId::from_parts(flow.arena(), u32::MAX).unwrap()),
                        Err(Error::InvalidSlot)
                    );
                })
                .expect("local raw boundary fixture");
            Ok(())
        })
        .unwrap();
}

#[test]
fn list_imports_validate_owner_kind_and_empty_ranges_before_minting_handles() {
    let counters = Counters::new();
    let (parsed, source, child) = simple(&counters);
    let (foreign, foreign_source, _) = simple(&counters);
    let foreign = foreign.publish_unbound();
    let foreign_list = foreign
        .view()
        .node(foreign_source)
        .unwrap()
        .statement_list()
        .unwrap();
    parsed
        .bind_and_publish(|builder| {
            let list = builder.node(source)?.statement_list().unwrap();
            let nodes = builder.parsed_view().list(list)?.nodes();
            let empty = nodes.slice(1..1)?;
            builder
                .with_local_scope(|mut local| {
                    let imported = local.import_list(list)?;
                    let imported_nodes = local.list(imported);
                    let edges = local.edges(imported_nodes);
                    assert_eq!(edges.len(), 1);
                    assert_eq!(
                        local.edge(edges, 0).map(|node| local.node_id(node)),
                        Some(child)
                    );
                    // Narrow mutation cannot invalidate a previously resolved range.
                    local.set_flags(local.import_node(child)?, node_flags::UNREACHABLE);
                    assert_eq!(
                        local.edge(edges, 0).map(|node| local.node_id(node)),
                        Some(child)
                    );
                    for slice in [NodeSlice::empty(), NodeSlice::missing(), empty, nodes] {
                        let scoped = local.import_slice(slice)?;
                        assert_eq!(scoped.is_nil(), slice.is_nil());
                        assert_eq!(scoped.len(), slice.len());
                        assert_eq!(local.edges(scoped).len(), slice.len());
                    }
                    assert_eq!(local.import_list(foreign_list), Err(Error::WrongOwner));
                    let wrong_owner =
                        ts_arena::AuxId::from_parts(foreign_list.0.arena(), u32::MAX).unwrap();
                    assert_eq!(
                        local.import_list(NodeListId(wrong_owner)),
                        Err(Error::WrongOwner)
                    );
                    let invalid_slot =
                        ts_arena::AuxId::from_parts(list.0.arena(), u32::MAX).unwrap();
                    assert_eq!(
                        local.import_list(NodeListId(invalid_slot)),
                        Err(Error::InvalidSlot)
                    );
                    assert_eq!(
                        local.import_list(NodeListId(nodes.backing.unwrap())),
                        Err(Error::InvalidGraph)
                    );
                    for slice in [
                        NodeSlice {
                            backing: Some(wrong_owner),
                            start: 0,
                            len: 0,
                        },
                        NodeSlice {
                            backing: Some(invalid_slot),
                            start: 0,
                            len: 0,
                        },
                        NodeSlice {
                            backing: Some(list.0),
                            start: 0,
                            len: 0,
                        },
                        NodeSlice {
                            start: 2,
                            len: 0,
                            ..nodes
                        },
                    ] {
                        let expected = local.parsed_view().node_slice(slice).err();
                        assert_eq!(local.import_slice(slice).err(), expected);
                        assert!(expected.is_some());
                    }
                    Ok::<_, Error>(())
                })
                .expect("core list fixture")?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn published_and_multiple_source_owners_do_not_invoke_local_callback() {
    let counters = Counters::new();
    let (parsed, source, _) = simple(&counters);
    let published = parsed.publish_unbound();
    published
        .bind_with(source, |builder| {
            assert!(builder
                .with_local_scope(|_| panic!("published owner entered local scope"))
                .is_none());
            Ok(())
        })
        .unwrap();
    let source_text = SourceText::default();
    let mut build = AstBuilder::new(source_text.clone(), &counters);
    let sibling = build.new_source_file(
        SourceFileParseOptions {
            file_name: text(b"/sibling.ts"),
            ..Default::default()
        },
        source_text.clone(),
        None,
        None,
    );
    let (parsed, _) = finish(build, source_text, &[]);
    parsed
        .bind_and_publish(|builder| {
            assert!(builder
                .with_local_scope(|_| panic!("multiple sources entered local scope"))
                .is_none());
            assert_eq!(builder.node(sibling)?.kind(), SyntaxKind::SourceFile);
            Ok(())
        })
        .unwrap();
}

#[test]
fn foreign_edges_and_unrestricted_mutation_keep_the_checked_backend() {
    let counters = Counters::new();
    let (foreign, _, foreign_child) = simple(&counters);
    let foreign = foreign.publish_unbound();
    let source_text = SourceText::default();
    let mut build = AstBuilder::new(source_text.clone(), &counters);
    build.retain_file(foreign);
    let statement = build.new_expression_statement(Some(foreign_child));
    let (parsed, _) = finish(build, source_text, &[statement]);
    parsed
        .bind_and_publish(|builder| {
            assert!(builder
                .with_local_scope(|_| panic!("foreign edge entered local scope"))
                .is_none());
            assert_eq!(
                builder.node(foreign_child)?.as_identifier().unwrap().text(),
                b"x"
            );
            Ok(())
        })
        .unwrap();
    let (parsed, _, child) = simple(&counters);
    parsed
        .bind_and_publish(|builder| {
            builder.node_mut(child)?.set_flags(node_flags::AMBIENT);
            assert!(builder
                .with_local_scope(|_| panic!("unrestricted mutation retained syntax proof"))
                .is_none());
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_lazy_node_created_inside_the_scope_still_crosses_a_checked_boundary() {
    let (parsed, source, child) = simple(&Counters::new());
    let mut lazy_id = None;
    let completed = parsed
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|local| {
                    let lazy = local.view().source_jsdoc(source, child, |transaction| {
                        let lazy = transaction.new_identifier(text(b"lazy"));
                        transaction.node_mut(lazy)?.set_parent(Some(child));
                        Ok(vec![lazy])
                    })?[0];
                    lazy_id = Some(lazy);
                    assert_eq!(local.import_node(lazy), Err(Error::WrongOwner));
                    assert_eq!(
                        local
                            .node(local.import_node(child)?)
                            .as_identifier()
                            .unwrap()
                            .text(),
                        b"x"
                    );
                    Ok::<_, Error>(())
                })
                .expect("initial core-only owner")?;
            assert!(builder
                .with_local_scope(|_| panic!("lazy owner reentered core-only scope"))
                .is_none());
            assert_eq!(
                builder
                    .node(lazy_id.unwrap())?
                    .as_identifier()
                    .unwrap()
                    .text(),
                b"lazy"
            );
            Ok(())
        })
        .unwrap();
    let retained = completed.retain_node(lazy_id.unwrap()).unwrap();
    drop(completed);
    assert_eq!(retained.node().as_identifier().unwrap().text(), b"lazy");
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        payload
            .downcast_ref::<&str>()
            .expect("text contract panic")
            .to_string()
    }
}

#[test]
fn child_kind_dispatch_and_shape_selected_names_match_constructed_mismatches() {
    let source_text = SourceText::default();
    let mut build = AstBuilder::new(source_text.clone(), &Counters::new());
    let expression = build.new_identifier(text(b"object"));
    let name = build.new_identifier(text(b"member"));
    let hidden_children = build.new_node(
        SyntaxKind::Identifier.into(),
        PropertyAccessExpressionData {
            expression: Some(expression),
            question_dot_token: None,
            name: Some(name),
        }
        .into(),
    );
    let wrong_payload = build.new_token(SyntaxKind::PropertyAccessExpression.into());
    let declaration = build.new_variable_declaration(Some(name), None, None, Some(expression));
    let roots = [hidden_children, wrong_payload, declaration];
    let (parsed, _) = finish(build, source_text, &roots);
    assert!(general_children(parsed.view(), hidden_children).is_empty());
    let expected_panic = panic_message(
        &*catch_unwind(AssertUnwindSafe(|| {
            general_children(parsed.view(), wrong_payload)
        }))
        .unwrap_err(),
    );
    assert_eq!(
        expected_panic,
        "PropertyAccessExpression kind requires PropertyAccessExpression payload"
    );
    let names = roots.map(|node| parsed.view().node(node).unwrap().name());
    parsed
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|local| {
                    for (node, expected) in roots.into_iter().zip(names) {
                        assert_eq!(
                            local
                                .node(local.import_node(node)?)
                                .name()
                                .map(|name| local.node_id(name)),
                            expected
                        );
                    }
                    let mut children = LocalChildren {
                        local: &local,
                        children: Vec::new(),
                    };
                    assert_eq!(
                        local
                            .node(local.import_node(hidden_children)?)
                            .for_each_child(&mut children),
                        ControlFlow::Continue(())
                    );
                    assert!(children.children.is_empty());
                    let error = catch_unwind(AssertUnwindSafe(|| {
                        local
                            .node(local.import_node(wrong_payload).unwrap())
                            .for_each_child(&mut children)
                    }))
                    .unwrap_err();
                    assert_eq!(panic_message(&*error), expected_panic);
                    Ok::<_, Error>(())
                })
                .expect("kind/shape mismatch remains a supported checked graph")?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn identifier_name_failures_preserve_nil_parent_and_interface_conversion_messages() {
    for (parent_kind, expected) in [
        (None, "nil node in source AST utility"),
        (
            Some(SyntaxKind::QualifiedName),
            "interface conversion: ast.nodeData is *ast.Token, not *ast.QualifiedName",
        ),
        (
            Some(SyntaxKind::BindingElement),
            "interface conversion: ast.nodeData is *ast.Token, not *ast.BindingElement",
        ),
        (
            Some(SyntaxKind::ImportSpecifier),
            "interface conversion: ast.nodeData is *ast.Token, not *ast.ImportSpecifier",
        ),
    ] {
        let source_text = SourceText::default();
        let mut build = AstBuilder::new(source_text.clone(), &Counters::new());
        let child = build.new_identifier(text(b"implements"));
        let parent = parent_kind.map(|kind| build.new_token(kind.into()));
        build.node_mut(child).unwrap().set_parent(parent);
        // A missing parent must stay absent; placing child in the source's
        // statement list would repair the condition this test needs to exercise.
        let roots = parent.into_iter().collect::<Vec<_>>();
        let (parsed, _) = finish(build, source_text, &roots);
        let checked_failure = catch_unwind(AssertUnwindSafe(|| {
            crate::is_identifier_name(parsed.view(), child).unwrap()
        }))
        .unwrap_err();
        assert_eq!(panic_message(&*checked_failure), expected);
        parsed
            .bind_and_publish(|builder| {
                builder
                    .with_local_scope(|local| {
                        let child = local.import_node(child).unwrap();
                        let local_failure =
                            catch_unwind(AssertUnwindSafe(|| local.is_identifier_name(child)))
                                .unwrap_err();
                        assert_eq!(panic_message(&*local_failure), expected);
                    })
                    .expect("constructed helper counterexample is locally eligible");
                Ok(())
            })
            .unwrap();
    }
}
