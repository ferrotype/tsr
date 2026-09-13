//! Signature predicates and assertions share relation and discriminant caches.
use crate::{
    type_facts as f, type_flags as tf, CheckerState, Error, RelationKind, TypeId, TypePredicateId,
    TypePredicateKind,
};
use ts_arena::NodeId;
use ts_ast::SyntaxKind as K;
impl CheckerState {
    // port: tsc/internal/checker/flow.go:Checker.getNarrowedType
    pub(crate) fn narrowed_flow_type(
        &mut self,
        ty: TypeId,
        candidate: TypeId,
        assume: bool,
    ) -> Result<TypeId, Error> {
        self.narrowed_flow_type_ex(ty, candidate, assume, false)
    }
    pub(crate) fn narrowed_flow_type_ex(
        &mut self,
        ty: TypeId,
        candidate: TypeId,
        assume: bool,
        check_derived: bool,
    ) -> Result<TypeId, Error> {
        let cache = self.types.flags(ty)? & tf::UNION != 0;
        let key = (ty, candidate, assume, check_derived);
        if cache {
            if let Some(result) = self.flow.narrowed.get(&key) {
                return Ok(*result);
            }
        }
        let result = self.narrowed_flow_type_worker(ty, candidate, assume, check_derived)?;
        if cache {
            self.flow.narrowed.insert(key, result);
        }
        Ok(result)
    }
    // port: tsc/internal/checker/flow.go:Checker.getNarrowedTypeWorker
    fn narrowed_flow_type_worker(
        &mut self,
        mut ty: TypeId,
        candidate: TypeId,
        assume: bool,
        check_derived: bool,
    ) -> Result<TypeId, Error> {
        if !assume {
            if ty == candidate {
                return Ok(self.builtins.never_type);
            }
            if check_derived {
                return self.filter_type(ty, &mut |state, part| {
                    Ok(!state.is_type_derived_from(part, candidate)?)
                });
            }
            if self.types.flags(ty)? & tf::UNKNOWN != 0 {
                ty = self.builtins.unknown_union_type;
            }
            let true_type = self.narrowed_flow_type(ty, candidate, true)?;
            let result = self.filter_type(ty, &mut |state, part| {
                Ok(!state.flow_type_subset(part, true_type)?)
            })?;
            return Ok(if result == self.builtins.unknown_union_type {
                self.builtins.unknown_type
            } else {
                result
            });
        }
        if self.types.flags(ty)? & tf::ANY_OR_UNKNOWN != 0 || ty == candidate {
            return Ok(candidate);
        }
        let key = if self.types.flags(ty)? & tf::UNION != 0 {
            self.flow_key_property_name(ty)?
        } else {
            ts_ast::JsString::default()
        };
        let narrowed = self
            .map_type(candidate, &mut |state, part| {
                let mut matching = ty;
                if !key.is_empty() {
                    if let Some(property) =
                        state.constituent_property(part, key.as_bytes(), false)?
                    {
                        let discriminant = state.get_type_of_symbol(property)?;
                        if let Some(found) = state.flow_constituent_for_key(ty, discriminant)? {
                            matching = found;
                        }
                    }
                }
                let directly = state
                    .map_type(matching, &mut |state, old| {
                        if check_derived {
                            return Ok(Some(if state.is_type_derived_from(old, part)? {
                                old
                            } else if state.is_type_derived_from(part, old)? {
                                part
                            } else {
                                state.builtins.never_type
                            }));
                        }
                        for relation in [RelationKind::StrictSubtype, RelationKind::Subtype] {
                            if state.is_type_related_to(old, part, relation)? {
                                return Ok(Some(old));
                            }
                            if state.is_type_related_to(part, old, relation)? {
                                return Ok(Some(part));
                            }
                        }
                        Ok(Some(state.builtins.never_type))
                    })?
                    .ok_or(Error::MissingLink("direct predicate narrowed type"))?;
                if state.types.flags(directly)? & tf::NEVER == 0 {
                    return Ok(Some(directly));
                }
                state.map_type(ty, &mut |state, old| {
                    if state.maybe_type_of_kind(old, tf::INSTANTIABLE)? {
                        let constraint = state.base_constraint_of_type(old)?;
                        if constraint.is_none()
                            || if check_derived {
                                state.is_type_derived_from(part, constraint.unwrap())?
                            } else {
                                state.is_type_related_to(
                                    part,
                                    constraint.unwrap(),
                                    RelationKind::Subtype,
                                )?
                            }
                        {
                            return Ok(Some(state.get_intersection_type(&[old, part])?));
                        }
                    }
                    Ok(Some(state.builtins.never_type))
                })
            })?
            .ok_or(Error::MissingLink("predicate narrowed type"))?;
        if self.types.flags(narrowed)? & tf::NEVER == 0 {
            return Ok(narrowed);
        }
        if self.is_type_related_to(candidate, ty, RelationKind::Subtype)? {
            return Ok(candidate);
        }
        if self.is_type_related_to(ty, candidate, RelationKind::Assignable)? {
            return Ok(ty);
        }
        if self.is_type_related_to(candidate, ty, RelationKind::Assignable)? {
            return Ok(candidate);
        }
        self.get_intersection_type(&[ty, candidate])
    }
    // port: tsc/internal/checker/flow.go:Checker.getTypePredicateArgument
    fn flow_predicate_argument(
        &self,
        predicate: TypePredicateId,
        call: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let predicate = self.signatures.predicate(predicate)?;
        let read = self.ast(call)?.node(call)?;
        if matches!(
            predicate.kind,
            TypePredicateKind::Identifier | TypePredicateKind::AssertsIdentifier
        ) {
            let args = self.source_list(call, read.argument_list())?;
            return Ok(usize::try_from(predicate.parameter_index)
                .ok()
                .and_then(|index| args.get(index).copied()));
        }
        let expression = read
            .expression()
            .ok_or(Error::MissingLink("predicate call expression"))?;
        let expression = ts_ast::skip_parentheses(self.ast(expression)?, expression)?;
        let read = self.ast(expression)?.node(expression)?;
        if matches!(
            read.kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) {
            let target = read
                .expression()
                .ok_or(Error::MissingLink("predicate this argument"))?;
            return Ok(Some(ts_ast::skip_parentheses(self.ast(target)?, target)?));
        }
        Ok(None)
    }
    // port: tsc/internal/checker/flow.go:Checker.narrowTypeByTypePredicate
    pub(crate) fn narrow_flow_predicate(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        mut ty: TypeId,
        predicate: TypePredicateId,
        call: NodeId,
        assume: bool,
    ) -> Result<TypeId, Error> {
        let Some(predicate_type) = self.signatures.predicate(predicate)?.t else {
            return Ok(ty);
        };
        if self.types.flags(ty)? & tf::ANY != 0
            && ["Object", "Function"]
                .into_iter()
                .any(|name| self.query.global_types.get(name).copied() == Some(predicate_type))
        {
            return Ok(ty);
        }
        let Some(argument) = self.flow_predicate_argument(predicate, call)? else {
            return Ok(ty);
        };
        if self.matching_reference(reference, argument)? {
            return self.narrowed_flow_type(ty, predicate_type, assume);
        }
        if self.options.strict_null_checks
            && self.optional_chain_contains_reference(argument, reference)?
        {
            let nullable = self.every_type_flags(predicate_type, tf::NULLABLE)?;
            if assume && self.type_facts(predicate_type, f::EQ_UNDEFINED)? == 0
                || !assume && nullable
            {
                ty = self.adjusted_type_with_facts(ty, f::NE_UNDEFINED_OR_NULL)?;
            }
        }
        if let Some(access) = self.flow_discriminant_access(reference, declared, argument, ty)? {
            return self.narrow_discriminant(ty, access, &mut |state, part| {
                state.narrowed_flow_type(part, predicate_type, assume)
            });
        }
        Ok(ty)
    }
    // port: tsc/internal/checker/flow.go:Checker.narrowTypeByAssertion
    pub(crate) fn narrow_flow_assertion(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        ty: TypeId,
        expression: NodeId,
    ) -> Result<TypeId, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let expression = ts_ast::skip_parentheses(self.ast(expression)?, expression)?;
            let read = self.ast(expression)?.node(expression)?;
            if read.kind() == K::FalseKeyword {
                return Ok(self.builtins.unreachable_never_type);
            }
            if let Some(binary) = read.data_source().as_binary_expression() {
                let operator = binary
                    .operator_token()
                    .ok_or(Error::MissingLink("assertion operator"))?;
                let left = binary.left().ok_or(Error::MissingLink("assertion left"))?;
                let right = binary
                    .right()
                    .ok_or(Error::MissingLink("assertion right"))?;
                match self.ast(operator)?.node(operator)?.kind().known() {
                    Some(K::AmpersandAmpersandToken) => {
                        let left = self.narrow_flow_assertion(reference, declared, ty, left)?;
                        return self.narrow_flow_assertion(reference, declared, left, right);
                    }
                    Some(K::BarBarToken) => {
                        let left = self.narrow_flow_assertion(reference, declared, ty, left)?;
                        let right = self.narrow_flow_assertion(reference, declared, ty, right)?;
                        return self.get_union_type(&[left, right]);
                    }
                    _ => {}
                }
            }
            self.narrow_reference_type(reference, declared, ty, expression, true)
        })
    }
    // port: tsc/internal/checker/flow.go:Checker.narrowTypeByCallExpression
    pub(crate) fn narrow_flow_call(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        ty: TypeId,
        call: NodeId,
        assume: bool,
    ) -> Result<TypeId, Error> {
        if self.flow_has_matching_argument(call, reference)? {
            let call_chain =
                self.ast(call)?.node(call)?.flags() & ts_ast::node_flags::OPTIONAL_CHAIN != 0;
            if assume || !call_chain {
                if let Some(signature) = self.effects_signature(call)? {
                    if let Some(predicate) = self.type_predicate_of_signature(signature)? {
                        if matches!(
                            self.signatures.predicate(predicate)?.kind,
                            TypePredicateKind::This | TypePredicateKind::Identifier
                        ) {
                            return self.narrow_flow_predicate(
                                reference, declared, ty, predicate, call, assume,
                            );
                        }
                    }
                }
            }
        }
        if self.type_contains_missing(ty)?
            && matches!(
                self.ast(reference)?.node(reference)?.kind().known(),
                Some(K::PropertyAccessExpression | K::ElementAccessExpression)
            )
        {
            let callee = self
                .ast(call)?
                .node(call)?
                .expression()
                .ok_or(Error::MissingLink("flow call expression"))?;
            let read = self.ast(callee)?.node(callee)?;
            if read.kind() == K::PropertyAccessExpression {
                let object = read
                    .expression()
                    .ok_or(Error::MissingLink("flow call object"))?;
                let name = read.name().ok_or(Error::MissingLink("flow call name"))?;
                let target = self
                    .ast(reference)?
                    .node(reference)?
                    .expression()
                    .ok_or(Error::MissingLink("flow reference object"))?;
                let object = self.reference_candidate(object)?;
                let args = self.source_list(call, self.ast(call)?.node(call)?.argument_list())?;
                if self.matching_reference(target, object)?
                    && self.ast(name)?.node(name)?.kind() == K::Identifier
                    && self.ast(name)?.node_text(name)?.as_bytes() == b"hasOwnProperty"
                    && args.len() == 1
                {
                    let argument = args[0];
                    if matches!(
                        self.ast(argument)?.node(argument)?.kind().known(),
                        Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
                    ) {
                        if let Some(name) = self.flow_property_name(reference)? {
                            if self.ast(argument)?.node_text(argument)?.as_bytes()
                                == name.as_bytes()
                            {
                                return self.type_with_facts(
                                    ty,
                                    if assume {
                                        f::NE_UNDEFINED
                                    } else {
                                        f::EQ_UNDEFINED
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
        Ok(ty)
    }
    fn every_type_flags(&self, ty: TypeId, flags: u32) -> Result<bool, Error> {
        let parts = if self.types.flags(ty)? & tf::UNION != 0 {
            self.types.types_of(ty)?
        } else {
            std::slice::from_ref(&ty)
        };
        for part in parts {
            if self.types.flags(*part)? & flags == 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
