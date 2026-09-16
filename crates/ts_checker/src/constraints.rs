//! Lazy constraint resolution. The resolution stack distinguishes a circular
//! constraint from an unconstrained parameter; only successful work is cached.

use crate::{
    object_flags as of, type_flags as tf, CheckerState, Error, TypeId, TypeSystemEntity,
    TypeSystemPropertyName,
};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{symbol_flags as sf, SyntaxKind as K};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecursionIdentity {
    Type(TypeId),
    Symbol(SymbolId),
    Node(NodeId),
}

impl crate::TypeStore {
    pub(crate) fn base_constraint_slot(
        &self,
        ty: TypeId,
    ) -> Result<Option<&Option<TypeId>>, Error> {
        let flags = self.flags(ty)?;
        if flags & tf::STRUCTURED_TYPE != 0 {
            return Ok(Some(&self.structured(ty)?.resolved_base_constraint));
        }
        if flags & tf::TYPE_PARAMETER != 0 {
            return Ok(Some(&self.type_parameter(ty)?.resolved_base_constraint));
        }
        if flags & tf::TEMPLATE_LITERAL != 0 {
            return Ok(Some(&self.template_literal(ty)?.resolved_base_constraint));
        }
        if flags & tf::INDEX != 0 {
            return Ok(Some(&self.index_type(ty)?.resolved_base_constraint));
        }
        if flags & tf::INDEXED_ACCESS != 0 {
            return Ok(Some(&self.indexed_access(ty)?.resolved_base_constraint));
        }
        if flags & tf::STRING_MAPPING != 0 {
            return Ok(Some(&self.string_mapping(ty)?.resolved_base_constraint));
        }
        if flags & tf::SUBSTITUTION != 0 {
            return Ok(Some(&self.substitution(ty)?.resolved_base_constraint));
        }
        if flags & tf::CONDITIONAL != 0 {
            return Ok(Some(&self.conditional(ty)?.resolved_base_constraint));
        }
        Ok(None)
    }

    fn save_base_constraint(&mut self, ty: TypeId, result: TypeId) -> Result<(), Error> {
        let flags = self.flags(ty)?;
        let slot = if flags & tf::STRUCTURED_TYPE != 0 {
            &mut self.structured_mut(ty)?.resolved_base_constraint
        } else if flags & tf::TYPE_PARAMETER != 0 {
            &mut self.type_parameter_mut(ty)?.resolved_base_constraint
        } else if flags & tf::TEMPLATE_LITERAL != 0 {
            &mut self.template_literal_mut(ty)?.resolved_base_constraint
        } else if flags & tf::INDEX != 0 {
            &mut self.index_type_mut(ty)?.resolved_base_constraint
        } else if flags & tf::INDEXED_ACCESS != 0 {
            &mut self.indexed_access_mut(ty)?.resolved_base_constraint
        } else if flags & tf::STRING_MAPPING != 0 {
            &mut self.string_mapping_mut(ty)?.resolved_base_constraint
        } else if flags & tf::SUBSTITUTION != 0 {
            &mut self.substitution_mut(ty)?.resolved_base_constraint
        } else if flags & tf::CONDITIONAL != 0 {
            &mut self.conditional_mut(ty)?.resolved_base_constraint
        } else {
            return Err(Error::MissingLink("constrained type payload"));
        };
        slot.get_or_insert(result);
        Ok(())
    }
}

