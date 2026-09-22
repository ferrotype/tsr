use super::*;
use crate::{AstBuilder, FactoryMethods};
use std::collections::HashMap;
use tsr_arena::Counters;
use tsr_jsstring::SourceText;

struct Context<'a> {
    view: AstView<'a>,
    values: HashMap<NodeId, EvaluationResult>,
    calls: Vec<(NodeId, Option<NodeId>)>,
    fail: Option<NodeId>,
    initializers: HashMap<NodeId, NodeId>,
}
impl<'a> Context<'a> {
    fn new(view: AstView<'a>) -> Self {
        Self {
            view,
            values: HashMap::new(),
            calls: Vec::new(),
            fail: None,
            initializers: HashMap::new(),
        }
    }
}
impl EvaluationContext for Context<'_> {
    type Error = &'static str;
    fn ast(&self, _: NodeId) -> Result<AstView<'_>, Self::Error> {
        Ok(self.view)
    }
    fn evaluate_entity(
        &mut self,
        expression: NodeId,
        location: Option<NodeId>,
    ) -> Result<EvaluationResult, Self::Error> {
        self.calls.push((expression, location));
        if self.fail == Some(expression) {
            return Err("entity failure");
        }
        if let Some(initializer) = self.initializers.get(&expression).copied() {
            return Ok(Evaluator::new(self, 0)
                .evaluate(initializer, Some(expression))
                .unwrap());
        }
        Ok(self.values.get(&expression).cloned().unwrap_or_default())
    }
}
fn builder() -> AstBuilder {
    AstBuilder::new(SourceText::default(), &Counters::default())
}
fn number(value: f64) -> EvaluatedValue {
    EvaluatedValue::Number(Number::new(value))
}
fn binary(builder: &mut AstBuilder, left: NodeId, kind: K, right: NodeId) -> NodeId {
    let operator = builder.new_token(kind.into());
    builder.new_binary_expression(None, Some(left), None, Some(operator), Some(right))
}
fn template(builder: &mut AstBuilder, expressions: &[NodeId]) -> NodeId {
    let head = builder.new_template_head(JsString::from_bytes(&b"h"[..]), JsString::default(), 0);
    let spans = expressions
        .iter()
        .map(|&expression| {
            let tail =
                builder.new_template_tail(JsString::from_bytes(&b"!"[..]), JsString::default(), 0);
            Some(builder.new_template_span(Some(expression), Some(tail)))
        })
        .collect();
    let nodes = builder.node_slice(spans).unwrap();
    let list = builder
        .new_list(tsr_core::TextRange::new(-1, -1), nodes)
        .unwrap();
    builder.new_template_expression(Some(head), Some(list))
}

#[test]
fn direct_helpers_cover_unknown_foreign_values_and_structural_bigint_zero() {
    for value in [None, Some(EvaluatedValue::Unsupported)] {
        assert_eq!(
            any_to_string(value.as_ref()).unwrap_err().to_string(),
            "Unhandled case in AnyToString"
        );
        assert_eq!(
            is_truthy(value.as_ref()).unwrap_err().to_string(),
            "Unhandled case in IsTruthy"
        );
    }
    for value in [0.0, -0.0, f64::NAN] {
        assert!(!is_truthy(Some(number(value)).as_ref()).unwrap());
    }
    assert!(is_truthy(Some(number(f64::INFINITY)).as_ref()).unwrap());
    assert_eq!(
        any_to_string(Some(number(-0.0)).as_ref())
            .unwrap()
            .as_bytes(),
        b"0"
    );
    for (value, text, truthy) in [
        (EvaluatedValue::Bool(false), &b"false"[..], false),
        (EvaluatedValue::Bool(true), &b"true"[..], true),
        (
            EvaluatedValue::BigInt(PseudoBigInt::default()),
            &b"0"[..],
            false,
        ),
        (
            EvaluatedValue::BigInt(PseudoBigInt::new(b"0042", true)),
            &b"-42"[..],
            true,
        ),
    ] {
        assert_eq!(any_to_string(Some(&value)).unwrap().as_bytes(), text);
        assert_eq!(is_truthy(Some(&value)).unwrap(), truthy);
    }
    // PseudoBigInt fields are public upstream too: truthiness compares the
    // entire struct rather than assuming its constructor's canonical form.
    assert!(is_truthy(Some(&EvaluatedValue::BigInt(PseudoBigInt {
        negative: true,
        base10_value: Vec::new()
    })))
    .unwrap());
}

