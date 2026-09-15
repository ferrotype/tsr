//! Arity, type argument constraints and argument inference for call resolution.

use crate::{
    inference::priority, type_flags as tf, CheckerState, Error, InferenceId, RelationKind,
    SignatureId, TypeId,
};
use ts_arena::NodeId;
use ts_ast::{Diagnostic, Factory, JsString, SyntaxKind as K};
use ts_diagnostics as messages;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getTypeArgumentArityError
    pub(crate) fn report_call_type_arity(
        &mut self,
        node: NodeId,
        parameters: &[TypeId],
        count: usize,
    ) -> Result<(), Error> {
        let minimum = self.min_type_argument_count(parameters)?;
        let maximum = parameters.len();
        let expected = if minimum < maximum {
            format!("{minimum}-{maximum}")
        } else {
            minimum.to_string()
        };
        let view = self.ast(node)?;
        let source = ts_ast::utilities::get_source_file_of_node(view, Some(node))?
            .ok_or(Error::MissingLink("type argument arity source"))?;
        let list = view
            .node(node)?
            .type_argument_list()
            .ok_or(Error::MissingLink("type argument arity list"))?;
        let loc = view.list(list)?.loc();
        let start = ts_scanner::skip_trivia(view.source_file(source)?.text().as_bytes(), loc.pos());
        let diagnostic = Diagnostic::new(
            Some(source),
            ts_core::TextRange::new(start, loc.end()),
            messages::Expected_0_type_arguments_but_got_1,
            vec![
                JsString::from_bytes(expected.as_bytes()),
                JsString::from_bytes(count.to_string().as_bytes()),
            ],
        );
        self.add_diagnostic(diagnostic)?;
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.hasCorrectArity
    pub(crate) fn call_has_correct_arity(
        &mut self,
        node: NodeId,
        args: &[NodeId],
        signature: SignatureId,
    ) -> Result<bool, Error> {
        let parameter_count = self.parameter_count(signature)?;
        let minimum = self.min_argument_count(signature)?;
        let view = self.ast(node)?;
        let read = view.node(node)?;
        let incomplete = if read.kind() == K::BinaryExpression {
            false
        } else if read.kind() == K::TaggedTemplateExpression {
            self.tagged_template_is_incomplete(node)?
        } else {
            match read.argument_list() {
                Some(list) => view.list(list)?.loc().end() == i64::from(read.end()),
                None if read.kind() == K::NewExpression => return Ok(minimum == 0),
                None => return Err(Error::MissingLink("call argument list")),
            }
        };
        if read.kind() != K::BinaryExpression {
            if let Some(index) = self.spread_argument_index(args)? {
                return Ok(index >= minimum
                    && (self.effective_rest_parameter(signature)? || index < parameter_count));
            }
        }
        if !self.effective_rest_parameter(signature)? && args.len() > parameter_count {
            return Ok(false);
        }
        if incomplete || args.len() >= minimum {
            return Ok(true);
        }
        for index in args.len()..minimum {
            let ty = self
                .parameter_type_at(signature, index)?
                .unwrap_or(self.builtins.any_type);
            let accepted = self.filter_type(ty, &mut |state, part| {
                Ok(state.types.flags(part)? & tf::VOID != 0)
            })?;
            if self.types.flags(accepted)? & tf::NEVER != 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTypeArguments
    pub(crate) fn call_type_arguments(
        &mut self,
        signature: SignatureId,
        nodes: &[NodeId],
        report: bool,
    ) -> Result<Option<Vec<TypeId>>, Error> {
        let parameters = self
            .signatures
            .get(signature)?
            .type_parameters
            .clone()
            .unwrap_or_else(|| [].into());
        let mut types = Vec::with_capacity(nodes.len());
        for &node in nodes {
            types.push(self.get_type_from_type_node(node)?);
        }
        let javascript = self
            .signatures
            .get(signature)?
            .declaration
            .map(|node| {
                self.ast(node)?
                    .node(node)
                    .map(|read| read.flags() & ts_ast::node_flags::JAVA_SCRIPT_FILE != 0)
                    .map_err(Error::from)
            })
            .transpose()?
            .unwrap_or(false);
        let types = self.fill_missing_type_arguments(&types, &parameters, javascript)?;
        let mut mapper = None;
        for (index, &node) in nodes.iter().enumerate() {
            if let Some(constraint) = self.constraint_of_type_parameter(parameters[index])? {
                let mapper = if let Some(mapper) = mapper {
                    mapper
                } else {
                    let value = self.new_type_mapper(&parameters, &types)?;
                    mapper = Some(value);
                    value
                };
                let constraint = self.instantiate_type(constraint, Some(mapper))?;
                let constraint =
                    self.get_type_with_this_argument(constraint, types[index], false)?;
                let (related, diagnostic) = self.check_type_related_ex(
                    types[index],
                    constraint,
                    RelationKind::Assignable,
                    report.then_some(node),
                    Some(messages::Type_0_does_not_satisfy_the_constraint_1),
                )?;
                if let Some(diagnostic) = diagnostic {
                    self.add_diagnostic(diagnostic)?;
                }
                if !related {
                    return Ok(None);
                }
            }
        }
        Ok(Some(types.to_vec()))
    }

    // port: tsc/internal/checker/checker.go:Checker.checkExpressionWithContextualType
    pub(crate) fn check_call_argument_ex(
        &mut self,
        node: NodeId,
        contextual: TypeId,
        inference: Option<InferenceId>,
        mode: u32,
    ) -> Result<TypeId, Error> {
        self.calls.contexts.push(crate::calls::ArgumentContext {
            node,
            ty: contextual,
            inference,
        });
        self.calls.inference_contexts.push((node, inference));
        let result = (|| {
            let mode = mode | 1 | if inference.is_some() { 2 } else { 0 };
            let mut ty = self.check_expression_ex(node, mode)?;
            if let Some(inference) = inference {
                self.inference_context_mut(inference)?
                    .intra_expression_sites
                    .clear();
            }
            let contextual = self.instantiate_call_contextual_type(contextual, inference, false)?;
            if self.maybe_type_of_kind(ty, tf::LITERAL)?
                && self.literal_of_context(ty, Some(contextual))?
            {
                ty = self.get_regular_type_of_literal_type(ty)?;
            }
            Ok(ty)
        })();
        self.calls.inference_contexts.pop();
        self.calls.contexts.pop();
        result
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateContextualType
    pub(crate) fn instantiate_call_contextual_type(
        &mut self,
        ty: TypeId,
        inference: Option<InferenceId>,
        signature: bool,
    ) -> Result<TypeId, Error> {
        if self.maybe_type_of_kind(ty, tf::INSTANTIABLE)? {
            if let Some(inference) = inference {
                if signature {
                    let infos = self.inference_context(inference)?.inferences.clone();
                    let mut candidates = false;
                    for info in infos {
                        let default = self.resolved_type_parameter_default(info.parameter)?;
                        if !info.candidates.is_empty()
                            || !info.contra_candidates.is_empty()
                            || default != self.builtins.no_constraint_type
                                && default != self.builtins.circular_constraint_type
                        {
                            candidates = true;
                            break;
                        }
                    }
                    if candidates {
                        let mapper = self.inference_context(inference)?.non_fixing_mapper;
                        let result = self.instantiate_call_instantiable_types(ty, mapper)?;
                        if self.types.flags(result)? & tf::ANY_OR_UNKNOWN == 0 {
                            return Ok(result);
                        }
                    }
                }
                if let Some(mapper) = self.inference_context(inference)?.return_mapper {
                    let result = self.instantiate_call_instantiable_types(ty, mapper)?;
                    if self.types.flags(result)? & tf::ANY_OR_UNKNOWN == 0 {
                        if self.types.flags(result)? & tf::UNION != 0 {
                            let parts = self.types.compound_types(result)?;
                            if parts.contains(&self.builtins.regular_false_type)
                                && parts.contains(&self.builtins.regular_true_type)
                            {
                                return self.filter_type(result, &mut |checker, part| {
                                    Ok(part != checker.builtins.regular_false_type
                                        && part != checker.builtins.regular_true_type)
                                });
                            }
                        }
                        return Ok(result);
                    }
                }
            }
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateInstantiableTypes
    fn instantiate_call_instantiable_types(
        &mut self,
        ty: TypeId,
        mapper: crate::MapperId,
    ) -> Result<TypeId, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::INSTANTIABLE != 0 {
            return self.instantiate_type(ty, Some(mapper));
        }
        if flags & (tf::UNION | tf::INTERSECTION) != 0 {
            let mut types = Vec::new();
            for part in self.types.compound_types(ty)?.to_vec() {
                types.push(self.instantiate_call_instantiable_types(part, mapper)?);
            }
            return if flags & tf::UNION != 0 {
                self.get_union_type_ex(&types, crate::UnionReduction::None, None, None)
            } else {
                self.get_intersection_type(&types)
            };
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.inferTypeArguments
    pub(crate) fn infer_call_type_arguments(
        &mut self,
        node: NodeId,
        signature: SignatureId,
        args: &[NodeId],
    ) -> Result<Vec<TypeId>, Error> {
        let parameters = self
            .signatures
            .get(signature)?
            .type_parameters
            .clone()
            .unwrap_or_else(|| [].into());
        let flags = if ts_ast::utilities::is_in_js_file(Some(&self.ast(node)?.node(node)?)) {
            crate::inference::ANY_DEFAULT
        } else {
            0
        };
        let context = self.new_inference_context(&parameters, Some(signature), flags)?;
        self.infer_call_type_arguments_ex(node, signature, args, 4 | 8, context)
    }

    pub(crate) fn infer_call_type_arguments_ex(
        &mut self,
        node: NodeId,
        signature: SignatureId,
        args: &[NodeId],
        mode: u32,
        context: InferenceId,
    ) -> Result<Vec<TypeId>, Error> {
        let parameters = self
            .signatures
            .get(signature)?
            .type_parameters
            .clone()
            .unwrap_or_else(|| [].into());
        if !matches!(
            self.ast(node)?.node(node)?.kind().known(),
            Some(K::Decorator | K::BinaryExpression)
        ) {
            let mut skip_patterns = true;
            for &parameter in parameters.iter() {
                let default = self.resolved_type_parameter_default(parameter)?;
                if default == self.builtins.no_constraint_type
                    || default == self.builtins.circular_constraint_type
                {
                    skip_patterns = false;
                    break;
                }
            }
            if let Some(contextual) =
                self.contextual_expression_type_ex(node, if skip_patterns { 8 } else { 0 })?
            {
                let target = self.return_type_of_signature(signature)?;
                if self.could_contain_type_variables(target)? {
                    let outer = self.call_inference_at_node(node)?;
                    let from_pattern = !skip_patterns
                        && self.contextual_expression_type_ex(node, 8)? != Some(contextual);
                    if !from_pattern {
                        let outer_clone = outer
                            .map(|outer| {
                                self.clone_call_inference_context(
                                    outer,
                                    crate::inference::NO_DEFAULT,
                                    false,
                                )
                            })
                            .transpose()?
                            .flatten();
                        let outer_mapper = outer_clone
                            .map(|context| {
                                self.inference_context(context)
                                    .map(|context| context.mapper)
                            })
                            .transpose()?;
                        let instantiated = self.instantiate_type(contextual, outer_mapper)?;
                        let source = if let Some(signature) =
                            self.call_single_signature(instantiated)?
                        {
                            let parameters = self
                                .signatures
                                .get(signature)?
                                .type_parameters
                                .clone()
                                .unwrap_or_else(|| [].into());
                            if parameters.is_empty() {
                                instantiated
                            } else {
                                let instantiated_signature =
                                    self.signature_instantiation(signature, &parameters, false)?;
                                self.isolated_signature_type(instantiated_signature)?
                            }
                        } else {
                            instantiated
                        };
                        self.infer_types(context, source, target, priority::RETURN_TYPE, false)?;
                    }
                    let flags = self.inference_context(context)?.flags;
                    let returned =
                        self.new_inference_context(&parameters, Some(signature), flags)?;
                    let outer_mapper = outer
                        .map(|outer| self.call_outer_return_mapper(outer))
                        .transpose()?;
                    let source = self.instantiate_type(contextual, outer_mapper)?;
                    self.infer_types(returned, source, target, priority::NONE, false)?;
                    let returned = self.clone_call_inference_context(returned, 0, true)?;
                    let mapper = returned
                        .map(|context| {
                            self.inference_context(context)
                                .map(|context| context.mapper)
                        })
                        .transpose()?;
                    self.inference_context_mut(context)?.return_mapper = mapper;
                }
            }
        }
        let rest = self.non_array_rest_type(signature)?;
        let arg_count = match rest {
            Some(_) => (self.parameter_count(signature)? - 1).min(args.len()),
            None => args.len(),
        };
        if let Some(rest) = rest {
            if self.types.flags(rest)? & tf::TYPE_PARAMETER != 0
                && self.spread_argument_index(&args[arg_count..])?.is_none()
            {
                if let Some(info) = self
                    .inference_context_mut(context)?
                    .inferences
                    .iter_mut()
                    .find(|info| info.parameter == rest)
                {
                    info.implied_arity = Some(args.len() - arg_count);
                }
            }
        }
        if let Some(this) = self.signatures.get(signature)?.this_parameter {
            let target = self.get_type_of_symbol(this)?;
            if self.could_contain_type_variables(target)? {
                let source = self.call_this_argument_type(node)?;
                self.infer_types(context, source, target, priority::NONE, false)?;
            }
        }
        for (index, &argument) in args[..arg_count].iter().enumerate() {
            let read = self.ast(argument)?.node(argument)?;
            if read.kind() == K::OmittedExpression {
                continue;
            }
            let target = self
                .parameter_type_at(signature, index)?
                .unwrap_or(self.builtins.any_type);
            if self.could_contain_type_variables(target)? {
                let source = self.check_call_argument_ex(argument, target, Some(context), mode)?;
                self.infer_types(context, source, target, priority::NONE, false)?;
            }
        }
        if let Some(rest) = rest {
            if self.could_contain_type_variables(rest)? {
                let spread = self.spread_argument_type(
                    args,
                    arg_count,
                    args.len(),
                    rest,
                    Some(context),
                    mode,
                )?;
                self.infer_types(context, spread, rest, priority::NONE, false)?;
            }
        }
        let mut inferred = Vec::with_capacity(parameters.len());
        for index in 0..parameters.len() {
            inferred.push(self.inferred_type(context, index)?);
        }
        Ok(inferred)
    }

    // port: tsc/internal/checker/checker.go:Checker.isSignatureApplicable
    pub(crate) fn call_signature_applicable_ex(
        &mut self,
        node: NodeId,
        args: &[NodeId],
        signature: SignatureId,
        relation: RelationKind,
        report: bool,
        mode: u32,
    ) -> Result<bool, Error> {
        let mut diagnostics = Vec::new();
        let result = self.collect_signature_applicability_errors(
            node,
            args,
            signature,
            relation,
            report,
            mode,
            &mut diagnostics,
        )?;
        for diagnostic in diagnostics {
            self.add_diagnostic(diagnostic)?;
        }
        Ok(result)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "Parameters preserve the upstream operation and its independently selected checking modes"
    )]
    pub(crate) fn collect_signature_applicability_errors(
        &mut self,
        node: NodeId,
        args: &[NodeId],
        signature: SignatureId,
        relation: RelationKind,
        report: bool,
        mode: u32,
        output: &mut Vec<Diagnostic>,
    ) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        let skip_this = read.kind() == K::NewExpression
            || read.kind() == K::CallExpression
                && match read.expression() {
                    Some(expression) => ts_ast::utilities_tail::is_super_property(
                        self.ast(expression)?,
                        &self.ast(expression)?.node(expression)?,
                    )?,
                    None => false,
                };
        if let Some(this) = self.signatures.get(signature)?.this_parameter {
            let target = self.get_type_of_symbol(this)?;
            if target != self.builtins.void_type && !skip_this {
                let source = self.call_this_argument_type(node)?;
                let error_node = if report {
                    self.call_this_argument_node(node)?.or(Some(node))
                } else {
                    None
                };
                let (related, diagnostic) = self.check_type_related_ex(source, target, relation, error_node, Some(messages::The_this_context_of_type_0_is_not_assignable_to_method_s_this_of_type_1))?;
                if let Some(diagnostic) = diagnostic {
                    output.push(diagnostic);
                }
                if !related {
                    return Ok(false);
                }
            }
        }
        let rest = self.non_array_rest_type(signature)?;
        let arg_count = match rest {
            Some(_) => (self.parameter_count(signature)? - 1).min(args.len()),
            None => args.len(),
        };
        for (index, &argument) in args[..arg_count].iter().enumerate() {
            if self.ast(argument)?.node(argument)?.kind() == K::OmittedExpression {
                continue;
            }
            let target = self
                .parameter_type_at(signature, index)?
                .unwrap_or(self.builtins.any_type);
            let source = self.check_call_argument_ex(argument, target, None, mode)?;
            let source = if mode & 4 != 0 {
                self.regular_object_literal_type(source)?
            } else {
                source
            };
            let checked = self.effective_expression_check_node(argument)?;
            if !self.collect_expression_relation_errors(
                source,
                target,
                relation,
                report.then_some(checked),
                Some(checked),
                Some(messages::Argument_of_type_0_is_not_assignable_to_parameter_of_type_1),
                output,
            )? {
                self.maybe_add_missing_await_info(
                    Some(argument),
                    source,
                    target,
                    relation,
                    report,
                    output,
                )?;
                return Ok(false);
            }
        }
        if let Some(rest) = rest {
            let spread =
                self.spread_argument_type(args, arg_count, args.len(), rest, None, mode)?;
            let error_node = if report {
                Some(match args.len() - arg_count {
                    0 => node,
                    1 => self.effective_expression_check_node(args[arg_count])?,
                    _ => {
                        let synthetic = self.synthetic_call_argument(node, spread, false, None)?;
                        let start = self.ast(args[arg_count])?.node(args[arg_count])?.pos();
                        let end = self
                            .ast(args[args.len() - 1])?
                            .node(args[args.len() - 1])?
                            .end();
                        self.factory.set_node_range(
                            synthetic,
                            ts_core::TextRange::new(i64::from(start), i64::from(end)),
                        );
                        synthetic
                    }
                })
            } else {
                None
            };
            let (related, diagnostic) = self.check_type_related_ex(
                spread,
                rest,
                relation,
                error_node,
                Some(messages::Argument_of_type_0_is_not_assignable_to_parameter_of_type_1),
            )?;
            if let Some(diagnostic) = diagnostic {
                output.push(diagnostic);
            }
            if !related {
                self.maybe_add_missing_await_info(
                    error_node, spread, rest, relation, report, output,
                )?;
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn call_this_argument_type(&mut self, node: NodeId) -> Result<TypeId, Error> {
        match self.call_this_argument_node(node)? {
            Some(receiver) => {
                let ty = self.check_expression(receiver)?;
                let Some(parent) = self.ast(receiver)?.node(receiver)?.parent() else {
                    return Ok(ty);
                };
                let read = self.ast(parent)?.node(parent)?;
                if ts_ast::utilities::is_optional_chain_root(&read) {
                    self.non_nullable_type(ty)
                } else if read.flags() & ts_ast::node_flags::OPTIONAL_CHAIN != 0 {
                    self.remove_optional_type_marker(ty)
                } else {
                    Ok(ty)
                }
            }
            None => Ok(self.builtins.void_type),
        }
    }

    fn call_this_argument_node(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        if self.ast(node)?.node(node)?.kind() == K::BinaryExpression {
            let right = self
                .ast(node)?
                .node(node)?
                .data_source()
                .as_binary_expression()
                .ok_or(Error::MissingLink("instanceof binary expression"))?
                .right()
                .ok_or(Error::MissingLink("instanceof right operand"))?;
            return Ok(Some(right));
        }
        if !matches!(
            self.ast(node)?.node(node)?.kind().known(),
            Some(K::CallExpression | K::TaggedTemplateExpression)
        ) {
            return Ok(None);
        }
        let mut expression =
            ts_ast::utilities_middle::get_invoked_expression(self.ast(node)?, node)?
                .ok_or(Error::MissingLink("this invoked expression"))?;
        loop {
            let read = self.ast(expression)?.node(expression)?;
            if matches!(
                read.kind().known(),
                Some(
                    K::ParenthesizedExpression
                        | K::AsExpression
                        | K::TypeAssertionExpression
                        | K::NonNullExpression
                        | K::SatisfiesExpression
                        | K::ExpressionWithTypeArguments
                        | K::PartiallyEmittedExpression
                )
            ) {
                expression = read
                    .expression()
                    .ok_or(Error::MissingLink("this call outer expression"))?;
            } else {
                break;
            }
        }
        let read = self.ast(expression)?.node(expression)?;
        if matches!(
            read.kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) {
            let target = read
                .expression()
                .ok_or(Error::MissingLink("this call receiver"))?;
            Ok(Some(target))
        } else {
            Ok(None)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getArgumentArityError
    pub(crate) fn report_call_arity(
        &mut self,
        node: NodeId,
        args: &[NodeId],
        signature: SignatureId,
    ) -> Result<(), Error> {
        self.report_call_arity_for_signatures(node, args, &[signature])
    }

    pub(crate) fn report_call_arity_for_signatures(
        &mut self,
        node: NodeId,
        args: &[NodeId],
        signatures: &[SignatureId],
    ) -> Result<(), Error> {
        if let Some(index) = self.spread_argument_index(args)? {
            self.error_at(Some(args[index]), messages::A_spread_argument_must_either_have_a_tuple_type_or_be_passed_to_a_rest_parameter, vec![])?;
            return Ok(());
        }
        let mut minimum = usize::MAX;
        let mut maximum = 0;
        let mut below = None;
        let mut above = None;
        let mut closest = None;
        let mut rest = false;
        for &signature in signatures {
            let min = self.min_argument_count(signature)?;
            let max = self.parameter_count(signature)?;
            if min < minimum {
                minimum = min;
                closest = Some(signature);
            }
            maximum = maximum.max(max);
            if min < args.len() {
                below = Some(below.map_or(min, |old: usize| old.max(min)));
            }
            if args.len() < max {
                above = Some(above.map_or(max, |old: usize| old.min(max)));
            }
            rest |= self.effective_rest_parameter(signature)?;
        }
        let signature = closest.ok_or(Error::MissingLink("arity candidate"))?;
        let count = if !rest && minimum < maximum {
            format!("{minimum}-{maximum}")
        } else {
            minimum.to_string()
        };
        let between = minimum < args.len() && args.len() < maximum;
        let message = if between {
            messages::No_overload_expects_0_arguments_but_overloads_do_exist_that_expect_either_1_or_2_arguments
        } else if rest {
            messages::Expected_at_least_0_arguments_but_got_1
        } else {
            messages::Expected_0_arguments_but_got_1
        };
        let diagnostic_args = if between {
            vec![
                JsString::from_bytes(args.len().to_string().as_bytes()),
                JsString::from_bytes(
                    below
                        .ok_or(Error::MissingLink("arity below"))?
                        .to_string()
                        .as_bytes(),
                ),
                JsString::from_bytes(
                    above
                        .ok_or(Error::MissingLink("arity above"))?
                        .to_string()
                        .as_bytes(),
                ),
            ]
        } else {
            vec![
                JsString::from_bytes(count.as_bytes()),
                JsString::from_bytes(args.len().to_string().as_bytes()),
            ]
        };
        let error_node = if self.ast(node)?.node(node)?.kind() == K::CallExpression {
            let target = self
                .ast(node)?
                .node(node)?
                .expression()
                .ok_or(Error::MissingLink("arity call target"))?;
            let read = self.ast(target)?.node(target)?;
            if read.kind() == K::PropertyAccessExpression {
                read.name()
                    .ok_or(Error::MissingLink("arity property name"))?
            } else {
                target
            }
        } else {
            node
        };
        let void_promise_error = !rest
            && count == "1"
            && args.is_empty()
            && self.is_promise_resolve_arity_error(node)?;
        if void_promise_error
            && self.ast(node)?.node(node)?.flags() & ts_ast::node_flags::JAVA_SCRIPT_FILE != 0
        {
            self.error_at(Some(error_node), messages::Expected_1_argument_but_got_0_new_Promise_needs_a_JSDoc_hint_to_produce_a_resolve_that_can_be_called_without_arguments, vec![])?;
            return Ok(());
        }
        let mut diagnostic = if args.len() <= maximum {
            self.diagnostic_for_node(Some(error_node), message, diagnostic_args)?
        } else {
            let view = self.ast(node)?;
            let source = ts_ast::utilities::get_source_file_of_node(view, Some(node))?
                .ok_or(Error::MissingLink("arity source"))?;
            // Spread arguments expand to synthetic expressions owned by the
            // checker factory; they carry the spread element's range.
            let first_extra = args[maximum];
            let last = *args
                .last()
                .ok_or(Error::MissingLink("arity last argument"))?;
            let mut start = i64::from(self.ast(first_extra)?.node(first_extra)?.pos());
            let mut end = i64::from(self.ast(last)?.node(last)?.end());
            if end == start {
                end += 1;
            }
            start = ts_scanner::skip_trivia(view.source_file(source)?.text().as_bytes(), start);
            end = end.max(start);
            Diagnostic::new(
                Some(source),
                ts_core::TextRange::new(start, end),
                message,
                diagnostic_args,
            )
        };
        if self.ast(node)?.node(node)?.kind() == K::BinaryExpression {
            diagnostic = Diagnostic::chain(Some(std::sync::Arc::new(diagnostic)), messages::The_left_hand_side_of_an_instanceof_expression_must_be_assignable_to_the_first_argument_of_the_right_hand_side_s_Symbol_hasInstance_method, vec![]);
        }
        if args.len() < minimum {
            if let Some(declaration) = self.signatures.get(signature)?.declaration {
                let parameters = self.source_list(
                    declaration,
                    self.ast(declaration)?.node(declaration)?.parameter_list(),
                )?;
                let index = args.len()
                    + usize::from(self.signatures.get(signature)?.this_parameter.is_some());
                if let Some(&parameter) = parameters.get(index) {
                    let read = self.ast(parameter)?.node(parameter)?;
                    let name = read
                        .name()
                        .ok_or(Error::MissingLink("missing argument parameter name"))?;
                    let pattern = matches!(
                        self.ast(name)?.node(name)?.kind().known(),
                        Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
                    );
                    let rest = read
                        .data_source()
                        .as_parameter_declaration()
                        .ok_or(Error::MissingLink("missing argument parameter"))?
                        .dot_dot_dot_token()
                        .is_some();
                    let arguments = if pattern {
                        vec![]
                    } else {
                        vec![self.ast(name)?.node_text(name)?.into_js_string()]
                    };
                    let related = self.diagnostic_for_node(
                        Some(parameter),
                        if pattern {
                            messages::An_argument_matching_this_binding_pattern_was_not_provided
                        } else if rest {
                            messages::Arguments_for_the_rest_parameter_0_were_not_provided
                        } else {
                            messages::An_argument_for_0_was_not_provided
                        },
                        arguments,
                    )?;
                    diagnostic
                        .related_information
                        .push(std::sync::Arc::new(related));
                }
            }
        }
        self.add_diagnostic(diagnostic)?;
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.isPromiseResolveArityError
    fn is_promise_resolve_arity_error(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.kind() != K::CallExpression {
            return Ok(false);
        }
        let callee = read
            .expression()
            .ok_or(Error::MissingLink("call expression callee"))?;
        if self.ast(callee)?.node(callee)?.kind() != K::Identifier {
            return Ok(false);
        }
        let text = self.ast(callee)?.node_text(callee)?.into_js_string();
        let Some(symbol) = self.resolve_name(
            Some(callee),
            text.as_bytes(),
            ts_ast::symbol_flags::VALUE,
            None,
            false,
        )?
        else {
            return Ok(false);
        };
        let Some(declaration) = self.symbol(symbol)?.value_declaration() else {
            return Ok(false);
        };
        let view = self.ast(declaration)?;
        if view.node(declaration)?.kind() != K::Parameter {
            return Ok(false);
        }
        let Some(function) = view.node(declaration)?.parent() else {
            return Ok(false);
        };
        if !matches!(
            view.node(function)?.kind().known(),
            Some(K::FunctionExpression | K::ArrowFunction)
        ) {
            return Ok(false);
        }
        let Some(construct) = view.node(function)?.parent() else {
            return Ok(false);
        };
        if view.node(construct)?.kind() != K::NewExpression {
            return Ok(false);
        }
        let Some(constructor) = view.node(construct)?.expression() else {
            return Ok(false);
        };
        if view.node(constructor)?.kind() != K::Identifier {
            return Ok(false);
        }
        let Some(promise) = self.global_promise_constructor_symbol(false)? else {
            return Ok(false);
        };
        Ok(self.resolved_value_symbol(constructor)? == promise)
    }
}
