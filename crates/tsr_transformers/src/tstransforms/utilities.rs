//! `transformers/tstransforms/utilities.go`: the literal expression of a
//! constant enum member value.
use tsr_ast::{token_flags, Factory, FactoryMethods, JsString, NodeId, SyntaxKind as K};
use tsr_printer::emit_resolver::ConstantValue;

/// The expression of a constant: a string literal, a numeric literal (negated
/// with a prefix minus below zero), `Infinity`, `-Infinity` or `NaN`.
/// Upstream takes `any` and answers nil for any other dynamic type; the
/// resolver's constant has no other case, so there is no nil result.
// port: tsc/internal/transformers/tstransforms/utilities.go:constantExpression
pub(crate) fn constant_expression(value: &ConstantValue, factory: &mut dyn Factory) -> NodeId {
    match value {
        ConstantValue::String(value) => {
            factory.new_string_literal(value.clone(), token_flags::NONE)
        }
        ConstantValue::Number(value) => {
            if value.is_inf() {
                if value.value() > 0.0 {
                    return factory.new_identifier(JsString::from_bytes(&b"Infinity"[..]));
                }
                let infinity = factory.new_identifier(JsString::from_bytes(&b"Infinity"[..]));
                return factory.new_prefix_unary_expression(K::MinusToken.into(), Some(infinity));
            }
            if value.is_nan() {
                return factory.new_identifier(JsString::from_bytes(&b"NaN"[..]));
            }
            if value.value() < 0.0 {
                let operand = constant_expression(
                    &ConstantValue::Number(tsr_jsnum::Number::new(-value.value())),
                    factory,
                );
                return factory.new_prefix_unary_expression(K::MinusToken.into(), Some(operand));
            }
            factory.new_numeric_literal(
                JsString::from_bytes(value.to_string().into_bytes()),
                token_flags::NONE,
            )
        }
    }
}
