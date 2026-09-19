use super::{transform::Transformer, util};
use tsr_ast::{
    node_flags as nf, Factory, FactoryMethods, JsString, NodeId, RuntimeFactory, SyntaxKind as K,
};
use tsr_printer::{
    emit_resolver::DeclarationEmitResolver, generated_identifier_flags as gif, AutoGenerateOptions,
};

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    pub fn unique_name(&mut self, text: JsString) -> NodeId {
        self.emit.new_unique_name_ex(
            self.output,
            text,
            AutoGenerateOptions {
                flags: gif::OPTIMISTIC,
                ..Default::default()
            },
        )
    }
    pub fn syntax_list(&mut self, statements: Vec<NodeId>) -> NodeId {
        let nodes = self
            .output
            .alloc_nodes(statements.into_iter().map(Some).collect());
        self.output.new_syntax_list(nodes)
    }
    #[allow(
        clippy::unnecessary_wraps,
        reason = "Returns the optional modifier-list slot expected by all factory declaration constructors, including an allocated empty list"
    )]
    pub fn declare_modifiers(&mut self) -> Option<tsr_ast::NodeListId> {
        let modifiers = if self.needs_declare {
            vec![Some(self.output.new_token(K::DeclareKeyword.into()))]
        } else {
            Vec::new()
        };
        let nodes = self.output.alloc_nodes(modifiers);
        Some(self.output.new_modifier_list(nodes))
    }
    pub fn const_variable(
        &mut self,
        name: NodeId,
        ty: Option<NodeId>,
        initializer: Option<NodeId>,
    ) -> NodeId {
        let declaration = self
            .output
            .new_variable_declaration(Some(name), None, ty, initializer);
        let declarations = self.new_list(vec![declaration]);
        let list = self
            .output
            .new_variable_declaration_list(Some(declarations), nf::CONST);
        let modifiers = self.declare_modifiers();
        self.output.new_variable_statement(modifiers, Some(list))
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.tryGetNameOfAssignedExpression
    pub fn try_assigned_expression_name(
        &mut self,
        expression: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let name = if self.node(expression).kind() == K::Identifier {
            Some(expression)
        } else if self.node(expression).kind() != K::PropertyAccessExpression {
            self.node(expression).name()
        } else {
            None
        };
        if let Some(name) = name {
            let text = self.output.view().node_text(name)?.into_js_string();
            if !text.is_empty() && text.as_bytes() != b"default" {
                return Ok(Some(
                    if self
                        .resolver
                        .name_resolvable(self.enclosing, text.as_bytes())?
                    {
                        self.unique_name(text)
                    } else {
                        self.output.new_identifier(text)
                    },
                ));
            }
        }
        Ok(None)
    }
    fn assigned_expression_name(
        &mut self,
        expression: NodeId,
        export_equals: bool,
    ) -> Result<NodeId, R::Error> {
        if let Some(name) = self.try_assigned_expression_name(expression)? {
            return Ok(name);
        }
        let text = if export_equals && self.output.read_source_file(self.source)?.is_js() {
            b"_exports".as_slice()
        } else {
            b"_default".as_slice()
        };
        Ok(self.unique_name(JsString::from_bytes(text)))
    }
    // port: tsc/internal/ast/utilities.go:SkipOuterExpressions
    // OEKExpressionTypePassthrough retains assertions; only parentheses and
    // assignment/comma results preserve the expression's inferred type here.
    fn assigned_expression(&self, mut node: NodeId) -> Result<NodeId, R::Error> {
        loop {
            if self.node(node).kind() == K::ParenthesizedExpression {
                node = Self::required(self.node(node).expression())?;
                continue;
            }
            if self.node(node).kind() == K::BinaryExpression {
                let read = self.node(node);
                let data = read.as_binary_expression().unwrap();
                let operator = Self::required(data.operator_token())?;
                if matches!(
                    self.node(operator).kind().known(),
                    Some(K::EqualsToken | K::CommaToken)
                ) {
                    node = Self::required(data.right())?;
                    continue;
                }
            }
            return Ok(node);
        }
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformExportAssignment
    pub fn export_assignment(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        let data = self
            .node(node)
            .data_source()
            .as_export_assignment()
            .unwrap()
            .to_owned();
        self.export_assignment_from(
            node,
            node,
            Self::required(data.expression)?,
            data.is_export_equals,
        )
    }
    pub fn export_assignment_from(
        &mut self,
        input: NodeId,
        node: NodeId,
        expression: NodeId,
        is_export_equals: bool,
    ) -> Result<NodeId, R::Error> {
        let parent = self.node(input).parent().map(|p| self.node(p).kind());
        if parent == Some(K::SourceFile.into()) {
            self.external_indicator = true;
        }
        self.has_scope_marker = true;
        if self.node(expression).kind() == K::Identifier
            && matches!(
                parent.and_then(tsr_ast::NodeKind::known),
                Some(K::SourceFile | K::ModuleBlock)
            )
        {
            let result =
                self.output
                    .new_export_assignment(None, is_export_equals, None, Some(expression));
            self.emit.assign_comment_range(self.output, result, input);
            return Ok(result);
        }
        let unwrapped = self.assigned_expression(expression)?;
        let name = self.assigned_expression_name(unwrapped, is_export_equals)?;
        self.cjs.assignment_name = Some(name);
        let declaration = if self.node(unwrapped).kind() == K::ClassExpression {
            let modifiers = self.declare_modifiers();
            self.class_expression_declaration(unwrapped, name, modifiers)?
        } else if tsr_ast::utilities::is_function_like(Some(&self.node(unwrapped))) {
            self.function_expression_declaration(
                unwrapped,
                name,
                if self.node(node).kind() == K::ExportAssignment {
                    self.node(node).type_node()
                } else {
                    None
                },
            )?
        } else {
            let old = self.tracker.selector.clone();
            self.tracker.selector = super::tracker::Selector::fixed(
                super::diagnostics::SymbolAccessibilityDiagnostic {
                    diagnostic_message:
                        tsr_diagnostics::Default_export_of_the_module_has_or_is_using_private_name_0,
                    error_node: Some(input),
                    type_name: None,
                },
            );
            self.tracker.fallback.push(Some(node));
            let result: Result<NodeId, R::Error> = (|| {
                let literal =
                    util::unwrap_parenthesized_expression(self.output.view(), expression)?;
                let initializer = if tsr_ast::utilities_tail::is_primitive_literal_value(
                    self.output.view(),
                    &self.node(literal),
                    true,
                )? {
                    let result = self.resolver.create_literal_const_value(
                        self.output,
                        self.emit,
                        node,
                        &mut self.tracker,
                    )?;
                    self.flush_reports()?;
                    result
                } else {
                    None
                };
                let ty = if initializer.is_none() {
                    self.ensure_type(node, false)?
                } else {
                    None
                };
                Ok(self.const_variable(name, ty, initializer))
            })();
            self.tracker.fallback.pop();
            self.tracker.selector = old;
            let declaration = result?;
            self.emit
                .assign_comment_range(self.output, declaration, input);
            let assignment =
                self.output
                    .new_export_assignment(None, is_export_equals, None, Some(name));
            return Ok(self.syntax_list(vec![declaration, assignment]));
        };
        self.emit
            .assign_comment_range(self.output, declaration, input);
        let assignment =
            self.output
                .new_export_assignment(None, is_export_equals, None, Some(name));
        self.emit
            .add_emit_flags(assignment, tsr_printer::emit_flags::NO_COMMENTS);
        Ok(self.syntax_list(vec![assignment, declaration]))
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformFunctionLikeToDeclaration
    fn function_expression_declaration(
        &mut self,
        node: NodeId,
        name: NodeId,
        full: Option<NodeId>,
    ) -> Result<NodeId, R::Error> {
        let read = self.node(node);
        let data = read.data_source();
        let signature = match read.kind().known() {
            Some(K::FunctionExpression) => data
                .as_function_expression()
                .and_then(|d| d.full_signature()),
            Some(K::ArrowFunction) => data.as_arrow_function().and_then(|d| d.full_signature()),
            _ => return Self::unsupported("declaration emit: exported function expression kind"),
        }
        .or(full);
        drop(read);
        if let Some(signature) = signature {
            let ty = self.visit(Some(signature))?;
            return Ok(self.const_variable(name, ty, None));
        }
        let modifiers = self.declare_modifiers();
        let parameters = self.parameters(node)?;
        let types = self.type_parameters(node)?;
        let result = self.ensure_type(node, false)?;
        Ok(self.output.new_function_declaration(
            modifiers,
            None,
            Some(name),
            types,
            parameters,
            result,
            None,
            None,
        ))
    }
}
