//! Source template literal types and the intrinsic string transformations.
//! Casing uses S04's pinned JavaScript byte semantics, including surrogates.

use crate::{object_flags as of, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::JsString;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getTypeFromTemplateTypeNode
    pub(crate) fn source_template_type(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let (head, spans) = self.template_source_parts(node)?;
        let mut texts = vec![self.node_text(head)?.into_js_string()];
        let mut types = Vec::new();
        for span in spans {
            let read = self.node(span)?;
            let data = read
                .data_source()
                .as_template_literal_type_span()
                .ok_or(Error::MissingLink("template span"))?;
            let literal = data
                .literal()
                .ok_or(Error::MissingLink("template span literal"))?;
            let annotation = read
                .type_node()
                .ok_or(Error::MissingLink("template span type"))?;
            texts.push(self.node_text(literal)?.into_js_string());
            types.push(self.get_type_from_type_node(annotation)?);
        }
        self.get_template_literal_type(&texts, &types)
    }

    fn template_source_parts(&self, node: NodeId) -> Result<(NodeId, Vec<NodeId>), Error> {
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_template_literal_type_node()
            .ok_or(Error::MissingLink("template literal type"))?;
        Ok((
            data.head().ok_or(Error::MissingLink("template head"))?,
            self.source_list(node, data.template_spans())?,
        ))
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTemplateLiteralType
    pub(crate) fn check_template_type(&mut self, node: NodeId) -> Result<(), Error> {
        for span in self.template_source_parts(node)?.1 {
            let annotation = self
                .ast(span)?
                .node(span)?
                .type_node()
                .ok_or(Error::MissingLink("template span annotation"))?;
            self.check_source_element(annotation)?;
            let ty = self.get_type_from_type_node(annotation)?;
            self.check_assignable_at(ty, self.builtins.template_constraint_type, annotation)?;
        }
        self.get_type_from_type_node(node)?;
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getStringMappingType
    pub(crate) fn get_string_mapping_type(
        &mut self,
        symbol: SymbolId,
        ty: TypeId,
    ) -> Result<TypeId, Error> {
        let record = *self.types.get(ty)?;
        if record.flags & (tf::UNION | tf::NEVER) != 0 {
            return self
                .map_type(ty, &mut |checker, ty| {
                    checker.get_string_mapping_type(symbol, ty).map(Some)
                })?
                .ok_or(Error::MissingLink("string mapping result"));
        }
        if record.flags & tf::STRING_LITERAL != 0 {
            let crate::LiteralValue::String(text) = &self.types.literal(ty)?.value else {
                return Err(Error::MissingLink("string mapping literal"));
            };
            let text = self.apply_string_mapping(symbol, text)?;
            return self.get_string_literal_type(text);
        }
        if record.flags & tf::TEMPLATE_LITERAL != 0 {
            let data = self.types.template_literal(ty)?;
            let mut texts = data.texts.to_vec();
            let mut types = data.types.to_vec();
            match self.symbol(symbol)?.name_bytes() {
                b"Uppercase" | b"Lowercase" => {
                    for text in &mut texts {
                        *text = self.apply_string_mapping(symbol, text)?;
                    }
                    for ty in &mut types {
                        *ty = self.get_string_mapping_type(symbol, *ty)?;
                    }
                }
                b"Capitalize" | b"Uncapitalize" => {
                    if texts[0].is_empty() {
                        types[0] = self.get_string_mapping_type(symbol, types[0])?;
                    } else {
                        texts[0] = self.apply_string_mapping(symbol, &texts[0])?;
                    }
                }
                _ => {}
            }
            return self.get_template_literal_type(&texts, &types);
        }
        if record.flags & tf::STRING_MAPPING != 0 && record.symbol == Some(symbol) {
            return Ok(ty);
        }
        if record.flags & (tf::ANY | tf::STRING | tf::STRING_MAPPING) != 0
            || self.is_generic_index_type(ty)?
        {
            return self.generic_string_mapping(symbol, ty);
        }
        if self.is_pattern_literal_placeholder_type(ty)? {
            let template =
                self.get_template_literal_type(&[JsString::default(), JsString::default()], &[ty])?;
            return self.generic_string_mapping(symbol, template);
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getStringMappingTypeForGenericType
    fn generic_string_mapping(&mut self, symbol: SymbolId, ty: TypeId) -> Result<TypeId, Error> {
        if let Some(&cached) = self.types.caches.string_mapping_types.get(&(symbol, ty)) {
            return Ok(cached);
        }
        let result = self.types.new_type(
            tf::STRING_MAPPING,
            of::NONE,
            crate::types::Payload::StringMapping(crate::types::StringMappingData {
                target: ty,
                resolved_base_constraint: None,
            }),
        )?;
        self.types.get_mut(result)?.symbol = Some(symbol);
        self.types
            .caches
            .string_mapping_types
            .insert((symbol, ty), result);
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:applyStringMapping
    fn apply_string_mapping(&self, symbol: SymbolId, text: &JsString) -> Result<JsString, Error> {
        use ts_jsstring::helpers::{to_lower_js, to_upper_js};
        let bytes = text.as_bytes();
        let (uppercase, first_only) = match self.symbol(symbol)?.name_bytes() {
            b"Uppercase" => (true, false),
            b"Lowercase" => (false, false),
            b"Capitalize" => (true, true),
            b"Uncapitalize" => (false, true),
            _ => return Ok(text.clone()),
        };
        let split = if first_only {
            ts_jsstring::wtf8::decode_rune(bytes).1
        } else {
            bytes.len()
        };
        let mut mapped = if uppercase {
            to_upper_js(&bytes[..split]).into_owned()
        } else {
            to_lower_js(&bytes[..split]).into_owned()
        };
        mapped.extend_from_slice(&bytes[split..]);
        Ok(JsString::from_bytes(mapped))
    }
}
