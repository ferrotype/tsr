use super::*;
use tsr_ast::{AstBuilder, FactoryMethods, SourceFileParseOptions};
use tsr_jsstring::SourceText;

fn targets(factory: &mut impl FactoryMethods) -> Vec<NodeId> {
    let this = factory.new_keyword_expression(K::ThisKeyword.into());
    let other = factory.new_keyword_expression(K::SuperKeyword.into());
    let name = factory.new_identifier(JsString::from_bytes(b"p".as_slice()));
    let property = factory.new_property_access_expression(Some(this), None, Some(name), 0);
    let literal = factory.new_string_literal(JsString::from_bytes(b"p".as_slice()), 0);
    let element = factory.new_element_access_expression(Some(this), None, Some(literal), 0);
    let wrapper = factory.new_parenthesized_expression(Some(property));
    let non_null = factory.new_non_null_expression(Some(element), 0);
    let equals = factory.new_token(K::EqualsToken.into());
    let assign =
        factory.new_binary_expression(None, Some(property), None, Some(equals), Some(other));
    let plus_equals = factory.new_token(K::PlusEqualsToken.into());
    let compound =
        factory.new_binary_expression(None, Some(element), None, Some(plus_equals), Some(other));
    let comma = factory.new_token(K::CommaToken.into());
    let comma_expression =
        factory.new_binary_expression(None, Some(other), None, Some(comma), Some(property));
    let plus = factory.new_token(K::PlusToken.into());
    let arithmetic =
        factory.new_binary_expression(None, Some(property), None, Some(plus), Some(this));
    // Neither a non-assignment operator nor a mismatching property name may
    // recurse into a malformed operand.
    let malformed = factory.new_parenthesized_expression(None);
    let ignored =
        factory.new_binary_expression(None, Some(malformed), None, Some(plus), Some(this));
    let other_name = factory.new_identifier(JsString::from_bytes(b"q".as_slice()));
    let mismatch =
        factory.new_property_access_expression(Some(malformed), None, Some(other_name), 0);
    // Missing-link order is observable: base precedes name, left precedes
    // right, and right precedes operator even for comma/assignment peels.
    let missing_base = factory.new_property_access_expression(None, None, None, 0);
    let missing_left = factory.new_binary_expression(None, None, None, None, None);
    let missing_right = factory.new_binary_expression(None, Some(this), None, None, None);
    let missing_operator = factory.new_binary_expression(None, Some(this), None, None, Some(other));
    vec![
        this,
        other,
        property,
        element,
        wrapper,
        non_null,
        assign,
        compound,
        comma_expression,
        arithmetic,
        ignored,
        mismatch,
        malformed,
        missing_base,
        missing_left,
        missing_right,
        missing_operator,
    ]
}

fn retained_targets(counters: &tsr_arena::Counters) -> (tsr_ast::SharedBoundFile, Vec<NodeId>) {
    let text = SourceText::from_bytes(b"".as_slice());
    let mut builder = AstBuilder::new(text.clone(), counters);
    let source = builder.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/flow-target.ts".as_slice()),
            ..SourceFileParseOptions::default()
        },
        text,
        None,
        None,
    );
    let targets = targets(&mut builder);
    let completed = builder
        .complete(source)
        .unwrap()
        .bind_and_publish(|_| Ok(()))
        .unwrap();
    (completed.shared().unwrap(), targets)
}

fn result(result: Result<bool, Error>) -> Result<bool, String> {
    result.map_err(|error| format!("{error:?}"))
}

#[test]
fn scoped_and_fallback_targets_follow_the_reference_matcher() {
    let (counters, _generation, _identity, owner) = crate::tests::owner();
    let (shared, retained) = retained_targets(&counters);
    let scope = Some(TargetScope::new(&shared));
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    let fallback = targets(&mut state.factory);
    // Build each source shape once, then compare identical syntax in two
    // owners against the unchanged general matcher as an independent oracle.
    for source in [fallback[0], fallback[2]] {
        let shape = state.reference_shape(source).unwrap();
        for (&fallback, &retained) in fallback.iter().zip(&retained) {
            let expected = result(state.matching_reference(source, fallback));
            assert_eq!(
                result(state.matching_shape(&shape, fallback, None)),
                expected
            );
            assert_eq!(
                result(state.matching_shape(&shape, fallback, scope)),
                expected
            );
            assert_eq!(
                result(state.matching_shape(&shape, retained, scope)),
                expected
            );
            let expected = result(state.contains_flow_reference(source, fallback));
            assert_eq!(
                result(state.shape_contains(&shape, fallback, None)),
                expected
            );
            assert_eq!(
                result(state.shape_contains(&shape, retained, scope)),
                expected
            );
        }
    }
}

#[test]
fn target_scope_validates_slots_and_falls_back_for_foreign_owners() {
    let (counters, _generation, _identity, owner) = crate::tests::owner();
    let (shared, retained) = retained_targets(&counters);
    let scope = Some(TargetScope::new(&shared));
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    let shape = ReferenceShape {
        accesses: Vec::new(),
        root: ReferenceRoot::This,
        contains_candidates: 0,
    };
    let invalid = NodeId::from_parts(retained[0].arena(), u32::MAX).unwrap();
    assert!(matches!(
        state.matching_shape(&shape, invalid, scope),
        Err(Error::Arena(tsr_arena::Error::InvalidSlot))
    ));
    let (_foreign, foreign) = retained_targets(&counters);
    assert_eq!(
        result(state.matching_shape(&shape, foreign[0], scope)),
        result(state.matching_shape(&shape, foreign[0], None))
    );
    let mutable = state.factory.new_keyword_expression(K::ThisKeyword.into());
    let read = MatchingTarget::new(scope, mutable).unwrap();
    assert!(read.read.is_none());
    assert!(state.matching_shape(&shape, mutable, scope).unwrap());
    *state.factory.node_mut(mutable).unwrap() = tsr_ast::Node::from_factory_parts(
        K::SuperKeyword.into(),
        tsr_ast::NodeData::KeywordExpression(tsr_ast::KeywordExpressionData {}),
    );
    assert_eq!(
        read.with_read(state, |node| Ok(node.kind())).unwrap(),
        K::SuperKeyword
    );
    assert!(!state.matching_shape(&shape, mutable, scope).unwrap());
}

#[test]
fn target_scope_observes_published_core_overlays() {
    let (counters, _generation, _identity, owner) = crate::tests::owner();
    let text = SourceText::from_bytes(b"".as_slice());
    let mut builder = AstBuilder::new(text.clone(), &counters);
    let source = builder.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/flow-target.ts".as_slice()),
            ..SourceFileParseOptions::default()
        },
        text,
        None,
        None,
    );
    let target = builder.new_keyword_expression(K::ThisKeyword.into());
    let parsed = builder.complete(source).unwrap().publish_unbound();
    parsed
        .bind_with(source, |binding| {
            *binding.node_mut(target)? = tsr_ast::Node::from_factory_parts(
                K::SuperKeyword.into(),
                tsr_ast::NodeData::KeywordExpression(tsr_ast::KeywordExpressionData {}),
            );
            Ok(())
        })
        .unwrap();
    let bound = parsed.retain_bound(source).unwrap();
    let shared = bound.shared().unwrap();
    let scope = Some(TargetScope::new(&shared));
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    let shape = ReferenceShape {
        accesses: Vec::new(),
        root: ReferenceRoot::Super,
        contains_candidates: 0,
    };
    assert!(state.matching_shape(&shape, target, scope).unwrap());
}
