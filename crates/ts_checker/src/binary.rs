//! Binary operators share the production relation and flow type stores.
use crate::{
    type_facts as f, type_flags as tf, CheckerState, Error, LiteralValue, RelationKind, TypeId,
    UnionReduction,
};
use ts_arena::NodeId;
use ts_ast::{JsString, NodeKind, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.isTypeAssignableToKindEx
    pub(crate) fn type_assignable_to_kind_strict(
        &mut self,
        ty: TypeId,
        flags: u32,
    ) -> Result<bool, Error> {
        let source = self.types.flags(ty)?;
        if source & flags != 0 {
            return Ok(true);
        }
        if source & (tf::ANY_OR_UNKNOWN | tf::VOID | tf::UNDEFINED | tf::NULL) != 0 {
            return Ok(false);
        }
        self.type_assignable_to_kind(ty, flags)
    }

    // port: tsc/internal/checker/checker.go:Checker.getBaseTypeOfLiteralTypeForComparison
    fn comparison_base_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let flags = self.types.flags(ty)?;
        for (mask, base) in [
            (
                tf::STRING_LITERAL | tf::TEMPLATE_LITERAL | tf::STRING_MAPPING,
                self.builtins.string_type,
            ),
            (tf::NUMBER_LITERAL | tf::ENUM, self.builtins.number_type),
            (tf::BIG_INT_LITERAL, self.builtins.bigint_type),
            (tf::BOOLEAN_LITERAL, self.builtins.boolean_type),
        ] {
            if flags & mask != 0 {
                return Ok(base);
            }
        }
        if flags & tf::UNION != 0 {
            return self
                .map_type(ty, &mut |c, t| c.comparison_base_type(t).map(Some))?
                .ok_or(Error::MissingLink("comparison base union"));
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.extractDefinitelyFalsyTypes
    fn definitely_falsy_types(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        self.map_type(ty, &mut |c,t| {
            let flags = c.types.flags(t)?;
            let result = if flags & tf::STRING != 0 { c.builtins.empty_string_type }
            else if flags & tf::NUMBER != 0 { c.builtins.zero_type }
            else if flags & tf::BIG_INT != 0 { c.builtins.zero_big_int_type }
            else if t == c.builtins.regular_false_type || t == c.builtins.false_type
                || flags & (tf::VOID | tf::UNDEFINED | tf::NULL | tf::ANY_OR_UNKNOWN) != 0
                || flags & tf::STRING_LITERAL != 0 && matches!(&c.types.literal(t)?.value, LiteralValue::String(s) if s.is_empty())
                || flags & tf::NUMBER_LITERAL != 0 && matches!(&c.types.literal(t)?.value, LiteralValue::Number(n) if n.value() == 0.0)
                || flags & tf::BIG_INT_LITERAL != 0 && matches!(&c.types.literal(t)?.value, LiteralValue::BigInt(n) if *n == ts_jsnum::PseudoBigInt::default()) { t }
            else { c.builtins.never_type };
            Ok(Some(result))
        })?.ok_or(Error::MissingLink("falsy union"))
    }

    pub(crate) fn logical_binary(
        &mut self,
        left: NodeId,
        right: NodeId,
        operator: NodeKind,
        a: TypeId,
        b: TypeId,
    ) -> Result<TypeId, Error> {
        // Shared source rules precede the operator result. Calls/enum guards are
        // a separate source check; unresolved dependencies remain explicit.
        if ts_ast::utilities::is_logical_or_coalescing_binary_operator(operator) {
            let mut parent = match self.ast(left)?.node(left)?.parent() {
                Some(parent) => self.ast(parent)?.node(parent)?.parent(),
                None => None,
            };
            while let Some(node) = parent {
                if self.ast(node)?.node(node)?.kind() != K::ParenthesizedExpression
                    && !ts_ast::utilities::is_logical_or_coalescing_binary_expression(
                        self.ast(node)?,
                        node,
                    )?
                {
                    break;
                }
                parent = self.ast(node)?.node(node)?.parent();
            }
            if operator == K::AmpersandAmpersandToken
                || parent
                    .map(|n| {
                        self.ast(n)?
                            .node(n)
                            .map(|n| n.kind() == K::IfStatement)
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(false)
            {
                let body = match parent {
                    Some(node) if self.ast(node)?.node(node)?.kind() == K::IfStatement => self
                        .ast(node)?
                        .node(node)?
                        .data_source()
                        .as_if_statement()
                        .ok_or(ts_arena::Error::InvalidGraph)?
                        .then_statement(),
                    _ => None,
                };
                self.check_known_truthy_guard(left, a, body)?;
            }
            if matches!(
                operator.known(),
                Some(K::AmpersandAmpersandToken | K::BarBarToken)
            ) {
                self.check_truthiness_type(a, left)?;
            }
        }
        let result = match operator.known() {
            Some(K::AmpersandAmpersandToken | K::AmpersandAmpersandEqualsToken) => {
                if self.type_facts(a, f::TRUTHY)? == 0 {
                    a
                } else {
                    let source = if self.options.strict_null_checks {
                        a
                    } else {
                        self.base_literal_type(b)?
                    };
                    let falsy = self.definitely_falsy_types(source)?;
                    self.get_union_type(&[falsy, b])?
                }
            }
            Some(K::BarBarToken | K::BarBarEqualsToken) => {
                if self.type_facts(a, f::FALSY)? == 0 {
                    a
                } else {
                    let truthy =
                        self.filter_type(a, &mut |c, t| Ok(c.type_facts(t, f::TRUTHY)? != 0))?;
                    let truthy = self.non_nullable_type(truthy)?;
                    self.get_union_type_ex(&[truthy, b], UnionReduction::Subtype, None, None)?
                }
            }
            Some(K::QuestionQuestionToken | K::QuestionQuestionEqualsToken) => {
                if operator == K::QuestionQuestionToken {
                    self.check_nullish_operands(left, right)?;
                }
                if self.type_facts(a, f::EQ_UNDEFINED_OR_NULL)? == 0 {
                    a
                } else {
                    let non_null = self.non_nullable_type(a)?;
                    self.get_union_type_ex(&[non_null, b], UnionReduction::Subtype, None, None)?
                }
            }
            _ => return Err(ts_arena::Error::InvalidGraph.into()),
        };
        if matches!(
            operator.known(),
            Some(
                K::AmpersandAmpersandEqualsToken
                    | K::BarBarEqualsToken
                    | K::QuestionQuestionEqualsToken
            )
        ) {
            self.assignment_operator(left, right, operator, a, b)?;
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkForDisallowedESSymbolOperand
    pub(crate) fn allowed_symbol_operands(
        &mut self,
        left: NodeId,
        right: NodeId,
        a: TypeId,
        b: TypeId,
        operator: NodeKind,
    ) -> Result<bool, Error> {
        for (node, ty) in [(left, a), (right, b)] {
            let symbol = if self.maybe_type_of_kind(ty, tf::ES_SYMBOL_LIKE)? {
                true
            } else {
                let base = self.base_constraint_of_type(ty)?.unwrap_or(ty);
                self.maybe_type_of_kind(base, tf::ES_SYMBOL_LIKE)?
            };
            if symbol {
                self.error_at(
                    Some(node),
                    d::The_0_operator_cannot_be_applied_to_type_symbol,
                    vec![operator_text(operator)],
                )?;
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn relational_binary(
        &mut self,
        node: NodeId,
        left: NodeId,
        right: NodeId,
        operator: NodeKind,
        a: TypeId,
        b: TypeId,
    ) -> Result<TypeId, Error> {
        if self.allowed_symbol_operands(left, right, a, b, operator)? {
            let a = self.check_non_null_type(a, left)?;
            let b = self.check_non_null_type(b, right)?;
            let a = self.comparison_base_type(a)?;
            let b = self.comparison_base_type(b)?;
            let compatible = if (self.types.flags(a)? | self.types.flags(b)?) & tf::ANY != 0 {
                true
            } else {
                let an = self.is_type_related_to(
                    a,
                    self.builtins.number_or_big_int_type,
                    RelationKind::Assignable,
                )?;
                let bn = self.is_type_related_to(
                    b,
                    self.builtins.number_or_big_int_type,
                    RelationKind::Assignable,
                )?;
                an && bn || !an && !bn && self.types_comparable(a, b)?
            };
            if !compatible {
                self.report_binary_operator_error(node, operator, a, b)?;
            }
        }
        Ok(self.builtins.boolean_type)
    }

    pub(crate) fn report_binary_operator_error(
        &mut self,
        node: NodeId,
        operator: NodeKind,
        a: TypeId,
        b: TypeId,
    ) -> Result<(), Error> {
        // Missing-await elaboration changes both message and related information.
        if self.property_type(a, b"then")?.is_some() || self.property_type(b, b"then")?.is_some() {
            return Err(Error::Unsupported(
                "reportOperatorError: awaited operand suggestions",
            ));
        }
        let a = self.type_to_string(a, crate::type_display::DEFAULT_FLAGS)?;
        let b = self.type_to_string(b, crate::type_display::DEFAULT_FLAGS)?;
        self.error_at(
            Some(node),
            d::Operator_0_cannot_be_applied_to_types_1_and_2,
            vec![operator_text(operator), a, b],
        )?;
        Ok(())
    }
}

fn operator_text(kind: NodeKind) -> JsString {
    JsString::from_bytes(
        ts_scanner::token_to_string(
            kind.known()
                .expect("operator dispatch checked the syntax kind"),
        )
        .as_bytes(),
    )
}
