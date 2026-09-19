//! Template matching operates on pinned JavaScript string bytes, including
//! lone surrogates. Adjacent placeholders consume code points, as Go does.
use crate::{
    relater::{Relater, RelationKind},
    ternary as tr, type_flags as tf, CheckerState, Error, LiteralValue, TypeId,
};
use ts_ast::JsString;

impl CheckerState {
    // port: tsc/internal/checker/relater.go:Checker.inferTypesFromTemplateLiteralType
    pub(crate) fn infer_template_types(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Option<Vec<TypeId>>, Error> {
        let target_data = self.types.template_literal(target)?;
        let target_texts = target_data.texts.clone();
        let target_types = target_data.types.clone();
        let flags = self.types.flags(source)?;
        if flags & tf::STRING_LITERAL != 0 {
            let LiteralValue::String(text) = &self.types.literal(source)?.value else {
                return Err(Error::MissingLink("string literal payload"));
            };
            let texts = [text.clone()];
            return self.infer_literal_parts(&texts, &[], &target_texts);
        }
        if flags & tf::TEMPLATE_LITERAL == 0 {
            return Ok(None);
        }
        let source_data = self.types.template_literal(source)?;
        let texts = source_data.texts.clone();
        let types = source_data.types.clone();
        if texts != target_texts {
            return self.infer_literal_parts(&texts, &types, &target_texts);
        }
        let mut result = Vec::with_capacity(types.len());
        for (&source, &target) in types.iter().zip(target_types.iter()) {
            let source_constraint = self.base_constraint_of_type(source)?.unwrap_or(source);
            let target_constraint = self.base_constraint_of_type(target)?.unwrap_or(target);
            if self.is_type_related_to(
                source_constraint,
                target_constraint,
                RelationKind::Assignable,
            )? || self.types.flags(source)? & (tf::ANY | tf::STRING_LIKE) != 0
            {
                result.push(source);
            } else {
                result.push(self.get_template_literal_type(
                    &[JsString::default(), JsString::default()],
                    &[source],
                )?);
            }
        }
        Ok(Some(result))
    }

    // port: tsc/internal/checker/relater.go:Checker.inferFromLiteralPartsToTemplateLiteral
    fn infer_literal_parts(
        &mut self,
        texts: &[JsString],
        types: &[TypeId],
        target: &[JsString],
    ) -> Result<Option<Vec<TypeId>>, Error> {
        let last = texts.len() - 1;
        let end = target.len() - 1;
        if last == 0
            && texts[0].as_bytes().len() < target[0].as_bytes().len() + target[end].as_bytes().len()
            || !texts[0].as_bytes().starts_with(target[0].as_bytes())
            || !texts[last].as_bytes().ends_with(target[end].as_bytes())
        {
            return Ok(None);
        }
        let remaining_end =
            &texts[last].as_bytes()[..texts[last].as_bytes().len() - target[end].as_bytes().len()];
        let source_text = |index: usize| {
            if index == last {
                remaining_end
            } else {
                texts[index].as_bytes()
            }
        };
        let mut segment = 0;
        let mut position = target[0].as_bytes().len();
        let mut matches = Vec::new();
        for delimiter in &target[1..end] {
            let delimiter = delimiter.as_bytes();
            let (s, p) = if !delimiter.is_empty() {
                let mut s = segment;
                let mut p = position;
                loop {
                    if let Some(found) = source_text(s)[p..]
                        .windows(delimiter.len())
                        .position(|bytes| bytes == delimiter)
                    {
                        break (s, p + found);
                    }
                    s += 1;
                    if s == texts.len() {
                        return Ok(None);
                    }
                    p = 0;
                }
            } else if position < source_text(segment).len() {
                (
                    segment,
                    position + ts_jsstring::wtf8::decode_rune(&source_text(segment)[position..]).1,
                )
            } else if segment < last {
                (segment + 1, 0)
            } else {
                return Ok(None);
            };
            matches.push(self.template_match_segment(
                texts,
                types,
                segment,
                position,
                s,
                p,
                source_text(s),
            )?);
            segment = s;
            position = p + delimiter.len();
        }
        matches.push(self.template_match_segment(
            texts,
            types,
            segment,
            position,
            last,
            remaining_end.len(),
            remaining_end,
        )?);
        Ok(Some(matches))
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "source text/type cursors delimit one inferred template placeholder"
    )]
    fn template_match_segment(
        &mut self,
        texts: &[JsString],
        types: &[TypeId],
        segment: usize,
        position: usize,
        end_segment: usize,
        end_position: usize,
        end_text: &[u8],
    ) -> Result<TypeId, Error> {
        if segment == end_segment {
            return self.get_string_literal_type(JsString::from_bytes(
                ts_jsstring::wtf8::combine_surrogate_pairs(&end_text[position..end_position])
                    .as_ref(),
            ));
        }
        let mut match_texts = Vec::with_capacity(end_segment - segment + 1);
        match_texts.push(JsString::from_bytes(&texts[segment].as_bytes()[position..]));
        match_texts.extend_from_slice(&texts[segment + 1..end_segment]);
        match_texts.push(JsString::from_bytes(&end_text[..end_position]));
        self.get_template_literal_type(&match_texts, &types[segment..end_segment])
    }

