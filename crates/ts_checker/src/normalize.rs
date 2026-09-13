//! Normalization shared by relations, constraints and inference. Read and write
//! indexed-access normal forms are cached separately.
use crate::{object_flags as of, type_flags as tf, CheckerState, Error, TypeId};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getNormalizedType
    pub(crate) fn normalized_type(
        &mut self,
        mut ty: TypeId,
        writing: bool,
    ) -> Result<TypeId, Error> {
        loop {
            let record = *self.types.get(ty)?;
            let normalized = if self.is_fresh_literal_type(ty)? {
                self.types.literal(ty)?.regular
            } else if self.is_generic_tuple_type(ty)? {
                let elements = self.element_types(ty)?;
                let mut normalized = Vec::with_capacity(elements.len());
                for &element in elements.iter() {
                    normalized.push(self.simplified_type(element, writing)?);
                }
                if normalized == elements.as_ref() {
                    ty
                } else {
                    self.normalize_tuple(self.types.target(ty)?, &normalized, 0)?
                }
            } else if record.object_flags & of::REFERENCE != 0 {
                if self.types.type_reference(ty)?.node.is_some() {
                    let arguments = self.get_type_arguments(ty)?;
                    self.create_type_reference(self.types.target(ty)?, &arguments)?
                } else {
                    self.single_equivalent_base(ty)?.unwrap_or(ty)
                }
            } else if record.flags & tf::UNION_OR_INTERSECTION != 0 {
                let reduced = self.get_reduced_type(ty)?;
                if reduced != ty {
                    reduced
                } else if record.flags & tf::INTERSECTION != 0 {
                    let parts = self.types.compound_types(ty)?.clone();
                    let mut instantiable = false;
                    let mut nullable_empty = false;
                    for &part in parts.iter() {
                        let flags = self.types.flags(part)?;
                        instantiable |= flags & tf::INSTANTIABLE != 0;
                        nullable_empty |= flags & tf::NULLABLE != 0
                            || self.is_empty_anonymous_object_type(part)?;
                    }
                    if instantiable && nullable_empty {
                        let mut normalized = Vec::new();
                        for &part in parts.iter() {
                            normalized.push(self.normalized_type(part, writing)?);
                        }
                        if normalized == parts.as_ref() {
                            ty
                        } else {
                            self.get_intersection_type(&normalized)?
                        }
                    } else {
                        ty
                    }
                } else {
                    ty
                }
            } else if record.flags & tf::SUBSTITUTION != 0 {
                if writing {
                    self.types.substitution(ty)?.base
                } else {
                    self.substitution_intersection(ty)?
                }
            } else {
                self.simplified_type(ty, writing)?
            };
            if normalized == ty {
                return Ok(ty);
            }
            ty = normalized;
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getSingleBaseForNonAugmentingSubtype
    pub(crate) fn single_equivalent_base(&mut self, ty: TypeId) -> Result<Option<TypeId>, Error> {
        if self.types.get(ty)?.object_flags & of::REFERENCE == 0 {
            return Ok(None);
        }
        let target = self.types.target(ty)?;
        if self.types.get(target)?.object_flags & of::CLASS_OR_INTERFACE == 0 {
            return Ok(None);
        }
        if self.types.get(ty)?.object_flags & of::IDENTICAL_BASE_TYPE_CALCULATED != 0 {
            return Ok(self.types.caches.equivalent_bases.get(&ty).copied());
        }
        self.types.get_mut(ty)?.object_flags |= of::IDENTICAL_BASE_TYPE_CALCULATED;
        let result = (|| {
            if self.types.get(target)?.object_flags & of::CLASS != 0 {
                if let Some(base) = self.class_base_type_node(target)? {
                    let expression = self
                        .ast(base)?
                        .node(base)?
                        .expression()
                        .ok_or(Error::MissingLink("class heritage expression"))?;
                    if !matches!(
                        self.ast(expression)?.node(expression)?.kind().known(),
                        Some(
                            ts_ast::SyntaxKind::Identifier
                                | ts_ast::SyntaxKind::PropertyAccessExpression
                        )
                    ) {
                        return Ok(None);
                    }
                }
            }
            let bases = self.interface_base_types(target)?;
            if bases.len() != 1 {
                return Ok(None);
            }
            let symbol = self
                .types
                .get(ty)?
                .symbol
                .ok_or(Error::MissingLink("equivalent base symbol"))?;
            if let Some(table) = self.symbol(symbol)?.members() {
                if !self.table(table)?.is_empty() {
                    return Ok(None);
                }
            }
            let parameters = self.types.interface(target)?.type_parameters().to_vec();
            let arguments = self.get_type_arguments(ty)?;
            let base = if parameters.is_empty() {
                bases[0]
            } else {
                let mapper = self.new_type_mapper(&parameters, &arguments[..parameters.len()])?;
                self.instantiate_type(bases[0], Some(mapper))?
            };
            Ok(Some(if arguments.len() > parameters.len() {
                self.get_type_with_this_argument(
                    base,
                    *arguments
                        .last()
                        .ok_or(Error::MissingLink("base this argument"))?,
                    false,
                )?
            } else {
                base
            }))
        })();
        match result {
            Ok(Some(base)) => {
                self.types.caches.equivalent_bases.insert(ty, base);
            }
            Err(_) => self.types.get_mut(ty)?.object_flags &= !of::IDENTICAL_BASE_TYPE_CALCULATED,
            _ => {}
        }
        result
    }

    // port: tsc/internal/checker/checker.go:Checker.getSimplifiedType
    pub(crate) fn simplified_type(&mut self, ty: TypeId, writing: bool) -> Result<TypeId, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::CONDITIONAL != 0 {
            return self.simplified_conditional_type(ty, writing);
        }
        if flags & tf::INDEXED_ACCESS == 0 {
            return Ok(ty);
        }
        let key = (ty, writing);
        if let Some(&cached) = self.types.caches.simplified_types.get(&key) {
            return Ok(if cached == self.builtins.circular_constraint_type {
                ty
            } else {
                cached
            });
        }
        self.types.caches.simplified_types.insert(key, ty);
        let result = self.simplify_indexed_type(ty, writing);
        match result {
            Ok(result) if result != ty => {
                let result = self.filter_type(result, &mut |_, part| Ok(part != ty))?;
                self.types.caches.simplified_types.insert(key, result);
                Ok(result)
            }
            Err(error) => {
                self.types.caches.simplified_types.remove(&key);
                Err(error)
            }
            result => result,
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getSimplifiedIndexedAccessTypeWorker
    fn simplify_indexed_type(&mut self, ty: TypeId, writing: bool) -> Result<TypeId, Error> {
        let data = *self.types.indexed_access(ty)?;
        let object = self.simplified_type(data.object_type, writing)?;
        let index = self.simplified_type(data.index_type, writing)?;
        if self.types.flags(index)? & tf::UNION != 0 {
            let parts = self.types.compound_types(index)?.clone();
            let mut types = Vec::new();
            for &part in parts.iter() {
                let access = self.get_indexed_access_type(object, part, 0, None, None)?;
                types.push(self.simplified_type(access, writing)?);
            }
            return if writing {
                self.get_intersection_type(&types)
            } else {
                self.get_union_type(&types)
            };
        }
        if self.types.flags(index)? & tf::INSTANTIABLE == 0 {
            if let Some(distributed) = self.distribute_index_over_object(object, index, writing)? {
                return Ok(distributed);
            }
        }
        if self.is_generic_tuple_type(object)? && self.types.flags(index)? & tf::NUMBER_LIKE != 0 {
            let start = if self.types.flags(index)? & tf::NUMBER != 0 {
                0
            } else {
                self.types.tuple(self.types.target(object)?)?.fixed_length as usize
            };
            if let Some(element) = self.tuple_slice_element_type(object, start, 0, writing)? {
                return Ok(element);
            }
        }
        if self.is_generic_mapped_type(object)? && !self.mapped_remaps_keys(object)? {
            let result = self.substitute_indexed_mapped(object, data.index_type)?;
            return self
                .map_type(result, &mut |checker, ty| {
                    checker.simplified_type(ty, writing).map(Some)
                })?
                .ok_or(Error::MissingLink("simplified mapped result"));
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.distributeIndexOverObjectType
    pub(crate) fn distribute_index_over_object(
        &mut self,
        object: TypeId,
        index: TypeId,
        writing: bool,
    ) -> Result<Option<TypeId>, Error> {
        let flags = self.types.flags(object)?;
        if flags & tf::UNION != 0
            || flags & tf::INTERSECTION != 0 && !self.should_defer_index(object, 0)?
        {
            let parts = self.types.compound_types(object)?.clone();
            let mut types = Vec::new();
            for &part in parts.iter() {
                let access = self.get_indexed_access_type(part, index, 0, None, None)?;
                types.push(self.simplified_type(access, writing)?);
            }
            return if writing || flags & tf::INTERSECTION != 0 {
                self.get_intersection_type(&types).map(Some)
            } else {
                self.get_union_type(&types).map(Some)
            };
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getSimplifiedTypeOrConstraint
    pub(crate) fn simplified_type_or_constraint(
        &mut self,
        ty: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        let simplified = self.simplified_type(ty, false)?;
        if simplified == ty {
            self.constraint_of_type(ty)
        } else {
            Ok(Some(simplified))
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getConstraintOfType
    pub(crate) fn constraint_of_type(&mut self, ty: TypeId) -> Result<Option<TypeId>, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::TYPE_PARAMETER != 0 {
            return self.constraint_of_type_parameter(ty);
        }
        if flags & tf::CONDITIONAL != 0 {
            return if self.has_non_circular_base_constraint(ty)? {
                self.constraint_from_conditional(ty).map(Some)
            } else {
                Ok(None)
            };
        }
        if flags & tf::INDEXED_ACCESS != 0 {
            if !self.has_non_circular_base_constraint(ty)? {
                return Ok(None);
            }
            let data = *self.types.indexed_access(ty)?;
            if self.mapped_generic_indexed_access(ty)? {
                return self
                    .substitute_indexed_mapped(data.object_type, data.index_type)
                    .map(Some);
            }
            if let Some(index) = self.simplified_type_or_constraint(data.index_type)? {
                if index != data.index_type {
                    if let Some(result) = self.indexed_access_or_undefined(
                        data.object_type,
                        index,
                        data.access_flags,
                        None,
                        None,
                    )? {
                        return Ok(Some(result));
                    }
                }
            }
            if let Some(object) = self.simplified_type_or_constraint(data.object_type)? {
                if object != data.object_type {
                    return self.indexed_access_or_undefined(
                        object,
                        data.index_type,
                        data.access_flags,
                        None,
                        None,
                    );
                }
            }
            return Ok(None);
        }
        self.base_constraint_of_type(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getElementTypeOfSliceOfTupleType
    pub(crate) fn tuple_slice_element_type(
        &mut self,
        ty: TypeId,
        start: usize,
        end_skip: usize,
        writing: bool,
    ) -> Result<Option<TypeId>, Error> {
        self.tuple_slice_element_type_ex(ty, start, end_skip, writing, false)
    }

    pub(crate) fn tuple_slice_element_type_ex(
        &mut self,
        ty: TypeId,
        start: usize,
        end_skip: usize,
        writing: bool,
        no_reductions: bool,
    ) -> Result<Option<TypeId>, Error> {
        let elements = self.element_types(ty)?;
        let infos = self
            .types
            .tuple(self.types.target(ty)?)?
            .element_infos
            .clone();
        let end = elements.len().saturating_sub(end_skip);
        if start >= end {
            return Ok(None);
        }
        let mut types = Vec::new();
        for position in start..end {
            let mut ty = elements[position];
            if infos[position].flags & crate::element_flags::VARIADIC != 0 {
                ty = self.get_indexed_access_type(ty, self.builtins.number_type, 0, None, None)?;
            }
            types.push(ty);
        }
        if writing {
            self.get_intersection_type(&types).map(Some)
        } else {
            self.get_union_type_ex(
                &types,
                if no_reductions {
                    crate::UnionReduction::None
                } else {
                    crate::UnionReduction::Literal
                },
                None,
                None,
            )
            .map(Some)
        }
    }
}
