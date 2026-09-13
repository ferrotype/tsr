//! Shared type operations used by the production relations and discriminants.
use crate::{object_flags as of, type_flags as tf, CheckerState, Error, TypeId};
use ts_ast::{check_flags as cf, symbol_flags as sf, JsString, SymbolTable};

impl CheckerState {
    // port: tsc/internal/checker/relater.go:Checker.getEffectiveConstraintOfIntersection
    pub(crate) fn effective_intersection_constraint(
        &mut self,
        types: &[TypeId],
        target_union: bool,
    ) -> Result<Option<TypeId>, Error> {
        let mut constraints = Vec::new();
        let mut disjoint = false;
        for &ty in types {
            if self.types.flags(ty)? & tf::INSTANTIABLE != 0 {
                let mut constraint = self.constraint_of_type(ty)?;
                while let Some(current) = constraint {
                    if self.types.flags(current)?
                        & (tf::TYPE_PARAMETER | tf::INDEX | tf::CONDITIONAL)
                        == 0
                    {
                        break;
                    }
                    constraint = self.constraint_of_type(current)?;
                }
                if let Some(constraint) = constraint {
                    constraints.push(constraint);
                    if target_union {
                        constraints.push(ty);
                    }
                }
            } else if self.types.flags(ty)? & tf::DISJOINT_DOMAINS != 0
                || self.is_empty_anonymous_object_type(ty)?
            {
                disjoint = true;
            }
        }
        if constraints.is_empty() || !target_union && !disjoint {
            return Ok(None);
        }
        if disjoint {
            for &ty in types {
                if self.types.flags(ty)? & tf::DISJOINT_DOMAINS != 0
                    || self.is_empty_anonymous_object_type(ty)?
                {
                    constraints.push(ty);
                }
            }
        }
        let constraint = self.get_intersection_type_ex(
            &constraints,
            crate::intersection::NO_CONSTRAINT_REDUCTION,
            None,
        )?;
        self.normalized_type(constraint, false).map(Some)
    }

