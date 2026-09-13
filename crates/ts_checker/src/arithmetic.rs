//! Arithmetic binary operators, including evaluator-backed shift suggestions.
use crate::{type_flags as tf, CheckerState, Error, RelationKind, TypeId};
use ts_arena::NodeId;
use ts_ast::{JsString, NodeKind, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkBinaryLikeExpression
    pub(crate) fn arithmetic_binary(
        &mut self,
        node: NodeId,
        left: NodeId,
        right: NodeId,
        token: NodeId,
        mut a: TypeId,
        mut b: TypeId,
    ) -> Result<TypeId, Error> {
        let operator = self.ast(token)?.node(token)?.kind();
        if a == self.builtins.silent_never_type || b == self.builtins.silent_never_type {
            return Ok(self.builtins.silent_never_type);
        }
        let plus = matches!(operator.known(), Some(K::PlusToken | K::PlusEqualsToken));
        if !plus
            || !self.type_assignable_to_kind(a, tf::STRING_LIKE)?
                && !self.type_assignable_to_kind(b, tf::STRING_LIKE)?
        {
            a = self.check_non_null_type(a, left)?;
            b = self.check_non_null_type(b, right)?;
        }
        if plus {
            let result = if self.type_assignable_to_kind_strict(a, tf::NUMBER_LIKE)?
                && self.type_assignable_to_kind_strict(b, tf::NUMBER_LIKE)?
            {
                Some(self.builtins.number_type)
            } else if self.type_assignable_to_kind_strict(a, tf::BIG_INT_LIKE)?
                && self.type_assignable_to_kind_strict(b, tf::BIG_INT_LIKE)?
            {
                Some(self.builtins.bigint_type)
            } else if self.type_assignable_to_kind_strict(a, tf::STRING_LIKE)?
                || self.type_assignable_to_kind_strict(b, tf::STRING_LIKE)?
            {
                Some(self.builtins.string_type)
            } else if (self.types.flags(a)? | self.types.flags(b)?) & tf::ANY != 0 {
                Some(if self.is_error_type(a)? || self.is_error_type(b)? {
                    self.builtins.error_type
                } else {
                    self.builtins.any_type
                })
            } else {
                None
            };
            if let Some(result) = result {
                if self.allowed_symbol_operands(left, right, a, b, operator)? {
                    self.assignment_operator(left, right, operator, a, result)?;
                }
                return Ok(result);
            }
            self.report_binary_operator_error(node, operator, a, b)?;
            return Ok(self.builtins.any_type);
        }
        if self.types.flags(a)? & tf::BOOLEAN_LIKE != 0
            && self.types.flags(b)? & tf::BOOLEAN_LIKE != 0
        {
            let suggestion = match operator.known() {
                Some(K::BarToken | K::BarEqualsToken) => Some(K::BarBarToken),
                Some(K::CaretToken | K::CaretEqualsToken) => Some(K::ExclamationEqualsEqualsToken),
                Some(K::AmpersandToken | K::AmpersandEqualsToken) => {
                    Some(K::AmpersandAmpersandToken)
                }
                _ => None,
            };
            if let Some(suggestion) = suggestion {
                self.error_at(
                    Some(token),
                    d::The_0_operator_is_not_allowed_for_boolean_types_Consider_using_1_instead,
                    vec![
                        operator_text(operator),
                        JsString::from_bytes(ts_scanner::token_to_string(suggestion).as_bytes()),
                    ],
                )?;
                return Ok(self.builtins.number_type);
            }
        }
        let left_ok = self.arithmetic_operand(left,a,d::The_left_hand_side_of_an_arithmetic_operation_must_be_of_type_any_number_bigint_or_an_enum_type,true)?;
        let right_ok = self.arithmetic_operand(right,b,d::The_right_hand_side_of_an_arithmetic_operation_must_be_of_type_any_number_bigint_or_an_enum_type,true)?;
        let result = if self.type_assignable_to_kind(a, tf::ANY_OR_UNKNOWN)?
            && self.type_assignable_to_kind(b, tf::ANY_OR_UNKNOWN)?
            || !self.maybe_type_of_kind(a, tf::BIG_INT_LIKE)?
                && !self.maybe_type_of_kind(b, tf::BIG_INT_LIKE)?
        {
            self.builtins.number_type
        } else if self.type_assignable_to_kind(a, tf::BIG_INT_LIKE)?
            && self.type_assignable_to_kind(b, tf::BIG_INT_LIKE)?
        {
            match operator.known() {
                Some(
                    K::GreaterThanGreaterThanGreaterThanToken
                    | K::GreaterThanGreaterThanGreaterThanEqualsToken,
                ) => self.report_binary_operator_error(node, operator, a, b)?,
                Some(K::AsteriskAsteriskToken | K::AsteriskAsteriskEqualsToken)
                    if self.program()?.host.options().emit_script_target()
                        < ts_core::ScriptTarget::ES2016 =>
                {
                    self.error_at(Some(node),d::Exponentiation_cannot_be_performed_on_bigint_values_unless_the_target_option_is_set_to_es2016_or_later,vec![])?;
                }
                _ => {}
            }
            self.builtins.bigint_type
        } else {
            self.report_binary_operator_error(node, operator, a, b)?;
            self.builtins.error_type
        };
        if left_ok && right_ok {
            self.assignment_operator(left, right, operator, a, result)?;
            if matches!(
                operator.known(),
                Some(
                    K::LessThanLessThanToken
                        | K::LessThanLessThanEqualsToken
                        | K::GreaterThanGreaterThanToken
                        | K::GreaterThanGreaterThanEqualsToken
                        | K::GreaterThanGreaterThanGreaterThanToken
                        | K::GreaterThanGreaterThanGreaterThanEqualsToken
                )
            ) {
                if let Some(crate::enums::EnumValue::Number(value)) =
                    self.evaluate_enum_expression(right, Some(right))?.value
                {
                    if value.value().abs() >= 32.0 {
                        let diagnostic = self.diagnostic_for_node(
                            Some(node),
                            d::This_operation_can_be_simplified_This_shift_is_identical_to_0_1_2,
                            vec![
                                ts_scanner::get_text_of_node(self.ast(left)?, left)?,
                                operator_text(operator),
                                JsString::from_bytes(
                                    ts_jsnum::Number::new(value.value() % 32.0)
                                        .to_string()
                                        .into_bytes(),
                                ),
                            ],
                        )?;
                        let mut container = self.ast(node)?.node(node)?.parent();
                        while let Some(parent) = container {
                            if self.ast(parent)?.node(parent)?.kind() != K::ParenthesizedExpression
                            {
                                break;
                            }
                            container = self.ast(parent)?.node(parent)?.parent();
                        }
                        if container
                            .map(|n| {
                                self.ast(n)?
                                    .node(n)
                                    .map(|n| n.kind() == K::EnumMember)
                                    .map_err(Error::from)
                            })
                            .transpose()?
                            .unwrap_or(false)
                        {
                            self.add_diagnostic(diagnostic)?;
                        } else {
                            let mut diagnostic = diagnostic;
                            diagnostic.category = ts_diagnostics::Category::Suggestion as i32;
                            self.add_suggestion_diagnostic(diagnostic)?;
                        }
                    }
                }
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkArithmeticOperandType
    pub(crate) fn arithmetic_operand(
        &mut self,
        node: NodeId,
        ty: TypeId,
        diagnostic: &'static d::Message,
        await_valid: bool,
    ) -> Result<bool, Error> {
        if self.is_type_related_to(
            ty,
            self.builtins.number_or_big_int_type,
            RelationKind::Assignable,
        )? {
            return Ok(true);
        }
        let awaited = if await_valid {
            self.awaited_type_of_promise(ty)?
        } else {
            None
        };
        let suggest = match awaited {
            Some(awaited) => self.is_type_related_to(
                awaited,
                self.builtins.number_or_big_int_type,
                RelationKind::Assignable,
            )?,
            None => false,
        };
        if let Some(index) = self.error_at(Some(node), diagnostic, vec![])? {
            if suggest {
                let related =
                    self.diagnostic_for_node(Some(node), d::Did_you_forget_to_use_await, vec![])?;
                self.add_related_diagnostic(index, related)?;
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkAssignmentOperator
    pub(crate) fn assignment_operator(
        &mut self,
        left: NodeId,
        right: NodeId,
        operator: NodeKind,
        mut a: TypeId,
        b: TypeId,
    ) -> Result<(), Error> {
        if !ts_ast::is_assignment_operator(operator) {
            return Ok(());
        }
        if let Some(parent) = self.ast(left)?.node(left)?.parent() {
            if ts_ast::is_declaration_node(&self.ast(parent)?.node(parent)?)
                && ts_ast::get_assignment_declaration_kind(self.ast(parent)?, parent)?
                    == ts_ast::JSDeclarationKind::ExportsProperty
            {
                if let Some(symbol) = self.query.resolved_symbols.try_get(left).copied().flatten() {
                    if self.symbol_declarations(symbol)?.len() > 1
                        && self.types.flags(b)? & tf::UNDEFINED != 0
                    {
                        return Ok(());
                    }
                }
            }
        }
        if operator != K::EqualsToken
            && self.ast(left)?.node(left)?.kind() == K::PropertyAccessExpression
        {
            a = self.check_property_access_ex(left, 0, true)?;
        }
        if self.check_reference_expression(left,d::The_left_hand_side_of_an_assignment_expression_must_be_a_variable_or_a_property_access,d::The_left_hand_side_of_an_assignment_expression_may_not_be_an_optional_property_access)? {
            let mut head=None;
            if self.options.exact_optional_property_types && self.ast(left)?.node(left)?.kind()==K::PropertyAccessExpression && self.maybe_type_of_kind(b,tf::UNDEFINED)? {
                let read=self.ast(left)?.node(left)?;
                let expression=read.expression().ok_or(Error::MissingLink("optional property receiver"))?;
                let name=read.name().ok_or(Error::MissingLink("optional property name"))?;
                let text=self.ast(name)?.node_text(name)?.into_js_string();
                let receiver=self.get_type_of_expression(expression)?;
                if let Some(target)=self.property_type(receiver,text.as_bytes())? {
                    if self.type_contains_missing(target)? {head=Some(d::Type_0_is_not_assignable_to_type_1_with_exactOptionalPropertyTypes_Colon_true_Consider_adding_undefined_to_the_type_of_the_target);}
                }
            }
            self.check_expression_related_with_elaboration(b,a,RelationKind::Assignable,Some(left),Some(right),head)?;
        }
        Ok(())
    }
}
fn operator_text(kind: NodeKind) -> JsString {
    JsString::from_bytes(
        ts_scanner::token_to_string(kind.known().expect("arithmetic operator dispatch")).as_bytes(),
    )
}
