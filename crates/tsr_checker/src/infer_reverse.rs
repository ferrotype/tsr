use crate::{
    element_flags as ef, object_flags as of, type_flags as tf, types::Map, CheckerState, Error,
    LinkStore, MapperId, RelationKind, TypeId,
};
use tsr_arena::SymbolId;
use tsr_ast::{check_flags as cf, symbol_flags as sf, SymbolTable};

type ReverseKey = (TypeId, TypeId, TypeId);

#[derive(Default)]
pub(crate) struct ReverseInference {
    homomorphic: Map<ReverseKey, Option<TypeId>>,
    properties: Map<ReverseKey, Option<TypeId>>,
    symbols: LinkStore<SymbolId, Option<ReverseKey>>,
    source_stack: Vec<TypeId>,
    target_stack: Vec<TypeId>,
    expanding: u32,
}

impl CheckerState {
    pub(crate) fn reverse_symbol_parts(&self, symbol: SymbolId) -> Option<ReverseKey> {
        self.inference
            .reverse
            .symbols
            .try_get(symbol)
            .copied()
            .flatten()
    }
    pub(crate) fn reverse_mapped_parts(&self, ty: TypeId) -> Result<ReverseKey, Error> {
        let data = self.types.reverse_mapped(ty)?;
        Ok((
            data.source
                .ok_or(Error::MissingLink("reverse mapped source"))?,
            data.mapped_type
                .ok_or(Error::MissingLink("reverse mapped template"))?,
            data.constraint_type
                .ok_or(Error::MissingLink("reverse mapped constraint"))?,
        ))
    }

    // port: tsc/internal/checker/inference.go:Checker.inferTypeForHomomorphicMappedType
    pub(crate) fn infer_homomorphic_type(
        &mut self,
        source: TypeId,
        target: TypeId,
        constraint: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        let key = (source, target, constraint);
        if let Some(Some(ty)) = self.inference.reverse.homomorphic.get(&key) {
            return Ok(Some(*ty));
        }
        let result = self.create_reverse_mapped_type(source, target, constraint)?;
        self.inference.reverse.homomorphic.insert(key, result);
        Ok(result)
    }

    // port: tsc/internal/checker/inference.go:Checker.createReverseMappedType
    fn create_reverse_mapped_type(
        &mut self,
        source: TypeId,
        target: TypeId,
        constraint: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        if self
            .index_info_of_type(source, self.builtins.string_type)?
            .is_none()
            && (self.get_properties_of_type(source)?.is_empty()
                || !self.partially_inferable_type(source)?)
        {
            return Ok(None);
        }
        if self.is_array_type(source)? {
            let element = self.get_type_arguments(source)?[0];
            let Some(element) = self.infer_reverse_property(element, target, constraint)? else {
                return Ok(None);
            };
            return self
                .create_array_type(element, self.readonly_array_or_tuple(source)?)
                .map(Some);
        }
        if self.is_tuple_type(source)? {
            let elements = self.element_types(source)?;
            let mut types = Vec::with_capacity(elements.len());
            let mut complete = true;
            for &element in elements.iter() {
                match self.infer_reverse_property(element, target, constraint)? {
                    Some(ty) => types.push(ty),
                    None => complete = false,
                }
            }
            if !complete {
                return Ok(None);
            }
            let data = self.types.tuple(self.types.target(source)?)?;
            let readonly = data.readonly;
            let mut infos = data.element_infos.to_vec();
            if self.mapped_modifiers(target)? & crate::mapped::INCLUDE_OPTIONAL != 0 {
                for info in &mut infos {
                    if info.flags & ef::OPTIONAL != 0 {
                        info.flags = ef::REQUIRED;
                    }
                }
            }
            return self
                .create_tuple_type_ex(&types, &infos, readonly)
                .map(Some);
        }
        let ty = self.new_object_type(of::REVERSE_MAPPED | of::ANONYMOUS, None)?;
        let data = self.types.reverse_mapped_mut(ty)?;
        data.source = Some(source);
        data.mapped_type = Some(target);
        data.constraint_type = Some(constraint);
        Ok(Some(ty))
    }

