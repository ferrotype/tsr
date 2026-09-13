//! The pinned evaluator with checker name resolution as its entity callback.
//! Assertions deliberately stop evaluation; only parentheses are skipped.

use crate::{
    enums::{EnumEvaluation, EnumValue},
    CheckerState, Error,
};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{node_flags as nf, symbol_flags as sf, SyntaxKind as K};
use ts_diagnostics as messages;
use ts_jsnum::Number;
use ts_jsstring::JsString;

impl CheckerState {
    // port: tsc/internal/evaluator/evaluator.go:NewEvaluator
    pub(crate) fn evaluate_enum_expression(
        &mut self,
        mut expression: NodeId,
        location: Option<NodeId>,
    ) -> Result<EnumEvaluation, Error> {
        while self.ast(expression)?.node(expression)?.kind() == K::ParenthesizedExpression {
            expression = self
                .ast(expression)?
                .node(expression)?
                .expression()
                .ok_or(Error::MissingLink("parenthesized enum expression"))?;
        }
        let read = self.ast(expression)?.node(expression)?;
        let mut result = EnumEvaluation::default();
        match read.kind().known() {
            Some(K::PrefixUnaryExpression) => {
                let data = read
                    .data_source()
                    .as_prefix_unary_expression()
                    .ok_or(Error::MissingLink("enum unary expression"))?;
                let operator = data.operator();
                let operand = data
                    .operand()
                    .ok_or(Error::MissingLink("enum unary operand"))?;
                let operand = self.evaluate_enum_expression(operand, location)?;
                result.resolved_other_files = operand.resolved_other_files;
                result.has_external_references = operand.has_external_references;
                if let Some(EnumValue::Number(number)) = operand.value {
                    result.value = match operator.known() {
                        Some(K::PlusToken) => Some(EnumValue::Number(number)),
                        Some(K::MinusToken) => {
                            Some(EnumValue::Number(Number::new(-number.value())))
                        }
                        Some(K::TildeToken) => Some(EnumValue::Number(number.bitwise_not())),
                        _ => None,
                    };
                }
            }
            Some(K::BinaryExpression) => {
                let data = read
                    .data_source()
                    .as_binary_expression()
                    .ok_or(Error::MissingLink("enum binary expression"))?;
                let left = data.left().ok_or(Error::MissingLink("enum binary left"))?;
                let right = data
                    .right()
                    .ok_or(Error::MissingLink("enum binary right"))?;
                let operator = data
                    .operator_token()
                    .ok_or(Error::MissingLink("enum binary operator"))?;
                let operator = self.ast(operator)?.node(operator)?.kind();
                let left = self.evaluate_enum_expression(left, location)?;
                let right = self.evaluate_enum_expression(right, location)?;
                result.is_syntactically_string = (left.is_syntactically_string
                    || right.is_syntactically_string)
                    && operator == K::PlusToken;
                result.resolved_other_files =
                    left.resolved_other_files || right.resolved_other_files;
                result.has_external_references =
                    left.has_external_references || right.has_external_references;
                if let (Some(EnumValue::Number(left)), Some(EnumValue::Number(right))) =
                    (&left.value, &right.value)
                {
                    let value = match operator.known() {
                        Some(K::BarToken) => Some(left.bitwise_or(*right)),
                        Some(K::AmpersandToken) => Some(left.bitwise_and(*right)),
                        Some(K::CaretToken) => Some(left.bitwise_xor(*right)),
                        Some(K::GreaterThanGreaterThanToken) => {
                            Some(left.signed_right_shift(*right))
                        }
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
                    if let Some(value) = value {
                        result.value = Some(EnumValue::Number(value));
                        return Ok(result);
                    }
                }
                if operator == K::PlusToken {
                    if let (Some(left), Some(right)) = (left.value, right.value) {
                        let mut text = left.text().as_bytes().to_vec();
                        text.extend_from_slice(right.text().as_bytes());
                        result.value = Some(EnumValue::String(JsString::from_bytes(text)));
                    }
                }
            }
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral) => {
                result.value = Some(EnumValue::String(
                    self.ast(expression)?
                        .node_text(expression)?
                        .into_js_string(),
                ));
                result.is_syntactically_string = true;
            }
            Some(K::NumericLiteral) => {
                result.value = Some(EnumValue::Number(ts_jsnum::from_string(
                    self.ast(expression)?.node_text(expression)?.as_bytes(),
                )));
            }
            Some(K::TemplateExpression) => {
                return self.evaluate_enum_template(expression, location)
            }
            Some(K::Identifier) => return self.evaluate_enum_entity(expression, location),
            Some(K::ElementAccessExpression | K::PropertyAccessExpression) => {
                let root = read
                    .expression()
                    .ok_or(Error::MissingLink("enum access expression"))?;
                if ts_ast::is_entity_name_expression(self.ast(root)?, root)? {
                    return self.evaluate_enum_entity(expression, location);
                }
            }
            _ => {}
        }
        Ok(result)
    }