impl CheckerState {
    // port: tsc/internal/checker/relater.go:getRecursionIdentity
    pub(crate) fn recursion_identity(&self, ty: TypeId) -> Result<RecursionIdentity, Error> {
        let record = self.types.get(ty)?;
        if record.flags & tf::INDEXED_ACCESS != 0 {
            return self.recursion_identity(self.types.indexed_access(ty)?.object_type);
        }
        if record.flags & tf::CONDITIONAL != 0 {
            return Ok(RecursionIdentity::Node(
                self.conditional_root(self.types.conditional(ty)?.root)?
                    .node,
            ));
        }
        if record.flags & tf::OBJECT != 0
            && record.object_flags & (of::OBJECT_LITERAL | of::ARRAY_LITERAL) == 0
        {
            if record.object_flags & of::REFERENCE != 0 {
                if let Some(node) = self.types.type_reference(ty)?.node {
                    return Ok(RecursionIdentity::Node(node));
                }
            }
            if let Some(symbol) = record.symbol {
                if record.object_flags & of::FROM_TYPE_NODE == 0
                    && !(record.object_flags & of::ANONYMOUS != 0
                        && self.symbol(symbol)?.flags() & sf::CLASS != 0)
                {
                    return Ok(RecursionIdentity::Symbol(symbol));
                }
            }
            if record.object_flags & of::REFERENCE != 0
                && self.types.get(self.types.target(ty)?)?.object_flags & of::TUPLE != 0
                && record.object_flags & of::FROM_TYPE_NODE == 0
            {
                return Ok(RecursionIdentity::Type(self.types.target(ty)?));
            }
        }
        if record.flags & tf::TYPE_PARAMETER != 0 {
            if let Some(symbol) = record.symbol {
                return Ok(RecursionIdentity::Symbol(symbol));
            }
        }
        Ok(RecursionIdentity::Type(ty))
    }

