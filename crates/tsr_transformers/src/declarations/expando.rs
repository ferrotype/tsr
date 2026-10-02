//! The expando transforms of `transform.go`: property assignments to a
//! function become a namespace merged with the function's declaration.
use super::{
    transform::{Transformer, NIL},
    util,
};
use tsr_ast::{
    modifier_flags as mf, symbol_flags as sf, Factory, FactoryMethods, JsString, NodeId,
    NodeListId, RuntimeFactory, SyntaxKind as K,
};
use tsr_printer::emit_resolver::DeclarationEmitResolver;

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    /// The pin's `transformExpandoAssignment`. Its port marker sits on the
    /// checker half of the bridge (`create_emit_scope`), which builds the
    /// synthesized namespace scope the type of the member is serialized in.
    pub fn transform_expando_assignment(&mut self, node: NodeId) -> Result<(), R::Error> {
        let left = self
            .node(node)
            .as_binary_expression()
            .expect("binary expression payload")
            .left()
            .expect(NIL);

        let Some(symbol) = self.resolver.bound_symbol_of_declaration(node)? else {
            return Ok(());
        };
        if self.resolver.symbol_flags(symbol)? & sf::ASSIGNMENT == 0 {
            return Ok(());
        }

        let ns = tsr_ast::get_leftmost_access_expression(self.view(), left)?;
        if self.kind(ns) != K::Identifier {
            return Ok(());
        }

        let Some(declaration) = self.resolver.referenced_value_declaration(ns)? else {
            return Ok(());
        };
        self.retain_source_of(declaration)?;

        if self.should_strip_internal(Some(declaration))? {
            return Ok(());
        }

        if self.kind(declaration) == K::VariableDeclaration
            && self.node(declaration).type_node().is_some()
        {
            return Ok(());
        }

        if self.kind(declaration) == K::FunctionDeclaration
            && self
                .function_like_full_signature(declaration)
                .expect(NIL)
                .is_some()
        {
            return Ok(());
        }

        if self.kind(declaration) == K::VariableDeclaration
            && !self
                .node(declaration)
                .initializer()
                .is_some_and(|initializer| {
                    tsr_ast::utilities::is_function_like(Some(&self.node(initializer)))
                })
        {
            return Ok(()); // We're going to add a type, no need to dupe members with a namespace
        }

        let Some(host) = self.resolver.bound_symbol_of_declaration(declaration)? else {
            return Ok(());
        };

        let ns_text = self.node_text(ns)?;
        let name = self.output.new_identifier(ns_text.clone());
        let property = self.try_get_property_name(left)?;
        if property.is_empty()
            || !tsr_scanner::is_identifier_text(
                property.as_bytes(),
                tsr_core::LanguageVariant::STANDARD,
            )
        {
            return Ok(());
        }

        let host_id = self.get_expando_host_id(declaration);

        if tsr_ast::is_declaration(&self.node(declaration))
            && util::is_declaration_and_not_visible(self, declaration)?
        {
            // The host isn't visible (yet) - printing the type of a visible declaration may still
            // late-mark it as visible (e.g. an exported variable whose type prints as `typeof host`),
            // so defer the assignment to be processed if and when that happens.
            self.deferred_expando_assignments
                .entry(host_id)
                .or_default()
                .push(node);
            return Ok(());
        }

        if self.kind(declaration) == K::FunctionDeclaration
            && !util::should_emit_function_properties(self, declaration)?
        {
            return Ok(());
        }

        self.transform_expando_host(name, declaration)?;

        let export_name = self.output.new_identifier(property.clone());
        let mut local_name = self.try_get_name_of_assigned_expression(node)?;
        if local_name.is_none()
            && !self
                .resolver
                .name_resolvable(self.enclosing_declaration, property.as_bytes())?
            && !is_non_contextual_keyword(property.as_bytes())
        {
            // use exportName as localName if there won't be any conflicts or keyword issues
            local_name = Some(export_name);
        }
        let local_name = match local_name {
            Some(local_name)
                if !is_non_contextual_keyword(self.node_text(local_name)?.as_bytes()) =>
            {
                local_name
            }
            // fallback to a generated name if the localName doesn't exist or is a keyword
            _ => self.emit.new_generated_name_for_node(self.output, node),
        };

        let (_, cleanup_diagnostic_context) = self.setup_diagnostic_context(node)?;
        let result = self.transform_expando_assignment_members(
            node,
            symbol,
            host,
            host_id,
            name,
            ns_text,
            export_name,
            local_name,
        );
        self.cleanup_diagnostic_context(cleanup_diagnostic_context);
        result
    }

    /// The tail of `transformExpandoAssignment`, inside its diagnostic
    /// context.
    #[allow(
        clippy::too_many_arguments,
        reason = "The pinned function's locals are carried into its tail unchanged"
    )]
    fn transform_expando_assignment_members(
        &mut self,
        node: NodeId,
        symbol: tsr_ast::SymbolId,
        host: tsr_ast::SymbolId,
        host_id: NodeId,
        name: NodeId,
        name_text: JsString,
        export_name: NodeId,
        local_name: NodeId,
    ) -> Result<(), R::Error> {
        let right = self
            .node(node)
            .as_binary_expression()
            .expect("binary expression payload")
            .right()
            .expect(NIL);
        if self.kind(right) == K::Identifier {
            // alias-like, emit an `export {name}` or `export {name as alias}`
            let result =
                self.transform_binary_expression_to_export_declaration(node, export_name)?;
            self.expando_members
                .entry(host_id)
                .or_default()
                .push(result);
            return Ok(());
        }

        let preexisting_expando_has_export =
            self.expando_members.get(&host_id).is_some_and(|members| {
                members
                    .iter()
                    .any(|member| self.kind(*member) == K::ExportDeclaration)
            });
        let var_modifiers =
            preexisting_expando_has_export.then(|| self.modifier_list_from_flags(mf::EXPORT));

        let _ = name;
        let local_text = self.node_text(local_name)?;
        let synthesized_namespace = self.resolver.create_expando_namespace_scope(
            self.enclosing_declaration,
            name_text,
            host,
            local_text.clone(),
            symbol,
        )?;

        let old_enclosing = self.enclosing_declaration;
        self.enclosing_declaration = synthesized_namespace;
        let ty = self.ensure_type(node, false);
        self.enclosing_declaration = old_enclosing;
        let ty = ty?;

        let var_decl = self.output.new_variable_declaration(
            Some(local_name),
            None, /*exclamationToken*/
            ty,
            None, /*initializer*/
        );
        let declarations = self.new_node_list(vec![var_decl]);
        let declaration_list = self
            .output
            .new_variable_declaration_list(Some(declarations), tsr_ast::node_flags::NONE);
        let mut statements = vec![self
            .output
            .new_variable_statement(var_modifiers, Some(declaration_list))];

        if local_text != self.node_text(export_name)? {
            statements.push(self.new_named_export_declaration(Some(local_name), export_name));
        }

        if statements.len() > 1 && !preexisting_expando_has_export {
            // Add an `export` modifier to all existing expando members so they remain exported after the `export {}` is added
            for decl in self
                .expando_members
                .get(&host_id)
                .cloned()
                .unwrap_or_default()
            {
                let modifier_flags = mf::EXPORT | self.combined_modifier_flags(decl)?;
                let modifiers = self.modifier_list_from_flags(modifier_flags);
                Factory::node_mut(&mut *self.output, decl).set_modifiers(Some(modifiers));
            }
        }
        self.expando_members
            .entry(host_id)
            .or_default()
            .extend(statements);
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.getExpandoHostId
    fn get_expando_host_id(&self, declaration: NodeId) -> NodeId {
        let root = self.expando_host_root(declaration);
        self.most_original(root)
    }
    /// `core.IfElse(ast.IsVariableDeclaration(declaration), declaration.Parent.Parent, declaration)`.
    fn expando_host_root(&self, declaration: NodeId) -> NodeId {
        if self.kind(declaration) == K::VariableDeclaration {
            let list = self.parent(declaration).expect(NIL);
            self.parent(list).expect(NIL)
        } else {
            declaration
        }
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformExpandoHost
    fn transform_expando_host(
        &mut self,
        name: NodeId,
        declaration: NodeId,
    ) -> Result<(), R::Error> {
        let root = self.expando_host_root(declaration);
        let id = self.get_expando_host_id(declaration);

        if self.expando_hosts.contains_key(&id) {
            return Ok(());
        }

        let save_needs_declare = self.needs_declare;
        self.needs_declare = true;

        let modifier_flags = self.ensure_modifier_flags(root);
        self.needs_declare = save_needs_declare;
        let mut modifier_flags = modifier_flags?;
        let default_export = modifier_flags & mf::EXPORT != 0 && modifier_flags & mf::DEFAULT != 0;

        if default_export {
            modifier_flags |= mf::AMBIENT;
            modifier_flags ^= mf::DEFAULT;
            modifier_flags ^= mf::EXPORT;
        }

        let (_, cleanup_diagnostic_context) = self.setup_diagnostic_context(declaration)?;
        let result = self.transform_expando_host_worker(
            name,
            declaration,
            id,
            modifier_flags,
            default_export,
        );
        self.cleanup_diagnostic_context(cleanup_diagnostic_context);
        result
    }
    fn transform_expando_host_worker(
        &mut self,
        name: NodeId,
        declaration: NodeId,
        id: NodeId,
        modifier_flags: u32,
        default_export: bool,
    ) -> Result<(), R::Error> {
        let modifiers = self.modifier_list_from_flags(modifier_flags);
        let mut replacement = Vec::new();

        if self.kind(declaration) == K::FunctionDeclaration {
            let (type_parameters, parameters, asterisk_token) =
                self.extract_expando_host_params(declaration);
            let declaration_name = self.node(declaration).name();
            let type_parameters = self.ensure_type_params(declaration, type_parameters)?;
            let parameters = self.update_param_list(declaration, parameters)?;
            let ty = self.ensure_type(declaration, false)?;
            replacement.push(self.output.update_function_declaration(
                declaration,
                Some(modifiers),
                asterisk_token,
                declaration_name,
                type_parameters,
                Some(parameters),
                ty,
                None, /*fullSignature*/
                None, /*body*/
            ));
        } else if self.kind(declaration) == K::VariableDeclaration
            && self
                .node(declaration)
                .initializer()
                .is_some_and(|initializer| {
                    tsr_ast::utilities::is_function_expression_or_arrow_function(
                        &self.node(initializer),
                    )
                })
        {
            let fn_ = self.node(declaration).initializer().expect(NIL);
            let (type_parameters, parameters, asterisk_token) =
                self.extract_expando_host_params(fn_);
            let text = self.node_text(name)?;
            let function_name = self.output.new_identifier(text);
            let type_parameters = self.ensure_type_params(fn_, type_parameters)?;
            let parameters = self.update_param_list(fn_, parameters)?;
            let ty = self.ensure_type(fn_, false)?;
            replacement.push(self.output.new_function_declaration(
                Some(modifiers),
                asterisk_token,
                Some(function_name),
                type_parameters,
                Some(parameters),
                ty,
                None, /*fullSignature*/
                None, /*body*/
            ));
        } else {
            let result = self.transform_top_level_declaration(declaration)?;
            self.expando_hosts.insert(id, result);
            return Ok(());
        }

        self.report_expando_function_errors(declaration)?;

        if default_export {
            if self.parent_kind(declaration) == K::SourceFile {
                self.result_has_external_module_indicator = true;
            }
            self.result_has_scope_marker = true;
            replacement.push(self.output.new_export_assignment(
                None,  /*modifiers*/
                false, /*isExportEquals*/
                None,  /*typeNode*/
                Some(name),
            ));
        }

        // store host result to be added to the output when it's actually visited
        let host = self.new_syntax_list(replacement);
        self.expando_hosts.insert(id, Some(host));
        if self.late_statement_replacement_map.contains_key(&id) {
            let block = self.create_full_expando_block(id)?;
            self.late_statement_replacement_map.insert(id, block);
        }
        Ok(())
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.createFullExpandoBlock
    pub fn create_full_expando_block(&mut self, id: NodeId) -> Result<Option<NodeId>, R::Error> {
        // Process any expando assignments on this host that were skipped because it wasn't
        // visible when they were collected - if it's still not visible, they simply get
        // re-deferred, and are dropped if the host is never late-marked visible.
        if let Some(deferred) = self.deferred_expando_assignments.remove(&id) {
            for assignment in deferred {
                self.transform_expando_assignment(assignment)?;
            }
        }
        let n = self.expando_hosts.get(&id).copied().flatten();
        if let Some(add_ons) = self.expando_members.get(&id).cloned() {
            let mut modifiers = None;
            let mut name = None;
            let mut host = Vec::new();
            if let Some(n) = n {
                if self.kind(n) == K::SyntaxList {
                    // find the first named syntax list element and use its' name & modifiers
                    let children = self.node_or_syntax_list_children(n);
                    for &c in &children {
                        if let Some(c_name) = self.node(c).name() {
                            name = Some(tsr_ast::clone_node(&mut *self.output, c_name));
                            if let Some(c_modifiers) = self.node(c).modifiers() {
                                modifiers =
                                    Some(self.output.clone_modifier_list_header(c_modifiers));
                            }
                            break;
                        }
                    }
                    host = children;
                } else {
                    let n_name = self.node(n).name().expect(NIL);
                    name = Some(tsr_ast::clone_node(&mut *self.output, n_name));
                    if let Some(n_modifiers) = self.node(n).modifiers() {
                        modifiers = Some(self.output.clone_modifier_list_header(n_modifiers));
                    }
                    host = vec![n];
                }
            }
            if let Some(name) = name {
                let statements = self.new_node_list(add_ons);
                let body = self.output.new_module_block(Some(statements));
                let module_decl = self.output.new_module_declaration(
                    modifiers,
                    K::NamespaceKeyword.into(),
                    Some(name),
                    None,
                    Some(body),
                );
                let mut members = host;
                members.push(module_decl);
                return Ok(Some(self.new_syntax_list(members)));
            }
        }
        Ok(n)
    }

    // port: tsc/internal/transformers/declarations/transform.go:extractExpandoHostParams
    fn extract_expando_host_params(
        &self,
        node: NodeId,
    ) -> (Option<NodeListId>, Option<NodeListId>, Option<NodeId>) {
        let read = self.node(node);
        match read.kind().known() {
            Some(K::FunctionExpression) => {
                let fn_ = read
                    .as_function_expression()
                    .expect("function expression payload");
                (
                    fn_.type_parameters(),
                    fn_.parameters(),
                    fn_.asterisk_token(),
                )
            }
            Some(K::ArrowFunction) => {
                let fn_ = read.as_arrow_function().expect("arrow function payload");
                (fn_.type_parameters(), fn_.parameters(), None)
            }
            _ => {
                let fn_ = read
                    .as_function_declaration()
                    .expect("function declaration payload");
                (
                    fn_.type_parameters(),
                    fn_.parameters(),
                    fn_.asterisk_token(),
                )
            }
        }
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.tryGetPropertyName
    fn try_get_property_name(&mut self, node: NodeId) -> Result<JsString, R::Error> {
        if self.kind(node) == K::ElementAccessExpression {
            return self.resolver.element_access_expression_name(node);
        }
        if self.kind(node) == K::PropertyAccessExpression {
            return self.node_text(self.node(node).name().expect(NIL));
        }
        Ok(JsString::default())
    }
}

/// `ast.IsNonContextualKeyword(scanner.StringToToken(text))`.
fn is_non_contextual_keyword(text: &[u8]) -> bool {
    tsr_ast::utilities_tail::is_non_contextual_keyword(tsr_scanner::string_to_token(text).into())
}
