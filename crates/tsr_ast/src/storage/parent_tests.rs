use super::*;
use crate::{
    BinaryExpressionData, BorrowedFactory, CallExpressionData, CallSignatureDeclarationData,
    ChildVisitor, Factory, FactoryMethods, JSDocParameterOrPropertyTagData, NodeKind,
    QualifiedNameData, RuntimeFactory, SyntaxKind, SyntaxListData, TokenData,
};
use std::{
    ops::ControlFlow,
    panic::{catch_unwind, AssertUnwindSafe},
};

fn message(failure: Box<dyn std::any::Any + Send>) -> String {
    match failure.downcast::<String>() {
        Ok(message) => *message,
        Err(failure) => failure.downcast::<&str>().unwrap().to_string(),
    }
}

fn identifier(builder: &mut AstBuilder) -> NodeId {
    builder.new_identifier(JsString::default())
}

#[test]
fn parent_attachment_preserves_sparse_lists_raw_slices_and_nonvisitor_fields() {
    let mut builder = AstBuilder::new(SourceText::default(), &Counters::new());
    let left = identifier(&mut builder);
    let right = identifier(&mut builder);
    let omitted = identifier(&mut builder);
    let operator = builder.new_token(SyntaxKind::PlusToken.into());
    let elements = builder
        .node_slice(vec![None, Some(right), None, Some(left)])
        .unwrap();
    let modifiers = builder.new_list(TextRange::new(-1, -1), elements).unwrap();
    let root = builder.new_binary_expression_data(
        SyntaxKind::BinaryExpression.into(),
        BinaryExpressionData {
            modifiers: Some(modifiers),
            left: Some(left),
            r#type: None,
            operator_token: Some(operator),
            right: Some(right),
        },
    );
    assert!(builder.override_core_parents(root));
    for id in [left, right, operator] {
        assert_eq!(builder.view().node(id).unwrap().parent(), Some(root));
    }
    assert_eq!(builder.view().node(omitted).unwrap().parent(), None);
    let signature = builder.new_call_signature_declaration_data(
        SyntaxKind::CallSignature.into(),
        CallSignatureDeclarationData {
            type_parameters: None,
            parameters: None,
            r#type: None,
            full_signature: Some(omitted),
        },
    );
    assert!(builder.override_core_parents(signature));
    assert_eq!(builder.view().node(omitted).unwrap().parent(), None);
    let raw = builder.new_syntax_list_data(
        SyntaxKind::SyntaxList.into(),
        SyntaxListData { children: elements },
    );
    let mut scratch = Vec::new();
    BorrowedFactory(&mut builder).override_parent_in_immediate_children(raw, &mut scratch);
    assert!(scratch.is_empty());
    for id in [left, right] {
        assert_eq!(builder.view().node(id).unwrap().parent(), Some(raw));
    }
    assert!(builder.construction_edges_valid);
    builder.complete(root).unwrap();
}

#[test]
fn parent_attachment_kind_mismatch_precedes_writes_and_unknown_kinds_have_no_children() {
    let mut builder = AstBuilder::new(SourceText::default(), &Counters::new());
    let child = identifier(&mut builder);
    let mismatch = builder.new_qualified_name_data(
        SyntaxKind::BinaryExpression.into(),
        QualifiedNameData {
            left: Some(child),
            right: None,
        },
    );
    let failure =
        catch_unwind(AssertUnwindSafe(|| builder.override_core_parents(mismatch))).unwrap_err();
    assert_eq!(
        message(failure),
        "BinaryExpression kind requires BinaryExpression payload"
    );
    assert_eq!(builder.view().node(child).unwrap().parent(), None);
    let unknown = builder.new_qualified_name_data(
        NodeKind::from_raw(-1),
        QualifiedNameData {
            left: Some(child),
            right: None,
        },
    );
    assert!(builder.override_core_parents(unknown));
    assert_eq!(builder.view().node(child).unwrap().parent(), None);
}

