//! Generated algorithms checked against pinned Go factory/dispatch witnesses.

use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};

use ts_arena::Counters;
use ts_ast::*;
use ts_core::TextRange;
use ts_jsstring::SourceText;

#[derive(Default)]
struct Hooks(Mutex<Vec<[i64; 7]>>);

impl Hooks {
    fn record(&self, stage: i64, factory: &dyn Factory, id: NodeId) {
        let node = factory.node(id);
        self.0.lock().unwrap().push([
            stage,
            i64::from(node.kind().raw()),
            i64::from(node.flags()),
            i64::from(node.pos()),
            i64::from(node.end()),
            factory.node_count(),
            factory.text_count(),
        ]);
    }
}

impl FactoryHooks for Hooks {
    fn on_create(&self, factory: &mut dyn Factory, id: NodeId) {
        self.record(0, factory, id);
        factory.set_node_flags(id, 0x8000_0000);
    }
    fn on_update(&self, factory: &mut dyn Factory, id: NodeId, _: NodeId) {
        self.record(1, factory, id);
    }
    fn on_clone(&self, factory: &mut dyn Factory, id: NodeId, _: NodeId) {
        self.record(2, factory, id);
    }
}

fn builder() -> AstBuilder {
    AstBuilder::new(
        SourceText::from_loaded_bytes(&b""[..]),
        &Counters::default(),
    )
}

#[test]
fn factory_masks_counts_and_hooks_follow_the_pinned_observation_order() {
    let hooks = Arc::new(Hooks::default());
    let mut factory = AstBuilder::with_hooks(
        SourceText::from_loaded_bytes(&b""[..]),
        &Counters::default(),
        hooks.clone(),
    );
    let name = factory.new_identifier(JsString::from_bytes(&b"id"[..]));
    let property = factory.new_property_access_expression(Some(name), None, Some(name), u32::MAX);
    assert_eq!(factory.node(property).flags(), 2_147_483_680);
    factory.node_mut(property).unwrap().set_flags(0xdead_beef);
    factory
        .node_mut(property)
        .unwrap()
        .set_range(TextRange::new(17, 29));
    let unchanged = factory.update_property_access_expression(
        property,
        Some(name),
        None,
        Some(name),
        0xdead_beef,
    );
    let changed =
        factory.update_property_access_expression(property, Some(name), None, None, 0xdead_beef);
    let cloned = factory.clone_property_access_expression(property);
    assert_eq!(unchanged, property);
    assert_ne!(changed, property);
    assert_ne!(cloned, property);
    // Values recorded by clean Go at pin 1f70213, including pre-mask OnCreate.
    assert_eq!(
        *hooks.0.lock().unwrap(),
        [
            [0, 79, 0, -1, -1, 1, 1],
            [0, 212, 0, -1, -1, 2, 1],
            [0, 212, 0, -1, -1, 3, 1],
            [1, 212, 3_735_928_559, 17, 29, 3, 1],
            [0, 212, 0, -1, -1, 4, 1],
            [1, 212, 3_735_928_559, 17, 29, 4, 1],
            [2, 212, 3_735_928_559, 17, 29, 4, 1],
        ]
    );
}

#[test]
fn raw_slice_updates_use_backing_identity_and_all_empty_slices_compare_same() {
    let mut factory = builder();
    let first = factory
        .text_slice(vec![JsString::from_bytes(&b"same"[..])])
        .unwrap();
    let second = factory
        .text_slice(vec![JsString::from_bytes(&b"same"[..])])
        .unwrap();
    let link = factory.new_js_doc_link(None, first);
    assert_eq!(factory.update_js_doc_link(link, None, first), link);
    let updated = factory.update_js_doc_link(link, None, second);
    assert_ne!(updated, link);
    let empty = factory.text_slice(vec![]).unwrap();
    let empty_link = factory.new_js_doc_link(None, TextSlice::empty());
    assert_eq!(
        factory.update_js_doc_link(empty_link, None, empty),
        empty_link
    );
    let cloned = factory.clone_js_doc_link(link);
    assert!(factory
        .node(cloned)
        .data()
        .as_js_doc_link()
        .unwrap()
        .text()
        .same(first));

    let missing = factory.new_case_or_default_clause(SyntaxKind::DefaultClause.into(), None, None);
    assert_eq!(
        factory
            .node(missing)
            .data()
            .as_case_or_default_clause()
            .unwrap()
            .expression(),
        None
    );
    let raw = factory.node_slice(vec![None, Some(missing)]).unwrap();
    let list = factory.new_syntax_list(raw);
    assert_eq!(
        factory
            .view()
            .node_slice(
                factory
                    .node(list)
                    .data()
                    .as_syntax_list()
                    .unwrap()
                    .children()
            )
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        &[None, Some(missing)]
    );
}

