//! Mapped type declarations and instantiations. Each instantiated iteration
//! parameter has its own identity; syntax, aliases and member links stay owned
//! by the checker and are resolved in the pinned checker's order.

use crate::{object_flags as of, type_flags as tf, AliasId, CheckerState, Error, MapperId, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{check_flags as cf, symbol_flags as sf, JsString, SymbolTable, SyntaxKind as K};

pub(crate) const INCLUDE_READONLY: u32 = 1;
pub(crate) const EXCLUDE_READONLY: u32 = 2;
pub(crate) const INCLUDE_OPTIONAL: u32 = 4;
pub(crate) const EXCLUDE_OPTIONAL: u32 = 8;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MappedSymbolLinks {
    pub key_type: Option<TypeId>,
    pub synthetic_origin: Option<SymbolId>,
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getMappedTypeNameTypeKind
    pub(crate) fn mapped_remaps_keys(&mut self, ty: TypeId) -> Result<bool, Error> {
        let Some(name) = self.mapped_name(ty)? else {
            return Ok(false);
        };
        let parameter = self.mapped_parameter(ty)?;
        Ok(!self.is_type_related_to(name, parameter, crate::RelationKind::Assignable)?)
    }

    // port: tsc/internal/checker/checker.go:Checker.isMappedTypeGenericIndexedAccess
    pub(crate) fn mapped_generic_indexed_access(&mut self, ty: TypeId) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::INDEXED_ACCESS == 0 {
            return Ok(false);
        }
        let data = *self.types.indexed_access(ty)?;
        Ok(
            self.types.get(data.object_type)?.object_flags & of::MAPPED != 0
                && !self.is_generic_mapped_type(data.object_type)?
                && self.is_generic_index_type(data.index_type)?
                && self.mapped_modifiers(data.object_type)? & EXCLUDE_OPTIONAL == 0
                && self
                    .ast(self.mapped_declaration(data.object_type)?)?
                    .node(self.mapped_declaration(data.object_type)?)?
                    .data_source()
                    .as_mapped_type_node()
                    .ok_or(Error::MissingLink("mapped syntax"))?
                    .name_type()
                    .is_none(),
        )
    }

    // port: tsc/internal/checker/checker.go:getMappedTypeOptionality
    pub(crate) fn mapped_optionality(&self, ty: TypeId) -> Result<i32, Error> {
        let modifiers = self.mapped_modifiers(ty)?;
        Ok(if modifiers & EXCLUDE_OPTIONAL != 0 {
            -1
        } else {
            i32::from(modifiers & INCLUDE_OPTIONAL != 0)
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getCombinedMappedTypeOptionality
    pub(crate) fn combined_mapped_optionality(&mut self, ty: TypeId) -> Result<i32, Error> {
        if self.types.get(ty)?.object_flags & of::MAPPED != 0 {
            let optionality = self.mapped_optionality(ty)?;
            if optionality != 0 {
                return Ok(optionality);
            }
            let modifiers = self.mapped_modifiers_type(ty)?;
            return self.combined_mapped_optionality(modifiers);
        }
        if self.types.flags(ty)? & tf::INTERSECTION != 0 {
            let parts = self.types.compound_types(ty)?.clone();
            let optionality = self.combined_mapped_optionality(parts[0])?;
            for &part in &parts[1..] {
                if self.combined_mapped_optionality(part)? != optionality {
                    return Ok(0);
                }
            }
            return Ok(optionality);
        }
        Ok(0)
    }

    // port: tsc/internal/checker/checker.go:Checker.substituteIndexedMappedType
    pub(crate) fn substitute_indexed_mapped(
        &mut self,
        object: TypeId,
        index: TypeId,
    ) -> Result<TypeId, Error> {
        let parameter = self.mapped_parameter(object)?;
        let mapper = self.new_type_mapper(&[parameter], &[index])?;
        let mapper = self.combine_type_mappers(self.types.object(object)?.mapper, mapper)?;
        let template = self.mapped_template(self.types.object(object)?.target.unwrap_or(object))?;
        let template = self.instantiate_type(template, Some(mapper))?;
        let mut optional = self.mapped_optionality(object)? > 0;
        if !optional {
            optional = if self.get_generic_object_flags(object)? & of::IS_GENERIC_TYPE != 0 {
                let modifiers = self.mapped_modifiers_type(object)?;
                self.combined_mapped_optionality(modifiers)? > 0
            } else {
                self.could_access_optional_property(object, index)?
            };
        }
        self.add_type_optionality(template, true, optional)
    }

    // port: tsc/internal/checker/checker.go:Checker.couldAccessOptionalProperty
    fn could_access_optional_property(
        &mut self,
        object: TypeId,
        index: TypeId,
    ) -> Result<bool, Error> {
        let Some(constraint) = self.base_constraint_of_type(index)? else {
            return Ok(false);
        };
        for property in self.get_properties_of_type(object)? {
            if self.symbol(property)?.flags() & sf::OPTIONAL != 0 {
                let key = self
                    .literal_type_from_property(property, tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE)?;
                if self.is_type_related_to(key, constraint, crate::RelationKind::Assignable)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromMappedTypeNode
    pub(crate) fn source_mapped_type(&mut self, node: NodeId) -> Result<TypeId, Error> {
        if let Some(Some(ty)) = self.query.type_nodes.try_get(node) {
            return Ok(*ty);
        }
        let symbol = self.get_symbol_of_declaration(node)?;
        let ty = self.new_object_type(of::MAPPED, symbol)?;
        self.types.mapped_mut(ty)?.declaration = Some(node);
        let alias = self
            .alias_for_type_node(node)?
            .map(|alias| self.types.push_alias(alias))
            .transpose()?;
        self.types.get_mut(ty)?.alias = alias;
        *self.query.type_nodes.get_or_default(node) = Some(ty);
        self.mapped_constraint(ty)?;
        Ok(ty)
    }

    pub(crate) fn mapped_declaration(&self, ty: TypeId) -> Result<NodeId, Error> {
        self.types
            .mapped(ty)?
            .declaration
            .ok_or(Error::MissingLink("mapped declaration"))
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeParameterFromMappedType
    pub(crate) fn mapped_parameter(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if let Some(parameter) = self.types.mapped(ty)?.type_parameter {
            return Ok(parameter);
        }
        let node = self.mapped_declaration(ty)?;
        let read = self.ast(node)?.node(node)?;
        let parameter = read
            .data_source()
            .as_mapped_type_node()
            .and_then(|data| data.type_parameter())
            .ok_or(Error::MissingLink("mapped type parameter"))?;
        let symbol = self
            .get_symbol_of_declaration(parameter)?
            .ok_or(Error::MissingLink("mapped parameter symbol"))?;
        let parameter = self.get_declared_type_of_type_parameter(symbol)?;
        self.types.mapped_mut(ty)?.type_parameter = Some(parameter);
        Ok(parameter)
    }

    // port: tsc/internal/checker/checker.go:Checker.getConstraintTypeFromMappedType
    pub(crate) fn mapped_constraint(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if let Some(constraint) = self.types.mapped(ty)?.constraint_type {
            return Ok(constraint);
        }
        let parameter = self.mapped_parameter(ty)?;
        let constraint = self
            .constraint_of_type_parameter(parameter)?
            .unwrap_or(self.builtins.error_type);
        self.types.mapped_mut(ty)?.constraint_type = Some(constraint);
        Ok(constraint)
    }

    // port: tsc/internal/checker/checker.go:Checker.getNameTypeFromMappedType
    pub(crate) fn mapped_name(&mut self, ty: TypeId) -> Result<Option<TypeId>, Error> {
        if let Some(name) = self.types.mapped(ty)?.name_type {
            return Ok(Some(name));
        }
        let node = self.mapped_declaration(ty)?;
        let read = self.ast(node)?.node(node)?;
        let Some(name) = read
            .data_source()
            .as_mapped_type_node()
            .and_then(|data| data.name_type())
        else {
            return Ok(None);
        };
        let name = self.get_type_from_type_node(name)?;
        let name = self.instantiate_type(name, self.types.object(ty)?.mapper)?;
        self.types.mapped_mut(ty)?.name_type = Some(name);
        Ok(Some(name))
    }

    // port: tsc/internal/checker/checker.go:Checker.getTemplateTypeFromMappedType
    pub(crate) fn mapped_template(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if let Some(template) = self.types.mapped(ty)?.template_type {
            return Ok(template);
        }
        let node = self.mapped_declaration(ty)?;
        let template = if let Some(annotation) = self.ast(node)?.node(node)?.type_node() {
            let annotation = self.get_type_from_type_node(annotation)?;
            let optional = self.mapped_modifiers(ty)? & INCLUDE_OPTIONAL != 0;
            let annotation = self.add_type_optionality(annotation, true, optional)?;
            self.instantiate_type(annotation, self.types.object(ty)?.mapper)?
        } else {
            self.builtins.error_type
        };
        self.types.mapped_mut(ty)?.template_type = Some(template);
        Ok(template)
    }

    // port: tsc/internal/checker/checker.go:getMappedTypeModifiers
    pub(crate) fn mapped_modifiers(&self, ty: TypeId) -> Result<u32, Error> {
        let node = self.mapped_declaration(ty)?;
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_mapped_type_node()
            .ok_or(Error::MissingLink("mapped syntax"))?;
        let mut flags = 0;
        if let Some(token) = data.readonly_token() {
            flags |= if self.ast(token)?.node(token)?.kind() == K::MinusToken {
                EXCLUDE_READONLY
            } else {
                INCLUDE_READONLY
            };
        }
        if let Some(token) = data.question_token() {
            flags |= if self.ast(token)?.node(token)?.kind() == K::MinusToken {
                EXCLUDE_OPTIONAL
            } else {
                INCLUDE_OPTIONAL
            };
        }
        Ok(flags)
    }

    // port: tsc/internal/checker/checker.go:Checker.getConstraintDeclarationForMappedType
    pub(crate) fn mapped_constraint_node(&self, ty: TypeId) -> Result<NodeId, Error> {
        let node = self.mapped_declaration(ty)?;
        let read = self.ast(node)?.node(node)?;
        let parameter = read
            .data_source()
            .as_mapped_type_node()
            .and_then(|data| data.type_parameter())
            .ok_or(Error::MissingLink("mapped parameter"))?;
        self.ast(parameter)?
            .node(parameter)?
            .data_source()
            .as_type_parameter_declaration()
            .and_then(|data| data.constraint())
            .ok_or(Error::MissingLink("mapped constraint node"))
    }

    pub(crate) fn mapped_keyof_constraint(&self, ty: TypeId) -> Result<bool, Error> {
        let node = self.mapped_constraint_node(ty)?;
        Ok(self
            .ast(node)?
            .node(node)?
            .data_source()
            .as_type_operator_node()
            .is_some_and(|data| data.operator() == K::KeyOfKeyword))
    }

    // port: tsc/internal/checker/checker.go:Checker.getModifiersTypeFromMappedType
    pub(crate) fn mapped_modifiers_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if let Some(modifiers) = self.types.mapped(ty)?.modifiers_type {
            return Ok(modifiers);
        }
        let mapper = self.types.object(ty)?.mapper;
        let modifiers = if self.mapped_keyof_constraint(ty)? {
            let node = self.mapped_constraint_node(ty)?;
            let operand = self
                .ast(node)?
                .node(node)?
                .type_node()
                .ok_or(Error::MissingLink("mapped keyof operand"))?;
            let source = self.get_type_from_type_node(operand)?;
            self.instantiate_type(source, mapper)?
        } else {
            let declared = self.source_mapped_type(self.mapped_declaration(ty)?)?;
            let constraint = self.mapped_constraint(declared)?;
            let extended = if self.types.flags(constraint)? & tf::TYPE_PARAMETER != 0 {
                self.constraint_of_type_parameter(constraint)?
            } else {
                Some(constraint)
            };
            match extended {
                Some(constraint) if self.types.flags(constraint)? & tf::INDEX != 0 => {
                    self.instantiate_type(self.types.target(constraint)?, mapper)?
                }
                _ => self.builtins.unknown_type,
            }
        };
        self.types.mapped_mut(ty)?.modifiers_type = Some(modifiers);
        Ok(modifiers)
    }

    // port: tsc/internal/checker/checker.go:Checker.isGenericMappedType
    pub(crate) fn is_generic_mapped_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        if self.types.get(ty)?.object_flags & of::MAPPED == 0 {
            return Ok(false);
        }
        let constraint = self.mapped_constraint(ty)?;
        if self.is_generic_index_type(constraint)? {
            return Ok(true);
        }
        if let Some(name) = self.mapped_name(ty)? {
            let parameter = self.mapped_parameter(ty)?;
            let mapper = self.new_type_mapper(&[parameter], &[constraint])?;
            let name = self.instantiate_type(name, Some(mapper))?;
            return self.is_generic_index_type(name);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getHomomorphicTypeVariable
    pub(crate) fn homomorphic_type_variable(
        &mut self,
        ty: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        let constraint = self.mapped_constraint(ty)?;
        if self.types.flags(constraint)? & tf::INDEX != 0 {
            let target = self.types.target(constraint)?;
            let target = self.actual_type_variable(target)?;
            if self.types.flags(target)? & tf::TYPE_PARAMETER != 0 {
                return Ok(Some(target));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateAnonymousType
    pub(crate) fn instantiate_mapped_object(
        &mut self,
        ty: TypeId,
        mut mapper: MapperId,
        alias: Option<AliasId>,
    ) -> Result<TypeId, Error> {
        let record = *self.types.get(ty)?;
        let result = self.new_object_type(
            record.object_flags
                & !(of::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED | of::COULD_CONTAIN_TYPE_VARIABLES)
                | of::INSTANTIATED,
            record.symbol,
        )?;
        self.types.mapped_mut(result)?.declaration = Some(self.mapped_declaration(ty)?);
        let parameter = self.mapped_parameter(ty)?;
        let fresh = self.new_type_parameter(self.types.get(parameter)?.symbol)?;
        self.types.type_parameter_mut(fresh)?.target = Some(parameter);
        self.types.mapped_mut(result)?.type_parameter = Some(fresh);
        let fresh_mapper = self.new_type_mapper(&[parameter], &[fresh])?;
        mapper = self.combine_type_mappers(Some(fresh_mapper), mapper)?;
        self.types.type_parameter_mut(fresh)?.mapper = Some(mapper);
        let alias = match alias {
            Some(alias) => Some(alias),
            None => self.instantiate_type_alias(ty, mapper)?,
        };
        self.types.get_mut(result)?.alias = alias;
        if let Some(alias) = self.types.alias_of(result)?.cloned() {
            let flags = self.get_propagating_flags_of_types(&alias.type_arguments, 0)?;
            self.types.get_mut(result)?.object_flags |= flags;
        }
        let data = self.types.object_mut(result)?;
        data.target = Some(ty);
        data.mapper = Some(mapper);
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateMappedType
    pub(crate) fn instantiate_mapped_type(
        &mut self,
        ty: TypeId,
        mapper: MapperId,
        alias: Option<AliasId>,
    ) -> Result<TypeId, Error> {
        if let Some(parameter) = self.homomorphic_type_variable(ty)? {
            let argument = self.instantiate_type(parameter, Some(mapper))?;
            if argument != parameter {
                let argument = self.get_reduced_type(argument)?;
                if self.types.flags(argument)? & tf::UNION != 0 && alias.is_some() {
                    let parts = self.types.compound_types(argument)?.clone();
                    let mut result = Vec::new();
                    for &part in parts.iter() {
                        result.push(
                            self.instantiate_mapped_constituent(part, ty, parameter, mapper)?,
                        );
                    }
                    return self.get_union_type_ex(
                        &result,
                        crate::UnionReduction::Literal,
                        alias,
                        None,
                    );
                }
                return self
                    .map_type(argument, &mut |checker, part| {
                        checker
                            .instantiate_mapped_constituent(part, ty, parameter, mapper)
                            .map(Some)
                    })?
                    .ok_or(Error::MissingLink("mapped union result"));
            }
        }
        let constraint = self.mapped_constraint(ty)?;
        if self.instantiate_type(constraint, Some(mapper))? == self.builtins.wildcard_type {
            return Ok(self.builtins.wildcard_type);
        }
        self.instantiate_mapped_object(ty, mapper, alias)
    }

    fn instantiate_mapped_constituent(
        &mut self,
        argument: TypeId,
        ty: TypeId,
        parameter: TypeId,
        mapper: MapperId,
    ) -> Result<TypeId, Error> {
        let flags = self.types.flags(argument)?;
        if flags
            & (tf::ANY_OR_UNKNOWN | tf::INSTANTIABLE_NON_PRIMITIVE | tf::OBJECT | tf::INTERSECTION)
            == 0
            || argument == self.builtins.wildcard_type
            || self.is_error_type(argument)?
        {
            return Ok(argument);
        }
        if self.mapped_name(ty)?.is_none() {
            let any_array = if flags & tf::ANY != 0 {
                let types = &self.types;
                let cycle = self
                    .resolution
                    .find_resolution_cycle_start_index(
                        crate::TypeSystemEntity::Type(parameter),
                        crate::TypeSystemPropertyName::ResolvedBaseConstraint,
                        |entry| {
                            let crate::TypeSystemEntity::Type(ty) = entry.target else {
                                return false;
                            };
                            entry.property_name
                                == crate::TypeSystemPropertyName::ResolvedBaseConstraint
                                && types.base_constraint_slot(ty).is_ok_and(|slot| {
                                    slot.is_some_and(std::option::Option::is_some)
                                })
                        },
                    )
                    .is_some();
                if cycle {
                    false
                } else if let Some(constraint) = self.constraint_of_type_parameter(parameter)? {
                    let parts = self.distributed_types(constraint)?;
                    let mut all_arrays = true;
                    for part in parts {
                        if !self.is_array_type(part)? && !self.is_tuple_type(part)? {
                            all_arrays = false;
                            break;
                        }
                    }
                    all_arrays
                } else {
                    false
                }
            } else {
                false
            };
            if self.is_array_type(argument)? || any_array {
                let mapper = self.prepend_type_mapping(parameter, argument, Some(mapper))?;
                let element =
                    self.instantiate_mapped_template(ty, self.builtins.number_type, true, mapper)?;
                if self.is_error_type(element)? {
                    return Ok(self.builtins.error_type);
                }
                let readonly = self.readonly_array_or_tuple(argument)?;
                return self.create_array_type(
                    element,
                    modified_readonly(readonly, self.mapped_modifiers(ty)?),
                );
            }
            if self.is_tuple_type(argument)? {
                return self.instantiate_mapped_tuple(argument, ty, parameter, mapper);
            }
            if flags & tf::INTERSECTION != 0 && self.array_tuple_intersection(argument)? {
                let parts = self.types.compound_types(argument)?.clone();
                let mut result = Vec::new();
                for &part in parts.iter() {
                    result.push(self.instantiate_mapped_constituent(part, ty, parameter, mapper)?);
                }
                return self.get_intersection_type(&result);
            }
        }
        let mapper = self.prepend_type_mapping(parameter, argument, Some(mapper))?;
        self.instantiate_mapped_object(ty, mapper, None)
    }

    pub(crate) fn array_tuple_intersection(&self, ty: TypeId) -> Result<bool, Error> {
        if self.is_array_type(ty)? || self.is_tuple_type(ty)? {
            return Ok(true);
        }
        if self.types.flags(ty)? & tf::INTERSECTION != 0 {
            for &part in self.types.types_of(ty)? {
                if !self.is_array_type(part)? && !self.is_tuple_type(part)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateMappedTupleType
    fn instantiate_mapped_tuple(
        &mut self,
        tuple: TypeId,
        mapped: TypeId,
        parameter: TypeId,
        mapper: MapperId,
    ) -> Result<TypeId, Error> {
        use crate::element_flags as ef;
        let target = self.types.tuple(self.types.target(tuple)?)?;
        let mut infos = target.element_infos.to_vec();
        let fixed = target.fixed_length as usize;
        let readonly = target.readonly;
        let fixed_mapper = if fixed != 0 {
            self.prepend_type_mapping(parameter, tuple, Some(mapper))?
        } else {
            mapper
        };
        let modifiers = self.mapped_modifiers(mapped)?;
        let elements = self.element_types(tuple)?;
        let mut result = Vec::new();
        for (index, &element) in elements.iter().enumerate() {
            let flags = infos[index].flags;
            let value = if index < fixed {
                let key = self.get_string_literal_type(JsString::from_bytes(
                    index.to_string().into_bytes(),
                ))?;
                self.instantiate_mapped_template(
                    mapped,
                    key,
                    flags & ef::OPTIONAL != 0,
                    fixed_mapper,
                )?
            } else if flags & ef::VARIADIC != 0 {
                let mapper = self.prepend_type_mapping(parameter, element, Some(mapper))?;
                self.instantiate_type(mapped, Some(mapper))?
            } else {
                let array = self.create_array_type(element, false)?;
                let mapper = self.prepend_type_mapping(parameter, array, Some(mapper))?;
                let mapped = self.instantiate_type(mapped, Some(mapper))?;
                if self.is_array_type(mapped)? {
                    self.get_type_arguments(mapped)?[0]
                } else {
                    self.builtins.unknown_type
                }
            };
            if modifiers & INCLUDE_OPTIONAL != 0 && flags & ef::REQUIRED != 0 {
                infos[index].flags = ef::OPTIONAL;
            } else if modifiers & EXCLUDE_OPTIONAL != 0 && flags & ef::OPTIONAL != 0 {
                infos[index].flags = ef::REQUIRED;
            }
            result.push(value);
        }
        if result.contains(&self.builtins.error_type) {
            return Ok(self.builtins.error_type);
        }
        self.create_tuple_type_ex(&result, &infos, modified_readonly(readonly, modifiers))
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateMappedTypeTemplate
    fn instantiate_mapped_template(
        &mut self,
        ty: TypeId,
        key: TypeId,
        optional: bool,
        mapper: MapperId,
    ) -> Result<TypeId, Error> {
        let parameter = self.mapped_parameter(ty)?;
        let mapper = self.append_type_mapping(Some(mapper), parameter, key)?;
        let template = self.mapped_template(self.types.object(ty)?.target.unwrap_or(ty))?;
        let value = self.instantiate_type(template, Some(mapper))?;
        let modifiers = self.mapped_modifiers(ty)?;
        if self.options.strict_null_checks
            && modifiers & INCLUDE_OPTIONAL != 0
            && !self.maybe_type_of_kind(value, tf::UNDEFINED | tf::VOID)?
        {
            return self.add_type_optionality(value, true, true);
        }
        if self.options.strict_null_checks && modifiers & EXCLUDE_OPTIONAL != 0 && optional {
            return self.remove_missing_or_undefined(value);
        }
        Ok(value)
    }

    // port: tsc/internal/checker/checker.go:Checker.removeMissingOrUndefinedType
    pub(crate) fn remove_missing_or_undefined(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let exact = self.options.exact_optional_property_types;
        let missing = self.builtins.missing_type;
        self.filter_type(ty, &mut |checker, part| {
            Ok(if exact {
                part != missing
            } else {
                checker.types.flags(part)? & tf::UNDEFINED == 0
            })
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.forEachMappedTypePropertyKeyTypeAndIndexSignatureKeyType
    pub(crate) fn mapped_property_keys(
        &mut self,
        ty: TypeId,
        strings_only: bool,
    ) -> Result<Vec<TypeId>, Error> {
        let mut keys = Vec::new();
        for property in self.get_properties_of_type(ty)? {
            keys.push(
                self.literal_type_from_property(property, tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE)?,
            );
        }
        if self.types.flags(ty)? & tf::ANY != 0 {
            keys.push(self.builtins.string_type);
        } else {
            for index in self.index_infos_of_type(ty)? {
                let key = self.signatures.index_info(index)?.key_type;
                if !strings_only
                    || self.types.flags(key)? & (tf::STRING | tf::TEMPLATE_LITERAL) != 0
                {
                    keys.push(key);
                }
            }
        }
        Ok(keys)
    }

    // port: tsc/internal/checker/checker.go:Checker.getLowerBoundOfKeyType
    pub(crate) fn lower_bound_of_key_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::INDEX != 0 {
            let target = self.apparent_type(self.types.target(ty)?)?;
            if self.is_generic_tuple_type(target)? {
                return self.known_tuple_keys(target);
            }
            return self.get_index_type(target, 0);
        }
        if flags & tf::CONDITIONAL != 0 {
            let data = *self.types.conditional(ty)?;
            let root = self.conditional_root(data.root)?;
            if root.distributive {
                let root_check = root.check_type;
                let constraint = self.lower_bound_of_key_type(data.check_type)?;
                if constraint != data.check_type {
                    let mapper = self.prepend_type_mapping(root_check, constraint, data.mapper)?;
                    return self.conditional_instantiation(ty, mapper, false, None);
                }
            }
            return Ok(ty);
        }
        if flags & tf::UNION != 0 {
            return self
                .map_type_ex(
                    ty,
                    &mut |checker, part| checker.lower_bound_of_key_type(part).map(Some),
                    true,
                )?
                .ok_or(Error::MissingLink("lower key union"));
        }
        if flags & tf::INTERSECTION != 0 {
            let parts = self.types.compound_types(ty)?.clone();
            if parts.len() == 2
                && self.types.flags(parts[0])? & (tf::STRING | tf::NUMBER | tf::BIG_INT) != 0
                && parts[1] == self.builtins.empty_type_literal_type
            {
                return Ok(ty);
            }
            let mut types = Vec::new();
            for &part in parts.iter() {
                types.push(self.lower_bound_of_key_type(part)?);
            }
            return self.get_intersection_type(&types);
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveMappedTypeMembers
    pub(crate) fn resolve_mapped_members(&mut self, ty: TypeId) -> Result<(), Error> {
        self.set_structured_type_members(ty, None, &[], &[], &[])?;
        let parameter = self.mapped_parameter(ty)?;
        let constraint = self.mapped_constraint(ty)?;
        let target = self.types.object(ty)?.target.unwrap_or(ty);
        let name_type = self.mapped_name(target)?;
        let link_declarations = match name_type {
            Some(name) => {
                let parameter = self.mapped_parameter(target)?;
                self.source_type_assignable(name, parameter, &mut Vec::new())?
            }
            None => true,
        };
        let template = self.mapped_template(target)?;
        let modifiers = self.mapped_modifiers_type(ty)?;
        let modifiers = self.apparent_type(modifiers)?;
        let template_modifiers = self.mapped_modifiers(ty)?;
        let keys = if self.mapped_keyof_constraint(ty)? {
            self.mapped_property_keys(modifiers, false)?
        } else {
            let keys = self.lower_bound_of_key_type(constraint)?;
            self.distributed_types(keys)?
        };
        let mapper = self.types.object(ty)?.mapper;
        let mut members = SymbolTable::new();
        let mut indexes = Vec::new();
        for key in keys {
            let name = match name_type {
                Some(name) => {
                    let mapper = self.append_type_mapping(mapper, parameter, key)?;
                    self.instantiate_type(name, Some(mapper))?
                }
                None => key,
            };
            for name_type in self.distributed_types(name)? {
                if let Some(name) = self.index_property_name(name_type)? {
                    if let Some(existing) = members.get(name.as_bytes()).copied().flatten() {
                        let previous_name = self
                            .value_symbol_links
                            .get_or_default(existing)
                            .name_type
                            .ok_or(Error::MissingLink("mapped name link"))?;
                        let joined_name = self.get_union_type(&[previous_name, name_type])?;
                        self.value_symbol_links.get_or_default(existing).name_type =
                            Some(joined_name);
                        let previous_key = self
                            .mapped_symbol_links
                            .get_or_default(existing)
                            .key_type
                            .ok_or(Error::MissingLink("mapped key link"))?;
                        let joined_key = self.get_union_type(&[previous_key, key])?;
                        self.mapped_symbol_links.get_or_default(existing).key_type =
                            Some(joined_key);
                    } else {
                        let modifier_property = match self.index_property_name(key)? {
                            Some(name) => {
                                self.constituent_property(modifiers, name.as_bytes(), false)?
                            }
                            None => None,
                        };
                        let source_optional = match modifier_property {
                            Some(property) => self.symbol(property)?.flags() & sf::OPTIONAL != 0,
                            None => false,
                        };
                        let optional = template_modifiers & INCLUDE_OPTIONAL != 0
                            || template_modifiers & EXCLUDE_OPTIONAL == 0 && source_optional;
                        let source_readonly = match modifier_property {
                            Some(property) => self.is_readonly_symbol(property)?,
                            None => false,
                        };
                        let readonly = template_modifiers & INCLUDE_READONLY != 0
                            || template_modifiers & EXCLUDE_READONLY == 0 && source_readonly;
                        let strip_optional =
                            self.options.strict_null_checks && !optional && source_optional;
                        let late = match modifier_property {
                            Some(property) => self.symbol(property)?.check_flags() & cf::LATE,
                            None => 0,
                        };
                        let property = self.new_symbol(
                            sf::PROPERTY | if optional { sf::OPTIONAL } else { 0 },
                            name.clone(),
                        )?;
                        self.symbol_mut(property)?.check_flags = late
                            | cf::MAPPED
                            | if readonly { cf::READONLY } else { 0 }
                            | if strip_optional {
                                cf::STRIP_OPTIONAL
                            } else {
                                0
                            };
                        let links = self.value_symbol_links.get_or_default(property);
                        links.containing_type = Some(ty);
                        links.name_type = Some(name_type);
                        let mapped_links = self.mapped_symbol_links.get_or_default(property);
                        mapped_links.key_type = Some(key);
                        mapped_links.synthetic_origin = modifier_property;
                        if link_declarations {
                            if let Some(source) = modifier_property {
                                self.symbol_mut(property)?.declarations =
                                    self.symbol(source)?.declarations();
                            }
                        }
                        members.insert(name, Some(property));
                    }
                } else if self.valid_index_key_type(name_type)?
                    || self.types.flags(name_type)? & (tf::ANY | tf::ENUM) != 0
                {
                    let flags = self.types.flags(name_type)?;
                    let index_key = if flags & (tf::ANY | tf::STRING) != 0 {
                        self.builtins.string_type
                    } else if flags & (tf::NUMBER | tf::ENUM) != 0 {
                        self.builtins.number_type
                    } else {
                        name_type
                    };
                    let mapper = self.append_type_mapping(mapper, parameter, key)?;
                    let value = self.instantiate_type(template, Some(mapper))?;
                    let source_index = self.applicable_index_info(modifiers, name_type)?;
                    let source_readonly = match source_index {
                        Some(index) => self.signatures.index_info(index)?.is_readonly,
                        None => false,
                    };
                    let readonly = template_modifiers & INCLUDE_READONLY != 0
                        || template_modifiers & EXCLUDE_READONLY == 0 && source_readonly;
                    let index = self
                        .signatures
                        .new_index_info(index_key, value, readonly, None, None)?;
                    self.append_index_info(&mut indexes, index, true)?;
                }
            }
        }
        let members = self.alloc_symbol_table(members);
        self.set_structured_type_members(ty, Some(members), &[], &[], &indexes)
    }

    pub(crate) fn distributed_types(&self, ty: TypeId) -> Result<Vec<TypeId>, Error> {
        if self.types.flags(ty)? & tf::UNION != 0 {
            Ok(self.types.types_of(ty)?.to_vec())
        } else {
            Ok(vec![ty])
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.isReadonlySymbol
    pub(crate) fn is_readonly_symbol(&mut self, symbol: SymbolId) -> Result<bool, Error> {
        let read = self.symbol(symbol)?;
        if read.check_flags() & cf::READONLY != 0 || read.flags() & sf::ENUM_MEMBER != 0 {
            return Ok(true);
        }
        if read.flags() & sf::ACCESSOR == sf::GET_ACCESSOR {
            return Ok(true);
        }
        if let Some(node) = read.value_declaration() {
            let view = self.ast(node)?;
            let node_id = node;
            let node = view.node(node)?;
            if read.flags() & sf::PROPERTY != 0
                && node.modifier_flags(view)? & ts_ast::modifier_flags::READONLY != 0
            {
                return Ok(true);
            }
            if read.flags() & sf::VARIABLE != 0
                && ts_ast::utilities::get_combined_node_flags(view, node_id)?
                    & ts_ast::node_flags::CONSTANT
                    != 0
            {
                return Ok(true);
            }
        }
        for index in 0..self.symbol_declarations(symbol)?.len() {
            if let Some(declaration) = self.symbol_declarations(symbol)?.at(index) {
                if self.is_readonly_assignment_declaration(declaration)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeOfMappedSymbol
    pub(crate) fn type_of_mapped_symbol(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        let links = self
            .value_symbol_links
            .try_get(symbol)
            .copied()
            .ok_or(Error::MissingLink("mapped value links"))?;
        if let Some(ty) = links.resolved_type {
            return Ok(ty);
        }
        let mapped = links
            .containing_type
            .ok_or(Error::MissingLink("mapped containing type"))?;
        let link_store = &self.value_symbol_links;
        if !self.resolution.push(
            crate::TypeSystemEntity::Symbol(symbol),
            crate::TypeSystemPropertyName::Type,
            |entry| {
                let crate::TypeSystemEntity::Symbol(symbol) = entry.target else {
                    return false;
                };
                entry.property_name == crate::TypeSystemPropertyName::Type
                    && link_store
                        .try_get(symbol)
                        .is_some_and(|links| links.resolved_type.is_some())
            },
        ) {
            self.types.mapped_mut(mapped)?.contains_error = true;
            return Ok(self.builtins.error_type);
        }
        let result = (|| {
            let target = self.types.object(mapped)?.target.unwrap_or(mapped);
            let template = self.mapped_template(target)?;
            let parameter = self.mapped_parameter(mapped)?;
            let key = self
                .mapped_symbol_links
                .try_get(symbol)
                .and_then(|links| links.key_type)
                .ok_or(Error::MissingLink("mapped symbol key"))?;
            let mapper =
                self.append_type_mapping(self.types.object(mapped)?.mapper, parameter, key)?;
            let value = self.instantiate_type(template, Some(mapper))?;
            if self.options.strict_null_checks
                && self.symbol(symbol)?.flags() & sf::OPTIONAL != 0
                && !self.maybe_type_of_kind(value, tf::UNDEFINED | tf::VOID)?
            {
                return self.add_type_optionality(value, true, true);
            }
            if self.symbol(symbol)?.check_flags() & cf::STRIP_OPTIONAL != 0 {
                return self.remove_missing_or_undefined(value);
            }
            Ok(value)
        })();
        let complete = self.resolution.pop();
        let value = result?;
        if !complete {
            self.value_symbol_links
                .get_or_default(symbol)
                .resolved_type
                .get_or_insert(self.builtins.error_type);
            let name = self.symbol_to_string(symbol)?;
            let mapped = self.type_to_string(mapped, crate::type_display::DEFAULT_FLAGS)?;
            self.error_at(
                self.current_node,
                ts_diagnostics::Type_of_property_0_circularly_references_itself_in_mapped_type_1,
                vec![name, mapped],
            )?;
        }
        Ok(*self
            .value_symbol_links
            .get_or_default(symbol)
            .resolved_type
            .get_or_insert(value))
    }

    // port: tsc/internal/checker/checker.go:Checker.getApparentMappedTypeKeys
    pub(crate) fn apparent_mapped_keys(
        &mut self,
        name: TypeId,
        ty: TypeId,
    ) -> Result<TypeId, Error> {
        let modifiers = self.mapped_modifiers_type(ty)?;
        let modifiers = self.apparent_type(modifiers)?;
        let mut keys = Vec::new();
        for key in self.mapped_property_keys(modifiers, false)? {
            let parameter = self.mapped_parameter(ty)?;
            let mapper = self.append_type_mapping(self.types.object(ty)?.mapper, parameter, key)?;
            keys.push(self.instantiate_type(name, Some(mapper))?);
        }
        self.get_union_type(&keys)
    }

    // port: tsc/internal/checker/checker.go:Checker.getIndexTypeForMappedType
    pub(crate) fn index_type_for_mapped(
        &mut self,
        ty: TypeId,
        flags: u32,
    ) -> Result<TypeId, Error> {
        let parameter = self.mapped_parameter(ty)?;
        let constraint = self.mapped_constraint(ty)?;
        let name = self.mapped_name(self.types.object(ty)?.target.unwrap_or(ty))?;
        if name.is_none() && flags & crate::indexes::NO_INDEX_SIGNATURES == 0 {
            return Ok(constraint);
        }
        let keys = if self.is_generic_index_type(constraint)? {
            if self.mapped_keyof_constraint(ty)? {
                return self.generic_index_type(ty, flags);
            }
            self.distributed_types(constraint)?
        } else if self.mapped_keyof_constraint(ty)? {
            let modifiers = self.mapped_modifiers_type(ty)?;
            let modifiers = self.apparent_type(modifiers)?;
            self.mapped_property_keys(modifiers, flags & crate::indexes::STRINGS_ONLY != 0)?
        } else {
            let lower = self.lower_bound_of_key_type(constraint)?;
            self.distributed_types(lower)?
        };
        let mut types = Vec::new();
        for key in keys {
            let ty = match name {
                Some(name) => {
                    let mapper =
                        self.append_type_mapping(self.types.object(ty)?.mapper, parameter, key)?;
                    self.instantiate_type(name, Some(mapper))?
                }
                None => key,
            };
            types.push(if ty == self.builtins.string_type {
                self.builtins.string_or_number_type
            } else {
                ty
            });
        }
        let mut result = self.get_union_type(&types)?;
        if flags & crate::indexes::NO_INDEX_SIGNATURES != 0 {
            result = self.filter_type(result, &mut |checker, ty| {
                Ok(checker.types.flags(ty)? & (tf::ANY | tf::STRING) == 0)
            })?;
        }
        if self.types.flags(result)? & tf::UNION != 0
            && self.types.flags(constraint)? & tf::UNION != 0
            && self.types.types_of(result)? == self.types.types_of(constraint)?
        {
            return Ok(constraint);
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkMappedType
    pub(crate) fn check_mapped_type(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_mapped_type_node()
            .ok_or(Error::MissingLink("mapped syntax"))?;
        let parameter = data.type_parameter();
        let name = data.name_type();
        let annotation = read.type_node();
        if let Some(&member) = self.source_list(node, data.members())?.first() {
            self.error_at(
                Some(member),
                ts_diagnostics::A_mapped_type_may_not_declare_properties_or_methods,
                vec![],
            )?;
        }
        for child in [parameter, name, annotation].into_iter().flatten() {
            self.check_source_element(child)?;
        }
        if annotation.is_none() {
            let options = self.program()?.host.options();
            if options.strict_option_value(options.no_implicit_any) {
                self.error_at(
                    Some(node),
                    ts_diagnostics::Mapped_object_type_implicitly_has_an_any_template_type,
                    vec![],
                )?;
            }
        }
        let ty = self.get_type_from_type_node(node)?;
        let name_type = self.mapped_name(ty)?;
        let value = match name_type {
            Some(name) => name,
            None => self.mapped_constraint(ty)?,
        };
        let location = match name {
            Some(name) => name,
            None => self.mapped_constraint_node(ty)?,
        };
        self.check_assignable_at(value, self.builtins.string_number_symbol_type, location)
    }
}

fn modified_readonly(original: bool, modifiers: u32) -> bool {
    if modifiers & INCLUDE_READONLY != 0 {
        true
    } else if modifiers & EXCLUDE_READONLY != 0 {
        false
    } else {
        original
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.isValidIndexKeyType
    pub(crate) fn valid_index_key_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        let flags = self.types.flags(ty)?;
        if flags & (tf::STRING | tf::NUMBER | tf::ES_SYMBOL) != 0
            || self.is_pattern_literal_type(ty)?
        {
            return Ok(true);
        }
        if flags & tf::INTERSECTION != 0 && !self.is_generic_index_type(ty)? {
            let parts = self.types.compound_types(ty)?.clone();
            for &part in parts.iter() {
                if !self.valid_index_key_type(part)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getApparentTypeOfMappedType
    // port: tsc/internal/checker/checker.go:Checker.getResolvedApparentTypeOfMappedType
    pub(crate) fn apparent_mapped_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if let Some(cached) = self.types.mapped(ty)?.resolved_apparent_type {
            return Ok(cached);
        }
        let target = self.types.object(ty)?.target.unwrap_or(ty);
        let parameter = self.homomorphic_type_variable(target)?;
        let result = if let Some(parameter) = parameter {
            if self.mapped_name(target)?.is_none() {
                let modifiers = self.mapped_modifiers_type(ty)?;
                let constraint = if self.is_generic_mapped_type(modifiers)? {
                    Some(self.apparent_mapped_type(modifiers)?)
                } else {
                    self.base_constraint_of_type(modifiers)?
                };
                if let Some(constraint) = constraint {
                    let mut arraylike = true;
                    for part in self.distributed_types(constraint)? {
                        arraylike &= self.array_tuple_intersection(part)?;
                    }
                    if arraylike {
                        let mapper = self.prepend_type_mapping(
                            parameter,
                            constraint,
                            self.types.object(ty)?.mapper,
                        )?;
                        self.instantiate_type(target, Some(mapper))?
                    } else {
                        ty
                    }
                } else {
                    ty
                }
            } else {
                ty
            }
        } else {
            ty
        };
        self.types.mapped_mut(ty)?.resolved_apparent_type = Some(result);
        Ok(result)
    }
}