#[test]
fn parent_attachment_keeps_gather_failures_and_retained_scratch_before_write_failures() {
    let mut builder = AstBuilder::new(SourceText::default(), &Counters::new());
    let child = identifier(&mut builder);
    let late = identifier(&mut builder);
    let empty = builder
        .new_list(TextRange::new(-1, -1), NodeSlice::empty())
        .unwrap();
    let root = builder.new_call_expression_data(
        SyntaxKind::CallExpression.into(),
        CallExpressionData {
            expression: Some(child),
            question_dot_token: None,
            type_arguments: None,
            arguments: Some(empty),
        },
    );
    builder.list_mut(empty).unwrap().set_nodes(NodeSlice {
        backing: None,
        start: 0,
        len: 1,
    });
    assert!(!builder.override_core_parents(root));
    let mut scratch = Vec::new();
    let failure = catch_unwind(AssertUnwindSafe(|| {
        builder.override_parent_in_immediate_children(root, &mut scratch);
    }))
    .unwrap_err();
    assert_eq!(message(failure), "factory slice: InvalidGraph");
    assert_eq!(scratch, [child]);
    assert_eq!(builder.view().node(child).unwrap().parent(), None);
    // Recovery retains the original scratch prefix; successful gathering appends.
    let elements = builder.node_slice(vec![None, Some(late)]).unwrap();
    builder.set_list_nodes(empty, elements).unwrap();
    builder.override_parent_in_immediate_children(root, &mut scratch);
    assert!(scratch.is_empty());
    assert_eq!(builder.view().node(child).unwrap().parent(), Some(root));
    assert_eq!(builder.view().node(late).unwrap().parent(), Some(root));

    // An invalid child ID differs: gathering succeeds and the earlier write stays.
    builder.write_parent(child, None).unwrap();
    let invalid = NodeId::from_parts(builder.id().arena(), u32::MAX).unwrap();
    builder.node_mut(root).unwrap().data = QualifiedNameData {
        left: Some(child),
        right: Some(invalid),
    }
    .into();
    builder.node_mut(root).unwrap().kind = SyntaxKind::QualifiedName.into();
    let failure = catch_unwind(AssertUnwindSafe(|| {
        builder.override_parent_in_immediate_children(root, &mut scratch);
    }))
    .unwrap_err();
    assert_eq!(
        message(failure),
        "factory owns mutable core node: InvalidSlot"
    );
    assert_eq!(builder.view().node(child).unwrap().parent(), Some(root));
    assert!(scratch.is_empty());
}

#[test]
fn parent_attachment_escaped_parent_cleanup_and_import_failure_use_compatibility_path() {
    let counters = Counters::new();
    let mut other = AstBuilder::new(SourceText::default(), &counters);
    let foreign = identifier(&mut other);
    let file = other.complete(foreign).unwrap().publish_unbound();
    let mut builder = AstBuilder::new(SourceText::default(), &counters);
    let child = identifier(&mut builder);
    let root = builder.new_parenthesized_expression(Some(child));
    builder.write_parent(child, Some(foreign)).unwrap();
    assert!(!builder.override_core_parents(root));
    builder.override_parent_in_immediate_children(root, &mut Vec::new());
    assert!(!builder.storage.store().has_link_escapes());
    assert_eq!(builder.view().node(child).unwrap().parent(), Some(root));
    builder.retain_file(file);
    let imported = builder.new_qualified_name(Some(child), Some(foreign));
    assert!(!builder.override_core_parents(imported));
    let failure = catch_unwind(AssertUnwindSafe(|| {
        builder.override_parent_in_immediate_children(imported, &mut Vec::new());
    }))
    .unwrap_err();
    assert_eq!(
        message(failure),
        "factory owns mutable core node: WrongOwner"
    );
    assert_eq!(builder.view().node(child).unwrap().parent(), Some(imported));
}

