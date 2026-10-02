//! The class transforms of `transform.go`.
use super::{
    transform::{
        Transformer, DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS,
        DECLARATION_EMIT_NODE_BUILDER_FLAGS, NIL,
    },
    util,
};
use tsr_ast::{
    modifier_flags as mf, node_flags as nf, FactoryMethods, JsString, NodeId, NodeListId,
    SyntaxKind as K,
};
use tsr_printer::emit_resolver::DeclarationEmitResolver;

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    /// `ast.IsInJSFile(node)`.
    pub fn is_in_js_file(&self, node: NodeId) -> bool {
        tsr_ast::utilities::is_in_js_file(Some(&self.node(node)))
    }
    fn class_type_parameters_and_heritage(
        &self,
        node: NodeId,
    ) -> (Option<NodeListId>, Option<NodeListId>) {
        let read = self.node(node);
        match read.kind().known() {
            Some(K::ClassDeclaration) => {
                let data = read.as_class_declaration().expect("class payload");
                (data.type_parameters(), data.heritage_clauses())
            }
            Some(K::ClassExpression) => {
                let data = read
                    .as_class_expression()
                    .expect("class expression payload");
                (data.type_parameters(), data.heritage_clauses())
            }
            _ => panic!("{NIL}"),
        }
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.buildClassMembers
    fn build_class_members(
        &mut self,
        class_node: NodeId,
        extra_members: Vec<NodeId>,
    ) -> Result<NodeListId, R::Error> {
        let ctor =
            tsr_ast::utilities_class::get_first_constructor_with_body(self.view(), class_node)?;
        let mut parameter_properties = Vec::new();
        if let Some(ctor) = ctor {
            let old_diag = self.tracker.selector.clone();
            for param in self.list_nodes(self.node(ctor).parameter_list()) {
                if !self.has_syntactic_modifier(param, mf::PARAMETER_PROPERTY_MODIFIER)?
                    || self.should_strip_internal(Some(param))?
                {
                    continue;
                }
                self.set_diagnostic_context_for_node(param)?;
                let name = self.node(param).name().expect(NIL);
                if self.kind(name) == K::Identifier {
                    let modifiers = self.ensure_modifiers(param)?;
                    let question_token = self.node(param).question_token(self.view())?;
                    let ty = self.ensure_type(param, false)?;
                    let initializer = self.ensure_no_initializer(param)?;
                    let updated = self.output.new_property_declaration(
                        modifiers,
                        Some(name),
                        question_token,
                        ty,
                        initializer,
                    );
                    self.preserve_js_doc(updated, param);
                    parameter_properties.push(updated);
                } else {
                    // Pattern - this is currently an error, but we emit declarations for it somewhat correctly
                    let elements = self.walk_binding_pattern(name, param)?;
                    parameter_properties.extend(elements);
                }
            }
            self.tracker.selector = old_diag;
        }

        // When the class has at least one private identifier, create a unique constant identifier to retain the nominal typing behavior
        // Prevents other classes with the same public members from being used in place of the current class
        let members = self.node(class_node).member_list();
        let private_identifier = if self.list_nodes(members).into_iter().any(|member| {
            self.node(member)
                .name()
                .is_some_and(|name| self.kind(name) == K::PrivateIdentifier)
        }) {
            let name = self
                .output
                .new_private_identifier(JsString::from_bytes(b"#private".as_slice()));
            Some(
                self.output
                    .new_property_declaration(None, Some(name), None, None, None),
            )
        } else {
            None
        };

        let late_indexes = self.resolver.create_late_bound_index_signatures(
            self.output,
            self.emit,
            class_node,
            self.enclosing_declaration,
            DECLARATION_EMIT_NODE_BUILDER_FLAGS,
            DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS,
            &mut self.tracker,
        )?;
        self.apply_tracker_reports()?;

        let mut member_nodes = Vec::new();
        member_nodes.extend(private_identifier);
        member_nodes.extend(late_indexes);
        member_nodes.extend(parameter_properties);
        member_nodes.extend(extra_members);
        let visit_result = self.visit_nodes(members)?;
        member_nodes.extend(self.list_nodes(visit_result));
        Ok(self.new_node_list(member_nodes))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformClassDeclaration
    pub fn transform_class_declaration(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        let previous_enclosing_declaration = self.enclosing_declaration;
        self.enclosing_declaration = input;
        self.tracker.error_name = self.node(input).name();
        self.tracker.push_error_fallback_node(Some(input));
        let result = self.transform_class_declaration_worker(input);
        self.tracker.pop_error_fallback_node();
        self.enclosing_declaration = previous_enclosing_declaration;
        result
    }
    fn transform_class_declaration_worker(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        let name = self.node(input).name();
        let (type_parameters, heritage_clauses) = self.class_type_parameters_and_heritage(input);
        let modifiers = self.ensure_modifiers(input)?;
        let type_parameters = self.ensure_type_params(input, type_parameters)?;

        // Collect this.x property assignments from constructors and static blocks in JS files
        let extra_members = if self.is_in_js_file(input) {
            self.collect_this_property_assignments(input)?
        } else {
            Vec::new()
        };

        let members = self.build_class_members(input, extra_members)?;

        let extends_clause = util::get_effective_base_type_node(self.view(), input)?;

        if let Some(extends_clause) = extends_clause {
            let expression = self.node(extends_clause).expression().expect(NIL);
            if !tsr_ast::is_entity_name_expression(self.view(), expression)?
                && self.kind(expression) != K::NullKeyword
            {
                self.report_inference_fallback(expression)?; // Add an isolated declarations error on this extends clause
                let mut old_id = b"default".to_vec();
                if let Some(name) = name {
                    if tsr_ast::node_is_present(Some(&self.node(name)))
                        && self.kind(name) == K::Identifier
                    {
                        let text = self.node_text(name)?;
                        if !text.is_empty() {
                            old_id = text.as_bytes().to_vec();
                        }
                    }
                }
                old_id.extend_from_slice(b"_base");
                let new_id = self.new_unique_name(&old_id);
                self.set_fixed_diagnostic_context(
                    tsr_diagnostics::X_extends_clause_of_exported_class_0_has_or_is_using_private_name_1,
                    extends_clause,
                    name,
                );

                let ty = self.resolver.create_type_of_expression(
                    self.output,
                    self.emit,
                    expression,
                    input,
                    DECLARATION_EMIT_NODE_BUILDER_FLAGS,
                    DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS,
                    &mut self.tracker,
                )?;
                self.apply_tracker_reports()?;
                let mods = self.declare_modifier_list(true);
                let statement =
                    self.new_single_variable_statement(mods, new_id, ty, None, nf::CONST);
                let parent = self.parent(extends_clause).expect(NIL);
                let token = self
                    .node(parent)
                    .as_heritage_clause()
                    .expect("heritage clause payload")
                    .token();
                let type_arguments = self.node(extends_clause).type_argument_list();
                let type_arguments = self.visit_nodes(type_arguments)?;
                let element = self.output.update_expression_with_type_arguments(
                    extends_clause,
                    Some(new_id),
                    type_arguments,
                );
                let types = self.new_node_list(vec![element]);
                let new_heritage_clause =
                    self.output
                        .update_heritage_clause(parent, token, Some(types));
                let retained_heritage_clauses = self.visit_nodes(heritage_clauses)?; // should just be `implements`
                let mut heritage_list = vec![new_heritage_clause];
                heritage_list.extend(self.list_nodes(retained_heritage_clauses));
                let heritage_clauses = self.new_node_list(heritage_list);

                let class = self.output.update_class_declaration(
                    input,
                    modifiers,
                    name,
                    type_parameters,
                    Some(heritage_clauses),
                    Some(members),
                );
                return Ok(self.new_syntax_list(vec![statement, class]));
            }
        }

        let heritage_clauses = self.visit_nodes(heritage_clauses)?;
        Ok(self.output.update_class_declaration(
            input,
            modifiers,
            name,
            type_parameters,
            heritage_clauses,
            Some(members),
        ))
    }

    // transformClassExpressionToDeclaration converts a class expression into a class declaration
    // for use in CJS export declarations (e.g., exports.K = class K {} or module.exports = class Thing {}).
    // This delegates to the shared buildClassMembers helper to stay in sync with transformClassDeclaration.
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformClassExpressionToDeclaration
    pub fn transform_class_expression_to_declaration(
        &mut self,
        class_expr: NodeId,
        class_name: NodeId,
        modifiers: Option<NodeListId>,
    ) -> Result<NodeId, R::Error> {
        let previous_enclosing_declaration = self.enclosing_declaration;
        self.enclosing_declaration = class_expr;
        let previous_in_class_expression_declaration = self.in_class_expression_declaration;
        self.in_class_expression_declaration = true;
        let result = self
            .transform_class_expression_to_declaration_worker(class_expr, class_name, modifiers);
        self.enclosing_declaration = previous_enclosing_declaration;
        self.in_class_expression_declaration = previous_in_class_expression_declaration;
        result
    }
    fn transform_class_expression_to_declaration_worker(
        &mut self,
        class_expr: NodeId,
        class_name: NodeId,
        modifiers: Option<NodeListId>,
    ) -> Result<NodeId, R::Error> {
        let extra_members = if self.is_in_js_file(class_expr) {
            self.collect_this_property_assignments(class_expr)?
        } else {
            Vec::new()
        };
        let members = self.build_class_members(class_expr, extra_members)?;
        let (type_parameters, heritage_clauses) =
            self.class_type_parameters_and_heritage(class_expr);
        let type_parameters = self.ensure_type_params(class_expr, type_parameters)?;
        let heritage_clauses = self.visit_nodes(heritage_clauses)?;
        Ok(self.output.new_class_declaration(
            modifiers,
            Some(class_name),
            type_parameters,
            heritage_clauses,
            Some(members),
        ))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.walkBindingPattern
    fn walk_binding_pattern(
        &mut self,
        pattern: NodeId,
        param: NodeId,
    ) -> Result<Vec<NodeId>, R::Error> {
        let mut elems = Vec::new();
        for elem in self.list_nodes(self.node(pattern).element_list()) {
            if self.kind(elem) == K::OmittedExpression {
                continue;
            }
            let name = self.node(elem).name();
            if let Some(name) = name.filter(|name| self.is_binding_pattern(*name)) {
                elems.extend(self.walk_binding_pattern(name, param)?);
                continue;
            }
            let modifiers = self.ensure_modifiers(param)?;
            let ty = self.ensure_type(elem, false)?;
            elems.push(self.output.new_property_declaration(
                modifiers, name, None, /*questionOrExclamationToken*/
                ty, None, /*initializer*/
            ));
        }
        Ok(elems)
    }
}
