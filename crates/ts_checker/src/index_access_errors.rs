//! Indexed expression diagnostics preserve the complete index type even when
//! property resolution visits its union constituents independently.
use crate::signatures::IndexInfo;
use crate::{
    access_flags as af, object_flags as of, type_flags as tf, CheckerState, Error, TypeId,
};
use ts_arena::NodeId;
use ts_ast::{symbol_flags as sf, JsString, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.errorIfWritingToReadonlyIndex
    pub(crate) fn error_writing_readonly_index(
        &mut self,
        info: Option<IndexInfo>,
        object: TypeId,
        node: Option<NodeId>,
    ) -> Result<(), Error> {
        if let (Some(info), Some(node)) = (info, node) {
            if info.is_readonly
                && (self.assignment_target_kind(node)?
                    != crate::flow_assignments::AssignmentKind::None
                    || self.access_is_delete_target(node)?)
            {
                let display = self.type_to_string(object, crate::type_display::DEFAULT_FLAGS)?;
                self.error_at(
                    Some(node),
                    d::Index_signature_in_type_0_only_permits_reading,
                    vec![display],
                )?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getPropertyTypeForIndexType
    pub(crate) fn missing_indexed_property(
        &mut self,
        node: NodeId,
        object: TypeId,
        index: TypeId,
        full_index: TypeId,
        name: Option<&JsString>,
        flags: u32,
    ) -> Result<Option<TypeId>, Error> {
        let strict = self
            .program()?
            .host
            .options()
            .strict_option_value(self.program()?.host.options().no_implicit_any);
        let index_flags = self.types.flags(index)?;
        if self.types.object_flags(object)? & of::OBJECT_LITERAL != 0 {
            if strict && index_flags & (tf::STRING_LITERAL | tf::NUMBER_LITERAL) != 0 {
                let value = self.index_literal_display(index)?;
                let object = self.type_to_string(object, crate::type_display::DEFAULT_FLAGS)?;
                self.error_at(
                    Some(node),
                    d::Property_0_does_not_exist_on_type_1,
                    vec![value, object],
                )?;
                return Ok(Some(self.builtins.undefined_type));
            } else if index_flags & (tf::NUMBER | tf::STRING) != 0 {
                self.resolve_type_members(object)?;
                let properties = self
                    .types
                    .structured(object)?
                    .properties
                    .clone()
                    .unwrap_or_default();
                let mut types = Vec::new();
                for property in properties.iter() {
                    types.push(self.get_type_of_symbol(*property)?);
                }
                types.push(self.builtins.undefined_type);
                return self.get_union_type(&types).map(Some);
            }
        }
        let index_node = self
            .index_access_node(Some(node))?
            .ok_or(Error::MissingLink("indexed diagnostic expression"))?;
        if self.types.get(object)?.symbol == Some(self.builtins.global_this_symbol) {
            if let Some(name) = name {
                let exports = self.symbol(self.builtins.global_this_symbol)?.exports();
                if let Some(property) = self.member_symbol(exports, name.as_bytes())? {
                    if self.symbol(property)?.flags() & sf::BLOCK_SCOPED != 0 {
                        let display =
                            self.type_to_string(object, crate::type_display::DEFAULT_FLAGS)?;
                        self.error_at(
                            Some(node),
                            d::Property_0_does_not_exist_on_type_1,
                            vec![name.clone(), display],
                        )?;
                        return Ok(None);
                    }
                }
            }
        }
        if !strict || flags & af::SUPPRESS_NO_IMPLICIT_ANY_ERROR != 0 {
            return Ok(None);
        }
        if let Some(name) = name {
            if self.index_type_has_static_property(name.as_bytes(), object)? {
                let display = self.type_to_string(object, crate::type_display::DEFAULT_FLAGS)?;
                let text = ts_scanner::get_text_of_node(self.ast(index_node)?, index_node)?;
                let mut suggested = display.as_bytes().to_vec();
                suggested.push(b'[');
                suggested.extend(text.as_bytes());
                suggested.push(b']');
                self.error_at(Some(node),d::Property_0_does_not_exist_on_type_1_Did_you_mean_to_access_the_static_member_2_instead,vec![name.clone(),display,JsString::from_bytes(suggested)])?;
                return Ok(None);
            }
        }
        for info in self.index_infos_of_type(object)? {
            if self.signatures.index_info(info)?.key_type == self.builtins.number_type {
                self.error_at(Some(index_node),d::Element_implicitly_has_an_any_type_because_index_expression_is_not_of_type_number,vec![])?;
                return Ok(None);
            }
        }
        if let Some(name) = name {
            let properties = self.get_properties_of_type(object)?;
            let mut names = Vec::new();
            for property in properties {
                if self.symbol(property)?.flags() & sf::VALUE != 0 {
                    names.push(self.symbol(property)?.name_to_owned());
                }
            }
            if let Some(suggestion) = ts_scanner::get_spelling_suggestion_for_strings(
                name.as_bytes(),
                names.iter().map(ts_jsstring::JsString::as_bytes),
            ) {
                let suggestion = JsString::from_bytes(suggestion);
                let display = self.type_to_string(object, crate::type_display::DEFAULT_FLAGS)?;
                self.error_at(
                    Some(index_node),
                    d::Property_0_does_not_exist_on_type_1_Did_you_mean_2,
                    vec![name.clone(), display, suggestion],
                )?;
                return Ok(None);
            }
        }
        if let Some(suggestion) = self.index_missing_method_suggestion(object, node, index)? {
            let display = self.type_to_string(object, crate::type_display::DEFAULT_FLAGS)?;
            self.error_at(Some(node),d::Element_implicitly_has_an_any_type_because_type_0_has_no_index_signature_Did_you_mean_to_call_1,vec![display,suggestion])?;
            return Ok(None);
        }
        let object_text = self.type_to_string(object, crate::type_display::DEFAULT_FLAGS)?;
        let child = if index_flags & tf::ENUM_LITERAL != 0 {
            let mut text = vec![b'['];
            text.extend(
                self.type_to_string(index, crate::type_display::DEFAULT_FLAGS)?
                    .as_bytes(),
            );
            text.push(b']');
            Some(self.diagnostic_for_node(
                Some(node),
                d::Property_0_does_not_exist_on_type_1,
                vec![JsString::from_bytes(text), object_text.clone()],
            )?)
        } else if index_flags & tf::UNIQUE_ES_SYMBOL != 0 {
            let symbol = self
                .types
                .get(index)?
                .symbol
                .ok_or(Error::MissingLink("unique symbol index symbol"))?;
            let qualified = self.fully_qualified_name(symbol, Some(node))?;
            let mut text = vec![b'['];
            text.extend_from_slice(qualified.as_bytes());
            text.push(b']');
            Some(self.diagnostic_for_node(
                Some(node),
                d::Property_0_does_not_exist_on_type_1,
                vec![JsString::from_bytes(text), object_text.clone()],
            )?)
        } else if index_flags & (tf::STRING_LITERAL | tf::NUMBER_LITERAL) != 0 {
            let value = self.index_literal_display(index)?;
            Some(self.diagnostic_for_node(
                Some(node),
                d::Property_0_does_not_exist_on_type_1,
                vec![value, object_text.clone()],
            )?)
        } else if index_flags & (tf::NUMBER | tf::STRING) != 0 {
            let index = self.type_to_string(index, crate::type_display::DEFAULT_FLAGS)?;
            Some(self.diagnostic_for_node(
                Some(node),
                d::No_index_signature_with_a_parameter_of_type_0_was_found_on_type_1,
                vec![index, object_text.clone()],
            )?)
        } else {
            None
        };
        let full = self.type_to_string(full_index, crate::type_display::DEFAULT_FLAGS)?;
        let message=d::Element_implicitly_has_an_any_type_because_expression_of_type_0_can_t_be_used_to_index_type_1;
        let diagnostic = match child {
            Some(child) => ts_ast::Diagnostic::chain(
                Some(std::sync::Arc::new(child)),
                message,
                vec![full, object_text],
            ),
            None => self.diagnostic_for_node(Some(node), message, vec![full, object_text])?,
        };
        self.add_diagnostic(diagnostic)?;
        Ok(None)
    }

    fn index_literal_display(&self, index: TypeId) -> Result<JsString, Error> {
        match self.types.literal(index)?.value.clone() {
            crate::LiteralValue::String(value) => Ok(value),
            crate::LiteralValue::Number(value) => {
                Ok(JsString::from_bytes(value.to_string().into_bytes()))
            }
            _ => Err(Error::MissingLink("string/number index literal")),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.typeHasStaticProperty
    pub(crate) fn index_type_has_static_property(
        &mut self,
        name: &[u8],
        object: TypeId,
    ) -> Result<bool, Error> {
        if let Some(symbol) = self.types.get(object)?.symbol {
            let ty = self.get_type_of_symbol(symbol)?;
            if let Some(property) = self.constituent_property(ty, name, false)? {
                if let Some(declaration) = self.symbol(property)?.value_declaration() {
                    return Ok(ts_ast::utilities::is_static(
                        self.ast(declaration)?,
                        declaration,
                    )?);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getSuggestionForNonexistentIndexSignature
    fn index_missing_method_suggestion(
        &mut self,
        object: TypeId,
        node: NodeId,
        index: TypeId,
    ) -> Result<Option<JsString>, Error> {
        let method = if self.assignment_target_kind(node)?
            == crate::flow_assignments::AssignmentKind::None
        {
            b"get".as_slice()
        } else {
            b"set".as_slice()
        };
        let Some(property) = self.constituent_property(object, method, false)? else {
            return Ok(None);
        };
        let ty = self.get_type_of_symbol(property)?;
        let signatures = self.signatures_of_type(ty, false)?;
        if signatures.len() != 1 {
            return Ok(None);
        }
        let signature = signatures[0];
        if self.min_argument_count(signature)? < 1 {
            return Ok(None);
        }
        let Some(parameter) = self.parameter_type_at(signature, 0)? else {
            return Ok(None);
        };
        if !self.is_type_related_to(index, parameter, crate::RelationKind::Assignable)? {
            return Ok(None);
        }
        let receiver = self
            .ast(node)?
            .node(node)?
            .expression()
            .ok_or(Error::MissingLink("indexed suggestion receiver"))?;
        let mut text = self.index_receiver_text(receiver)?.unwrap_or_default();
        if !text.is_empty() {
            text.push(b'.');
        }
        text.extend(method);
        Ok(Some(JsString::from_bytes(text)))
    }

    // port: tsc/internal/checker/utilities.go:tryGetPropertyAccessOrIdentifierToString
    fn index_receiver_text(&self, node: NodeId) -> Result<Option<Vec<u8>>, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.kind() == K::Identifier {
            return Ok(Some(self.ast(node)?.node_text(node)?.as_bytes().to_vec()));
        }
        if matches!(
            read.kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) {
            let expression = read
                .expression()
                .ok_or(Error::MissingLink("suggestion property expression"))?;
            let name = if read.kind() == K::PropertyAccessExpression {
                read.name()
                    .ok_or(Error::MissingLink("suggestion property name"))?
            } else {
                let name = read
                    .data_source()
                    .as_element_access_expression()
                    .and_then(|data| data.argument_expression())
                    .ok_or(Error::MissingLink("suggestion index"))?;
                if !ts_ast::utilities::is_property_name(&self.ast(name)?.node(name)?) {
                    return Ok(None);
                }
                name
            };
            if let Some(mut text) = self.index_receiver_text(expression)? {
                text.push(b'.');
                text.extend(self.index_property_name_node(name)?.as_bytes());
                return Ok(Some(text));
            }
        }
        if read.kind() == K::JsxNamespacedName {
            let data = read
                .data_source()
                .as_jsx_namespaced_name()
                .ok_or(Error::MissingLink("JSX namespace payload"))?;
            let namespace = data
                .namespace()
                .ok_or(Error::MissingLink("JSX namespace"))?;
            let name = data
                .name()
                .ok_or(Error::MissingLink("JSX namespace name"))?;
            let mut text = self
                .ast(namespace)?
                .node_text(namespace)?
                .as_bytes()
                .to_vec();
            text.push(b':');
            text.extend(self.ast(name)?.node_text(name)?.as_bytes());
            return Ok(Some(text));
        }
        Ok(None)
    }
    // port: tsc/internal/ast/utilities.go:GetPropertyNameForPropertyNameNode
    pub(crate) fn index_property_name_node(&self, node: NodeId) -> Result<JsString, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.kind() != K::ComputedPropertyName {
            return Ok(self.ast(node)?.node_text(node)?.into_js_string());
        }
        let expression = read
            .expression()
            .ok_or(Error::MissingLink("computed property expression"))?;
        let read = self.ast(expression)?.node(expression)?;
        if matches!(
            read.kind().known(),
            Some(
                K::StringLiteral
                    | K::NumericLiteral
                    | K::BigIntLiteral
                    | K::NoSubstitutionTemplateLiteral
            )
        ) {
            return Ok(self
                .ast(expression)?
                .node_text(expression)?
                .into_js_string());
        }
        if let Some(data) = read.data_source().as_prefix_unary_expression() {
            let operand = data
                .operand()
                .ok_or(Error::MissingLink("signed property operand"))?;
            if self.ast(operand)?.node(operand)?.kind() == K::NumericLiteral
                && matches!(data.operator().known(), Some(K::PlusToken | K::MinusToken))
            {
                let mut text = Vec::new();
                if data.operator() == K::MinusToken {
                    text.push(b'-');
                }
                text.extend(self.ast(operand)?.node_text(operand)?.as_bytes());
                return Ok(JsString::from_bytes(text));
            }
        }
        Ok(JsString::from_bytes(
            ts_ast::internal_symbol_names::MISSING.to_vec(),
        ))
    }
}