#[test]
fn stored_child_dispatch_preserves_dynamic_jsdoc_order() {
    struct Children(Vec<NodeId>);
    impl ChildVisitor for Children {
        fn visit_node(&mut self, id: NodeId) -> ControlFlow<()> {
            self.0.push(id);
            ControlFlow::Continue(())
        }
        fn visit_list(&mut self, _: NodeListId) -> ControlFlow<()> {
            panic!("fixture has no list")
        }
        fn visit_node_slice(&mut self, _: NodeSlice) -> ControlFlow<()> {
            panic!("fixture has no slice")
        }
    }
    let mut builder = AstBuilder::new(SourceText::default(), &Counters::new());
    let tag = identifier(&mut builder);
    let name = identifier(&mut builder);
    let typ = identifier(&mut builder);
    for name_first in [false, true] {
        let root = builder.new_js_doc_parameter_or_property_tag_data(
            SyntaxKind::JSDocParameterTag.into(),
            JSDocParameterOrPropertyTagData {
                tag_name: Some(tag),
                comment: None,
                name: Some(name),
                is_bracketed: false,
                type_expression: Some(typ),
                is_name_first: name_first,
            },
        );
        let read = builder.view().node(root).unwrap();
        let mut expected = Children(Vec::new());
        let _ = read.for_each_child(&mut expected);
        let header = builder.storage.core_node(root).unwrap();
        let context = read.compact_context();
        let mut direct = Children(Vec::new());
        let _ = context.store.payloads.for_each_stored_child(
            header.kind,
            header.actual_shape(),
            header.ordinal,
            header.end,
            context,
            &mut direct,
        );
        assert_eq!(
            expected.0,
            if name_first {
                vec![tag, name, typ]
            } else {
                vec![tag, typ, name]
            }
        );
        assert_eq!(direct.0, expected.0);
        assert!(builder.override_core_parents(root));
    }
}

#[test]
fn parent_attachment_lazy_transaction_uses_existing_staged_mutation() {
    let mut builder = AstBuilder::new(SourceText::default(), &Counters::new());
    let root = builder.new_node(NodeKind::from_raw(-1), TokenData {}.into());
    let file = builder.complete(root).unwrap().publish_unbound();
    let roots = file
        .view()
        .jsdoc(root, |transaction| {
            let child = transaction.new_identifier(JsString::default());
            let parent = transaction.new_parenthesized_expression(Some(child));
            BorrowedFactory(&mut *transaction)
                .override_parent_in_immediate_children(parent, &mut Vec::new());
            assert_eq!(transaction.node(child)?.parent(), Some(parent));
            Ok(vec![parent])
        })
        .unwrap();
    let parent = roots[0];
    let read = file.view().node(parent).unwrap();
    let child = read
        .as_parenthesized_expression()
        .unwrap()
        .expression()
        .unwrap();
    assert_eq!(file.view().node(child).unwrap().parent(), Some(parent));
}