    pub(crate) fn filter_type_flags(
        &mut self,
        ty: TypeId,
        flags: crate::TypeFlags,
    ) -> Result<TypeId, Error> {
        self.filter_type(ty, &mut |state, part| {
            Ok(state.types.flags(part)? & flags != 0)
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getNonMissingTypeOfSymbol
    pub(crate) fn non_missing_symbol_type(
        &mut self,
        symbol: ts_arena::SymbolId,
    ) -> Result<TypeId, Error> {
        let ty = self.get_type_of_symbol(symbol)?;
        self.remove_missing_type(ty, self.symbol(symbol)?.flags() & sf::OPTIONAL != 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.removeMissingType
    pub(crate) fn remove_missing_type(
        &mut self,
        ty: TypeId,
        optional: bool,
    ) -> Result<TypeId, Error> {
        if optional && self.options.exact_optional_property_types {
            let missing = self.builtins.missing_type;
            self.filter_type(ty, &mut |_, part| Ok(part != missing))
        } else {
            Ok(ty)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.createSymbolWithType
    pub(crate) fn create_symbol_with_type(
        &mut self,
        source: ts_arena::SymbolId,
        ty: TypeId,
    ) -> Result<ts_arena::SymbolId, Error> {
        self.clone_symbol_with_type(source, Some(ty))
    }

    pub(crate) fn clone_symbol_with_type(
        &mut self,
        source: ts_arena::SymbolId,
        ty: Option<TypeId>,
    ) -> Result<ts_arena::SymbolId, Error> {
        let read = self.symbol(source)?;
        let flags = read.flags();
        let name = read.name_to_owned();
        let check_flags = read.check_flags() & cf::READONLY;
        let declarations = read.declarations();
        let parent = read.parent();
        let declaration = read.value_declaration();
        let name_type = self
            .value_symbol_links
            .try_get(source)
            .and_then(|links| links.name_type);
        let symbol = self.new_symbol_ex(flags, name, check_flags)?;
        let record = self.symbol_mut(symbol)?;
        record.declarations = declarations;
        record.parent = parent;
        record.value_declaration = declaration;
        let links = self.value_symbol_links.get_or_default(symbol);
        links.resolved_type = ty;
        links.target = Some(source);
        links.name_type = name_type;
        Ok(symbol)
    }

    // port: tsc/internal/checker/checker.go:Checker.getRegularTypeOfObjectLiteral
    pub(crate) fn regular_object_literal_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let record = *self.types.get(ty)?;
        if record.object_flags & (of::OBJECT_LITERAL | of::FRESH_LITERAL)
            != (of::OBJECT_LITERAL | of::FRESH_LITERAL)
        {
            return Ok(ty);
        }
        if let Some(&regular) = self.types.caches.regular_object_literals.get(&ty) {
            return Ok(regular);
        }
        self.resolve_type_members(ty)?;
        let members = self.types.structured(ty)?;
        let signatures = members.signatures.clone().unwrap_or_else(|| [].into());
        let calls = members.call_signature_count as usize;
        let indexes = members.index_infos.clone().unwrap_or_else(|| [].into());
        let mut table = SymbolTable::new();
        for mut property in self.get_properties_of_type(ty)? {
            let original = self.get_type_of_symbol(property)?;
            let updated = self.regular_object_literal_type(original)?;
            if original != updated {
                property = self.create_symbol_with_type(property, updated)?;
            }
            table.insert(self.symbol(property)?.name_to_owned(), Some(property));
        }
        let table = self.alloc_symbol_table(table);
        let regular = self.new_anonymous_type(
            record.symbol,
            Some(table),
            &signatures[..calls],
            &signatures[calls..],
            &indexes,
        )?;
        self.types.get_mut(regular)?.flags = record.flags;
        self.types.get_mut(regular)?.object_flags |=
            self.types.get(ty)?.object_flags & !of::FRESH_LITERAL;
        self.types
            .caches
            .regular_object_literals
            .insert(ty, regular);
        Ok(regular)
    }

    // port: tsc/internal/checker/relater.go:Checker.getMatchingUnionConstituentForType
    pub(crate) fn matching_union_constituent(
        &mut self,
        union: TypeId,
        source: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        if self.types.union(union)?.key_property_name.is_none() {
            self.compute_key_property_name(union)?;
        }
        let name = self
            .types
            .union(union)?
            .key_property_name
            .clone()
            .unwrap_or_default();
        if name.as_bytes().is_empty() {
            return Ok(None);
        }
        let Some(property) = self.constituent_property(source, name.as_bytes(), false)? else {
            return Ok(None);
        };
        let ty = self.get_type_of_symbol(property)?;
        let ty = self.get_regular_type_of_literal_type(ty)?;
        Ok(self
            .types
            .union(union)?
            .constituent_map
            .as_ref()
            .and_then(|map| map.get(&ty))
            .copied()
            .filter(|&ty| ty != self.builtins.unknown_type))
    }

    // port: tsc/internal/checker/relater.go:Checker.computeKeyPropertyNameAndMap
    pub(crate) fn compute_key_property_name(&mut self, union: TypeId) -> Result<(), Error> {
        let types = self.types.compound_types(union)?.clone();
        self.types.union_mut(union)?.key_property_name = Some(JsString::default());
        let result = (|| {
            let mut count = 0;
            for &part in types.iter() {
                if self.types.flags(part)? & (tf::OBJECT | tf::INSTANTIABLE_NON_PRIMITIVE) != 0 {
                    count += 1;
                }
            }
            if types.len() < 10
                || self.types.get(union)?.object_flags & of::PRIMITIVE_UNION != 0
                || count < 10
            {
                return Ok(None);
            }
            let mut candidate = None;
            'types: for &part in types.iter() {
                if self.types.flags(part)? & (tf::OBJECT | tf::INSTANTIABLE_NON_PRIMITIVE) != 0 {
                    for property in self.get_properties_of_type(part)? {
                        let ty = self.get_type_of_symbol(property)?;
                        if self.types.flags(ty)? & tf::UNIT != 0 {
                            candidate = Some(self.symbol(property)?.name_to_owned());
                            break 'types;
                        }
                    }
                }
            }
            let Some(name) = candidate else {
                return Ok(None);
            };
            let Some(map) = self.map_types_by_key_property(&types, name.as_bytes())? else {
                return Ok(None);
            };
            Ok(Some((name, map)))
        })();
        match result {
            Ok(Some((name, map))) => {
                self.types.union_mut(union)?.key_property_name = Some(name);
                self.types.union_mut(union)?.constituent_map = Some(Box::new(map));
            }
            Ok(None) => {}
            Err(error) => {
                self.types.union_mut(union)?.key_property_name = None;
                return Err(error);
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/relater.go:Checker.mapTypesByKeyProperty
    fn map_types_by_key_property(
        &mut self,
        types: &[TypeId],
        name: &[u8],
    ) -> Result<Option<crate::types::Map<TypeId, TypeId>>, Error> {
        let mut map = crate::types::Map::default();
        let mut count = 0;
        for &ty in types {
            if self.types.flags(ty)?
                & (tf::OBJECT | tf::INTERSECTION | tf::INSTANTIABLE_NON_PRIMITIVE)
                != 0
            {
                let Some(property) = self.constituent_property(ty, name, false)? else {
                    return Ok(None);
                };
                let discriminant = self.get_type_of_symbol(property)?;
                if !self.is_literal_type(discriminant)? {
                    return Ok(None);
                }
                let parts = if self.types.flags(discriminant)? & tf::UNION != 0 {
                    self.types.compound_types(discriminant)?.clone()
                } else {
                    vec![discriminant].into()
                };
                let mut duplicate = false;
                for &part in parts.iter() {
                    let key = self.get_regular_type_of_literal_type(part)?;
                    if let Some(value) = map.get_mut(&key) {
                        if *value != self.builtins.unknown_type {
                            *value = self.builtins.unknown_type;
                            duplicate = true;
                        }
                    } else {
                        map.insert(key, ty);
                    }
                }
                if !duplicate {
                    count += 1;
                }
            }
        }
        Ok((count >= 10 && count * 2 >= types.len()).then_some(map))
    }
}