struct NoChildren;
impl ChildVisitor for NoChildren {
    fn visit_node(&mut self, _: NodeId) -> ControlFlow<()> {
        panic!("unexpected child");
    }
    fn visit_list(&mut self, _: NodeListId) -> ControlFlow<()> {
        panic!("unexpected list");
    }
    fn visit_node_slice(&mut self, _: NodeSlice) -> ControlFlow<()> {
        panic!("unexpected raw slice");
    }
}

struct IdentityVisitor<'a> {
    factory: &'a mut AstBuilder,
    calls: Vec<ChildRole>,
}
impl Factory for IdentityVisitor<'_> {
    fn node(&self, id: NodeId) -> NodeRead<'_> {
        self.factory.node(id)
    }
    fn node_mut(&mut self, id: NodeId) -> NodeMut<'_> {
        Factory::node_mut(self.factory, id)
    }
    fn node_count(&self) -> i64 {
        self.factory.node_count()
    }
    fn text_count(&self) -> i64 {
        self.factory.text_count()
    }
    fn new_node(&mut self, kind: NodeKind, data: NodeData) -> NodeId {
        self.factory.new_node(kind, data)
    }
    fn increment_text_count(&mut self) {
        self.factory.increment_text_count();
    }
    fn set_node_flags(&mut self, id: NodeId, flags: u32) {
        self.factory.set_node_flags(id, flags);
    }
    fn finish_update(&mut self, updated: NodeId, original: NodeId) -> NodeId {
        self.factory.finish_update(updated, original)
    }
    fn finish_clone(&mut self, updated: NodeId, original: NodeId) -> NodeId {
        self.factory.finish_clone(updated, original)
    }
}
impl VisitContext for IdentityVisitor<'_> {
    fn visit_node(&mut self, node: Option<NodeId>, role: ChildRole) -> Option<NodeId> {
        self.calls.push(role);
        node
    }
    fn visit_list(&mut self, list: Option<NodeListId>, role: ChildRole) -> Option<NodeListId> {
        self.calls.push(role);
        list
    }
    fn map_raw_nodes(&mut self, nodes: NodeSlice) -> NodeSlice {
        self.calls.push(ChildRole::RawNodes);
        nodes
    }
    fn visit_each_child_source_file(&mut self, _: NodeId) -> NodeId {
        panic!("Token payload must not dispatch SourceFile visitor");
    }
}

#[test]
fn enumeration_uses_kind_but_clone_and_transformation_use_payload() {
    let mut factory = builder();
    let mismatch = factory.new_token(SyntaxKind::SourceFile.into());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| factory
            .node(mismatch)
            .for_each_child(&mut NoChildren)))
        .is_err()
    );
    let unknown = factory.new_token(NodeKind::from_raw(-1));
    assert_eq!(
        factory.node(unknown).for_each_child(&mut NoChildren),
        ControlFlow::Continue(())
    );
    let cloned = factory.clone_node_generated(unknown).unwrap();
    assert_eq!(factory.node(cloned).kind().raw(), -1);
    let unknown_block = factory.new_node(
        NodeKind::from_raw(-1),
        BlockData {
            statements: None,
            multi_line: false,
        }
        .into(),
    );
    let mut visitor = IdentityVisitor {
        factory: &mut factory,
        calls: vec![],
    };
    assert_eq!(visitor.visit_each_child_generated(mismatch), mismatch);
    assert!(visitor.calls.is_empty());
    assert_eq!(
        visitor.visit_each_child_generated(unknown_block),
        unknown_block
    );
    assert_eq!(visitor.calls, [ChildRole::Nodes]);
}

