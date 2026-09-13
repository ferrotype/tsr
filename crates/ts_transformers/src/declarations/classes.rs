use super::transform::{Transformer, BUILDER_FLAGS, INTERNAL_FLAGS};
use ts_ast::{
    modifier_flags as mf, Factory, FactoryMethods, JsString, NodeId, NodeListId, SyntaxKind as K,
};
use ts_printer::emit_resolver::DeclarationEmitResolver;

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformClassDeclaration
    pub fn class_declaration(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        let old_enclosing = self.enclosing;
        self.enclosing = node;
        self.tracker.error_name = self.node(node).name();
        self.tracker.fallback.push(Some(node));
        let result = self.class_worker(node);
        self.tracker.fallback.pop();
        self.enclosing = old_enclosing;
        result
    }
    fn class_worker(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        let data = self
            .node(node)
            .data_source()
            .as_class_declaration()
            .unwrap()
            .to_owned();
        let modifiers = self.modifiers(node)?;
        let parameters = self.type_parameters(node)?;
        let extra = if self.output.read_source_file(self.source)?.is_js() {
            self.collect_this_property_assignments(node)?
        } else {
            Vec::new()
        };
        let members = self.class_members(node, extra)?;
        for clause in self.list_nodes(data.heritage_clauses) {
            let clause_data = self
                .node(clause)
                .data_source()
                .as_heritage_clause()
                .unwrap()
                .to_owned();
            if clause_data.token != K::ExtendsKeyword {
                continue;
            }
            let Some(base) = self.list_nodes(clause_data.types).first().copied() else {
                continue;
            };
            if self.node(base).kind() != K::ExpressionWithTypeArguments {
                continue;
            }
            let expression = Self::required(self.node(base).expression())?;
            if ts_ast::is_entity_name_expression(self.output.view(), expression)?
                || self.node(expression).kind() == K::NullKeyword
            {
                continue;
            }
            self.inference_fallback(expression)?;
            let mut name = data
                .name
                .filter(|name| {
                    self.node(*name).kind() == K::Identifier
                        && !self.node(*name).as_identifier().unwrap().text().is_empty()
                })
                .map(|name| self.node(name).as_identifier().unwrap().text().to_vec())
                .unwrap_or_else(|| b"default".to_vec());
            name.extend_from_slice(b"_base");
            let new_id = self.unique_name(JsString::from_bytes(name));
            self.tracker.selector = super::tracker::Selector::fixed(super::diagnostics::SymbolAccessibilityDiagnostic {
                diagnostic_message: ts_diagnostics::X_extends_clause_of_exported_class_0_has_or_is_using_private_name_1,
                error_node: Some(base), type_name: data.name,
            });
            let ty = self.resolver.create_type_of_expression(
                self.output,
                self.emit,
                expression,
                node,
                BUILDER_FLAGS,
                INTERNAL_FLAGS,
                &mut self.tracker,
            )?;
            self.flush_reports()?;
            let statement = self.const_variable(new_id, ty, None);
            let arguments = self.visit_list_result(self.node(base).type_argument_list())?;
            let base =
                self.output
                    .update_expression_with_type_arguments(base, Some(new_id), arguments);
            let bases = self.new_list(vec![base]);
            let clause =
                self.output
                    .update_heritage_clause(clause, K::ExtendsKeyword.into(), Some(bases));
            let retained = self.visit_list_result(data.heritage_clauses)?;
            let mut clauses = vec![clause];
            clauses.extend(self.list_nodes(retained));
            let heritage = self.new_list(clauses);
            let class = self.output.update_class_declaration(
                node,
                modifiers,
                data.name,
                parameters,
                Some(heritage),
                Some(members),
            );
            return Ok(self.syntax_list(vec![statement, class]));
        }
        let heritage = self.visit_list_result(data.heritage_clauses)?;
        Ok(self.output.update_class_declaration(
            node,
            modifiers,
            data.name,
            parameters,
            heritage,
            Some(members),
        ))
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformClassExpressionToDeclaration
    pub fn class_expression_declaration(
        &mut self,
        node: NodeId,
        name: NodeId,
        modifiers: Option<ts_ast::NodeListId>,
    ) -> Result<NodeId, R::Error> {
        let enclosing = self.enclosing;
        self.enclosing = node;
        let in_class = self.in_class_expression_declaration;
        self.in_class_expression_declaration = true;
        let result: Result<NodeId, R::Error> = (|| {
            let extra = if self.output.read_source_file(self.source)?.is_js() {
                self.collect_this_property_assignments(node)?
            } else {
                Vec::new()
            };
            let data = self
                .node(node)
                .data_source()
                .as_class_expression()
                .unwrap()
                .to_owned();
            let members = self.class_members(node, extra)?;
            let parameters = self.type_parameters(node)?;
            let heritage = self.visit_list_result(data.heritage_clauses)?;
            Ok(self.output.new_class_declaration(
                modifiers,
                Some(name),
                parameters,
                heritage,
                Some(members),
            ))
        })();
        self.enclosing = enclosing;
        self.in_class_expression_declaration = in_class;
        result
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.buildClassMembers
    fn class_members(&mut self, node: NodeId, extra: Vec<NodeId>) -> Result<NodeListId, R::Error> {
        let members = self.node(node).member_list();
        let original = self.list_nodes(members);
        let constructor = original.iter().copied().find(|member| {
            self.node(*member).kind() == K::Constructor && self.node(*member).body().is_some()
        });
        let mut properties = Vec::new();
        if let Some(constructor) = constructor {
            let old = self.tracker.selector.clone();
            for parameter in self.list_nodes(self.node(constructor).parameter_list()) {
                if ts_ast::utilities::get_combined_modifier_flags(self.output.view(), parameter)?
                    & mf::PARAMETER_PROPERTY_MODIFIER
                    == 0
                    || self.strip_internal(parameter)?
                {
                    continue;
                }
                self.select_context(parameter, false)?;
                let name = Self::required(self.node(parameter).name())?;
                if self.node(name).kind() == K::Identifier {
                    let modifiers = self.modifiers(parameter)?;
                    let question = self.node(parameter).question_token(self.output.view())?;
                    let ty = self.ensure_type(parameter, false)?;
                    let initializer = self.ensure_initializer(parameter)?;
                    properties.push(self.output.new_property_declaration(
                        modifiers,
                        Some(name),
                        question,
                        ty,
                        initializer,
                    ));
                } else {
                    self.parameter_properties(name, parameter, &mut properties)?;
                }
            }
            self.tracker.selector = old;
        }
        let mut result = Vec::new();
        if original.iter().any(|member| {
            self.node(*member)
                .name()
                .is_some_and(|name| self.node(name).kind() == K::PrivateIdentifier)
        }) {
            let name = self
                .output
                .new_private_identifier(JsString::from_bytes(b"#private".as_slice()));
            result.push(
                self.output
                    .new_property_declaration(None, Some(name), None, None, None),
            );
        }
        let indexes = self.resolver.create_late_bound_index_signatures(
            self.output,
            self.emit,
            node,
            self.enclosing,
            BUILDER_FLAGS,
            INTERNAL_FLAGS,
            &mut self.tracker,
        )?;
        self.flush_reports()?;
        result.extend(indexes);
        result.extend(properties);
        result.extend(extra);
        let visited = self.visit_list_result(members)?;
        result.extend(self.list_nodes(visited));
        Ok(self.new_list(result))
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.walkBindingPattern
    fn parameter_properties(
        &mut self,
        pattern: NodeId,
        parameter: NodeId,
        out: &mut Vec<NodeId>,
    ) -> Result<(), R::Error> {
        for element in self.list_nodes(self.node(pattern).element_list()) {
            if self.node(element).kind() == K::OmittedExpression {
                continue;
            }
            let Some(name) = self.node(element).name() else {
                continue;
            };
            if matches!(
                self.node(name).kind().known(),
                Some(K::ArrayBindingPattern | K::ObjectBindingPattern)
            ) {
                self.parameter_properties(name, parameter, out)?;
            } else {
                let modifiers = self.modifiers(parameter)?;
                let ty = self.ensure_type(element, false)?;
                out.push(self.output.new_property_declaration(
                    modifiers,
                    Some(name),
                    None,
                    ty,
                    None,
                ));
            }
        }
        Ok(())
    }
}