#[test]
fn parent_attachment_default_and_borrowed_dispatch_observe_hook_edits_and_write_order() {
    struct Hook;
    impl crate::FactoryHooks for Hook {
        fn on_create(&self, factory: &mut dyn Factory, id: NodeId) {
            let read = factory.node(id);
            if read.kind() != SyntaxKind::CallExpression {
                return;
            }
            let expression = read.as_call_expression().unwrap().expression();
            drop(read);
            // A hook can change the shape after creation. Finishing reads it anew.
            let mut node = factory.node_mut(id);
            node.kind = SyntaxKind::QualifiedName.into();
            node.data = QualifiedNameData {
                left: expression,
                right: expression,
            }
            .into();
        }
    }
    struct RecordingFactory {
        inner: AstBuilder,
        writes: Vec<NodeId>,
    }
    impl Factory for RecordingFactory {
        fn node(&self, id: NodeId) -> NodeRead<'_> {
            Factory::node(&self.inner, id)
        }
        fn node_mut(&mut self, id: NodeId) -> NodeMut<'_> {
            Factory::node_mut(&mut self.inner, id)
        }
        fn node_count(&self) -> i64 {
            self.inner.node_count()
        }
        fn text_count(&self) -> i64 {
            self.inner.text_count()
        }
        fn new_node(&mut self, kind: NodeKind, data: NodeData) -> NodeId {
            self.inner.new_node(kind, data)
        }
        fn increment_text_count(&mut self) {
            self.inner.increment_text_count();
        }
        fn set_node_flags(&mut self, id: NodeId, flags: u32) {
            self.inner.set_node_flags(id, flags);
        }
        fn set_node_parent(&mut self, id: NodeId, parent: Option<NodeId>) {
            self.writes.push(id);
            self.inner.set_node_parent(id, parent);
        }
        fn finish_update(&mut self, id: NodeId, original: NodeId) -> NodeId {
            self.inner.finish_update(id, original)
        }
        fn finish_clone(&mut self, id: NodeId, original: NodeId) -> NodeId {
            self.inner.finish_clone(id, original)
        }
    }
    impl RuntimeFactory for RecordingFactory {
        fn read_list(&self, id: NodeListId) -> NodeListRead<'_> {
            self.inner.read_list(id)
        }
        fn read_nodes(&self, slice: NodeSlice) -> NodeSliceRead<'_> {
            self.inner.read_nodes(slice)
        }
        fn alloc_nodes(&mut self, nodes: Vec<Option<NodeId>>) -> NodeSlice {
            self.inner.alloc_nodes(nodes)
        }
        fn alloc_list(&mut self, loc: TextRange, nodes: NodeSlice) -> NodeListId {
            self.inner.alloc_list(loc, nodes)
        }
        fn mutable_list(&mut self, id: NodeListId) -> &mut NodeList {
            self.inner.mutable_list(id)
        }
        fn clone_source(&mut self, original: NodeId) -> NodeId {
            self.inner.clone_source(original)
        }
        fn update_source(
            &mut self,
            original: NodeId,
            statements: Option<NodeListId>,
            eof: Option<NodeId>,
        ) -> NodeId {
            self.inner.update_source(original, statements, eof)
        }
    }
    let inner = AstBuilder::with_hooks(SourceText::default(), &Counters::new(), Arc::new(Hook));
    let mut factory = RecordingFactory {
        inner,
        writes: Vec::new(),
    };
    let expression = factory.new_identifier(JsString::default());
    let ignored_argument = factory.new_identifier(JsString::default());
    let elements = factory.alloc_nodes(vec![Some(ignored_argument)]);
    let arguments = factory.alloc_list(TextRange::new(-1, -1), elements);
    let parent = factory.new_call_expression_data(
        SyntaxKind::CallExpression.into(),
        CallExpressionData {
            expression: Some(expression),
            question_dot_token: None,
            type_arguments: None,
            arguments: Some(arguments),
        },
    );
    assert!(!factory.inner.override_core_parents(parent));
    BorrowedFactory(&mut factory).override_parent_in_immediate_children(parent, &mut Vec::new());
    assert_eq!(factory.writes, [expression, expression]);
    assert_eq!(factory.node(ignored_argument).parent(), None);
    assert_eq!(factory.node(expression).parent(), Some(parent));

    let first = factory.new_identifier(JsString::default());
    let last = factory.new_identifier(JsString::default());
    let slice = factory.alloc_nodes(vec![None, Some(first), None, Some(last), Some(first)]);
    let raw = factory.new_syntax_list_data(
        SyntaxKind::SyntaxList.into(),
        SyntaxListData { children: slice },
    );
    factory.writes.clear();
    BorrowedFactory(&mut factory).override_parent_in_immediate_children(raw, &mut Vec::new());
    assert_eq!(factory.writes, [first, last, first]);
}