#[test]
fn byte_strings_survive_owner_drop_and_concatenation_does_not_join_surrogates() {
    let result = {
        let mut builder = builder();
        let left = builder.new_string_literal(JsString::from_bytes(&b"\xed\xa0\x80"[..]), 0);
        let right = builder.new_string_literal(JsString::from_bytes(&b"\xed\xb0\x80\xff"[..]), 0);
        let expression = binary(&mut builder, left, K::PlusToken, right);
        let mut context = Context::new(builder.view());
        let result = Evaluator::new(&mut context, 0)
            .evaluate(expression, None)
            .unwrap();
        assert!(result.is_syntactically_string);
        assert!(context.calls.is_empty());
        result
    };
    assert_eq!(
        any_to_string(result.value.as_ref()).unwrap().as_bytes(),
        b"\xed\xa0\x80\xed\xb0\x80\xff"
    );
    let retained = {
        let value = EvaluatedValue::String(JsString::from_bytes(&b"\xff\xed\xa0\x80"[..]));
        any_to_string(Some(&value)).unwrap()
    };
    assert_eq!(retained.as_bytes(), b"\xff\xed\xa0\x80");
}

#[test]
fn binary_evaluates_both_sides_in_order_and_combines_flags_for_unknown_values() {
    let mut builder = builder();
    let left = builder.new_identifier(JsString::from_bytes(&b"left"[..]));
    let right = builder.new_identifier(JsString::from_bytes(&b"right"[..]));
    let expression = binary(&mut builder, left, K::PlusToken, right);
    let mut context = Context::new(builder.view());
    context
        .values
        .insert(left, EvaluationResult::new(None, false, true, false));
    context.values.insert(
        right,
        EvaluationResult::new(Some(EvaluatedValue::Bool(true)), true, false, true),
    );
    let result = Evaluator::new(&mut context, 0)
        .evaluate(expression, Some(expression))
        .unwrap();
    assert_eq!(
        context.calls,
        [(left, Some(expression)), (right, Some(expression))]
    );
    assert_eq!(result, EvaluationResult::new(None, true, true, true));
}

#[test]
fn unary_preserves_negative_zero_and_reference_flags_but_clears_string_flag() {
    let mut builder = builder();
    let entity = builder.new_identifier(JsString::from_bytes(&b"zero"[..]));
    let expression = builder.new_prefix_unary_expression(K::MinusToken.into(), Some(entity));
    let mut context = Context::new(builder.view());
    context.values.insert(
        entity,
        EvaluationResult::new(Some(number(0.0)), true, true, true),
    );
    let result = Evaluator::new(&mut context, 0)
        .evaluate(expression, None)
        .unwrap();
    let Some(EvaluatedValue::Number(value)) = result.value else {
        panic!("expected number")
    };
    assert!(value.value().is_sign_negative());
    assert!(!result.is_syntactically_string);
    assert!(result.resolved_other_files && result.has_external_references);
}

#[test]
fn template_unknown_discards_flags_and_skips_remaining_callbacks() {
    let mut builder = builder();
    let names = [b"first", b"empty", b"later"]
        .map(|text| builder.new_identifier(JsString::from_bytes(text.as_slice())));
    let expression = template(&mut builder, &names);
    let mut context = Context::new(builder.view());
    context.values.insert(
        names[0],
        EvaluationResult::new(Some(number(2.0)), false, true, true),
    );
    context
        .values
        .insert(names[1], EvaluationResult::new(None, false, true, true));
    let result = Evaluator::new(&mut context, 0)
        .evaluate(expression, Some(expression))
        .unwrap();
    assert_eq!(result, EvaluationResult::new(None, true, false, false));
    assert_eq!(
        context.calls,
        [(names[0], Some(expression)), (names[1], Some(expression))]
    );
}