#[test]
fn owner_validation_includes_references_omitted_from_child_enumeration() {
    let mut foreign = builder();
    let foreign_id = foreign.new_identifier(JsString::from_bytes(&b"foreign"[..]));
    let mut destination = builder();
    let data: NodeData = CallSignatureDeclarationData {
        type_parameters: None,
        parameters: None,
        r#type: None,
        full_signature: Some(foreign_id),
    }
    .into();
    // This is deliberately not a source visitor edge, but still needs an owner.
    assert_eq!(
        data.for_each_child(&mut NoChildren),
        ControlFlow::Continue(())
    );
    let result = data.validate_references(
        |id| destination.view().node(id).map(|_| ()),
        |id| destination.view().list(id).map(|_| ()),
        |nodes| destination.view().node_slice(nodes).map(|_| ()),
        |text| destination.view().text_slice(text).map(|_| ()),
    );
    assert_eq!(result, Err(ts_arena::Error::WrongOwner));
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        destination.new_node(SyntaxKind::CallSignature.into(), data)
    }))
    .is_err());
    assert_eq!(destination.node_count(), 0);
}

struct InterceptingFactory {
    inner: AstBuilder,
    calls: Vec<(NodeKind, i64, i64)>,
}
impl Factory for InterceptingFactory {
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
        self.calls
            .push((kind, self.node_count(), self.text_count()));
        self.inner.new_node(kind, data)
    }
    fn increment_text_count(&mut self) {
        self.inner.increment_text_count();
    }
    fn set_node_flags(&mut self, id: NodeId, flags: u32) {
        self.inner.set_node_flags(id, flags);
    }
    fn finish_update(&mut self, updated: NodeId, original: NodeId) -> NodeId {
        self.inner.finish_update(updated, original)
    }
    fn finish_clone(&mut self, updated: NodeId, original: NodeId) -> NodeId {
        self.inner.finish_clone(updated, original)
    }
}

#[test]
fn concrete_entries_preserve_custom_interception_and_prevalidation_text_counts() {
    let mut foreign = builder();
    let wrong = foreign.new_token(SyntaxKind::Unknown.into());
    let mut factory = InterceptingFactory {
        inner: builder(),
        calls: Vec::new(),
    };
    let first = factory.new_identifier(JsString::from_bytes(b"name".as_slice()));
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        factory.new_js_doc_link(Some(wrong), TextSlice::empty());
    }));
    assert!(failure.is_err());
    assert_eq!((factory.node_count(), factory.text_count()), (1, 2));
    let next = BorrowedFactory(&mut factory).new_token(NodeKind::from_raw(-1));
    assert_eq!(next.slot(), first.slot() + 1);
    assert_eq!(
        factory.calls,
        [
            (SyntaxKind::Identifier.into(), 0, 1),
            (SyntaxKind::JSDocLink.into(), 1, 2),
            (NodeKind::from_raw(-1), 1, 2),
        ]
    );
    assert_eq!(factory.node(first).as_identifier().unwrap().text(), b"name");
    assert_eq!(factory.node(next).kind(), NodeKind::from_raw(-1));
}

