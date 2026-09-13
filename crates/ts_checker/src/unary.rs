//! Unary expression checks preserve native literal fast paths and error recovery.
use crate::{type_facts as facts, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{JsString, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.maybeTypeOfKindConsideringBaseConstraint
    pub(crate) fn maybe_type_with_constraint(
        &mut self,
        ty: TypeId,
        flags: u32,
    ) -> Result<bool, Error> {
        if self.maybe_type_of_kind(ty, flags)? {
            return Ok(true);
        }
        let base = self.base_constraint_of_type(ty)?.unwrap_or(ty);
        self.maybe_type_of_kind(base, flags)
    }

    // port: tsc/internal/checker/checker.go:Checker.getUnaryResultType
    fn unary_result_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if self.maybe_type_of_kind(ty, tf::BIG_INT_LIKE)? {
            if self.type_assignable_to_kind(ty, tf::ANY_OR_UNKNOWN)?
                || self.maybe_type_of_kind(ty, tf::NUMBER_LIKE)?
            {
                return Ok(self.builtins.number_or_big_int_type);
            }
            return Ok(self.builtins.bigint_type);
        }
        Ok(self.builtins.number_type)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkPrefixUnaryExpression
    // port: tsc/internal/checker/checker.go:Checker.checkPostfixUnaryExpression
    pub(crate) fn check_unary_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let prefix = read.kind() == K::PrefixUnaryExpression;
        let (operand, operator) = if prefix {
            let data = read
                .data_source()
                .as_prefix_unary_expression()
                .ok_or(Error::MissingLink("prefix expression"))?;
            (data.operand(), data.operator())
        } else {
            let data = read
                .data_source()
                .as_postfix_unary_expression()
                .ok_or(Error::MissingLink("postfix expression"))?;
            (data.operand(), data.operator())
        };
        let operand = operand.ok_or(Error::MissingLink("unary operand"))?;
        let ty = self.check_expression(operand)?;
        if ty == self.builtins.silent_never_type {
            return Ok(ty);
        }
        if prefix {
            let kind = self.ast(operand)?.node(operand)?.kind();
            if kind == K::NumericLiteral
                && matches!(operator.known(), Some(K::PlusToken | K::MinusToken))
            {
                let n = ts_jsnum::from_string(self.ast(operand)?.node_text(operand)?.as_bytes())
                    .value();
                let literal = self.get_number_literal_type(ts_jsnum::Number::new(
                    if operator == K::MinusToken { -n } else { n },
                ))?;
                return self.get_fresh_type_of_literal_type(literal);
            }
            if kind == K::BigIntLiteral && operator == K::MinusToken {
                let n = ts_jsnum::PseudoBigInt::new(
                    &ts_jsnum::parse_pseudo_big_int(
                        self.ast(operand)?.node_text(operand)?.as_bytes(),
                    ),
                    true,
                );
                let literal = self.get_big_int_literal_type(n)?;
                return self.get_fresh_type_of_literal_type(literal);
            }
            match operator.known() {
                Some(K::PlusToken | K::MinusToken | K::TildeToken) => {
                    self.check_non_null_type(ty, operand)?;
                    let token = JsString::from_bytes(
                        ts_scanner::token_to_string(operator.known().expect("unary operator arm"))
                            .as_bytes(),
                    );
                    if self.maybe_type_with_constraint(ty, tf::ES_SYMBOL_LIKE)? {
                        self.error_at(
                            Some(operand),
                            d::The_0_operator_cannot_be_applied_to_type_symbol,
                            vec![token.clone()],
                        )?;
                    }
                    if operator == K::PlusToken {
                        if self.maybe_type_with_constraint(ty, tf::BIG_INT_LIKE)? {
                            let base = self.base_literal_type(ty)?;
                            let display =
                                self.type_to_string(base, crate::type_display::DEFAULT_FLAGS)?;
                            self.error_at(
                                Some(operand),
                                d::Operator_0_cannot_be_applied_to_type_1,
                                vec![token, display],
                            )?;
                        }
                        return Ok(self.builtins.number_type);
                    }
                    return self.unary_result_type(ty);
                }
                Some(K::ExclamationToken) => {
                    self.check_truthiness_type(ty, operand)?;
                    return Ok(match self.type_facts(ty, facts::TRUTHY | facts::FALSY)? {
                        facts::TRUTHY => self.builtins.false_type,
                        facts::FALSY => self.builtins.true_type,
                        _ => self.builtins.boolean_type,
                    });
                }
                Some(K::PlusPlusToken | K::MinusMinusToken) => {}
                _ => return Ok(self.builtins.error_type),
            }
        }
        let non_null = self.check_non_null_type(ty, operand)?;
        if self.arithmetic_operand(
            operand,
            non_null,
            d::An_arithmetic_operand_must_be_of_type_any_number_bigint_or_an_enum_type,
            false,
        )? {
            self.check_reference_expression(operand, d::The_operand_of_an_increment_or_decrement_operator_must_be_a_variable_or_a_property_access, d::The_operand_of_an_increment_or_decrement_operator_may_not_be_an_optional_property_access)?;
        }
        self.unary_result_type(ty)
    }
}
