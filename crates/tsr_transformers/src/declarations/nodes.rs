//! `visitDeclarationSubtree` and the `ensure*` helpers of `transform.go`
//! that every member and signature transform shares.
use super::{
    tracker::Selector,
    transform::{
        Transformer, DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS,
        DECLARATION_EMIT_NODE_BUILDER_FLAGS, NIL,
    },
    util,
};
use tsr_ast::{
    modifier_flags as mf, FactoryMethods, NodeId, NodeListId, RuntimeFactory, SyntaxKind as K,
};
use tsr_core::debug::{assert_never, Argument, Member};
use tsr_printer::emit_resolver::DeclarationEmitResolver;

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visitDeclarationSubtree
    pub fn visit_declaration_subtree(&mut self, input: NodeId) -> Result<Option<NodeId>, R::Error> {
        if self.should_strip_internal(Some(input))? {
            return Ok(None);
        }
        if tsr_ast::is_declaration(&self.node(input)) {
            if util::is_declaration_and_not_visible(self, input)? {
                return Ok(None);
            }
            if tsr_ast::has_dynamic_name(self.view(), Some(input))? {
                let name_expression = self.node(self.node(input).name().expect(NIL)).expression();
                if self.isolated_declarations {
                    // Classes and object literals usually elide properties with computed names that are not of a literal type
                    // In isolated declarations TSC needs to error on these as we don't know the type in a DTE.
                    if !self
                        .resolver
                        .definitely_reference_to_global_symbol_object(name_expression.expect(NIL))?
                    {
                        let parent_kind = self.parent_kind(input);
                        if matches!(
                            parent_kind,
                            K::ClassDeclaration | K::ObjectLiteralExpression
                        ) {
                            self.add_diagnostic_for_node(
                                input,
                                tsr_diagnostics::Computed_property_names_on_class_or_object_literals_cannot_be_inferred_with_isolatedDeclarations,
                                vec![],
                            )?;
                            return Ok(None);
                        } else if matches!(parent_kind, K::InterfaceDeclaration | K::TypeLiteral)
                            && !tsr_ast::is_entity_name_expression(
                                self.view(),
                                name_expression.expect(NIL),
                            )?
                        {
                            // Type declarations just need to double-check that the input computed name is an entity name expression
                            self.add_diagnostic_for_node(
                                input,
                                tsr_diagnostics::Computed_properties_must_be_number_or_string_literals_variables_or_dotted_expressions_with_isolatedDeclarations,
                                vec![],
                            )?;
                            return Ok(None);
                        }
                    }
                } else {
                    let parse_node = self.parse_node(input).expect(NIL);
                    if !self.resolver.late_bound(parse_node)?
                        || !tsr_ast::is_entity_name_expression(
                            self.view(),
                            name_expression.expect(NIL),
                        )?
                    {
                        return Ok(None);
                    }
                }
            }
        }

        // Elide implementation signatures from overload sets
        if tsr_ast::utilities::is_function_like(Some(&self.node(input)))
            && self.resolver.implementation_of_overload(input)?
        {
            return Ok(None);
        }

        if self.kind(input) == K::SemicolonClassElement {
            return Ok(None);
        }

        if self.kind(input) == K::HeritageClause {
            let types = self.list_nodes(
                self.node(input)
                    .as_heritage_clause()
                    .expect("heritage clause payload")
                    .types(),
            );
            if types.is_empty()
                || (types.len() == 1 && tsr_ast::node_is_missing(Some(&self.node(types[0]))))
            {
                return Ok(None);
            }
        }

        let previous_enclosing_declaration = self.enclosing_declaration;
        if util::is_enclosing_declaration(&self.node(input)) {
            self.enclosing_declaration = input;
        }

        let (can_produce_diagnostic, cleanup_diagnostic_context) =
            self.setup_diagnostic_context(input)?;

        let result = match self.kind(input) {
            K::MappedType => self.transform_mapped_type_node(input).map(Some),
            K::HeritageClause => self.transform_heritage_clause(input),
            K::MethodSignature => self.transform_method_signature_declaration(input),
            K::MethodDeclaration => self.transform_method_declaration(input),
            K::ConstructSignature => self
                .transform_construct_signature_declaration(input)
                .map(Some),
            K::Constructor => self.transform_constructor_declaration(input).map(Some),
            K::GetAccessor => self.transform_get_accesor_declaration(input),
            K::SetAccessor => self.transform_set_accessor_declaration(input),
            K::PropertyDeclaration => self.transform_property_declaration(input),
            K::PropertySignature => self.transform_property_signature_declaration(input),
            K::CallSignature => self.transform_call_signature_declaration(input).map(Some),
            K::IndexSignature => self.transform_index_signature_declaration(input).map(Some),
            K::VariableDeclaration => self.transform_variable_declaration(input),
            K::TypeParameter => self.transform_type_parameter_declaration(input).map(Some),
            K::ExpressionWithTypeArguments => self
                .transform_expression_with_type_arguments(input)
                .map(Some),
            K::TypeReference => self.transform_type_reference(input).map(Some),
            K::ConditionalType => self.transform_conditional_type_node(input).map(Some),
            K::FunctionType => self.transform_function_type_node(input).map(Some),
            K::ConstructorType => self.transform_constructor_type_node(input).map(Some),
            K::ImportType => self.transform_import_type_node(input).map(Some),
            K::TypeQuery => {
                let expr_name = self
                    .node(input)
                    .as_type_query_node()
                    .expect("type query payload")
                    .expr_name()
                    .expect(NIL);
                self.check_entity_name_visibility(expr_name, self.enclosing_declaration)?;
                self.visit_each_child(input).map(Some)
            }
            K::QualifiedName => {
                let right = self
                    .node(input)
                    .as_qualified_name()
                    .expect("qualified name payload")
                    .right()
                    .expect(NIL);
                if self.kind(right) == K::PrivateIdentifier {
                    let text = self.node_text(right)?;
                    self.add_diagnostic_for_node(
                        input,
                        tsr_diagnostics::Declaration_emit_elides_private_members_but_0_refers_to_a_private_member_Write_an_explicit_type_here,
                        vec![text],
                    )?;
                }
                self.visit_each_child(input).map(Some)
            }
            K::TupleType => {
                let result = self.visit_each_child(input)?;
                let single_line = match crate::utilities::is_original_node_single_line(
                    self.emit,
                    &*self.output,
                    Some(input),
                ) {
                    Ok(single_line) => single_line,
                    Err(crate::Error::Arena(error)) => return Err(error.into()),
                    Err(error) => panic!("IsOriginalNodeSingleLine reads only storage: {error:?}"),
                };
                if single_line {
                    self.emit
                        .add_emit_flags(result, tsr_printer::emit_flags::SINGLE_LINE);
                }
                Ok(Some(result))
            }
            K::JSDocTypeExpression => self.transform_js_doc_type_expression(input),
            K::JSDocTypeLiteral => self.transform_js_doc_type_literal(input).map(Some),
            K::JSDocPropertyTag => self.transform_js_doc_property_tag(input).map(Some),
            K::JSDocAllType => Ok(Some(self.transform_js_doc_all_type(input))),
            K::JSDocNullableType => self.transform_js_doc_nullable_type(input).map(Some),
            K::JSDocNonNullableType => self.transform_js_doc_non_nullable_type(input),
            K::JSDocOptionalType => self.transform_js_doc_optional_type(input).map(Some),
            K::JSDocVariadicType => self.transform_js_doc_variadic_type(input).map(Some),
            _ => self.visit_each_child(input).map(Some),
        }?;

        if result.is_some()
            && can_produce_diagnostic
            && tsr_ast::has_dynamic_name(self.view(), Some(input))?
        {
            self.check_name(input)?;
        }

        self.enclosing_declaration = previous_enclosing_declaration;
        self.cleanup_diagnostic_context(cleanup_diagnostic_context);
        Ok(result)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.checkName
    pub fn check_name(&mut self, node: NodeId) -> Result<(), R::Error> {
        let old_diag = self.tracker.selector.clone();
        if !self.suppress_new_diagnostic_contexts {
            self.set_diagnostic_context_for_node_name(node)?;
        }
        self.tracker.error_name = self.node(node).name();
        tsr_core::debug::assert(tsr_ast::has_dynamic_name(self.view(), Some(node))?, &[]); // Should only be called with dynamic names
        let entity_name = self
            .node(self.node(node).name().expect(NIL))
            .expression()
            .expect(NIL);
        self.check_entity_name_visibility(entity_name, self.enclosing_declaration)?;
        if !self.suppress_new_diagnostic_contexts {
            self.tracker.selector = old_diag;
        }
        self.tracker.error_name = None;
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.ensureType
    pub fn ensure_type(
        &mut self,
        node: NodeId,
        ignore_private: bool,
    ) -> Result<Option<NodeId>, R::Error> {
        if !ignore_private && self.effective_declaration_flags(node, mf::PRIVATE)? != 0 {
            // Private nodes emit no types (except private parameter properties, whose parameter types are actually visible)
            return Ok(None);
        }

        if self.should_print_with_initializer(node)? {
            // Literal const declarations will have an initializer ensured rather than a type
            return Ok(None);
        }

        // Should be removed createTypeOfDeclaration will actually now reuse the existing annotation so there is no real need to duplicate type walking
        // Left in for now to minimize diff during syntactic type node builder refactor
        let kind = self.kind(node);
        if kind != K::ExportAssignment && kind != K::BindingElement {
            if let Some(ty) = self.node(node).type_node() {
                if kind != K::Parameter
                    || !self.resolver.requires_adding_implicit_undefined(
                        node,
                        None,
                        Some(self.enclosing_declaration),
                    )?
                {
                    if self.is_source_file_js()? {
                        // JS types have a heap of constructs we can't directly emit into .d.ts files; the node builder contains logic to remap those where possible, so we invoke it here
                        // In strada we always built js declarations symbolically, so all js type nodes went through this postprocessing
                        let js_flags = self.builder_flags();
                        let res = self.resolver.try_js_type_node_to_type_node(
                            self.output,
                            self.emit,
                            ty,
                            self.enclosing_declaration,
                            js_flags,
                            DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS,
                            &mut self.tracker,
                        )?;
                        self.apply_tracker_reports()?;
                        if res.is_some() {
                            return Ok(res);
                        }
                        // otherwise, fall back to full serialization
                    } else {
                        return self.visit(Some(ty));
                    }
                }
            }
        }

        let old_error_name_node = self.tracker.error_name;
        self.tracker.error_name = self.node(node).name();
        let mut old_diag = None;
        if !self.suppress_new_diagnostic_contexts {
            old_diag = Some(self.tracker.selector.clone());
            if util::can_produce_diagnostics(&self.node(node)) {
                self.set_diagnostic_context_for_node(node)?;
            }
        }

        let flags = self.builder_flags();
        let type_node = if tsr_ast::utilities_tail::has_inferred_type(&self.node(node)) {
            self.resolver.create_type_of_declaration(
                self.output,
                self.emit,
                node,
                self.enclosing_declaration,
                flags,
                DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS,
                &mut self.tracker,
            )?
        } else if tsr_ast::utilities::is_function_like(Some(&self.node(node))) {
            self.resolver.create_return_type_of_signature(
                self.output,
                self.emit,
                node,
                self.enclosing_declaration,
                flags,
                DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS,
                &mut self.tracker,
            )?
        } else {
            let kind_string = self.node(node).kind_string();
            assert_never(Member::with_kind_string(Argument::Nil, &kind_string), &[]);
        };
        self.apply_tracker_reports()?;

        self.tracker.error_name = old_error_name_node;
        if !self.suppress_new_diagnostic_contexts {
            self.tracker.selector = old_diag.unwrap_or_else(Selector::throw);
        }
        Ok(Some(type_node.unwrap_or_else(|| {
            self.output.new_keyword_type_node(K::AnyKeyword.into())
        })))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.shouldPrintWithInitializer
    pub fn should_print_with_initializer(&mut self, node: NodeId) -> Result<bool, R::Error> {
        Ok(util::can_have_literal_initializer(self, node)?
            && self.node(node).initializer().is_some()
            && {
                let original = self.most_original(node);
                self.resolver.literal_const_declaration(original)?
            })
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.ensureTypeParams
    pub fn ensure_type_params(
        &mut self,
        node: NodeId,
        params: Option<NodeListId>,
    ) -> Result<Option<NodeListId>, R::Error> {
        if self.effective_declaration_flags(node, mf::PRIVATE)? != 0 {
            return Ok(None);
        }
        let mut type_parameters = self.visit_nodes(params)?;
        if type_parameters.is_some() {
            return Ok(type_parameters);
        }
        let old_error_name_node = self.tracker.error_name;
        self.tracker.error_name = self.node(node).name();
        let mut old_diag = None;
        if !self.suppress_new_diagnostic_contexts {
            old_diag = Some(self.tracker.selector.clone());
            if util::can_produce_diagnostics(&self.node(node)) {
                self.set_diagnostic_context_for_node(node)?;
            }
        }

        if self.function_like_full_signature(node).flatten().is_some() {
            let nodes = self.resolver.create_type_parameters_of_signature(
                self.output,
                self.emit,
                node,
                self.enclosing_declaration,
                DECLARATION_EMIT_NODE_BUILDER_FLAGS,
                DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS,
                &mut self.tracker,
            )?;
            self.apply_tracker_reports()?;
            if !nodes.is_empty() {
                let loc = self.node(node).range();
                let list = self.new_node_list(nodes);
                self.output.set_list_location(list, loc)?;
                type_parameters = Some(list);
            }
        }

        self.tracker.error_name = old_error_name_node;
        if !self.suppress_new_diagnostic_contexts {
            self.tracker.selector = old_diag.unwrap_or_else(Selector::throw);
        }
        Ok(type_parameters)
    }

    /// `node.FunctionLikeData()`'s `FullSignature`: `None` when the node is
    /// not function-like (its function-like data is nil).
    #[allow(
        clippy::option_option,
        reason = "Go distinguishes a nil FunctionLikeData from a nil FullSignature"
    )]
    pub fn function_like_full_signature(&self, node: NodeId) -> Option<Option<NodeId>> {
        let read = self.node(node);
        Some(match read.kind().known()? {
            K::FunctionDeclaration => read.as_function_declaration()?.full_signature(),
            K::CallSignature => read.as_call_signature_declaration()?.full_signature(),
            K::ConstructSignature => read.as_construct_signature_declaration()?.full_signature(),
            K::Constructor => read.as_constructor_declaration()?.full_signature(),
            K::GetAccessor => read.as_get_accessor_declaration()?.full_signature(),
            K::SetAccessor => read.as_set_accessor_declaration()?.full_signature(),
            K::IndexSignature => read.as_index_signature_declaration()?.full_signature(),
            K::MethodSignature => read.as_method_signature_declaration()?.full_signature(),
            K::MethodDeclaration => read.as_method_declaration()?.full_signature(),
            K::ArrowFunction => read.as_arrow_function()?.full_signature(),
            K::FunctionExpression => read.as_function_expression()?.full_signature(),
            K::FunctionType => read.as_function_type_node()?.full_signature(),
            K::ConstructorType => read.as_constructor_type_node()?.full_signature(),
            K::JSDocSignature => read.as_js_doc_signature()?.full_signature(),
            _ => return None,
        })
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.updateParamList
    pub fn update_param_list(
        &mut self,
        node: NodeId,
        params: Option<NodeListId>,
    ) -> Result<NodeListId, R::Error> {
        let nodes = self.list_nodes(Some(params.expect(NIL)));
        if self.effective_declaration_flags(node, mf::PRIVATE)? != 0 || nodes.is_empty() {
            return Ok(self.new_node_list(Vec::new()));
        }
        let mut results = Vec::with_capacity(nodes.len());
        for p in nodes {
            results.push(self.ensure_parameter(p)?);
        }
        Ok(self.new_node_list(results))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.ensureParameter
    pub fn ensure_parameter(&mut self, p: NodeId) -> Result<NodeId, R::Error> {
        let old_diag = self.tracker.selector.clone();
        if !self.suppress_new_diagnostic_contexts {
            self.set_diagnostic_context_for_node(p)?;
        }
        let (dot_dot_dot_token, name, question) = {
            let read = self.node(p);
            let data = read
                .as_parameter_declaration()
                .expect("parameter declaration payload");
            (data.dot_dot_dot_token(), data.name(), data.question_token())
        };
        let question_token = if self.resolver.optional_parameter(p)? {
            Some(question.unwrap_or_else(|| self.output.new_token(K::QuestionToken.into())))
        } else {
            None
        };
        let name = self.visit_binding_name_node(name)?;
        let ty = self.ensure_type(p, true)?;
        let initializer = self.ensure_no_initializer(p)?;
        let result = self.output.update_parameter_declaration(
            p,
            None,
            dot_dot_dot_token,
            name,
            question_token,
            ty,
            initializer,
        );
        self.tracker.selector = old_diag;
        Ok(result)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.ensureNoInitializer
    pub fn ensure_no_initializer(&mut self, node: NodeId) -> Result<Option<NodeId>, R::Error> {
        if self.should_print_with_initializer(node)? {
            let unwrapped_initializer = util::unwrap_parenthesized_expression(
                self.view(),
                self.node(node).initializer().expect(NIL),
            )?;
            if !tsr_ast::utilities_tail::is_primitive_literal_value(
                self.view(),
                &self.node(unwrapped_initializer),
                true,
            )? {
                self.report_inference_fallback(node)?;
            }
            let parse_node = self.parse_node(node).expect(NIL);
            let result = self.resolver.create_literal_const_value(
                self.output,
                self.emit,
                parse_node,
                &mut self.tracker,
            )?;
            self.apply_tracker_reports()?;
            return Ok(result);
        }
        Ok(None)
    }

    /// `bindingNameVisitor.VisitNode(name)`.
    pub fn visit_binding_name_node(
        &mut self,
        node: Option<NodeId>,
    ) -> Result<Option<NodeId>, R::Error> {
        let Some(node) = node else { return Ok(None) };
        let visited = self.visit_binding_name(node)?;
        // The binding name visitor never returns a syntax list.
        Ok(Some(visited))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visitBindingName
    fn visit_binding_name(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        match self.kind(node) {
            K::ArrayBindingPattern | K::ObjectBindingPattern => {
                // node.VisitEachChild(tx.bindingNameVisitor)
                let elements = self.node(node).element_list();
                let nodes = self.list_nodes(elements);
                let mut visited = Vec::with_capacity(nodes.len());
                let mut changed = false;
                for element in nodes {
                    let result = self.visit_binding_name(element)?;
                    changed |= result != element;
                    visited.push(result);
                }
                let elements = if changed {
                    let original = elements.expect(NIL);
                    let list = self.new_node_list(visited);
                    let loc = self.output.read_list(original).loc();
                    self.output.set_list_location(list, loc)?;
                    Some(list)
                } else {
                    elements
                };
                Ok(self.output.update_binding_pattern(node, elements))
            }
            K::BindingElement => {
                let (dot_dot_dot_token, property_name, name) = {
                    let read = self.node(node);
                    let data = read.as_binding_element().expect("binding element payload");
                    (data.dot_dot_dot_token(), data.property_name(), data.name())
                };
                if let Some(property_name) = property_name {
                    if self.kind(property_name) == K::ComputedPropertyName {
                        let expression = self.node(property_name).expression().expect(NIL);
                        if tsr_ast::is_entity_name_expression(self.view(), expression)? {
                            self.check_entity_name_visibility(
                                expression,
                                self.enclosing_declaration,
                            )?;
                        }
                    }
                }
                let name = self.visit_binding_name_node(name)?;
                Ok(self.output.update_binding_element(
                    node,
                    dot_dot_dot_token,
                    property_name,
                    name,
                    None, /*initializer*/
                ))
            }
            // KindIdentifier, KindOmittedExpression and every other kind
            _ => Ok(node),
        }
    }
}