    // port: tsc/internal/checker/checker.go:Checker.getConstraintDeclaration
    pub(crate) fn constraint_declaration(&self, ty: TypeId) -> Result<Option<NodeId>, Error> {
        let Some(symbol) = self.types.get(ty)?.symbol else {
            return Ok(None);
        };
        for node in self.symbol_declarations(symbol)?.iter().flatten() {
            if let Some(parameter) = self
                .ast(node)?
                .node(node)?
                .data_source()
                .as_type_parameter_declaration()
            {
                if parameter.constraint().is_some() {
                    return Ok(parameter.constraint());
                }
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getConstraintFromTypeParameter
    fn constraint_from_type_parameter(&mut self, ty: TypeId) -> Result<Option<TypeId>, Error> {
        if self.types.flags(ty)? & tf::TYPE_PARAMETER == 0 {
            return Ok(None);
        }
        let parameter = self.types.type_parameter(ty)?;
        if let Some(constraint) = parameter.constraint {
            return Ok((constraint != self.builtins.no_constraint_type).then_some(constraint));
        }
        let target = parameter.target;
        let mapper = parameter.mapper;
        let constraint = if let Some(target) = target {
            match self.constraint_of_type_parameter(target)? {
                Some(ty) => Some(self.instantiate_type(ty, mapper)?),
                None => None,
            }
        } else if let Some(node) = self.constraint_declaration(ty)? {
            let mut constraint = self.get_type_from_type_node(node)?;
            if self.types.flags(constraint)? & tf::ANY != 0
                && constraint != self.builtins.error_type
            {
                let parent = self.ast(node)?.node(node)?.parent();
                let grandparent = parent
                    .map(|parent| {
                        self.ast(parent)?
                            .node(parent)
                            .map(|read| read.parent())
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .flatten();
                let mapped = match grandparent {
                    Some(node) => self.ast(node)?.node(node)?.kind() == K::MappedType,
                    None => false,
                };
                constraint = if mapped {
                    self.builtins.string_number_symbol_type
                } else {
                    self.builtins.unknown_type
                };
            }
            Some(constraint)
        } else {
            self.inferred_parameter_constraint(ty, false)?
        };
        self.types.type_parameter_mut(ty)?.constraint =
            Some(constraint.unwrap_or(self.builtins.no_constraint_type));
        Ok(constraint)
    }

    pub(crate) fn has_non_circular_base_constraint(&mut self, ty: TypeId) -> Result<bool, Error> {
        Ok(self.resolved_base_constraint(ty, &mut Vec::new())?
            != self.builtins.circular_constraint_type)
    }

    // port: tsc/internal/checker/checker.go:Checker.getConstraintOfTypeParameter
    pub(crate) fn constraint_of_type_parameter(
        &mut self,
        ty: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        if self.resolved_base_constraint(ty, &mut Vec::new())?
            == self.builtins.circular_constraint_type
        {
            return Ok(None);
        }
        self.constraint_from_type_parameter(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getBaseConstraintOfType
    pub(crate) fn base_constraint_of_type(&mut self, ty: TypeId) -> Result<Option<TypeId>, Error> {
        if self.types.flags(ty)?
            & (tf::INSTANTIABLE_NON_PRIMITIVE
                | tf::UNION_OR_INTERSECTION
                | tf::TEMPLATE_LITERAL
                | tf::STRING_MAPPING
                | tf::INDEX)
            == 0
            && !self.is_generic_tuple_type(ty)?
        {
            return Ok(None);
        }
        self.next_base_constraint(ty, &mut Vec::new())
    }

    // port: tsc/internal/checker/checker.go:Checker.getNextBaseConstraint
    fn next_base_constraint(
        &mut self,
        ty: TypeId,
        stack: &mut Vec<RecursionIdentity>,
    ) -> Result<Option<TypeId>, Error> {
        let constraint = self.resolved_base_constraint(ty, stack)?;
        Ok((constraint != self.builtins.no_constraint_type
            && constraint != self.builtins.circular_constraint_type)
            .then_some(constraint))
    }

    // port: tsc/internal/checker/checker.go:Checker.getResolvedBaseConstraint
    pub(crate) fn resolved_base_constraint(
        &mut self,
        ty: TypeId,
        stack: &mut Vec<RecursionIdentity>,
    ) -> Result<TypeId, Error> {
        let Some(slot) = self.types.base_constraint_slot(ty)? else {
            return Ok(ty);
        };
        if let Some(constraint) = *slot {
            return Ok(constraint);
        }
        let types = &self.types;
        if !self.resolution.push(
            TypeSystemEntity::Type(ty),
            TypeSystemPropertyName::ResolvedBaseConstraint,
            |entry| {
                let TypeSystemEntity::Type(ty) = entry.target else {
                    return false;
                };
                entry.property_name == TypeSystemPropertyName::ResolvedBaseConstraint
                    && types
                        .base_constraint_slot(ty)
                        .is_ok_and(|slot| slot.is_some_and(std::option::Option::is_some))
            },
        ) {
            return Ok(self.builtins.circular_constraint_type);
        }
        let result = (|| {
            let identity = self.recursion_identity(ty)?;
            if stack.len() >= 10 && (stack.len() >= 50 || stack.contains(&identity)) {
                return Ok(None);
            }
            stack.push(identity);
            let result = self.compute_base_constraint(ty, stack);
            stack.pop();
            result
        })();
        let complete = self.resolution.pop();
        let mut constraint = result?.unwrap_or(self.builtins.no_constraint_type);
        if !complete {
            if self.types.flags(ty)? & tf::TYPE_PARAMETER != 0 {
                if let Some(node) = self.constraint_declaration(ty)? {
                    let name = self.type_to_string(ty, crate::type_format_flags::NONE)?;
                    self.error_at(
                        Some(node),
                        ts_diagnostics::Type_parameter_0_has_a_circular_constraint,
                        vec![name],
                    )?;
                }
            }
            constraint = self.builtins.circular_constraint_type;
        }
        self.types.save_base_constraint(ty, constraint)?;
        Ok(constraint)
    }

    // port: tsc/internal/checker/checker.go:Checker.computeBaseConstraint
    fn compute_base_constraint(
        &mut self,
        ty: TypeId,
        stack: &mut Vec<RecursionIdentity>,
    ) -> Result<Option<TypeId>, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::TYPE_PARAMETER != 0 {
            let constraint = self.constraint_from_type_parameter(ty)?;
            if self.types.type_parameter(ty)?.is_this_type {
                return Ok(constraint);
            }
            return match constraint {
                Some(ty) => self.next_base_constraint(ty, stack),
                None => Ok(None),
            };
        }
        if flags & tf::UNION_OR_INTERSECTION != 0 {
            let types = self.types.compound_types(ty)?.clone();
            let mut constraints = Vec::with_capacity(types.len());
            for &ty in types.iter() {
                if let Some(constraint) = self.next_base_constraint(ty, stack)? {
                    constraints.push(constraint);
                }
            }
            if constraints == types.as_ref() {
                return Ok(Some(ty));
            }
            if flags & tf::UNION != 0 && constraints.len() == types.len() {
                return self.get_union_type(&constraints).map(Some);
            }
            if flags & tf::INTERSECTION != 0 && !constraints.is_empty() {
                return self.get_intersection_type(&constraints).map(Some);
            }
            return Ok(None);
        }
        if flags & tf::TEMPLATE_LITERAL != 0 {
            let data = self.types.template_literal(ty)?;
            let texts = data.texts.clone();
            let types = data.types.clone();
            let mut constraints = Vec::with_capacity(types.len());
            for &ty in types.iter() {
                if let Some(constraint) = self.next_base_constraint(ty, stack)? {
                    constraints.push(constraint);
                }
            }
            return if constraints.len() == types.len() {
                self.get_template_literal_type(&texts, &constraints)
                    .map(Some)
            } else {
                Ok(Some(self.builtins.string_type))
            };
        }
        if flags & tf::STRING_MAPPING != 0 {
            let target = self.types.target(ty)?;
            if let Some(constraint) = self.next_base_constraint(target, stack)? {
                if constraint != target {
                    return self
                        .get_string_mapping_type(
                            self.types
                                .get(ty)?
                                .symbol
                                .ok_or(Error::MissingLink("string mapping symbol"))?,
                            constraint,
                        )
                        .map(Some);
                }
            }
            return Ok(Some(self.builtins.string_type));
        }
        if flags & tf::INDEX != 0 {
            let target = self.types.index_type(ty)?.target;
            if self.is_generic_mapped_type(target)?
                && self.mapped_name(target)?.is_some()
                && !self.mapped_keyof_constraint(target)?
            {
                let index = self.index_type_for_mapped(target, 0)?;
                return self.next_base_constraint(index, stack);
            }
            return Ok(Some(self.builtins.string_number_symbol_type));
        }
        if flags & tf::INDEXED_ACCESS != 0 {
            let data = *self.types.indexed_access(ty)?;
            if self.mapped_generic_indexed_access(ty)? {
                let substituted =
                    self.substitute_indexed_mapped(data.object_type, data.index_type)?;
                return self.next_base_constraint(substituted, stack);
            }
            let object = self.next_base_constraint(data.object_type, stack)?;
            let index = self.next_base_constraint(data.index_type, stack)?;
            let (Some(object), Some(index)) = (object, index) else {
                return Ok(None);
            };
            return match self.indexed_access_or_undefined(
                object,
                index,
                data.access_flags,
                None,
                None,
            )? {
                Some(result) => self.next_base_constraint(result, stack),
                None => Ok(None),
            };
        }
        if flags & tf::SUBSTITUTION != 0 {
            let constraint = self.substitution_intersection(ty)?;
            return self.next_base_constraint(constraint, stack);
        }
        if flags & tf::CONDITIONAL != 0 {
            let constraint = self.constraint_from_conditional(ty)?;
            return self.next_base_constraint(constraint, stack);
        }
        if self.is_generic_tuple_type(ty)? {
            let original = self.element_types(ty)?;
            let data = self.types.tuple(self.types.target(ty)?)?;
            let infos = data.element_infos.clone();
            let readonly = data.readonly;
            let mut elements = original.to_vec();
            for (i, &element) in original.iter().enumerate() {
                if self.types.flags(element)? & tf::TYPE_PARAMETER != 0
                    && infos[i].flags & crate::element_flags::VARIADIC != 0
                {
                    if let Some(constraint) = self.next_base_constraint(element, stack)? {
                        if constraint != element {
                            let parts = if self.types.flags(constraint)? & tf::UNION != 0 {
                                self.types.compound_types(constraint)?.clone()
                            } else {
                                vec![constraint].into()
                            };
                            let mut valid = true;
                            for &part in parts.iter() {
                                valid &= (self.is_array_type(part)? || self.is_tuple_type(part)?)
                                    && !self.is_generic_tuple_type(part)?;
                            }
                            if valid {
                                elements[i] = constraint;
                            }
                        }
                    }
                }
            }
            return self
                .create_tuple_type_ex(&elements, &infos, readonly)
                .map(Some);
        }
        Ok(Some(ty))
    }
}