    // port: tsc/internal/checker/inference.go:Checker.isPartiallyInferableType
    fn partially_inferable_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        let flags = self.types.object_flags(ty)?;
        if flags & of::NON_INFERRABLE_TYPE == 0 {
            return Ok(true);
        }
        if flags & of::OBJECT_LITERAL != 0 {
            for property in self.get_properties_of_type(ty)? {
                let ty = self.get_type_of_symbol(property)?;
                if self.partially_inferable_type(ty)? {
                    return Ok(true);
                }
            }
        }
        if self.is_tuple_type(ty)? {
            for &ty in self.element_types(ty)?.iter() {
                if self.partially_inferable_type(ty)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/inference.go:Checker.inferReverseMappedType
    fn infer_reverse_property(
        &mut self,
        source: TypeId,
        target: TypeId,
        constraint: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        let key = (source, target, constraint);
        if let Some(cached) = self.inference.reverse.properties.get(&key) {
            return Ok(Some(cached.unwrap_or(self.builtins.unknown_type)));
        }
        self.inference.reverse.source_stack.push(source);
        self.inference.reverse.target_stack.push(target);
        let saved = self.inference.reverse.expanding;
        let result = (|| {
            if self.inference.reverse.source_stack.len() >= 2 && {
                let stack = self.inference.reverse.source_stack.clone();
                self.deeply_nested_type(source, &stack, 2)?
            } {
                self.inference.reverse.expanding |= crate::relater::SOURCE;
            }
            if self.inference.reverse.target_stack.len() >= 2 && {
                let stack = self.inference.reverse.target_stack.clone();
                self.deeply_nested_type(target, &stack, 2)?
            } {
                self.inference.reverse.expanding |= crate::relater::TARGET;
            }
            if self.inference.reverse.expanding == crate::relater::BOTH {
                return Ok(None);
            }
            let parameter = self.mapped_parameter(target)?;
            let parameter = self.get_indexed_access_type(
                self.types.target(constraint)?,
                parameter,
                0,
                None,
                None,
            )?;
            let template = self.mapped_template(target)?;
            let context = self.new_inference_context(&[parameter], None, 0)?;
            self.infer_types(
                context,
                source,
                template,
                crate::inference::priority::NONE,
                false,
            )?;
            let info = &self.inference_context(context)?.inferences[0];
            let ty = if !info.candidates.is_empty() {
                let candidates = info.candidates.clone();
                self.get_union_type_ex(&candidates, crate::UnionReduction::Subtype, None, None)?
            } else if !info.contra_candidates.is_empty() {
                let candidates = info.contra_candidates.clone();
                self.get_intersection_type(&candidates)?
            } else {
                self.builtins.unknown_type
            };
            self.widened_inference_type(ty).map(Some)
        })();
        self.inference.reverse.source_stack.pop();
        self.inference.reverse.target_stack.pop();
        self.inference.reverse.expanding = saved;
        if let Ok(result) = result {
            self.inference.reverse.properties.insert(key, result);
        }
        result
    }

    // port: tsc/internal/checker/inference.go:Checker.resolveReverseMappedTypeMembers
    pub(crate) fn resolve_reverse_mapped_members(&mut self, ty: TypeId) -> Result<(), Error> {
        let (source, mapped, constraint) = self.reverse_mapped_parts(ty)?;
        let index = self.index_info_of_type(source, self.builtins.string_type)?;
        let modifiers = self.mapped_modifiers(mapped)?;
        let readonly_mask = modifiers & crate::mapped::INCLUDE_READONLY == 0;
        let optional_mask = if modifiers & crate::mapped::INCLUDE_OPTIONAL != 0 {
            0
        } else {
            sf::OPTIONAL
        };
        let mut indexes = Vec::new();
        if let Some(index) = index {
            let info = self.signatures.index_info(index)?.clone();
            let value = self
                .infer_reverse_property(info.value_type, mapped, constraint)?
                .unwrap_or(self.builtins.unknown_type);
            indexes.push(self.signatures.new_index_info(
                self.builtins.string_type,
                value,
                readonly_mask && info.is_readonly,
                None,
                None,
            )?);
        }
        let mut members = SymbolTable::default();
        let limited = self.limited_reverse_constraint(mapped, constraint)?;
        for property in self.get_properties_of_type(source)? {
            if let Some(limited) = limited {
                let key = self
                    .literal_type_from_property(property, tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE)?;
                if !self.is_type_related_to(key, limited, RelationKind::Assignable)? {
                    continue;
                }
            }
            let checks = cf::REVERSE_MAPPED
                | if readonly_mask && self.is_readonly_symbol(property)? {
                    cf::READONLY
                } else {
                    0
                };
            let read = self.symbol(property)?;
            let flags = read.flags();
            let name = read.name_to_owned();
            let declarations = read.declarations();
            let inferred =
                self.new_symbol_ex(sf::PROPERTY | flags & optional_mask, name.clone(), checks)?;
            self.symbol_mut(inferred)?.declarations = declarations;
            // Go evaluates the Get on the assignment's left before the right.
            let inferred_key = self.value_symbol_key(inferred)?;
            let name_type = self
                .value_symbol_links
                .get_or_default(self.value_symbol_key(property)?)
                .name_type;
            self.value_symbol_links
                .get_or_default(inferred_key)
                .name_type = name_type;
            let value = self.get_type_of_symbol(property)?;
            let target = self.types.target(constraint)?;
            let (mapped, constraint) = if self.types.flags(target)? & tf::INDEXED_ACCESS != 0 {
                let data = *self.types.indexed_access(target)?;
                if self.types.flags(data.object_type)?
                    & self.types.flags(data.index_type)?
                    & tf::TYPE_PARAMETER
                    != 0
                {
                    let zero = self.get_number_literal_type(tsr_jsnum::Number::new(0.0))?;
                    let tuple = self.create_tuple_type(&[data.object_type])?;
                    let mapper =
                        self.new_type_mapper(&[data.index_type, data.object_type], &[zero, tuple])?;
                    (
                        self.instantiate_type(mapped, Some(mapper))?,
                        self.get_index_type(data.object_type, 0)?,
                    )
                } else {
                    (mapped, constraint)
                }
            } else {
                (mapped, constraint)
            };
            *self.inference.reverse.symbols.get_or_default(inferred) =
                Some((value, mapped, constraint));
            members.insert(name, Some(inferred));
        }
        let members = self.alloc_symbol_table(members);
        self.set_structured_type_members(ty, Some(members), &[], &[], &indexes)
    }

    // port: tsc/internal/checker/inference.go:Checker.getLimitedConstraint
    fn limited_reverse_constraint(
        &mut self,
        mapped: TypeId,
        constraint: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        let mut origin = self.mapped_constraint(mapped)?;
        let flags = self.types.flags(origin)?;
        if flags & tf::UNION_OR_INTERSECTION == 0 {
            return Ok(None);
        }
        if flags & tf::UNION != 0 {
            let Some(ty) = self.types.union(origin)?.origin else {
                return Ok(None);
            };
            origin = ty;
        }
        if self.types.flags(origin)? & tf::INTERSECTION == 0 {
            return Ok(None);
        }
        let types: Vec<_> = self
            .types
            .types_of(origin)?
            .iter()
            .copied()
            .filter(|ty| *ty != constraint)
            .collect();
        let result = self.get_intersection_type(&types)?;
        Ok((result != self.builtins.never_type).then_some(result))
    }

    // port: tsc/internal/checker/inference.go:Checker.getTypeOfReverseMappedSymbol
    pub(crate) fn type_of_reverse_mapped_symbol(
        &mut self,
        symbol: SymbolId,
    ) -> Result<TypeId, Error> {
        if let Some(ty) = self
            .value_symbol_links
            .get_or_default(self.value_symbol_key(symbol)?)
            .resolved_type
        {
            return Ok(ty);
        }
        let (source, mapped, constraint) = self
            .inference
            .reverse
            .symbols
            .try_get(symbol)
            .copied()
            .flatten()
            .ok_or(Error::MissingLink("reverse mapped symbol links"))?;
        let ty = self
            .infer_reverse_property(source, mapped, constraint)?
            .unwrap_or(self.builtins.unknown_type);
        self.value_symbol_links
            .get_or_default(self.value_symbol_key(symbol)?)
            .resolved_type = Some(ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateReverseMappedType
    pub(crate) fn instantiate_reverse_mapped(
        &mut self,
        ty: TypeId,
        mapper: MapperId,
    ) -> Result<TypeId, Error> {
        let (source, mapped, constraint) = self.reverse_mapped_parts(ty)?;
        let mapped = self.instantiate_type(mapped, Some(mapper))?;
        if self.types.object_flags(mapped)? & of::MAPPED == 0 {
            return Ok(ty);
        }
        let constraint = self.instantiate_type(constraint, Some(mapper))?;
        if self.types.flags(constraint)? & tf::INDEX == 0 {
            return Ok(ty);
        }
        let source = self.instantiate_type(source, Some(mapper))?;
        Ok(self
            .infer_homomorphic_type(source, mapped, constraint)?
            .unwrap_or(ty))
    }
}

#[cfg(any(test, feature = "storage-pilot"))]
impl ReverseInference {
    pub(crate) fn census(&self, census: &mut crate::census::Census) {
        census.map("inference", &self.homomorphic);
        census.map("inference", &self.properties);
        census.links("inference", &self.symbols);
        census.vec_capacity(
            "inference",
            &self.source_stack,
            self.source_stack.capacity(),
        );
        census.vec_capacity(
            "inference",
            &self.target_stack,
            self.target_stack.capacity(),
        );
    }
}

#[cfg(any(test, feature = "storage-pilot"))]
impl ReverseInference {
    pub(crate) fn census_type_roots(&self, roots: &mut Vec<TypeId>) {
        for map in [&self.homomorphic, &self.properties] {
            for (&(source, target, constraint), &value) in map {
                roots.extend([source, target, constraint]);
                roots.extend(value);
            }
        }
        for &(source, target, constraint) in self.symbols.values().flatten() {
            roots.extend([source, target, constraint]);
        }
        roots.extend_from_slice(&self.source_stack);
        roots.extend_from_slice(&self.target_stack);
    }
}
