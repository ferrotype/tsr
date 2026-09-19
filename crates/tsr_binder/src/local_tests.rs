//! Counterexamples at the boundary between local and checked binder access.
use crate::Binder;
use std::{
    ops::ControlFlow,
    panic::{catch_unwind, AssertUnwindSafe},
};
use ts_arena::Counters;
use ts_ast::{
    AstBuilder, AstView, ChildVisitor, Diagnostic, Factory, FactoryMethods, JsString, NodeId,
    NodeListId, NodeSlice, ParsedFile, SourceFileParseOptions, SyntaxKind,
};
use ts_core::{ScriptKind, TextRange};
use ts_jsstring::SourceText;

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| {
            payload
                .downcast_ref::<&str>()
                .expect("text contract panic")
                .to_string()
        })
}

fn malformed_receiver(kind: SyntaxKind) -> (ParsedFile, NodeId) {
    let source_text = SourceText::default();
    let mut build = AstBuilder::new(source_text.clone(), &Counters::new());
    let node = build.new_token(kind.into());
    let nodes = build.node_slice(vec![Some(node)]).unwrap();
    let list = build.new_list(TextRange::new(0, 0), nodes).unwrap();
    let source = build.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/local-receiver.ts".as_slice()),
            ..Default::default()
        },
        source_text,
        Some(list),
        None,
    );
    build.node_mut(node).unwrap().set_parent(Some(source));
    (build.complete(source).unwrap(), node)
}

#[test]
fn target_text_preserves_checked_kind_shape_failures() {
    for (kind, payload) in [
        (SyntaxKind::Identifier, "Identifier"),
        (SyntaxKind::PrivateIdentifier, "PrivateIdentifier"),
        (SyntaxKind::StringLiteral, "StringLiteral"),
    ] {
        let (parsed, node) = malformed_receiver(kind);
        let expected =
            format!("interface conversion: ast.nodeData is *ast.Token, not *ast.{payload}");
        parsed
            .bind_and_publish(|builder| {
                let checked_failure = {
                    let binder = Binder::new(builder);
                    catch_unwind(AssertUnwindSafe(|| {
                        binder.target_text(crate::target::BindingNode::Checked(node))
                    }))
                    .unwrap_err()
                };
                assert_eq!(panic_message(&*checked_failure), expected);
                builder
                    .with_local_scope(|local| {
                        let node = local.import_node(node).unwrap();
                        let binder = Binder::from_backend(crate::backend::Backend::Local(local));
                        let local_failure = catch_unwind(AssertUnwindSafe(|| {
                            binder.target_text(crate::target::BindingNode::Local(node))
                        }))
                        .unwrap_err();
                        assert_eq!(panic_message(&*local_failure), expected);
                    })
                    .expect("constructed text receiver admits local access");
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn target_text_shares_core_and_lazy_backings() {
    let source_text = SourceText::from_loaded_bytes(b"raw \\u0061".as_slice());
    let mut build = AstBuilder::new(source_text.clone(), &Counters::new());
    let raw = build.new_identifier(JsString::from_bytes(b"raw".as_slice()));
    build.set_node_range(raw, TextRange::new(0, 3));
    let escaped = build.new_identifier(JsString::from_bytes(b"a".as_slice()));
    build.set_node_range(escaped, TextRange::new(4, 10));
    let source = build.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/local-owned-text.ts".as_slice()),
            ..Default::default()
        },
        source_text,
        None,
        None,
    );
    for node in [raw, escaped] {
        build.node_mut(node).unwrap().set_parent(Some(source));
    }
    let parsed = build.complete(source).unwrap();
    let mut retained = Vec::new();
    let completed = parsed
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|local| {
                    let lazy = local.view().source_jsdoc(source, raw, |transaction| {
                        let lazy =
                            transaction.new_identifier(JsString::from_bytes(b"lazy".as_slice()));
                        transaction.node_mut(lazy)?.set_parent(Some(raw));
                        Ok(vec![lazy])
                    })?[0];
                    let binder = Binder::from_backend(crate::backend::Backend::Local(local));
                    for (node, bytes) in
                        [(raw, b"raw".as_slice()), (escaped, b"a"), (lazy, b"lazy")]
                    {
                        let target = binder.binding_node(node);
                        assert_eq!(
                            matches!(target, crate::target::BindingNode::Checked(_)),
                            node == lazy
                        );
                        let checked = binder.text(node);
                        let owned = binder.target_text(target);
                        assert_eq!(owned.as_bytes(), bytes);
                        assert_eq!(owned.validity(), checked.validity());
                        assert_eq!(owned.as_bytes().as_ptr(), checked.as_bytes().as_ptr());
                        retained.push((owned, bytes));
                    }
                    Ok(())
                })
                .expect("fresh core admits local access")
        })
        .unwrap();
    drop(completed);
    for (owned, bytes) in retained {
        assert_eq!(owned.as_bytes(), bytes);
    }
}

