//! Constructor narrowing uses inheritance identity; ordinary predicates retain
//! their structural subtype relation.
use crate::{
    object_flags as of, type_facts as facts, type_flags as tf, CheckerState, Error, RelationKind,
    TypeId, TypePredicateKind,
};
use tsr_arena::NodeId;
use tsr_ast::{NodeKind, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/flow.go:Checker.narrowTypeByPrivateIdentifierInInExpression
    pub(crate) fn narrow_flow_private_in(
        &mut self,
        reference: NodeId,
        ty: TypeId,
        left: NodeId,
        right: NodeId,
        assume: bool,
    ) -> Result<TypeId, Error> {
        let target = self.reference_candidate(right)?;
        if !self.matching_reference(reference, target)? {
            return Ok(ty);
        }
        let Some(symbol) = self.private_identifier_expression_symbol(left)? else {
            return Ok(ty);
        };
        let read = self.symbol(symbol)?;
        let class = read
            .parent()
            .ok_or(Error::MissingLink("private property class"))?;
        let declaration = read
            .value_declaration()
            .ok_or(Error::MissingLink("private property declaration"))?;
        let target = if tsr_ast::utilities::is_static(self.ast(declaration)?, declaration)? {
            self.get_type_of_symbol(class)?
        } else {
            self.get_declared_type_of_symbol(class)?
        };
        self.narrowed_flow_type_ex(ty, target, assume, true)
    }

    // port: tsc/internal/checker/relater.go:Checker.isTypeDerivedFrom
    pub(crate) fn is_type_derived_from(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<bool, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let source_flags = self.types.flags(source)?;
            let target_flags = self.types.flags(target)?;
            if source_flags & tf::UNION != 0 {
                for part in self.types.compound_types(source)?.to_vec() {
                    if !self.is_type_derived_from(part, target)? {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
            if target_flags & tf::UNION != 0 {
                for part in self.types.compound_types(target)?.to_vec() {
                    if self.is_type_derived_from(source, part)? {
                        return Ok(true);
                    }
                }
                return Ok(false);
            }
            if source_flags & tf::INTERSECTION != 0 {
                for part in self.types.compound_types(source)?.to_vec() {
                    if self.is_type_derived_from(part, target)? {
                        return Ok(true);
                    }
                }
                return Ok(false);
            }
            if source_flags & tf::INSTANTIABLE_NON_PRIMITIVE != 0 {
                let constraint = self
                    .base_constraint_of_type(source)?
                    .unwrap_or(self.builtins.unknown_type);
                return self.is_type_derived_from(constraint, target);
            }
            if self.is_empty_anonymous_object_type(target)? {
                return Ok(source_flags & (tf::OBJECT | tf::NON_PRIMITIVE) != 0);
            }
            if self.query.global_types.get("Object") == Some(&target) {
                return Ok(source_flags & (tf::OBJECT | tf::NON_PRIMITIVE) != 0
                    && !self.is_empty_anonymous_object_type(source)?);
            }
            if self.query.global_types.get("Function") == Some(&target) {
                return Ok(source_flags & tf::OBJECT != 0 && self.is_function_object_type(source)?);
            }
            let target_type = if self.types.object_flags(target)? & of::REFERENCE != 0 {
                self.types.target(target)?
            } else {
                target
            };
            if self.has_base_type(source, target_type)? {
                return Ok(true);
            }
            if self.is_array_type(target)?
                && self.query.global_types.get("ReadonlyArray") != Some(&target_type)
            {
                return self.is_type_derived_from(source, self.array_target(true)?);
            }
            Ok(false)
        })
    }

    // port: tsc/internal/checker/flow.go:Checker.getInstanceType
    fn flow_instance_type(&mut self, constructor: TypeId) -> Result<TypeId, Error> {
        if let Some(prototype) = self.property_type(constructor, b"prototype")? {
            if self.types.flags(prototype)? & tf::ANY == 0 {
                return Ok(prototype);
            }
        }
        let signatures = self.signatures_of_type(constructor, true)?;
        if signatures.is_empty() {
            return Ok(self.builtins.empty_object_type);
        }
        let mut returns = Vec::with_capacity(signatures.len());
        for signature in signatures {
            let erased = self.erased_signature(signature)?;
            returns.push(self.return_type_of_signature(erased)?);
        }
        self.get_union_type(&returns)
    }

    // port: tsc/internal/checker/flow.go:Checker.narrowTypeByInstanceof
    #[allow(
        clippy::nonminimal_bool,
        reason = "Keep the native predicate grouping and short-circuit order of fallible type queries"
    )]
    pub(crate) fn narrow_flow_instanceof(
        &mut self,
        reference: NodeId,
        ty: TypeId,
        expression: NodeId,
        left: NodeId,
        right: NodeId,
        assume: bool,
    ) -> Result<TypeId, Error> {
        let left = self.reference_candidate(left)?;
        if !self.matching_reference(reference, left)? {
            if assume
                && self.options.strict_null_checks
                && self.optional_chain_contains_reference(left, reference)?
            {
                return self.adjusted_type_with_facts(ty, facts::NE_UNDEFINED_OR_NULL);
            }
            return Ok(ty);
        }
        let right_type = self.get_type_of_expression(right)?;
        let global_object = self.get_global_type("Object", 0, false)?;
        if !self.is_type_derived_from(right_type, global_object)? {
            return Ok(ty);
        }
        if let Some(signature) = self.effects_signature(expression)? {
            if let Some(predicate) = self.type_predicate_of_signature(signature)? {
                let predicate = self.signatures.predicate(predicate)?;
                if predicate.kind == TypePredicateKind::Identifier && predicate.parameter_index == 0
                {
                    let candidate = predicate
                        .t
                        .ok_or(Error::MissingLink("hasInstance predicate type"))?;
                    return self.narrowed_flow_type_ex(ty, candidate, assume, true);
                }
            }
        }
        let global_function = self.get_global_type("Function", 0, false)?;
        if !self.is_type_derived_from(right_type, global_function)? {
            return Ok(ty);
        }
        let instance_type = self
            .map_type(right_type, &mut |state, part| {
                state.flow_instance_type(part).map(Some)
            })?
            .ok_or(Error::MissingLink("instance type map"))?;
        if self.types.flags(ty)? & tf::ANY != 0
            && (instance_type == global_object || instance_type == global_function)
            || !assume
                && !(self.types.flags(instance_type)? & tf::OBJECT != 0
                    && !self.is_empty_anonymous_object_type(instance_type)?)
        {
            return Ok(ty);
        }
        self.narrowed_flow_type_ex(ty, instance_type, assume, true)
    }

    // port: tsc/internal/checker/flow.go:Checker.isMatchingConstructorReference
    pub(crate) fn matching_constructor_reference(
        &mut self,
        reference: NodeId,
        expression: NodeId,
    ) -> Result<bool, Error> {
        let read = self.node(expression)?;
        let property = if read.kind() == K::PropertyAccessExpression {
            read.name()
        } else if read.kind() == K::ElementAccessExpression {
            let argument = read
                .data_source()
                .as_element_access_expression()
                .and_then(|data| data.argument_expression());
            match argument {
                Some(argument)
                    if matches!(
                        self.node(argument)?.kind().known(),
                        Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
                    ) =>
                {
                    Some(argument)
                }
                _ => None,
            }
        } else {
            None
        };
        let Some(property) = property else {
            return Ok(false);
        };
        if self.node_text(property)?.as_bytes() != b"constructor" {
            return Ok(false);
        }
        let object = read
            .expression()
            .ok_or(Error::MissingLink("constructor reference object"))?;
        self.matching_reference(reference, object)
    }

    // port: tsc/internal/checker/flow.go:Checker.narrowTypeByConstructor
    pub(crate) fn narrow_flow_constructor(
        &mut self,
        ty: TypeId,
        operator: NodeKind,
        identifier: NodeId,
        assume: bool,
    ) -> Result<TypeId, Error> {
        let equality = matches!(
            operator.known(),
            Some(K::EqualsEqualsToken | K::EqualsEqualsEqualsToken)
        );
        if assume != equality {
            return Ok(ty);
        }
        let identifier_type = self.get_type_of_expression(identifier)?;
        if self.signatures_of_type(identifier_type, false)?.is_empty()
            && !self.is_constructor_type(identifier_type)?
        {
            return Ok(ty);
        }
        let Some(candidate) = self.property_type(identifier_type, b"prototype")? else {
            return Ok(ty);
        };
        if self.types.flags(candidate)? & tf::ANY != 0
            || self.query.global_types.get("Object") == Some(&candidate)
            || self.query.global_types.get("Function") == Some(&candidate)
        {
            return Ok(ty);
        }
        if self.types.flags(ty)? & tf::ANY != 0 {
            return Ok(candidate);
        }
        self.filter_type(ty, &mut |state, source| {
            // port: tsc/internal/checker/flow.go:Checker.isConstructedBy
            if state.types.object_flags(source)? & of::CLASS != 0
                || state.types.object_flags(candidate)? & of::CLASS != 0
            {
                Ok(state.types.get(source)?.symbol == state.types.get(candidate)?.symbol)
            } else {
                state.is_type_related_to(source, candidate, RelationKind::Subtype)
            }
        })
    }
}