#[test]
fn template_converts_boolean_bigint_and_raw_bytes_then_reports_foreign_value() {
    let mut builder = builder();
    let names = [b"a", b"b", b"c"]
        .map(|text| builder.new_identifier(JsString::from_bytes(text.as_slice())));
    let expression = template(&mut builder, &names);
    let mut context = Context::new(builder.view());
    context.values.insert(
        names[0],
        EvaluationResult::new(Some(EvaluatedValue::Bool(true)), false, true, false),
    );
    context.values.insert(
        names[1],
        EvaluationResult::new(
            Some(EvaluatedValue::BigInt(PseudoBigInt::new(b"42", true))),
            false,
            false,
            true,
        ),
    );
    context.values.insert(
        names[2],
        EvaluationResult::new(
            Some(EvaluatedValue::String(JsString::from_bytes(
                &b"\xff\xed\xa0\x80"[..],
            ))),
            false,
            false,
            false,
        ),
    );
    let result = Evaluator::new(&mut context, 0)
        .evaluate(expression, None)
        .unwrap();
    assert_eq!(
        any_to_string(result.value.as_ref()).unwrap().as_bytes(),
        b"htrue!-42!\xff\xed\xa0\x80!"
    );
    assert!(
        result.is_syntactically_string
            && result.resolved_other_files
            && result.has_external_references
    );
    context.calls.clear();
    context.values.get_mut(&names[1]).unwrap().value = Some(EvaluatedValue::Unsupported);
    assert_eq!(
        Evaluator::new(&mut context, 0).evaluate(expression, None),
        Err(Error::Unhandled(Unhandled::AnyToString))
    );
    assert_eq!(context.calls, [(names[0], None), (names[1], None)]);
}

#[test]
fn failures_keep_their_owner_and_a_retry_has_no_partial_evaluator_state() {
    let mut builder = builder();
    let left = builder.new_identifier(JsString::from_bytes(&b"left"[..]));
    let right = builder.new_numeric_literal(JsString::from_bytes(&b"1"[..]), 0);
    let expression = binary(&mut builder, left, K::PlusToken, right);
    let mut other = self::builder();
    let foreign = other.new_identifier(JsString::from_bytes(&b"foreign"[..]));
    let mut context = Context::new(builder.view());
    context.fail = Some(left);
    assert_eq!(
        Evaluator::new(&mut context, 0).evaluate(expression, None),
        Err(Error::Context("entity failure"))
    );
    assert_eq!(
        Evaluator::new(&mut context, 0).evaluate(foreign, None),
        Err(Error::Storage(tsr_arena::Error::WrongOwner))
    );
    context.fail = None;
    context.values.insert(
        left,
        EvaluationResult::new(Some(number(2.0)), false, false, false),
    );
    assert_eq!(
        Evaluator::new(&mut context, 0)
            .evaluate(expression, None)
            .unwrap()
            .value,
        Some(number(3.0))
    );
}

