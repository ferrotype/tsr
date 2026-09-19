use std::panic::{catch_unwind, AssertUnwindSafe};
use ts_arena::Counters;
use ts_ast::{
    deep_clone_node, deep_clone_reparse, modifier_flags, node_flags, AstBuilder, ChildRole,
    Factory, FactoryMethods, JsString, NodeId, NodeSlice, NodeVisitor, NodeVisitorHooks,
    RuntimeFactory, SyntaxKind, VisitContext,
};
use ts_core::TextRange;
use ts_jsstring::SourceText;
fn builder() -> AstBuilder {
    AstBuilder::new(SourceText::from_loaded_bytes(&b""[..]), &Counters::new())
}
fn identifier(b: &mut AstBuilder, name: &[u8]) -> NodeId {
    b.new_identifier(JsString::from_bytes(name))
}
fn identity(_: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
    node
}

#[test]
fn slice_visits_nil_and_flattens_replacements_then_honors_disabled_callback() {
    let mut b = builder();
    let a = identifier(&mut b, b"a");
    let c = identifier(&mut b, b"c");
    let expansion = b.node_slice(vec![Some(c), None]).unwrap();
    let syntax = b.new_syntax_list(expansion);
    let input = b.node_slice(vec![None, Some(a), Some(a), Some(c)]).unwrap();
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
    let mut v = NodeVisitor::new(Some(&visit), Some(&mut b), NodeVisitorHooks::default());
    let (result, changed) = v.visit_slice(input);
    assert!(changed);
    assert_eq!(calls.get(), 3);
    assert_eq!(
        &v.factory().read_nodes(result).iter().collect::<Vec<_>>(),
        &[Some(c), None, Some(c), None, Some(c)]
    );
    let (nil, changed) = v.visit_slice(NodeSlice::empty());
    assert!(nil.is_nil());
    assert!(!changed);
}

#[test]
fn unchanged_syntax_list_is_not_flattened_and_node_lifting_checks_exact_contracts() {
    let mut b = builder();
    let a = identifier(&mut b, b"a");
    let one = b.node_slice(vec![Some(a)]).unwrap();
    let syntax = b.new_syntax_list(one);
    let input = b.node_slice(vec![Some(syntax)]).unwrap();
    let mut v = NodeVisitor::new(Some(&identity), Some(&mut b), NodeVisitorHooks::default());
    assert_eq!(v.visit_slice(input), (input, false));
    assert_eq!(v.visit_node(Some(syntax)), Some(a));
    let empty = v.new_syntax_list(NodeSlice::empty());
    let error = catch_unwind(AssertUnwindSafe(|| v.visit_node(Some(empty)))).unwrap_err();
    assert_eq!(
        error.downcast_ref::<&str>().copied(),
        Some("Expected only a single node to be written to output")
    );
}

#[test]
fn private_embedded_fallback_lifts_nil_while_public_path_and_token_skip_general_hook() {
    let mut b = builder();
    let token = b.new_token(SyntaxKind::ThisKeyword.into());
    let remove = |_: &mut NodeVisitor<'_>, _: Option<NodeId>| None;
    let hooks = NodeVisitorHooks {
        visit_node: Some(&remove),
        ..NodeVisitorHooks::default()
    };
    let mut v = NodeVisitor::new(Some(&identity), Some(&mut b), hooks);
    assert_eq!(
        VisitContext::visit_node(&mut v, Some(token), ChildRole::Token),
        Some(token)
    );
    assert_eq!(v.visit_embedded_statement(Some(token)), Some(token));
    let block =
        VisitContext::visit_node(&mut v, Some(token), ChildRole::EmbeddedStatement).unwrap();
    let list = v
        .node(block)
        .data_source()
        .as_block()
        .unwrap()
        .statements()
        .unwrap();
    assert!(v.factory().read_list(list).nodes().is_empty());
    v.visit = Some(&remove);
    assert_eq!(v.visit_embedded_statement(Some(token)), None);
}

#[test]
fn changed_modifiers_recompute_flags_and_preserve_callback_mutated_location() {
    let mut b = builder();
    let public = b.new_modifier(SyntaxKind::PublicKeyword.into());
    let private = b.new_modifier(SyntaxKind::PrivateKeyword.into());
    let input = b.node_slice(vec![Some(public)]).unwrap();
    let list = b.new_modifier_list(input);
    b.list_mut(list).unwrap().set_modifier_flags(u32::MAX);
    let visit = |v: &mut NodeVisitor<'_>, _: Option<NodeId>| {
        v.factory_mut()
            .mutable_list(list)
            .set_loc(TextRange::new(9, 12));
        Some(private)
    };
    let mut v = NodeVisitor::new(Some(&visit), Some(&mut b), NodeVisitorHooks::default());
    let new = v.visit_modifiers(Some(list)).unwrap();
    assert_ne!(new, list);
    assert_eq!(
        v.factory().read_list(new).modifier_flags(),
        modifier_flags::PRIVATE
    );
    assert_eq!(v.factory().read_list(new).loc(), TextRange::new(9, 12));
}

