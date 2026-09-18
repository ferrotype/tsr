//! Byte-preserving template construction and matching from the pinned checker.

use super::{
    flags, is_empty_anonymous_object_type, unsupported, Error, Graph, LiteralValue, Mode, Rc,
    Relater, Ternary, TypeCell, Weak, FALSE, RECURSION_BOTH, TRUE,
};

#[derive(Debug)]
pub struct TemplateParts {
    pub texts: Vec<Vec<u8>>,
    types: Vec<Weak<TypeCell>>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct TemplateKey {
    texts: Vec<Vec<u8>>,
    types: Vec<u32>,
}

impl TemplateParts {
    pub fn types(&self) -> Result<Vec<Rc<TypeCell>>, Error> {
        self.types
            .iter()
            .map(|t| t.upgrade().ok_or(Error::Released))
            .collect()
    }
}

impl TypeCell {
    pub fn set_template_parts(
        &self,
        texts: Vec<Vec<u8>>,
        types: &[Rc<TypeCell>],
    ) -> Result<(), Error> {
        if self.flags & flags::TEMPLATE_LITERAL == 0 || texts.len() != types.len() + 1 {
            return unsupported("invalid template literal metadata");
        }
        self.template_parts
            .set(TemplateParts {
                texts,
                types: types.iter().map(Rc::downgrade).collect(),
            })
            .map_err(|_| Error::Unsupported(Rc::from("template literal metadata already set")))
    }

