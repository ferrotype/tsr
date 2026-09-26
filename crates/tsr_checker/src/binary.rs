//! Binary operators share the production relation and flow type stores.
use crate::{
    type_facts as f, type_flags as tf, CheckerState, Error, LiteralValue, RelationKind, TypeId,
    UnionReduction,
};
use tsr_arena::NodeId;
use tsr_ast::{JsString, NodeKind, SyntaxKind as K};
use tsr_diagnostics as d;

type OperandRelation = fn(&mut CheckerState, TypeId, TypeId) -> Result<bool, Error>;

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
                || flags & tf::BIG_INT_LITERAL != 0 && matches!(&c.types.literal(t)?.value, LiteralValue::BigInt(n) if *n == tsr_jsnum::PseudoBigInt::default()) { t }
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
        if tsr_ast::utilities::is_logical_or_coalescing_binary_operator(operator) {
            let mut parent = match self.node(left)?.parent() {
                Some(parent) => self.node(parent)?.parent(),
                None => None,
            };
            while let Some(node) = parent {
                if self.node(node)?.kind() != K::ParenthesizedExpression
                    && !tsr_ast::utilities::is_logical_or_coalescing_binary_expression(
                        self.ast(node)?,
                        node,
                    )?
                {
                    break;
                }
                parent = self.node(node)?.parent();
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
                    Some(node) if self.node(node)?.kind() == K::IfStatement => self
                        .ast(node)?
                        .node(node)?
                        .data_source()
                        .as_if_statement()
                        .ok_or(tsr_arena::Error::InvalidGraph)?
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
            _ => return Err(tsr_arena::Error::InvalidGraph.into()),
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
            if !self.relational_operands_compatible(a, b)? {
                self.report_binary_operator_error(
                    node,
                    operator,
                    a,
                    b,
                    Some(Self::relational_operands_compatible),
                )?;
            }
        }
        Ok(self.builtins.boolean_type)
    }

    fn relational_operands_compatible(&mut self, a: TypeId, b: TypeId) -> Result<bool, Error> {
        if (self.types.flags(a)? | self.types.flags(b)?) & tf::ANY != 0 {
            return Ok(true);
        }
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
        Ok(an && bn || !an && !bn && self.types_comparable(a, b)?)
    }

    pub(crate) fn addition_operands_close_enough(
        &mut self,
        a: TypeId,
        b: TypeId,
    ) -> Result<bool, Error> {
        let close = tf::NUMBER_LIKE | tf::BIG_INT_LIKE | tf::STRING_LIKE | tf::ANY_OR_UNKNOWN;
        Ok(self.type_assignable_to_kind(a, close)? && self.type_assignable_to_kind(b, close)?)
    }

    // port: tsc/internal/checker/checker.go:Checker.bothAreBigIntLike
    pub(crate) fn both_big_int_like(&mut self, a: TypeId, b: TypeId) -> Result<bool, Error> {
        Ok(self.type_assignable_to_kind(a, tf::BIG_INT_LIKE)?
            && self.type_assignable_to_kind(b, tf::BIG_INT_LIKE)?)
    }

    // port: tsc/internal/checker/checker.go:Checker.reportOperatorError
    // port: tsc/internal/checker/checker.go:Checker.getBaseTypesIfUnrelated
    // port: tsc/internal/checker/checker.go:Checker.errorAndMaybeSuggestAwait
    pub(crate) fn report_binary_operator_error(
        &mut self,
        node: NodeId,
        operator: NodeKind,
        a: TypeId,
        b: TypeId,
        is_related: Option<OperandRelation>,
    ) -> Result<(), Error> {
        let mut would_work_with_await = false;
        if let Some(is_related) = is_related {
            if let (Some(awaited_a), Some(awaited_b)) = (
                self.awaited_type_no_alias(a)?,
                self.awaited_type_no_alias(b)?,
            ) {
                would_work_with_await =
                    !(awaited_a == a && awaited_b == b) && is_related(self, awaited_a, awaited_b)?;
            }
        }
        let (mut a, mut b) = (a, b);
        if !would_work_with_await {
            if let Some(is_related) = is_related {
                let left_base = self.base_literal_type(a)?;
                let right_base = self.base_literal_type(b)?;
                if !is_related(self, left_base, right_base)? {
                    a = left_base;
                    b = right_base;
                }
            }
        }
        let (a, b) = self.type_names_for_error_display(a, b)?;
        let (message, arguments) = match operator.known() {
            Some(
                K::EqualsEqualsToken
                | K::EqualsEqualsEqualsToken
                | K::ExclamationEqualsToken
                | K::ExclamationEqualsEqualsToken,
            ) => (
                d::This_comparison_appears_to_be_unintentional_because_the_types_0_and_1_have_no_overlap,
                vec![a, b],
            ),
            _ => (
                d::Operator_0_cannot_be_applied_to_types_1_and_2,
                vec![operator_text(operator), a, b],
            ),
        };
        let mut diagnostic = self.diagnostic_for_node(Some(node), message, arguments)?;
        if would_work_with_await {
            diagnostic
                .related_information
                .push(std::sync::Arc::new(self.diagnostic_for_node(
                    Some(node),
                    d::Did_you_forget_to_use_await,
                    vec![],
                )?));
        }
        self.add_diagnostic(diagnostic)?;
        Ok(())
    }
}

fn operator_text(kind: NodeKind) -> JsString {
    JsString::from_bytes(
        tsr_scanner::token_to_string(
            kind.known()
                .expect("operator dispatch checked the syntax kind"),
        )
        .as_bytes(),
    )
}
