use ts_arena::Counters;
use ts_ast::{
    subtree_flags as f, AstBuilder, Factory, FactoryMethods, JsString, NodeData, SyntaxKind,
};
use ts_jsstring::SourceText;

fn builder() -> AstBuilder {
    AstBuilder::new(SourceText::from_loaded_bytes(&b""[..]), &Counters::new())
}

#[test]
fn cache_survives_same_node_mutation_and_clone_recomputes() {
    let mut b = builder();
    let this = b.new_keyword_expression(SyntaxKind::ThisKeyword.into());
    let ident = b.new_identifier(JsString::from_bytes(&b"x"[..]));
    let outer = b.new_computed_property_name(Some(this));
    assert_eq!(b.view().subtree_facts(outer), f::LEXICAL_THIS);
    match b.node_mut(outer).unwrap().data_mut() {
        NodeData::ComputedPropertyName(d) => d.expression = Some(ident),
        _ => unreachable!(),
    }
    assert_eq!(b.view().subtree_facts(outer), f::LEXICAL_THIS);
    let clone = b.clone_computed_property_name(outer);
    assert_eq!(b.view().subtree_facts(clone), f::IDENTIFIER);
    let file = b.complete(outer).unwrap().publish_unbound();
    assert_eq!(file.view().subtree_facts(outer), f::LEXICAL_THIS);
}

#[test]
fn computed_cache_survives_a_temporary_noncomposite_payload() {
    let mut b = builder();
    let this = b.new_keyword_expression(SyntaxKind::ThisKeyword.into());
    let identifier = b.new_identifier(JsString::from_bytes(b"x".as_slice()));
    let outer = b.new_computed_property_name(Some(this));
    assert_eq!(b.view().subtree_facts(outer), f::LEXICAL_THIS);
    *b.node_mut(outer).unwrap().data_mut() = ts_ast::TokenData {}.into();
    assert!(b.node(outer).as_token().is_some());
    *b.node_mut(outer).unwrap().data_mut() = ts_ast::ComputedPropertyNameData {
        expression: Some(identifier),
    }
    .into();
    // Public payload replacement keeps the same Node and its existing cache.
    assert_eq!(b.view().subtree_facts(outer), f::LEXICAL_THIS);
    let cloned = b.clone_computed_property_name(outer);
    assert_eq!(b.view().subtree_facts(cloned), f::IDENTIFIER);
}

#[test]
fn scope_masks_preserve_transform_facts_and_exclude_lexical_markers() {
    let mut b = builder();
    let this = b.new_keyword_expression(SyntaxKind::ThisKeyword.into());
    let awaited = b.new_await_expression(Some(this));
    let function = b.new_node(
        SyntaxKind::FunctionExpression.into(),
        ts_ast::FunctionExpressionData {
            modifiers: None,
            type_parameters: None,
            parameters: None,
            r#type: None,
            full_signature: None,
            asterisk_token: None,
            body: Some(awaited),
            name: None,
        }
        .into(),
    );
    assert_eq!(
        b.view().subtree_facts(function),
        f::LEXICAL_THIS | f::AWAIT | f::ANY_AWAIT | f::FOR_AWAIT_OR_ASYNC_GENERATOR
    );
    assert_eq!(
        b.view().propagate_subtree_facts(Some(function)),
        f::ANY_AWAIT | f::FOR_AWAIT_OR_ASYNC_GENERATOR
    );
    let arrow = b.new_node(
        SyntaxKind::ArrowFunction.into(),
        ts_ast::ArrowFunctionData {
            modifiers: None,
            type_parameters: None,
            parameters: None,
            r#type: None,
            full_signature: None,
            equals_greater_than_token: None,
            asterisk_token: None,
            body: Some(awaited),
        }
        .into(),
    );
    assert_eq!(
        b.view().propagate_subtree_facts(Some(arrow)),
        f::LEXICAL_THIS | f::ANY_AWAIT | f::FOR_AWAIT_OR_ASYNC_GENERATOR
    );
}

#[test]
fn erased_types_do_not_visit_their_stored_children() {
    let mut b = builder();
    let malformed = b.new_node(
        SyntaxKind::CallExpression.into(),
        ts_ast::CallExpressionData {
            expression: None,
            question_dot_token: None,
            type_arguments: None,
            arguments: None,
        }
        .into(),
    );
    let declaration = b.new_node(
        SyntaxKind::VariableDeclaration.into(),
        ts_ast::VariableDeclarationData {
            name: None,
            exclamation_token: None,
            r#type: Some(malformed),
            initializer: None,
        }
        .into(),
    );
    assert_eq!(b.view().subtree_facts(declaration), f::TYPE_SCRIPT);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b
        .view()
        .subtree_facts(malformed)))
    .is_err());
}

#[test]
fn facts_walk_grows_a_small_native_stack_for_deep_valid_trees() {
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            let mut b = builder();
            let mut node = b.new_keyword_expression(SyntaxKind::ThisKeyword.into());
            for _ in 0..20_000 {
                node = b.new_parenthesized_expression(Some(node));
            }
            assert_eq!(b.view().subtree_facts(node), f::LEXICAL_THIS);
        })
        .unwrap()
        .join()
        .unwrap();
}
