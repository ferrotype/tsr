use super::*;
use tsr_ast::{FactoryMethods, JsString};

#[test]
fn contextual_operation_checks_real_nested_expression_and_restores_modes() {
    let (_counters, _generation, _identity, owner) = crate::tests::owner();
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    let literal = state
        .factory
        .new_string_literal(JsString::from_bytes(b"value".as_slice()), 0);
    let expression = state.factory.new_parenthesized_expression(Some(literal));
    let previous = state.factory.new_token(K::TrueKeyword.into());
    state.current_node = Some(previous);
    state.expression_mode = 16;
    let regular = state
        .get_string_literal_type(JsString::from_bytes(b"value".as_slice()))
        .unwrap();
    let inferred = state.new_inference_context(&[], None, 0).unwrap();
    state
        .inference_context_mut(inferred)
        .unwrap()
        .intra_expression_sites
        .push((previous, regular));
    assert_eq!(
        check(
            state,
            ArgumentRequest {
                node: expression,
                contextual: regular,
                inference: Some(inferred),
                mode: 4,
            },
        ),
        Ok(regular)
    );
    assert_eq!(state.current_node, Some(previous));
    assert_eq!(state.expression_mode, 16);
    assert!(state.calls.contexts.is_empty());
    assert!(state.calls.inference_contexts.is_empty());
    assert!(state
        .inference_context(inferred)
        .unwrap()
        .intra_expression_sites
        .is_empty());
}

#[test]
fn expression_error_preserves_outer_context_and_inference_sites() {
    let (_counters, _generation, _identity, owner) = crate::tests::owner();
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    let number = state.builtins.number_type;
    let argument = state.new_synthetic_expression(number, false, None).unwrap();
    let outer = state.factory.new_token(K::FalseKeyword.into());
    let inference = state.new_inference_context(&[], None, 0).unwrap();
    state
        .inference_context_mut(inference)
        .unwrap()
        .intra_expression_sites
        .push((argument, number));
    state.calls.contexts.push(crate::calls::ArgumentContext {
        node: outer,
        ty: None,
        is_cache: true,
    });
    state.calls.inference_contexts.push((outer, None));
    let failure = Error::MissingLink("injected expression failure");
    assert_eq!(
        check_with_expression(
            state,
            ArgumentRequest {
                node: argument,
                contextual: number,
                inference: Some(inference),
                mode: 4,
            },
            |state, node, mode| {
                assert_eq!(node, argument);
                assert_eq!(mode, 4 | 1 | 2);
                let context = state.calls.contexts.last().unwrap();
                assert_eq!(context.node, argument);
                assert_eq!(context.ty, Some(number));
                assert!(!context.is_cache);
                assert_eq!(
                    state.calls.inference_contexts.last(),
                    Some(&(argument, Some(inference)))
                );
                Err(failure.clone())
            },
        ),
        Err(failure)
    );
    assert_eq!(state.calls.contexts.len(), 1);
    assert_eq!(state.calls.contexts[0].node, outer);
    assert!(state.calls.contexts[0].is_cache);
    assert_eq!(state.calls.inference_contexts, [(outer, None)]);
    assert_eq!(
        state
            .inference_context(inference)
            .unwrap()
            .intra_expression_sites,
        [(argument, number)]
    );
}

#[test]
fn malformed_jsx_context_fails_before_expression_or_stack_mutation() {
    let (_counters, _generation, _identity, owner) = crate::tests::owner();
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    let attributes = state.factory.new_jsx_attributes(None);
    let number = state.builtins.number_type;
    assert_eq!(
        check_with_expression(
            state,
            ArgumentRequest {
                node: attributes,
                contextual: number,
                inference: None,
                mode: 0,
            },
            |_, _, _| panic!("malformed JSX must fail before expression execution"),
        ),
        Err(Error::MissingLink("JSX attributes parent"))
    );
    assert!(state.calls.contexts.is_empty());
    assert!(state.calls.inference_contexts.is_empty());
}

#[test]
fn jsx_children_share_the_element_context_but_self_closing_attributes_do_not() {
    let (_counters, _generation, _identity, owner) = crate::tests::owner();
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    let number = state.builtins.number_type;
    for self_closing in [false, true] {
        let attributes = state.factory.new_jsx_attributes(None);
        let parent = if self_closing {
            state
                .factory
                .new_jsx_self_closing_element(None, None, Some(attributes))
        } else {
            state
                .factory
                .new_jsx_opening_element(None, None, Some(attributes))
        };
        let element = state.factory.new_jsx_element(Some(parent), None, None);
        state
            .factory
            .node_mut(attributes)
            .unwrap()
            .set_parent(Some(parent));
        state
            .factory
            .node_mut(parent)
            .unwrap()
            .set_parent(Some(element));
        let expected = if self_closing { attributes } else { element };
        assert_eq!(
            check_with_expression(
                state,
                ArgumentRequest {
                    node: attributes,
                    contextual: number,
                    inference: None,
                    mode: 0
                },
                |state, _, mode| {
                    assert_eq!(mode, 1);
                    assert_eq!(state.calls.contexts.last().unwrap().node, expected);
                    assert_eq!(
                        state.calls.inference_contexts.last(),
                        Some(&(expected, None))
                    );
                    Ok(number)
                },
            ),
            Ok(number)
        );
        assert!(state.calls.contexts.is_empty());
        assert!(state.calls.inference_contexts.is_empty());
    }
}

