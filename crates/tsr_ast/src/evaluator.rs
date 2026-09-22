//! Constant expression evaluation with caller-owned entity resolution.
//!
//! AST reads borrow the caller's storage; returned strings retain their bytes.
//! The evaluator keeps no cache or callback state of its own. Callbacks run left
//! to right and can reenter evaluation (as the checker does for constants).
//! Recursive expression and callback chains use the production growing stack.

use crate::{AstView, NodeId, NodeKind, SyntaxKind as K};
use tsr_jsnum::{Number, PseudoBigInt};
use tsr_jsstring::JsString;

/// The values understood by the pinned Go evaluator. `None` represents its nil
/// (unknown) value. `Unsupported` represents a non-nil value outside this domain.
#[derive(Clone, Debug, PartialEq)]
pub enum EvaluatedValue {
    Number(Number),
    String(JsString),
    Bool(bool),
    BigInt(PseudoBigInt),
    Unsupported,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EvaluationResult {
    pub value: Option<EvaluatedValue>,
    pub is_syntactically_string: bool,
    pub resolved_other_files: bool,
    pub has_external_references: bool,
}

impl EvaluationResult {
    // port: tsc/internal/evaluator/evaluator.go:NewResult
    pub fn new(
        value: Option<EvaluatedValue>,
        is_syntactically_string: bool,
        resolved_other_files: bool,
        has_external_references: bool,
    ) -> Self {
        Self {
            value,
            is_syntactically_string,
            resolved_other_files,
            has_external_references,
        }
    }
}

/// An upstream contract panic, kept separate from invalid storage and callback
/// failures so callers can preserve their own error handling and diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unhandled {
    AnyToString,
    IsTruthy,
}
impl std::fmt::Display for Unhandled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::AnyToString => "Unhandled case in AnyToString",
            Self::IsTruthy => "Unhandled case in IsTruthy",
        })
    }
}
impl std::error::Error for Unhandled {}

/// Borrowed primitive view used by the checker and evaluator. Converting an
/// existing literal does not copy its string or bigint backing.
#[derive(Clone, Copy)]
pub enum PrimitiveValue<'a> {
    String(&'a JsString),
    Number(Number),
    Bool(bool),
    BigInt(&'a PseudoBigInt),
}
impl<'a> PrimitiveValue<'a> {
    // port: tsc/internal/evaluator/evaluator.go:AnyToString
    pub fn text_bytes(self) -> std::borrow::Cow<'a, [u8]> {
        use std::borrow::Cow;
        match self {
            Self::String(text) => Cow::Borrowed(text.as_bytes()),
            Self::Number(number) => Cow::Owned(number.to_string().into_bytes()),
            Self::Bool(value) => Cow::Borrowed(if value { &b"true"[..] } else { &b"false"[..] }),
            Self::BigInt(value) => Cow::Owned(value.to_text()),
        }
    }

    pub fn to_text(self) -> JsString {
        match self {
            Self::String(text) => text.clone(),
            value => match value.text_bytes() {
                std::borrow::Cow::Borrowed(bytes) => JsString::from_bytes(bytes),
                std::borrow::Cow::Owned(bytes) => JsString::from_bytes(bytes),
            },
        }
    }

    // port: tsc/internal/evaluator/evaluator.go:IsTruthy
    pub fn is_truthy(self) -> bool {
        match self {
            Self::String(text) => !text.is_empty(),
            Self::Number(number) => number.value() != 0.0 && !number.is_nan(),
            Self::Bool(value) => value,
            // Go compares the struct, including its sign, with the zero value.
            Self::BigInt(value) => *value != PseudoBigInt::default(),
        }
    }
}
impl EvaluatedValue {
    fn primitive(&self) -> Option<PrimitiveValue<'_>> {
        Some(match self {
            Self::String(value) => PrimitiveValue::String(value),
            Self::Number(value) => PrimitiveValue::Number(*value),
            Self::Bool(value) => PrimitiveValue::Bool(*value),
            Self::BigInt(value) => PrimitiveValue::BigInt(value),
            Self::Unsupported => return None,
        })
    }
}