#[test]
fn concrete_entries_validate_in_field_order_before_node_allocation_and_hooks() {
    let hooks = Arc::new(Hooks::default());
    let mut factory =
        AstBuilder::with_hooks(SourceText::default(), &Counters::new(), hooks.clone());
    let first = factory.new_identifier(JsString::from_bytes(b"first".as_slice()));
    let mut foreign = builder();
    let wrong = foreign.new_token(SyntaxKind::Unknown.into());
    let text = TextSlice::empty();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        factory.new_js_doc_link(Some(wrong), text);
    }))
    .unwrap_err();
    assert_eq!(
        failure.downcast_ref::<String>().unwrap(),
        "factory edges belong to retained storage: WrongOwner"
    );
    assert_eq!((factory.node_count(), factory.text_count()), (1, 2));
    assert_eq!(hooks.0.lock().unwrap().len(), 1);
    let invalid = NodeId::from_parts(factory.id().arena(), u32::MAX).unwrap();
    let data = BinaryExpressionData {
        modifiers: None,
        left: Some(invalid),
        r#type: Some(wrong),
        operator_token: None,
        right: None,
    };
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        factory.new_binary_expression_data(NodeKind::from_raw(-1), data);
    }))
    .unwrap_err();
    // Schema order places left before type: local missing slot wins over wrong owner.
    assert_eq!(
        failure.downcast_ref::<String>().unwrap(),
        "factory edges belong to retained storage: InvalidSlot"
    );
    assert_eq!(factory.node_count(), 1);
    let next =
        BorrowedFactory(&mut factory).new_identifier(JsString::from_bytes(b"next".as_slice()));
    assert_eq!(next.slot(), first.slot() + 1);
    assert_eq!(
        *hooks.0.lock().unwrap(),
        [
            [0, i64::from(SyntaxKind::Identifier as u16), 0, -1, -1, 1, 1],
            [0, i64::from(SyntaxKind::Identifier as u16), 0, -1, -1, 2, 3],
        ]
    );
}

#[test]
fn concrete_large_payload_keeps_all_fields_forged_kind_and_lazy_compatibility() {
    let mut factory = builder();
    let left = factory.new_identifier(JsString::from_bytes(b"left".as_slice()));
    let right = factory.new_identifier(JsString::from_bytes(b"right".as_slice()));
    let operator = factory.new_token(SyntaxKind::PlusToken.into());
    let edges = factory
        .node_slice(vec![Some(left), None, Some(right)])
        .unwrap();
    let modifiers = factory.new_list(TextRange::new(-1, 5), edges).unwrap();
    let data = BinaryExpressionData {
        modifiers: Some(modifiers),
        left: Some(left),
        r#type: Some(right),
        operator_token: Some(operator),
        right: Some(right),
    };
    let kind = NodeKind::from_raw(-1);
    let concrete = factory.new_binary_expression_data(kind, data.clone());
    let generic = factory.new_node(kind, data.clone().into());
    assert_eq!((factory.node_count(), factory.text_count()), (5, 2));
    for id in [concrete, generic] {
        let node = factory.node(id);
        assert_eq!(node.kind(), kind);
        assert_eq!(node.data().to_owned(), NodeData::from(data.clone()));
        assert_eq!(
            (node.parent(), node.flags(), node.pos(), node.end()),
            (None, 0, -1, -1)
        );
        assert_eq!(NodeAccess::cached_subtree_facts(&node), 0);
        assert_eq!(existing_runtime_node_id(&node), 0);
    }
    let root = factory.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/concrete.ts".as_slice()),
            ..SourceFileParseOptions::default()
        },
        SourceText::default(),
        Some(modifiers),
        Some(operator),
    );
    let file = factory.complete(root).unwrap().publish_unbound();
    assert_eq!(
        file.view().source_file(root).unwrap().file_name(),
        b"/concrete.ts"
    );
    let docs = file
        .view()
        .jsdoc(root, |transaction| {
            let lazy =
                BorrowedFactory(&mut *transaction).new_binary_expression_data(kind, data.clone());
            assert_eq!(
                Factory::node(transaction, lazy).data().to_owned(),
                NodeData::from(data.clone())
            );
            assert_eq!(transaction.node_count(), 1);
            Ok(vec![lazy])
        })
        .unwrap();
    let lazy = file.view().node(docs[0]).unwrap();
    assert_eq!(lazy.kind(), kind);
    assert_eq!(lazy.data().to_owned(), NodeData::from(data));
}
