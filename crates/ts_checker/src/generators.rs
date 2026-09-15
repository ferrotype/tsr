//! Generator bodies infer yield, return and next independently, then construct
//! the pinned global Generator (or IterableIterator fallback) instantiation.
use crate::{
    iteration::{IterationKind, ALLOW_ASYNC, ALLOW_SYNC, YIELD_STAR_FLAG},
    object_flags as of, type_flags as tf, CheckerState, Error, RelationKind, TypeId,
    UnionReduction,
};
use ts_arena::NodeId;
use ts_ast::{node_flags as nf, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getReturnTypeFromBody
    pub(crate) fn generator_return_type_from_body(
        &mut self,
        function: NodeId,
        body: NodeId,
        mode: u32,
        asynchronous: bool,
    ) -> Result<TypeId, Error> {
        let (returns, never) = self.aggregate_return_expression_types(function, body, mode)?;
        let mut returned = if returns.is_empty() {
            None
        } else {
            Some(self.get_union_type_ex(&returns, UnionReduction::Subtype, None, None)?)
        };
        let (yields, nexts) = self.aggregate_yield_expression_types(function, body, mode)?;
        let mut yielded = if yields.is_empty() {
            None
        } else {
            Some(self.get_union_type_ex(&yields, UnionReduction::Subtype, None, None)?)
        };
        let mut next = if nexts.is_empty() {
            None
        } else {
            Some(self.get_intersection_type(&nexts)?)
        };
        if yielded.is_some() || returned.is_some() || next.is_some() {
            for (kind, ty) in [
                (IterationKind::Yield, yielded),
                (IterationKind::Return, returned),
                (IterationKind::Next, next),
            ] {
                if let Some(ty) = ty {
                    self.report_function_widening(function, ty, kind)?;
                }
            }
            let mut unit = false;
            for ty in [returned, yielded, next].into_iter().flatten() {
                unit |= self.types.flags(ty)? & tf::UNIT != 0;
            }
            if unit {
                let contextual =
                    if let Some(signature) = self.contextual_body_signature(function)? {
                        if signature == self.signature_from_declaration(function)? {
                            None
                        } else {
                            let ty = self.return_type_of_signature(signature)?;
                            let inference = self.call_inference_at_node(function)?;
                            Some(self.instantiate_call_contextual_type(ty, inference, false)?)
                        }
                    } else {
                        None
                    };
                yielded = self.widen_contextual_iteration_type(
                    yielded,
                    contextual,
                    IterationKind::Yield,
                    asynchronous,
                )?;
                returned = self.widen_contextual_iteration_type(
                    returned,
                    contextual,
                    IterationKind::Return,
                    asynchronous,
                )?;
                next = self.widen_contextual_iteration_type(
                    next,
                    contextual,
                    IterationKind::Next,
                    asynchronous,
                )?;
            }
            yielded = yielded.map(|ty| self.widened_type(ty)).transpose()?;
            returned = returned.map(|ty| self.widened_type(ty)).transpose()?;
            next = next.map(|ty| self.widened_type(ty)).transpose()?;
        }
        let returned = returned.unwrap_or(if never {
            self.builtins.never_type
        } else {
            self.builtins.void_type
        });
        let yielded = yielded.unwrap_or(self.builtins.never_type);
        let next = match next {
            Some(next) => next,
            None => self
                .contextual_iteration_type(IterationKind::Next, function)?
                .unwrap_or(self.builtins.unknown_type),
        };
        self.create_generator_type(yielded, returned, next, asynchronous)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkAndAggregateYieldOperandTypes
    fn aggregate_yield_expression_types(
        &mut self,
        function: NodeId,
        body: NodeId,
        mode: u32,
    ) -> Result<(Vec<TypeId>, Vec<TypeId>), Error> {
        let asynchronous = self.body_function_flags(function)?.0;
        let mut yields = Vec::new();
        let mut nexts = Vec::new();
        for node in self.yield_expressions(body)? {
            let operand = self.ast(node)?.node(node)?.expression();
            let mut operand_type = match operand {
                Some(operand) => self.check_expression_ex(operand, mode & !8)?,
                None => self.builtins.undefined_widening_type,
            };
            if let Some(operand) = operand {
                if self.is_const_context(operand)? {
                    operand_type = self.get_regular_type_of_literal_type(operand_type)?;
                }
            }
            if let Some(ty) = self.yielded_type_of_yield_expression(
                node,
                operand_type,
                self.builtins.any_type,
                asynchronous,
            )? {
                if !yields.contains(&ty) {
                    yields.push(ty);
                }
            }
            let next = if self.yield_is_star(node)? {
                self.iteration_types_of_iterable(
                    operand_type,
                    ALLOW_SYNC | YIELD_STAR_FLAG | if asynchronous { ALLOW_ASYNC } else { 0 },
                    operand,
                )?
                .next_type
            } else {
                self.contextual_expression_type(node)?
            };
            if let Some(next) = next {
                if !nexts.contains(&next) {
                    nexts.push(next);
                }
            }
        }
        Ok((yields, nexts))
    }

    // port: tsc/internal/checker/utilities.go:forEachYieldExpression
    pub(crate) fn yield_expressions(&self, body: NodeId) -> Result<Vec<NodeId>, Error> {
        let mut result = Vec::new();
        let mut stack = vec![body];
        while let Some(node) = stack.pop() {
            let read = self.ast(node)?.node(node)?;
            match read.kind().known() {
                Some(K::YieldExpression) => {
                    result.push(node);
                    if let Some(operand) = read.expression() {
                        stack.push(operand);
                    }
                }
                Some(
                    K::EnumDeclaration
                    | K::InterfaceDeclaration
                    | K::ModuleDeclaration
                    | K::TypeAliasDeclaration,
                ) => {}
                _ => {
                    if ts_ast::utilities::is_function_like(Some(&read)) {
                        if let Some(name) = read.name() {
                            if self.ast(name)?.node(name)?.kind() == K::ComputedPropertyName {
                                stack.push(
                                    self.ast(name)?
                                        .node(name)?
                                        .expression()
                                        .ok_or(Error::MissingLink("computed yield name"))?,
                                );
                            }
                        }
                    } else if !self.is_part_of_type_node(node)? {
                        stack.extend(self.source_children(node)?.into_iter().rev());
                    }
                }
            }
        }
        Ok(result)
    }

    fn yield_is_star(&self, node: NodeId) -> Result<bool, Error> {
        Ok(self
            .ast(node)?
            .node(node)?
            .data_source()
            .as_yield_expression()
            .ok_or(Error::MissingLink("yield expression"))?
            .asterisk_token()
            .is_some())
    }

    // port: tsc/internal/checker/checker.go:Checker.getYieldedTypeOfYieldExpression
    fn yielded_type_of_yield_expression(
        &mut self,
        node: NodeId,
        operand_type: TypeId,
        sent: TypeId,
        asynchronous: bool,
    ) -> Result<Option<TypeId>, Error> {
        let error = self.ast(node)?.node(node)?.expression().or(Some(node));
        let star = self.yield_is_star(node)?;
        let yielded = if star {
            self.check_iterated_type_or_element_type(
                ALLOW_SYNC | YIELD_STAR_FLAG | if asynchronous { ALLOW_ASYNC } else { 0 },
                operand_type,
                sent,
                error,
            )?
        } else {
            operand_type
        };
        if !asynchronous {
            return Ok(Some(yielded));
        }
        self.awaited_type_ex(yielded,error,Some(if star {d::Type_of_iterated_elements_of_a_yield_Asterisk_operand_must_either_be_a_valid_promise_or_must_not_contain_a_callable_then_member} else {d::Type_of_yield_operand_in_an_async_generator_must_either_be_a_valid_promise_or_must_not_contain_a_callable_then_member}),&[])
    }

    // port: tsc/internal/checker/checker.go:Checker.createGeneratorType
    pub(crate) fn create_generator_type(
        &mut self,
        yielded: TypeId,
        returned: TypeId,
        next: TypeId,
        asynchronous: bool,
    ) -> Result<TypeId, Error> {
        let generator = self.iteration_global(
            if asynchronous {
                "AsyncGenerator"
            } else {
                "Generator"
            },
            3,
        )?;
        let yielded = self
            .resolve_iteration_type(yielded, asynchronous, None)?
            .unwrap_or(self.builtins.unknown_type);
        let returned = self
            .resolve_iteration_type(returned, asynchronous, None)?
            .unwrap_or(self.builtins.unknown_type);
        let target = if generator == self.builtins.empty_generic_type {
            let name = if asynchronous {
                "AsyncIterableIterator"
            } else {
                "IterableIterator"
            };
            let fallback = self.iteration_global(name, 3)?;
            if fallback == self.builtins.empty_generic_type {
                self.iteration_global_checked(name, 3)?;
                return Ok(self.builtins.empty_object_type);
            }
            fallback
        } else {
            generator
        };
        self.create_type_reference(target, &[yielded, returned, next])
    }

    // port: tsc/internal/checker/checker.go:Checker.getWidenedLiteralLikeTypeForContextualIterationTypeIfNeeded
    fn widen_contextual_iteration_type(
        &mut self,
        ty: Option<TypeId>,
        context: Option<TypeId>,
        kind: IterationKind,
        asynchronous: bool,
    ) -> Result<Option<TypeId>, Error> {
        let Some(mut ty) = ty else {
            return Ok(None);
        };
        if self.types.flags(ty)? & tf::UNIT != 0 {
            let context = context
                .map(|ty| self.generator_return_iteration_type(kind, ty, asynchronous))
                .transpose()?
                .flatten();
            if !self.literal_of_context(ty, context)? {
                ty = self.widen_literal_type(ty)?;
                if self.types.flags(ty)? & tf::UNIQUE_ES_SYMBOL != 0 {
                    ty = self.builtins.es_symbol_type;
                }
            }
            ty = self.get_regular_type_of_literal_type(ty)?;
        }
        Ok(Some(ty))
    }

    // port: tsc/internal/checker/checker.go:Checker.reportErrorsFromWidening
    pub(crate) fn report_function_widening(
        &mut self,
        function: NodeId,
        ty: TypeId,
        kind: IterationKind,
    ) -> Result<(), Error> {
        let options = self.program()?.host.options();
        if !options.strict_option_value(options.no_implicit_any)
            || self.types.object_flags(ty)? & of::CONTAINS_WIDENING_TYPE == 0
        {
            return Ok(());
        }
        if let Some(signature) = self.contextual_body_signature(function)? {
            let context = self.return_type_of_signature(signature)?;
            let (asynchronous, generator) = self.body_function_flags(function)?;
            let part = if generator {
                self.generator_return_iteration_type(kind, context, asynchronous)?
            } else if asynchronous {
                self.awaited_type_no_alias(context)?
            } else {
                Some(context)
            };
            let part = match kind {
                IterationKind::Return => Some(part.unwrap_or(context)),
                _ => part,
            };
            if !part
                .map(|part| self.is_generic_type(part))
                .transpose()?
                .unwrap_or(false)
            {
                return Ok(());
            }
        }
        if self.report_widening_errors_in_type(ty)? {
            return Ok(());
        }
        let widened = self.widened_type(ty)?;
        let text = self.type_to_string(widened, crate::type_display::DEFAULT_FLAGS)?;
        let read = self.ast(function)?.node(function)?;
        let name = read.name();
        let yield_kind = matches!(kind, IterationKind::Yield);
        if name.is_none() {
            self.error_at(Some(function),if yield_kind {d::Generator_implicitly_has_yield_type_0_Consider_supplying_a_return_type_annotation} else {d::Function_expression_which_lacks_return_type_annotation_implicitly_has_an_0_return_type},vec![text])?;
            return Ok(());
        }
        let name = ts_scanner::declaration_name_to_string(self.ast(function)?, name)?;
        let reparsed = read.flags() & nf::REPARSED != 0;
        if reparsed && name.as_bytes().is_empty() {
            self.error_at(Some(function),d::This_overload_implicitly_returns_the_type_0_because_it_lacks_a_return_type_annotation,vec![text])?;
        } else {
            self.error_at(
                Some(function),
                if yield_kind && !reparsed {
                    d::X_0_which_lacks_return_type_annotation_implicitly_has_an_1_yield_type
                } else {
                    d::X_0_which_lacks_return_type_annotation_implicitly_has_an_1_return_type
                },
                vec![name, text],
            )?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getContextualIterationType
    pub(crate) fn contextual_iteration_type(
        &mut self,
        kind: IterationKind,
        function: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        let asynchronous = self.body_function_flags(function)?.0;
        let Some(context) = self.contextual_body_return_type(function)? else {
            return Ok(None);
        };
        self.generator_return_iteration_type(kind, context, asynchronous)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkGeneratorInstantiationAssignabilityToReturnType
    pub(crate) fn generator_instantiation_assignable(
        &mut self,
        returned: TypeId,
        asynchronous: bool,
        error: Option<NodeId>,
    ) -> Result<bool, Error> {
        let yielded = self
            .generator_return_iteration_type(IterationKind::Yield, returned, asynchronous)?
            .unwrap_or(self.builtins.any_type);
        let result = self
            .generator_return_iteration_type(IterationKind::Return, returned, asynchronous)?
            .unwrap_or(yielded);
        let next = self
            .generator_return_iteration_type(IterationKind::Next, returned, asynchronous)?
            .unwrap_or(self.builtins.unknown_type);
        let generated = self.create_generator_type(yielded, result, next, asynchronous)?;
        let (related, diagnostic) =
            self.check_type_related_ex(generated, returned, RelationKind::Assignable, error, None)?;
        if let Some(diagnostic) = diagnostic {
            self.add_diagnostic(diagnostic)?;
        }
        Ok(related)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkSignatureDeclaration
    pub(crate) fn check_generator_return_annotation(
        &mut self,
        function: NodeId,
        annotation: NodeId,
    ) -> Result<(), Error> {
        let returned = self.get_type_from_type_node(annotation)?;
        if returned == self.builtins.void_type {
            self.error_at(
                Some(annotation),
                d::A_generator_cannot_have_a_void_type_annotation,
                vec![],
            )?;
        } else {
            let asynchronous = self.body_function_flags(function)?.0;
            self.generator_instantiation_assignable(returned, asynchronous, Some(annotation))?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarForGenerator
    pub(crate) fn check_grammar_generator(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        let data = read.data_source();
        let token = match read.kind().known() {
            Some(K::FunctionDeclaration) => data
                .as_function_declaration()
                .and_then(|data| data.asterisk_token()),
            Some(K::FunctionExpression) => data
                .as_function_expression()
                .and_then(|data| data.asterisk_token()),
            Some(K::MethodDeclaration) => data
                .as_method_declaration()
                .and_then(|data| data.asterisk_token()),
            _ => None,
        };
        let Some(token) = token else {
            return Ok(false);
        };
        if read.flags() & nf::AMBIENT != 0 {
            self.grammar_error_node(
                token,
                d::Generators_are_not_allowed_in_an_ambient_context,
                vec![],
            )?;
            return Ok(true);
        }
        if read.body().is_none() {
            self.grammar_error_node(
                token,
                d::An_overload_signature_cannot_be_declared_as_a_generator,
                vec![],
            )?;
            return Ok(true);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkYieldExpression
    pub(crate) fn check_yield_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        if self.ast(node)?.node(node)?.flags() & nf::YIELD_CONTEXT == 0 {
            self.grammar_error_first_token(
                node,
                d::A_yield_expression_is_only_allowed_in_a_generator_body,
                vec![],
            )?;
        }
        if self.in_parameter_initializer_before_function(node)? {
            self.error_at(
                Some(node),
                d::X_yield_expressions_cannot_be_used_in_a_parameter_initializer,
                vec![],
            )?;
        }
        let operand = self.ast(node)?.node(node)?.expression();
        let operand_type = match operand {
            Some(operand) => self.check_expression(operand)?,
            None => self.builtins.undefined_widening_type,
        };
        let Some(function) = self.containing_body_function(node)? else {
            return Ok(self.builtins.any_type);
        };
        let (asynchronous, generator) = self.body_function_flags(function)?;
        if !generator {
            return Ok(self.builtins.any_type);
        }
        let star = self.yield_is_star(node)?;
        if star
            && asynchronous
            && self.program()?.host.options().emit_script_target() < ts_core::ScriptTarget::ES2018
        {
            // Async generator functions prior to ES2018 require the __await, __asyncDelegator,
            // and __asyncValues helpers
            self.check_external_emit_helpers(
                node,
                crate::external_emit_helpers::ASYNC_DELEGATOR_INCLUDES,
            )?;
        }
        let mut returned = self.return_type_from_annotation(function)?;
        if let Some(ty) = returned {
            if self.types.flags(ty)? & tf::UNION != 0 {
                returned = Some(self.filter_type(ty, &mut |checker, part| {
                    checker.generator_instantiation_assignable(part, asynchronous, None)
                })?);
            }
        }
        let types = returned
            .map(|ty| self.generator_return_iteration_types(ty, asynchronous))
            .transpose()?
            .unwrap_or_default();
        let yielded = self.yielded_type_of_yield_expression(
            node,
            operand_type,
            types.next_type.unwrap_or(self.builtins.any_type),
            asynchronous,
        )?;
        if returned.is_some() {
            if let Some(yielded) = yielded {
                self.check_expression_related_with_elaboration(
                    yielded,
                    types.yield_type.unwrap_or(self.builtins.any_type),
                    RelationKind::Assignable,
                    operand.or(Some(node)),
                    operand,
                    None,
                )?;
            }
        }
        if star {
            if self.types.flags(operand_type)? & tf::ANY != 0 {
                return Ok(self.builtins.any_type);
            }
            return Ok(self
                .iteration_types_of_iterable(
                    operand_type,
                    ALLOW_SYNC | YIELD_STAR_FLAG | if asynchronous { ALLOW_ASYNC } else { 0 },
                    operand,
                )?
                .return_type
                .unwrap_or(self.builtins.any_type));
        }
        if let Some(returned) = returned {
            return Ok(self
                .generator_return_iteration_type(IterationKind::Next, returned, asynchronous)?
                .unwrap_or(self.builtins.any_type));
        }
        if let Some(next) = self.contextual_iteration_type(IterationKind::Next, function)? {
            return Ok(next);
        }
        let options = self.program()?.host.options();
        if options.strict_option_value(options.no_implicit_any)
            && !self.expression_result_is_unused(node)?
        {
            let context = self.contextual_expression_type(node)?;
            if match context {
                Some(ty) => self.types.flags(ty)? & tf::ANY != 0,
                None => true,
            } {
                self.error_at(Some(node),d::X_yield_expression_implicitly_results_in_an_any_type_because_its_containing_generator_lacks_a_return_type_annotation,vec![])?;
            }
        }
        Ok(self.builtins.any_type)
    }

    // port: tsc/internal/checker/utilities.go:expressionResultIsUnused
    pub(crate) fn expression_result_is_unused(&self, mut node: NodeId) -> Result<bool, Error> {
        loop {
            let Some(parent) = self.ast(node)?.node(node)?.parent() else {
                return Ok(false);
            };
            let read = self.ast(parent)?.node(parent)?;
            match read.kind().known() {
                Some(K::ParenthesizedExpression) => node = parent,
                Some(K::ExpressionStatement | K::VoidExpression) => return Ok(true),
                Some(K::ForStatement) => {
                    let data = read
                        .data_source()
                        .as_for_statement()
                        .ok_or(Error::MissingLink("for statement"))?;
                    return Ok(data.initializer() == Some(node) || data.incrementor() == Some(node));
                }
                Some(K::BinaryExpression) => {
                    let data = read
                        .data_source()
                        .as_binary_expression()
                        .ok_or(Error::MissingLink("unused binary expression"))?;
                    let operator = data
                        .operator_token()
                        .ok_or(Error::MissingLink("unused binary operator"))?;
                    if self.ast(operator)?.node(operator)?.kind() != K::CommaToken {
                        return Ok(false);
                    }
                    if data.left() == Some(node) {
                        return Ok(true);
                    }
                    node = parent;
                }
                _ => return Ok(false),
            }
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getContextualTypeForYieldOperand
    pub(crate) fn contextual_type_for_yield_operand(
        &mut self,
        node: NodeId,
        flags: u32,
    ) -> Result<Option<TypeId>, Error> {
        let Some(function) = self.containing_body_function(node)? else {
            return Ok(None);
        };
        let Some(mut context) = self.contextual_body_return_type_ex(function, flags)? else {
            return Ok(None);
        };
        let asynchronous = self.body_function_flags(function)?.0;
        let star = self.yield_is_star(node)?;
        if !star && self.types.flags(context)? & tf::UNION != 0 {
            context = self.filter_type(context, &mut |checker, part| {
                Ok(checker
                    .generator_return_iteration_type(IterationKind::Return, part, asynchronous)?
                    .is_some())
            })?;
        }
        if star {
            let types = self.generator_return_iteration_types(context, asynchronous)?;
            let yielded = types.yield_type.unwrap_or(self.builtins.silent_never_type);
            let returned = self
                .contextual_expression_type_ex(node, flags)?
                .unwrap_or(self.builtins.silent_never_type);
            let next = types.next_type.unwrap_or(self.builtins.unknown_type);
            let sync = self.create_generator_type(yielded, returned, next, false)?;
            if asynchronous {
                let async_ = self.create_generator_type(yielded, returned, next, true)?;
                return self.get_union_type(&[sync, async_]).map(Some);
            }
            return Ok(Some(sync));
        }
        self.generator_return_iteration_type(IterationKind::Yield, context, asynchronous)
    }
}