pub fn any_to_string(value: Option<&EvaluatedValue>) -> Result<JsString, Unhandled> {
    value
        .and_then(EvaluatedValue::primitive)
        .map(PrimitiveValue::to_text)
        .ok_or(Unhandled::AnyToString)
}

pub fn is_truthy(value: Option<&EvaluatedValue>) -> Result<bool, Unhandled> {
    value
        .and_then(EvaluatedValue::primitive)
        .map(PrimitiveValue::is_truthy)
        .ok_or(Unhandled::IsTruthy)
}

pub type OuterExpressionKinds = u16;
/// Pinned `ast.OuterExpressionKinds` bits. Parentheses are always added by the
/// evaluator; `EXCLUDE_JSDOC_TYPE_ASSERTION` still excludes assertion parentheses.
pub mod outer_expression_kinds {
    use super::OuterExpressionKinds;
    pub const PARENTHESES: OuterExpressionKinds = 1 << 0;
    pub const TYPE_ASSERTIONS: OuterExpressionKinds = 1 << 1;
    pub const NON_NULL_ASSERTIONS: OuterExpressionKinds = 1 << 2;
    pub const PARTIALLY_EMITTED_EXPRESSIONS: OuterExpressionKinds = 1 << 3;
    pub const EXPRESSIONS_WITH_TYPE_ARGUMENTS: OuterExpressionKinds = 1 << 4;
    pub const SATISFIES: OuterExpressionKinds = 1 << 5;
    pub const EXCLUDE_JSDOC_TYPE_ASSERTION: OuterExpressionKinds = 1 << 6;
    pub const ASSIGNMENTS: OuterExpressionKinds = 1 << 7;
    pub const COMMA: OuterExpressionKinds = 1 << 8;
    pub const ASSERTIONS: OuterExpressionKinds = TYPE_ASSERTIONS | NON_NULL_ASSERTIONS | SATISFIES;
    pub const ALL: OuterExpressionKinds =
        PARENTHESES | ASSERTIONS | PARTIALLY_EMITTED_EXPRESSIONS | EXPRESSIONS_WITH_TYPE_ARGUMENTS;
    pub const ALL_EXCEPT_ASSERTIONS_OR_EXPRESSIONS_WITH_TYPE_ARGUMENTS: OuterExpressionKinds =
        ALL & !ASSERTIONS & !EXPRESSIONS_WITH_TYPE_ARGUMENTS;
    pub const EXPRESSION_TYPE_PASSTHROUGH: OuterExpressionKinds = PARENTHESES | ASSIGNMENTS | COMMA;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error<E> {
    Storage(tsr_arena::Error),
    MissingLink(&'static str),
    Context(E),
    Unhandled(Unhandled),
}
impl<E> From<tsr_arena::Error> for Error<E> {
    fn from(value: tsr_arena::Error) -> Self {
        Self::Storage(value)
    }
}
impl<E> From<Unhandled> for Error<E> {
    fn from(value: Unhandled) -> Self {
        Self::Unhandled(value)
    }
}

/// Combines borrowed AST access with the mutable entity callback. A callback
/// may select another source file and recursively evaluate an initializer.
/// `location` is forwarded unchanged, including `None` for checker queries.
pub trait EvaluationContext {
    type Error;
    fn ast(&self, node: NodeId) -> Result<AstView<'_>, Self::Error>;
    fn evaluate_entity(
        &mut self,
        expression: NodeId,
        location: Option<NodeId>,
    ) -> Result<EvaluationResult, Self::Error>;
}

pub struct Evaluator<'a, C: EvaluationContext + ?Sized> {
    context: &'a mut C,
    outer_expressions_to_skip: OuterExpressionKinds,
}
impl<'a, C: EvaluationContext + ?Sized> Evaluator<'a, C> {
    // port: tsc/internal/evaluator/evaluator.go:NewEvaluator
    pub fn new(context: &'a mut C, outer_expressions_to_skip: OuterExpressionKinds) -> Self {
        Self {
            context,
            outer_expressions_to_skip: outer_expressions_to_skip
                | outer_expression_kinds::PARENTHESES,
        }
    }

