//! Base types and apparent types, including the `this` argument of a generic
//! reference. Completion flags are set only after all recursive work succeeds.

use crate::{
    object_flags as of, type_flags as tf, CheckerState, Error, TypeId, TypeList, TypeSystemEntity,
    TypeSystemPropertyName,
};
use ts_ast::{symbol_flags as sf, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getThisType
    pub(crate) fn type_from_this_node(&mut self, node: ts_arena::NodeId) -> Result<TypeId, Error> {
        let view = self.ast(node)?;
        let container = ts_ast::get_this_container(view, node, false, false)?;
        let read = view.node(container)?;
        if let Some(parent) = read.parent() {
            if matches!(
                view.node(parent)?.kind().known(),
                Some(K::ClassDeclaration | K::ClassExpression | K::InterfaceDeclaration)
            ) && read.modifier_flags(view)? & ts_ast::modifier_flags::STATIC == 0
                && (read.kind() != K::Constructor
                    || ts_ast::utilities::is_node_descendant_of(view, Some(node), read.body())?)
            {
                let symbol = self
                    .get_symbol_of_declaration(parent)?
                    .ok_or(Error::MissingLink("this container symbol"))?;
                let ty = self.get_declared_type_of_symbol(symbol)?;
                return Ok(self
                    .types
                    .interface(ty)?
                    .this_type
                    .unwrap_or(self.builtins.error_type));
            }
        }
        self.error_at(Some(node), ts_diagnostics::A_this_type_is_available_only_in_a_non_static_member_of_a_class_or_interface, vec![])?;
        Ok(self.builtins.error_type)
    }

    // port: tsc/internal/checker/checker.go:Checker.getBaseTypes
    pub(crate) fn interface_base_types(&mut self, ty: TypeId) -> Result<TypeList, Error> {
        if self.types.get(ty)?.object_flags & (of::CLASS_OR_INTERFACE | of::TUPLE) == 0 {
            return Ok([].into());
        }
        let data = self.types.interface(ty)?;
        if data.base_types_resolved {
            return Ok(data
                .resolved_base_types
                .clone()
                .unwrap_or_else(|| [].into()));
        }
        let types = &self.types;
        if !self.resolution.push(
            TypeSystemEntity::Type(ty),
            TypeSystemPropertyName::ResolvedBaseTypes,
            |entry| {
                let TypeSystemEntity::Type(ty) = entry.target else {
                    return false;
                };
                entry.property_name == TypeSystemPropertyName::ResolvedBaseTypes
                    && types
                        .interface(ty)
                        .is_ok_and(|data| data.base_types_resolved)
            },
        ) {
            return Ok(self
                .types
                .interface(ty)?
                .resolved_base_types
                .clone()
                .unwrap_or_else(|| [].into()));
        }
        let result = self.resolve_interface_base_types(ty);
        let complete = self.resolution.pop();
        if let Err(error) = result {
            self.types.interface_mut(ty)?.resolved_base_types = None;
            return Err(error);
        }
        if !complete {
            if let Some(symbol) = self.types.get(ty)?.symbol {
                for node in self
                    .symbol_declarations(symbol)?
                    .to_vec()
                    .into_iter()
                    .flatten()
                {
                    if matches!(
                        self.ast(node)?.node(node)?.kind().known(),
                        Some(K::ClassDeclaration | K::InterfaceDeclaration)
                    ) {
                        self.report_circular_base_type(node, ty)?;
                    }
                }
            }
        }
        self.types.get_mut(ty)?.object_flags &= !of::MEMBERS_RESOLVED;
        let data = self.types.interface_mut(ty)?;
        data.base_types_resolved = true;
        Ok(data
            .resolved_base_types
            .clone()
            .unwrap_or_else(|| [].into()))
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveBaseTypesOfInterface
    fn resolve_interface_base_types(&mut self, ty: TypeId) -> Result<(), Error> {
        if self.types.get(ty)?.object_flags & of::TUPLE != 0 {
            let base = self.tuple_base_type(ty)?;
            self.types.interface_mut(ty)?.resolved_base_types = Some(vec![base].into());
            return Ok(());
        }
        let symbol = self
            .types
            .get(ty)?
            .symbol
            .ok_or(Error::MissingLink("interface symbol"))?;
        if self.symbol(symbol)?.flags() & sf::CLASS != 0 {
            return self.resolve_class_base_types(ty);
        }
        for node in self
            .symbol_declarations(symbol)?
            .to_vec()
            .into_iter()
            .flatten()
        {
            if self.ast(node)?.node(node)?.kind() != K::InterfaceDeclaration {
                continue;
            }
            for base_node in self.interface_base_nodes(node)? {
                let base = self.get_type_from_type_node(base_node)?;
                let base = self.get_reduced_type(base)?;
                if base == self.builtins.error_type {
                    continue;
                }
                if !self.is_valid_base_type(base)? {
                    self.error_at(Some(base_node), ts_diagnostics::An_interface_can_only_extend_an_object_type_or_intersection_of_object_types_with_statically_known_members, vec![])?;
                } else if ty == base || self.has_base_type(base, ty)? {
                    self.report_circular_base_type(node, ty)?;
                } else {
                    let mut bases = self
                        .types
                        .interface(ty)?
                        .resolved_base_types
                        .clone()
                        .unwrap_or_else(|| [].into())
                        .to_vec();
                    bases.push(base);
                    self.types.interface_mut(ty)?.resolved_base_types = Some(bases.into());
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.reportCircularBaseType
    pub(crate) fn report_circular_base_type(
        &mut self,
        node: ts_arena::NodeId,
        ty: TypeId,
    ) -> Result<(), Error> {
        let text =
            self.type_to_string(ty, crate::type_format_flags::WRITE_ARRAY_AS_GENERIC_TYPE)?;
        self.error_at(
            Some(node),
            ts_diagnostics::Type_0_recursively_references_itself_as_a_base_type,
            vec![text],
        )?;
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.isValidBaseType
    pub(crate) fn is_valid_base_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::TYPE_PARAMETER != 0 {
            if let Some(constraint) = self.base_constraint_of_type(ty)? {
                return self.is_valid_base_type(constraint);
            }
        }
        if flags & (tf::OBJECT | tf::NON_PRIMITIVE | tf::ANY) != 0 {
            return Ok(!self.is_generic_mapped_type(ty)?);
        }
        if flags & tf::INTERSECTION != 0 {
            for &part in self.types.compound_types(ty)?.clone().iter() {
                if !self.is_valid_base_type(part)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.hasBaseType
    pub(crate) fn has_base_type(&mut self, ty: TypeId, check: TypeId) -> Result<bool, Error> {
        if self.types.get(ty)?.object_flags & (of::CLASS_OR_INTERFACE | of::REFERENCE) != 0 {
            // getTargetType returns the originating interface itself when it
            // has no type parameters or this type. Type.Target may be nil.
            let target = if self.types.object_flags(ty)? & of::REFERENCE != 0 {
                self.types.target(ty)?
            } else {
                ty
            };
            if target == check {
                return Ok(true);
            }
            for &base in self.interface_base_types(target)?.iter() {
                if self.has_base_type(base, check)? {
                    return Ok(true);
                }
            }
        } else if self.types.flags(ty)? & tf::INTERSECTION != 0 {
            for &part in self.types.compound_types(ty)?.clone().iter() {
                if self.has_base_type(part, check)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeWithThisArgument
    pub(crate) fn get_type_with_this_argument(
        &mut self,
        ty: TypeId,
        this: TypeId,
        need_apparent: bool,
    ) -> Result<TypeId, Error> {
        if self.types.get(ty)?.object_flags & of::REFERENCE != 0 {
            let target = self.types.target(ty)?;
            let arguments = self.get_type_arguments(ty)?;
            if self.types.interface(target)?.type_parameters().len() == arguments.len() {
                let mut arguments = arguments.to_vec();
                arguments.push(this);
                return self.create_type_reference(target, &arguments);
            }
            return Ok(ty);
        }
        if self.types.flags(ty)? & tf::INTERSECTION != 0 {
            let parts = self.types.compound_types(ty)?.clone();
            let mut mapped = Vec::with_capacity(parts.len());
            for &part in parts.iter() {
                mapped.push(self.get_type_with_this_argument(part, this, need_apparent)?);
            }
            return if mapped == parts.as_ref() {
                Ok(ty)
            } else {
                self.get_intersection_type(&mapped)
            };
        }
        if need_apparent {
            self.apparent_type(ty)
        } else {
            Ok(ty)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getApparentType
    pub(crate) fn apparent_type(&mut self, original: TypeId) -> Result<TypeId, Error> {
        let ty = if self.types.flags(original)? & tf::INSTANTIABLE != 0 {
            self.base_constraint_of_type(original)?
                .unwrap_or(self.builtins.unknown_type)
        } else {
            original
        };
        let record = *self.types.get(ty)?;
        if record.object_flags & of::MAPPED != 0 {
            return self.apparent_mapped_type(ty);
        }
        if record.object_flags & of::REFERENCE != 0 && ty != original {
            return self.get_type_with_this_argument(ty, original, false);
        }
        if record.flags & tf::INTERSECTION != 0 {
            if let Some(cached) = if original == ty {
                self.types.intersection(ty)?.resolved_apparent_type
            } else {
                self.query.apparent_types.get(&original).copied()
            } {
                return Ok(cached);
            }
            let result = self.get_type_with_this_argument(ty, original, true)?;
            if original == ty {
                self.types.intersection_mut(ty)?.resolved_apparent_type = Some(result);
            } else {
                self.query.apparent_types.insert(original, result);
            }
            return Ok(result);
        }
        if record.flags & tf::INDEX != 0 {
            return Ok(self.builtins.string_number_symbol_type);
        }
        if record.flags & tf::UNKNOWN != 0 && !self.options.strict_null_checks {
            return Ok(self.builtins.empty_object_type);
        }
        if record.flags
            & (tf::STRING_LIKE
                | tf::NUMBER_LIKE
                | tf::BIG_INT_LIKE
                | tf::BOOLEAN_LIKE
                | tf::ES_SYMBOL_LIKE
                | tf::NON_PRIMITIVE)
            != 0
        {
            return Ok(self.apparent_primitive_type(ty)?.unwrap_or(ty));
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getReducedApparentType
    pub(crate) fn reduced_apparent_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let reduced = self.get_reduced_type(ty)?;
        let apparent = self.apparent_type(reduced)?;
        self.get_reduced_type(apparent)
    }
}