    // port: tsc/internal/evaluator/evaluator.go:evaluateTemplateExpression
    fn evaluate_enum_template(
        &mut self,
        expression: NodeId,
        location: Option<NodeId>,
    ) -> Result<EnumEvaluation, Error> {
        let read = self.ast(expression)?.node(expression)?;
        let data = read
            .data_source()
            .as_template_expression()
            .ok_or(Error::MissingLink("enum template expression"))?;
        let head = data
            .head()
            .ok_or(Error::MissingLink("enum template head"))?;
        let spans = self.source_list(expression, data.template_spans())?;
        let mut text = self.ast(head)?.node_text(head)?.as_bytes().to_vec();
        let mut result = EnumEvaluation {
            is_syntactically_string: true,
            ..Default::default()
        };
        for span in spans {
            let read = self.ast(span)?.node(span)?;
            let data = read
                .data_source()
                .as_template_span()
                .ok_or(Error::MissingLink("enum template span"))?;
            let expression = data
                .expression()
                .ok_or(Error::MissingLink("enum template span expression"))?;
            let literal = data
                .literal()
                .ok_or(Error::MissingLink("enum template span literal"))?;
            let value = self.evaluate_enum_expression(expression, location)?;
            let Some(part) = value.value else {
                // Upstream discards accumulated external-reference flags on this exit.
                return Ok(EnumEvaluation {
                    is_syntactically_string: true,
                    ..Default::default()
                });
            };
            text.extend_from_slice(part.text().as_bytes());
            text.extend_from_slice(self.ast(literal)?.node_text(literal)?.as_bytes());
            result.resolved_other_files |= value.resolved_other_files;
            result.has_external_references |= value.has_external_references;
        }
        result.value = Some(EnumValue::String(JsString::from_bytes(text)));
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.evaluateEntity
    fn evaluate_enum_entity(
        &mut self,
        expression: NodeId,
        location: Option<NodeId>,
    ) -> Result<EnumEvaluation, Error> {
        let read = self.ast(expression)?.node(expression)?;
        if matches!(
            read.kind().known(),
            Some(K::Identifier | K::PropertyAccessExpression)
        ) {
            let is_identifier = read.kind() == K::Identifier;
            let Some(symbol) = self.resolve_entity_name(expression, sf::VALUE, true)? else {
                return Ok(EnumEvaluation::default());
            };
            if is_identifier {
                let text = self
                    .ast(expression)?
                    .node_text(expression)?
                    .into_js_string();
                if matches!(text.as_bytes(), b"Infinity" | b"-Infinity" | b"NaN")
                    && self.resolve_name(None, text.as_bytes(), sf::VALUE, None, false)?
                        == Some(symbol)
                {
                    return Ok(EnumEvaluation {
                        value: Some(EnumValue::Number(ts_jsnum::from_string(text.as_bytes()))),
                        ..Default::default()
                    });
                }
            }
            if self.symbol(symbol)?.flags() & sf::ENUM_MEMBER != 0 {
                return if let Some(location) = location {
                    self.evaluate_enum_member(expression, symbol, location)
                } else {
                    let declaration = self
                        .symbol(symbol)?
                        .value_declaration()
                        .ok_or(Error::MissingLink("enum member value declaration"))?;
                    self.enum_member_value(declaration)
                };
            }
            let declaration = self.symbol(symbol)?.value_declaration();
            if self.symbol(symbol)?.flags() & sf::VARIABLE != 0 {
                if let Some(declaration) = declaration {
                    let read = self.ast(declaration)?.node(declaration)?;
                    if read.kind() == K::VariableDeclaration
                        && read.type_node().is_none()
                        && ts_ast::utilities::get_combined_node_flags(
                            self.ast(declaration)?,
                            declaration,
                        )? & nf::CONSTANT
                            != 0
                    {
                        if let Some(initializer) = read.initializer() {
                            let before = match location {
                                Some(location) => {
                                    declaration != location
                                        && self
                                            .enum_declaration_before_use(declaration, location)?
                                }
                                None => true,
                            };
                            if before {
                                let mut result =
                                    self.evaluate_enum_expression(initializer, Some(declaration))?;
                                if let Some(location) = location {
                                    if self.enum_source_file(location)?
                                        != self.enum_source_file(declaration)?
                                    {
                                        result.is_syntactically_string = false;
                                        result.resolved_other_files = true;
                                    }
                                }
                                result.has_external_references = true;
                                return Ok(result);
                            }
                        }
                    }
                }
            }
        } else if read.kind() == K::ElementAccessExpression {
            let data = read
                .data_source()
                .as_element_access_expression()
                .ok_or(Error::MissingLink("enum element access"))?;
            let root = data
                .expression()
                .ok_or(Error::MissingLink("enum element root"))?;
            let argument = data
                .argument_expression()
                .ok_or(Error::MissingLink("enum element argument"))?;
            if ts_ast::is_entity_name_expression(self.ast(root)?, root)?
                && matches!(
                    self.ast(argument)?.node(argument)?.kind().known(),
                    Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
                )
            {
                if let Some(root) = self.resolve_entity_name(root, sf::VALUE, true)? {
                    if self.symbol(root)?.flags() & sf::ENUM != 0 {
                        let name = self.ast(argument)?.node_text(argument)?.into_js_string();
                        if let Some(member) =
                            self.member_symbol(self.symbol(root)?.exports(), name.as_bytes())?
                        {
                            return if let Some(location) = location {
                                self.evaluate_enum_member(expression, member, location)
                            } else {
                                let declaration = self
                                    .symbol(member)?
                                    .value_declaration()
                                    .ok_or(Error::MissingLink("enum member declaration"))?;
                                self.enum_member_value(declaration)
                            };
                        }
                    }
                }
            }
        }
        Ok(EnumEvaluation::default())
    }

    // port: tsc/internal/checker/checker.go:Checker.evaluateEnumMember
    fn evaluate_enum_member(
        &mut self,
        expression: NodeId,
        symbol: SymbolId,
        location: NodeId,
    ) -> Result<EnumEvaluation, Error> {
        let declaration = self.symbol(symbol)?.value_declaration();
        if declaration.is_none() || declaration == Some(location) {
            let name = self.symbol_to_string(symbol)?;
            self.error_at(
                Some(expression),
                messages::Property_0_is_used_before_being_assigned,
                vec![name],
            )?;
            return Ok(EnumEvaluation::default());
        }
        let declaration = declaration.expect("enum member declaration was checked");
        if !self.enum_declaration_before_use(declaration, location)? {
            self.error_at(Some(expression), messages::A_member_initializer_in_a_enum_declaration_cannot_reference_members_declared_after_it_including_members_defined_in_other_enums, vec![])?;
            return Ok(EnumEvaluation {
                value: Some(EnumValue::Number(Number::new(0.0))),
                ..Default::default()
            });
        }
        let mut value = self.enum_member_value(declaration)?;
        if self.ast(location)?.node(location)?.parent()
            != self.ast(declaration)?.node(declaration)?.parent()
        {
            value.has_external_references = true;
        }
        Ok(value)
    }

    pub(crate) fn enum_source_file(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        Ok(ts_ast::utilities::get_source_file_of_node(
            self.ast(node)?,
            Some(node),
        )?)
    }

    /// The evaluator calls this with an EnumMember or a constant variable as
    /// both location and declaration. General deferred-use checking is separate.
    // port: tsc/internal/checker/checker.go:Checker.isBlockScopedNameDeclaredBeforeUse
    fn enum_declaration_before_use(
        &self,
        declaration: NodeId,
        usage: NodeId,
    ) -> Result<bool, Error> {
        if self.enum_source_file(declaration)? != self.enum_source_file(usage)? {
            return Ok(true);
        }
        if self.ast(declaration)?.node(declaration)?.pos() > self.ast(usage)?.node(usage)?.pos() {
            return Ok(false);
        }
        if self.ast(declaration)?.node(declaration)?.kind() == K::VariableDeclaration {
            let mut ancestor = Some(usage);
            while let Some(node) = ancestor {
                if node == declaration {
                    return Ok(false);
                }
                ancestor = self.ast(node)?.node(node)?.parent();
            }
        }
        Ok(true)
    }
}
