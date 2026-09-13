use crate::{
    inference::{self, priority as p, InferenceInfo},
    object_flags as of, type_flags as tf, CheckerState, Error, InferenceId, RelationKind,
    SignatureId, TypeId,
};

impl CheckerState {
    // port: tsc/internal/checker/inference.go:Checker.getInferredType
    pub(crate) fn signature_inference_candidates(
        &mut self,
        context: InferenceId,
        index: usize,
        info: &InferenceInfo,
        signature: SignatureId,
    ) -> Result<(Option<TypeId>, Option<TypeId>), Error> {
        let covariant = if info.candidates.is_empty() {
            None
        } else {
            Some(self.covariant_inference(info, signature)?)
        };
        let contravariant = if info.contra_candidates.is_empty() {
            None
        } else {
            Some(if info.priority & p::COMBINATION != 0 {
                self.get_intersection_type(&info.contra_candidates)?
            } else {
                self.common_subtype(&info.contra_candidates)?
            })
        };
        if covariant.is_some() || contravariant.is_some() {
            let mut prefer = covariant.is_some();
            if let (Some(co), Some(_)) = (covariant, contravariant) {
                prefer = false;
                if self.types.flags(co)? & (tf::NEVER | tf::ANY) == 0 {
                    for &candidate in &info.contra_candidates {
                        if self.is_type_related_to(co, candidate, RelationKind::Assignable)? {
                            prefer = true;
                            break;
                        }
                    }
                    if prefer {
                        let others = self.inference_context(context)?.inferences.clone();
                        'outer: for (i, other) in others.iter().enumerate() {
                            if i != index
                                && self.constraint_of_type_parameter(other.parameter)?
                                    != Some(info.parameter)
                            {
                                continue;
                            }
                            for &candidate in &other.candidates {
                                if !self.is_type_related_to(
                                    candidate,
                                    co,
                                    RelationKind::Assignable,
                                )? {
                                    prefer = false;
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
            }
            return Ok(if prefer {
                (covariant, contravariant)
            } else {
                (contravariant, covariant)
            });
        }
        if self.inference_context(context)?.flags & inference::NO_DEFAULT != 0 {
            return Ok((Some(self.builtins.silent_never_type), None));
        }
        let default = self.resolved_type_parameter_default(info.parameter)?;
        if default == self.builtins.no_constraint_type
            || default == self.builtins.circular_constraint_type
        {
            return Ok((None, None));
        }
        let forward: Vec<_> = self.inference_context(context)?.inferences[index..]
            .iter()
            .map(|i| i.parameter)
            .collect();
        let first =
            self.new_type_mapper(&forward, &vec![self.builtins.unknown_type; forward.len()])?;
        let second = self.inference_context(context)?.non_fixing_mapper;
        let mapper = self.alloc_mapper(crate::mapper::Mapper::Merged { first, second })?;
        Ok((Some(self.instantiate_type(default, Some(mapper))?), None))
    }

    // port: tsc/internal/checker/inference.go:Checker.getCovariantInference
    fn covariant_inference(
        &mut self,
        info: &InferenceInfo,
        signature: SignatureId,
    ) -> Result<TypeId, Error> {
        let mut candidates = info.candidates.clone();
        if candidates.len() > 1 {
            let mut literal = Vec::new();
            let mut other = Vec::new();
            for &candidate in &candidates {
                if self.types.object_flags(candidate)? & (of::OBJECT_LITERAL | of::ARRAY_LITERAL)
                    != 0
                {
                    literal.push(candidate);
                } else {
                    other.push(candidate);
                }
            }
            if !literal.is_empty() {
                other.push(self.get_union_type_ex(
                    &literal,
                    crate::UnionReduction::Subtype,
                    None,
                    None,
                )?);
                candidates = other;
            }
        }
        let mut primitive = self.is_const_type_variable(info.parameter, 0)?;
        if let Some(mut constraint) = self.constraint_of_type_parameter(info.parameter)? {
            if self.types.flags(constraint)? & tf::CONDITIONAL != 0 {
                constraint = self.default_conditional_constraint(constraint)?;
            }
            primitive |= self.maybe_kind(
                constraint,
                tf::PRIMITIVE | tf::INDEX | tf::TEMPLATE_LITERAL | tf::STRING_MAPPING,
            )?;
        }
        let widen = !primitive
            && info.top_level
            && (info.fixed || !self.parameter_at_top_level_in_return(signature, info.parameter)?);
        for candidate in &mut candidates {
            if primitive {
                *candidate = self.get_regular_type_of_literal_type(*candidate)?;
            } else if widen {
                *candidate = self.widen_literal_type(*candidate)?;
            }
        }
        let unwidened = if info.priority & p::COMBINATION != 0 {
            self.get_union_type_ex(&candidates, crate::UnionReduction::Subtype, None, None)?
        } else {
            self.common_supertype(&candidates)?
        };
        self.widened_inference_type(unwidened)
    }

    pub(crate) fn parameter_at_top_level_in_return(
        &mut self,
        signature: SignatureId,
        parameter: TypeId,
    ) -> Result<bool, Error> {
        if let Some(predicate) = self.type_predicate_of_signature(signature)? {
            return match self.signatures.predicate(predicate)?.t {
                Some(ty) => self.type_parameter_at_top_level(ty, parameter, 0),
                None => Ok(false),
            };
        }
        let ty = self.return_type_of_signature(signature)?;
        self.type_parameter_at_top_level(ty, parameter, 0)
    }

    // port: tsc/internal/checker/inference.go:Checker.getCommonSupertype
    pub(crate) fn common_supertype(&mut self, types: &[TypeId]) -> Result<TypeId, Error> {
        if types.len() == 1 {
            return Ok(types[0]);
        }
        if types.is_empty() {
            return Err(Error::MissingLink("common supertype candidates"));
        }
        let mut primary = types.to_vec();
        if self.options.strict_null_checks {
            for ty in &mut primary {
                *ty = self.filter_type_flags(*ty, !tf::NULLABLE)?;
            }
        }
        let mut common_base = None;
        let mut same_base = true;
        for &ty in &primary {
            if self.types.flags(ty)? & tf::NEVER == 0 {
                let base = self.base_literal_type(ty)?;
                let common = *common_base.get_or_insert(base);
                if base == ty || base != common {
                    same_base = false;
                    break;
                }
            }
        }
        let mut result = if same_base {
            self.get_union_type(&primary)?
        } else {
            let candidate = self.leftmost_supertype(&primary, RelationKind::StrictSubtype)?;
            let mut strict = true;
            for &ty in &primary {
                if ty != candidate
                    && !self.is_type_related_to(ty, candidate, RelationKind::StrictSubtype)?
                {
                    strict = false;
                    break;
                }
            }
            if strict {
                candidate
            } else {
                self.leftmost_supertype(&primary, RelationKind::Subtype)?
            }
        };
        if primary != types {
            let mut flags = 0;
            for &ty in types {
                flags |= self.combined_type_flags(ty)?;
            }
            let mut nullable = vec![result];
            if flags & tf::UNDEFINED != 0 {
                nullable.push(self.builtins.undefined_type);
            }
            if flags & tf::NULL != 0 {
                nullable.push(self.builtins.null_type);
            }
            result = self.get_union_type(&nullable)?;
        }
        Ok(result)
    }

    fn leftmost_supertype(
        &mut self,
        types: &[TypeId],
        kind: RelationKind,
    ) -> Result<TypeId, Error> {
        let mut candidate = types[0];
        for &ty in &types[1..] {
            if self.is_type_related_to(candidate, ty, kind)? {
                candidate = ty;
            }
        }
        Ok(candidate)
    }
    fn common_subtype(&mut self, types: &[TypeId]) -> Result<TypeId, Error> {
        let mut candidate = types[0];
        for &ty in &types[1..] {
            if self.is_type_related_to(ty, candidate, RelationKind::Subtype)? {
                candidate = ty;
            }
        }
        Ok(candidate)
    }
    fn combined_type_flags(&self, ty: TypeId) -> Result<crate::TypeFlags, Error> {
        if self.types.flags(ty)? & tf::UNION == 0 {
            return self.types.flags(ty);
        }
        let mut result = 0;
        for &part in self.types.types_of(ty)? {
            result |= self.combined_type_flags(part)?;
        }
        Ok(result)
    }
    fn maybe_kind(&self, ty: TypeId, flags: crate::TypeFlags) -> Result<bool, Error> {
        if self.types.flags(ty)? & flags != 0 {
            return Ok(true);
        }
        if self.types.flags(ty)? & tf::UNION_OR_INTERSECTION != 0 {
            for &part in self.types.types_of(ty)? {
                if self.maybe_kind(part, flags)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getBaseTypeOfLiteralType
    pub(crate) fn base_literal_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::ENUM_LIKE != 0 {
            return self.base_type_of_enum_like(ty);
        }
        if flags & (tf::STRING_LITERAL | tf::TEMPLATE_LITERAL | tf::STRING_MAPPING) != 0 {
            return Ok(self.builtins.string_type);
        }
        if flags & tf::NUMBER_LITERAL != 0 {
            return Ok(self.builtins.number_type);
        }
        if flags & tf::BIG_INT_LITERAL != 0 {
            return Ok(self.builtins.bigint_type);
        }
        if flags & tf::BOOLEAN_LITERAL != 0 {
            return Ok(self.builtins.boolean_type);
        }
        if flags & tf::UNION != 0 {
            if let Some(&cached) = self.types.caches.literal_union_bases.get(&ty) {
                return Ok(cached);
            }
            let result = self
                .map_type(ty, &mut |state, part| {
                    state.base_literal_type(part).map(Some)
                })?
                .ok_or(Error::MissingLink("literal union base"))?;
            self.types.caches.literal_union_bases.insert(ty, result);
            return Ok(result);
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getWidenedTypeWithContext
    pub(crate) fn widened_inference_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        self.widened_type(ty)
    }
}