    pub fn evaluate(
        &mut self,
        expression: NodeId,
        location: Option<NodeId>,
    ) -> Result<EvaluationResult, Error<C::Error>> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
            self.evaluate_worker(expression, location)
        })
    }

    fn ast(&self, node: NodeId) -> Result<AstView<'_>, Error<C::Error>> {
        self.context.ast(node).map_err(Error::Context)
    }

    fn evaluate_worker(
        &mut self,
        expression: NodeId,
        location: Option<NodeId>,
    ) -> Result<EvaluationResult, Error<C::Error>> {
        let expression = crate::utilities::skip_outer_expressions(
            self.ast(expression)?,
            expression,
            self.outer_expressions_to_skip,
        )?;
        let read = self.ast(expression)?.node(expression)?;
        match read.kind().known() {
            Some(K::PrefixUnaryExpression) => {
                let data = read
                    .data_source()
                    .as_prefix_unary_expression()
                    .ok_or(Error::MissingLink("unary expression"))?;
                let operator = data.operator();
                let operand = data.operand().ok_or(Error::MissingLink("unary operand"))?;
                let operand = self.evaluate(operand, location)?;
                let value = match operand.value {
                    Some(EvaluatedValue::Number(number)) => match operator.known() {
                        Some(K::PlusToken) => Some(EvaluatedValue::Number(number)),
                        Some(K::MinusToken) => {
                            Some(EvaluatedValue::Number(Number::new(-number.value())))
                        }
                        Some(K::TildeToken) => Some(EvaluatedValue::Number(number.bitwise_not())),
                        _ => None,
                    },
                    _ => None,
                };
                Ok(EvaluationResult::new(
                    value,
                    false,
                    operand.resolved_other_files,
                    operand.has_external_references,
                ))
            }
            Some(K::BinaryExpression) => {
                let data = read
                    .data_source()
                    .as_binary_expression()
                    .ok_or(Error::MissingLink("binary expression"))?;
                let left = data.left().ok_or(Error::MissingLink("binary left"))?;
                let right = data.right().ok_or(Error::MissingLink("binary right"))?;
                let operator = data
                    .operator_token()
                    .ok_or(Error::MissingLink("binary operator"))?;
                let left = self.evaluate(left, location)?;
                let right = self.evaluate(right, location)?;
                let operator = self.ast(operator)?.node(operator)?.kind();
                let value = binary_value(operator, left.value.as_ref(), right.value.as_ref());
                Ok(EvaluationResult::new(
                    value,
                    (left.is_syntactically_string || right.is_syntactically_string)
                        && operator == K::PlusToken,
                    left.resolved_other_files || right.resolved_other_files,
                    left.has_external_references || right.has_external_references,
                ))
            }
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral) => Ok(EvaluationResult::new(
                Some(EvaluatedValue::String(
                    self.ast(expression)?
                        .node_text(expression)?
                        .into_js_string(),
                )),
                true,
                false,
                false,
            )),
            Some(K::NumericLiteral) => Ok(EvaluationResult::new(
                Some(EvaluatedValue::Number(tsr_jsnum::from_string(
                    self.ast(expression)?.node_text(expression)?.as_bytes(),
                ))),
                false,
                false,
                false,
            )),
            Some(K::TemplateExpression) => self.evaluate_template(expression, location),
            Some(K::Identifier) => self
                .context
                .evaluate_entity(expression, location)
                .map_err(Error::Context),
            Some(K::ElementAccessExpression | K::PropertyAccessExpression) => {
                let root = read
                    .expression()
                    .ok_or(Error::MissingLink("access expression"))?;
                if crate::is_entity_name_expression(self.ast(root)?, root)? {
                    self.context
                        .evaluate_entity(expression, location)
                        .map_err(Error::Context)
                } else {
                    Ok(EvaluationResult::default())
                }
            }
            _ => Ok(EvaluationResult::default()),
        }
    }

    // port: tsc/internal/evaluator/evaluator.go:evaluateTemplateExpression
    fn evaluate_template(
        &mut self,
        expression: NodeId,
        location: Option<NodeId>,
    ) -> Result<EvaluationResult, Error<C::Error>> {
        let view = self.ast(expression)?;
        let read = view.node(expression)?;
        let data = read
            .data_source()
            .as_template_expression()
            .ok_or(Error::MissingLink("template expression"))?;
        let head = data.head().ok_or(Error::MissingLink("template head"))?;
        let spans = data
            .template_spans()
            .ok_or(Error::MissingLink("template spans"))?;
        let mut text = view.node_text(head)?.as_bytes().to_vec();
        let count = view.node_slice(view.list(spans)?.nodes())?.len();
        let mut result = EvaluationResult {
            is_syntactically_string: true,
            ..Default::default()
        };
        for index in 0..count {
            // Resolve one span at a time; do not clone the whole list just to
            // release the immutable context borrow before the callback.
            let view = self.ast(expression)?;
            let span = view
                .node_slice(view.list(spans)?.nodes())?
                .get(index)
                .flatten()
                .ok_or(Error::MissingLink("template span"))?;
            let read = view.node(span)?;
            let data = read
                .data_source()
                .as_template_span()
                .ok_or(Error::MissingLink("template span"))?;
            let expression = data
                .expression()
                .ok_or(Error::MissingLink("template span expression"))?;
            let literal = data
                .literal()
                .ok_or(Error::MissingLink("template span literal"))?;
            let value = self.evaluate(expression, location)?;
            if value.value.is_none() {
                // Upstream discards accumulated flags and skips later callbacks.
                return Ok(EvaluationResult {
                    is_syntactically_string: true,
                    ..Default::default()
                });
            }
            text.extend_from_slice(any_to_string(value.value.as_ref())?.as_bytes());
            text.extend_from_slice(self.ast(literal)?.node_text(literal)?.as_bytes());
            result.resolved_other_files |= value.resolved_other_files;
            result.has_external_references |= value.has_external_references;
        }
        result.value = Some(EvaluatedValue::String(JsString::from_bytes(text)));
        Ok(result)
    }
}

