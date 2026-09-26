use crate::{object_flags as of, type_flags as tf, CheckerState, Error, RelationKind, TypeId};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.removeStringLiteralsMatchedByTemplateLiterals
    pub(crate) fn remove_matched_string_literals(
        &mut self,
        mut types: Vec<TypeId>,
    ) -> Result<Vec<TypeId>, Error> {
        let mut templates = Vec::new();
        for &ty in &types {
            if self.is_pattern_literal_type(ty)? {
                templates.push(ty);
            }
        }
        if !templates.is_empty() {
            for index in (0..types.len()).rev() {
                let source = types[index];
                if self.types.flags(source)? & tf::STRING_LITERAL == 0 {
                    continue;
                }
                for &target in &templates {
                    let matched = if self.types.flags(target)? & tf::TEMPLATE_LITERAL != 0 {
                        self.template_types_match(source, target, None)?
                    } else {
                        self.member_of_string_mapping(source, target)?
                    };
                    if matched {
                        types.remove(index);
                        break;
                    }
                }
            }
        }
        Ok(types)
    }

    fn constrained_intersection_parts(
        &self,
        ty: TypeId,
    ) -> Result<Option<(TypeId, TypeId)>, Error> {
        if self.types.flags(ty)? & tf::INTERSECTION == 0
            || self.types.object_flags(ty)? & of::IS_CONSTRAINED_TYPE_VARIABLE == 0
        {
            return Ok(None);
        }
        let types = self.types.types_of(ty)?;
        if types.len() != 2 {
            return Err(Error::MissingLink("constraining intersection arity"));
        }
        let index = usize::from(self.types.flags(types[0])? & tf::TYPE_VARIABLE == 0);
        Ok(Some((types[index], types[1 - index])))
    }

    // port: tsc/internal/checker/checker.go:Checker.removeConstrainedTypeVariables
    pub(crate) fn remove_constrained_variables(
        &mut self,
        mut types: Vec<TypeId>,
    ) -> Result<Vec<TypeId>, Error> {
        let mut variables = Vec::new();
        for &ty in &types {
            if let Some((variable, _)) = self.constrained_intersection_parts(ty)? {
                if !variables.contains(&variable) {
                    variables.push(variable);
                }
            }
        }
        for variable in variables {
            let mut primitives = Vec::new();
            for &ty in &types {
                if let Some((v, primitive)) = self.constrained_intersection_parts(ty)? {
                    if v == variable && !primitives.contains(&primitive) {
                        primitives.push(primitive);
                    }
                }
            }
            let constraint = self
                .base_constraint_of_type(variable)?
                .ok_or(Error::MissingLink("constrained variable base"))?;
            if self
                .distributed_types(constraint)?
                .iter()
                .all(|ty| primitives.contains(ty))
            {
                for index in (0..types.len()).rev() {
                    if let Some((v, primitive)) =
                        self.constrained_intersection_parts(types[index])?
                    {
                        if v == variable && primitives.contains(&primitive) {
                            types.remove(index);
                        }
                    }
                }
                if !types.contains(&variable) {
                    let mut position = 0;
                    while position < types.len()
                        && self.compare_types(types[position], variable)?.is_lt()
                    {
                        position += 1;
                    }
                    types.insert(position, variable);
                }
            }
        }
        Ok(types)
    }

    // port: tsc/internal/checker/checker.go:Checker.removeSubtypes
    /// `None` when the union is too complex to represent (the diagnostic is reported here).
    pub(crate) fn remove_subtypes(
        &mut self,
        mut types: Vec<TypeId>,
        has_objects: bool,
    ) -> Result<Option<Vec<TypeId>>, Error> {
        if types.len() < 2 {
            return Ok(Some(types));
        }
        let key = crate::key::type_list_key(&types);
        if let Some(cached) = self.types.caches.subtype_reductions.get(&key) {
            return Ok(Some(cached.to_vec()));
        }
        let mut has_empty = false;
        if has_objects {
            for &ty in &types {
                if self.types.flags(ty)? & tf::OBJECT != 0
                    && !self.is_generic_mapped_type(ty)?
                    && self.empty_object_type(ty)?
                {
                    has_empty = true;
                    break;
                }
            }
        }
        let original_length = types.len();
        let mut index = types.len();
        let mut count = 0;
        while index > 0 {
            index -= 1;
            let source = types[index];
            let flags = self.types.flags(source)?;
            if !has_empty && flags & tf::STRUCTURED_OR_INSTANTIABLE == 0 {
                continue;
            }
            if flags & tf::TYPE_PARAMETER != 0 {
                let base = self.base_constraint_of_type(source)?.unwrap_or(source);
                if self.types.flags(base)? & tf::UNION != 0 {
                    let others: Vec<_> = types
                        .iter()
                        .map(|&ty| {
                            if ty == source {
                                self.builtins.never_type
                            } else {
                                ty
                            }
                        })
                        .collect();
                    let union = self.get_union_type(&others)?;
                    if self.is_type_related_to(source, union, RelationKind::StrictSubtype)? {
                        types.remove(index);
                    }
                    continue;
                }
            }
            let mut key_property = None;
            if flags & (tf::OBJECT | tf::INTERSECTION | tf::INSTANTIABLE_NON_PRIMITIVE) != 0 {
                for property in self.get_properties_of_type(source)? {
                    let ty = self.get_type_of_symbol(property)?;
                    if self.types.flags(ty)? & tf::UNIT != 0 {
                        key_property = Some((
                            self.symbol(property)?.name_to_owned(),
                            self.get_regular_type_of_literal_type(ty)?,
                        ));
                        break;
                    }
                }
            }
            let mut remove = false;
            for &target in &types {
                if source == target {
                    continue;
                }
                if count == 100_000
                    && (count / (original_length - index)) * original_length > 1_000_000
                {
                    #[cfg(feature = "recursion-probe")]
                    if let Some(probe) = &mut self.c2_limits_probe {
                        probe
                            .subtype_estimates
                            .push((count, (count / (original_length - index)) * original_length));
                    }
                    // After 100000 subtype checks we estimate the remaining amount of work by assuming the
                    // same ratio of checks per element. If the estimated number of remaining type checks is
                    // greater than 1M we deem the union type too complex to represent.
                    self.error_at(
                        self.current_node,
                        tsr_diagnostics::Expression_produces_a_union_type_that_is_too_complex_to_represent,
                        vec![],
                    )?;
                    return Ok(None);
                }
                count += 1;
                if let Some((name, key_type)) = &key_property {
                    if self.types.flags(target)?
                        & (tf::OBJECT | tf::INTERSECTION | tf::INSTANTIABLE_NON_PRIMITIVE)
                        != 0
                    {
                        if let Some(property) =
                            self.constituent_property(target, name.as_bytes(), false)?
                        {
                            let ty = self.get_type_of_symbol(property)?;
                            if self.types.flags(ty)? & tf::UNIT != 0
                                && self.get_regular_type_of_literal_type(ty)? != *key_type
                            {
                                continue;
                            }
                        }
                    }
                }
                if (source == self.builtins.empty_object_type
                    || source == self.builtins.unknown_empty_object_type)
                    && self.types.get(target)?.symbol.is_some()
                    && self.is_empty_anonymous_object_type(target)?
                {
                    continue;
                }
                if self.is_type_related_to(source, target, RelationKind::StrictSubtype)? {
                    let source_target = if self.types.object_flags(source)? & of::REFERENCE != 0 {
                        self.types.target(source)?
                    } else {
                        source
                    };
                    let target_target = if self.types.object_flags(target)? & of::REFERENCE != 0 {
                        self.types.target(target)?
                    } else {
                        target
                    };
                    // Two classes are only subtype-reducible when one derives from the other.
                    if self.types.object_flags(source_target)?
                        & self.types.object_flags(target_target)?
                        & of::CLASS
                        != 0
                        && !self.is_type_derived_from(source, target)?
                    {
                        continue;
                    }
                    remove = true;
                    break;
                }
            }
            if remove {
                types.remove(index);
            }
        }
        self.types
            .caches
            .subtype_reductions
            .insert(key, types.clone().into());
        Ok(Some(types))
    }
}
