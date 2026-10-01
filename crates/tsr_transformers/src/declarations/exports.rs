//! The export-assignment transforms of `transform.go`.
use super::{
    transform::{Transformer, NIL},
    util,
};
use tsr_ast::{node_flags as nf, FactoryMethods, JsString, NodeId, NodeListId, SyntaxKind as K};
use tsr_printer::emit_resolver::DeclarationEmitResolver;

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.tryGetNameOfAssignedExpression
    pub fn try_get_name_of_assigned_expression(
        &mut self,
        unwrapped: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let mut name_text = JsString::default();
        let name = self
            .node(unwrapped)
            .name()
            .filter(|_| self.kind(unwrapped) != K::PropertyAccessExpression);
        if let Some(name) = name {
            name_text = self.node_text(name)?;
        } else if self.kind(unwrapped) == K::Identifier {
            name_text = self.node_text(unwrapped)?;
        }
        let mut name_node = None;
        if !name_text.is_empty() && name_text.as_bytes() != b"default" {
            if self
                .resolver
                .name_resolvable(self.enclosing_declaration, name_text.as_bytes())?
            {
                // create a unique name that shares the same text as its' base
                name_node = Some(self.new_unique_name(name_text.as_bytes()));
            } else {
                // use the node's name as-is, since it's not otherwise in-scope
                name_node = Some(self.output.new_identifier(name_text));
            }
        }
        Ok(name_node)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.getNameOfExportedAssignedExpression
    fn get_name_of_exported_assigned_expression(
        &mut self,
        unwrapped: NodeId,
        is_export_equals: bool,
    ) -> Result<NodeId, R::Error> {
        let name_node = match self.try_get_name_of_assigned_expression(unwrapped)? {
            Some(name_node) => name_node,
            // fallback to a default name
            None => {
                if is_export_equals && self.is_source_file_js()? {
                    // only JS files prefer to use `_exports` for export assignments - TS has always used `_default` for both `export=` and `export default`
                    self.new_unique_name(b"_exports")
                } else {
                    self.new_unique_name(b"_default")
                }
            }
        };
        self.cjs_export_assignment_name = Some(name_node);
        Ok(name_node)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformExportAssignment
    pub fn transform_export_assignment(
        &mut self,
        input: NodeId,
        assignment: NodeId,
        expression: NodeId,
        is_export_equals: bool,
    ) -> Result<NodeId, R::Error> {
        let parent_kind = self.parent_kind(input);
        if parent_kind == K::SourceFile {
            self.result_has_external_module_indicator = true;
        }
        self.result_has_scope_marker = true;
        if self.kind(expression) == K::Identifier
            && matches!(parent_kind, K::SourceFile | K::ModuleBlock)
        {
            let export_assignment =
                self.output
                    .new_export_assignment(None, is_export_equals, None, Some(expression));
            self.preserve_js_doc(export_assignment, input);
            return Ok(export_assignment);
        }

        // Check if the expression is a class expression - emit as a class declaration + export assignment
        let unwrapped = tsr_ast::utilities::skip_outer_expressions(
            self.view(),
            expression,
            tsr_ast::evaluator::outer_expression_kinds::EXPRESSION_TYPE_PASSTHROUGH,
        )?;
        let new_id = self.get_name_of_exported_assigned_expression(unwrapped, is_export_equals)?;
        if self.kind(unwrapped) == K::ClassExpression {
            let mods = self.declare_modifier_list(false);
            let class_decl =
                self.transform_class_expression_to_declaration(unwrapped, new_id, mods)?;
            self.preserve_js_doc(class_decl, input);
            // Reuse the same name node for the export so unique names resolve consistently
            let export_assignment =
                self.output
                    .new_export_assignment(None, is_export_equals, None, Some(new_id));
            self.remove_all_comments(export_assignment);
            return Ok(self.new_syntax_list(vec![export_assignment, class_decl]));
        } else if tsr_ast::utilities::is_function_like(Some(&self.node(unwrapped))) {
            // Promote function or arrow function expressions to a function declaration
            let mods = self.declare_modifier_list(false);
            let full_signature_type = self.node(assignment).type_node();
            let func_decl = self.transform_function_like_to_declaration(
                unwrapped,
                new_id,
                mods,
                full_signature_type,
            )?;
            self.preserve_js_doc(func_decl, input);
            // Reuse the same name node for the export so unique names resolve consistently
            let export_assignment =
                self.output
                    .new_export_assignment(None, is_export_equals, None, Some(new_id));
            self.remove_all_comments(export_assignment);
            return Ok(self.new_syntax_list(vec![export_assignment, func_decl]));
        }

        // expression is non-identifier, create _default typed variable to reference
        self.set_fixed_diagnostic_context(
            tsr_diagnostics::Default_export_of_the_module_has_or_is_using_private_name_0,
            input,
            None,
        );
        self.cjs_export_assignment_name = Some(new_id);
        self.tracker.push_error_fallback_node(Some(assignment));
        let mut type_ = None;
        let mut initializer = None;
        let literal = util::unwrap_parenthesized_expression(self.view(), expression)?;
        if tsr_ast::utilities_tail::is_primitive_literal_value(
            self.view(),
            &self.node(literal),
            true,
        )? {
            let parse_node = self.parse_node(assignment).expect(NIL);
            initializer = self.resolver.create_literal_const_value(
                self.output,
                self.emit,
                parse_node,
                &mut self.tracker,
            )?;
            self.apply_tracker_reports()?;
        }
        if initializer.is_none() {
            type_ = self.ensure_type(assignment, false)?;
        }
        let var_decl = self
            .output
            .new_variable_declaration(Some(new_id), None, type_, initializer);
        self.tracker.pop_error_fallback_node();
        let mod_list = self.declare_modifier_list(false);
        let declarations = self.new_node_list(vec![var_decl]);
        let declaration_list = self
            .output
            .new_variable_declaration_list(Some(declarations), nf::CONST);
        let statement = self
            .output
            .new_variable_statement(mod_list, Some(declaration_list));
        let export_assignment =
            self.output
                .new_export_assignment(None, is_export_equals, None, Some(new_id));
        // Remove comments from the export declaration and copy them onto the synthetic _default declaration
        self.preserve_js_doc(statement, input);
        Ok(self.new_syntax_list(vec![statement, export_assignment]))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformFunctionLikeToDeclaration
    fn transform_function_like_to_declaration(
        &mut self,
        unwrapped: NodeId,
        func_name: NodeId,
        mods: Option<NodeListId>,
        full_signature_type: Option<NodeId>,
    ) -> Result<NodeId, R::Error> {
        let sig = self
            .function_like_full_signature(unwrapped)
            .expect(NIL)
            .or(full_signature_type);
        if sig.is_none() {
            let (type_parameters, parameters) = {
                let read = self.node(unwrapped);
                (read.type_parameter_list(), read.parameter_list())
            };
            let type_parameters = self.ensure_type_params(unwrapped, type_parameters)?;
            let parameters = self.update_param_list(unwrapped, parameters)?;
            let ty = self.ensure_type(unwrapped, false)?;
            let full_signature = self.visit_single(sig)?;
            return Ok(self.output.new_function_declaration(
                mods,
                None,
                Some(func_name),
                type_parameters,
                Some(parameters),
                ty,
                full_signature,
                None,
            ));
        }
        // If a full signature type node is present, emit as a variable statement to reuse it
        let ty = self.visit_single(sig)?;
        Ok(self.new_single_variable_statement(mods, func_name, ty, None, nf::CONST))
    }
}
