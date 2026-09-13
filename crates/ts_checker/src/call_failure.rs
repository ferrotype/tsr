//! Failed overloads still select a signature for contextual typing and queries.
//! Error elaboration then uses the last applicable-arity candidate in source
//! order, rather than the combined signature used for that fallback result.
use crate::{
    calls::TypedCall, signature_flags as sg, CheckerState, Error, RelationKind, SignatureId,
    TypeId, UnionReduction,
};
use std::sync::Arc;
use ts_arena::{NodeId, SymbolId};
use ts_ast::{Diagnostic, JsString, SyntaxKind as K};
use ts_diagnostics as messages;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getCandidateForOverloadFailure
    pub(crate) fn overload_failure_candidate(
        &mut self,
        state: &mut TypedCall,
    ) -> Result<SignatureId, Error> {
        self.defer_checker_node(state.node)?;
        let mut has_type_parameters = false;
        for &candidate in &state.candidates {
            has_type_parameters |= self
                .signatures
                .get(candidate)?
                .type_parameters
                .as_ref()
                .is_some_and(|p| !p.is_empty());
        }
        if state.candidates.len() == 1 || has_type_parameters {
            let mut best = None;
            let mut longest = 0;
            for (index, &candidate) in state.candidates.iter().enumerate() {
                let count = self.parameter_count(candidate)?;
                if self.effective_rest_parameter(candidate)? || count >= state.args.len() {
                    best = Some(index);
                    break;
                }
                if best.is_none() || count > longest {
                    best = Some(index);
                    longest = count;
                }
            }
            let index = best.ok_or(Error::MissingLink("overload failure candidate"))?;
            let candidate = state.candidates[index];
            let parameters = self
                .signatures
                .get(candidate)?
                .type_parameters
                .clone()
                .unwrap_or_else(|| [].into());
            if parameters.is_empty() {
                return Ok(candidate);
            }
            let arguments = if state.type_arguments.is_empty() {
                self.infer_call_type_arguments(state.node, candidate, &state.args)?
            } else {
                let mut arguments = Vec::with_capacity(parameters.len());
                for &node in state.type_arguments.iter().take(parameters.len()) {
                    arguments.push(self.get_type_from_type_node(node)?);
                }
                for &parameter in &parameters[arguments.len()..] {
                    let default = self.resolved_type_parameter_default(parameter)?;
                    arguments.push(
                        if default != self.builtins.no_constraint_type
                            && default != self.builtins.circular_constraint_type
                        {
                            default
                        } else {
                            self.constraint_of_type_parameter(parameter)?
                                .unwrap_or(self.builtins.unknown_type)
                        },
                    );
                }
                arguments
            };
            let result = self.create_signature_instantiation(candidate, &arguments)?;
            state.candidates[index] = result;
            return Ok(result);
        }
        self.union_of_failed_signatures(&state.candidates)
    }

    // port: tsc/internal/checker/checker.go:Checker.createUnionOfSignaturesForOverloadFailure
    fn union_of_failed_signatures(
        &mut self,
        candidates: &[SignatureId],
    ) -> Result<SignatureId, Error> {
        let mut this_parameters = Vec::new();
        for &candidate in candidates {
            if let Some(this) = self.signatures.get(candidate)?.this_parameter {
                this_parameters.push(this);
            }
        }
        let this = if this_parameters.is_empty() {
            None
        } else {
            let mut types = Vec::with_capacity(this_parameters.len());
            for &parameter in &this_parameters {
                types.push(self.type_of_parameter(parameter)?);
            }
            Some(self.combined_failed_parameter(&this_parameters, &types)?)
        };
        let mut minimum = usize::MAX;
        let mut maximum = 0;
        for &candidate in candidates {
            let signature = self.signatures.get(candidate)?;
            let count = signature.parameters.as_deref().unwrap_or_default().len()
                - usize::from(signature.flags & sg::HAS_REST_PARAMETER != 0);
            minimum = minimum.min(count);
            maximum = maximum.max(count);
        }
        let mut parameters = Vec::with_capacity(maximum);
        for index in 0..maximum {
            let mut symbols = Vec::new();
            let mut types = Vec::new();
            for &candidate in candidates {
                let signature = self.signatures.get(candidate)?;
                let params = signature.parameters.as_deref().unwrap_or_default();
                let symbol = if signature.flags & sg::HAS_REST_PARAMETER != 0 {
                    params.get(index.min(params.len() - 1))
                } else {
                    params.get(index)
                };
                if let Some(&symbol) = symbol {
                    symbols.push(symbol);
                }
            }
            for &candidate in candidates {
                if let Some(ty) = self.parameter_type_at(candidate, index)? {
                    types.push(ty);
                }
            }
            parameters.push(self.combined_failed_parameter(&symbols, &types)?);
        }
        let mut rest_symbols = Vec::new();
        for &candidate in candidates {
            let signature = self.signatures.get(candidate)?;
            if signature.flags & sg::HAS_REST_PARAMETER != 0 {
                rest_symbols.push(
                    *signature
                        .parameters
                        .as_deref()
                        .unwrap_or_default()
                        .last()
                        .ok_or(Error::MissingLink("failed rest parameter"))?,
                );
            }
        }
        let mut flags = sg::IS_SIGNATURE_CANDIDATE_FOR_OVERLOAD_FAILURE;
        if !rest_symbols.is_empty() {
            let mut types = Vec::new();
            for &candidate in candidates {
                if let Some(ty) = self.failed_signature_rest_element(candidate)? {
                    types.push(ty);
                }
            }
            let element = self.get_union_type_ex(&types, UnionReduction::Subtype, None, None)?;
            let ty = self.create_array_type(element, false)?;
            parameters.push(self.create_symbol_with_type(rest_symbols[0], ty)?);
            flags |= sg::HAS_REST_PARAMETER;
        }
        let mut returns = Vec::with_capacity(candidates.len());
        for &candidate in candidates {
            flags |= self.signatures.get(candidate)?.flags & sg::HAS_LITERAL_TYPES;
            returns.push(self.return_type_of_signature(candidate)?);
        }
        let returned = self.get_intersection_type(&returns)?;
        let declaration = self
            .signatures
            .get(
                *candidates
                    .first()
                    .ok_or(Error::MissingLink("failed signatures"))?,
            )?
            .declaration;
        self.signatures.new_signature(
            flags,
            declaration,
            None,
            this,
            Some(parameters.into()),
            Some(returned),
            None,
            minimum as i32,
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.createCombinedSymbolFromTypes
    fn combined_failed_parameter(
        &mut self,
        sources: &[SymbolId],
        types: &[TypeId],
    ) -> Result<SymbolId, Error> {
        let ty = self.get_union_type_ex(types, UnionReduction::Subtype, None, None)?;
        self.create_symbol_with_type(
            *sources
                .first()
                .ok_or(Error::MissingLink("combined parameter source"))?,
            ty,
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.tryGetRestTypeOfSignature
    fn failed_signature_rest_element(
        &mut self,
        signature: SignatureId,
    ) -> Result<Option<TypeId>, Error> {
        let signature = self.signatures.get(signature)?;
        if signature.flags & sg::HAS_REST_PARAMETER == 0 {
            return Ok(None);
        }
        let symbol = *signature
            .parameters
            .as_deref()
            .unwrap_or_default()
            .last()
            .ok_or(Error::MissingLink("failed rest parameter"))?;
        let mut rest = self.get_type_of_symbol(symbol)?;
        if self.is_tuple_type(rest)? {
            let target = self.types.tuple(self.types.target(rest)?)?;
            if target.combined_flags & crate::element_flags::VARIABLE == 0 {
                return Ok(None);
            }
            let start = target.fixed_length as usize;
            let Some(element) = self.tuple_slice_element_type(rest, start, 0, false)? else {
                return Ok(None);
            };
            rest = element;
        }
        self.index_info_of_type(rest, self.builtins.number_type)?
            .map(|index| {
                self.signatures
                    .index_info(index)
                    .map(|info| info.value_type)
            })
            .transpose()
    }

    // port: tsc/internal/checker/checker.go:Checker.reportCallResolutionErrors
    pub(crate) fn report_typed_call_failure(
        &mut self,
        state: &TypedCall,
        signatures: &[SignatureId],
    ) -> Result<(), Error> {
        if let Some(&last) = state.argument_errors.last() {
            let mut diagnostics = Vec::new();
            self.collect_signature_applicability_errors(
                state.node,
                &state.args,
                last,
                RelationKind::Assignable,
                true,
                0,
                &mut diagnostics,
            )?;
            for mut diagnostic in diagnostics {
                if state.argument_errors.len() > 1 {
                    diagnostic = Diagnostic::chain(
                        Some(Arc::new(diagnostic)),
                        messages::The_last_overload_gave_the_following_error,
                        vec![],
                    );
                    diagnostic = Diagnostic::chain(
                        Some(Arc::new(diagnostic)),
                        messages::No_overload_matches_this_call,
                        vec![],
                    );
                }
                if self.ast(state.node)?.node(state.node)?.kind() == K::BinaryExpression {
                    diagnostic=Diagnostic::chain(Some(Arc::new(diagnostic)),messages::The_left_hand_side_of_an_instanceof_expression_must_be_assignable_to_the_first_argument_of_the_right_hand_side_s_Symbol_hasInstance_method,vec![]);
                }
                if state.argument_errors.len() > 1 {
                    if let Some(declaration) = self.signatures.get(last)?.declaration {
                        diagnostic
                            .related_information
                            .push(Arc::new(self.diagnostic_for_node(
                                Some(declaration),
                                messages::The_last_overload_is_declared_here,
                                vec![],
                            )?));
                    }
                }
                self.add_implementation_success_elaboration(state, last, &mut diagnostic)?;
                self.add_diagnostic(diagnostic)?;
            }
        } else if let Some(arity) = state.arity_error {
            self.report_call_arity(state.node, &state.args, arity)?;
        } else if let Some(constraint) = state.constraint_error {
            self.call_type_arguments(constraint, &state.type_arguments, true)?;
        } else {
            let mut correct = Vec::new();
            for &signature in signatures {
                let parameters = self
                    .signatures
                    .get(signature)?
                    .type_parameters
                    .clone()
                    .unwrap_or_else(|| [].into());
                if state.type_arguments.is_empty()
                    || state.type_arguments.len() >= self.min_type_argument_count(&parameters)?
                        && state.type_arguments.len() <= parameters.len()
                {
                    correct.push(signature);
                }
            }
            if correct.is_empty() {
                self.report_overload_type_arity(
                    state.node,
                    signatures,
                    state.type_arguments.len(),
                )?;
            } else {
                self.report_call_arity_for_signatures(state.node, &state.args, &correct)?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.addImplementationSuccessElaboration
    fn add_implementation_success_elaboration(
        &mut self,
        state: &TypedCall,
        failed: SignatureId,
        diagnostic: &mut Diagnostic,
    ) -> Result<(), Error> {
        let Some(declaration) = self.signatures.get(failed)?.declaration else {
            return Ok(());
        };
        let Some(symbol) = self.get_symbol_of_declaration(declaration)? else {
            return Ok(());
        };
        let declarations = self
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect::<Vec<_>>();
        if declarations.len() <= 1 {
            return Ok(());
        }
        for declaration in declarations {
            let read = self.ast(declaration)?.node(declaration)?;
            if ts_ast::utilities::is_function_like(Some(&read)) {
                if let Some(body) = read.body() {
                    if self.ast(body)?.node(body)?.pos() != self.ast(body)?.node(body)?.end() {
                        let signature = self.signature_from_declaration(declaration)?;
                        let mut local = state.clone();
                        local.candidates = vec![signature];
                        local.single_non_generic = self
                            .signatures
                            .get(signature)?
                            .type_parameters
                            .as_ref()
                            .is_none_or(|p| p.is_empty());
                        if self
                            .choose_typed_call(&mut local, RelationKind::Assignable)?
                            .is_some()
                        {
                            diagnostic.related_information.push(Arc::new(self.diagnostic_for_node(Some(declaration),messages::The_call_would_have_succeeded_against_this_implementation_but_implementation_signatures_of_overloads_are_not_externally_visible,vec![])?));
                        }
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeArgumentArityError
    fn report_overload_type_arity(
        &mut self,
        node: NodeId,
        signatures: &[SignatureId],
        count: usize,
    ) -> Result<(), Error> {
        if signatures.len() == 1 {
            let parameters = self
                .signatures
                .get(signatures[0])?
                .type_parameters
                .clone()
                .unwrap_or_else(|| [].into());
            return self.report_call_type_arity(node, &parameters, count);
        }
        let mut below = None;
        let mut above = None;
        for &signature in signatures {
            let parameters = self
                .signatures
                .get(signature)?
                .type_parameters
                .clone()
                .unwrap_or_else(|| [].into());
            let minimum = self.min_type_argument_count(&parameters)?;
            if minimum > count {
                above = Some(above.map_or(minimum, |old: usize| old.min(minimum)));
            } else if parameters.len() < count {
                below =
                    Some(below.map_or(parameters.len(), |old: usize| old.max(parameters.len())));
            }
        }
        let string = |number: usize| JsString::from_bytes(number.to_string().as_bytes());
        let (message,args)=match (below,above) {
            (Some(below),Some(above))=>(messages::No_overload_expects_0_type_arguments_but_overloads_do_exist_that_expect_either_1_or_2_type_arguments,vec![string(count),string(below),string(above)]),
            (below,above)=>(messages::Expected_0_type_arguments_but_got_1,vec![string(below.or(above).ok_or(Error::MissingLink("overload type arity range"))?),string(count)]),
        };
        let view = self.ast(node)?;
        let source = ts_ast::utilities::get_source_file_of_node(view, Some(node))?
            .ok_or(Error::MissingLink("type arity source"))?;
        let list = view
            .node(node)?
            .type_argument_list()
            .ok_or(Error::MissingLink("type arity list"))?;
        let loc = view.list(list)?.loc();
        let start = ts_scanner::skip_trivia(view.source_file(source)?.text().as_bytes(), loc.pos());
        self.add_diagnostic(Diagnostic::new(
            Some(source),
            ts_core::TextRange::new(start, loc.end()),
            message,
            args,
        ))?;
        Ok(())
    }
}
