//! Conditional-flow constraints and the `NoInfer<T>` substitution form.
use crate::{type_flags as tf, CheckerState, Error, MapperId, RelationKind, TypeId};
use ts_arena::NodeId;
use ts_ast::SyntaxKind as K;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getSubstitutionType
    pub(crate) fn substitution_type(
        &mut self,
        base: TypeId,
        constraint: TypeId,
    ) -> Result<TypeId, Error> {
        if self.types.flags(constraint)? & tf::ANY_OR_UNKNOWN != 0
            || constraint == base
            || self.types.flags(base)? & tf::ANY != 0
        {
            return Ok(base);
        }
        self.intern_substitution(base, constraint)
    }
    // port: tsc/internal/checker/checker.go:Checker.getOrCreateSubstitutionType
    fn intern_substitution(&mut self, base: TypeId, constraint: TypeId) -> Result<TypeId, Error> {
        if let Some(&result) = self
            .types
            .caches
            .substitution_types
            .get(&(base, constraint))
        {
            return Ok(result);
        }
        let result = self.types.new_type(
            tf::SUBSTITUTION,
            0,
            crate::types::Payload::Substitution(crate::types::SubstitutionData {
                base,
                constraint,
                resolved_base_constraint: None,
            }),
        )?;
        self.types
            .caches
            .substitution_types
            .insert((base, constraint), result);
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.isNoInferType
    pub(crate) fn is_no_infer_type(&self, ty: TypeId) -> Result<bool, Error> {
        Ok(self.types.flags(ty)? & tf::SUBSTITUTION != 0
            && self.types.flags(self.types.substitution(ty)?.constraint)? & tf::UNKNOWN != 0)
    }
    // port: tsc/internal/checker/checker.go:Checker.getNoInferType
    pub(crate) fn no_infer_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if self.no_infer_target(ty)? {
            self.intern_substitution(ty, self.builtins.unknown_type)
        } else {
            Ok(ty)
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.isNoInferTargetType
    fn no_infer_target(&mut self, ty: TypeId) -> Result<bool, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::UNION_OR_INTERSECTION != 0 {
            for part in self.types.types_of(ty)?.to_vec() {
                if self.no_infer_target(part)? {
                    return Ok(true);
                }
            }
        }
        if flags & tf::SUBSTITUTION != 0 && !self.is_no_infer_type(ty)? {
            return self.no_infer_target(self.types.substitution(ty)?.base);
        }
        Ok(
            flags & tf::OBJECT != 0 && !self.is_empty_anonymous_object_type(ty)?
                || flags & (tf::INSTANTIABLE & !tf::SUBSTITUTION) != 0
                    && !self.is_pattern_literal_type(ty)?,
        )
    }
    // port: tsc/internal/checker/checker.go:Checker.getSubstitutionIntersection
    pub(crate) fn substitution_intersection(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let data = *self.types.substitution(ty)?;
        if self.is_no_infer_type(ty)? {
            Ok(data.base)
        } else {
            self.get_intersection_type(&[data.constraint, data.base])
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.getActualTypeVariable
    pub(crate) fn actual_type_variable(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::SUBSTITUTION != 0 {
            return self.actual_type_variable(self.types.substitution(ty)?.base);
        }
        if flags & tf::INDEXED_ACCESS != 0 {
            let data = *self.types.indexed_access(ty)?;
            if (self.types.flags(data.object_type)? | self.types.flags(data.index_type)?)
                & tf::SUBSTITUTION
                != 0
            {
                let object = self.actual_type_variable(data.object_type)?;
                let index = self.actual_type_variable(data.index_type)?;
                return self.get_indexed_access_type(object, index, 0, None, None);
            }
        }
        Ok(ty)
    }
    pub(crate) fn instantiate_substitution(
        &mut self,
        ty: TypeId,
        mapper: MapperId,
    ) -> Result<TypeId, Error> {
        let data = *self.types.substitution(ty)?;
        let base = self.instantiate_type(data.base, Some(mapper))?;
        if self.is_no_infer_type(ty)? {
            return self.no_infer_type(base);
        }
        let constraint = self.instantiate_type(data.constraint, Some(mapper))?;
        if self.types.flags(base)? & tf::TYPE_VARIABLE != 0 && self.is_generic_type(constraint)? {
            return self.substitution_type(base, constraint);
        }
        if self.types.flags(constraint)? & tf::ANY_OR_UNKNOWN != 0 {
            return Ok(base);
        }
        let source = self.restrictive_instantiation(base)?;
        let target = self.restrictive_instantiation(constraint)?;
        if self.is_type_related_to(source, target, RelationKind::Assignable)? {
            return Ok(base);
        }
        if self.types.flags(base)? & tf::TYPE_VARIABLE != 0 {
            self.substitution_type(base, constraint)
        } else {
            self.get_intersection_type(&[constraint, base])
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.getConditionalFlowTypeOfType
    pub(crate) fn conditional_flow_type(
        &mut self,
        ty: TypeId,
        mut node: NodeId,
    ) -> Result<TypeId, Error> {
        let mut constraints = Vec::new();
        let mut covariant = true;
        loop {
            let read = self.node(node)?;
            if read.kind() == K::JSDoc || ts_ast::utilities::is_statement(self.ast(node)?, node)? {
                break;
            }
            let Some(parent) = read.parent() else { break };
            let read = self.node(parent)?;
            if read.kind() == K::Parameter {
                covariant = !covariant;
            }
            if (covariant || self.types.flags(ty)? & tf::TYPE_VARIABLE != 0)
                && read.kind() == K::ConditionalType
            {
                let data = read
                    .data_source()
                    .as_conditional_type_node()
                    .ok_or(Error::MissingLink("conditional flow node"))?;
                if data.true_type() == Some(node) {
                    let check = data
                        .check_type()
                        .ok_or(Error::MissingLink("conditional check"))?;
                    let extends = data
                        .extends_type()
                        .ok_or(Error::MissingLink("conditional extends"))?;
                    if let Some(constraint) = self.implied_constraint(ty, check, extends)? {
                        constraints.push(constraint);
                    }
                }
            } else if self.types.flags(ty)? & tf::TYPE_PARAMETER != 0
                && read.kind() == K::MappedType
            {
                let data = read
                    .data_source()
                    .as_mapped_type_node()
                    .ok_or(Error::MissingLink("mapped flow node"))?;
                if data.name_type().is_none() && read.type_node() == Some(node) {
                    let mapped = self.get_type_from_type_node(parent)?;
                    let parameter = self.mapped_parameter(mapped)?;
                    if parameter == self.actual_type_variable(ty)? {
                        if let Some(variable) = self.homomorphic_type_variable(mapped)? {
                            if let Some(constraint) = self.constraint_of_type_parameter(variable)? {
                                let mut arrays = true;
                                for part in self.types.types_of(constraint)?.to_vec() {
                                    if !self.is_array_type(part)? && !self.is_tuple_type(part)? {
                                        arrays = false;
                                        break;
                                    }
                                }
                                if arrays {
                                    constraints.push(self.get_union_type(&[
                                        self.builtins.number_type,
                                        self.builtins.numeric_string_type,
                                    ])?);
                                }
                            }
                        }
                    }
                }
            }
            node = parent;
        }
        if constraints.is_empty() {
            Ok(ty)
        } else {
            let constraint = self.get_intersection_type(&constraints)?;
            self.substitution_type(ty, constraint)
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.getImpliedConstraint
    fn implied_constraint(
        &mut self,
        ty: TypeId,
        check: NodeId,
        extends: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        if self.node(check)?.kind() == K::TupleType && self.node(extends)?.kind() == K::TupleType {
            let a = self.source_list(check, self.node(check)?.element_list())?;
            let b = self.source_list(extends, self.node(extends)?.element_list())?;
            if a.len() == 1 && b.len() == 1 {
                return self.implied_constraint(ty, a[0], b[0]);
            }
        }
        let check = self.get_type_from_type_node(check)?;
        if self.actual_type_variable(check)? == self.actual_type_variable(ty)? {
            self.get_type_from_type_node(extends).map(Some)
        } else {
            Ok(None)
        }
    }
}
