//! The pinned evaluator with checker name resolution as its entity callback.
//! Assertions deliberately stop evaluation; only parentheses are skipped.

use crate::{
    enums::{EnumEvaluation, EnumValue},
    CheckerState, Error,
};
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{node_flags as nf, symbol_flags as sf, SyntaxKind as K};
use tsr_diagnostics as messages;
use tsr_jsnum::Number;

impl CheckerState {
    pub(crate) fn evaluate_enum_expression(
        &mut self,
        expression: NodeId,
        location: Option<NodeId>,
    ) -> Result<EnumEvaluation, Error> {
        let result = tsr_ast::evaluator::Evaluator::new(&mut EnumEvaluator(self), 0)
            .evaluate(expression, location)
            .map_err(|error| match error {
                tsr_ast::evaluator::Error::Storage(error) => Error::from(error),
                tsr_ast::evaluator::Error::Context(error) => error,
                tsr_ast::evaluator::Error::MissingLink(link) => Error::MissingLink(link),
                tsr_ast::evaluator::Error::Unhandled(_) => {
                    Error::MissingLink("enum evaluator value")
                }
            })?;
        Ok(EnumEvaluation {
            value: result
                .value
                .map(|value| match value {
                    tsr_ast::evaluator::EvaluatedValue::Number(number) => {
                        Ok(EnumValue::Number(number))
                    }
                    tsr_ast::evaluator::EvaluatedValue::String(text) => Ok(EnumValue::String(text)),
                    _ => Err(Error::MissingLink("enum evaluator value")),
                })
                .transpose()?,
            is_syntactically_string: result.is_syntactically_string,
            resolved_other_files: result.resolved_other_files,
            has_external_references: result.has_external_references,
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.evaluateEntity
    fn evaluate_enum_entity(
        &mut self,
        expression: NodeId,
        location: Option<NodeId>,
    ) -> Result<EnumEvaluation, Error> {
        let read = self.node(expression)?;
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
                        value: Some(EnumValue::Number(tsr_jsnum::from_string(text.as_bytes()))),
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
                    let read = self.node(declaration)?;
                    if read.kind() == K::VariableDeclaration
                        && read.type_node().is_none()
                        && tsr_ast::utilities::get_combined_node_flags(
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
            if tsr_ast::is_entity_name_expression(self.ast(root)?, root)?
                && matches!(
                    self.node(argument)?.kind().known(),
                    Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
                )
            {
                if let Some(root) = self.resolve_entity_name(root, sf::VALUE, true)? {
                    if self.symbol(root)?.flags() & sf::ENUM != 0 {
                        let name = self.node_text(argument)?.into_js_string();
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
        if self.node(location)?.parent() != self.node(declaration)?.parent() {
            value.has_external_references = true;
        }
        Ok(value)
    }

    pub(crate) fn enum_source_file(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        Ok(tsr_ast::utilities::get_source_file_of_node(
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
        if self.node(declaration)?.pos() > self.node(usage)?.pos() {
            return Ok(false);
        }
        if self.node(declaration)?.kind() == K::VariableDeclaration {
            let mut ancestor = Some(usage);
            while let Some(node) = ancestor {
                if node == declaration {
                    return Ok(false);
                }
                ancestor = self.node(node)?.parent();
            }
        }
        Ok(true)
    }
}

// Keep the checker's entity resolver private; only this adapter implements the
// public callback interface. It adds no owner retention or callback allocation.
struct EnumEvaluator<'a>(&'a mut CheckerState);

impl tsr_ast::evaluator::EvaluationContext for EnumEvaluator<'_> {
    type Error = Error;

    fn ast(&self, node: NodeId) -> Result<tsr_ast::AstView<'_>, Error> {
        self.0.ast(node)
    }

    fn evaluate_entity(
        &mut self,
        expression: NodeId,
        location: Option<NodeId>,
    ) -> Result<tsr_ast::evaluator::EvaluationResult, Error> {
        let result = self.0.evaluate_enum_entity(expression, location)?;
        Ok(tsr_ast::evaluator::EvaluationResult::new(
            result.value.map(|value| match value {
                EnumValue::Number(number) => tsr_ast::evaluator::EvaluatedValue::Number(number),
                EnumValue::String(text) => tsr_ast::evaluator::EvaluatedValue::String(text),
            }),
            result.is_syntactically_string,
            result.resolved_other_files,
            result.has_external_references,
        ))
    }
}