#[test]
fn narrowable_receivers_preserve_checked_interface_conversion_panics() {
    for (kind, payload) in [
        (
            SyntaxKind::PropertyAccessExpression,
            "PropertyAccessExpression",
        ),
        (
            SyntaxKind::ParenthesizedExpression,
            "ParenthesizedExpression",
        ),
        (SyntaxKind::NonNullExpression, "NonNullExpression"),
    ] {
        let expected =
            format!("interface conversion: ast.nodeData is *ast.Token, not *ast.{payload}");
        let (parsed, node) = malformed_receiver(kind);
        parsed
            .bind_and_publish(|builder| {
                // Explicit Checked selection exercises the retained implementation,
                // even though the source also admits the new local capability.
                let checked_failure = {
                    let binder = Binder::new(builder);
                    catch_unwind(AssertUnwindSafe(|| {
                        binder.is_narrowable_reference(binder.binding_node(node))
                    }))
                    .unwrap_err()
                };
                assert_eq!(panic_message(&*checked_failure), expected);
                builder
                    .with_local_scope(|local| {
                        let node = local.import_node(node).unwrap();
                        let binder = Binder::from_backend(crate::backend::Backend::Local(local));
                        let local_failure = catch_unwind(AssertUnwindSafe(|| {
                            binder.is_narrowable_reference(crate::target::BindingNode::Local(node))
                        }))
                        .unwrap_err();
                        assert_eq!(panic_message(&*local_failure), expected);
                    })
                    .expect("constructed receiver admits local access");
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn local_block_dispatch_preserves_checked_payload_failures() {
    for kind in [SyntaxKind::Block, SyntaxKind::ModuleBlock] {
        let (parsed, node) = malformed_receiver(kind);
        parsed
            .bind_and_publish(|builder| {
                let checked_failure = {
                    let mut binder = Binder::new(builder);
                    binder.current_flow = Some(binder.new_flow_node(ts_ast::flow_flags::START));
                    catch_unwind(AssertUnwindSafe(|| binder.bind(Some(node)))).unwrap_err()
                };
                builder
                    .with_local_scope(|local| {
                        let mut binder =
                            Binder::from_backend(crate::backend::Backend::Local(local));
                        binder.current_flow = Some(binder.new_flow_node(ts_ast::flow_flags::START));
                        let local_failure =
                            catch_unwind(AssertUnwindSafe(|| binder.bind(Some(node)))).unwrap_err();
                        assert_eq!(
                            panic_message(&*local_failure),
                            panic_message(&*checked_failure)
                        );
                    })
                    .expect("constructed block admits local access");
                Ok(())
            })
            .unwrap();
    }
}

fn parse(bytes: &[u8]) -> ParsedFile {
    ts_parser::parse_source_file_with_counters(
        SourceText::from_loaded_bytes(bytes),
        ScriptKind::TS,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/contextual.ts".as_slice()),
            ..Default::default()
        },
        &Counters::new(),
    )
}

#[derive(Debug, PartialEq, Eq)]
struct Header {
    kind: i16,
    pos: i32,
    end: i32,
    flags: u32,
}

struct SyntaxHeaders<'view> {
    view: AstView<'view>,
    headers: Vec<Header>,
}
impl ChildVisitor for SyntaxHeaders<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        let read = self.view.node(node).unwrap();
        self.headers.push(Header {
            kind: read.kind().raw(),
            pos: read.pos(),
            end: read.end(),
            flags: read.flags(),
        });
        read.for_each_child(self)
    }
    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        self.visit_node_slice(self.view.list(list).unwrap().nodes())
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        for node in self.view.node_slice(nodes).unwrap().iter().flatten() {
            self.visit_node(node)?;
        }
        ControlFlow::Continue(())
    }
}

