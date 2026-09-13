//! Spread folding follows source order; optional right properties retain both
//! original symbols for later elaboration and the native diagnostic provenance.
use crate::{
    object_flags as of, type_flags as tf, CheckerState, Error, IndexInfoId, TypeId, UnionReduction,
};
use ts_arena::SymbolId;
use ts_ast::{check_flags as cf, modifier_flags as mf, symbol_flags as sf, SymbolTable};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getSpreadType
    pub(crate) fn object_spread_type(
        &mut self,
        left: TypeId,
        right: TypeId,
        symbol: Option<SymbolId>,
        flags: u32,
        readonly: bool,
    ) -> Result<TypeId, Error> {
        let lf = self.types.flags(left)?;
        let rf = self.types.flags(right)?;
        if (lf | rf) & tf::ANY != 0 {
            return Ok(self.builtins.any_type);
        }
        if (lf | rf) & tf::UNKNOWN != 0 {
            return Ok(self.builtins.unknown_type);
        }
        if lf & tf::NEVER != 0 {
            return Ok(right);
        }
        if rf & tf::NEVER != 0 {
            return Ok(left);
        }
        let left = self.merge_spread_union(left, readonly)?;
        if self.types.flags(left)? & tf::UNION != 0 {
            if !self.check_cross_product_union(&[left, right])? {
                return Ok(self.builtins.error_type);
            }
            return self
                .map_type(left, &mut |checker, part| {
                    checker
                        .object_spread_type(part, right, symbol, flags, readonly)
                        .map(Some)
                })?
                .ok_or(Error::MissingLink("spread left union"));
        }
        let right = self.merge_spread_union(right, readonly)?;
        if self.types.flags(right)? & tf::UNION != 0 {
            if !self.check_cross_product_union(&[left, right])? {
                return Ok(self.builtins.error_type);
            }
            return self
                .map_type(right, &mut |checker, part| {
                    checker
                        .object_spread_type(left, part, symbol, flags, readonly)
                        .map(Some)
                })?
                .ok_or(Error::MissingLink("spread right union"));
        }
        if self.types.flags(right)?
            & (tf::BOOLEAN_LIKE
                | tf::NUMBER_LIKE
                | tf::BIG_INT_LIKE
                | tf::STRING_LIKE
                | tf::ENUM_LIKE
                | tf::NON_PRIMITIVE
                | tf::INDEX)
            != 0
        {
            return Ok(left);
        }
        if self.get_generic_object_flags(left)? & of::IS_GENERIC_OBJECT_TYPE != 0
            || self.get_generic_object_flags(right)? & of::IS_GENERIC_OBJECT_TYPE != 0
        {
            if self.empty_object_type(left)? {
                return Ok(right);
            }
            if self.types.flags(left)? & tf::INTERSECTION != 0 {
                let mut parts = self.types.compound_types(left)?.to_vec();
                let last = *parts
                    .last()
                    .ok_or(Error::MissingLink("spread intersection"))?;
                if self.types.flags(last)? & tf::OBJECT != 0
                    && !self.is_generic_mapped_type(last)?
                    && self.types.flags(right)? & tf::OBJECT != 0
                    && !self.is_generic_mapped_type(right)?
                {
                    let tail = self.object_spread_type(last, right, symbol, flags, readonly)?;
                    *parts
                        .last_mut()
                        .ok_or(Error::MissingLink("spread intersection"))? = tail;
                    return self.get_intersection_type(&parts);
                }
            }
            return self.get_intersection_type(&[left, right]);
        }
        let mut members = SymbolTable::default();
        let mut skipped = crate::types::Map::default();
        let indices = if left == self.builtins.empty_object_type {
            self.index_infos_of_type(right)?
        } else {
            self.spread_union_indices(&[left, right])?
        };
        for property in self.get_properties_of_type(right)? {
            let name = self.symbol(property)?.name_to_owned();
            if self.rest_declaration_modifiers(property)? & mf::NON_PUBLIC_ACCESSIBILITY_MODIFIER
                != 0
            {
                skipped.insert(name, ());
            } else if self.spreadable_property(property)? {
                let property = self.spread_symbol(property, readonly)?;
                members.insert(name, Some(property));
            }
        }
        for left_property in self.get_properties_of_type(left)? {
            let name = self.symbol(left_property)?.name_to_owned();
            if skipped.contains_key(&name) || !self.spreadable_property(left_property)? {
                continue;
            }
            if let Some(right_property) = members.get(name.as_bytes()).copied().flatten() {
                let right_type = self.get_type_of_symbol(right_property)?;
                if self.symbol(right_property)?.flags() & sf::OPTIONAL != 0 {
                    let mut declarations = self
                        .symbol_declarations(left_property)?
                        .iter()
                        .collect::<Vec<_>>();
                    declarations.extend(self.symbol_declarations(right_property)?.iter());
                    let property = self.new_symbol(
                        sf::PROPERTY | (self.symbol(left_property)?.flags() & sf::OPTIONAL),
                        name.clone(),
                    )?;
                    let left_type = self.get_type_of_symbol(left_property)?;
                    let left_without = self.remove_missing_or_undefined(left_type)?;
                    let right_without = self.remove_missing_or_undefined(right_type)?;
                    let value = if left_without == right_without {
                        left_type
                    } else {
                        self.get_union_type_ex(
                            &[left_type, right_without],
                            UnionReduction::Subtype,
                            None,
                            None,
                        )?
                    };
                    let name_type = self
                        .value_symbol_links
                        .try_get(left_property)
                        .and_then(|links| links.name_type);
                    let links = self.value_symbol_links.get_or_default(property);
                    links.resolved_type = Some(value);
                    links.name_type = name_type;
                    self.bindings
                        .spread_links
                        .insert(property, (left_property, right_property));
                    let declarations = self.declarations.alloc(declarations)?;
                    self.symbol_mut(property)?.declarations = declarations;
                    members.insert(name, Some(property));
                }
            } else {
                let property = self.spread_symbol(left_property, readonly)?;
                members.insert(name, Some(property));
            }
        }
        let mut spread_indices = Vec::with_capacity(indices.len());
        for index in indices {
            let info = self.signatures.index_info(index)?.clone();
            spread_indices.push(if info.is_readonly == readonly {
                index
            } else {
                self.signatures.new_index_info(
                    info.key_type,
                    info.value_type,
                    readonly,
                    info.declaration,
                    info.components,
                )?
            });
        }
        let members = self.alloc_symbol_table(members);
        let result = self.new_anonymous_type(symbol, Some(members), &[], &[], &spread_indices)?;
        self.types.get_mut(result)?.object_flags |=
            of::OBJECT_LITERAL | of::CONTAINS_OBJECT_OR_ARRAY_LITERAL | of::CONTAINS_SPREAD | flags;
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.getUnionIndexInfos
    pub(crate) fn spread_union_indices(
        &mut self,
        types: &[TypeId],
    ) -> Result<Vec<IndexInfoId>, Error> {
        let mut result = Vec::new();
        for index in self.index_infos_of_type(types[0])? {
            let key = self.signatures.index_info(index)?.key_type;
            let mut values = Vec::new();
            let mut readonly = false;
            let mut missing = false;
            for &ty in types {
                if let Some(info) = self.index_info_of_type(ty, key)? {
                    let info = self.signatures.index_info(info)?;
                    values.push(info.value_type);
                    readonly |= info.is_readonly;
                } else {
                    missing = true;
                    break;
                }
            }
            if !missing {
                let value = self.get_union_type(&values)?;
                result.push(
                    self.signatures
                        .new_index_info(key, value, readonly, None, None)?,
                );
            }
        }
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.tryMergeUnionOfObjectTypeAndEmptyObject
    pub(crate) fn merge_spread_union(
        &mut self,
        ty: TypeId,
        readonly: bool,
    ) -> Result<TypeId, Error> {
        if self.types.flags(ty)? & tf::UNION == 0 {
            return Ok(ty);
        }
        let parts = self.types.compound_types(ty)?.clone();
        let mut first = None;
        let mut all_empty = true;
        for &part in parts.iter() {
            if !self.empty_spread_type(part)? {
                all_empty = false;
                if first.is_none() {
                    first = Some(part);
                } else if first != Some(part) {
                    return Ok(ty);
                }
            }
        }
        if all_empty {
            for &part in parts.iter() {
                if self.empty_object_type(part)? {
                    return Ok(part);
                }
            }
            return Ok(self.builtins.empty_object_type);
        }
        let Some(first) = first else {
            return Ok(ty);
        };
        let mut members = SymbolTable::default();
        for source in self.get_properties_of_type(first)? {
            if self.rest_declaration_modifiers(source)? & mf::NON_PUBLIC_ACCESSIBILITY_MODIFIER != 0
                || !self.spreadable_property(source)?
            {
                continue;
            }
            let read = self.symbol(source)?;
            let name = read.name_to_owned();
            let declarations = read.declarations();
            let setonly =
                read.flags() & sf::SET_ACCESSOR != 0 && read.flags() & sf::GET_ACCESSOR == 0;
            let checks = read.check_flags() & cf::LATE | if readonly { cf::READONLY } else { 0 };
            let property = self.new_symbol_ex(sf::PROPERTY | sf::OPTIONAL, name.clone(), checks)?;
            let value = if setonly {
                self.builtins.undefined_type
            } else {
                let value = self.get_type_of_symbol(source)?;
                self.add_type_optionality(value, true, true)?
            };
            let name_type = self
                .value_symbol_links
                .try_get(source)
                .and_then(|links| links.name_type);
            let links = self.value_symbol_links.get_or_default(property);
            links.resolved_type = Some(value);
            links.name_type = name_type;
            self.symbol_mut(property)?.declarations = declarations;
            self.mapped_symbol_links
                .get_or_default(property)
                .synthetic_origin = Some(source);
            members.insert(name, Some(property));
        }
        let members = self.alloc_symbol_table(members);
        let indices = self.index_infos_of_type(first)?;
        let result = self.new_anonymous_type(
            self.types.get(first)?.symbol,
            Some(members),
            &[],
            &[],
            &indices,
        )?;
        self.types.get_mut(result)?.object_flags |=
            of::OBJECT_LITERAL | of::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.isEmptyObjectTypeOrSpreadsIntoEmptyObject
    fn empty_spread_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        Ok(self.empty_object_type(ty)?
            || self.types.flags(ty)?
                & (tf::NULL
                    | tf::UNDEFINED
                    | tf::BOOLEAN_LIKE
                    | tf::NUMBER_LIKE
                    | tf::BIG_INT_LIKE
                    | tf::STRING_LIKE
                    | tf::ENUM_LIKE
                    | tf::NON_PRIMITIVE
                    | tf::INDEX)
                != 0)
    }
}
