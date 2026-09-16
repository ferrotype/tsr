//! Statement checking and the deferred function-expression body schedule.
//! Queries can resolve a function's type before the surrounding variable has
//! finished inference. Its body is checked after source statements, in insertion
//! order, so a recursive reference cannot poison that variable's type.

use crate::{
    signature_flags as sg, type_flags as tf, types::Map, CheckerState, Error, SignatureId, TypeId,
};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{node_flags as nf, Diagnostic, SyntaxKind as K};
use ts_core::{TextRange, Tristate};
use ts_diagnostics as messages;

#[derive(Clone, Copy)]
enum ContextStatus {
    Checking,
    Complete,
    Failed(Error),
}

#[derive(Default)]
struct DeferredBodies {
    nodes: Vec<NodeId>,
    queued: Map<NodeId, ()>,
}

#[derive(Default)]
pub(crate) struct BodyCheckState {
    contexts: Map<NodeId, ContextStatus>,
    deferred: Map<NodeId, DeferredBodies>,
    ambient_reported: Map<NodeId, ()>,
    pub(crate) arguments_referenced: Map<NodeId, bool>,
}

#[cfg(any(test, feature = "storage-pilot"))]
impl BodyCheckState {
    pub(crate) fn census(&self, census: &mut crate::census::Census) {
        census.map("body_check_state", &self.contexts);
        census.map("body_check_state", &self.deferred);
        census.map("body_check_state", &self.ambient_reported);
        census.map("body_check_state", &self.arguments_referenced);
        for queue in self.deferred.values() {
            census.vec_capacity("body_check_state", &queue.nodes, queue.nodes.capacity());
            census.map("body_check_state", &queue.queued);
        }
    }
}