fn binary_value(
    operator: NodeKind,
    left: Option<&EvaluatedValue>,
    right: Option<&EvaluatedValue>,
) -> Option<EvaluatedValue> {
    if let (Some(EvaluatedValue::Number(left)), Some(EvaluatedValue::Number(right))) = (left, right)
    {
        let value = match operator.known() {
            Some(K::BarToken) => Some(left.bitwise_or(*right)),
            Some(K::AmpersandToken) => Some(left.bitwise_and(*right)),
            Some(K::CaretToken) => Some(left.bitwise_xor(*right)),
            Some(K::GreaterThanGreaterThanToken) => Some(left.signed_right_shift(*right)),
            Some(K::GreaterThanGreaterThanGreaterThanToken) => {
                Some(left.unsigned_right_shift(*right))
            }
            Some(K::LessThanLessThanToken) => Some(left.left_shift(*right)),
            Some(K::AsteriskToken) => Some(Number::new(left.value() * right.value())),
            Some(K::SlashToken) => Some(Number::new(left.value() / right.value())),
            Some(K::PlusToken) => Some(Number::new(left.value() + right.value())),
            Some(K::MinusToken) => Some(Number::new(left.value() - right.value())),
            Some(K::PercentToken) => Some(left.remainder(*right)),
            Some(K::AsteriskAsteriskToken) => Some(left.exponentiate(*right)),
            _ => None,
        };
        return value.map(EvaluatedValue::Number);
    }
    if operator == K::PlusToken
        && matches!(
            left,
            Some(EvaluatedValue::String(_) | EvaluatedValue::Number(_))
        )
        && matches!(
            right,
            Some(EvaluatedValue::String(_) | EvaluatedValue::Number(_))
        )
    {
        let mut text = any_to_string(left)
            .expect("string or number")
            .as_bytes()
            .to_vec();
        text.extend_from_slice(any_to_string(right).expect("string or number").as_bytes());
        return Some(EvaluatedValue::String(JsString::from_bytes(text)));
    }
    None
}

#[cfg(test)]
mod tests;
