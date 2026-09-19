use crate::{
    infer_types::InferenceRun, type_flags as tf, CheckerState, Error, LiteralValue, TypeId,
};

impl CheckerState {
    // port: tsc/internal/checker/inference.go:Checker.inferToTemplateLiteralType
    pub(crate) fn infer_to_template(
        &mut self,
        run: &mut InferenceRun,
        source: TypeId,
        target: TypeId,
    ) -> Result<(), Error> {
        let matches = self.infer_template_types(source, target)?;
        let data = self.types.template_literal(target)?;
        if matches.is_none() && !data.texts.iter().all(|s| s.as_bytes().is_empty()) {
            return Ok(());
        }
        let targets = data.types.clone();
        for (i, &target) in targets.iter().enumerate() {
            let source = matches
                .as_ref()
                .map_or(self.builtins.never_type, |types| types[i]);
            if self.types.flags(source)? & tf::STRING_LITERAL != 0
                && self.types.flags(target)? & tf::TYPE_VARIABLE != 0
            {
                if let Some(index) = self.inference_index(run, target)? {
                    let parameter =
                        self.inference_context(run.context)?.inferences[index].parameter;
                    if let Some(constraint) = self.base_constraint_of_type(parameter)? {
                        if self.types.flags(constraint)? & tf::ANY == 0 {
                            let constituents = self.distributed_types(constraint)?;
                            let mut flags = 0;
                            for &ty in &constituents {
                                flags |= self.types.flags(ty)?;
                            }
                            if flags & tf::STRING == 0 {
                                let LiteralValue::String(text) =
                                    self.types.literal(source)?.value.clone()
                                else {
                                    return Err(Error::MissingLink("template inference string"));
                                };
                                let bytes = text.as_bytes();
                                let number = ts_jsnum::from_string(bytes);
                                if flags & tf::NUMBER_LIKE != 0
                                    && (bytes.is_empty()
                                        || !number.value().is_finite()
                                        || number.to_string().as_bytes() != bytes)
                                {
                                    flags &= !tf::NUMBER_LIKE;
                                }
                                if flags & tf::BIG_INT_LIKE != 0
                                    && (!crate::template_relation::valid_bigint_string(bytes)
                                        || template_bigint(bytes).to_text() != bytes)
                                {
                                    flags &= !tf::BIG_INT_LIKE;
                                }
                                let mut matching = self.builtins.never_type;
                                for ty in constituents {
                                    matching = self.preferred_template_candidate(
                                        source, matching, ty, flags, bytes,
                                    )?;
                                }
                                if self.types.flags(matching)? & tf::NEVER == 0 {
                                    self.infer_from_types(run, matching, target)?;
                                    continue;
                                }
                            }
                        }
                    }
                }
            }
            self.infer_from_types(run, source, target)?;
        }
        Ok(())
    }

    fn preferred_template_candidate(
        &mut self,
        source: TypeId,
        left: TypeId,
        right: TypeId,
        allowed: crate::TypeFlags,
        text: &[u8],
    ) -> Result<TypeId, Error> {
        let l = self.types.flags(left)?;
        let r = self.types.flags(right)?;
        if r & allowed == 0 {
            return Ok(left);
        }
        if l & tf::STRING != 0 {
            return Ok(left);
        }
        if r & tf::STRING != 0 {
            return Ok(source);
        }
        if l & tf::TEMPLATE_LITERAL != 0 {
            return Ok(left);
        }
        if r & tf::TEMPLATE_LITERAL != 0 && self.template_types_match(source, right, None)? {
            return Ok(source);
        }
        if l & tf::STRING_MAPPING != 0 {
            return Ok(left);
        }
        if r & tf::STRING_MAPPING != 0 {
            let symbol = self
                .types
                .get(right)?
                .symbol
                .ok_or(Error::MissingLink("string mapping symbol"))?;
            if self.get_string_mapping_type(symbol, source)? == source {
                return Ok(source);
            }
        }
        if l & tf::STRING_LITERAL != 0 {
            return Ok(left);
        }
        if r & tf::STRING_LITERAL != 0
            && matches!(&self.types.literal(right)?.value, LiteralValue::String(s) if s.as_bytes() == text)
        {
            return Ok(right);
        }
        if l & tf::NUMBER != 0 {
            return Ok(left);
        }
        if r & tf::NUMBER != 0 {
            return self.get_number_literal_type(ts_jsnum::from_string(text));
        }
        if l & tf::ENUM != 0 {
            return Ok(left);
        }
        if r & tf::ENUM != 0 {
            return self.get_number_literal_type(ts_jsnum::from_string(text));
        }
        if l & tf::NUMBER_LITERAL != 0 {
            return Ok(left);
        }
        if r & tf::NUMBER_LITERAL != 0
            && matches!(self.types.literal(right)?.value, LiteralValue::Number(n) if n == ts_jsnum::from_string(text))
        {
            return Ok(right);
        }
        if l & tf::BIG_INT != 0 {
            return Ok(left);
        }
        if r & tf::BIG_INT != 0 {
            return self.get_big_int_literal_type(template_bigint(text));
        }
        if l & tf::BIG_INT_LITERAL != 0 {
            return Ok(left);
        }
        if r & tf::BIG_INT_LITERAL != 0
            && matches!(&self.types.literal(right)?.value, LiteralValue::BigInt(n) if n.to_text() == text)
        {
            return Ok(right);
        }
        if l & tf::BOOLEAN != 0 {
            return Ok(left);
        }
        if r & tf::BOOLEAN != 0 {
            return Ok(match text {
                b"true" => self.builtins.true_type,
                b"false" => self.builtins.false_type,
                _ => self.builtins.boolean_type,
            });
        }
        if l & tf::BOOLEAN_LITERAL != 0 {
            return Ok(left);
        }
        if r & tf::BOOLEAN_LITERAL != 0
            && matches!(self.types.literal(right)?.value, LiteralValue::Boolean(b) if if b { text == b"true" } else { text == b"false" })
        {
            return Ok(right);
        }
        if l & tf::UNDEFINED != 0 {
            return Ok(left);
        }
        if r & tf::UNDEFINED != 0 && text == b"undefined" {
            return Ok(right);
        }
        if l & tf::NULL != 0 {
            return Ok(left);
        }
        if r & tf::NULL != 0 && text == b"null" {
            return Ok(right);
        }
        Ok(left)
    }
}

fn template_bigint(bytes: &[u8]) -> ts_jsnum::PseudoBigInt {
    let negative = bytes.starts_with(b"-");
    let magnitude = if negative { &bytes[1..] } else { bytes };
    let mut token = magnitude.to_vec();
    token.push(b'n');
    ts_jsnum::PseudoBigInt::new(&ts_jsnum::parse_pseudo_big_int(&token), negative)
}
