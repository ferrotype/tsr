//! Implicit-any and widening diagnostics run when native variable inference
//! reports them, rather than being inferred from the eventual displayed type.
use crate::{object_flags as of, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{symbol_flags as sf, JsString, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.reportCircularityError
    pub(crate) fn report_symbol_circularity(
        &mut self,
        symbol: ts_arena::SymbolId,
    ) -> Result<TypeId, Error> {
        if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
            let read = self.ast(declaration)?.node(declaration)?;
            if read.type_node().is_some() {
                let name = self.symbol_to_string(symbol)?;
                self.error_at(Some(declaration), ts_diagnostics::X_0_is_referenced_directly_or_indirectly_in_its_own_type_annotation, vec![name])?;
                return Ok(self.builtins.error_type);
            }
            if self
                .program()?
                .host
                .options()
                .strict_option_value(self.program()?.host.options().no_implicit_any)
                && (read.kind() != ts_ast::SyntaxKind::Parameter || read.initializer().is_some())
            {
                let name = self.symbol_to_string(symbol)?;
                self.error_at(Some(declaration), ts_diagnostics::X_0_implicitly_has_type_any_because_it_does_not_have_a_type_annotation_and_is_referenced_directly_or_indirectly_in_its_own_initializer, vec![name])?;
            }
        } else if self.symbol(symbol)?.flags() & ts_ast::symbol_flags::ALIAS != 0 {
            if let Some(declaration) = self.alias_declaration_or_none(symbol)? {
                let name = self.symbol_to_string(symbol)?;
                self.error_at(
                    Some(declaration),
                    ts_diagnostics::Circular_definition_of_import_alias_0,
                    vec![name],
                )?;
            }
        }
        Ok(self.builtins.any_type)
    }

    // port: tsc/internal/checker/checker.go:Checker.reportImplicitAny
    pub(crate) fn report_implicit_any(
        &mut self,
        declaration: NodeId,
        ty: TypeId,
    ) -> Result<(), Error> {
        let no_implicit = self
            .program()?
            .host
            .options()
            .strict_option_value(self.program()?.host.options().no_implicit_any);
        let widened = self.widened_type(ty)?;
        let display = self.type_to_string(widened, crate::type_display::DEFAULT_FLAGS)?;
        let read = self.ast(declaration)?.node(declaration)?;
        let kind = read.kind();
        let name = ts_ast::get_name_of_declaration(self.ast(declaration)?, Some(declaration))?;
        let spelling = ts_scanner::declaration_name_to_string(self.ast(declaration)?, name)?;
        let message = match kind.known() {
            Some(K::BinaryExpression | K::PropertyDeclaration | K::PropertySignature) => {
                if no_implicit {
                    d::Member_0_implicitly_has_an_1_type
                } else {
                    d::Member_0_implicitly_has_an_1_type_but_a_better_type_may_be_inferred_from_usage
                }
            }
            Some(K::Parameter) => {
                let rest = read
                    .data_source()
                    .as_parameter_declaration()
                    .ok_or(Error::MissingLink("implicit parameter"))?
                    .dot_dot_dot_token()
                    .is_some();
                let parent = read
                    .parent()
                    .ok_or(Error::MissingLink("implicit parameter parent"))?;
                if let Some(name) = name {
                    let read = self.ast(name)?.node(name)?;
                    if read.kind() == K::Identifier
                        && matches!(
                            self.ast(parent)?.node(parent)?.kind().known(),
                            Some(K::CallSignature | K::MethodSignature | K::FunctionType)
                        )
                    {
                        let parameters = self.source_list(
                            parent,
                            self.ast(parent)?.node(parent)?.parameter_list(),
                        )?;
                        if let Some(index) = parameters.iter().position(|&node| node == declaration)
                        {
                            let text = self.ast(name)?.node_text(name)?.into_js_string();
                            let keyword =
                                ts_scanner::identifier_to_keyword_kind(&ts_ast::IdentifierData {
                                    text: text.clone(),
                                });
                            if ts_ast::utilities::is_type_node_kind(keyword.into())
                                || self
                                    .resolve_name(
                                        Some(declaration),
                                        text.as_bytes(),
                                        sf::TYPE,
                                        None,
                                        true,
                                    )?
                                    .is_some()
                            {
                                let mut type_name = spelling.as_bytes().to_vec();
                                if rest {
                                    type_name.extend_from_slice(b"[]");
                                }
                                let diagnostic = self.diagnostic_for_node(
                                    Some(declaration),
                                    d::Parameter_has_a_name_but_no_type_Did_you_mean_0_Colon_1,
                                    vec![
                                        JsString::from_bytes(format!("arg{index}").into_bytes()),
                                        JsString::from_bytes(type_name),
                                    ],
                                )?;
                                return self.variable_error_or_suggestion(no_implicit, diagnostic);
                            }
                        }
                    }
                }
                if rest {
                    if no_implicit {
                        d::Rest_parameter_0_implicitly_has_an_any_type
                    } else {
                        d::Rest_parameter_0_implicitly_has_an_any_type_but_a_better_type_may_be_inferred_from_usage
                    }
                } else if no_implicit {
                    d::Parameter_0_implicitly_has_an_1_type
                } else {
                    d::Parameter_0_implicitly_has_an_1_type_but_a_better_type_may_be_inferred_from_usage
                }
            }
            Some(K::BindingElement) => {
                if !no_implicit {
                    return Ok(());
                }
                d::Binding_element_0_implicitly_has_an_1_type
            }
            Some(K::VariableDeclaration) => {
                if no_implicit {
                    d::Variable_0_implicitly_has_an_1_type
                } else {
                    d::Variable_0_implicitly_has_an_1_type_but_a_better_type_may_be_inferred_from_usage
                }
            }
            _ => {
                return Err(Error::Unsupported(
                    "reportImplicitAny: non-variable widening kind",
                ))
            }
        };
        let diagnostic =
            self.diagnostic_for_node(Some(declaration), message, vec![spelling, display])?;
        self.variable_error_or_suggestion(no_implicit, diagnostic)
    }

    pub(crate) fn variable_error_or_suggestion(
        &mut self,
        error: bool,
        mut diagnostic: ts_ast::Diagnostic,
    ) -> Result<(), Error> {
        if error {
            self.add_diagnostic(diagnostic)?;
        } else {
            diagnostic.category = d::Category::Suggestion as i32;
            self.add_suggestion_diagnostic(diagnostic)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.reportWideningErrorsInType
    pub(crate) fn report_widening_errors_in_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        let record = *self.types.get(ty)?;
        if record.object_flags & of::CONTAINS_WIDENING_TYPE == 0 {
            return Ok(false);
        }
        if record.flags & tf::UNION != 0 {
            let parts = self.types.compound_types(ty)?.clone();
            for &part in parts.iter() {
                if self.empty_object_type(part)? {
                    return Ok(true);
                }
            }
            for &part in parts.iter() {
                if self.report_widening_errors_in_type(part)? {
                    return Ok(true);
                }
            }
        } else if self.is_array_type(ty)? || self.is_tuple_type(ty)? {
            for &part in self.get_type_arguments(ty)?.clone().iter() {
                if self.report_widening_errors_in_type(part)? {
                    return Ok(true);
                }
            }
        } else if record.object_flags & of::OBJECT_LITERAL != 0 {
            let owner = record
                .symbol
                .map(|symbol| {
                    self.symbol(symbol)
                        .map(ts_ast::SymbolRef::value_declaration)
                })
                .transpose()?
                .flatten();
            let mut reported = false;
            for property in self.get_properties_of_type(ty)? {
                let property_type = self.get_type_of_symbol(property)?;
                if self.types.get(property_type)?.object_flags & of::CONTAINS_WIDENING_TYPE == 0 {
                    continue;
                }
                reported = self.report_widening_errors_in_type(property_type)?;
                if !reported {
                    for declaration in self
                        .symbol_declarations(property)?
                        .to_vec()
                        .into_iter()
                        .flatten()
                    {
                        let original = self
                            .get_symbol_of_declaration(declaration)?
                            .ok_or(Error::MissingLink("widening property symbol"))?;
                        if let Some(value) = self.symbol(original)?.value_declaration() {
                            if self.ast(value)?.node(value)?.parent() == owner {
                                let name = self.symbol_to_string(property)?;
                                let widened = self.widened_type(property_type)?;
                                let display = self
                                    .type_to_string(widened, crate::type_display::DEFAULT_FLAGS)?;
                                self.error_at(
                                    Some(declaration),
                                    d::Object_literal_s_property_0_implicitly_has_an_1_type,
                                    vec![name, display],
                                )?;
                                reported = true;
                                break;
                            }
                        }
                    }
                }
            }
            return Ok(reported);
        }
        Ok(false)
    }
}