    pub fn template_parts(&self) -> Option<&TemplateParts> {
        self.template_parts.get()
    }
}

impl Graph {
    // port: tsc/internal/checker/checker.go:Checker.getTemplateLiteralType
    pub fn template_literal(
        &self,
        texts: &[Vec<u8>],
        types: &[Rc<TypeCell>],
    ) -> Result<Rc<TypeCell>, Error> {
        if texts.len() != types.len() + 1 {
            return unsupported("invalid template literal spans");
        }
        if let Some(index) = types
            .iter()
            .position(|t| t.flags & (flags::NEVER | flags::UNION) != 0)
        {
            if types[index].flags & flags::NEVER != 0 {
                return self.intrinsic_cell(flags::NEVER);
            }
            // The pinned cross-product limit rejects before expanding.
            let mut product = 1usize;
            for ty in types {
                if ty.flags & flags::UNION != 0 {
                    product = product.checked_mul(ty.constituent_count()).ok_or_else(|| {
                        Error::Unsupported(Rc::from("template union cross-product limit"))
                    })?;
                    if product >= 100_000 {
                        return unsupported("template union cross-product limit");
                    }
                }
            }
            let mut results = Vec::new();
            for part in types[index].types()? {
                let mut replaced = types.to_vec();
                replaced[index] = part;
                results.push(self.template_literal(texts, &replaced)?);
            }
            return self.union(&results);
        }
        let mut normalized_types = Vec::new();
        let mut normalized_texts = Vec::new();
        let mut text = texts[0].clone();
        if !add_spans(
            texts,
            types,
            &mut text,
            &mut normalized_texts,
            &mut normalized_types,
        )? {
            return self.intrinsic_cell(flags::STRING);
        }
        if normalized_types.is_empty() {
            return Ok(self.string_literal(&text));
        }
        normalized_texts.push(ts_jsstring::wtf8::combine_surrogate_pairs(&text).into_owned());
        if normalized_texts.iter().all(Vec::is_empty)
            && normalized_types
                .iter()
                .all(|t| t.flags & flags::STRING != 0)
        {
            return self.intrinsic_cell(flags::STRING);
        }
        let key = TemplateKey {
            texts: normalized_texts.clone(),
            types: normalized_types.iter().map(|t| t.id).collect(),
        };
        if let Some(existing) = self
            .template_cache
            .borrow()
            .get(&key)
            .and_then(Weak::upgrade)
        {
            return Ok(existing);
        }
        let mut name = String::from("`");
        for (index, part) in normalized_texts.iter().enumerate() {
            name.push_str(
                &String::from_utf8(ts_jsstring::escape::escape_string(
                    part,
                    ts_jsstring::QuoteChar::Backtick,
                ))
                .expect("EscapeString escapes invalid UTF-8 and lone surrogates"),
            );
            if let Some(ty) = normalized_types.get(index) {
                name.push_str("${");
                name.push_str(&ty.name);
                name.push('}');
            }
        }
        name.push('`');
        let cell = self.allocate_full(
            flags::TEMPLATE_LITERAL,
            0,
            Rc::from(name),
            None,
            None,
            None,
            false,
            Vec::new(),
            false,
            None,
        );
        cell.set_template_parts(normalized_texts, &normalized_types)?;
        self.template_cache
            .borrow_mut()
            .insert(key, Rc::downgrade(&cell));
        Ok(cell)
    }
}

fn add_spans(
    texts: &[Vec<u8>],
    types: &[Rc<TypeCell>],
    text: &mut Vec<u8>,
    normalized_texts: &mut Vec<Vec<u8>>,
    normalized_types: &mut Vec<Rc<TypeCell>>,
) -> Result<bool, Error> {
    for (index, ty) in types.iter().enumerate() {
        if let Some(literal) = template_string(ty) {
            text.extend_from_slice(&literal);
        } else if ty.flags & flags::TEMPLATE_LITERAL != 0 {
            let data = ty
                .template_parts()
                .ok_or_else(|| Error::Unsupported(Rc::from("template literal payload missing")))?;
            text.extend_from_slice(&data.texts[0]);
            if !add_spans(
                &data.texts,
                &data.types()?,
                text,
                normalized_texts,
                normalized_types,
            )? {
                return Ok(false);
            }
        } else if ty.flags
            & (flags::ANY
                | flags::STRING
                | flags::NUMBER
                | flags::BIG_INT
                | flags::TYPE_PARAMETER
                | flags::INDEX
                | flags::INDEXED_ACCESS)
            != 0
        {
            normalized_types.push(ty.clone());
            normalized_texts.push(ts_jsstring::wtf8::combine_surrogate_pairs(text).into_owned());
            text.clear();
        } else {
            return Ok(false);
        }
        text.extend_from_slice(&texts[index + 1]);
    }
    Ok(true)
}

fn template_string(ty: &TypeCell) -> Option<Vec<u8>> {
    Some(match &ty.literal {
        Some(LiteralValue::String(value)) => value.clone(),
        Some(LiteralValue::Number(bits)) => ts_jsnum::Number::new(f64::from_bits(*bits))
            .to_string()
            .into_bytes(),
        Some(LiteralValue::Boolean(value)) => value.to_string().into_bytes(),
        Some(LiteralValue::BigInt { negative, digits }) => {
            let mut value = Vec::new();
            if *negative {
                value.push(b'-');
            }
            value.extend_from_slice(digits);
            value
        }
        None if ty.flags & flags::NULL != 0 => b"null".to_vec(),
        None if ty.flags & flags::UNDEFINED != 0 => b"undefined".to_vec(),
        _ => return None,
    })
}

impl Relater<'_> {
    pub(super) fn template_identity_related_to(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<Ternary, Error> {
        let source = source
            .template_parts()
            .ok_or_else(|| Error::Unsupported(Rc::from("template source metadata missing")))?;
        let target = target
            .template_parts()
            .ok_or_else(|| Error::Unsupported(Rc::from("template target metadata missing")))?;
        if source.texts != target.texts {
            return Ok(FALSE);
        }
        let mut result = TRUE;
        for (s, t) in source.types()?.iter().zip(target.types()?) {
            result &= self.is_related_to(s, &t, RECURSION_BOTH, false)?;
            if result == FALSE {
                break;
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/relater.go:Checker.isTypeMatchedByTemplateLiteralType
    pub(super) fn template_related_to(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<Ternary, Error> {
        let target_parts = target
            .template_parts()
            .ok_or_else(|| Error::Unsupported(Rc::from("template target metadata missing")))?;
        if source.flags & flags::TEMPLATE_LITERAL != 0 && self.mode == Mode::Comparable {
            let source_parts = source
                .template_parts()
                .ok_or_else(|| Error::Unsupported(Rc::from("template source metadata missing")))?;
            let source_start = &source_parts.texts[0];
            let target_start = &target_parts.texts[0];
            let source_end = source_parts.texts.last().expect("validated template span");
            let target_end = target_parts.texts.last().expect("validated template span");
            let start = source_start.len().min(target_start.len());
            let end = source_end.len().min(target_end.len());
            return Ok(
                if source_start[..start] == target_start[..start]
                    && source_end[source_end.len() - end..] == target_end[target_end.len() - end..]
                {
                    TRUE
                } else {
                    FALSE
                },
            );
        }
        let Some(inferences) = self.infer_template_types(source, target_parts)? else {
            return Ok(FALSE);
        };
        for (source, target) in inferences.iter().zip(target_parts.types()?) {
            if !self.valid_template_placeholder(source, &target)? {
                return Ok(FALSE);
            }
        }
        Ok(TRUE)
    }

    // port: tsc/internal/checker/relater.go:Checker.inferTypesFromTemplateLiteralType
    fn infer_template_types(
        &mut self,
        source: &Rc<TypeCell>,
        target: &TemplateParts,
    ) -> Result<Option<Vec<Rc<TypeCell>>>, Error> {
        if let Some(LiteralValue::String(bytes)) = &source.literal {
            return self.infer_literal_parts(std::slice::from_ref(bytes), &[], &target.texts);
        }
        let Some(parts) = source.template_parts() else {
            return Ok(None);
        };
        let types = parts.types()?;
        if parts.texts == target.texts {
            let mut inferred = Vec::new();
            for (source, target) in types.iter().zip(target.types()?) {
                if source.flags & (flags::TYPE_PARAMETER | flags::INDEXED_ACCESS) != 0
                    || target.flags & (flags::TYPE_PARAMETER | flags::INDEXED_ACCESS) != 0
                {
                    return unsupported("template base constraint of type parameter");
                }
                if self.checker.is_type_assignable_to(source, &target)?
                    || source.flags & (flags::ANY | flags::STRING_LIKE) != 0
                {
                    inferred.push(source.clone());
                } else {
                    inferred.push(self.graph().template_literal(
                        &[Vec::new(), Vec::new()],
                        std::slice::from_ref(source),
                    )?);
                }
            }
            Ok(Some(inferred))
        } else {
            self.infer_literal_parts(&parts.texts, &types, &target.texts)
        }
    }

    // port: tsc/internal/checker/relater.go:Checker.inferFromLiteralPartsToTemplateLiteral
    fn infer_literal_parts(
        &self,
        texts: &[Vec<u8>],
        types: &[Rc<TypeCell>],
        target: &[Vec<u8>],
    ) -> Result<Option<Vec<Rc<TypeCell>>>, Error> {
        let last = texts.len() - 1;
        let end = target.len() - 1;
        if last == 0 && texts[0].len() < target[0].len() + target[end].len()
            || !texts[0].starts_with(&target[0])
            || !texts[last].ends_with(&target[end])
        {
            return Ok(None);
        }
        let remaining_end = &texts[last][..texts[last].len() - target[end].len()];
        let source_text = |index: usize| {
            if index == last {
                remaining_end
            } else {
                &texts[index]
            }
        };
        let mut segment = 0;
        let mut position = target[0].len();
        let mut matches = Vec::new();
        for delimiter in &target[1..end] {
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
        reason = "source cursors delimit a single template inference segment"
    )]
    fn template_match_segment(
        &self,
        texts: &[Vec<u8>],
        types: &[Rc<TypeCell>],
        segment: usize,
        position: usize,
        end_segment: usize,
        end_position: usize,
        end_text: &[u8],
    ) -> Result<Rc<TypeCell>, Error> {
        if segment == end_segment {
            return Ok(self
                .graph()
                .string_literal(&end_text[position..end_position]));
        }
        let mut match_texts = Vec::with_capacity(end_segment - segment + 1);
        match_texts.push(texts[segment][position..].to_vec());
        match_texts.extend_from_slice(&texts[segment + 1..end_segment]);
        match_texts.push(end_text[..end_position].to_vec());
        self.graph()
            .template_literal(&match_texts, &types[segment..end_segment])
    }

    // port: tsc/internal/checker/relater.go:Checker.isValidTypeForTemplateLiteralPlaceholder
    fn valid_template_placeholder(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<bool, Error> {
        if target.flags & flags::INTERSECTION != 0 {
            for part in target.types()? {
                if !is_empty_anonymous_object_type(&part)
                    && !self.valid_template_placeholder(source, &part)?
                {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        if target.flags & flags::STRING != 0
            || self.is_related_to(source, target, RECURSION_BOTH, false)? != FALSE
        {
            return Ok(true);
        }
        if let Some(LiteralValue::String(bytes)) = &source.literal {
            if target.flags & flags::NUMBER != 0
                && !bytes.is_empty()
                && ts_jsnum::from_string(bytes).value().is_finite()
            {
                return Ok(true);
            }
            if target.flags & flags::BIG_INT != 0 && valid_bigint_string(bytes) {
                return Ok(true);
            }
            if target.flags & (flags::BOOLEAN_LITERAL | flags::NULLABLE) != 0
                && template_string(target).is_some_and(|text| text == *bytes)
            {
                return Ok(true);
            }
            if target.flags & flags::TEMPLATE_LITERAL != 0 {
                return Ok(self.template_related_to(source, target)? != FALSE);
            }
        }
        if let Some(parts) = source.template_parts() {
            if parts.texts.len() == 2 && parts.texts.iter().all(Vec::is_empty) {
                return Ok(self.is_related_to(
                    &parts.types()?[0],
                    target,
                    RECURSION_BOTH,
                    false,
                )? != FALSE);
            }
        }
        Ok(false)
    }
}

fn valid_bigint_string(bytes: &[u8]) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Checker;

    fn checker() -> Checker {
        let checker = Checker::new();
        let cells = [
            checker.graph.primitive(flags::STRING, "string"),
            checker.graph.primitive(flags::NUMBER, "number"),
            checker.graph.primitive(flags::NEVER, "never"),
        ];
        checker.register_intrinsics(&cells);
        checker
    }

    #[test]
    fn template_union_expansion_and_pattern_relation_are_lazy_interned() {
        let checker = checker();
        let graph = &checker.graph;
        let a = graph.string_literal(b"a");
        let b = graph.string_literal(b"b");
        let parts = graph.union(&[a, b]).unwrap();
        let spans = [b"prefix-".to_vec(), Vec::new()];
        let source = graph.template_literal(&spans, &[parts]).unwrap();
        let target = graph
            .template_literal(&spans, &[checker.intrinsic(flags::STRING).unwrap()])
            .unwrap();
        let before = graph.len();
        assert!(
            checker
                .check_type_related_to(&source, &target, Mode::Assignable, false)
                .unwrap()
                .1
        );
        assert_eq!(
            before,
            graph.len(),
            "placeholder inference reuses literal intern entries"
        );
        assert!(
            !checker
                .check_type_related_to(&target, &source, Mode::Assignable, false)
                .unwrap()
                .1
        );
    }

    #[test]
    fn adjacent_placeholders_consume_js_code_points() {
        let checker = checker();
        let graph = &checker.graph;
        let string = checker.intrinsic(flags::STRING).unwrap();
        let number = checker.intrinsic(flags::NUMBER).unwrap();
        // The first adjacent placeholder consumes the entire code point. The
        // remainder must be numeric, so splitting a sentinel or emoji fails.
        let pattern = graph
            .template_literal(
                &[b"x".to_vec(), Vec::new(), b"z".to_vec()],
                &[string, number],
            )
            .unwrap();
        let value = graph.string_literal("x😀42z".as_bytes());
        assert!(
            checker
                .check_type_related_to(&value, &pattern, Mode::Assignable, false)
                .unwrap()
                .1
        );
        let sentinel = graph.string_literal(&[b'x', 0xed, 0xa0, 0x80, b'4', b'2', b'z']);
        assert!(
            checker
                .check_type_related_to(&sentinel, &pattern, Mode::Assignable, false)
                .unwrap()
                .1
        );
    }

    #[test]
    fn template_edges_do_not_retain_the_graph() {
        let retained;
        {
            let checker = checker();
            retained = checker
                .graph
                .template_literal(
                    &[b"p".to_vec(), Vec::new()],
                    &[checker.intrinsic(flags::STRING).unwrap()],
                )
                .unwrap();
        }
        assert!(matches!(
            retained.template_parts().unwrap().types(),
            Err(Error::Released)
        ));
    }
}
