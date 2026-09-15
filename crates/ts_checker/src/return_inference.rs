//! Function-body return aggregation and inferred type predicates. Source order
//! and the distinction between implicit return, `return;`, and `never` are
//! observable through signature inference and the relater's type census.

use crate::{
    object_flags as of, type_flags as tf, CheckerState, Error, SignatureId, TypeId,
    TypePredicateId, UnionReduction,
};
use ts_arena::NodeId;
use ts_ast::{modifier_flags as mf, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getReturnTypeFromBody
    pub(crate) fn return_type_from_body(&mut self, function: NodeId) -> Result<TypeId, Error> {
        self.return_type_from_body_ex(function, 0)
    }

    pub(crate) fn return_type_from_body_ex(
        &mut self,
        function: NodeId,
        mode: u32,
    ) -> Result<TypeId, Error> {
        let read = self.ast(function)?.node(function)?;
        let Some(body) = read.body() else {
            return Ok(self.builtins.error_type);
        };
        let (is_async, is_generator) = self.body_function_flags(function)?;
        if is_generator {
            return self.generator_return_type_from_body(function, body, mode, is_async);
        }
        let mut return_type = if self.ast(body)?.node(body)?.kind() == K::Block {
            let (types, never_returning) =
                self.aggregate_return_expression_types(function, body, mode)?;
            if never_returning {
                return if is_async {
                    self.create_promise_return_type(function, self.builtins.never_type)
                } else {
                    Ok(self.builtins.never_type)
                };
            }
            if types.is_empty() {
                let contextual = self.contextual_body_return_type(function)?;
                let contextual = contextual
                    .map(|ty| self.unwrap_body_return_type(function, ty))
                    .transpose()?;
                let contains_undefined = contextual
                    .map(|ty| self.return_type_contains_undefined(ty))
                    .transpose()?
                    .unwrap_or(false);
                let ty = if contains_undefined {
                    self.builtins.undefined_type
                } else {
                    self.builtins.void_type
                };
                return if is_async {
                    self.create_promise_return_type(function, ty)
                } else {
                    Ok(ty)
                };
            }
            self.get_union_type_ex(&types, UnionReduction::Subtype, None, None)?
        } else {
            let mut ty = self.return_expression_type(body, mode)?;
            if self.is_const_context(body)? {
                ty = self.get_regular_type_of_literal_type(ty)?;
            }
            if is_async {
                self.awaited_body_return_type(ty, function)?
            } else {
                ty
            }
        };
        self.report_function_widening(
            function,
            return_type,
            crate::iteration::IterationKind::Return,
        )?;
        if self.types.flags(return_type)? & tf::UNIT != 0 {
            // When the contextual signature is the function's own signature, the
            // contextual type is the inferred return type itself.
            let own = self.signature_from_declaration(function)?;
            let contextual = if self.contextual_body_signature(function)? == Some(own) {
                Some(return_type)
            } else {
                let contextual = self.contextual_body_return_type(function)?;
                let inference = self.call_inference_at_node(function)?;
                contextual
                    .map(|ty| self.instantiate_call_contextual_type(ty, inference, false))
                    .transpose()?
            };
            let contextual = if is_async {
                contextual
                    .map(|ty| self.get_promised_type_of_promise(ty))
                    .transpose()?
                    .flatten()
            } else {
                contextual
            };
            if !self.literal_of_context(return_type, contextual)? {
                return_type = self.widen_literal_type(return_type)?;
                if self.types.flags(return_type)? & tf::UNIQUE_ES_SYMBOL != 0 {
                    return_type = self.builtins.es_symbol_type;
                }
            }
            return_type = self.get_regular_type_of_literal_type(return_type)?;
        }
        let ty = self.widened_inference_type(return_type)?;
        if is_async {
            self.create_promise_type(ty)
        } else {
            Ok(ty)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkAwaitedType
    fn awaited_body_return_type(&mut self, ty: TypeId, node: NodeId) -> Result<TypeId, Error> {
        let awaited = self.awaited_type_no_alias_ex(ty, Some(node), Some(ts_diagnostics::The_return_type_of_an_async_function_must_either_be_a_valid_promise_or_must_not_contain_a_callable_then_member), &[])?
            .unwrap_or(self.builtins.error_type);
        self.unwrap_awaited_type(awaited)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkAsyncFunctionReturnType
    pub(crate) fn check_async_return_annotation(
        &mut self,
        function: NodeId,
        annotation: NodeId,
    ) -> Result<(), Error> {
        let returned = self.get_type_from_type_node(annotation)?;
        if self.is_error_type(returned)? {
            return Ok(());
        }
        let promise = self.global_promise_type(true)?;
        if promise != self.builtins.empty_generic_type
            && (self.types.object_flags(returned)? & of::REFERENCE == 0
                || self.types.target(returned)? != promise)
        {
            let awaited = self
                .awaited_type_no_alias(returned)?
                .unwrap_or(self.builtins.void_type);
            let text = self.type_to_string(
                awaited,
                crate::type_format_flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                    | crate::type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
            )?;
            self.error_at(Some(annotation), ts_diagnostics::The_return_type_of_an_async_function_or_method_must_be_the_global_Promise_T_type_Did_you_mean_to_write_Promise_0, vec![text])?;
            return Ok(());
        }
        self.awaited_body_return_type(returned, function)?;
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.createPromiseReturnType
    pub(crate) fn create_promise_return_type(
        &mut self,
        function: NodeId,
        ty: TypeId,
    ) -> Result<TypeId, Error> {
        let promise = self.create_promise_type(ty)?;
        let import = crate::external_resolution::is_import_call(
            self.ast(function)?,
            &self.ast(function)?.node(function)?,
        )?;
        if promise == self.builtins.unknown_type {
            self.error_at(Some(function), if import { ts_diagnostics::A_dynamic_import_call_returns_a_Promise_Make_sure_you_have_a_declaration_for_Promise_or_include_ES2015_in_your_lib_option } else { ts_diagnostics::An_async_function_or_method_must_return_a_Promise_Make_sure_you_have_a_declaration_for_Promise_or_include_ES2015_in_your_lib_option }, vec![])?;
            return Ok(self.builtins.error_type);
        }
        if self.global_promise_constructor_symbol(true)?.is_none() {
            self.error_at(Some(function), if import { ts_diagnostics::A_dynamic_import_call_in_ES5_requires_the_Promise_constructor_Make_sure_you_have_a_declaration_for_the_Promise_constructor_or_include_ES2015_in_your_lib_option } else { ts_diagnostics::An_async_function_or_method_in_ES5_requires_the_Promise_constructor_Make_sure_you_have_a_declaration_for_the_Promise_constructor_or_include_ES2015_in_your_lib_option }, vec![])?;
        }
        Ok(promise)
    }

    // port: tsc/internal/checker/checker.go:Checker.unwrapReturnType
    pub(crate) fn unwrap_body_return_type(
        &mut self,
        function: NodeId,
        ty: TypeId,
    ) -> Result<TypeId, Error> {
        let (asynchronous, generator) = self.body_function_flags(function)?;
        if generator {
            let part = self.generator_return_iteration_type(
                crate::iteration::IterationKind::Return,
                ty,
                asynchronous,
            )?;
            let Some(part) = part else {
                return Ok(self.builtins.error_type);
            };
            if asynchronous {
                let part = self.unwrap_awaited_type(part)?;
                return Ok(self
                    .awaited_type_no_alias(part)?
                    .unwrap_or(self.builtins.error_type));
            }
            return Ok(part);
        }
        if asynchronous {
            return Ok(self
                .awaited_type_no_alias(ty)?
                .unwrap_or(self.builtins.error_type));
        }
        Ok(ty)
    }

    fn return_expression_type(&mut self, expression: NodeId, mode: u32) -> Result<TypeId, Error> {
        let mode = mode & !8; // CheckModeSkipGenericFunctions
        if mode == 0 {
            self.check_expression_cached(expression)
        } else {
            self.check_expression_ex(expression, mode)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkAndAggregateReturnExpressionTypes
    pub(crate) fn aggregate_return_expression_types(
        &mut self,
        function: NodeId,
        body: NodeId,
        mode: u32,
    ) -> Result<(Vec<TypeId>, bool), Error> {
        let mut types = Vec::new();
        let asynchronous = self.body_function_flags(function)?.0;
        let mut has_empty_return = self.function_has_implicit_return(function)?;
        let mut has_never_return = false;
        for statement in self.return_statements(body)? {
            let Some(expression) = self.ast(statement)?.node(statement)?.expression() else {
                has_empty_return = true;
                continue;
            };
            let mut expression = self.skip_return_parentheses(expression)?;
            if asynchronous && self.ast(expression)?.node(expression)?.kind() == K::AwaitExpression
            {
                expression = self.skip_return_parentheses(
                    self.ast(expression)?
                        .node(expression)?
                        .expression()
                        .ok_or(Error::MissingLink("awaited recursive return"))?,
                )?;
            }
            if self.is_bare_recursive_return(function, expression)? {
                has_never_return = true;
                continue;
            }
            let mut ty = self.return_expression_type(expression, mode)?;
            if asynchronous {
                ty = self.awaited_body_return_type(ty, function)?;
            }
            has_never_return |= self.types.flags(ty)? & tf::NEVER != 0;
            if self.is_const_context(expression)? {
                ty = self.get_regular_type_of_literal_type(ty)?;
            }
            if !types.contains(&ty) {
                types.push(ty);
            }
        }
        if types.is_empty()
            && !has_empty_return
            && (has_never_return || self.may_return_never(function)?)
        {
            return Ok((types, true));
        }
        if self.options.strict_null_checks
            && !types.is_empty()
            && has_empty_return
            && !types.contains(&self.builtins.undefined_type)
        {
            types.push(self.builtins.undefined_type);
        }
        Ok((types, false))
    }

    // port: tsc/internal/ast/utilities.go:ForEachReturnStatement
    // The restricted statement traversal intentionally does not enter nested
    // functions, class expressions, or arbitrary expression children.
    pub(crate) fn return_statements(&self, body: NodeId) -> Result<Vec<NodeId>, Error> {
        let mut stack = vec![body];
        let mut returns = Vec::new();
        while let Some(node) = stack.pop() {
            match self.ast(node)?.node(node)?.kind().known() {
                Some(K::ReturnStatement) => returns.push(node),
                Some(
                    K::CaseBlock
                    | K::Block
                    | K::IfStatement
                    | K::DoStatement
                    | K::WhileStatement
                    | K::ForStatement
                    | K::ForInStatement
                    | K::ForOfStatement
                    | K::WithStatement
                    | K::SwitchStatement
                    | K::CaseClause
                    | K::DefaultClause
                    | K::LabeledStatement
                    | K::TryStatement
                    | K::CatchClause,
                ) => stack.extend(self.source_children(node)?.into_iter().rev()),
                _ => {}
            }
        }
        Ok(returns)
    }

    // port: tsc/internal/checker/checker.go:mayReturnNever
    fn may_return_never(&self, function: NodeId) -> Result<bool, Error> {
        let read = self.ast(function)?.node(function)?;
        Ok(match read.kind().known() {
            Some(K::FunctionExpression | K::ArrowFunction) => true,
            Some(K::MethodDeclaration) => match read.parent() {
                Some(parent) => {
                    self.ast(parent)?.node(parent)?.kind() == K::ObjectLiteralExpression
                }
                None => false,
            },
            _ => false,
        })
    }

    // port: tsc/internal/ast/functionflags.go:GetFunctionFlags
    pub(crate) fn body_function_flags(&self, function: NodeId) -> Result<(bool, bool), Error> {
        let view = self.ast(function)?;
        let read = view.node(function)?;
        let data = read.data_source();
        let generator = match read.kind().known() {
            Some(K::FunctionDeclaration) => data
                .as_function_declaration()
                .is_some_and(|data| data.asterisk_token().is_some()),
            Some(K::FunctionExpression) => data
                .as_function_expression()
                .is_some_and(|data| data.asterisk_token().is_some()),
            Some(K::MethodDeclaration) => data
                .as_method_declaration()
                .is_some_and(|data| data.asterisk_token().is_some()),
            _ => false,
        };
        let asynchronous = matches!(
            read.kind().known(),
            Some(
                K::FunctionDeclaration
                    | K::FunctionExpression
                    | K::MethodDeclaration
                    | K::ArrowFunction
            )
        ) && read.modifier_flags(view)? & mf::ASYNC != 0;
        Ok((asynchronous, generator))
    }

    fn skip_return_parentheses(&self, mut expression: NodeId) -> Result<NodeId, Error> {
        loop {
            let read = self.ast(expression)?.node(expression)?;
            if read.kind() != K::ParenthesizedExpression {
                return Ok(expression);
            }
            expression = read
                .expression()
                .ok_or(Error::MissingLink("parenthesized return"))?;
        }
    }

    fn is_bare_recursive_return(
        &mut self,
        function: NodeId,
        expression: NodeId,
    ) -> Result<bool, Error> {
        let read = self.ast(expression)?.node(expression)?;
        if read.kind() != K::CallExpression {
            return Ok(false);
        }
        let callee = read
            .expression()
            .ok_or(Error::MissingLink("return call target"))?;
        if self.ast(callee)?.node(callee)?.kind() != K::Identifier {
            return Ok(false);
        }
        let callee_type = self.check_expression_cached(callee)?;
        let function_symbol = self.get_symbol_of_declaration(function)?;
        let Some(function_symbol) = function_symbol else {
            return Ok(false);
        };
        let function_symbol = self.get_merged_symbol(function_symbol);
        if self.types.get(callee_type)?.symbol != Some(function_symbol) {
            return Ok(false);
        }
        if let Some(value) = self.symbol(function_symbol)?.value_declaration() {
            if matches!(
                self.ast(value)?.node(value)?.kind().known(),
                Some(K::FunctionExpression | K::ArrowFunction)
            ) {
                return self.constant_flow_reference(callee);
            }
        }
        Ok(true)
    }

    fn return_type_contains_undefined(&self, ty: TypeId) -> Result<bool, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::UNDEFINED != 0 {
            return Ok(true);
        }
        if flags & tf::UNION != 0 {
            for &part in self.types.compound_types(ty)?.iter() {
                if self.return_type_contains_undefined(part)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getContextualReturnType
    pub(crate) fn contextual_body_return_type(
        &mut self,
        function: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        self.contextual_body_return_type_ex(function, 0)
    }
    pub(crate) fn contextual_body_return_type_ex(
        &mut self,
        function: NodeId,
        context_flags: u32,
    ) -> Result<Option<TypeId>, Error> {
        if let Some(ty) = self.return_type_from_annotation(function)? {
            return Ok(Some(ty));
        }
        if let Some(signature) = self.contextual_body_signature(function)? {
            if !self.resolving_signature_return(signature)? {
                let ty = self.return_type_of_signature(signature)?;
                let (asynchronous, generator) = self.body_function_flags(function)?;
                if generator {
                    return self
                        .filter_type(ty, &mut |checker, part| {
                            Ok(checker.types.flags(part)?
                                & (tf::ANY_OR_UNKNOWN | tf::VOID | tf::INSTANTIABLE_NON_PRIMITIVE)
                                != 0
                                || checker.generator_instantiation_assignable(
                                    part,
                                    asynchronous,
                                    None,
                                )?)
                        })
                        .map(Some);
                }
                if asynchronous {
                    return self
                        .filter_type(ty, &mut |checker, part| {
                            Ok(checker.types.flags(part)?
                                & (tf::ANY_OR_UNKNOWN | tf::VOID | tf::INSTANTIABLE_NON_PRIMITIVE)
                                != 0
                                || checker.awaited_type_of_promise(part)?.is_some())
                        })
                        .map(Some);
                }
                return Ok(Some(ty));
            }
        }
        if let Some(call) =
            ts_ast::get_immediately_invoked_function_expression(self.ast(function)?, function)?
        {
            return self.contextual_expression_type_ex(call, context_flags);
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getContextualTypeForReturnExpression
    pub(crate) fn contextual_type_for_return_expression_ex(
        &mut self,
        expression: NodeId,
        context_flags: u32,
    ) -> Result<Option<TypeId>, Error> {
        let Some(function) = self.containing_body_function(expression)? else {
            return Ok(None);
        };
        let Some(mut ty) = self.contextual_body_return_type_ex(function, context_flags)? else {
            return Ok(None);
        };
        let (asynchronous, generator) = self.body_function_flags(function)?;
        if generator {
            if self.types.flags(ty)? & tf::UNION != 0 {
                ty = self.filter_type(ty, &mut |checker, part| {
                    Ok(checker
                        .generator_return_iteration_type(
                            crate::iteration::IterationKind::Return,
                            part,
                            asynchronous,
                        )?
                        .is_some())
                })?;
            }
            let Some(part) = self.generator_return_iteration_type(
                crate::iteration::IterationKind::Return,
                ty,
                asynchronous,
            )?
            else {
                return Ok(None);
            };
            ty = part;
        }
        if asynchronous {
            let Some(awaited) =
                self.map_type(ty, &mut |checker, part| checker.awaited_type_no_alias(part))?
            else {
                return Ok(None);
            };
            let promise = self.create_promise_like_type(awaited)?;
            return self.get_union_type(&[awaited, promise]).map(Some);
        }
        Ok(Some(ty))
    }

    // port: tsc/internal/ast/utilities.go:GetContainingFunction
    pub(crate) fn containing_body_function(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let mut current = self.ast(node)?.node(node)?.parent();
        while let Some(node) = current {
            let read = self.ast(node)?.node(node)?;
            if ts_ast::utilities::is_function_like(Some(&read)) {
                return Ok(Some(node));
            }
            current = read.parent();
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getContextualSignature
    pub(crate) fn contextual_body_signature(
        &mut self,
        function: NodeId,
    ) -> Result<Option<SignatureId>, Error> {
        let read = self.ast(function)?.node(function)?;
        let method = read.kind() == K::MethodDeclaration
            && read
                .parent()
                .map(|parent| {
                    self.ast(parent)?
                        .node(parent)
                        .map(|read| read.kind() == K::ObjectLiteralExpression)
                        .map_err(Error::from)
                })
                .transpose()?
                .unwrap_or(false);
        if !matches!(
            read.kind().known(),
            Some(K::FunctionExpression | K::ArrowFunction)
        ) && !method
        {
            return Ok(None);
        }
        let Some(ty) = self.apparent_contextual_expression_type_ex(function, 1)? else {
            return Ok(None);
        };
        if self.types.flags(ty)? & tf::UNION == 0 {
            return self.contextual_signature_for_type(function, ty);
        }
        let mut signatures = Vec::new();
        for &part in self.types.compound_types(ty)?.clone().iter() {
            if let Some(signature) = self.contextual_signature_for_type(function, part)? {
                if let Some(&first) = signatures.first() {
                    let identical = self.compare_signatures_identical(
                        first,
                        signature,
                        false,
                        true,
                        true,
                        &mut |checker, source, target| {
                            Ok(
                                if checker.is_type_related_to(
                                    source,
                                    target,
                                    crate::RelationKind::Identity,
                                )? {
                                    crate::ternary::TRUE
                                } else {
                                    crate::ternary::FALSE
                                },
                            )
                        },
                    )?;
                    if identical == crate::ternary::FALSE {
                        return Ok(None);
                    }
                }
                signatures.push(signature);
            }
        }
        match signatures.as_slice() {
            [] => Ok(None),
            [signature] => Ok(Some(*signature)),
            _ => {
                let result = self.clone_signature(signatures[0])?;
                let stored = self.signatures.get_mut(result)?;
                stored.composite = Some(crate::signatures::CompositeSignature {
                    is_union: true,
                    signatures: signatures.into(),
                });
                stored.target = None;
                stored.mapper = None;
                Ok(Some(result))
            }
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getContextualCallSignature
    pub(crate) fn contextual_signature_for_type(
        &mut self,
        function: NodeId,
        ty: TypeId,
    ) -> Result<Option<SignatureId>, Error> {
        let signatures = self.signatures_of_type(ty, false)?;
        let parameters = self.source_list(
            function,
            self.ast(function)?.node(function)?.parameter_list(),
        )?;
        let mut minimum: isize = 0;
        for parameter in &parameters {
            let read = self.ast(*parameter)?.node(*parameter)?;
            if read.initializer().is_some()
                || read.question_token(self.ast(*parameter)?)?.is_some()
                || read
                    .data_source()
                    .as_parameter_declaration()
                    .ok_or(Error::MissingLink("contextual parameter"))?
                    .dot_dot_dot_token()
                    .is_some()
            {
                break;
            }
            minimum += 1;
        }
        if let Some(&first) = parameters.first() {
            if let Some(name) = self.ast(first)?.node(first)?.name() {
                if self.ast(name)?.node(name)?.kind() == K::Identifier
                    && self.ast(name)?.node_text(name)?.as_bytes() == b"this"
                {
                    minimum -= 1;
                }
            }
        }
        let mut applicable = Vec::new();
        for signature in signatures {
            if self.effective_rest_parameter(signature)?
                || self.parameter_count(signature)? as isize >= minimum
            {
                applicable.push(signature);
            }
        }
        if applicable.len() == 1 {
            return Ok(Some(applicable[0]));
        }
        if !self
            .program()?
            .host
            .options()
            .strict_option_value(self.program()?.host.options().no_implicit_any)
        {
            return Ok(None);
        }
        let mut combined = None;
        for signature in applicable {
            let Some(previous) = combined else {
                combined = Some(signature);
                continue;
            };
            if previous == signature {
                continue;
            }
            let left = self
                .signatures
                .get(previous)?
                .type_parameters
                .clone()
                .unwrap_or_else(|| [].into());
            let right = self
                .signatures
                .get(signature)?
                .type_parameters
                .clone()
                .unwrap_or_else(|| [].into());
            if !self.signature_type_parameters_identical(&left, &right)? {
                return Ok(None);
            }
            combined = Some(self.combine_member_signatures(previous, signature, false)?);
        }
        Ok(combined)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypePredicateFromBody
    pub(crate) fn type_predicate_from_body(
        &mut self,
        function: NodeId,
    ) -> Result<Option<TypePredicateId>, Error> {
        let read = self.ast(function)?.node(function)?;
        if matches!(
            read.kind().known(),
            Some(K::Constructor | K::GetAccessor | K::SetAccessor)
        ) {
            return Ok(None);
        }
        let Some(body) = read.body() else {
            return Ok(None);
        };
        if self.body_function_flags(function)? != (false, false) {
            return Ok(None);
        }
        let expression = if self.ast(body)?.node(body)?.kind() == K::Block {
            let returns = self.return_statements(body)?;
            if returns.len() != 1 || self.function_has_implicit_return(function)? {
                return Ok(None);
            }
            let Some(expression) = self.ast(returns[0])?.node(returns[0])?.expression() else {
                return Ok(None);
            };
            expression
        } else {
            body
        };
        self.expression_refines_any_parameter(function, expression)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkIfExpressionRefinesAnyParameter
    fn expression_refines_any_parameter(
        &mut self,
        function: NodeId,
        expression: NodeId,
    ) -> Result<Option<TypePredicateId>, Error> {
        let expression = self.skip_return_parentheses(expression)?;
        let return_type = self.check_expression_cached(expression)?;
        if self.types.flags(return_type)? & tf::BOOLEAN == 0 {
            return Ok(None);
        }
        let parameters = self.source_list(
            function,
            self.ast(function)?.node(function)?.parameter_list(),
        )?;
        for (index, parameter) in parameters.into_iter().enumerate() {
            let symbol = self
                .get_symbol_of_declaration(parameter)?
                .ok_or(Error::MissingLink("predicate parameter symbol"))?;
            let initial = self.get_type_of_symbol(symbol)?;
            let read = self.ast(parameter)?.node(parameter)?;
            let name = read
                .name()
                .ok_or(Error::MissingLink("predicate parameter name"))?;
            let rest = read
                .data_source()
                .as_parameter_declaration()
                .ok_or(Error::MissingLink("predicate parameter"))?
                .dot_dot_dot_token()
                .is_some();
            if self.types.flags(initial)? & tf::BOOLEAN != 0
                || self.ast(name)?.node(name)?.kind() != K::Identifier
                || self.is_symbol_assigned(symbol)?
                || rest
            {
                continue;
            }
            if let Some(ty) =
                self.expression_refines_parameter(function, expression, name, initial)?
            {
                let text = self.ast(name)?.node_text(name)?.into_js_string();
                return self
                    .signatures
                    .new_type_predicate(crate::signatures::TypePredicate {
                        kind: crate::TypePredicateKind::Identifier,
                        parameter_name: text,
                        parameter_index: index as i32,
                        t: Some(ty),
                    })
                    .map(Some);
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkIfExpressionRefinesParameter
    fn expression_refines_parameter(
        &mut self,
        function: NodeId,
        expression: NodeId,
        parameter_name: NodeId,
        initial: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        let true_type = self.infer_predicate_flow_type(
            parameter_name,
            initial,
            initial,
            function,
            expression,
            true,
        )?;
        if true_type == initial {
            return Ok(None);
        }
        let false_subtype = self.infer_predicate_flow_type(
            parameter_name,
            initial,
            true_type,
            function,
            expression,
            false,
        )?;
        let false_subtype = self.get_reduced_type(false_subtype)?;
        Ok((self.types.flags(false_subtype)? & tf::NEVER != 0).then_some(true_type))
    }
}
