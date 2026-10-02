//! The CommonJS export collection of `transform.go`: `module.exports =`,
//! `exports.x =` and `Object.defineProperty(exports, ...)` become declaration
//! statements before the source file's statements are visited.
use super::transform::{Transformer, NIL};
use std::ops::ControlFlow;
use tsr_ast::{
    modifier_flags as mf, node_flags as nf, AstView, ChildVisitor, Factory, FactoryMethods,
    JSDeclarationKind, JsString, NodeId, NodeListId, NodeSlice, RuntimeFactory, SyntaxKind as K,
};
use tsr_printer::emit_resolver::DeclarationEmitResolver;

/// One step of an expression-visitor walk: entering a node, or restoring
/// the diagnostic context its visit installed.
enum Frame {
    Enter(NodeId),
    Exit(super::transform::DiagnosticContext),
}

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    /// The pin's `cjsExportAssignmentVisitor.VisitNode(root)` and
    /// `expressionVisitor.VisitNode(root)`: every node of the tree, in visit
    /// order, inside the diagnostic context its own visit installs. The
    /// visitors return each node unchanged, so only their effects remain.
    pub fn walk_expressions(
        &mut self,
        root: NodeId,
        visit: fn(&mut Self, NodeId) -> Result<(), R::Error>,
    ) -> Result<(), R::Error> {
        let mut frames = vec![Frame::Enter(root)];
        while let Some(frame) = frames.pop() {
            match frame {
                Frame::Exit(context) => self.cleanup_diagnostic_context(context),
                Frame::Enter(node) => {
                    let (_, context) = self.setup_diagnostic_context(node)?;
                    visit(self, node)?;
                    frames.push(Frame::Exit(context));
                    let children = self.children_of(node)?;
                    frames.extend(children.into_iter().rev().map(Frame::Enter));
                }
            }
        }
        Ok(())
    }

    /// The children `VisitEachChild` visits, in order.
    pub fn children_of(&self, node: NodeId) -> Result<Vec<NodeId>, R::Error> {
        let mut visitor = Children {
            view: self.view(),
            nodes: Vec::new(),
            error: None,
        };
        let _ = self.node(node).for_each_child(&mut visitor);
        if let Some(error) = visitor.error {
            return Err(error.into());
        }
        Ok(visitor.nodes)
    }

    fn has_common_js_module_indicator(&self) -> Result<bool, R::Error> {
        Ok(self
            .output
            .read_source_file(self.current_source_file)?
            .common_js_module_indicator()
            .is_some())
    }

    /// The switch of the pin's `visitCJSExportAssignments`; `walk_expressions`
    /// is its recursion.
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visitCJSExportAssignments
    pub fn visit_cjs_export_assignments(&mut self, expression: NodeId) -> Result<(), R::Error> {
        if tsr_ast::get_assignment_declaration_kind(self.view(), expression)?
            == JSDeclarationKind::ModuleExports
            && self.has_common_js_module_indicator()?
        {
            let parent = self.parent(expression).expect(NIL);
            let right = self
                .node(expression)
                .as_binary_expression()
                .expect("binary expression payload")
                .right()
                .expect(NIL);
            let result = self.transform_export_assignment(
                parent, expression, right, true, /*isExportEquals*/
            )?;
            self.cjs_export_assignment = Some(result);
            self.result_has_scope_marker = true;
            self.result_has_external_module_indicator = true;
        }
        Ok(())
    }

    /// The switch of the pin's `visitNestedExpression`; `walk_expressions`
    /// is its recursion.
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visitNestedExpression
    pub fn visit_nested_expression(&mut self, expression: NodeId) -> Result<(), R::Error> {
        match tsr_ast::get_assignment_declaration_kind(self.view(), expression)? {
            JSDeclarationKind::Property => self.transform_expando_assignment(expression)?,
            JSDeclarationKind::ExportsProperty if self.has_common_js_module_indicator()? => {
                let left = self
                    .node(expression)
                    .as_binary_expression()
                    .expect("binary expression payload")
                    .left()
                    .expect(NIL);
                let name =
                    tsr_ast::get_element_or_property_access_name(self.view(), left)?.expect(NIL);
                let name = self.get_name_expression_preferring_identifier(name)?;
                if let Some(result) = self.transform_common_js_export(expression, name)? {
                    self.cjs_export_members.push(result);
                }
            }
            JSDeclarationKind::ObjectDefinePropertyExports
                if self.has_common_js_module_indicator()? =>
            {
                let arguments = self.list_nodes(self.node(expression).argument_list());
                let name = *arguments
                    .get(1)
                    .expect("runtime error: index out of range [1] with length 1");
                let name = self.get_name_expression_preferring_identifier(name)?;
                if let Some(result) = self.transform_common_js_export(expression, name)? {
                    self.cjs_export_members.push(result);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The pin's `getNameExpressionPreferringIdentifier`. Its port marker sits
    /// on the checker half of the bridge (`emit_referenced_name_declaration`),
    /// which answers the reference lookup for the identifier made here.
    pub fn get_name_expression_preferring_identifier(
        &mut self,
        mut name_expr: NodeId,
    ) -> Result<NodeId, R::Error> {
        if self.kind(name_expr) == K::NumericLiteral {
            // Numeric property names are string properties in JS; convert to string literal
            let text = self.node_text(name_expr)?;
            name_expr = self.output.new_string_literal(text, 0);
        }
        if matches!(
            self.kind(name_expr),
            K::StringLiteral | K::NoSubstitutionTemplateLiteral
        ) {
            let text = self.node_text(name_expr)?;
            if tsr_scanner::is_identifier_text(text.as_bytes(), tsr_core::LanguageVariant::STANDARD)
            {
                let result = self.output.new_identifier(text.clone()); // prefer non-string literal names where possible
                let kw_kind = tsr_scanner::string_to_token(text.as_bytes());
                // keep keywords as strings, except `default`, which has special reformulations in the transformer
                if kw_kind == K::Unknown || kw_kind == K::DefaultKeyword {
                    // fake this into a parse tree node so the reference resolver resolves the node via `resolveName`
                    let parent = self.parent(name_expr);
                    self.output.set_node_parent(result, parent);
                    let flags = self.node(result).flags() & !nf::SYNTHESIZED;
                    self.output.set_node_flags(result, flags);
                    // intentionally leave Loc unset so the string isn't used as the text source of the identifier
                    return Ok(result);
                }
            }
        }
        Ok(name_expr)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformBinaryExpressionToExportDeclaration
    pub fn transform_binary_expression_to_export_declaration(
        &mut self,
        input: NodeId,
        name: NodeId,
    ) -> Result<NodeId, R::Error> {
        let mut property_name = self
            .node(input)
            .as_binary_expression()
            .expect("binary expression payload")
            .right();

        // track alias target so referenced declarations are included in the output
        let result = self
            .resolver
            .entity_name_visible(property_name.expect(NIL), self.enclosing_declaration)?;
        self.handle_symbol_accessibility_error(result)?;

        if self.kind(name) == K::Identifier
            && self.node_text(property_name.expect(NIL))? == self.node_text(name)?
        {
            property_name = None;
        }

        Ok(self.new_named_export_declaration(property_name, name))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformCommonJSExport
    fn transform_common_js_export(
        &mut self,
        input: NodeId,
        name: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let Some(res) = self.transform_common_js_export_worker(input, name)? else {
            return Ok(None);
        };
        Ok(Some(self.wrap_in_cjs_export_namespace(res)))
    }

    /// The pin's `GetReferencedValueDeclaration(name)` for an export name: a
    /// name made by `getNameExpressionPreferringIdentifier` is answered from
    /// its text and its parse-tree parent.
    fn referenced_value_declaration_of_name(
        &mut self,
        name: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        if name.arena() == self.output.id().arena() {
            return match self.parent(name) {
                Some(parent) => {
                    let text = self.node_text(name)?;
                    self.resolver.referenced_name_declaration(text, parent)
                }
                None => Ok(None),
            };
        }
        self.resolver.referenced_value_declaration(name)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformCommonJSExportWorker
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the pinned branch order of transformCommonJSExportWorker in one function"
    )]
    fn transform_common_js_export_worker(
        &mut self,
        input: NodeId,
        name: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let mut name_text = JsString::default();
        if matches!(self.kind(name), K::Identifier | K::StringLiteral) {
            name_text = self.node_text(name)?;
        }
        if self.witnessed_cjs_exports.contains(&name_text) && !name_text.is_empty() {
            return Ok(None); // Already emitted this export name
        }
        self.witnessed_cjs_exports.insert(name_text);
        self.result_has_external_module_indicator = true;
        self.result_has_scope_marker = true;
        // only transform cjs exports to shorthand at the top-level of a source file, otherwise we uniformly emit nested exports with a type annotation
        if self.is_common_js_alias_export(input)? {
            let parent = self.parent(input).expect(NIL);
            if self.kind(parent) == K::ExpressionStatement
                && self.parent_kind(parent) == K::SourceFile
            {
                // export { name }
                // export { source as name }
                return self
                    .transform_binary_expression_to_export_declaration(input, name)
                    .map(Some);
            }
        }

        // Check if the RHS is a class expression - emit as a class declaration instead of a typed variable
        if self.kind(input) == K::BinaryExpression {
            let right = self
                .node(input)
                .as_binary_expression()
                .expect("binary expression payload")
                .right()
                .expect(NIL);
            let rhs = super::util::unwrap_parenthesized_expression(self.view(), right)?;
            if self.kind(rhs) == K::ClassExpression {
                let class_expr_name = self.node(rhs).name();
                let class_expr_text = match class_expr_name {
                    Some(class_expr_name) => self.node_text(class_expr_name)?,
                    None => JsString::default(),
                };
                let has_expr_name = class_expr_name.is_some() && !class_expr_text.is_empty();

                if has_expr_name {
                    // Set up TrackSymbol watch to detect if the class expression's own
                    // symbol is referenced during member type serialization.
                    self.tracker.watched_class_symbol =
                        self.resolver.bound_symbol_of_declaration(rhs)?;
                    self.tracker.class_symbol_tracked = false;
                    let result = self.transform_named_class_expression_export(
                        input,
                        name,
                        rhs,
                        &class_expr_text,
                    );
                    self.tracker.watched_class_symbol = None;
                    self.tracker.class_symbol_tracked = false;
                    return result.map(Some);
                }
                let mut mods = vec![self.new_modifier(K::ExportKeyword)];
                if self.needs_declare {
                    mods.push(self.new_modifier(K::DeclareKeyword));
                }
                let class_name = if self.kind(name) == K::Identifier {
                    name
                } else {
                    self.new_unique_name(b"_class")
                };
                let mods = self.new_modifier_list(mods);
                let class_decl =
                    self.transform_class_expression_to_declaration(rhs, class_name, Some(mods))?;
                self.preserve_js_doc(class_decl, input);
                if self.kind(name) != K::Identifier {
                    // Non-identifier name: emit class declaration + named export
                    let export_decl = self.new_named_export_declaration(Some(class_name), name);
                    self.remove_all_comments(export_decl);
                    return Ok(Some(self.new_syntax_list(vec![class_decl, export_decl])));
                }
                return Ok(Some(class_decl));
            }
        }

        if self.kind(name) == K::Identifier {
            if self.node_text(name)?.as_bytes() == b"default" {
                // const _default: Type; export default _default;
                let new_id = self.new_unique_name(b"_default");
                self.set_fixed_diagnostic_context(
                    tsr_diagnostics::Default_export_of_the_module_has_or_is_using_private_name_0,
                    input,
                    None,
                );
                self.tracker.push_error_fallback_node(Some(input));
                let type_ = self.ensure_type(input, false)?;
                let var_decl =
                    self.output
                        .new_variable_declaration(Some(new_id), None, type_, None);
                self.tracker.pop_error_fallback_node();
                let mod_list = self.declare_modifier_list(false);
                let declarations = self.new_node_list(vec![var_decl]);
                let declaration_list = self
                    .output
                    .new_variable_declaration_list(Some(declarations), nf::CONST);
                let statement = self
                    .output
                    .new_variable_statement(mod_list, Some(declaration_list));

                let modifiers = self.node(input).modifiers();
                let assignment =
                    self.output
                        .new_export_assignment(modifiers, false, None, Some(new_id));
                // Remove comments from the export declaration and copy them onto the synthetic _default declaration
                self.preserve_js_doc(statement, input);
                self.remove_all_comments(assignment);
                return Ok(Some(self.new_syntax_list(vec![statement, assignment])));
            }
            let referenced = self.referenced_value_declaration_of_name(name)?;
            if referenced == Some(input)
                || self.referenced_value_declaration_of_name(name)?.is_none()
            {
                // only inline to a export var if the `name` lookup points at this assignment or nothing - if it points at something else, we must use a temp name
                // export var name: Type
                self.tracker.push_error_fallback_node(Some(input));
                let type_ = self.ensure_type(input, false)?;
                let var_decl = self
                    .output
                    .new_variable_declaration(Some(name), None, type_, None);
                self.tracker.pop_error_fallback_node();
                let mut mods = vec![self.new_modifier(K::ExportKeyword)];
                if self.needs_declare {
                    mods.push(self.new_modifier(K::DeclareKeyword));
                }
                let mod_list = self.new_modifier_list(mods);
                let declarations = self.new_node_list(vec![var_decl]);
                let declaration_list = self
                    .output
                    .new_variable_declaration_list(Some(declarations), nf::NONE);
                return Ok(Some(
                    self.output
                        .new_variable_statement(Some(mod_list), Some(declaration_list)),
                ));
            }
        }
        // const _exported: Type; export {_exported as "name"};
        let new_id = self.new_unique_name(b"_exported");
        self.set_fixed_diagnostic_context(
            tsr_diagnostics::Default_export_of_the_module_has_or_is_using_private_name_0,
            input,
            None,
        );
        self.tracker.push_error_fallback_node(Some(input));
        let type_ = self.ensure_type(input, false)?;
        let var_decl = self
            .output
            .new_variable_declaration(Some(new_id), None, type_, None);
        self.tracker.pop_error_fallback_node();
        let mod_list = self.declare_modifier_list(false);
        let declarations = self.new_node_list(vec![var_decl]);
        let declaration_list = self
            .output
            .new_variable_declaration_list(Some(declarations), nf::CONST);
        let statement = self
            .output
            .new_variable_statement(mod_list, Some(declaration_list));

        let assignment = self.new_named_export_declaration(Some(new_id), name);
        // Remove comments from the export declaration and copy them onto the synthetic _default declaration
        self.preserve_js_doc(statement, input);
        self.remove_all_comments(assignment);
        Ok(Some(self.new_syntax_list(vec![statement, assignment])))
    }

    /// The named class expression arm of `transformCommonJSExportWorker`, run
    /// while the tracker watches the class symbol.
    fn transform_named_class_expression_export(
        &mut self,
        input: NodeId,
        name: NodeId,
        rhs: NodeId,
        class_expr_text: &JsString,
    ) -> Result<NodeId, R::Error> {
        // Serialize class members using the class expression name, which
        // triggers TrackSymbol for any self-referential member types.
        let class_name = self.output.new_identifier(class_expr_text.clone());
        let class_mods = vec![self.new_modifier(K::ExportKeyword)];
        let class_mods = self.new_modifier_list(class_mods);
        let class_decl =
            self.transform_class_expression_to_declaration(rhs, class_name, Some(class_mods))?;
        self.preserve_js_doc(class_decl, input);

        // Determine if namespace isolation is needed:
        // - The class expression name differs from the export name, OR
        // - The class's own symbol was used in a member's serialized type
        let names_differ =
            self.kind(name) != K::Identifier || *class_expr_text != self.node_text(name)?;
        let needs_isolation = names_differ || self.tracker.class_symbol_tracked;

        if needs_isolation {
            let ns_name = self.new_unique_name(b"_ns");
            let ns_mods = self.declare_modifier_list(false);
            let statements = self.new_node_list(vec![class_decl]);
            let body = self.output.new_module_block(Some(statements));
            let ns_decl = self.output.new_module_declaration(
                ns_mods,
                K::NamespaceKeyword.into(),
                Some(ns_name),
                None,
                Some(body),
            );

            let mut alias_base = b"_exported".to_vec();
            let name_text = self.node_text(name)?;
            let mut underscored = b"_".to_vec();
            underscored.extend_from_slice(name_text.as_bytes());
            if self.kind(name) == K::Identifier
                && tsr_scanner::is_identifier_text(
                    &underscored,
                    tsr_core::LanguageVariant::STANDARD,
                )
            {
                alias_base = underscored;
            }
            let import_alias = self.new_unique_name(&alias_base);
            let qualified_name = self
                .output
                .new_qualified_name(Some(ns_name), Some(class_name));
            let import_decl = self.output.new_import_equals_declaration(
                None,
                false,
                Some(import_alias),
                Some(qualified_name),
            );

            let export_decl = self.new_named_export_declaration(Some(import_alias), name);
            self.remove_all_comments(export_decl);

            return Ok(self.new_syntax_list(vec![ns_decl, import_decl, export_decl]));
        }

        // No isolation needed: names match and no self-references.
        // Update modifiers to include declare if needed.
        let mut mods = vec![self.new_modifier(K::ExportKeyword)];
        if self.needs_declare {
            mods.push(self.new_modifier(K::DeclareKeyword));
        }
        let mods = self.new_modifier_list(mods);
        let (class_name, type_parameters, heritage_clauses, members) = {
            let read = self.node(class_decl);
            let data = read.as_class_declaration().expect("class payload");
            (
                read.name(),
                data.type_parameters(),
                data.heritage_clauses(),
                data.members(),
            )
        };
        Ok(self.output.update_class_declaration(
            class_decl,
            Some(mods),
            class_name,
            type_parameters,
            heritage_clauses,
            members,
        ))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.wrapInCJSExportNamespace
    fn wrap_in_cjs_export_namespace(&mut self, content: NodeId) -> NodeId {
        let Some(ns_name) = self.cjs_export_assignment_name else {
            return content;
        };
        // Reuse the same name node so unique names resolve consistently with the class/export
        let members = self.node_or_syntax_list_children(content);
        let ns_mods = self.declare_modifier_list(false);
        let mut stripped = Vec::with_capacity(members.len());
        for member in members {
            if let Some(member) = self.strip_declare_modifiers(Some(member)) {
                stripped.push(member);
            }
        }
        let statements = self.new_node_list(stripped);
        let body = self.output.new_module_block(Some(statements));
        self.output.new_module_declaration(
            ns_mods,
            K::NamespaceKeyword.into(),
            Some(ns_name),
            None,
            Some(body),
        )
    }

    // port: tsc/internal/transformers/declarations/transform.go:isCommonJSAliasExport
    fn is_common_js_alias_export(&mut self, node: NodeId) -> Result<bool, R::Error> {
        if self.kind(node) == K::BinaryExpression {
            let right = self
                .node(node)
                .as_binary_expression()
                .expect("binary expression payload")
                .right()
                .expect(NIL);
            if self.kind(right) == K::Identifier {
                if let Some(symbol) = self.resolver.bound_symbol_of_declaration(node)? {
                    if self.resolver.symbol_declarations(symbol)?.len() == 1 {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.stripDeclareModifiers
    fn strip_declare_modifiers(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        let node = node?;
        if let Some(mods) = self.node(node).modifiers() {
            let flags = self.output.read_list(mods).modifier_flags();
            if flags & mf::AMBIENT != 0 {
                let filtered: Vec<_> = self
                    .list_nodes(Some(mods))
                    .into_iter()
                    .filter(|modifier| is_not_declare_modifier(&self.node(*modifier)))
                    .collect();
                let list = self.new_modifier_list(filtered);
                Factory::node_mut(&mut *self.output, node).set_modifiers(Some(list));
            }
        }
        Some(node) // no need to recur into children, only strip at top-level
    }

    /// `[export]`-style modifier list from flags
    /// (`NewModifierList(CreateModifiersFromModifierFlags(flags, NewModifier))`).
    pub fn modifier_list_from_flags(&mut self, flags: u32) -> NodeListId {
        let modifiers = self.create_modifiers_from_modifier_flags(flags);
        self.new_modifier_list(modifiers)
    }
}

// port: tsc/internal/transformers/declarations/transform.go:isNotDeclareModifier
fn is_not_declare_modifier(modifier: &impl tsr_ast::NodeAccess) -> bool {
    modifier.kind() != K::DeclareKeyword
}

/// Collects the children of one node, as `VisitEachChild` visits them.
struct Children<'a> {
    view: AstView<'a>,
    nodes: Vec<NodeId>,
    error: Option<tsr_arena::Error>,
}
impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.nodes.push(node);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        match self.view.list(list) {
            Ok(list) => self.visit_node_slice(list.nodes()),
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        match self.view.node_slice(nodes) {
            Ok(nodes) => {
                self.nodes.extend(nodes.iter().flatten());
                ControlFlow::Continue(())
            }
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
}
