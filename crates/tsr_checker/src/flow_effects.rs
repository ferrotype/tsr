//! Assertion/predicate calls use explicit dotted names to avoid feeding a
//! transient flow type back into the signature being resolved.
use crate::{types::Map, CheckerState, Error, SignatureId, TypeId, TypePredicateKind};
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{check_flags as cf, node_flags as nf, symbol_flags as sf, SyntaxKind as K};

#[derive(Default)]
pub(crate) struct FlowEffects {
    pub(crate) signatures: Map<NodeId, Result<Option<SignatureId>, Error>>,
    pub(crate) explicit_symbols: crate::types::Set<SymbolId>,
}

fn required<T>(value: Option<T>, context: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(context))
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkCallExpression
    pub(crate) fn check_assertion_call_target(
        &mut self,
        call: NodeId,
        _signature: SignatureId,
    ) -> Result<(), Error> {
        let expression = required(self.node(call)?.expression(), "assertion callee")?;
        if !tsr_ast::is_dotted_name(self.ast(expression)?, expression)? {
            self.error_at(Some(expression), tsr_diagnostics::Assertions_require_the_call_target_to_be_an_identifier_or_qualified_name, vec![])?;
        } else if self.effects_signature(call)?.is_none() {
            let diagnostic = self.error_at(Some(expression), tsr_diagnostics::Assertions_require_every_name_in_the_call_target_to_be_declared_with_an_explicit_type_annotation, vec![])?;
            self.type_of_dotted_name_with_diagnostic(expression, diagnostic)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/flow.go:Checker.getExplicitThisType
    fn explicit_flow_this_type(&mut self, node: NodeId) -> Result<Option<TypeId>, Error> {
        let container = tsr_ast::get_this_container(self.ast(node)?, node, false, false)?;
        let read = self.node(container)?;
        if tsr_ast::utilities::is_function_like(Some(&read)) {
            let signature = self.signature_from_declaration(container)?;
            if let Some(parameter) = self.signatures.get(signature)?.this_parameter {
                return self.explicit_type_of_symbol(parameter);
            }
        }
        if let Some(class) = self.node(container)?.parent() {
            if matches!(
                self.node(class)?.kind().known(),
                Some(K::ClassDeclaration | K::ClassExpression)
            ) {
                let symbol = required(
                    self.get_symbol_of_declaration(class)?,
                    "explicit this class symbol",
                )?;
                if tsr_ast::utilities::is_static(self.ast(container)?, container)? {
                    return self.get_type_of_symbol(symbol).map(Some);
                }
                let ty = self.get_declared_type_of_symbol(symbol)?;
                return Ok(self.types.interface(ty)?.this_type);
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/flow.go:Checker.getEffectsSignature
    pub(crate) fn effects_signature(&mut self, node: NodeId) -> Result<Option<SignatureId>, Error> {
        if let Some(cached) = self.flow.effects.signatures.get(&node).cloned() {
            return cached;
        }
        // Upstream leaves `links.effectsSignature` nil while resolving, so a
        // reentrant request recomputes; `getExplicitTypeOfSymbol`'s resolving
        // set is what terminates the recursion.
        let result = self.effects_signature_worker(node);
        self.flow.effects.signatures.insert(node, result.clone());
        result
    }

    fn effects_signature_worker(&mut self, node: NodeId) -> Result<Option<SignatureId>, Error> {
        let read = self.node(node)?;
        let parent = read.parent();
        let binary_right = read
            .data_source()
            .as_binary_expression()
            .and_then(|data| data.right());
        let expression = if let Some(right) = binary_right {
            right
        } else {
            required(read.expression(), "effects callee")?
        };
        let optional = read.flags() & nf::OPTIONAL_CHAIN != 0;
        let expression_statement = match parent {
            Some(parent) => self.node(parent)?.kind() == K::ExpressionStatement,
            None => false,
        };
        let func_type = if binary_right.is_some() {
            let right_type = self.check_expression(expression)?;
            let right_type = self.check_non_null_type(right_type, expression)?;
            self.symbol_has_instance_method_of_object_type(right_type)?
        } else if expression_statement {
            self.type_of_dotted_name(expression)?
        } else if self.node(expression)?.kind() == K::SuperKeyword {
            None
        } else {
            let ty = self.check_expression(expression)?;
            let ty = if optional {
                self.optional_expression_type(ty, expression)?
            } else {
                ty
            };
            Some(self.check_non_null_type(ty, expression)?)
        };
        let apparent = match func_type {
            Some(ty) => self.apparent_type(ty)?,
            None => self.builtins.unknown_type,
        };
        let signatures = self.signatures_of_type(apparent, false)?;
        let signature = if signatures.len() == 1
            && self
                .signatures
                .get(signatures[0])?
                .type_parameters
                .as_ref()
                .is_none_or(|p| p.is_empty())
        {
            Some(signatures[0])
        } else {
            let mut effect = false;
            for &signature in &signatures {
                if self.has_predicate_or_never_return(signature)? {
                    effect = true;
                    break;
                }
            }
            if effect {
                Some(self.resolved_call_signature(node)?)
            } else {
                None
            }
        };
        if let Some(signature) = signature {
            if self.has_predicate_or_never_return(signature)? {
                return Ok(Some(signature));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/flow.go:Checker.hasTypePredicateOrNeverReturnType
    fn has_predicate_or_never_return(&mut self, signature: SignatureId) -> Result<bool, Error> {
        if self.type_predicate_of_signature(signature)?.is_some() {
            return Ok(true);
        }
        let Some(declaration) = self.signatures.get(signature)?.declaration else {
            return Ok(false);
        };
        let ty = self
            .return_type_from_annotation(declaration)?
            .unwrap_or(self.builtins.unknown_type);
        Ok(self.types.flags(ty)? & crate::type_flags::NEVER != 0)
    }

    // port: tsc/internal/checker/flow.go:Checker.getTypeOfDottedName
    fn type_of_dotted_name(&mut self, node: NodeId) -> Result<Option<TypeId>, Error> {
        self.type_of_dotted_name_with_diagnostic(node, None)
    }
    fn type_of_dotted_name_with_diagnostic(
        &mut self,
        node: NodeId,
        diagnostic: Option<usize>,
    ) -> Result<Option<TypeId>, Error> {
        let read = self.node(node)?;
        if read.flags() & nf::IN_WITH_STATEMENT != 0 {
            return Ok(None);
        }
        match read.kind().known() {
            Some(K::Identifier) => {
                let symbol = self.resolved_value_symbol(node)?;
                let record = self.symbol(symbol)?;
                let symbol = if record.flags() & sf::EXPORT_VALUE != 0 {
                    record.export_symbol().unwrap_or(symbol)
                } else {
                    symbol
                };
                self.explicit_type_of_symbol_with_diagnostic(symbol, diagnostic)
            }
            Some(K::ThisKeyword) => self.explicit_flow_this_type(node),
            Some(K::SuperKeyword) => self.check_super_expression(node).map(Some),
            Some(K::PropertyAccessExpression) => {
                let expression = required(read.expression(), "dotted expression")?;
                let name = required(read.name(), "dotted property")?;
                let Some(ty) = self.type_of_dotted_name_with_diagnostic(expression, diagnostic)?
                else {
                    return Ok(None);
                };
                let text = self.node_text(name)?.into_js_string();
                let name = if self.node(name)?.kind() == K::PrivateIdentifier {
                    let Some(symbol) = self.types.get(ty)?.symbol else {
                        return Ok(None);
                    };
                    tsr_binder::get_symbol_name_for_private_identifier(
                        &self.symbol(symbol)?,
                        text.as_bytes(),
                    )
                } else {
                    text
                };
                match self.constituent_property(ty, name.as_bytes(), false)? {
                    Some(symbol) => {
                        self.explicit_type_of_symbol_with_diagnostic(symbol, diagnostic)
                    }
                    None => Ok(None),
                }
            }
            Some(K::ParenthesizedExpression) => self.type_of_dotted_name_with_diagnostic(
                required(read.expression(), "dotted parenthesized expression")?,
                diagnostic,
            ),
            _ => Ok(None),
        }
    }

    // port: tsc/internal/checker/flow.go:Checker.getExplicitTypeOfSymbol
    fn explicit_type_of_symbol(&mut self, symbol: SymbolId) -> Result<Option<TypeId>, Error> {
        self.explicit_type_of_symbol_with_diagnostic(symbol, None)
    }
    fn explicit_type_of_symbol_with_diagnostic(
        &mut self,
        symbol: SymbolId,
        diagnostic: Option<usize>,
    ) -> Result<Option<TypeId>, Error> {
        let symbol = if tsr_ast::is_non_local_alias(
            Some(&self.symbol(symbol)?),
            sf::VALUE | sf::TYPE | sf::NAMESPACE,
        ) {
            self.resolve_alias(symbol)?
        } else {
            symbol
        };
        if !self.flow.effects.explicit_symbols.insert(symbol) {
            return Ok(None);
        }
        let result = (|| {
            let record = self.symbol(symbol)?;
            let flags = record.flags();
            if flags & (sf::FUNCTION | sf::METHOD | sf::CLASS | sf::VALUE_MODULE) != 0 {
                return self.get_type_of_symbol(symbol).map(Some);
            }
            if flags & (sf::VARIABLE | sf::PROPERTY) != 0 {
                if record.check_flags() & cf::MAPPED != 0 {
                    let origin = self
                        .mapped_symbol_links
                        .try_get(symbol)
                        .and_then(|links| links.synthetic_origin);
                    if let Some(origin) = origin {
                        if self
                            .explicit_type_of_symbol_with_diagnostic(origin, diagnostic)?
                            .is_some()
                        {
                            return self.get_type_of_symbol(symbol).map(Some);
                        }
                    }
                }
                if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
                    let read = self.node(declaration)?;
                    let explicit = if matches!(
                        read.kind().known(),
                        // port: tsc/internal/checker/flow.go:Checker.isDeclarationWithExplicitTypeAnnotation
                        Some(
                            K::VariableDeclaration
                                | K::PropertyDeclaration
                                | K::PropertySignature
                                | K::Parameter
                        )
                    ) {
                        read.type_node().is_some()
                    } else if let Some(binary) = read.data_source().as_binary_expression() {
                        // port: tsc/internal/checker/flow.go:Checker.isExpandoPropertyFunctionWithReturnTypeAnnotation
                        let right = required(binary.right(), "expando function")?;
                        let right = self.node(right)?;
                        tsr_ast::utilities::is_function_like(Some(&right))
                            && right.type_node().is_some()
                    } else {
                        false
                    };
                    if explicit {
                        return self.get_type_of_symbol(symbol).map(Some);
                    }
                    if read.kind() == K::VariableDeclaration {
                        let list = required(read.parent(), "explicit declaration list")?;
                        let statement =
                            required(self.node(list)?.parent(), "explicit declaration statement")?;
                        if self.node(statement)?.kind() == K::ForOfStatement {
                            let read = self.node(statement)?;
                            let data = read
                                .data_source()
                                .as_for_in_or_of_statement()
                                .ok_or(tsr_arena::Error::InvalidGraph)?;
                            let expression =
                                required(data.expression(), "explicit for-of expression")?;
                            let use_ = crate::iteration::FOR_OF
                                | if data.await_modifier().is_some() {
                                    crate::iteration::ALLOW_ASYNC
                                } else {
                                    0
                                };
                            if let Some(expression_type) = self.type_of_dotted_name(expression)? {
                                return self
                                    .check_iterated_type_or_element_type(
                                        use_,
                                        expression_type,
                                        self.builtins.undefined_type,
                                        None,
                                    )
                                    .map(Some);
                            }
                        }
                    }
                    if let Some(index) = diagnostic {
                        let name = self.symbol_to_string(symbol)?;
                        let related = self.diagnostic_for_node(
                            Some(declaration),
                            tsr_diagnostics::X_0_needs_an_explicit_type_annotation,
                            vec![name],
                        )?;
                        self.add_related_diagnostic(index, related)?;
                    }
                }
            }
            Ok(None)
        })();
        self.flow.effects.explicit_symbols.remove(&symbol);
        result
    }

    // port: tsc/internal/checker/flow.go:Checker.isFalseExpression
    pub(crate) fn false_flow_expression(&self, expression: NodeId) -> Result<bool, Error> {
        let node = tsr_ast::skip_parentheses(self.ast(expression)?, expression)?;
        let read = self.node(node)?;
        if read.kind() == K::FalseKeyword {
            return Ok(true);
        }
        let Some(binary) = read.data_source().as_binary_expression() else {
            return Ok(false);
        };
        let left = required(binary.left(), "false binary left")?;
        let right = required(binary.right(), "false binary right")?;
        let operator = self
            .ast(node)?
            .node(required(binary.operator_token(), "false binary operator")?)?
            .kind();
        Ok(operator == K::AmpersandAmpersandToken
            && (self.false_flow_expression(left)? || self.false_flow_expression(right)?)
            || operator == K::BarBarToken
                && self.false_flow_expression(left)?
                && self.false_flow_expression(right)?)
    }

    pub(crate) fn flow_call_is_never(&mut self, call: NodeId) -> Result<bool, Error> {
        let Some(signature) = self.effects_signature(call)? else {
            return Ok(false);
        };
        if let Some(predicate) = self.type_predicate_of_signature(signature)? {
            let data = self.signatures.predicate(predicate)?;
            if data.kind == TypePredicateKind::AssertsIdentifier
                && data.t.is_none()
                && data.parameter_index >= 0
            {
                let index = data.parameter_index as usize;
                let arguments = self.source_list(call, self.node(call)?.argument_list())?;
                if let Some(&argument) = arguments.get(index) {
                    if self.false_flow_expression(argument)? {
                        return Ok(true);
                    }
                }
            }
        }
        let ty = self.return_type_of_signature(signature)?;
        Ok(self.types.flags(ty)? & crate::type_flags::NEVER != 0)
    }
}
