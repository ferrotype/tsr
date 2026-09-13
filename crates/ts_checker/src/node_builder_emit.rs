//! Request serialization into the transformer's retained output owner.
use super::NodeBuilder;
use crate::{type_flags as tf, Error, LiteralValue};
use ts_arena::NodeId;
use ts_ast::{symbol_flags as sf, Factory, FactoryMethods, JsString, SyntaxKind as K};

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/emitresolver.go:EmitResolver.CreateLateBoundIndexSignatures
    pub(crate) fn declaration_late_indexes(
        &mut self,
        container: NodeId,
    ) -> Result<Vec<NodeId>, Error> {
        let symbol = self
            .checker
            .raw_declaration_symbol(container)?
            .ok_or(Error::MissingLink("late index class symbol"))?;
        let static_type = self.checker.get_type_of_symbol(symbol)?;
        let statics = self.checker.index_infos_of_type(static_type)?;
        let members = self.checker.members_of_symbol(symbol)?;
        let index_symbol = self
            .checker
            .member_symbol(members, ts_ast::internal_symbol_names::INDEX)?;
        let instances = self.checker.index_infos_of_symbol(index_symbol, members)?;
        let mut result = Vec::new();
        for (is_static, indexes) in [(true, statics), (false, instances)] {
            for index in indexes {
                let info = self.checker.signatures.index_info(index)?.clone();
                if info.declaration.is_some()
                    || index == self.checker.builtins.any_base_type_index_info
                {
                    continue;
                }
                if let Some(components) = info.components.as_ref().filter(|v| !v.is_empty()) {
                    let mut serializable = self.enclosing.is_some();
                    for &component in components.iter() {
                        if !serializable {
                            break;
                        }
                        let Some(name) = self.checker.ast(component)?.node(component)?.name()
                        else {
                            serializable = false;
                            break;
                        };
                        let read = self.checker.ast(name)?.node(name)?;
                        if read.kind() != K::ComputedPropertyName {
                            serializable = false;
                            break;
                        }
                        let expression = read
                            .expression()
                            .ok_or(Error::MissingLink("computed index component"))?;
                        if !ts_ast::is_entity_name_expression(
                            self.checker.ast(expression)?,
                            expression,
                        )? || self
                            .checker
                            .emit_entity_visible_ex(
                                expression,
                                self.enclosing.expect("enclosing was checked"),
                                false,
                            )?
                            .accessibility
                            != ts_printer::emit_resolver::SymbolAccessibility::Accessible
                        {
                            serializable = false;
                            break;
                        }
                    }
                    if serializable {
                        for &component in components.iter() {
                            if let Some(name) = self.checker.late_name(component)? {
                                let ty = self.checker.late_name_type(name)?;
                                if self.checker.types.flags(ty)?
                                    & (tf::STRING_OR_NUMBER_LITERAL | tf::UNIQUE_ES_SYMBOL)
                                    != 0
                                {
                                    continue;
                                }
                            }
                            let read = self.checker.ast(component)?.node(component)?;
                            let name = read
                                .name()
                                .ok_or(Error::MissingLink("late index component name"))?;
                            let question = read.question_token(self.checker.ast(component)?)?;
                            let expression = self
                                .checker
                                .ast(name)?
                                .node(name)?
                                .expression()
                                .ok_or(Error::MissingLink("late index expression"))?;
                            let first = ts_ast::utilities_middle::get_first_identifier(
                                self.checker.ast(expression)?,
                                expression,
                            )?;
                            let text = self.checker.ast(first)?.node_text(first)?.into_js_string();
                            if let Some(symbol) = self.checker.resolve_name(
                                Some(first),
                                text.as_bytes(),
                                sf::VALUE | sf::EXPORT_VALUE,
                                None,
                                true,
                            )? {
                                self.track_symbol(symbol, sf::VALUE)?;
                            }
                            let mut modifiers = Vec::new();
                            if is_static {
                                modifiers.push(self.ast.new_modifier(K::StaticKeyword.into()));
                            }
                            if info.is_readonly {
                                modifiers.push(self.ast.new_modifier(K::ReadonlyKeyword.into()));
                            }
                            let modifiers = if modifiers.is_empty() {
                                None
                            } else {
                                Some(self.list(modifiers)?)
                            };
                            self.retain_source_node(name)?;
                            let symbol = self
                                .checker
                                .raw_declaration_symbol(component)?
                                .ok_or(Error::MissingLink("late component symbol"))?;
                            let ty = self.checker.get_type_of_symbol(symbol)?;
                            let ty = self.type_node(ty)?;
                            result.push(self.ast.new_property_declaration(
                                modifiers,
                                Some(name),
                                question,
                                Some(ty),
                                None,
                            ));
                        }
                        continue;
                    }
                }
                let mut node = self.index_signature_node(index)?;
                if is_static {
                    let data = self
                        .ast
                        .view()
                        .node(node)?
                        .data_source()
                        .as_index_signature_declaration()
                        .ok_or(ts_arena::Error::InvalidGraph)?
                        .to_owned();
                    let mut modifiers = vec![self.ast.new_modifier(K::StaticKeyword.into())];
                    if let Some(list) = data.modifiers {
                        modifiers.extend(
                            self.ast
                                .view()
                                .node_slice(self.ast.view().list(list)?.nodes())?
                                .iter()
                                .flatten(),
                        );
                    }
                    let modifiers = self.list(modifiers)?;
                    node = self.ast.update_index_signature_declaration(
                        node,
                        Some(modifiers),
                        data.parameters,
                        data.r#type,
                    );
                }
                result.push(node);
            }
        }
        Ok(result)
    }

    pub(super) fn retain_source_node(&mut self, node: NodeId) -> Result<(), Error> {
        let source =
            ts_ast::utilities::get_source_file_of_node(self.checker.ast(node)?, Some(node))?
                .ok_or(Error::MissingLink("serialization source owner"))?;
        let program = self.checker.program()?;
        for index in 0..program.host.source_file_count() {
            let file = program.host.source_file(index);
            if file.source() == source {
                self.ast.retain_completed(file);
                return Ok(());
            }
        }
        Err(ts_arena::Error::WrongOwner.into())
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.typeParametersToTypeParameterDeclarations
    pub(crate) fn declaration_type_parameters(
        &mut self,
        declaration: NodeId,
    ) -> Result<Vec<NodeId>, Error> {
        let Some(symbol) = self.checker.get_symbol_of_declaration(declaration)? else {
            return Ok(vec![]);
        };
        let target = self.checker.target_symbol(symbol)?;
        let flags = self.checker.symbol(target)?.flags();
        let parameters = if flags & (sf::CLASS | sf::INTERFACE | sf::ALIAS) != 0 {
            self.checker.get_local_type_parameters(symbol)?
        } else if flags & sf::FUNCTION != 0 {
            let declaration = self
                .checker
                .symbol(symbol)?
                .value_declaration()
                .ok_or(Error::MissingLink("function type parameters declaration"))?;
            self.checker.type_parameters_from_declaration(declaration)?
        } else {
            return Ok(vec![]);
        };
        parameters
            .iter()
            .map(|&ty| self.type_parameter_node(ty))
            .collect()
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.CreateLiteralConstValue
    pub(crate) fn declaration_literal_value(
        &mut self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let Some(symbol) = self.checker.get_symbol_of_declaration(node)? else {
            return Ok(None);
        };
        let ty = self.checker.get_type_of_symbol(symbol)?;
        let record = *self.checker.types.get(ty)?;
        if record.flags & tf::ENUM_LIKE != 0 {
            let symbol = record
                .symbol
                .ok_or(Error::MissingLink("enum literal symbol"))?;
            self.track_symbol(symbol, sf::VALUE)?;
            return self
                .symbol_expression_with_meaning(symbol, Some(node), sf::VALUE)
                .map(Some);
        }
        if ty == self.checker.builtins.true_type || ty == self.checker.builtins.false_type {
            return Ok(Some(
                self.ast.new_keyword_expression(
                    if ty == self.checker.builtins.true_type {
                        K::TrueKeyword
                    } else {
                        K::FalseKeyword
                    }
                    .into(),
                ),
            ));
        }
        if record.flags & tf::LITERAL == 0 {
            return Ok(None);
        }
        let value = self.checker.types.literal(ty)?.value.clone();
        Ok(Some(match value {
            LiteralValue::String(value) => self.ast.new_string_literal(value, 0),
            LiteralValue::Number(value) => {
                let text = value.to_string();
                let negative = value.value().abs() != value.value();
                let operand = if value.value().is_infinite() {
                    self.ast
                        .new_identifier(JsString::from_bytes(b"Infinity".as_slice()))
                } else if value.value().is_nan() {
                    self.ast
                        .new_identifier(JsString::from_bytes(b"NaN".as_slice()))
                } else {
                    self.ast.new_numeric_literal(
                        JsString::from_bytes(if negative {
                            &text.as_bytes()[1..]
                        } else {
                            text.as_bytes()
                        }),
                        0,
                    )
                };
                if negative && !value.value().is_nan() {
                    self.ast
                        .new_prefix_unary_expression(K::MinusToken.into(), Some(operand))
                } else {
                    operand
                }
            }
            LiteralValue::BigInt(value) => {
                let mut text = value.to_text();
                text.push(b'n');
                self.ast.new_big_int_literal(JsString::from_bytes(text), 0)
            }
            LiteralValue::Boolean(value) => self.ast.new_keyword_expression(
                if value {
                    K::TrueKeyword
                } else {
                    K::FalseKeyword
                }
                .into(),
            ),
            LiteralValue::ComputedEnum => {
                return Err(Error::MissingLink("computed enum literal serialization"))
            }
        }))
    }
}