#[test]
fn outer_masks_skip_assertions_assignment_and_comma_with_original_location() {
    use outer_expression_kinds as o;
    let mut builder = builder();
    let entity = builder.new_identifier(JsString::from_bytes(&b"entity"[..]));
    let assertion = builder.new_as_expression(Some(entity), None);
    let parentheses = builder.new_parenthesized_expression(Some(assertion));
    let satisfies = builder.new_satisfies_expression(Some(entity), None);
    let ignored = builder.new_identifier(JsString::from_bytes(&b"ignored"[..]));
    let assignment = binary(&mut builder, ignored, K::EqualsToken, entity);
    let comma = binary(&mut builder, ignored, K::CommaToken, assignment);
    let mut context = Context::new(builder.view());
    context.values.insert(
        entity,
        EvaluationResult::new(Some(number(7.0)), false, false, false),
    );
    assert_eq!(
        Evaluator::new(&mut context, 0)
            .evaluate(parentheses, None)
            .unwrap(),
        EvaluationResult::default()
    );
    assert!(context.calls.is_empty());
    assert_eq!(
        Evaluator::new(&mut context, o::TYPE_ASSERTIONS)
            .evaluate(parentheses, Some(parentheses))
            .unwrap()
            .value,
        Some(number(7.0))
    );
    // Pinned upstream also skips satisfies under EXPRESSIONS_WITH_TYPE_ARGUMENTS.
    assert_eq!(
        Evaluator::new(&mut context, o::EXPRESSIONS_WITH_TYPE_ARGUMENTS)
            .evaluate(satisfies, Some(satisfies))
            .unwrap()
            .value,
        Some(number(7.0))
    );
    assert_eq!(
        Evaluator::new(&mut context, o::EXPRESSION_TYPE_PASSTHROUGH)
            .evaluate(comma, Some(comma))
            .unwrap()
            .value,
        Some(number(7.0))
    );
    assert_eq!(
        context.calls,
        [
            (entity, Some(parentheses)),
            (entity, Some(satisfies)),
            (entity, Some(comma))
        ]
    );
}

#[test]
fn excluded_jsdoc_assertion_parentheses_remain_opaque_even_with_all_skips() {
    use outer_expression_kinds as o;
    let mut builder = builder();
    let entity = builder.new_identifier(JsString::from_bytes(&b"entity"[..]));
    let ty = builder.new_token(K::StringKeyword.into());
    builder
        .node_mut(ty)
        .unwrap()
        .set_flags(crate::node_flags::REPARSED);
    let assertion = builder.new_as_expression(Some(entity), Some(ty));
    let parentheses = builder.new_parenthesized_expression(Some(assertion));
    builder
        .node_mut(parentheses)
        .unwrap()
        .set_flags(crate::node_flags::JAVA_SCRIPT_FILE);
    let mut context = Context::new(builder.view());
    let result = Evaluator::new(&mut context, o::ALL | o::EXCLUDE_JSDOC_TYPE_ASSERTION)
        .evaluate(parentheses, None)
        .unwrap();
    assert_eq!(result, EvaluationResult::default());
    assert!(context.calls.is_empty());
    Evaluator::new(&mut context, o::ALL)
        .evaluate(parentheses, None)
        .unwrap();
    assert_eq!(context.calls, [(entity, None)]);
}

#[test]
fn deep_expression_and_reentrant_callback_chains_grow_the_stack() {
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            let mut builder = builder();
            let zero = builder.new_numeric_literal(JsString::from_bytes(&b"0"[..]), 0);
            let mut expression = zero;
            for _ in 0..4_000 {
                expression = binary(&mut builder, expression, K::PlusToken, zero);
            }
            let mut context = Context::new(builder.view());
            assert_eq!(
                Evaluator::new(&mut context, 0)
                    .evaluate(expression, None)
                    .unwrap()
                    .value,
                Some(number(0.0))
            );
            drop(context);
            let mut initializers = HashMap::new();
            expression = zero;
            for _ in 0..1_000 {
                let entity = builder.new_identifier(JsString::from_bytes(&b"constant"[..]));
                initializers.insert(entity, expression);
                expression = entity;
            }
            let mut context = Context::new(builder.view());
            context.initializers = initializers;
            assert_eq!(
                Evaluator::new(&mut context, 0)
                    .evaluate(expression, None)
                    .unwrap()
                    .value,
                Some(number(0.0))
            );
            assert_eq!(context.calls.len(), 1_000);
        })
        .unwrap()
        .join()
        .unwrap();
}