    // port: tsc/internal/checker/relater.go:Checker.isMemberOfStringMapping
    pub(crate) fn member_of_string_mapping(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<bool, Error> {
        let flags = self.types.flags(target)?;
        if flags & tf::ANY != 0 {
            return Ok(true);
        }
        if flags & (tf::STRING | tf::TEMPLATE_LITERAL) != 0 {
            return self.is_type_related_to(source, target, RelationKind::Assignable);
        }
        if flags & tf::STRING_MAPPING != 0 {
            let (mapped, inner) = self.apply_target_mapping(source, target)?;
            return Ok(mapped == source && self.member_of_string_mapping(source, inner)?);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/relater.go:Checker.applyTargetStringMappingToSource
    fn apply_target_mapping(
        &mut self,
        mut source: TypeId,
        target: TypeId,
    ) -> Result<(TypeId, TypeId), Error> {
        let mut inner = self.types.target(target)?;
        if self.types.flags(inner)? & tf::STRING_MAPPING != 0 {
            (source, inner) = self.apply_target_mapping(source, inner)?;
        }
        let symbol = self
            .types
            .get(target)?
            .symbol
            .ok_or(Error::MissingLink("mapping intrinsic symbol"))?;
        Ok((self.get_string_mapping_type(symbol, source)?, inner))
    }
}

impl Relater<'_> {
    // port: tsc/internal/checker/relater.go:Checker.isTypeMatchedByTemplateLiteralType
    pub(crate) fn template_related(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<crate::Ternary, Error> {
        if self.kind == RelationKind::Comparable
            && self.checker.types.flags(source)? & tf::TEMPLATE_LITERAL != 0
        {
            let source = &self.checker.types.template_literal(source)?.texts;
            let target = &self.checker.types.template_literal(target)?.texts;
            let (s, t) = (source[0].as_bytes(), target[0].as_bytes());
            let start = s.len().min(t.len());
            if s[..start] != t[..start] {
                return Ok(tr::FALSE);
            }
            let (s, t) = (
                source[source.len() - 1].as_bytes(),
                target[target.len() - 1].as_bytes(),
            );
            let end = s.len().min(t.len());
            return Ok(if s[s.len() - end..] == t[t.len() - end..] {
                tr::TRUE
            } else {
                tr::FALSE
            });
        }
        if self.checker.types.flags(source)? & tf::TEMPLATE_LITERAL != 0 {
            self.checker.report_unreliable_markers(source, false)?;
        }
        Ok(
            if self
                .checker
                .template_types_match(source, target, Some(self.frame))?
            {
                tr::TRUE
            } else {
                tr::FALSE
            },
        )
    }
}

impl CheckerState {
    pub(crate) fn template_types_match(
        &mut self,
        source: TypeId,
        target: TypeId,
        comparer: Option<crate::RelationFrameId>,
    ) -> Result<bool, Error> {
        let Some(types) = self.infer_template_types(source, target)? else {
            return Ok(false);
        };
        let targets = self.types.template_literal(target)?.types.clone();
        for (&source, &target) in types.iter().zip(targets.iter()) {
            if !self.valid_template_placeholder(source, target, comparer)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn compare_template_placeholder(
        &mut self,
        source: TypeId,
        target: TypeId,
        comparer: Option<crate::RelationFrameId>,
    ) -> Result<bool, Error> {
        match comparer {
            Some(frame) => self
                .compare_in_relation_frame(frame, source, target)
                .map(|r| r != tr::FALSE),
            None => self.is_type_related_to(source, target, RelationKind::Assignable),
        }
    }

    // port: tsc/internal/checker/relater.go:Checker.isValidTypeForTemplateLiteralPlaceholder
    fn valid_template_placeholder(
        &mut self,
        source: TypeId,
        target: TypeId,
        comparer: Option<crate::RelationFrameId>,
    ) -> Result<bool, Error> {
        let t = self.types.flags(target)?;
        if t & tf::INTERSECTION != 0 {
            let parts = self.types.compound_types(target)?.clone();
            for &part in parts.iter() {
                if part != self.builtins.empty_type_literal_type
                    && !self.valid_template_placeholder(source, part, comparer)?
                {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        if t & tf::STRING != 0 || self.compare_template_placeholder(source, target, comparer)? {
            return Ok(true);
        }
        let s = self.types.flags(source)?;
        if s & tf::STRING_LITERAL != 0 {
            let LiteralValue::String(value) = &self.types.literal(source)?.value else {
                return Err(Error::MissingLink("placeholder literal"));
            };
            let bytes = value.as_bytes();
            if t & tf::NUMBER != 0
                && !bytes.is_empty()
                && ts_jsnum::from_string(bytes).value().is_finite()
            {
                return Ok(true);
            }
            if t & tf::BIG_INT != 0 && valid_bigint_string(bytes) {
                return Ok(true);
            }
            if t & (tf::BOOLEAN_LITERAL | tf::NULLABLE) != 0 {
                let expected: &[u8] = if t & tf::BOOLEAN_LITERAL != 0 {
                    match self.types.literal(target)?.value {
                        LiteralValue::Boolean(true) => b"true".as_slice(),
                        LiteralValue::Boolean(false) => b"false".as_slice(),
                        _ => return Err(Error::MissingLink("boolean literal")),
                    }
                } else if t & tf::NULL != 0 {
                    b"null"
                } else {
                    b"undefined"
                };
                if bytes == expected {
                    return Ok(true);
                }
            }
            if t & tf::STRING_MAPPING != 0 {
                return self.member_of_string_mapping(source, target);
            }
            if t & tf::TEMPLATE_LITERAL != 0 {
                return self.template_types_match(source, target, comparer);
            }
        }
        if s & tf::TEMPLATE_LITERAL != 0 {
            let data = self.types.template_literal(source)?;
            if data.texts.len() == 2 && data.texts.iter().all(|text| text.as_bytes().is_empty()) {
                return self.compare_template_placeholder(data.types[0], target, comparer);
            }
        }
        Ok(false)
    }
}

// port: tsc/internal/checker/utilities.go:isValidBigIntString
pub(crate) fn valid_bigint_string(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut text = bytes.to_vec();
    text.push(b'n');
    let mut scanner = ts_scanner::Scanner::new();
    scanner.set_skip_trivia(false);
    scanner.buffer_diagnostics();
    scanner.set_text(&text);
    let mut kind = scanner.scan();
    if kind == ts_ast::SyntaxKind::MinusToken {
        kind = scanner.scan();
    }
    let success = scanner.drain_diagnostics().next().is_none();
    success
        && kind == ts_ast::SyntaxKind::BigIntLiteral
        && scanner.token_end() == text.len() as i64
        && scanner.token_flags() & ts_ast::token_flags::CONTAINS_SEPARATOR == 0
}