impl CheckerState {
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarForUseStrictSimpleParameterList
    pub(crate) fn check_body_use_strict_parameters(
        &mut self,
        function: NodeId,
    ) -> Result<bool, Error> {
        if self.program()?.host.options().emit_script_target() < ts_core::ScriptTarget::ES2016 {
            return Ok(false);
        }
        let view = self.ast(function)?;
        let Some(body) = view.node(function)?.body() else {
            return Ok(false);
        };
        if view.node(body)?.kind() != K::Block {
            return Ok(false);
        }
        let source = ts_ast::utilities::get_source_file_of_node(view, Some(function))?
            .ok_or(Error::MissingLink("function strict source"))?;
        let statements: Vec<_> = view
            .node_slice(view.node(body)?.statements(view)?)?
            .iter()
            .collect();
        let Some(directive) = ts_binder::find_use_strict_prologue(view, source, &statements) else {
            return Ok(false);
        };
        let mut non_simple = Vec::new();
        for parameter in self.source_list(function, view.node(function)?.parameter_list())? {
            let read = self.ast(parameter)?.node(parameter)?;
            let name = read
                .name()
                .ok_or(Error::MissingLink("strict parameter name"))?;
            if read.initializer().is_some()
                || matches!(
                    self.ast(name)?.node(name)?.kind().known(),
                    Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
                )
                || read
                    .data_source()
                    .as_parameter_declaration()
                    .ok_or(Error::MissingLink("strict parameter"))?
                    .dot_dot_dot_token()
                    .is_some()
            {
                non_simple.push(parameter);
            }
        }
        if non_simple.is_empty() {
            return Ok(false);
        }
        for &parameter in &non_simple {
            let mut diagnostic = self.diagnostic_for_node(
                Some(parameter),
                messages::This_parameter_is_not_allowed_with_use_strict_directive,
                vec![],
            )?;
            diagnostic
                .related_information
                .push(std::sync::Arc::new(self.diagnostic_for_node(
                    Some(directive),
                    messages::X_use_strict_directive_used_here,
                    vec![],
                )?));
            self.add_diagnostic(diagnostic)?;
        }
        let mut diagnostic = self.diagnostic_for_node(
            Some(directive),
            messages::X_use_strict_directive_cannot_be_used_with_non_simple_parameter_list,
            vec![],
        )?;
        for (index, parameter) in non_simple.into_iter().enumerate() {
            diagnostic
                .related_information
                .push(std::sync::Arc::new(self.diagnostic_for_node(
                    Some(parameter),
                    if index == 0 {
                        messages::Non_simple_parameter_declared_here
                    } else {
                        messages::X_and_here
                    },
                    vec![],
                )?));
        }
        self.add_diagnostic(diagnostic)?;
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkFunctionDeclaration
    // port: tsc/internal/checker/checker.go:Checker.checkFunctionOrMethodDeclaration
    pub(crate) fn check_function_declaration(&mut self, function: NodeId) -> Result<(), Error> {
        if self.ast(function)?.node(function)?.kind() == K::Constructor {
            return self.check_constructor_body(function);
        }
        if matches!(
            self.ast(function)?.node(function)?.kind().known(),
            Some(K::MethodDeclaration | K::MethodSignature)
        ) {
            self.check_source_method_grammar(function)?;
        }
        self.check_signature_syntax(function)?;
        let read = self.ast(function)?.node(function)?;
        let body = read.body();
        let annotation = read.type_node();
        let kind = read.kind();
        if !matches!(
            kind.known(),
            Some(K::FunctionDeclaration | K::MethodDeclaration | K::MethodSignature)
        ) {
            return Err(Error::MissingLink("function or method declaration"));
        }
        if let Some(name) = read.name() {
            if self.ast(name)?.node(name)?.kind() == K::ComputedPropertyName {
                self.check_computed_property_name(name)?;
            }
        }
        let bindable = if !ts_ast::has_dynamic_name(self.ast(function)?, Some(function))? {
            true
        } else if let Some(name) = self.late_name(function)? {
            let ty = self.late_name_type(name)?;
            self.types.flags(ty)? & (tf::STRING_OR_NUMBER_LITERAL | tf::UNIQUE_ES_SYMBOL) != 0
        } else {
            false
        };
        if bindable {
            let symbol = self
                .get_symbol_of_declaration(function)?
                .ok_or(Error::MissingLink("function declaration symbol"))?;
            let local = self
                .program()?
                .bound(function)?
                .node_binding(function)?
                .and_then(|binding| binding.local_symbol)
                .unwrap_or(symbol);
            if self.ast(function)?.node(function)?.flags() & nf::JAVA_SCRIPT_FILE == 0 {
                self.check_function_or_constructor_symbol(local)?;
            }
            if self.symbol(symbol)?.parent().is_some() {
                self.check_function_or_constructor_symbol(symbol)?;
            }
        }
        if let Some(body) = body {
            self.check_source_element(body)?;
        }
        let return_type = self.return_type_from_annotation(function)?;
        self.check_function_return_paths(function, return_type)?;
        self.check_full_signature_arity(function)?;
        let body_missing = match body {
            None => true,
            Some(body) => ts_ast::node_is_missing(Some(&self.ast(body)?.node(body)?)),
        };
        if annotation.is_none() && body_missing {
            let read = self.ast(function)?.node(function)?;
            let private_ambient = self.binding_private_ambient(function)?;
            if !private_ambient
                && self
                    .program()?
                    .host
                    .options()
                    .strict_option_value(self.program()?.host.options().no_implicit_any)
            {
                let name =
                    ts_scanner::declaration_name_to_string(self.ast(function)?, read.name())?;
                self.error_at(Some(function), messages::X_0_which_lacks_return_type_annotation_implicitly_has_an_1_return_type, vec![name, ts_ast::JsString::from_bytes(b"any".as_slice())])?;
            }
        }
        if annotation.is_none() && !body_missing && self.body_function_flags(function)?.1 {
            let signature = self.signature_from_declaration(function)?;
            self.return_type_of_signature(signature)?;
        }
        if kind == K::FunctionDeclaration {
            self.check_grammar_generator(function)?;
            self.check_function_name_collision_boundary(function)?;
        } else {
            let read = self.ast(function)?.node(function)?;
            if kind == K::MethodDeclaration
                && read.modifier_flags(self.ast(function)?)? & ts_ast::modifier_flags::ABSTRACT != 0
                && body.is_some()
            {
                let name =
                    ts_scanner::declaration_name_to_string(self.ast(function)?, read.name())?;
                self.error_at(
                    Some(function),
                    messages::Method_0_cannot_have_an_implementation_because_it_is_marked_abstract,
                    vec![name],
                )?;
            }
            // Private named methods are only allowed in class declarations.
            let read = self.ast(function)?.node(function)?;
            if let Some(name) = read.name() {
                if self.ast(name)?.node(name)?.kind() == K::PrivateIdentifier
                    && ts_ast::utilities::get_containing_class(self.ast(function)?, function)?
                        .is_none()
                {
                    self.error_at(
                        Some(function),
                        messages::Private_identifiers_are_not_allowed_outside_class_bodies,
                        vec![],
                    )?;
                }
            }
            self.set_node_links_for_private_identifier_scope(function)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkConstructorDeclaration
    fn check_constructor_body(&mut self, node: NodeId) -> Result<(), Error> {
        self.check_signature_syntax(node)?;
        let read = self.ast(node)?.node(node)?;
        let parameters = read.type_parameter_list();
        let annotation = read.type_node();
        let body = read.body();
        let mut grammar_error = false;
        if let Some(parameters) = parameters {
            let view = self.ast(node)?;
            let loc = view.list(parameters)?.loc();
            let start = if loc.pos() == loc.end() {
                loc.pos()
            } else {
                let source = ts_ast::utilities::get_source_file_of_node(view, Some(node))?
                    .ok_or(Error::MissingLink("constructor type parameter source"))?;
                ts_scanner::skip_trivia(view.source_file(source)?.text().as_bytes(), loc.pos())
            };
            grammar_error = self.grammar_error_range(
                node,
                start,
                loc.end(),
                messages::Type_parameters_cannot_appear_on_a_constructor_declaration,
            )?;
        }
        if !grammar_error {
            if let Some(annotation) = annotation {
                self.grammar_error_node(
                    annotation,
                    messages::Type_annotation_cannot_appear_on_a_constructor_declaration,
                    vec![],
                )?;
            }
        }
        if let Some(body) = body {
            self.check_source_element(body)?;
        }
        let symbol = self
            .get_symbol_of_declaration(node)?
            .ok_or(Error::MissingLink("constructor symbol"))?;
        self.check_function_or_constructor_symbol(symbol)?;
        if body.is_none() {
            return Ok(());
        }
        self.check_constructor_super_calls(node)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkNodeDeferred
    pub(crate) fn defer_checker_node(&mut self, function: NodeId) -> Result<(), Error> {
        let source =
            ts_ast::utilities::get_source_file_of_node(self.ast(function)?, Some(function))?
                .ok_or(Error::MissingLink("deferred function source"))?;
        if matches!(
            self.source_checks.get(&source),
            Some(crate::check::SourceCheckStatus::Complete)
        ) {
            return Ok(());
        }
        let queue = self.body_checks.deferred.entry(source).or_default();
        if queue.queued.insert(function, ()).is_none() {
            queue.nodes.push(function);
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkFunctionExpressionOrObjectLiteralMethod
    // port: tsc/internal/checker/checker.go:Checker.contextuallyCheckFunctionExpressionOrObjectLiteralMethod
    pub(crate) fn check_function_expression(&mut self, function: NodeId) -> Result<TypeId, Error> {
        self.defer_checker_node(function)?;
        self.check_function_name_collision_boundary(function)?;
        if self.expression_mode & 4 != 0 && self.expression_is_context_sensitive(function)? {
            if self.ast(function)?.node(function)?.type_node().is_none()
                && !self.body_has_context_sensitive_parameters(function)?
            {
                if let Some(context) = self.contextual_body_signature(function)? {
                    let returned = self.return_type_of_signature(context)?;
                    if self.could_contain_type_variables(returned)? {
                        if let Some(&cached) = self.query.context_free_types.get(&function) {
                            return Ok(cached);
                        }
                        let returned =
                            self.return_type_from_body_ex(function, self.expression_mode)?;
                        let signature = self.signatures.new_signature(
                            sg::IS_NON_INFERRABLE,
                            None,
                            None,
                            None,
                            None,
                            Some(returned),
                            None,
                            0,
                        )?;
                        let symbol = self.get_symbol_of_declaration(function)?;
                        let ty = self.new_anonymous_type(symbol, None, &[signature], &[], &[])?;
                        self.types.get_mut(ty)?.object_flags |=
                            crate::object_flags::NON_INFERRABLE_TYPE;
                        self.query.context_free_types.insert(function, ty);
                        return Ok(ty);
                    }
                }
            }
            return Ok(self.builtins.any_function_type);
        }
        if !self.check_grammar_function_like(function)?
            && self.ast(function)?.node(function)?.kind() == K::FunctionExpression
        {
            self.check_grammar_generator(function)?;
        }
        self.check_full_signature_arity(function)?;
        match self.body_checks.contexts.get(&function).copied() {
            Some(ContextStatus::Failed(error)) => return Err(error),
            Some(ContextStatus::Checking | ContextStatus::Complete) => {}
            None => {
                self.body_checks
                    .contexts
                    .insert(function, ContextStatus::Checking);
                let result = self.check_function_expression_context(function);
                self.body_checks.contexts.insert(
                    function,
                    match result {
                        Ok(()) => ContextStatus::Complete,
                        Err(error) => ContextStatus::Failed(error),
                    },
                );
                result?;
            }
        }
        let symbol = self
            .get_symbol_of_declaration(function)?
            .ok_or(Error::MissingLink("function expression symbol"))?;
        self.get_type_of_symbol(symbol)
    }

    fn check_function_expression_context(&mut self, function: NodeId) -> Result<(), Error> {
        let read = self.ast(function)?.node(function)?;
        if !matches!(
            read.kind().known(),
            Some(K::FunctionExpression | K::ArrowFunction | K::MethodDeclaration)
        ) {
            return Err(Error::Unsupported(
                "contextuallyCheckFunctionExpression: object literal method",
            ));
        }
        let annotation = read.type_node();
        let contextual = self.contextual_body_signature(function)?;
        let symbol = self
            .get_symbol_of_declaration(function)?
            .ok_or(Error::MissingLink("contextual function symbol"))?;
        let function_type = self.get_type_of_symbol(symbol)?;
        let signatures = self.signatures_of_type(function_type, false)?;
        let Some(&signature) = signatures.first() else {
            return Ok(());
        };
        if self.expression_is_context_sensitive(function)? {
            if let Some(context) = contextual {
                let context = if let Some(inference) = self.call_inference_at_node(function)? {
                    if self.expression_mode & 2 != 0 {
                        self.infer_annotated_body_signature(signature, context, inference)?;
                    }
                    let rest = self.effective_rest_type(context)?;
                    let non_fixing = match rest {
                        Some(rest) => {
                            self.expression_mode & 2 != 0
                                && self.types.flags(rest)? & tf::TYPE_PARAMETER != 0
                        }
                        None => false,
                    };
                    let inference = self.inference_context(inference)?;
                    let mapper = if non_fixing {
                        inference.non_fixing_mapper
                    } else {
                        inference.mapper
                    };
                    self.instantiate_signature(context, mapper)?
                } else {
                    context
                };
                self.assign_contextual_body_parameters(signature, context)?;
            } else {
                let data = self.signatures.get(signature)?.clone();
                if let Some(parameter) = data.this_parameter {
                    self.assign_body_parameter_type(parameter, None)?;
                }
                for &parameter in data.parameters.as_deref().unwrap_or_default() {
                    self.assign_body_parameter_type(parameter, None)?;
                }
            }
        } else if let Some(context) = contextual {
            let parameters = self.source_list(
                function,
                self.ast(function)?.node(function)?.parameter_list(),
            )?;
            if self
                .ast(function)?
                .node(function)?
                .type_parameter_list()
                .is_none()
                && self
                    .signatures
                    .get(context)?
                    .parameters
                    .as_ref()
                    .map_or(0, |p| p.len())
                    > parameters.len()
                && self.expression_mode & 2 != 0
            {
                let inference = self
                    .call_inference_at_node(function)?
                    .ok_or(Error::MissingLink("annotated function inference"))?;
                self.infer_annotated_body_signature(signature, context, inference)?;
            }
        }
        if contextual.is_some()
            && annotation.is_none()
            && self
                .signatures
                .get(signature)?
                .resolved_return_type
                .is_none()
        {
            let ty = self.return_type_from_body_ex(function, self.expression_mode & !4)?;
            self.signatures
                .get_mut(signature)?
                .resolved_return_type
                .get_or_insert(ty);
        }
        self.check_signature_syntax(function)
    }

    // port: tsc/internal/checker/checker.go:Checker.inferFromAnnotatedParametersAndReturn
    fn infer_annotated_body_signature(
        &mut self,
        signature: SignatureId,
        context: SignatureId,
        inference: crate::InferenceId,
    ) -> Result<(), Error> {
        let data = self.signatures.get(signature)?.clone();
        let parameters = data.parameters.unwrap_or_else(|| [].into());
        let length = parameters.len() - usize::from(data.flags & sg::HAS_REST_PARAMETER != 0);
        for (index, &parameter) in parameters[..length].iter().enumerate() {
            let declaration = self
                .symbol(parameter)?
                .value_declaration()
                .ok_or(Error::MissingLink("annotated inference parameter"))?;
            let read = self.ast(declaration)?.node(declaration)?;
            if let Some(annotation) = read.type_node() {
                let optional = read.question_token(self.ast(declaration)?)?.is_some();
                let source = self.get_type_from_type_node(annotation)?;
                let source = self.add_type_optionality(source, false, optional)?;
                let target = self
                    .parameter_type_at(context, index)?
                    .unwrap_or(self.builtins.any_type);
                self.infer_types(
                    inference,
                    source,
                    target,
                    crate::inference::priority::NONE,
                    false,
                )?;
            }
        }
        if let Some(declaration) = data.declaration {
            if let Some(annotation) = self.ast(declaration)?.node(declaration)?.type_node() {
                let source = self.get_type_from_type_node(annotation)?;
                let target = self.return_type_of_signature(context)?;
                self.infer_types(
                    inference,
                    source,
                    target,
                    crate::inference::priority::NONE,
                    false,
                )?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/ast/utilities.go:HasContextSensitiveParameters
    fn body_has_context_sensitive_parameters(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.type_parameter_list().is_some() {
            return Ok(false);
        }
        let parameters = self.source_list(node, read.parameter_list())?;
        for &parameter in &parameters {
            if self.ast(parameter)?.node(parameter)?.type_node().is_none() {
                return Ok(true);
            }
        }
        if read.kind() != K::ArrowFunction {
            let first_this = match parameters.first().copied() {
                Some(parameter) => match self.ast(parameter)?.node(parameter)?.name() {
                    Some(name) => {
                        self.ast(name)?.node(name)?.kind() == K::Identifier
                            && self.ast(name)?.node_text(name)?.as_bytes() == b"this"
                    }
                    None => false,
                },
                None => false,
            };
            if !first_this {
                return Ok(read.flags() & nf::CONTAINS_THIS != 0);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.isContextSensitive
    pub(crate) fn expression_is_context_sensitive(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        match read.kind().known() {
            Some(
                K::FunctionExpression
                | K::ArrowFunction
                | K::MethodDeclaration
                | K::FunctionDeclaration,
            ) => {
                if self.body_has_context_sensitive_parameters(node)? {
                    return Ok(true);
                }
                if read.type_parameter_list().is_none() && read.type_node().is_none() {
                    if let Some(body) = read.body() {
                        if self.ast(body)?.node(body)?.kind() != K::Block {
                            return self.expression_is_context_sensitive(body);
                        }
                        for statement in self.return_statements(body)? {
                            if let Some(expression) =
                                self.ast(statement)?.node(statement)?.expression()
                            {
                                if self.expression_is_context_sensitive(expression)? {
                                    return Ok(true);
                                }
                            }
                        }
                    }
                }
                if read.type_node().is_none() && self.body_function_flags(node)?.1 {
                    if let Some(body) = read.body() {
                        for yielded in self.yield_expressions(body)? {
                            if let Some(operand) = self.ast(yielded)?.node(yielded)?.expression() {
                                if self.expression_is_context_sensitive(operand)? {
                                    return Ok(true);
                                }
                            }
                        }
                    }
                }
                Ok(false)
            }
            Some(K::ObjectLiteralExpression | K::ArrayLiteralExpression) => {
                for child in self.source_children(node)? {
                    if self.expression_is_context_sensitive(child)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Some(K::ParenthesizedExpression | K::YieldExpression) => match read.expression() {
                Some(expression) => self.expression_is_context_sensitive(expression),
                None => Ok(false),
            },
            Some(K::PropertyAssignment) => match read.initializer() {
                Some(expression) => self.expression_is_context_sensitive(expression),
                None => Ok(false),
            },
            Some(K::ConditionalExpression) => {
                let data = read.data_source();
                let data = data
                    .as_conditional_expression()
                    .ok_or(Error::MissingLink("context conditional"))?;
                Ok(self.expression_is_context_sensitive(
                    data.when_true().ok_or(Error::MissingLink("context true"))?,
                )? || self.expression_is_context_sensitive(
                    data.when_false()
                        .ok_or(Error::MissingLink("context false"))?,
                )?)
            }
            Some(K::BinaryExpression) => {
                let data = read.data_source();
                let data = data
                    .as_binary_expression()
                    .ok_or(Error::MissingLink("context binary"))?;
                let operator = data
                    .operator_token()
                    .ok_or(Error::MissingLink("context binary operator"))?;
                if matches!(
                    self.ast(operator)?.node(operator)?.kind().known(),
                    Some(K::BarBarToken | K::QuestionQuestionToken)
                ) {
                    return Ok(self.expression_is_context_sensitive(
                        data.left().ok_or(Error::MissingLink("context left"))?,
                    )? || self.expression_is_context_sensitive(
                        data.right().ok_or(Error::MissingLink("context right"))?,
                    )?);
                }
                Ok(false)
            }
            Some(K::JsxAttributes | K::JsxAttribute | K::JsxExpression) => {
                Err(Error::Unsupported("isContextSensitive: JSX"))
            }
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.assignContextualParameterTypes
    fn assign_contextual_body_parameters(
        &mut self,
        signature: SignatureId,
        context: SignatureId,
    ) -> Result<(), Error> {
        let source = self.signatures.get(context)?.clone();
        let target = self.signatures.get(signature)?.clone();
        if source
            .type_parameters
            .as_ref()
            .is_some_and(|types| !types.is_empty())
        {
            if target
                .type_parameters
                .as_ref()
                .is_some_and(|types| !types.is_empty())
            {
                return Ok(());
            }
            self.signatures
                .get_mut(signature)?
                .type_parameters
                .clone_from(&source.type_parameters);
        }
        if let Some(context_this) = source.this_parameter {
            let parameter = target.this_parameter;
            let has_annotation = match parameter {
                Some(parameter) => match self.symbol(parameter)?.value_declaration() {
                    Some(declaration) => self
                        .ast(declaration)?
                        .node(declaration)?
                        .type_node()
                        .is_some(),
                    None => true,
                },
                None => false,
            };
            if !has_annotation {
                let parameter = if let Some(parameter) = parameter {
                    parameter
                } else {
                    let parameter = self.clone_symbol_with_type(context_this, None)?;
                    self.signatures.get_mut(signature)?.this_parameter = Some(parameter);
                    parameter
                };
                let ty = self.get_type_of_symbol(context_this)?;
                self.assign_body_parameter_type(parameter, Some(ty))?;
            }
        }
        let parameters = target.parameters.unwrap_or_else(|| [].into());
        let has_rest = target.flags & sg::HAS_REST_PARAMETER != 0;
        let length = parameters.len() - usize::from(has_rest);
        for (index, &parameter) in parameters[..length].iter().enumerate() {
            let declaration = self
                .symbol(parameter)?
                .value_declaration()
                .ok_or(Error::MissingLink("contextual body parameter declaration"))?;
            let read = self.ast(declaration)?.node(declaration)?;
            if read.type_node().is_none() {
                let initializer = read.initializer();
                let mut contextual = self.parameter_type_at(context, index)?;
                if let Some(target) = contextual.filter(|_| initializer.is_some()) {
                    let initial = self.check_declaration_initializer(declaration, 0, None)?;
                    if !self.is_type_related_to(initial, target, crate::RelationKind::Assignable)? {
                        let initial =
                            self.widen_type_inferred_from_initializer(declaration, initial)?;
                        if self.is_type_related_to(
                            target,
                            initial,
                            crate::RelationKind::Assignable,
                        )? {
                            contextual = Some(initial);
                        }
                    }
                }
                self.assign_body_parameter_type(parameter, contextual)?;
            }
        }
        if has_rest {
            let parameter = parameters[length];
            let declaration =
                self.symbol(parameter)?
                    .value_declaration()
                    .ok_or(Error::Unsupported(
                        "assignContextualParameterTypes: synthetic rest",
                    ))?;
            if self
                .ast(declaration)?
                .node(declaration)?
                .type_node()
                .is_none()
            {
                let contextual = self.rest_type_at(context, length)?;
                self.assign_body_parameter_type(parameter, Some(contextual))?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.assignParameterType
    fn assign_body_parameter_type(
        &mut self,
        parameter: SymbolId,
        contextual: Option<TypeId>,
    ) -> Result<(), Error> {
        if self
            .value_symbol_links
            .try_get(parameter)
            .and_then(|links| links.resolved_type)
            .is_some()
        {
            return Ok(());
        }
        let declaration = self.symbol(parameter)?.value_declaration();
        let optional = if let Some(declaration) = declaration {
            let read = self.ast(declaration)?.node(declaration)?;
            let name = read
                .name()
                .ok_or(Error::MissingLink("assigned parameter name"))?;
            let _ = name;
            read.initializer().is_none() && read.question_token(self.ast(declaration)?)?.is_some()
        } else {
            false
        };
        let ty = match contextual {
            Some(ty) => ty,
            None => {
                if let Some(declaration) = declaration {
                    let raw = self.type_for_variable_like_raw(declaration, true, 0)?;
                    self.widen_type_for_variable_like(declaration, raw, true)?
                } else {
                    self.get_type_of_symbol(parameter)?
                }
            }
        };
        let ty = self.add_type_optionality(ty, false, optional)?;
        self.value_symbol_links
            .get_or_default(parameter)
            .resolved_type = Some(ty);
        if let Some(declaration) = declaration {
            if let Some(name) = self.ast(declaration)?.node(declaration)?.name() {
                if self.is_binding_pattern(name)? {
                    let ty = if ty == self.builtins.unknown_type {
                        let ty = self.type_from_binding_pattern(name, false, false)?;
                        self.value_symbol_links
                            .get_or_default(parameter)
                            .resolved_type = Some(ty);
                        ty
                    } else {
                        ty
                    };
                    self.assign_binding_element_types(name, ty)?;
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkDeferredNodes
    pub(crate) fn finish_deferred_function_bodies(&mut self, source: NodeId) -> Result<(), Error> {
        let mut index = 0;
        loop {
            let node = self
                .body_checks
                .deferred
                .get(&source)
                .and_then(|queue| queue.nodes.get(index))
                .copied();
            let Some(node) = node else { break };
            let saved = self.current_node.replace(node);
            self.instantiation.count = 0;
            let result = if matches!(
                self.ast(node)?.node(node)?.kind().known(),
                Some(K::CallExpression | K::NewExpression | K::BinaryExpression)
            ) {
                self.resolve_untyped_call(node).map(|_| ())
            } else if self.ast(node)?.node(node)?.kind() == K::ObjectLiteralExpression {
                self.check_object_contextual_deprecations(node)
            } else if matches!(
                self.ast(node)?.node(node)?.kind().known(),
                Some(K::GetAccessor | K::SetAccessor)
            ) {
                self.check_class_accessor(node)
            } else if self.ast(node)?.node(node)?.kind() == K::TypeParameter {
                self.check_type_parameter_deferred(node)
            } else if self.ast(node)?.node(node)?.kind() == K::ClassExpression {
                self.check_class_expression_deferred(node)
            } else if matches!(
                self.ast(node)?.node(node)?.kind().known(),
                Some(K::AsExpression | K::TypeAssertionExpression)
            ) {
                self.check_assertion_deferred(node)
            } else {
                self.check_function_expression_body(node)
            };
            self.current_node = saved;
            result?;
            index += 1;
        }
        self.body_checks.deferred.remove(&source);
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkFunctionExpressionOrObjectLiteralMethodDeferred
    fn check_function_expression_body(&mut self, function: NodeId) -> Result<(), Error> {
        let read = self.ast(function)?.node(function)?;
        let annotation = read.type_node();
        let body = read.body();
        let return_type = annotation
            .map(|node| self.get_type_from_type_node(node))
            .transpose()?;
        self.check_function_return_paths(function, return_type)?;
        let Some(body) = body else { return Ok(()) };
        if annotation.is_none() {
            let signature = self.signature_from_declaration(function)?;
            self.return_type_of_signature(signature)?;
        }
        if self.ast(body)?.node(body)?.kind() == K::Block {
            self.check_source_element(body)
        } else {
            let ty = self.check_expression(body)?;
            if let Some(return_type) = return_type {
                let return_type = self.unwrap_body_return_type(function, return_type)?;
                self.check_body_return_expression(
                    function,
                    return_type,
                    body,
                    Some(body),
                    ty,
                    false,
                )?;
            }
            Ok(())
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkAllCodePathsInNonVoidFunctionReturnOrThrow
    pub(crate) fn check_function_return_paths(
        &mut self,
        function: NodeId,
        annotation: Option<TypeId>,
    ) -> Result<(), Error> {
        let annotation = annotation
            .map(|ty| self.unwrap_body_return_type(function, ty))
            .transpose()?;
        if let Some(ty) = annotation {
            if self.return_type_is_undefined_void_or_any(ty)? {
                return Ok(());
            }
        }
        let read = self.ast(function)?.node(function)?;
        let Some(body) = read.body() else {
            return Ok(());
        };
        if self.ast(body)?.node(body)?.kind() != K::Block
            || !self.function_has_implicit_return(function)?
        {
            return Ok(());
        }
        let read = self.ast(function)?.node(function)?;
        let explicit = read.flags() & nf::HAS_EXPLICIT_RETURN != 0;
        let error_node = read.type_node().unwrap_or(function);
        let diagnostic = if let Some(ty) = annotation {
            if self.types.flags(ty)? & tf::NEVER != 0 {
                Some(messages::A_function_returning_never_cannot_have_a_reachable_end_point)
            } else if !explicit {
                Some(messages::A_function_whose_declared_type_is_neither_undefined_void_nor_any_must_return_a_value)
            } else if self.options.strict_null_checks
                && !self.is_type_related_to(
                    self.builtins.undefined_type,
                    ty,
                    crate::RelationKind::Assignable,
                )?
            {
                Some(messages::Function_lacks_ending_return_statement_and_return_type_does_not_include_undefined)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(diagnostic) = diagnostic {
            return self
                .error_at(Some(error_node), diagnostic, vec![])
                .map(|_| ());
        }
        if self.program()?.host.options().no_implicit_returns == Tristate::TRUE {
            if annotation.is_none() {
                if !explicit {
                    return Ok(());
                }
                let signature = self.signature_from_declaration(function)?;
                let ty = self.return_type_of_signature(signature)?;
                let ty = self.unwrap_body_return_type(function, ty)?;
                if self.return_type_is_undefined_void_or_any(ty)? {
                    return Ok(());
                }
            }
            self.error_at(
                Some(error_node),
                messages::Not_all_code_paths_return_a_value,
                vec![],
            )?;
        }
        Ok(())
    }

    fn return_type_is_undefined_void_or_any(&mut self, ty: TypeId) -> Result<bool, Error> {
        Ok(self.maybe_type_of_kind(ty, tf::VOID)?
            || self.types.flags(ty)? & (tf::ANY | tf::UNDEFINED) != 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkReturnStatement
    pub(crate) fn check_return_statement(&mut self, node: NodeId) -> Result<(), Error> {
        let expression = self.ast(node)?.node(node)?.expression();
        let expr_type = expression
            .map(|node| self.check_expression_cached(node))
            .transpose()?
            .unwrap_or(self.builtins.undefined_type);
        if self.check_statement_ambient_context(node)? {
            return Ok(());
        }
        let Some(function) = self.containing_function_or_static_block(node)? else {
            self.grammar_error_first_token(
                node,
                messages::A_return_statement_can_only_be_used_within_a_function_body,
                vec![],
            )?;
            return Ok(());
        };
        if self.ast(function)?.node(function)?.kind() == K::ClassStaticBlockDeclaration {
            self.grammar_error_first_token(
                node,
                messages::A_return_statement_cannot_be_used_inside_a_class_static_block,
                vec![],
            )?;
            return Ok(());
        }
        let signature = self.signature_from_declaration(function)?;
        let return_type = self.return_type_of_signature(signature)?;
        let read = self.ast(function)?.node(function)?;
        let kind = read.kind();
        let annotated = self.return_type_from_annotation(function)?.is_some();
        if self.options.strict_null_checks
            || expression.is_some()
            || self.types.flags(return_type)? & tf::NEVER != 0
        {
            if kind == K::SetAccessor {
                if expression.is_some() {
                    self.error_at(Some(node), messages::Setters_cannot_return_a_value, vec![])?;
                }
            } else if kind == K::Constructor {
                if expression.is_some()
                    && !self.check_expression_related_with_elaboration(
                        expr_type,
                        return_type,
                        crate::RelationKind::Assignable,
                        Some(node),
                        expression,
                        None,
                    )?
                {
                    self.error_at(Some(node), messages::Return_type_of_constructor_signature_must_be_assignable_to_the_instance_type_of_the_class, vec![])?;
                }
            } else if annotated {
                let return_type = self.unwrap_body_return_type(function, return_type)?;
                self.check_body_return_expression(
                    function,
                    return_type,
                    node,
                    expression,
                    expr_type,
                    false,
                )?;
            }
        } else if kind != K::Constructor
            && self.program()?.host.options().no_implicit_returns == Tristate::TRUE
        {
            let returned = self.unwrap_body_return_type(function, return_type)?;
            if !self.return_type_is_undefined_void_or_any(returned)? {
                self.error_at(
                    Some(node),
                    messages::Not_all_code_paths_return_a_value,
                    vec![],
                )?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkReturnExpression
    fn check_body_return_expression(
        &mut self,
        function: NodeId,
        return_type: TypeId,
        node: NodeId,
        expression: Option<NodeId>,
        expression_type: TypeId,
        in_conditional: bool,
    ) -> Result<(), Error> {
        if let Some(expression) = expression {
            let mut unwrapped = expression;
            while self.ast(unwrapped)?.node(unwrapped)?.kind() == K::ParenthesizedExpression {
                unwrapped = self
                    .ast(unwrapped)?
                    .node(unwrapped)?
                    .expression()
                    .ok_or(Error::MissingLink("return parentheses"))?;
            }
            let read = self.ast(unwrapped)?.node(unwrapped)?;
            if let Some(conditional) = read.data_source().as_conditional_expression() {
                let yes = conditional
                    .when_true()
                    .ok_or(Error::MissingLink("conditional return true"))?;
                let no = conditional
                    .when_false()
                    .ok_or(Error::MissingLink("conditional return false"))?;
                let yes_type = self.check_expression(yes)?;
                self.check_body_return_expression(
                    function,
                    return_type,
                    node,
                    Some(yes),
                    yes_type,
                    true,
                )?;
                let no_type = self.check_expression(no)?;
                return self.check_body_return_expression(
                    function,
                    return_type,
                    node,
                    Some(no),
                    no_type,
                    true,
                );
            }
        }
        let (asynchronous, _) = self.body_function_flags(function)?;
        let expression_type = if asynchronous {
            self.awaited_type_no_alias_ex(expression_type, Some(node), Some(messages::The_return_type_of_an_async_function_must_either_be_a_valid_promise_or_must_not_contain_a_callable_then_member), &[])?.unwrap_or(self.builtins.error_type)
        } else {
            expression_type
        };
        let expression = expression
            .map(|expression| self.effective_expression_check_node(expression))
            .transpose()?;
        let location =
            if self.ast(node)?.node(node)?.kind() == K::ReturnStatement && !in_conditional {
                node
            } else {
                expression.unwrap_or(node)
            };
        self.check_expression_related_with_elaboration(
            expression_type,
            return_type,
            crate::RelationKind::Assignable,
            Some(location),
            expression,
            None,
        )?;
        Ok(())
    }

    pub(crate) fn containing_function_or_static_block(
        &self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let mut current = self.ast(node)?.node(node)?.parent();
        while let Some(node) = current {
            let read = self.ast(node)?.node(node)?;
            if ts_ast::utilities::is_function_like(Some(&read))
                || read.kind() == K::ClassStaticBlockDeclaration
            {
                return Ok(Some(node));
            }
            current = read.parent();
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkBlock
    pub(crate) fn check_block_statement(&mut self, block: NodeId) -> Result<(), Error> {
        if self.ast(block)?.node(block)?.kind() == K::Block {
            self.check_statement_ambient_context(block)?;
        }
        let view = self.ast(block)?;
        let statements: Vec<_> = view
            .node_slice(view.node(block)?.statements(view)?)?
            .iter()
            .flatten()
            .collect();
        let restore_flow = ts_ast::utilities::is_function_or_module_block(self.ast(block)?, block)?;
        let saved_disabled = self.flow.disabled;
        let result: Result<(), Error> = (|| {
            for statement in statements {
                self.check_source_element(statement)?;
            }
            Ok(())
        })();
        if restore_flow {
            self.flow.disabled = saved_disabled;
        }
        result?;
        if let Some(locals) = self
            .program()?
            .bound(block)?
            .node_binding(block)?
            .and_then(|binding| binding.locals)
        {
            if !self.table(locals)?.is_empty() {
                self.register_for_unused_identifiers_check(block)?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkIfStatement
    pub(crate) fn check_if_statement(&mut self, node: NodeId) -> Result<(), Error> {
        self.check_statement_ambient_context(node)?;
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_if_statement()
            .ok_or(Error::MissingLink("if statement"))?;
        let expression = data
            .expression()
            .ok_or(Error::MissingLink("if condition"))?;
        let then_statement = data.then_statement().ok_or(Error::MissingLink("if then"))?;
        let else_statement = data.else_statement();
        let ty = self.check_truthiness_expression(expression)?;
        self.check_known_truthy_guard(expression, ty, Some(then_statement))?;
        self.check_source_element(then_statement)?;
        if self.ast(then_statement)?.node(then_statement)?.kind() == K::EmptyStatement {
            self.error_at(
                Some(then_statement),
                messages::The_body_of_an_if_statement_cannot_be_the_empty_statement,
                vec![],
            )?;
        }
        if let Some(else_statement) = else_statement {
            self.check_source_element(else_statement)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkThrowStatement
    pub(crate) fn check_throw_statement(&mut self, node: NodeId) -> Result<(), Error> {
        let expression = self
            .ast(node)?
            .node(node)?
            .expression()
            .ok_or(Error::MissingLink("throw expression"))?;
        if !self.check_statement_ambient_context(node)? {
            let read = self.ast(expression)?.node(expression)?;
            if read.kind() == K::Identifier
                && self
                    .ast(expression)?
                    .node_text(expression)?
                    .as_bytes()
                    .is_empty()
            {
                let source =
                    ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
                        .ok_or(Error::MissingLink("throw source"))?;
                if self
                    .ast(source)?
                    .source_file(source)?
                    .diagnostics()
                    .is_empty()
                {
                    self.add_diagnostic(Diagnostic::new(
                        Some(source),
                        TextRange::new(i64::from(read.pos()), i64::from(read.pos())),
                        messages::Line_break_not_permitted_here,
                        vec![],
                    ))?;
                }
            }
        }
        self.check_expression(expression).map(|_| ())
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarStatementInAmbientContext
    pub(crate) fn check_statement_ambient_context(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.flags() & nf::AMBIENT == 0 {
            return Ok(false);
        }
        let parent = read
            .parent()
            .ok_or(Error::MissingLink("ambient statement parent"))?;
        let parent_read = self.ast(parent)?.node(parent)?;
        let (key, diagnostic) = if !self.body_checks.ambient_reported.contains_key(&node)
            && ts_ast::utilities::is_function_like(Some(&parent_read))
        {
            (
                node,
                messages::An_implementation_cannot_be_declared_in_ambient_contexts,
            )
        } else if matches!(
            parent_read.kind().known(),
            Some(K::Block | K::ModuleBlock | K::SourceFile)
        ) && !self.body_checks.ambient_reported.contains_key(&parent)
        {
            (
                parent,
                messages::Statements_are_not_allowed_in_ambient_contexts,
            )
        } else {
            return Ok(false);
        };
        let reported = self.grammar_error_first_token(node, diagnostic, vec![])?;
        if reported {
            self.body_checks.ambient_reported.insert(key, ());
        }
        Ok(reported)
    }

    pub(crate) fn check_function_name_collision_boundary(
        &mut self,
        function: NodeId,
    ) -> Result<(), Error> {
        self.check_collisions_for_declaration_name(function)
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkMethodDeclaration
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarMethod
    pub(crate) fn check_source_method_grammar(&mut self, node: NodeId) -> Result<(), Error> {
        if self.check_grammar_function_like(node)? || self.check_grammar_generator(node)? {
            return Ok(());
        }
        let read = self.ast(node)?.node(node)?;
        let name = read
            .name()
            .ok_or(Error::MissingLink("source method name"))?;
        let parent = read
            .parent()
            .ok_or(Error::MissingLink("source method parent"))?;
        let parent_kind = self.ast(parent)?.node(parent)?.kind();
        let ambient = read.flags() & nf::AMBIENT != 0;
        let no_body = read.body().is_none();
        let dynamic_error = match parent_kind.known() {
            Some(K::ClassDeclaration | K::ClassExpression) if ambient => Some(messages::A_computed_property_name_in_an_ambient_context_must_refer_to_an_expression_whose_type_is_a_literal_type_or_a_unique_symbol_type),
            Some(K::ClassDeclaration | K::ClassExpression) if no_body => Some(messages::A_computed_property_name_in_a_method_overload_must_refer_to_an_expression_whose_type_is_a_literal_type_or_a_unique_symbol_type),
            Some(K::InterfaceDeclaration) => Some(messages::A_computed_property_name_in_an_interface_must_refer_to_an_expression_whose_type_is_a_literal_type_or_a_unique_symbol_type),
            Some(K::TypeLiteral) => Some(messages::A_computed_property_name_in_a_type_literal_must_refer_to_an_expression_whose_type_is_a_literal_type_or_a_unique_symbol_type),
            _ => None,
        };
        if let Some(message) = dynamic_error {
            if self.check_grammar_invalid_dynamic_name(name, message)? {
                return Ok(());
            }
        }
        self.check_grammar_computed_property_name(name)?;
        let generator = self
            .ast(node)?
            .node(node)?
            .data_source()
            .as_method_declaration()
            .is_some_and(|data| data.asterisk_token().is_some());
        if generator
            && self.ast(name)?.node(name)?.kind() == K::Identifier
            && self.ast(name)?.node_text(name)?.as_bytes() == b"constructor"
        {
            self.error_at(
                Some(name),
                messages::Class_constructor_may_not_be_a_generator,
                vec![],
            )?;
        }
        Ok(())
    }
}