#[cfg(feature = "operation-experiments")]
fn completed_literal(counters: &tsr_arena::Counters) -> (tsr_ast::CompletedFile, NodeId) {
    let text = tsr_jsstring::SourceText::from_loaded_bytes(b"'value';".as_slice());
    let mut builder = tsr_ast::AstBuilder::new(text.clone(), counters);
    let literal = builder.new_string_literal(JsString::from_bytes(b"value".as_slice()), 0);
    let statement = builder.new_expression_statement(Some(literal));
    let nodes = builder.node_slice(vec![Some(statement)]).unwrap();
    let statements = builder
        .new_list(tsr_core::TextRange::new(0, 8), nodes)
        .unwrap();
    let source = builder.new_source_file(
        tsr_ast::SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/argument.ts".as_slice()),
            ..Default::default()
        },
        text,
        Some(statements),
        None,
    );
    builder
        .node_mut(literal)
        .unwrap()
        .set_parent(Some(statement));
    builder
        .node_mut(statement)
        .unwrap()
        .set_parent(Some(source));
    let completed = tsr_binder::bind_parsed_file(builder.complete(source).unwrap()).unwrap();
    (completed, literal)
}

#[test]
#[cfg(feature = "operation-experiments")]
fn retained_scope_checks_real_expression_and_restores_on_type_error() {
    let (counters, _generation, _identity, owner) = crate::tests::owner();
    let (completed, literal) = completed_literal(&counters);
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    let regular = state
        .get_string_literal_type(JsString::from_bytes(b"value".as_slice()))
        .unwrap();
    let request = ArgumentRequest {
        node: literal,
        contextual: regular,
        inference: None,
        mode: 0,
    };
    assert_eq!(
        check_in_scope(
            state,
            request,
            crate::program::ArgumentReadScope::new(completed.shared().unwrap()),
        ),
        Ok(regular)
    );
    assert!(state.argument_read_scope.is_none());
    let invalid = TypeId::next(0, state.types.len() + 100).unwrap();
    assert_eq!(
        check_in_scope(
            state,
            ArgumentRequest {
                contextual: invalid,
                ..request
            },
            crate::program::ArgumentReadScope::new(completed.shared().unwrap()),
        ),
        Err(Error::Arena(tsr_arena::Error::InvalidSlot))
    );
    assert!(state.argument_read_scope.is_none());
    assert!(state.calls.contexts.is_empty());
    assert!(state.calls.inference_contexts.is_empty());
}

#[test]
#[cfg(feature = "operation-experiments")]
fn retained_scope_preserves_checked_reads_and_factory_fallback() {
    let (counters, _generation, _identity, owner) = crate::tests::owner();
    let (completed, literal) = completed_literal(&counters);
    let (_foreign, foreign_literal) = completed_literal(&counters);
    let source = completed.source();
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    state.argument_read_scope = Some(crate::program::ArgumentReadScope::new(
        completed.shared().unwrap(),
    ));
    drop(completed);
    assert_eq!(state.node(literal).unwrap().kind(), K::StringLiteral);
    assert_eq!(state.node_text(literal).unwrap().as_bytes(), b"value");
    assert_eq!(
        state.source_file_read(source).unwrap().file_name(),
        b"/argument.ts"
    );
    assert_eq!(
        state.ast(literal).unwrap().node(literal).unwrap().kind(),
        K::StringLiteral
    );
    // A foreign core owner must reach the ordinary program route. This state
    // has no program, so accidentally sending it through the retained owner
    // would produce WrongOwner instead of the existing missing-program error.
    assert!(matches!(
        state.node(foreign_literal),
        Err(Error::Unsupported("query without a checker program"))
    ));
    let invalid = NodeId::from_parts(literal.arena(), u32::MAX).unwrap();
    assert!(matches!(
        state.node(invalid),
        Err(Error::Arena(tsr_arena::Error::InvalidSlot))
    ));
    let factory = state.factory.new_token(K::TrueKeyword.into());
    assert_eq!(state.node(factory).unwrap().kind(), K::TrueKeyword);
    let boolean = state.builtins.boolean_type;
    assert_eq!(
        check_scoped(
            state,
            ArgumentRequest {
                node: factory,
                contextual: boolean,
                inference: None,
                mode: 0,
            }
        ),
        Ok(state.builtins.regular_true_type),
    );
    assert!(state.argument_read_scope.is_some());
    assert_eq!(state.node(literal).unwrap().kind(), K::StringLiteral);
}
