//! Property discovery over source object types, unions and intersections. All synthesized
//! symbols, declaration lists and cache entries belong to the checker.

use crate::{object_flags as of, type_flags as tf, CheckerState, Error, TypeId};
use std::collections::HashSet;
use ts_arena::SymbolId;
use ts_ast::{check_flags as cf, modifier_flags as mf, symbol_flags as sf, JsString, SymbolTable};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getPropertiesOfType
    pub(crate) fn get_properties_of_type(&mut self, ty: TypeId) -> Result<Vec<SymbolId>, Error> {
        let ty = self.reduced_apparent_type(ty)?;
        let flags = self.types.flags(ty)?;
        if flags & tf::UNION_OR_INTERSECTION != 0 && flags & tf::BOOLEAN == 0 {
            return self.get_properties_of_union_or_intersection_type(ty);
        }
        if flags & tf::OBJECT == 0 {
            return self.properties_of_primitive_type(ty);
        }
        self.resolve_type_members(ty)?;
        Ok(self
            .types
            .structured(ty)?
            .properties
            .as_deref()
            .unwrap_or_default()
            .to_vec())
    }

    // port: tsc/internal/checker/checker.go:Checker.getPropertiesOfUnionOrIntersectionType
    pub(crate) fn get_properties_of_union_or_intersection_type(
        &mut self,
        ty: TypeId,
    ) -> Result<Vec<SymbolId>, Error> {
        if let Some(properties) = &self.types.compound_members(ty)?.resolved_properties {
            return Ok(properties.to_vec());
        }
        let is_union = self.types.flags(ty)? & tf::UNION != 0;
        let types = self.types.compound_types(ty)?.clone();
        let mut checked = HashSet::new();
        let mut properties = Vec::new();
        for &current in types.iter() {
            for prop in self.get_properties_of_type(current)? {
                let name = self.symbol(prop)?.name_to_owned();
                if checked.insert(name.clone()) {
                    if let Some(property) = self.get_compound_property(ty, name, !is_union)? {
                        properties.push(property);
                    }
                }
            }
            // Continue past an index-signature constituent: later explicit
            // properties may be supplied by that index signature.
            if is_union && self.index_infos_of_type(current)?.is_empty() {
                break;
            }
        }
        self.types.compound_members_mut(ty)?.resolved_properties = Some(properties.clone().into());
        Ok(properties)
    }

    // port: tsc/internal/checker/checker.go:Checker.getPropertyOfTypeEx
    pub(crate) fn constituent_property(
        &mut self,
        ty: TypeId,
        name: &[u8],
        skip_augment: bool,
    ) -> Result<Option<SymbolId>, Error> {
        self.constituent_property_ex(ty, name, skip_augment, false)
    }

    pub(crate) fn constituent_property_ex(
        &mut self,
        ty: TypeId,
        name: &[u8],
        skip_augment: bool,
        include_type_only: bool,
    ) -> Result<Option<SymbolId>, Error> {
        let ty = self.reduced_apparent_type(ty)?;
        let flags = self.types.flags(ty)?;
        if flags & tf::INTERSECTION != 0 {
            if let Some(prop) = self.get_compound_property(ty, JsString::from_bytes(name), true)? {
                return Ok(Some(prop));
            }
            if !skip_augment {
                return self.get_compound_property(ty, JsString::from_bytes(name), false);
            }
            return Ok(None);
        }
        if flags & tf::UNION != 0 && flags & tf::BOOLEAN == 0 {
            return self.get_compound_property(ty, JsString::from_bytes(name), skip_augment);
        }
        let object = if flags & tf::OBJECT != 0 {
            Some(ty)
        } else {
            self.apparent_primitive_type(ty)?
        };
        if let Some(ty) = object {
            if let Some(prop) = self.object_property(ty, name, include_type_only)? {
                return Ok(Some(prop));
            }
            if skip_augment {
                return Ok(None);
            }
            let members = self.types.structured(ty)?;
            let function = if ty == self.builtins.any_function_type {
                Some("Function")
            } else if members.call_signature_count != 0 {
                Some("CallableFunction")
            } else if members.signatures.as_ref().is_some_and(|s| !s.is_empty()) {
                Some("NewableFunction")
            } else {
                None
            };
            if let Some(function) =
                function.and_then(|name| self.query.global_types.get(name).copied())
            {
                if let Some(property) = self.object_property(function, name, false)? {
                    return Ok(Some(property));
                }
            }
            if let Some(&object) = self.query.global_types.get("Object") {
                if object != ty {
                    return self.object_property(object, name, false);
                }
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getPropertyOfObjectType
    fn object_property(
        &mut self,
        ty: TypeId,
        name: &[u8],
        include_type_only: bool,
    ) -> Result<Option<SymbolId>, Error> {
        self.resolve_type_members(ty)?;
        let Some(table) = self.types.structured(ty)?.members else {
            return Ok(None);
        };
        let Some(prop) = self.table(table)?.get(name).flatten() else {
            return Ok(None);
        };
        if !include_type_only {
            if let Some(symbol) = self.types.get(ty)?.symbol {
                if self.symbol(symbol)?.flags() & sf::VALUE_MODULE != 0
                    && self
                        .module_aliases
                        .type_only_exports
                        .get(&symbol)
                        .is_some_and(|table| table.contains_key(name))
                {
                    return Ok(None);
                }
            }
        }
        Ok(self
            .symbol_is_value_ex(prop, include_type_only)?
            .then_some(prop))
    }

    // port: tsc/internal/checker/checker.go:Checker.getPropertyOfUnionOrIntersectionType
    // port: tsc/internal/checker/checker.go:Checker.getUnionOrIntersectionProperty
    fn get_compound_property(
        &mut self,
        ty: TypeId,
        name: JsString,
        skip_augment: bool,
    ) -> Result<Option<SymbolId>, Error> {
        let common = self.types.compound_members(ty)?;
        let cache = if skip_augment {
            common.property_cache_without_function_property_augment
        } else {
            common.property_cache
        };
        if let Some(cache) = cache {
            if let Some(prop) = self.table(cache)?.get(name.as_bytes()).flatten() {
                return Ok(
                    (self.symbol(prop)?.check_flags() & cf::READ_PARTIAL == 0).then_some(prop)
                );
            }
        }
        let Some(prop) = self.create_compound_property(ty, &name, skip_augment)? else {
            return Ok(None);
        };
        let cache = self.compound_property_cache(ty, skip_augment)?;
        self.tables.get_mut(cache)?.insert(name.clone(), Some(prop));
        let flags = self.symbol(prop)?.check_flags();
        if skip_augment && flags & cf::PARTIAL == 0 {
            let augmented = self.compound_property_cache(ty, false)?;
            if self
                .table(augmented)?
                .get(name.as_bytes())
                .flatten()
                .is_none()
            {
                self.tables.get_mut(augmented)?.insert(name, Some(prop));
            }
        }
        Ok((flags & cf::READ_PARTIAL == 0).then_some(prop))
    }

    fn compound_property_cache(
        &mut self,
        ty: TypeId,
        skip_augment: bool,
    ) -> Result<ts_ast::SymbolTableId, Error> {
        let common = self.types.compound_members(ty)?;
        let cache = if skip_augment {
            common.property_cache_without_function_property_augment
        } else {
            common.property_cache
        };
        if let Some(cache) = cache {
            return Ok(cache);
        }
        let cache = self.alloc_symbol_table(SymbolTable::new());
        let common = self.types.compound_members_mut(ty)?;
        if skip_augment {
            common.property_cache_without_function_property_augment = Some(cache);
        } else {
            common.property_cache = Some(cache);
        }
        Ok(cache)
    }

    // port: tsc/internal/checker/checker.go:Checker.createUnionOrIntersectionProperty
    fn create_compound_property(
        &mut self,
        containing: TypeId,
        name: &JsString,
        skip_augment: bool,
    ) -> Result<Option<SymbolId>, Error> {
        let types = self.types.compound_types(containing)?.clone();
        let is_union = self.types.flags(containing)? & tf::UNION != 0;
        let mut props = Vec::new();
        let mut first = None;
        let mut index_types = Vec::new();
        let mut optional = if is_union { 0 } else { sf::OPTIONAL };
        let mut flags = if is_union { 0 } else { cf::READONLY };
        let mut property_flags = 0;
        let mut synthetic = cf::SYNTHETIC_METHOD;
        let mut merged_instantiations = false;
        for &current in types.iter() {
            let current = self.apparent_type(current)?;
            if self.is_error_type(current)? || self.types.flags(current)? & tf::NEVER != 0 {
                continue;
            }
            if let Some(prop) = self.constituent_property(current, name.as_bytes(), skip_augment)? {
                let read = self.symbol(prop)?;
                let prop_flags = read.flags();
                let prop_check = read.check_flags();
                let modifiers = self.property_modifiers(prop)?;
                let write_modifiers = self.property_modifiers_ex(prop, true)?;
                if prop_flags & sf::CLASS_MEMBER != 0 {
                    if is_union {
                        optional |= prop_flags & sf::OPTIONAL;
                    } else {
                        optional &= prop_flags;
                    }
                }
                if let Some(single) = first {
                    if single != prop {
                        if self.target_symbol(prop)? == self.target_symbol(single)?
                            && self.compare_properties(single, prop, &mut |_, a, b| {
                                Ok(if a == b {
                                    crate::ternary::TRUE
                                } else {
                                    crate::ternary::FALSE
                                })
                            })? == crate::ternary::TRUE
                        {
                            merged_instantiations =
                                if let Some(parent) = self.symbol(single)?.parent() {
                                    !self.get_local_type_parameters(parent)?.is_empty()
                                } else {
                                    false
                                };
                        } else {
                            if props.is_empty() {
                                props.push(single);
                            }
                            if !props.contains(&prop) {
                                props.push(prop);
                            }
                        }
                        if property_flags & sf::ACCESSOR != 0
                            && prop_flags & sf::ACCESSOR != property_flags & sf::ACCESSOR
                        {
                            property_flags = property_flags & !sf::ACCESSOR | sf::PROPERTY;
                        }
                    }
                } else {
                    first = Some(prop);
                    property_flags = if prop_flags & sf::ACCESSOR != 0 {
                        prop_flags & sf::ACCESSOR
                    } else {
                        sf::PROPERTY
                    };
                }
                let readonly = self.is_readonly_symbol(prop)?;
                if is_union && readonly {
                    flags |= cf::READONLY;
                } else if !is_union && !readonly {
                    flags &= !cf::READONLY;
                }
                flags |= if modifiers & mf::PROTECTED != 0 && modifiers & mf::PUBLIC == 0 {
                    cf::CONTAINS_PROTECTED
                } else if modifiers & mf::PRIVATE != 0 && modifiers & mf::PUBLIC == 0 {
                    cf::CONTAINS_PRIVATE
                } else {
                    cf::CONTAINS_PUBLIC
                };
                flags |= if write_modifiers & mf::PROTECTED != 0
                    && write_modifiers & mf::PUBLIC == 0
                {
                    cf::CONTAINS_WRITE_PROTECTED
                } else if write_modifiers & mf::PRIVATE != 0 && write_modifiers & mf::PUBLIC == 0 {
                    cf::CONTAINS_WRITE_PRIVATE
                } else {
                    cf::CONTAINS_WRITE_PUBLIC
                };
                if modifiers & mf::STATIC != 0 {
                    flags |= cf::CONTAINS_STATIC;
                }
                if prop_flags & sf::METHOD == 0 && prop_check & cf::SYNTHETIC_METHOD == 0 {
                    synthetic = cf::SYNTHETIC_PROPERTY;
                }
            } else if is_union {
                let index = if name.as_bytes().starts_with(b"\xfe@") {
                    None
                } else {
                    let key = self.get_string_literal_type(name.clone())?;
                    self.applicable_index_info(current, key)?
                };
                if let Some(index) = index {
                    let info = self.signatures.index_info(index)?;
                    property_flags = property_flags & !sf::ACCESSOR | sf::PROPERTY;
                    flags |= cf::WRITE_PARTIAL | if info.is_readonly { cf::READONLY } else { 0 };
                    let mut value = info.value_type;
                    if self.is_tuple_type(current)? {
                        let fixed =
                            self.types.tuple(self.types.target(current)?)?.fixed_length as usize;
                        value = self
                            .tuple_slice_element_type(current, fixed, 0, false)?
                            .unwrap_or(self.builtins.undefined_type);
                    }
                    index_types.push(value);
                } else if self.types.get(current)?.object_flags & of::OBJECT_LITERAL != 0
                    && self.types.get(current)?.object_flags & of::CONTAINS_SPREAD == 0
                {
                    flags |= cf::WRITE_PARTIAL;
                    index_types.push(self.builtins.undefined_type);
                } else {
                    flags |= cf::READ_PARTIAL;
                }
            }
        }
        let Some(first) = first else {
            return Ok(None);
        };
        if is_union
            && (!props.is_empty() || flags & cf::PARTIAL != 0)
            && flags
                & (cf::CONTAINS_PRIVATE
                    | cf::CONTAINS_PROTECTED
                    | cf::CONTAINS_WRITE_PRIVATE
                    | cf::CONTAINS_WRITE_PROTECTED)
                != 0
            && (props.is_empty() || !self.common_property_declaration(&props)?)
        {
            if flags & (cf::CONTAINS_PRIVATE | cf::CONTAINS_PROTECTED) != 0 {
                return Ok(None);
            }
            if flags & cf::CONTAINS_WRITE_PRIVATE != 0 {
                flags &= !(cf::CONTAINS_WRITE_PUBLIC | cf::CONTAINS_WRITE_PROTECTED);
            } else if flags & cf::CONTAINS_WRITE_PROTECTED != 0 {
                flags &= !cf::CONTAINS_WRITE_PUBLIC;
            }
        }
        if props.is_empty() && flags & cf::READ_PARTIAL == 0 && index_types.is_empty() {
            if !merged_instantiations {
                return Ok(Some(first));
            }
            let links = self
                .value_symbol_links
                .try_get(first)
                .copied()
                .unwrap_or_default();
            let cloned = self.clone_symbol_with_type(first, links.resolved_type)?;
            if let Some(declaration) = self.symbol(first)?.value_declaration() {
                let source = self
                    .program()?
                    .bound(declaration)?
                    .node_binding(declaration)?
                    .and_then(|b| b.symbol)
                    .ok_or(Error::MissingLink("property declaration symbol"))?;
                let parent = self.symbol(source)?.parent();
                self.symbol_mut(cloned)?.parent = parent;
            }
            let write = self.write_type_of_symbol(first)?;
            let result = self.value_symbol_links.get_or_default(cloned);
            result.containing_type = Some(containing);
            result.mapper = links.mapper;
            result.write_type = Some(write);
            return Ok(Some(cloned));
        }
        if props.is_empty() {
            props.push(first);
        }
        let mut declarations = Vec::new();
        let mut prop_types = Vec::with_capacity(props.len());
        let mut write_types: Option<Vec<TypeId>> = None;
        let mut first_value = None;
        let mut nonuniform_value = false;
        let mut name_type = None;
        for &prop in &props {
            let value = self.symbol(prop)?.value_declaration();
            if first_value.is_none() {
                first_value = value;
            } else if value.is_some() && value != first_value {
                nonuniform_value = true;
            }
            for declaration in self.symbol_declarations(prop)?.iter() {
                if !declarations.contains(&declaration) {
                    declarations.push(declaration);
                }
            }
            let ty = self.get_type_of_symbol(prop)?;
            if let Some(&first_type) = prop_types.first() {
                if ty != first_type {
                    flags |= cf::HAS_NON_UNIFORM_TYPE;
                }
            } else {
                name_type = self
                    .value_symbol_links
                    .try_get(prop)
                    .and_then(|l| l.name_type);
            }
            let write = self.write_type_of_symbol(prop)?;
            if write_types.is_some() || write != ty {
                write_types
                    .get_or_insert_with(|| prop_types.clone())
                    .push(write);
            }
            if self.is_literal_type(ty)? || self.is_pattern_literal_type(ty)? {
                flags |= cf::HAS_LITERAL_TYPE;
            }
            if self.types.flags(ty)? & tf::NEVER != 0 && ty != self.builtins.unique_literal_type {
                flags |= cf::HAS_NEVER_TYPE;
            }
            prop_types.push(ty);
        }
        prop_types.extend(index_types);
        let declaration_list = if declarations.is_empty() {
            ts_ast::DeclarationSlice::empty()
        } else {
            self.declarations.alloc(declarations)?
        };
        let result =
            self.new_symbol_ex(property_flags | optional, name.clone(), flags | synthetic)?;
        self.symbol_mut(result)?.declarations = declaration_list;
        if !nonuniform_value {
            if let Some(value) = first_value {
                // Go inherits the raw declaration symbol's parent, before
                // checker-local merges or late-bound resolution.
                let source_symbol = self
                    .program()?
                    .bound(value)?
                    .node_binding(value)?
                    .and_then(|binding| binding.symbol)
                    .ok_or(Error::MissingLink("property declaration symbol"))?;
                let parent = self.symbol(source_symbol)?.parent();
                let result = self.symbol_mut(result)?;
                result.value_declaration = Some(value);
                result.parent = parent;
            }
        }
        let links = self.value_symbol_links.get_or_default(result);
        links.containing_type = Some(containing);
        links.name_type = name_type;
        if prop_types.len() > 2 {
            self.symbol_mut(result)?.check_flags |= cf::DEFERRED_TYPE;
            *self.query.deferred_property_types.get_or_default(result) = Some(prop_types.into());
            *self
                .query
                .deferred_property_write_types
                .get_or_default(result) = write_types.map(Into::into);
        } else {
            let ty = if is_union {
                self.get_union_type(&prop_types)?
            } else {
                self.get_intersection_type(&prop_types)?
            };
            self.value_symbol_links.get_or_default(result).resolved_type = Some(ty);
            if let Some(write) = write_types {
                let ty = if is_union {
                    self.get_union_type(&write)?
                } else {
                    self.get_intersection_type(&write)?
                };
                self.value_symbol_links.get_or_default(result).write_type = Some(ty);
            }
        }
        Ok(Some(result))
    }

    // port: tsc/internal/checker/checker.go:isLiteralType
    pub(crate) fn is_literal_type(&self, ty: TypeId) -> Result<bool, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::BOOLEAN != 0 {
            return Ok(true);
        }
        if flags & tf::UNION != 0 {
            if flags & tf::ENUM_LITERAL != 0 {
                return Ok(true);
            }
            for &ty in self.types.union(ty)?.types.iter() {
                if self.types.flags(ty)? & tf::UNIT == 0 {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Ok(flags & tf::UNIT != 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeOfSymbolWithDeferredType
    pub(crate) fn get_type_of_symbol_with_deferred_type(
        &mut self,
        symbol: SymbolId,
    ) -> Result<TypeId, Error> {
        if let Some(ty) = self
            .value_symbol_links
            .try_get(symbol)
            .and_then(|l| l.resolved_type)
        {
            return Ok(ty);
        }
        let types = self
            .query
            .deferred_property_types
            .try_get(symbol)
            .and_then(Option::as_ref)
            .ok_or(Error::Unsupported(
                "deferred property: non-union constituents",
            ))?
            .clone();
        let containing = self
            .value_symbol_links
            .try_get(symbol)
            .and_then(|links| links.containing_type)
            .ok_or(Error::MissingLink("deferred property containing type"))?;
        let ty = if self.types.flags(containing)? & tf::UNION != 0 {
            self.get_union_type(&types)?
        } else {
            self.get_intersection_type(&types)?
        };
        self.value_symbol_links.get_or_default(symbol).resolved_type = Some(ty);
        Ok(ty)
    }
}