fn syntax_headers(view: AstView<'_>, source: NodeId) -> Vec<Header> {
    let mut visitor = SyntaxHeaders {
        view,
        headers: Vec::new(),
    };
    assert_eq!(visitor.visit_node(source), ControlFlow::Continue(()));
    visitor.headers
}

fn diagnostics(values: &[Diagnostic], source: NodeId) -> Vec<Diagnostic> {
    values
        .iter()
        .map(|diagnostic| {
            assert_eq!(diagnostic.file, Some(source));
            assert!(diagnostic.message_chain.is_empty());
            assert!(diagnostic.related_information.is_empty());
            let mut diagnostic = diagnostic.clone();
            // Separate parses deliberately allocate distinct source identities.
            // Every other diagnostic field, including argument bytes, stays exact.
            diagnostic.file = None;
            diagnostic
        })
        .collect()
}

#[test]
fn parsed_contextual_keywords_preserve_diagnostics_and_error_flags_across_backends() {
    for (name, bytes) in [
        (
            "cooked reserved word",
            b"'use strict'; let \\u0069mplements = 1; implements;".as_slice(),
        ),
        ("external module", b"export {}; let await = 1; await;"),
        (
            "clean external module",
            b"export {}; let \\u0069mplements = 1; implements;",
        ),
        (
            "generator and async contexts",
            b"function* f() { let yield = 1; } async function g() { let await = 1; }",
        ),
        (
            "property names",
            b"const object = { implements: 1, await: 2 }; object.implements; object.await;",
        ),
        (
            "ambient context",
            b"declare namespace N { const implements: number; }",
        ),
        (
            "parse-error suppression",
            b"let broken = ; let \\u0069mplements = 1; implements;",
        ),
        // Run a clean source after the error/module cases to catch cached source
        // facts accidentally surviving the Binder that owns them.
        ("subsequent clean source", b"let value = 1; value;"),
    ] {
        let parsed = parse(bytes);
        let file = parsed.publish_unbound();
        let source = file.root().unwrap();
        let checked = crate::bind_source_file(&file, source).unwrap();
        let checked_view = checked.view();
        let expected_headers = syntax_headers(checked_view.ast(), source);
        let expected_diagnostics = diagnostics(checked_view.result().diagnostics(), source);

        let completed = crate::bind_parsed_file(parse(bytes)).unwrap();
        // Await reparsing can leave multiple source records before binding.
        // Those fixtures must still match through the existing fallback. Cases
        // below prove that this comparison also exercises the local backend.
        if matches!(
            name,
            "clean external module" | "property names" | "subsequent clean source"
        ) {
            assert!(
                completed.bound_with_local_scope(),
                "{name}: expected local scope"
            );
        }
        let actual = completed.view();
        assert_eq!(
            syntax_headers(actual.ast(), completed.source()),
            expected_headers,
            "{name}: bound flags"
        );
        assert_eq!(
            diagnostics(actual.result().diagnostics(), completed.source()),
            expected_diagnostics,
            "{name}: diagnostics"
        );
    }
}

#[test]
fn scoped_and_checked_node_aliases_share_identity_and_live_flags() {
    parse(b"let value = 1;")
        .bind_and_publish(|builder| {
            let raw = builder.source();
            builder
                .with_local_scope(|local| {
                    let node = local.import_node(raw).unwrap();
                    let mut binder = Binder::from_backend(crate::backend::Backend::Local(local));
                    let local = crate::target::BindingNode::Local(node);
                    let checked = crate::target::BindingNode::Checked(raw);
                    assert!(binder.same_node(Some(local), Some(checked)));
                    assert!(binder.same_node(Some(checked), Some(local)));
                    assert!(!binder.same_node(Some(local), None));
                    let flags = binder.node_flags(local) | ts_ast::node_flags::THIS_NODE_HAS_ERROR;
                    binder.set_binding_flags(local, flags);
                    assert_eq!(binder.node_flags(checked), flags);
                })
                .expect("parsed source admits local scope");
            Ok(())
        })
        .unwrap();
}

