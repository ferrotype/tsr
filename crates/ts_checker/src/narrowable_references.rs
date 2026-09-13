//! Constraint substitution is permitted only at the native reference contexts.
use crate::{type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::SyntaxKind as K;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getNarrowableTypeForReference
    pub(crate) fn narrowable_reference_type(
        &mut self,
        mut ty: TypeId,
        node: NodeId,
        mode: u32,
    ) -> Result<TypeId, Error> {
        if self.is_no_infer_type(ty)? {
            ty = self.types.substitution(ty)?.base;
        }
        if mode & 2 == 0
            && self.any_type(ty, &mut |c, t| c.generic_union_constraint(t))?
            && (self.reference_constraint_position(ty, node)?
                || self.reference_context_without_generics(node, mode)?)
        {
            return self
                .map_type_ex(
                    ty,
                    &mut |c, t| Ok(Some(c.base_constraint_of_type(t)?.unwrap_or(t))),
                    false,
                )?
                .ok_or(Error::MissingLink("narrowable constraint mapping"));
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.isGenericTypeWithUnionConstraint
    fn generic_union_constraint(&mut self, ty: TypeId) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::INTERSECTION != 0 {
            for &part in self.types.compound_types(ty)?.clone().iter() {
                if self.generic_union_constraint(part)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        if self.types.flags(ty)? & tf::INSTANTIABLE == 0 {
            return Ok(false);
        }
        let constraint = self.base_constraint_of_type(ty)?.unwrap_or(ty);
        Ok(self.types.flags(constraint)? & (tf::NULLABLE | tf::UNION) != 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.isGenericTypeWithoutNullableConstraint
    fn generic_nonnullable_constraint(&mut self, ty: TypeId) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::INTERSECTION != 0 {
            for &part in self.types.compound_types(ty)?.clone().iter() {
                if self.generic_nonnullable_constraint(part)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        if self.types.flags(ty)? & tf::INSTANTIABLE == 0 {
            return Ok(false);
        }
        let constraint = self.base_constraint_of_type(ty)?.unwrap_or(ty);
        Ok(!self.maybe_type_of_kind(constraint, tf::NULLABLE)?)
    }

    // port: tsc/internal/checker/checker.go:Checker.isConstraintPosition
    fn reference_constraint_position(&mut self, ty: TypeId, node: NodeId) -> Result<bool, Error> {
        let Some(parent) = self.ast(node)?.node(node)?.parent() else {
            return Ok(false);
        };
        let read = self.ast(parent)?.node(parent)?;
        match read.kind().known() {
            Some(K::PropertyAccessExpression | K::QualifiedName) => Ok(true),
            Some(K::CallExpression | K::NewExpression) => Ok(read.expression() == Some(node)),
            Some(K::ElementAccessExpression) if read.expression() == Some(node) => {
                let argument = read
                    .data_source()
                    .as_element_access_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .argument_expression()
                    .ok_or(Error::MissingLink("index constraint argument"))?;
                if !self.any_type(ty, &mut |c, t| c.generic_nonnullable_constraint(t))? {
                    return Ok(true);
                }
                let key = self.get_type_of_expression(argument)?;
                Ok(!self.is_generic_index_type(key)?)
            }
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.hasContextualTypeWithNoGenericTypes
    fn reference_context_without_generics(
        &mut self,
        node: NodeId,
        mode: u32,
    ) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        if !matches!(
            read.kind().known(),
            Some(K::Identifier | K::PropertyAccessExpression | K::ElementAccessExpression)
        ) {
            return Ok(false);
        }
        if let Some(parent) = read.parent() {
            let parent = self.ast(parent)?.node(parent)?;
            if matches!(
                parent.kind().known(),
                Some(K::JsxOpeningElement | K::JsxSelfClosingElement)
            ) && parent.tag_name() == Some(node)
            {
                return Ok(false);
            }
        }
        let context =
            self.contextual_expression_type_ex(node, if mode & 32 != 0 { 8 } else { 0 })?;
        match context {
            Some(ty) => Ok(!self.is_generic_type(ty)?),
            None => Ok(false),
        }
    }
}
