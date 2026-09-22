use tsr_ast::{
    evaluator::outer_expression_kinds as o, utilities as u, AstBuilder, FactoryMethods,
    SyntaxKind as K,
};
use tsr_jsstring::SourceText;

#[test]
fn outer_masks_select_the_pinned_operand_and_stop_at_other_kinds() {
    let mut f = AstBuilder::new(SourceText::default(), &tsr_arena::Counters::new());
    let leaf = f.new_identifier(tsr_ast::JsString::from_bytes(b"x".as_slice()));
    let ty = f.new_token(K::NumberKeyword.into());
    let paren = f.new_parenthesized_expression(Some(leaf));
    let as_expr = f.new_as_expression(Some(leaf), Some(ty));
    let assertion = f.new_type_assertion(Some(ty), Some(leaf));
    let non_null = f.new_non_null_expression(Some(leaf), 0);
    let partial = f.new_partially_emitted_expression(Some(leaf));
    let satisfies = f.new_satisfies_expression(Some(leaf), Some(ty));
    let with_types = f.new_expression_with_type_arguments(Some(leaf), None);
    let left = f.new_identifier(tsr_ast::JsString::from_bytes(b"left".as_slice()));
    let equal = f.new_token(K::EqualsToken.into());
    let comma = f.new_token(K::CommaToken.into());
    let plus = f.new_token(K::PlusToken.into());
    let assignment = f.new_binary_expression(None, Some(left), None, Some(equal), Some(leaf));
    let sequence = f.new_binary_expression(None, Some(left), None, Some(comma), Some(leaf));
    let addition = f.new_binary_expression(None, Some(left), None, Some(plus), Some(leaf));
    for (node, masks) in [
        (paren, o::PARENTHESES),
        (as_expr, o::TYPE_ASSERTIONS),
        (assertion, o::TYPE_ASSERTIONS),
        (non_null, o::NON_NULL_ASSERTIONS),
        (partial, o::PARTIALLY_EMITTED_EXPRESSIONS),
        (satisfies, o::SATISFIES | o::EXPRESSIONS_WITH_TYPE_ARGUMENTS),
        (with_types, o::EXPRESSIONS_WITH_TYPE_ARGUMENTS),
        (assignment, o::ASSIGNMENTS),
        (sequence, o::COMMA),
    ] {
        for bit in 0..9 {
            let mask = 1 << bit;
            let expected = masks & mask != 0;
            assert_eq!(
                u::is_outer_expression(f.view(), node, mask).unwrap(),
                expected
            );
            assert_eq!(
                u::skip_outer_expressions(f.view(), node, mask).unwrap(),
                if expected { leaf } else { node }
            );
        }
    }
    assert_eq!(
        u::skip_outer_expressions(f.view(), addition, u16::MAX).unwrap(),
        addition
    );
    assert!(!u::is_jsdoc_type_assertion(f.view(), None).unwrap());
}

#[test]
fn jsdoc_assertion_exclusion_requires_js_parentheses_as_and_reparsed_type() {
    let mut f = AstBuilder::new(SourceText::default(), &tsr_arena::Counters::new());
    let leaf = f.new_identifier(tsr_ast::JsString::from_bytes(b"x".as_slice()));
    let ty = f.new_token(K::NumberKeyword.into());
    let cast = f.new_as_expression(Some(leaf), Some(ty));
    let outer = f.new_parenthesized_expression(Some(cast));
    let mask = o::PARENTHESES | o::EXCLUDE_JSDOC_TYPE_ASSERTION;
    assert!(!u::is_jsdoc_type_assertion(f.view(), Some(outer)).unwrap());
    f.node_mut(ty)
        .unwrap()
        .set_flags(tsr_ast::node_flags::REPARSED);
    assert!(!u::is_jsdoc_type_assertion(f.view(), Some(outer)).unwrap());
    f.node_mut(outer)
        .unwrap()
        .set_flags(tsr_ast::node_flags::JAVA_SCRIPT_FILE);
    assert!(u::is_jsdoc_type_assertion(f.view(), Some(outer)).unwrap());
    assert_eq!(
        u::skip_outer_expressions(f.view(), outer, mask).unwrap(),
        outer
    );
    assert_eq!(
        u::skip_outer_expressions(f.view(), outer, o::PARENTHESES).unwrap(),
        cast
    );
    assert!(!u::is_jsdoc_type_assertion(f.view(), Some(cast)).unwrap());
}
