//! Template literal types (`getTemplateLiteralType` and the pattern-literal
//! predicates in `tsc/internal/checker/checker.go`). `NewChecker` creates the
//! `${number}` type through this path, so it is part of the first checker slice.

use crate::key::template_type_key;
use crate::{
    object_flags, type_flags, CheckerState, Error, LiteralValue, ObjectFlags, Payload,
    TemplateLiteralData, TypeId,
};
use std::sync::Arc;
use ts_ast::JsString;
use ts_jsstring::wtf8::combine_surrogate_pairs;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getTemplateLiteralType
    pub(crate) fn get_template_literal_type(
        &mut self,
        texts: &[JsString],
        types: &[TypeId],
    ) -> Result<TypeId, Error> {
        let mut union_index = None;
        for (index, t) in types.iter().enumerate() {
            if self.types.flags(*t)? & (type_flags::NEVER | type_flags::UNION) != 0 {
                union_index = Some(index);
                break;
            }
        }
        if let Some(union_index) = union_index {
            if !self.check_cross_product_union(types)? {
                return Ok(self.builtins.error_type);
            }
            let texts = texts.to_vec();
            let types = types.to_vec();
            return self
                .map_type(types[union_index], &mut |state, t| {
                    let mut replaced = types.clone();
                    replaced[union_index] = t;
                    state.get_template_literal_type(&texts, &replaced).map(Some)
                })?
                .ok_or(Error::MissingLink("template literal union map"));
        }
        if types.contains(&self.builtins.wildcard_type) {
            return Ok(self.builtins.wildcard_type);
        }
        let mut new_types: Vec<TypeId> = Vec::new();
        let mut new_texts: Vec<JsString> = Vec::new();
        let mut sb: Vec<u8> = Vec::new();
        sb.extend_from_slice(texts[0].as_bytes());
        if !self.add_template_spans(texts, types, &mut sb, &mut new_types, &mut new_texts)? {
            return Ok(self.builtins.string_type);
        }
        if new_types.is_empty() {
            let text = combine_surrogate_pairs(&sb).into_owned();
            return self.get_string_literal_type(JsString::from_bytes(text));
        }
        new_texts.push(JsString::from_bytes(
            combine_surrogate_pairs(&sb).into_owned(),
        ));
        if new_texts.iter().all(JsString::is_empty) {
            let mut all_string = true;
            for t in &new_types {
                if self.types.flags(*t)? & type_flags::STRING == 0 {
                    all_string = false;
                    break;
                }
            }
            if all_string {
                return Ok(self.builtins.string_type);
            }
            // Normalize `${Mapping<xxx>}` into Mapping<xxx>
            if new_types.len() == 1 && self.is_pattern_literal_type(new_types[0])? {
                return Ok(new_types[0]);
            }
        }
        let key = template_type_key(&new_texts, &new_types);
        if let Some(t) = self.types.caches.template_literal_types.get(&key) {
            return Ok(*t);
        }
        let t = self.new_template_literal_type(Arc::from(new_texts), Arc::from(new_types))?;
        self.types.caches.template_literal_types.insert(key, t);
        Ok(t)
    }

    /// The `addSpans` closure of `getTemplateLiteralType`: literal spans fold
    /// into the text, nested templates flatten, placeholders stay as types.
    fn add_template_spans(
        &mut self,
        texts: &[JsString],
        types: &[TypeId],
        sb: &mut Vec<u8>,
        new_types: &mut Vec<TypeId>,
        new_texts: &mut Vec<JsString>,
    ) -> Result<bool, Error> {
        for (i, t) in types.iter().enumerate() {
            let flags = self.types.flags(*t)?;
            if flags & (type_flags::LITERAL | type_flags::NULL | type_flags::UNDEFINED) != 0 {
                sb.extend_from_slice(&self.get_template_string_for_type(*t)?);
                sb.extend_from_slice(texts[i + 1].as_bytes());
            } else if flags & type_flags::TEMPLATE_LITERAL != 0 {
                let (inner_texts, inner_types) = {
                    let data = self.types.template_literal(*t)?;
                    (data.texts.clone(), data.types.clone())
                };
                sb.extend_from_slice(inner_texts[0].as_bytes());
                if !self.add_template_spans(&inner_texts, &inner_types, sb, new_types, new_texts)? {
                    return Ok(false);
                }
                sb.extend_from_slice(texts[i + 1].as_bytes());
            } else if self.is_generic_index_type(*t)?
                || self.is_pattern_literal_placeholder_type(*t)?
            {
                new_types.push(*t);
                new_texts.push(JsString::from_bytes(
                    combine_surrogate_pairs(sb).into_owned(),
                ));
                sb.clear();
                sb.extend_from_slice(texts[i + 1].as_bytes());
            } else {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTemplateStringForType
    pub(crate) fn get_template_string_for_type(&self, t: TypeId) -> Result<Vec<u8>, Error> {
        let flags = self.types.flags(t)?;
        if flags
            & (type_flags::STRING_LITERAL
                | type_flags::NUMBER_LITERAL
                | type_flags::BOOLEAN_LITERAL
                | type_flags::BIG_INT_LITERAL)
            != 0
        {
            return literal_value_text(&self.types.literal(t)?.value);
        }
        if flags & type_flags::NULLABLE != 0 {
            return Ok(self.types.intrinsic(t)?.name.as_bytes().to_vec());
        }
        Ok(Vec::new())
    }

    // port: tsc/internal/checker/checker.go:Checker.newTemplateLiteralType
    pub(crate) fn new_template_literal_type(
        &mut self,
        texts: Arc<[JsString]>,
        types: Arc<[TypeId]>,
    ) -> Result<TypeId, Error> {
        self.types.new_type(
            type_flags::TEMPLATE_LITERAL,
            object_flags::NONE,
            Payload::TemplateLiteral(TemplateLiteralData {
                resolved_base_constraint: None,
                texts,
                types,
            }),
        )
    }

    /// Too complex a cross product is a diagnostic on the current node, which
    /// arrives with diagnostics (P2); the size check itself is complete.
    // port: tsc/internal/checker/checker.go:Checker.checkCrossProductUnion
    pub(crate) fn check_cross_product_union(&self, types: &[TypeId]) -> Result<bool, Error> {
        let size = self.get_cross_product_union_size(types)?;
        if size >= 100_000 {
            return Err(Error::Unsupported(
                "Expression_produces_a_union_type_that_is_too_complex_to_represent",
            ));
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.getCrossProductUnionSize
    pub(crate) fn get_cross_product_union_size(&self, types: &[TypeId]) -> Result<usize, Error> {
        let mut size: usize = 1;
        for t in types {
            let flags = self.types.flags(*t)?;
            if flags & type_flags::UNION != 0 {
                let n = self.types.types_of(*t)?.len();
                // Cap the result to avoid integer overflow when computing the cross product of many large unions.
                if n > 0 && size > usize::MAX / n {
                    return Ok(usize::MAX);
                }
                size *= n;
            } else if flags & type_flags::NEVER != 0 {
                return Ok(0);
            }
        }
        Ok(size)
    }

    /// An intersection of placeholders and object tags is a placeholder; so are
    /// `any`, `string`, `number`, `bigint` and pattern literal types.
    // port: tsc/internal/checker/checker.go:Checker.isPatternLiteralPlaceholderType
    pub(crate) fn is_pattern_literal_placeholder_type(&mut self, t: TypeId) -> Result<bool, Error> {
        let flags = self.types.flags(t)?;
        if flags & type_flags::INTERSECTION != 0 {
            let mut seen_placeholder = false;
            let constituents = self.types.types_of(t)?.to_vec();
            for s in constituents {
                let s_flags = self.types.flags(s)?;
                if s_flags & (type_flags::LITERAL | type_flags::NULLABLE) != 0
                    || self.is_pattern_literal_placeholder_type(s)?
                {
                    seen_placeholder = true;
                } else if s_flags & type_flags::OBJECT == 0 {
                    return Ok(false);
                }
            }
            return Ok(seen_placeholder);
        }
        Ok(flags
            & (type_flags::ANY | type_flags::STRING | type_flags::NUMBER | type_flags::BIG_INT)
            != 0
            || self.is_pattern_literal_type(t)?)
    }

    /// A template literal or string mapping type whose placeholders are all
    /// non-generic pattern literal placeholders.
    // port: tsc/internal/checker/checker.go:Checker.isPatternLiteralType
    pub(crate) fn is_pattern_literal_type(&mut self, t: TypeId) -> Result<bool, Error> {
        let flags = self.types.flags(t)?;
        if flags & type_flags::TEMPLATE_LITERAL != 0 {
            let types = self.types.template_literal(t)?.types.clone();
            for s in types.iter() {
                if !self.is_pattern_literal_placeholder_type(*s)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        if flags & type_flags::STRING_MAPPING != 0 {
            return self.is_pattern_literal_placeholder_type(self.types.target(t)?);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.isGenericIndexType
    pub(crate) fn is_generic_type(&mut self, t: TypeId) -> Result<bool, Error> {
        Ok(self.get_generic_object_flags(t)? & object_flags::IS_GENERIC_TYPE != 0)
    }

    pub(crate) fn is_generic_index_type(&mut self, t: TypeId) -> Result<bool, Error> {
        Ok(self.get_generic_object_flags(t)? & object_flags::IS_GENERIC_INDEX_TYPE != 0)
    }

    /// Computed lazily for unions, intersections and substitutions and cached in
    /// the object flags, as upstream does.
    // port: tsc/internal/checker/checker.go:Checker.getGenericObjectFlags
    pub(crate) fn get_generic_object_flags(&mut self, t: TypeId) -> Result<ObjectFlags, Error> {
        let record = *self.types.get(t)?;
        let mut combined_flags = object_flags::NONE;
        if record.flags & (type_flags::UNION_OR_INTERSECTION | type_flags::SUBSTITUTION) != 0 {
            if record.object_flags & object_flags::IS_GENERIC_TYPE_COMPUTED == 0 {
                if record.flags & type_flags::UNION_OR_INTERSECTION != 0 {
                    let constituents = self.types.types_of(t)?.to_vec();
                    for u in constituents {
                        combined_flags |= self.get_generic_object_flags(u)?;
                    }
                } else {
                    let data = *self.types.substitution(t)?;
                    combined_flags = self.get_generic_object_flags(data.base)?
                        | self.get_generic_object_flags(data.constraint)?;
                }
                self.types.get_mut(t)?.object_flags |=
                    object_flags::IS_GENERIC_TYPE_COMPUTED | combined_flags;
            }
            return Ok(self.types.object_flags(t)? & object_flags::IS_GENERIC_TYPE);
        }
        if record.flags & type_flags::INSTANTIABLE_NON_PRIMITIVE != 0
            || self.is_generic_mapped_type(t)?
            || self.is_generic_tuple_type(t)?
        {
            combined_flags |= object_flags::IS_GENERIC_OBJECT_TYPE;
        }
        if record.flags & (type_flags::INSTANTIABLE_NON_PRIMITIVE | type_flags::INDEX) != 0
            || self.is_generic_string_like_type(t)?
        {
            combined_flags |= object_flags::IS_GENERIC_INDEX_TYPE;
        }
        Ok(combined_flags)
    }

    // port: tsc/internal/checker/checker.go:Checker.isGenericTupleType
    pub(crate) fn is_generic_tuple_type(&self, t: TypeId) -> Result<bool, Error> {
        if !self.is_tuple_type(t)? {
            return Ok(false);
        }
        let target = self.types.target(t)?;
        Ok(self.types.tuple(target)?.combined_flags & crate::element_flags::VARIADIC != 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.isGenericStringLikeType
    pub(crate) fn is_generic_string_like_type(&mut self, t: TypeId) -> Result<bool, Error> {
        Ok(
            self.types.flags(t)? & (type_flags::TEMPLATE_LITERAL | type_flags::STRING_MAPPING) != 0
                && !self.is_pattern_literal_type(t)?,
        )
    }
}

/// `evaluator.AnyToString` for literal values.
// port: tsc/internal/evaluator/evaluator.go:AnyToString
pub(crate) fn literal_value_text(value: &LiteralValue) -> Result<Vec<u8>, Error> {
    Ok(match value {
        LiteralValue::String(text) => text.as_bytes().to_vec(),
        LiteralValue::Number(number) => number.to_string().into_bytes(),
        LiteralValue::Boolean(value) => {
            if *value {
                b"true".to_vec()
            } else {
                b"false".to_vec()
            }
        }
        LiteralValue::BigInt(value) => value.to_text(),
        LiteralValue::ComputedEnum => {
            return Err(Error::Unsupported("AnyToString: computed enum value"))
        }
    })
}
