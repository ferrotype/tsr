use super::NodeRead;
use crate::{
    AstBuilder, Factory, FactoryMethods, FunctionExpressionData, JsString, Node, NodeAccess,
    NodeData, NodeId, NodeListId, SyntaxKind as K,
};
use tsr_arena::Counters;
use tsr_core::TextRange;
use tsr_jsstring::SourceText;

fn function_data(name: NodeId, list: Option<NodeListId>, body: NodeId) -> FunctionExpressionData {
    FunctionExpressionData {
        modifiers: list,
        type_parameters: list,
        parameters: list,
        r#type: Some(name),
        full_signature: None,
        asterisk_token: None,
        body: Some(body),
        name: Some(name),
    }
}

fn assert_function_fields(read: &NodeRead<'_>, data: &FunctionExpressionData) {
    assert_eq!(read.name(), data.name);
    assert_eq!(read.modifiers(), data.modifiers);
    assert_eq!(read.body(), data.body);
    assert_eq!(read.parameter_list(), data.parameters);
    assert_eq!(read.type_parameter_list(), data.type_parameters);
    assert_eq!(read.type_node(), data.r#type);
    assert_eq!(
        NodeAccess::function_fields(read),
        Some((data.type_parameters, data.parameters, data.r#type))
    );
}

#[test]
fn typed_shape_fields_preserve_foreign_edges_and_reborrow_selection() {
    let counters = Counters::new();
    let source = SourceText::from_loaded_bytes(b"foreign".as_slice());
    let mut foreign = AstBuilder::new(source, &counters);
    let name = foreign.new_identifier(JsString::from_bytes(b"\xffname".as_slice()));
    let foreign = foreign.complete(name).unwrap().publish_unbound();

    let mut builder = AstBuilder::new(SourceText::default(), &counters);
    builder.retain_file(foreign);
    let nodes = builder.node_slice(vec![Some(name)]).unwrap();
    let list = builder.new_list(TextRange::new(-1, -1), nodes).unwrap();
    let data = function_data(name, Some(list), name);
    let root = builder.new_function_expression_data(K::FunctionExpression.into(), data.clone());
    let parsed = builder.complete(root).unwrap();
    let file = parsed.publish_unbound();
    let plain = file.view().node(root).unwrap();
    assert_eq!(plain.typed_payload_shape(), None);
    assert_function_fields(&plain, &data);
    let typed = file
        .view()
        .node(root)
        .unwrap()
        .with_typed_payload_fields(true);
    assert!(typed.typed_payload_shape().is_some());
    assert_function_fields(&typed, &data);
    let borrowed = typed.as_borrowed().unwrap();
    assert!(borrowed.typed_payload_shape().is_some());
    assert_function_fields(&borrowed, &data);
    assert_eq!(
        file.view()
            .node(typed.name().unwrap())
            .unwrap()
            .as_identifier()
            .unwrap()
            .text(),
        b"\xffname"
    );
    assert_eq!(
        typed.with_typed_payload_fields(false).typed_payload_shape(),
        None
    );
}

#[test]
fn typed_selection_preserves_open_kind_shape_pairs_and_panic_text() {
    let counters = Counters::new();
    let mut builder = AstBuilder::new(SourceText::default(), &counters);
    let name = builder.new_identifier(JsString::from_bytes(b"name".as_slice()));
    let data = function_data(name, None, name);
    // Factory nodes deliberately permit this mismatch: name/body and function
    // fields select by shape, whereas expression must retain its kind-specific
    // interface-conversion panic. Test both compact and owned construction.
    let root = builder.new_node(K::CallExpression.into(), data.clone().into());
    let owned = Node::from_factory_parts(K::CallExpression.into(), data.clone().into());
    let owned_read = NodeRead::owned_file(root, &owned, builder.view().0.physical_owner());
    for typed in [false, true] {
        let owned_read = owned_read
            .as_borrowed()
            .unwrap()
            .with_typed_payload_fields(typed);
        assert_eq!(owned_read.typed_payload_shape(), None);
        assert_eq!(owned_read.name(), data.name);
        assert_eq!(owned_read.body(), data.body);
        assert_eq!(
            NodeAccess::function_fields(&owned_read),
            Some((None, None, data.r#type))
        );
        let compact = builder
            .view()
            .node(root)
            .unwrap()
            .with_typed_payload_fields(typed);
        assert_eq!(compact.name(), data.name);
        assert_eq!(compact.body(), data.body);
        assert_eq!(
            NodeAccess::function_fields(&compact),
            Some((None, None, data.r#type))
        );
        for read in [&compact, &owned_read] {
            let error =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read.expression()))
                    .unwrap_err();
            let message = error.downcast_ref::<String>().unwrap();
            assert_eq!(
                message,
                "interface conversion: ast.nodeData is *ast.FunctionExpression, not *ast.CallExpression"
            );
        }
    }
    drop(owned_read);
    let token = builder.new_node(K::Identifier.into(), NodeData::Token(crate::TokenData {}));
    let read = builder
        .view()
        .node(token)
        .unwrap()
        .with_typed_payload_fields(true);
    assert_eq!(read.name(), None);
    assert_eq!(read.modifiers(), None);
    assert_eq!(read.body(), None);
    assert_eq!(NodeAccess::function_fields(&read), None);
}

#[test]
fn lazy_and_retained_payloads_keep_the_guarded_fallback() {
    let counters = Counters::new();
    let mut builder = AstBuilder::new(SourceText::default(), &counters);
    let name = builder.new_identifier(JsString::from_bytes(b"name".as_slice()));
    let file = builder.complete(name).unwrap().publish_unbound();
    let data = function_data(name, None, name);
    let roots = file
        .view()
        .jsdoc(name, |transaction| {
            let lazy = transaction
                .new_function_expression_data(K::FunctionExpression.into(), data.clone());
            let read = transaction.node(lazy)?.with_typed_payload_fields(true);
            assert_eq!(read.typed_payload_shape(), None);
            assert_function_fields(&read, &data);
            Ok(vec![lazy])
        })
        .unwrap();
    let lazy = roots[0];
    for typed in [false, true] {
        let read = file
            .view()
            .node(lazy)
            .unwrap()
            .with_typed_payload_fields(typed);
        assert_eq!(read.typed_payload_shape(), None);
        assert_function_fields(&read, &data);
        assert!(read.as_borrowed().is_none());
    }
    let retained = file.retain_node(lazy).unwrap();
    drop(roots);
    drop(file);
    let read = retained.read().with_typed_payload_fields(true);
    assert_eq!(read.typed_payload_shape(), None);
    assert_function_fields(&read, &data);
    assert_function_fields(&read.as_borrowed().unwrap(), &data);
}
