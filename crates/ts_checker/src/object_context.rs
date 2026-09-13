//! Contextual object properties preserve intersections, mapped substitutions,
//! and index signatures without reducing the contextual union prematurely.
use crate::{type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{check_flags as cf, symbol_flags as sf, JsString, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getTypeOfPropertyOfType
    pub(crate) fn property_type(
        &mut self,
        ty: TypeId,
        name: &[u8],
    ) -> Result<Option<TypeId>, Error> {
        self.constituent_property(ty, name, false)?
            .map(|symbol| self.get_type_of_symbol(symbol))
            .transpose()
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeOfPropertyOfContextualType
    pub(crate) fn type_of_property_of_contextual_type(
        &mut self,
        ty: TypeId,
        name: &[u8],
    ) -> Result<Option<TypeId>, Error> {
        self.contextual_property_type_ex(ty, name, None)
    }
    // port: tsc/internal/checker/checker.go:Checker.getTypeOfPropertyOfContextualTypeEx
    pub(crate) fn contextual_property_type_ex(
        &mut self,
        ty: TypeId,
        name: &[u8],
        name_type: Option<TypeId>,
    ) -> Result<Option<TypeId>, Error> {
        self.map_type_ex(
            ty,
            &mut |checker, part| {
                if checker.types.flags(part)? & tf::INTERSECTION != 0 {
                    let mut values = Vec::new();
                    let mut candidates = Vec::new();
                    let mut ignore_indices = false;
                    for &constituent in checker.types.compound_types(part)?.clone().iter() {
                        if checker.types.flags(constituent)? & tf::OBJECT == 0 {
                            continue;
                        }
                        let value = if checker.is_generic_mapped_type(constituent)?
                            && !checker.mapped_remaps_keys(constituent)?
                        {
                            checker.contextual_mapped_property(constituent, name, name_type)?
                        } else {
                            let value = checker.contextual_concrete_property(constituent, name)?;
                            if value.is_none() {
                                if !ignore_indices {
                                    candidates.push(constituent);
                                }
                                continue;
                            }
                            ignore_indices = true;
                            candidates.clear();
                            value
                        };
                        if let Some(value) = value {
                            values.push(if checker.types.flags(value)? & tf::ANY != 0 {
                                checker.builtins.unknown_type
                            } else {
                                value
                            });
                        }
                    }
                    for candidate in candidates {
                        if let Some(value) =
                            checker.contextual_index_property(candidate, name, name_type)?
                        {
                            values.push(if checker.types.flags(value)? & tf::ANY != 0 {
                                checker.builtins.unknown_type
                            } else {
                                value
                            });
                        }
                    }
                    return match values.as_slice() {
                        [] => Ok(None),
                        [value] => Ok(Some(*value)),
                        _ => checker.get_intersection_type(&values).map(Some),
                    };
                }
                if checker.types.flags(part)? & tf::OBJECT == 0 {
                    return Ok(None);
                }
                if checker.is_generic_mapped_type(part)? && !checker.mapped_remaps_keys(part)? {
                    return checker.contextual_mapped_property(part, name, name_type);
                }
                if let Some(value) = checker.contextual_concrete_property(part, name)? {
                    return Ok(Some(value));
                }
                checker.contextual_index_property(part, name, name_type)
            },
            true,
        )
    }
    // port: tsc/internal/checker/checker.go:Checker.getIndexedMappedTypeSubstitutedTypeOfContextualType
    fn contextual_mapped_property(
        &mut self,
        ty: TypeId,
        name: &[u8],
        name_type: Option<TypeId>,
    ) -> Result<Option<TypeId>, Error> {
        let key = match name_type {
            Some(ty) => ty,
            None => self.get_string_literal_type(JsString::from_bytes(name))?,
        };
        let constraint = self.mapped_constraint(ty)?;
        if let Some(mapped_name) = self.types.mapped(ty)?.name_type {
            if self.excluded_mapped_property_name(mapped_name, key)? {
                return Ok(None);
            }
        }
        if self.excluded_mapped_property_name(constraint, key)? {
            return Ok(None);
        }
        let constraint = self
            .base_constraint_of_type(constraint)?
            .unwrap_or(constraint);
        if !self.is_type_related_to(key, constraint, crate::RelationKind::Assignable)? {
            return Ok(None);
        }
        self.substitute_indexed_mapped(ty, key).map(Some)
    }
    // port: tsc/internal/checker/checker.go:Checker.isExcludedMappedPropertyName
    fn excluded_mapped_property_name(&mut self, ty: TypeId, key: TypeId) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::CONDITIONAL != 0 {
            let truth = self.conditional_true_type(ty, false)?;
            let truth = self.get_reduced_type(truth)?;
            if self.types.flags(truth)? & tf::NEVER == 0 {
                return Ok(false);
            }
            let false_type = self.conditional_false_type(ty)?;
            let data = *self.types.conditional(ty)?;
            return Ok(self.actual_type_variable(false_type)?
                == self.actual_type_variable(data.check_type)?
                && self.is_type_related_to(
                    key,
                    data.extends_type,
                    crate::RelationKind::Assignable,
                )?);
        }
        if self.types.flags(ty)? & tf::INTERSECTION != 0 {
            for &part in self.types.compound_types(ty)?.clone().iter() {
                if self.excluded_mapped_property_name(part, key)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    // port: tsc/internal/checker/checker.go:Checker.getTypeOfConcretePropertyOfContextualType
    fn contextual_concrete_property(
        &mut self,
        ty: TypeId,
        name: &[u8],
    ) -> Result<Option<TypeId>, Error> {
        let Some(symbol) = self.constituent_property(ty, name, false)? else {
            return Ok(None);
        };
        if self.symbol(symbol)?.check_flags() & cf::MAPPED != 0
            && self
                .value_symbol_links
                .try_get(symbol)
                .is_none_or(|links| links.resolved_type.is_none())
        {
            let values = &self.value_symbol_links;
            if self
                .resolution
                .find_resolution_cycle_start_index(
                    crate::TypeSystemEntity::Symbol(symbol),
                    crate::TypeSystemPropertyName::Type,
                    |entry| {
                        let crate::TypeSystemEntity::Symbol(symbol) = entry.target else {
                            return false;
                        };
                        entry.property_name == crate::TypeSystemPropertyName::Type
                            && values
                                .try_get(symbol)
                                .is_some_and(|links| links.resolved_type.is_some())
                    },
                )
                .is_some()
            {
                return Ok(None);
            }
        }
        let optional = self.symbol(symbol)?.flags() & sf::OPTIONAL != 0;
        let ty = self.get_type_of_symbol(symbol)?;
        self.remove_missing_type(ty, optional).map(Some)
    }
    // port: tsc/internal/checker/checker.go:Checker.getTypeFromIndexInfosOfContextualType
    fn contextual_index_property(
        &mut self,
        ty: TypeId,
        name: &[u8],
        name_type: Option<TypeId>,
    ) -> Result<Option<TypeId>, Error> {
        if self.is_tuple_type(ty)? && crate::indexes::numeric_name(name).is_some_and(|n| n >= 0.0) {
            let fixed = self.types.tuple(self.types.target(ty)?)?.fixed_length;
            if let Some(rest) =
                self.tuple_slice_element_type_ex(ty, fixed as usize, 0, false, true)?
            {
                return Ok(Some(rest));
            }
        }
        let name_type = match name_type {
            Some(ty) => ty,
            None => self.get_string_literal_type(JsString::from_bytes(name))?,
        };
        self.applicable_index_info(ty, name_type)?
            .map(|index| {
                self.signatures
                    .index_info(index)
                    .map(|info| info.value_type)
            })
            .transpose()
    }
    // port: tsc/internal/checker/checker.go:Checker.getContextualTypeForObjectLiteralElement
    pub(crate) fn contextual_property_type_with_flags(
        &mut self,
        element: NodeId,
        flags: u32,
    ) -> Result<Option<TypeId>, Error> {
        let read = self.ast(element)?.node(element)?;
        if read.kind() != K::MethodDeclaration {
            if let Some(annotation) = read.type_node() {
                return self.get_type_from_type_node(annotation).map(Some);
            }
        }
        let object = read
            .parent()
            .ok_or(Error::MissingLink("object member parent"))?;
        let Some(context) = self.apparent_contextual_expression_type_ex(object, flags)? else {
            return Ok(None);
        };
        let name = self.ast(element)?.node(element)?.name();
        let dynamic = ts_ast::has_dynamic_name(self.ast(element)?, Some(element))?;
        let late = if dynamic {
            match self.late_name(element)? {
                Some(name) => {
                    let ty = self.late_name_type(name)?;
                    self.types.flags(ty)? & tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE != 0
                }
                None => false,
            }
        } else {
            false
        };
        if !dynamic || late {
            let symbol = self
                .get_symbol_of_declaration(element)?
                .ok_or(Error::MissingLink("object contextual symbol"))?;
            let text = self.symbol(symbol)?.name_to_owned();
            let key = self
                .value_symbol_links
                .try_get(symbol)
                .and_then(|links| links.name_type);
            return self.contextual_property_type_ex(context, text.as_bytes(), key);
        }
        if let Some(name) = name {
            if self.ast(name)?.node(name)?.kind() == K::ComputedPropertyName {
                let expression = self
                    .ast(name)?
                    .node(name)?
                    .expression()
                    .ok_or(Error::MissingLink("computed name expression"))?;
                let key = self.check_expression(expression)?;
                if let Some(text) = self.index_property_name(key)? {
                    if let Some(value) =
                        self.type_of_property_of_contextual_type(context, text.as_bytes())?
                    {
                        return Ok(Some(value));
                    }
                }
            }
            let key = self.literal_type_from_property_name(name)?;
            return self.map_type_ex(
                context,
                &mut |checker, part| {
                    checker
                        .applicable_index_info(part, key)?
                        .map(|index| {
                            checker
                                .signatures
                                .index_info(index)
                                .map(|info| info.value_type)
                        })
                        .transpose()
                },
                true,
            );
        }
        Ok(None)
    }
}