#[test]
fn strict_predicates_preserve_selected_payload_failures() {
    for (kind, message) in [
        (SyntaxKind::BinaryExpression, "binary payload"),
        (SyntaxKind::PrefixUnaryExpression, "prefix payload"),
        (SyntaxKind::PostfixUnaryExpression, "postfix payload"),
        (SyntaxKind::CatchClause, "catch payload"),
        (SyntaxKind::LabeledStatement, "label payload"),
    ] {
        let (parsed, node) = malformed_receiver(kind);
        parsed
            .bind_and_publish(|builder| {
                {
                    let mut binder = Binder::new(builder);
                    let failure = catch_unwind(AssertUnwindSafe(|| {
                        invoke_strict_predicate(
                            &mut binder,
                            kind,
                            crate::target::BindingNode::Checked(node),
                        );
                    }))
                    .unwrap_err();
                    assert_eq!(panic_message(&*failure), message);
                }
                builder
                    .with_local_scope(|local| {
                        let target =
                            crate::target::BindingNode::Local(local.import_node(node).unwrap());
                        let mut binder =
                            Binder::from_backend(crate::backend::Backend::Local(local));
                        let failure = catch_unwind(AssertUnwindSafe(|| {
                            invoke_strict_predicate(&mut binder, kind, target);
                        }))
                        .unwrap_err();
                        assert_eq!(panic_message(&*failure), message);
                    })
                    .expect("malformed kind retains a valid local graph");
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn strict_predicates_keep_diagnostic_order_and_source_ranges() {
    let bytes = b"export {}; let eval = 1; eval++; ++arguments; delete eval; function arguments(eval) {} try {} catch (arguments) {}";
    let file = parse(bytes).publish_unbound();
    let source = file.root().unwrap();
    let checked = crate::bind_source_file(&file, source).unwrap();
    let expected = diagnostics(checked.view().result().diagnostics(), source);
    assert!(
        !expected.is_empty(),
        "fixture must exercise diagnostic construction"
    );
    let completed = crate::bind_parsed_file(parse(bytes)).unwrap();
    assert!(completed.bound_with_local_scope());
    assert_eq!(
        diagnostics(completed.view().result().diagnostics(), completed.source()),
        expected
    );
    assert_eq!(
        syntax_headers(completed.view().ast(), completed.source()),
        syntax_headers(checked.view().ast(), source)
    );
}

#[test]
fn declaration_helper_bridges_keep_lazy_names_checked() {
    let source_text = SourceText::default();
    let mut build = AstBuilder::new(source_text.clone(), &Counters::new());
    let root = build.new_identifier(JsString::from_bytes(b"root".as_slice()));
    let source = build.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/lazy-name.ts".as_slice()),
            ..Default::default()
        },
        source_text,
        None,
        None,
    );
    build.set_node_parent(root, Some(source));
    build
        .complete(source)
        .unwrap()
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|local| {
                    let lazy = local.view().source_jsdoc(source, root, |transaction| {
                        let name =
                            transaction.new_identifier(JsString::from_bytes(b"lazy".as_slice()));
                        let declaration =
                            transaction.new_variable_declaration(Some(name), None, None, None);
                        transaction.node_mut(name)?.set_parent(Some(declaration));
                        transaction.node_mut(declaration)?.set_parent(Some(root));
                        Ok(vec![name, declaration])
                    })?;
                    let binder = Binder::from_backend(crate::backend::Backend::Local(local));
                    let name = binder
                        .target_name_of_declaration(Some(binder.binding_node(lazy[1])))
                        .unwrap();
                    assert!(matches!(name, crate::target::BindingNode::Checked(_)));
                    assert_eq!(binder.node_id(name), lazy[0]);
                    assert!(!binder.target_has_dynamic_name(Some(binder.binding_node(lazy[1]))));
                    assert_eq!(
                        binder.target_combined_modifier_flags(binder.binding_node(lazy[1])),
                        0
                    );
                    Ok(())
                })
                .expect("valid root admits local scope")
        })
        .unwrap();
}

fn invoke_strict_predicate<'scope>(
    binder: &mut Binder<'_, 'scope, '_>,
    kind: SyntaxKind,
    node: crate::target::BindingNode<'scope>,
) {
    match kind {
        SyntaxKind::BinaryExpression => binder.check_strict_mode_binary_expression(node),
        SyntaxKind::PrefixUnaryExpression => binder.check_strict_mode_prefix_unary_expression(node),
        SyntaxKind::PostfixUnaryExpression => {
            binder.check_strict_mode_postfix_unary_expression(node);
        }
        SyntaxKind::CatchClause => binder.check_strict_mode_catch_clause(node),
        SyntaxKind::LabeledStatement => binder.check_strict_mode_labeled_statement(node),
        _ => unreachable!(),
    }
}
