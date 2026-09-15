//! Structured relation dispatch, after normalization and recursive cache lookup.
use crate::{
    object_flags as of,
    relater::{Relater, RelationKind, BOTH, SOURCE, TARGET},
    ternary as tr, type_flags as tf, Error, Ternary, TypeId,
};

impl Relater<'_> {
    // port: tsc/internal/checker/relater.go:Relater.structuredTypeRelatedTo
    pub(crate) fn structured_related(
        &mut self,
        source: TypeId,
        target: TypeId,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        let saved = self.errors.clone();
        let mut result = self.structured_worker(source, target, intersection)?;
        if self.kind == RelationKind::Identity {
            return Ok(result);
        }
        let s = self.checker.types.flags(source)?;
        let t = self.checker.types.flags(target)?;
        if result == tr::FALSE
            && (s & tf::INTERSECTION != 0 || s & tf::TYPE_PARAMETER != 0 && t & tf::UNION != 0)
        {
            let parts = if s & tf::INTERSECTION != 0 {
                self.checker.types.compound_types(source)?.clone()
            } else {
                vec![source].into()
            };
            if let Some(constraint) = self
                .checker
                .effective_intersection_constraint(&parts, t & tf::UNION != 0)?
            {
                let contains_source = if self.checker.types.flags(constraint)? & tf::UNION != 0 {
                    self.checker.types.types_of(constraint)?.contains(&source)
                } else {
                    constraint == source
                };
                if !contains_source {
                    result =
                        self.related_with_errors(constraint, target, SOURCE, intersection, false)?;
                }
            }
        }
        if result != tr::FALSE
            && intersection & TARGET == 0
            && t & tf::INTERSECTION != 0
            && self.checker.get_generic_object_flags(target)? & of::IS_GENERIC_OBJECT_TYPE == 0
            && s & (tf::OBJECT | tf::INTERSECTION) != 0
        {
            result &= self.properties_related(source, target, false, 0)?;
            if result != tr::FALSE
                && self.checker.types.get(source)?.object_flags
                    & (of::OBJECT_LITERAL | of::FRESH_LITERAL)
                    == (of::OBJECT_LITERAL | of::FRESH_LITERAL)
            {
                result &= self.indexes_related(source, target, false, 0)?;
            }
        } else if result != tr::FALSE
            && t & tf::OBJECT != 0
            && self.checker.get_generic_object_flags(target)? & of::IS_GENERIC_OBJECT_TYPE == 0
            && !self.checker.is_tuple_type(target)?
            && !self.checker.is_array_type(target)?
            && s & tf::INTERSECTION != 0
        {
            let apparent = self.checker.apparent_type(source)?;
            let mut check = self.checker.types.flags(apparent)? & tf::STRUCTURED_TYPE != 0;
            for &part in self.checker.types.types_of(source)? {
                check &= part != target
                    && self.checker.types.get(part)?.object_flags & of::NON_INFERRABLE_TYPE == 0;
            }
            if check {
                result &= self.properties_related(source, target, true, intersection)?;
            }
        }
        if result != tr::FALSE {
            self.errors = saved;
        }
        Ok(result)
    }

    // port: tsc/internal/checker/relater.go:Relater.structuredTypeRelatedToWorker
    fn structured_worker(
        &mut self,
        mut source: TypeId,
        target: TypeId,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        let mut variance = crate::relater_variance::VarianceCheck::default();
        let saved = self.errors.clone();
        let s = self.checker.types.flags(source)?;
        let t = self.checker.types.flags(target)?;
        if self.kind == RelationKind::Identity {
            if s & tf::UNION_OR_INTERSECTION != 0 {
                let mut result = self.each_source_some_target(source, target)?;
                if result != tr::FALSE {
                    result &= self.each_source_some_target(target, source)?;
                }
                return Ok(result);
            }
            if s & tf::INDEX != 0 {
                return self.related(
                    self.checker.types.target(source)?,
                    self.checker.types.target(target)?,
                    BOTH,
                    0,
                );
            }
            if s & tf::INDEXED_ACCESS != 0 {
                let source = *self.checker.types.indexed_access(source)?;
                let target = *self.checker.types.indexed_access(target)?;
                let mut result = self.related(source.object_type, target.object_type, BOTH, 0)?;
                if result != tr::FALSE {
                    result &= self.related(source.index_type, target.index_type, BOTH, 0)?;
                }
                return Ok(result);
            }
            if s & tf::TEMPLATE_LITERAL != 0 {
                let source_data = self.checker.types.template_literal(source)?;
                let target_data = self.checker.types.template_literal(target)?;
                if source_data.texts != target_data.texts {
                    return Ok(tr::FALSE);
                }
                let sources = source_data.types.clone();
                let targets = target_data.types.clone();
                let mut result = tr::TRUE;
                for (&s, &t) in sources.iter().zip(targets.iter()) {
                    result &= self.related(s, t, BOTH, 0)?;
                    if result == tr::FALSE {
                        break;
                    }
                }
                return Ok(result);
            }
            if s & tf::STRING_MAPPING != 0 {
                return if self.checker.types.get(source)?.symbol
                    == self.checker.types.get(target)?.symbol
                {
                    self.related(
                        self.checker.types.target(source)?,
                        self.checker.types.target(target)?,
                        BOTH,
                        0,
                    )
                } else {
                    Ok(tr::FALSE)
                };
            }
            if s & tf::CONDITIONAL != 0 {
                return self.conditional_identity(source, target);
            }
            if s & tf::SUBSTITUTION != 0 {
                let a = *self.checker.types.substitution(source)?;
                let b = *self.checker.types.substitution(target)?;
                let result = self.related(a.base, b.base, BOTH, 0)?;
                return if result == tr::FALSE {
                    Ok(result)
                } else {
                    Ok(result & self.related(a.constraint, b.constraint, BOTH, 0)?)
                };
            }
            if s & tf::OBJECT == 0 {
                return Ok(tr::FALSE);
            }
        } else if (s | t) & tf::UNION_OR_INTERSECTION != 0 {
            let result = self.union_intersection_related(source, target, intersection)?;
            if result != tr::FALSE {
                return Ok(result);
            }
            if !(s & tf::INSTANTIABLE != 0
                || s & tf::OBJECT != 0 && t & tf::UNION != 0
                || s & tf::INTERSECTION != 0
                    && t & (tf::OBJECT | tf::UNION | tf::INSTANTIABLE) != 0)
            {
                return Ok(tr::FALSE);
            }
        }
        if s & (tf::OBJECT | tf::CONDITIONAL) != 0
            && !self.checker.variance.markers.contains(&source)
            && !self.checker.variance.markers.contains(&target)
        {
            if let (Some(source_alias), Some(target_alias)) = (
                self.checker.types.alias_of(source)?.cloned(),
                self.checker.types.alias_of(target)?.cloned(),
            ) {
                if source_alias.symbol == target_alias.symbol
                    && !source_alias.type_arguments.is_empty()
                {
                    let variances = self.checker.alias_variances(source_alias.symbol)?;
                    if variances.is_empty() {
                        return Ok(tr::UNKNOWN);
                    }
                    let parameters = self
                        .checker
                        .query
                        .type_aliases
                        .try_get(source_alias.symbol)
                        .and_then(|l| l.parameters.clone())
                        .ok_or(Error::MissingLink("alias parameters"))?;
                    let in_js = match self
                        .checker
                        .symbol(source_alias.symbol)?
                        .value_declaration()
                    {
                        Some(node) => {
                            self.checker.ast(node)?.node(node)?.flags()
                                & ts_ast::node_flags::JAVA_SCRIPT_FILE
                                != 0
                        }
                        None => false,
                    };
                    let sources = self.checker.fill_missing_type_arguments(
                        &source_alias.type_arguments,
                        &parameters,
                        in_js,
                    )?;
                    let targets = self.checker.fill_missing_type_arguments(
                        &target_alias.type_arguments,
                        &parameters,
                        in_js,
                    )?;
                    if let Some(result) = self.relate_variances(
                        &sources,
                        &targets,
                        &variances,
                        intersection,
                        &saved,
                        &mut variance,
                    )? {
                        return Ok(result);
                    }
                }
            }
        }
        if t & tf::INDEXED_ACCESS != 0 {
            let target_data = *self.checker.types.indexed_access(target)?;
            if s & tf::INDEXED_ACCESS != 0 {
                let source_data = *self.checker.types.indexed_access(source)?;
                let mut result = self.related(
                    source_data.object_type,
                    target_data.object_type,
                    BOTH,
                    intersection,
                )?;
                if result != tr::FALSE {
                    result &= self.related(
                        source_data.index_type,
                        target_data.index_type,
                        BOTH,
                        intersection,
                    )?;
                }
                if result != tr::FALSE {
                    return Ok(result);
                }
            }
            let object = self
                .checker
                .base_constraint_of_type(target_data.object_type)?
                .unwrap_or(target_data.object_type);
            let index = self
                .checker
                .base_constraint_of_type(target_data.index_type)?
                .unwrap_or(target_data.index_type);
            if matches!(
                self.kind,
                RelationKind::Assignable | RelationKind::Comparable
            ) && self.checker.get_generic_object_flags(object)? & of::IS_GENERIC_OBJECT_TYPE == 0
                && !self.checker.is_generic_index_type(index)?
            {
                let flags = crate::access_flags::WRITING
                    | if object == target_data.object_type {
                        0
                    } else {
                        crate::access_flags::NO_INDEX_SIGNATURES
                    };
                if let Some(constraint) = self
                    .checker
                    .indexed_access_or_undefined(object, index, flags, None, None)?
                {
                    let result = self.related(source, constraint, TARGET, intersection)?;
                    if result != tr::FALSE {
                        return Ok(result);
                    }
                }
            }
        } else if t & tf::INDEX != 0 {
            let target_data = *self.checker.types.index_type(target)?;
            if s & tf::INDEX != 0 {
                let result = self.related_with_errors(
                    target_data.target,
                    self.checker.types.target(source)?,
                    BOTH,
                    0,
                    false,
                )?;
                if result != tr::FALSE {
                    return Ok(result);
                }
            }
            if self.checker.is_tuple_type(target_data.target)? {
                let keys = self.checker.known_tuple_keys(target_data.target)?;
                let result = self.related(source, keys, TARGET, 0)?;
                if result != tr::FALSE {
                    return Ok(result);
                }
            } else if let Some(constraint) = self
                .checker
                .simplified_type_or_constraint(target_data.target)?
            {
                let index = self.checker.get_index_type(
                    constraint,
                    target_data.index_flags | crate::indexes::NO_REDUCIBLE_CHECK,
                )?;
                if self.related(source, index, TARGET, 0)? == tr::TRUE {
                    return Ok(tr::TRUE);
                }
            } else if self.checker.is_generic_mapped_type(target_data.target)? {
                let name = self.checker.mapped_name(target_data.target)?;
                let constraint = self.checker.mapped_constraint(target_data.target)?;
                let keys = if let Some(name) = name {
                    if self.checker.mapped_keyof_constraint(target_data.target)? {
                        let mapped = self
                            .checker
                            .apparent_mapped_keys(name, target_data.target)?;
                        self.checker.get_union_type(&[mapped, name])?
                    } else {
                        name
                    }
                } else {
                    constraint
                };
                if self.related(source, keys, TARGET, 0)? == tr::TRUE {
                    return Ok(tr::TRUE);
                }
            }
        } else if t & tf::CONDITIONAL != 0 {
            let result = self.conditional_target(source, target, intersection)?;
            if result != tr::FALSE {
                return Ok(result);
            }
        } else if t & tf::TEMPLATE_LITERAL != 0 {
            return self.template_related(source, target);
        } else if t & tf::STRING_MAPPING != 0 && s & tf::STRING_MAPPING == 0 {
            if self.checker.member_of_string_mapping(source, target)? {
                return Ok(tr::TRUE);
            }
        } else if self.checker.is_generic_mapped_type(target)?
            && self.kind != RelationKind::Identity
        {
            let result = self.generic_mapped_target(source, target)?;
            if result != tr::FALSE {
                return Ok(result);
            }
        }
        if s & tf::TYPE_VARIABLE != 0 {
            if s & tf::INDEXED_ACCESS == 0 || t & tf::INDEXED_ACCESS == 0 {
                let constraint = self
                    .checker
                    .constraint_of_type(source)?
                    .unwrap_or(self.checker.builtins.unknown_type);
                // hi-speed no-this-instantiation check (less accurate, but avoids costly
                // `this`-instantiation when the constraint will suffice); it never reports.
                let result =
                    self.related_with_errors(constraint, target, SOURCE, intersection, false)?;
                if result != tr::FALSE {
                    return Ok(result);
                }
                let with_this = self
                    .checker
                    .get_type_with_this_argument(constraint, source, false)?;
                let report = self.report_errors
                    && constraint != self.checker.builtins.unknown_type
                    && t & s & tf::TYPE_PARAMETER == 0;
                let result =
                    self.related_with_errors(with_this, target, SOURCE, intersection, report)?;
                if result != tr::FALSE {
                    return Ok(result);
                }
                if self.checker.mapped_generic_indexed_access(source)? {
                    let data = *self.checker.types.indexed_access(source)?;
                    if let Some(index) = self.checker.constraint_of_type(data.index_type)? {
                        let access = self.checker.get_indexed_access_type(
                            data.object_type,
                            index,
                            0,
                            None,
                            None,
                        )?;
                        return self.related(access, target, SOURCE, 0);
                    }
                }
            }
            return Ok(tr::FALSE);
        }
        if s & tf::INDEX != 0 {
            let index = *self.checker.types.index_type(source)?;
            let deferred = self
                .checker
                .should_defer_index(index.target, index.index_flags)?
                && self.checker.types.get(index.target)?.object_flags & of::MAPPED != 0;
            let result = self.related_with_errors(
                self.checker.builtins.string_number_symbol_type,
                target,
                SOURCE,
                0,
                self.report_errors && !deferred,
            )?;
            if result != tr::FALSE {
                return Ok(result);
            }
            if deferred {
                let keys = if let Some(name) = self.checker.mapped_name(index.target)? {
                    if self.checker.mapped_keyof_constraint(index.target)? {
                        self.checker.apparent_mapped_keys(name, index.target)?
                    } else {
                        name
                    }
                } else {
                    self.checker.mapped_constraint(index.target)?
                };
                let result = self.related(keys, target, SOURCE, 0)?;
                if result != tr::FALSE {
                    return Ok(result);
                }
            }
            return Ok(tr::FALSE);
        }
        if s & tf::CONDITIONAL != 0 {
            return self.conditional_source(source, target);
        }
        if s & tf::TEMPLATE_LITERAL != 0 && t & tf::OBJECT == 0 {
            if t & tf::TEMPLATE_LITERAL == 0 {
                if let Some(constraint) = self.checker.base_constraint_of_type(source)? {
                    if constraint != source {
                        return self.related(constraint, target, SOURCE, 0);
                    }
                }
            }
            return Ok(tr::FALSE);
        }
        if s & tf::STRING_MAPPING != 0 {
            if t & tf::STRING_MAPPING != 0 {
                if self.checker.types.get(source)?.symbol != self.checker.types.get(target)?.symbol
                {
                    return Ok(tr::FALSE);
                }
                return self.related(
                    self.checker.types.target(source)?,
                    self.checker.types.target(target)?,
                    BOTH,
                    0,
                );
            }
            if let Some(constraint) = self.checker.base_constraint_of_type(source)? {
                return self.related(constraint, target, SOURCE, 0);
            }
            return Ok(tr::FALSE);
        }
        if !matches!(
            self.kind,
            RelationKind::Subtype | RelationKind::StrictSubtype
        ) && self.checker.types.get(target)?.object_flags & of::MAPPED != 0
            && self.checker.mapped_optionality(target)? > 0
            && self.checker.empty_object_type(source)?
        {
            return Ok(tr::TRUE);
        }
        if self.checker.is_generic_mapped_type(target)? {
            return if self.checker.is_generic_mapped_type(source)? {
                self.mapped_related(source, target)
            } else {
                Ok(tr::FALSE)
            };
        }
        let primitive = s & tf::PRIMITIVE != 0;
        if self.kind != RelationKind::Identity {
            source = self.checker.apparent_type(source)?;
        } else if self.checker.is_generic_mapped_type(source)? {
            return Ok(tr::FALSE);
        }
        if self.checker.types.object_flags(source)? & of::REFERENCE != 0
            && self.checker.types.object_flags(target)? & of::REFERENCE != 0
            && self.checker.types.target(source)? == self.checker.types.target(target)?
            && !self.checker.is_tuple_type(source)?
            && !self.checker.variance.markers.contains(&source)
            && !self.checker.variance.markers.contains(&target)
        {
            let variances = self
                .checker
                .variances_of(self.checker.types.target(source)?)?;
            if variances.is_empty() {
                return Ok(tr::UNKNOWN);
            }
            let sources = self.checker.get_type_arguments(source)?;
            let targets = self.checker.get_type_arguments(target)?;
            if let Some(result) = self.relate_variances(
                &sources,
                &targets,
                &variances,
                intersection,
                &saved,
                &mut variance,
            )? {
                return Ok(result);
            }
        }
        if let Some(result) = self.array_relation(source, target)? {
            return Ok(result);
        }
        if self.checker.is_generic_tuple_type(source)?
            && self.checker.is_tuple_type(target)?
            && !self.checker.is_generic_tuple_type(target)?
        {
            if let Some(constraint) = self.checker.base_constraint_of_type(source)? {
                if constraint != source {
                    return self.related(constraint, target, SOURCE, 0);
                }
            }
        }
        if matches!(
            self.kind,
            RelationKind::Subtype | RelationKind::StrictSubtype
        ) && self.checker.empty_object_type(target)?
            && self.checker.types.get(target)?.object_flags & of::FRESH_LITERAL != 0
            && !self.checker.empty_object_type(source)?
        {
            return Ok(tr::FALSE);
        }
        if self.checker.types.flags(source)? & (tf::OBJECT | tf::INTERSECTION) != 0
            && t & tf::OBJECT != 0
        {
            let report = self.report_errors && self.errors.chain == saved.chain && !primitive;
            let result = self.with_reporting(report, |this| {
                let mut result = this.properties_related(source, target, false, intersection)?;
                if result != tr::FALSE {
                    result &= this.signatures_related(source, target, false, intersection)?;
                }
                if result != tr::FALSE {
                    result &= this.signatures_related(source, target, true, intersection)?;
                }
                if result != tr::FALSE {
                    result &= this.indexes_related(source, target, primitive, intersection)?;
                }
                Ok(result)
            })?;
            if result != tr::FALSE {
                if !variance.failed {
                    return Ok(result);
                }
                if let Some(chain) = variance.original_chain {
                    self.errors.chain = chain;
                } else if self.errors.chain.is_empty() {
                    self.errors.chain = saved.chain;
                }
            }
        }
        if self.checker.types.flags(source)? & (tf::OBJECT | tf::INTERSECTION) != 0
            && t & tf::UNION != 0
        {
            let object_target = self
                .checker
                .filter_type_flags(target, tf::OBJECT | tf::INTERSECTION | tf::SUBSTITUTION)?;
            if self.checker.types.flags(object_target)? & tf::UNION != 0 {
                return self.with_reporting(false, |this| {
                    this.discriminated_related(source, object_target)
                });
            }
        }
        Ok(tr::FALSE)
    }
}
