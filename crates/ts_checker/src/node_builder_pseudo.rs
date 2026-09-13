//! Semantic validation of syntactic declaration type skeletons. The pseudo
//! tree preserves source order and errors; identity checks use the checker.
use super::NodeBuilder;
use crate::{type_flags as tf, CheckerState, Error, RelationKind, SignatureId, TypeId};
use ts_arena::NodeId;
use ts_ast::{symbol_flags as sf, SyntaxKind as K};
use ts_printer::emit_resolver::DeclarationTrackerEvent as Event;
use ts_pseudochecker::{
    PseudoObjectElementData as E, PseudoParameter, PseudoType, PseudoTypeData as P,
};

impl ts_pseudochecker::Host for CheckerState {
    type Error = Error;
    fn ast(&self, node: NodeId) -> Result<ts_ast::AstView<'_>, Error> {
        self.ast(node)
    }
    fn raw_symbol_declarations(&self, node: NodeId) -> Result<Option<Vec<NodeId>>, Error> {
        self.raw_declaration_symbol(node)?
            .map(|symbol| {
                self.symbol_declarations(symbol)
                    .map(|items| items.iter().flatten().collect())
            })
            .transpose()
    }
}

impl CheckerState {
    pub(crate) fn pseudo_checker(&self) -> ts_pseudochecker::PseudoChecker {
        ts_pseudochecker::PseudoChecker::new(
            self.options.strict_null_checks,
            self.options.exact_optional_property_types,
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.getRegularTypeOfExpression
    pub(crate) fn regular_type_of_expression(&mut self, mut node: NodeId) -> Result<TypeId, Error> {
        if ts_ast::utilities_middle::is_right_side_of_qualified_name_or_property_access(
            self.ast(node)?,
            node,
        )? {
            node = self
                .ast(node)?
                .node(node)?
                .parent()
                .ok_or(Error::MissingLink("expression parent"))?;
        }
        let ty = self.get_type_of_expression(node)?;
        self.get_regular_type_of_literal_type(ty)
    }
}

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/pseudotypenodebuilder.go:NodeBuilderImpl.pseudoTypeToType
    pub(super) fn pseudo_type_to_type(
        &mut self,
        pseudo: &PseudoType,
    ) -> Result<Option<TypeId>, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.pseudo_type_to_type_worker(pseudo)
        })
    }
    fn pseudo_type_to_type_worker(&mut self, pseudo: &PseudoType) -> Result<Option<TypeId>, Error> {
        let ty = match pseudo.as_ref() {
            P::Direct { type_node } => self.checker.get_type_from_type_node(*type_node)?,
            P::Inferred {
                expression,
                is_signature_return,
                ..
            } => {
                if *is_signature_return {
                    let sig = self.checker.signature_from_declaration(*expression)?;
                    self.checker.return_type_of_signature(sig)?
                } else {
                    let ty = self.checker.regular_type_of_expression(*expression)?;
                    self.checker.widened_type(ty)?
                }
            }
            P::MaybeConstLocation {
                node,
                const_type,
                regular_type,
            } => {
                return if self.checker.is_const_context(*node)? {
                    self.pseudo_type_to_type(const_type)
                } else {
                    self.pseudo_type_to_type(regular_type)
                };
            }
            P::Union { types } => {
                let mut result = Vec::new();
                let mut elided = false;
                for item in types {
                    if !self.checker.options.strict_null_checks
                        && matches!(item.as_ref(), P::Undefined | P::Null)
                    {
                        elided = true;
                        continue;
                    }
                    let Some(ty) = self.pseudo_type_to_type(item)? else {
                        return Ok(None);
                    };
                    result.push(ty);
                }
                match result.as_slice() {
                    [] => {
                        if elided {
                            self.checker.builtins.any_type
                        } else {
                            self.checker.builtins.never_type
                        }
                    }
                    [ty] => *ty,
                    _ => self.checker.get_union_type(&result)?,
                }
            }
            P::Undefined => self.checker.builtins.undefined_widening_type,
            P::Null => self.checker.builtins.null_widening_type,
            P::Any => self.checker.builtins.any_type,
            P::String => self.checker.builtins.string_type,
            P::Number => self.checker.builtins.number_type,
            P::BigInt => self.checker.builtins.bigint_type,
            P::Boolean => self.checker.builtins.boolean_type,
            P::False => self.checker.builtins.false_type,
            P::True => self.checker.builtins.true_type,
            P::StringLiteral { node } | P::NumericLiteral { node } | P::BigIntLiteral { node } => {
                self.checker.regular_type_of_expression(*node)?
            }
            P::NoResult { .. }
            | P::ObjectLiteral { .. }
            | P::SingleCallSignature(_)
            | P::Tuple { .. } => return Ok(None),
        };
        Ok(Some(ty))
    }

    fn pseudo_parent(&self, node: NodeId) -> Result<NodeId, Error> {
        self.checker
            .ast(node)?
            .node(node)?
            .parent()
            .ok_or(Error::MissingLink("pseudo source parent"))
    }
    fn pseudo_report(&mut self, report: bool, node: NodeId) {
        if report {
            self.report(Event::InferenceFallback(node));
        }
    }

    // port: tsc/internal/checker/pseudotypenodebuilder.go:NodeBuilderImpl.pseudoTypeEquivalentToType
    pub(super) fn pseudo_type_equivalent(
        &mut self,
        pseudo: &PseudoType,
        ty: Option<TypeId>,
        optional: bool,
        report: bool,
    ) -> Result<bool, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.pseudo_type_equivalent_worker(pseudo, ty, optional, report)
        })
    }
    fn pseudo_type_equivalent_worker(
        &mut self,
        pseudo: &PseudoType,
        ty: Option<TypeId>,
        optional: bool,
        report: bool,
    ) -> Result<bool, Error> {
        if let Some(ty) = ty {
            if self.checker.is_error_type(ty)? {
                return Ok(true);
            }
        }
        let from = self.pseudo_type_to_type(pseudo)?;
        if from == ty {
            return Ok(true);
        }
        let stripped = match ty {
            Some(ty) if optional => Some(
                self.checker
                    .type_with_facts(ty, crate::type_facts::NE_UNDEFINED)?,
            ),
            _ => ty,
        };
        if let (Some(from), Some(ty)) = (from, ty) {
            if optional {
                let stripped = stripped.expect("type was present");
                if from == stripped
                    || self.checker.types.flags(from)?
                        & self.checker.types.flags(stripped)?
                        & tf::UNION
                        != 0
                        && self.checker.is_type_related_to(
                            from,
                            stripped,
                            RelationKind::Identity,
                        )?
                {
                    return Ok(true);
                }
            }
            if self.checker.get_regular_type_of_literal_type(from)?
                == self.checker.get_regular_type_of_literal_type(ty)?
            {
                return Ok(true);
            }
            if self.checker.types.flags(from)? & self.checker.types.flags(ty)? & tf::UNION != 0
                && self
                    .checker
                    .is_type_related_to(from, ty, RelationKind::Identity)?
            {
                return Ok(true);
            }
        }
        match pseudo.as_ref() {
            P::Inferred {
                expression,
                error_nodes,
                ..
            } => {
                if error_nodes.is_empty() {
                    self.pseudo_report(report, *expression);
                } else {
                    for &node in error_nodes {
                        self.pseudo_report(report, node);
                    }
                }
                Ok(false)
            }
            P::ObjectLiteral { elements } => {
                let Some(stripped) = stripped else {
                    return Ok(false);
                };
                let properties = self.checker.get_properties_of_type(stripped)?;
                let mut declarations = 0;
                for &symbol in &properties {
                    declarations += self.checker.symbol_declarations(symbol)?.len();
                }
                if declarations != elements.len() {
                    return Ok(false);
                }
                for element in elements {
                    let parent = self.pseudo_parent(element.name)?;
                    let mut target = None;
                    if let Some(symbol) = self.checker.raw_declaration_symbol(parent)? {
                        let name = self.checker.symbol(symbol)?.name_to_owned();
                        target =
                            self.checker
                                .constituent_property(stripped, name.as_bytes(), false)?;
                    }
                    if target.is_none() {
                        for &symbol in &properties {
                            if let Some(decl) = self.checker.symbol(symbol)?.value_declaration() {
                                if self.checker.ast(decl)?.node(decl)?.name() == Some(element.name)
                                {
                                    target = Some(symbol);
                                    break;
                                }
                            }
                        }
                    }
                    let Some(target) = target else {
                        self.pseudo_report(report, parent);
                        return Ok(false);
                    };
                    let target_optional = self.checker.symbol(target)?.flags() & sf::OPTIONAL != 0;
                    if target_optional != element.optional {
                        self.pseudo_report(report, parent);
                        return Ok(false);
                    }
                    let prop = self.checker.get_type_of_symbol(target)?;
                    let prop = self.checker.remove_missing_type(prop, target_optional)?;
                    match &element.data {
                        E::PropertyAssignment { ty, .. } => {
                            if !self.pseudo_type_equivalent(
                                ty,
                                Some(prop),
                                element.optional,
                                false,
                            )? {
                                if let P::Inferred { error_nodes, .. } = ty.as_ref() {
                                    if error_nodes.is_empty() {
                                        self.pseudo_report(report, parent);
                                    } else {
                                        for &node in error_nodes {
                                            self.pseudo_report(report, node);
                                        }
                                    }
                                } else if !is_structural(ty) {
                                    self.pseudo_report(report, parent);
                                }
                                return Ok(false);
                            }
                        }
                        E::Method(method) => {
                            let Some(sig) = self.checker.call_single_signature(prop)? else {
                                continue;
                            };
                            if !self.pseudo_parameters_equivalent(
                                &method.parameters,
                                sig,
                                report,
                                parent,
                            )? {
                                return Ok(false);
                            }
                            let equal = if let Some(predicate) =
                                self.checker.type_predicate_of_signature(sig)?
                            {
                                self.pseudo_return_matches_predicate(
                                    &method.return_type,
                                    predicate,
                                )?
                            } else {
                                let ret = self.checker.return_type_of_signature(sig)?;
                                self.pseudo_type_equivalent(
                                    &method.return_type,
                                    Some(ret),
                                    false,
                                    false,
                                )?
                            };
                            if !equal {
                                self.pseudo_report(report, parent);
                                return Ok(false);
                            }
                        }
                        E::GetAccessor { ty, .. } => {
                            if !self.pseudo_type_equivalent(ty, Some(prop), false, false)? {
                                self.pseudo_report(report, parent);
                                return Ok(false);
                            }
                        }
                        E::SetAccessor { parameter, .. } => {
                            let write = self.checker.write_type_of_symbol(target)?;
                            if !self.pseudo_type_equivalent(
                                &parameter.ty,
                                Some(write),
                                false,
                                false,
                            )? {
                                self.pseudo_report(report, parent);
                                return Ok(false);
                            }
                        }
                    }
                }
                Ok(true)
            }
            P::Tuple { elements } => {
                let Some(ty) = stripped else {
                    return Ok(false);
                };
                if !self.checker.is_tuple_type(ty)? {
                    return Ok(false);
                }
                let target = self.checker.types.target(ty)?;
                if self.checker.types.tuple(target)?.combined_flags
                    & crate::element_flags::NON_REQUIRED
                    != 0
                {
                    return Ok(false);
                }
                let args = self.checker.get_type_arguments(ty)?;
                if elements.len() != args.len() {
                    return Ok(false);
                }
                for (element, &arg) in elements.iter().zip(args.iter()) {
                    if !self.pseudo_type_equivalent(element, Some(arg), false, report)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            P::SingleCallSignature(pseudo) => {
                let Some(ty) = stripped else {
                    return Ok(false);
                };
                let Some(sig) = self.checker.call_single_signature(ty)? else {
                    return Ok(false);
                };
                if self
                    .checker
                    .signatures
                    .get(sig)?
                    .type_parameters
                    .as_deref()
                    .unwrap_or_default()
                    .len()
                    != pseudo.type_parameters.len()
                {
                    self.pseudo_report(report, pseudo.signature);
                    return Ok(false);
                }
                if !self.pseudo_parameters_equivalent(
                    &pseudo.parameters,
                    sig,
                    report,
                    pseudo.signature,
                )? {
                    return Ok(false);
                }
                if let Some(predicate) = self.checker.type_predicate_of_signature(sig)? {
                    if !self.pseudo_return_matches_predicate(&pseudo.return_type, predicate)? {
                        self.pseudo_report(report, pseudo.signature);
                        return Ok(false);
                    }
                } else {
                    let ret = self.checker.return_type_of_signature(sig)?;
                    if !self.pseudo_type_equivalent(
                        &pseudo.return_type,
                        Some(ret),
                        false,
                        report,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            P::NoResult { declaration } => {
                self.pseudo_report(report, *declaration);
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/pseudotypenodebuilder.go:NodeBuilderImpl.pseudoParametersEquivalentToParameters
    fn pseudo_parameters_equivalent(
        &mut self,
        mut params: &[PseudoParameter],
        sig: SignatureId,
        report: bool,
        location: NodeId,
    ) -> Result<bool, Error> {
        let data = self.checker.signatures.get(sig)?.clone();
        if let Some(this) = data.this_parameter {
            let Some(first) = params.first() else {
                self.pseudo_report(report, location);
                return Ok(false);
            };
            if self.checker.ast(first.name)?.node(first.name)?.kind() != K::Identifier
                || self
                    .checker
                    .ast(first.name)?
                    .node_text(first.name)?
                    .as_bytes()
                    != b"this"
            {
                self.pseudo_report(report, location);
                return Ok(false);
            }
            let ty = self.checker.type_of_parameter(this)?;
            if !self.pseudo_type_equivalent(&first.ty, Some(ty), first.optional, false)? {
                let parent = self.pseudo_parent(first.name)?;
                self.pseudo_report(report, parent);
                return Ok(false);
            }
            params = &params[1..];
        }
        let target = data.parameters.as_deref().unwrap_or_default();
        if params.len() != target.len() {
            self.pseudo_report(report, location);
            return Ok(false);
        }
        for (param, &symbol) in params.iter().zip(target.iter()) {
            let decl = self
                .checker
                .symbol(symbol)?
                .value_declaration()
                .ok_or(Error::MissingLink("pseudo target parameter declaration"))?;
            let parent = self.pseudo_parent(param.name)?;
            if param.optional != self.checker.is_optional_parameter(decl)? {
                self.pseudo_report(report, parent);
                return Ok(false);
            }
            let ty = self.checker.type_of_parameter(symbol)?;
            if !self.pseudo_type_equivalent(&param.ty, Some(ty), param.optional, false)? {
                self.pseudo_report(report, parent);
                return Ok(false);
            }
        }
        Ok(true)
    }

    // port: tsc/internal/checker/pseudotypenodebuilder.go:NodeBuilderImpl.pseudoReturnTypeMatchesPredicate
    pub(super) fn pseudo_return_matches_predicate(
        &mut self,
        pseudo: &PseudoType,
        predicate: crate::TypePredicateId,
    ) -> Result<bool, Error> {
        let P::Direct { type_node } = pseudo.as_ref() else {
            return Ok(false);
        };
        let read = self.checker.ast(*type_node)?.node(*type_node)?;
        if read.kind() != K::TypePredicate {
            return Ok(false);
        }
        let data = read
            .data_source()
            .as_type_predicate_node()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .to_owned();
        let predicate = self.checker.signatures.predicate(predicate)?.clone();
        use crate::TypePredicateKind as PK;
        if data.asserts_modifier.is_some()
            != matches!(predicate.kind, PK::AssertsThis | PK::AssertsIdentifier)
        {
            return Ok(false);
        }
        let name = data
            .parameter_name
            .ok_or(Error::MissingLink("predicate parameter name"))?;
        let is_this = self.checker.ast(name)?.node(name)?.kind() == K::ThisType;
        if is_this != matches!(predicate.kind, PK::This | PK::AssertsThis) {
            return Ok(false);
        }
        if !is_this
            && self.checker.ast(name)?.node_text(name)?.as_bytes()
                != predicate.parameter_name.as_bytes()
        {
            return Ok(false);
        }
        match (data.r#type, predicate.t) {
            (Some(node), Some(ty)) => {
                let actual = self.checker.get_type_from_type_node(node)?;
                self.checker
                    .is_type_related_to(actual, ty, RelationKind::Identity)
            }
            (None, None) => Ok(true),
            _ => Ok(false),
        }
    }
}

// port: tsc/internal/checker/pseudotypenodebuilder.go:isStructuralPseudoType
fn is_structural(pseudo: &PseudoType) -> bool {
    match pseudo.as_ref() {
        P::ObjectLiteral { .. } | P::Tuple { .. } | P::SingleCallSignature(_) => true,
        P::MaybeConstLocation {
            const_type,
            regular_type,
            ..
        } => is_structural(const_type) || is_structural(regular_type),
        _ => false,
    }
}
