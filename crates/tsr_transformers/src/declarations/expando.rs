use super::transform::Transformer;
use ts_ast::{
    modifier_flags as mf, symbol_flags as sf, Factory, FactoryMethods, JsString, NodeId,
    NodeListId, RuntimeFactory, SyntaxKind as K,
};
use ts_printer::{emit_resolver::DeclarationEmitResolver, AutoGenerateOptions};

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    fn expando_host_root(&self, declaration: NodeId) -> Result<NodeId, R::Error> {
        let root = if self.node(declaration).kind() == K::VariableDeclaration {
            let list = Self::required(self.node(declaration).parent())?;
            Self::required(self.node(list).parent())?
        } else {
            declaration
        };
        Ok(self.emit.most_original(root))
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformExpandoAssignment
    pub fn expando_assignment(&mut self, node: NodeId) -> Result<(), R::Error> {
        let Some(symbol) = self.resolver.bound_symbol_of_declaration(node)? else {
            return Ok(());
        };
        if self.resolver.symbol_flags(symbol)? & sf::ASSIGNMENT == 0 {
            return Ok(());
        }
        let left = Self::required(self.node(node).as_binary_expression().unwrap().left())?;
        let namespace = ts_ast::get_leftmost_access_expression(self.output.view(), left)?;
        if self.node(namespace).kind() != K::Identifier {
            return Ok(());
        }
        let Some(declaration) = self.resolver.referenced_value_declaration(namespace)? else {
            return Ok(());
        };
        if self.strip_internal(declaration)? {
            return Ok(());
        }
        if self.node(declaration).kind() == K::VariableDeclaration
            && (self.node(declaration).type_node().is_some()
                || !self
                    .node(declaration)
                    .initializer()
                    .is_some_and(|initializer| {
                        ts_ast::utilities::is_function_like(Some(&self.node(initializer)))
                    }))
        {
            return Ok(());
        }
        if self.node(declaration).kind() == K::FunctionDeclaration
            && self
                .node(declaration)
                .as_function_declaration()
                .unwrap()
                .full_signature()
                .is_some()
        {
            return Ok(());
        }
        let Some(host) = self.resolver.bound_symbol_of_declaration(declaration)? else {
            return Ok(());
        };
        let namespace_text = self.output.view().node_text(namespace)?.into_js_string();
        let name = self.output.new_identifier(namespace_text.clone());
        let property = match self.node(left).kind().known() {
            Some(K::ElementAccessExpression) => {
                self.resolver.element_access_expression_name(left)?
            }
            Some(K::PropertyAccessExpression) => self
                .output
                .view()
                .node_text(Self::required(self.node(left).name())?)?
                .into_js_string(),
            _ => JsString::default(),
        };
        if property.is_empty()
            || !ts_scanner::is_identifier_text(
                property.as_bytes(),
                ts_core::LanguageVariant::STANDARD,
            )
        {
            return Ok(());
        }
        let host_id = self.expando_host_root(declaration)?;
        if self.declaration_not_visible(declaration)? {
            self.cjs
                .deferred_expando
                .entry(host_id)
                .or_default()
                .push(node);
            return Ok(());
        }
        if self.node(declaration).kind() == K::FunctionDeclaration
            && self.node(declaration).body().is_none()
        {
            let declarations = self.resolver.symbol_declarations(host)?;
            if !declarations.into_iter().any(|declaration| {
                self.node(declaration).kind() == K::FunctionDeclaration
                    && self.node(declaration).body().is_some()
            }) {
                return Ok(());
            }
        }
        self.expando_host(name, declaration)?;
        let export_name = self.output.new_identifier(property.clone());
        let mut local_name = self.try_assigned_expression_name(node)?;
        if local_name.is_none()
            && !self
                .resolver
                .name_resolvable(self.enclosing, property.as_bytes())?
            && !non_contextual_keyword(property.as_bytes())
        {
            local_name = Some(export_name);
        }
        let usable = match local_name {
            Some(local) => !non_contextual_keyword(self.output.view().node_text(local)?.as_bytes()),
            None => false,
        };
        let local_name = if usable {
            local_name.expect("usable name is present")
        } else {
            self.emit.new_generated_name_for_node_ex(
                self.output,
                node,
                AutoGenerateOptions::default(),
            )
        };
        let context = self.save_expression_context(node)?;
        let result = (|| {
            let right = Self::required(self.node(node).as_binary_expression().unwrap().right())?;
            if self.node(right).kind() == K::Identifier {
                let export = self.binary_export(node, export_name)?;
                self.cjs
                    .expando_members
                    .entry(host_id)
                    .or_default()
                    .push(export);
                return Ok(());
            }
            let had_exports = self
                .cjs
                .expando_members
                .get(&host_id)
                .is_some_and(|members| {
                    members
                        .iter()
                        .any(|node| self.node(*node).kind() == K::ExportDeclaration)
                });
            let modifiers = self.modifier_list(if had_exports { mf::EXPORT } else { 0 });
            let local_text = self.output.view().node_text(local_name)?.into_js_string();
            let namespace = self.resolver.create_expando_namespace_scope(
                self.enclosing,
                namespace_text,
                host,
                local_text.clone(),
                symbol,
            )?;
            let enclosing = self.enclosing;
            self.enclosing = namespace;
            let ty = self.ensure_type(node, false);
            self.enclosing = enclosing;
            let ty = ty?;
            let declaration =
                self.output
                    .new_variable_declaration(Some(local_name), None, ty, None);
            let declarations = self.new_list(vec![declaration]);
            let list = self
                .output
                .new_variable_declaration_list(Some(declarations), 0);
            let statement = self.output.new_variable_statement(modifiers, Some(list));
            let mut statements = vec![statement];
            if local_text.as_bytes() != property.as_bytes() {
                statements.push(self.named_export(Some(local_name), export_name));
            }
            if statements.len() > 1 && !had_exports {
                let existing = self
                    .cjs
                    .expando_members
                    .remove(&host_id)
                    .unwrap_or_default();
                let mut updated = Vec::with_capacity(existing.len());
                for node in existing {
                    let flags =
                        ts_ast::utilities::get_combined_modifier_flags(self.output.view(), node)?
                            | mf::EXPORT;
                    let modifiers = self.modifier_list(flags);
                    updated.push(self.replace_top_level_modifiers(node, modifiers)?);
                }
                self.cjs.expando_members.insert(host_id, updated);
            }
            self.cjs
                .expando_members
                .entry(host_id)
                .or_default()
                .extend(statements);
            Ok(())
        })();
        self.restore_expression_context(context);
        result
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformExpandoHost
    fn expando_host(&mut self, name: NodeId, declaration: NodeId) -> Result<(), R::Error> {
        let root = self.expando_host_root(declaration)?;
        if self.cjs.expando_hosts.contains_key(&root) {
            return Ok(());
        }
        let old_declare = self.needs_declare;
        self.needs_declare = true;
        let modifiers = self.modifiers(root);
        self.needs_declare = old_declare;
        let modifiers = modifiers?;
        let mut flags = modifiers.map_or(0, |list| self.output.read_list(list).modifier_flags());
        let default_export = flags & (mf::EXPORT | mf::DEFAULT) == (mf::EXPORT | mf::DEFAULT);
        if default_export {
            flags = (flags | mf::AMBIENT) & !(mf::EXPORT | mf::DEFAULT);
        }
        let context = self.save_expression_context(declaration)?;
        let result = (|| {
            let modifiers = self.modifier_list(flags);
            let function = if self.node(declaration).kind() == K::FunctionDeclaration {
                declaration
            } else if self.node(declaration).kind() == K::VariableDeclaration {
                Self::required(self.node(declaration).initializer())?
            } else {
                let replacement = self.top_level(declaration)?;
                self.cjs.expando_hosts.insert(root, replacement);
                return Ok(());
            };
            let read = self.node(function);
            let data = read.data_source();
            let asterisk = match read.kind().known() {
                Some(K::FunctionDeclaration) => {
                    data.as_function_declaration().unwrap().asterisk_token()
                }
                Some(K::FunctionExpression) => {
                    data.as_function_expression().unwrap().asterisk_token()
                }
                Some(K::ArrowFunction) => None,
                _ => return Self::unsupported("declaration emit: expando host function kind"),
            };
            drop(read);
            let type_parameters = self.type_parameters(function)?;
            let parameters = self.parameters(function)?;
            let ty = self.ensure_type(function, false)?;
            let statement = if function == declaration {
                self.output.update_function_declaration(
                    declaration,
                    modifiers,
                    asterisk,
                    self.node(declaration).name(),
                    type_parameters,
                    parameters,
                    ty,
                    None,
                    None,
                )
            } else {
                let text = self.output.view().node_text(name)?.into_js_string();
                let name = self.output.new_identifier(text);
                self.output.new_function_declaration(
                    modifiers,
                    asterisk,
                    Some(name),
                    type_parameters,
                    parameters,
                    ty,
                    None,
                    None,
                )
            };
            let mut statements = vec![statement];
            self.expando_errors(declaration)?;
            if default_export {
                if self
                    .node(declaration)
                    .parent()
                    .is_some_and(|parent| self.node(parent).kind() == K::SourceFile)
                {
                    self.external_indicator = true;
                }
                self.has_scope_marker = true;
                statements.push(
                    self.output
                        .new_export_assignment(None, false, None, Some(name)),
                );
            }
            let result = self.syntax_list(statements);
            self.cjs.expando_hosts.insert(root, Some(result));
            if self.replacements.contains_key(&root) {
                let block = self.full_expando_block(root)?;
                self.replacements.insert(root, block);
            }
            Ok(())
        })();
        self.restore_expression_context(context);
        result
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.createFullExpandoBlock
    pub fn full_expando_block(&mut self, root: NodeId) -> Result<Option<NodeId>, R::Error> {
        if let Some(deferred) = self.cjs.deferred_expando.remove(&root) {
            for node in deferred {
                self.expando_assignment(node)?;
            }
        }
        let host = self.cjs.expando_hosts.get(&root).copied().flatten();
        let Some(members) = self.cjs.expando_members.get(&root).cloned() else {
            return Ok(host);
        };
        let Some(host) = host else { return Ok(None) };
        let mut statements = Vec::new();
        self.append_flat(host, &mut statements);
        let Some(named) = statements
            .iter()
            .copied()
            .find(|node| self.node(*node).name().is_some())
        else {
            return Ok(Some(host));
        };
        let name = Self::required(self.node(named).name())?;
        let name = self
            .output
            .clone_node_generated(name)
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let modifiers = self.node(named).modifiers();
        let list = self.new_list(members);
        let body = self.output.new_module_block(Some(list));
        let module = self.output.new_module_declaration(
            modifiers,
            K::NamespaceKeyword.into(),
            Some(name),
            None,
            Some(body),
        );
        statements.push(module);
        Ok(Some(self.syntax_list(statements)))
    }
    pub fn replace_top_level_modifiers(
        &mut self,
        node: NodeId,
        modifiers: Option<NodeListId>,
    ) -> Result<NodeId, R::Error> {
        let mut data = self.node(node).data().to_owned();
        match &mut data {
            ts_ast::NodeData::VariableStatement(d) => d.modifiers = modifiers,
            ts_ast::NodeData::FunctionDeclaration(d) => d.modifiers = modifiers,
            ts_ast::NodeData::InterfaceDeclaration(d) => d.modifiers = modifiers,
            ts_ast::NodeData::TypeAliasDeclaration(d) => d.modifiers = modifiers,
            ts_ast::NodeData::ClassDeclaration(d) => d.modifiers = modifiers,
            ts_ast::NodeData::ModuleDeclaration(d) => d.modifiers = modifiers,
            ts_ast::NodeData::EnumDeclaration(d) => d.modifiers = modifiers,
            _ => return Self::unsupported("declaration emit: replace declaration modifiers"),
        }
        let result = self.output.new_node(self.node(node).kind(), data);
        Ok(self.output.finish_update(result, node))
    }
}
fn non_contextual_keyword(text: &[u8]) -> bool {
    ts_ast::utilities_tail::is_non_contextual_keyword(ts_scanner::string_to_token(text).into())
}