#[test]
fn missing_list_follows_backing_through_new_headers_and_replacement() {
    let mut b = builder();
    let list = b
        .new_list(TextRange::new(0, 0), NodeSlice::empty())
        .unwrap();
    b.mark_list_missing(list).unwrap();
    let nodes = b.view().list(list).unwrap().nodes();
    assert!(!nodes.is_nil());
    let other = b.new_list(TextRange::new(0, 0), nodes).unwrap();
    assert!(b.view().list(other).unwrap().is_missing());
    assert!(nodes.same(NodeSlice::empty()));
    b.set_list_nodes(list, NodeSlice::empty()).unwrap();
    assert!(!b.view().list(list).unwrap().is_missing());
    assert!(b.view().list(other).unwrap().is_missing());
}

#[test]
fn deep_clone_retains_imports_and_preserves_trailing_comma_with_synthetic_locations() {
    let mut source = builder();
    let child = identifier(&mut source, b"child");
    source
        .node_mut(child)
        .unwrap()
        .set_range(TextRange::new(1, 6));
    let edges = source.node_slice(vec![Some(child)]).unwrap();
    let list = source.new_list(TextRange::new(0, 7), edges).unwrap();
    let root = source.new_array_literal_expression(Some(list), false);
    source
        .node_mut(root)
        .unwrap()
        .set_range(TextRange::new(0, 8));
    let file = source.complete(root).unwrap().publish_unbound();
    let mut destination = builder();
    destination.retain_file(file);
    let cloned = deep_clone_node(&mut destination, Some(root)).unwrap();
    assert_ne!(root, cloned);
    let data = destination.node(cloned);
    assert_eq!(data.range(), TextRange::new(-1, -1));
    let list = data
        .data_source()
        .as_array_literal_expression()
        .unwrap()
        .elements()
        .unwrap();
    drop(data);
    assert_eq!(
        destination.view().list(list).unwrap().loc(),
        TextRange::new(-1, -1)
    );
    assert!(destination.view().list_has_trailing_comma(list).unwrap());
    let cloned_child = destination
        .view()
        .node_slice(destination.view().list(list).unwrap().nodes())
        .unwrap()
        .at(0)
        .unwrap();
    assert_ne!(child, cloned_child);
    assert_eq!(
        destination.node(cloned_child).range(),
        TextRange::new(-2, -2)
    );
    assert_eq!(destination.node(child).range(), TextRange::new(1, 6));
    let reparsed = deep_clone_reparse(&mut destination, Some(root)).unwrap();
    assert_eq!(destination.node(reparsed).range(), TextRange::new(0, 8));
    assert_ne!(destination.node(reparsed).flags() & node_flags::REPARSED, 0);
    let list = destination
        .node(reparsed)
        .data_source()
        .as_array_literal_expression()
        .unwrap()
        .elements()
        .unwrap();
    let child = destination
        .view()
        .node_slice(destination.view().list(list).unwrap().nodes())
        .unwrap()
        .at(0)
        .unwrap();
    assert_eq!(destination.node(child).parent(), Some(reparsed));
}

#[test]
fn parent_setter_preserves_prior_mutation_before_nil_list_child_panic() {
    let mut b = builder();
    let child = identifier(&mut b, b"child");
    let nodes = b.node_slice(vec![Some(child), None]).unwrap();
    let list = b.new_list(TextRange::new(0, 2), nodes).unwrap();
    let root = b.new_array_literal_expression(Some(list), false);
    assert!(
        catch_unwind(AssertUnwindSafe(|| ts_ast::set_parent_in_children(
            &mut b, root
        )))
        .is_err()
    );
    assert_eq!(b.node(child).parent(), Some(root));
}

#[test]
fn deep_clone_grows_a_small_native_stack() {
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            let mut b = builder();
            let mut root = identifier(&mut b, b"leaf");
            for _ in 0..5_000 {
                root = b.new_parenthesized_expression(Some(root));
            }
            let cloned = deep_clone_reparse(&mut b, Some(root)).unwrap();
            assert_ne!(cloned, root);
            assert!(matches!(
                b.node(cloned).data(),
                ts_ast::NodeDataRead::ParenthesizedExpression(_)
            ));
        })
        .unwrap()
        .join()
        .unwrap();
}
