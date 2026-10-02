//! The top-level statement transforms of `transform.go`.
use super::{
    transform::{Transformer, NIL},
    util,
};
use tsr_ast::{
    modifier_flags as mf, node_flags as nf, Factory, FactoryMethods, JsString, NodeId, NodeListId,
    RuntimeFactory, SyntaxKind as K,
};
use tsr_printer::emit_resolver::{ConstantValue, DeclarationEmitResolver};

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformTypeAliasDeclaration
    pub fn transform_type_alias_declaration(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        self.needs_declare = false;
        let (name, type_parameters, ty) = {
            let read = self.node(input);
            let data = read
                .as_type_alias_declaration()
                .expect("type alias payload");
            (read.name(), data.type_parameters(), data.r#type())
        };
        let modifiers = self.ensure_modifiers(input)?;
        let type_parameters = self.visit_nodes(type_parameters)?;
        let ty = self.visit(ty)?;
        Ok(self
            .output
            .update_type_alias_declaration(input, modifiers, name, type_parameters, ty))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformInterfaceDeclaration
    pub fn transform_interface_declaration(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        let (name, type_parameters, heritage_clauses, members) = {
            let read = self.node(input);
            let data = read.as_interface_declaration().expect("interface payload");
            (
                read.name(),
                data.type_parameters(),
                data.heritage_clauses(),
                data.members(),
            )
        };
        let modifiers = self.ensure_modifiers(input)?;
        let type_parameters = self.visit_nodes(type_parameters)?;
        let heritage_clauses = self.visit_nodes(heritage_clauses)?;
        let members = self.visit_nodes(members)?;
        Ok(self.output.update_interface_declaration(
            input,
            modifiers,
            name,
            type_parameters,
            heritage_clauses,
            members,
        ))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformFunctionDeclaration
    pub fn transform_function_declaration(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        if self.resolver.expando_function_declaration(input)? {
            self.report_expando_function_errors(input)?;
        }
        let (name, type_parameters, parameters) = {
            let read = self.node(input);
            (
                read.name(),
                read.type_parameter_list(),
                read.parameter_list(),
            )
        };
        let modifiers = self.ensure_modifiers(input)?;
        let type_parameters = self.ensure_type_params(input, type_parameters)?;
        let parameters = self.update_param_list(input, parameters)?;
        let ty = self.ensure_type(input, false)?;
        Ok(self.output.update_function_declaration(
            input,
            modifiers,
            None,
            name,
            type_parameters,
            Some(parameters),
            ty,
            None, /*fullSignature*/
            None,
        ))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformModuleDeclaration
    pub fn transform_module_declaration(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        // !!! TODO: module declarations are now parsed into nested module objects with export modifiers
        // It'd be good to collapse those back in the declaration output, but the AST can't represent the
        // `namespace a.b.c` shape for the printer (without using invalid identifier names).
        let mods = self.ensure_modifiers(input)?;
        let save_needs_declare = self.needs_declare;
        self.needs_declare = false;
        let (name, inner, keyword, attributes) = {
            let read = self.node(input);
            let data = read.as_module_declaration().expect("module payload");
            (read.name(), data.body(), data.keyword(), data.attributes())
        };
        let mut keyword = keyword;
        if keyword != K::GlobalKeyword
            && name.is_none_or(|name| self.kind(name) != K::StringLiteral)
        {
            keyword = K::NamespaceKeyword.into();
        }
        let attributes = self.visit(attributes)?;

        if let Some(inner) = inner.filter(|inner| self.kind(*inner) == K::ModuleBlock) {
            let old_needs_scope_fix = self.needs_scope_fix_marker;
            let old_has_scope_fix = self.result_has_scope_marker;
            self.result_has_scope_marker = false;
            self.needs_scope_fix_marker = false;
            let statement_list = self.node(inner).statement_list();
            let statements = self.visit_nodes(statement_list)?.expect(NIL);
            let mut late_statements =
                self.transform_and_replace_late_painted_statements(statements)?;
            if self.node(input).flags() & nf::AMBIENT != 0 {
                self.needs_scope_fix_marker = false; // If it was `declare`'d everything is implicitly exported already, ignore late printed "privates"
            }
            // With the final list of statements, there are 3 possibilities:
            // 1. There's an export assignment or export declaration in the namespace - do nothing
            // 2. Everything is exported and there are no export assignments or export declarations - strip all export modifiers
            // 3. Some things are exported, some are not, and there's no marker - add an empty marker
            if !tsr_ast::utilities::is_global_scope_augmentation(&self.node(input))
                && !self.result_has_scope_marker
                && !util::has_scope_marker(self.view(), &self.list_nodes(Some(late_statements)))?
            {
                if self.needs_scope_fix_marker {
                    let mut nodes = self.list_nodes(Some(late_statements));
                    let marker = self.create_empty_exports();
                    nodes.push(marker);
                    late_statements = self.new_node_list(nodes);
                } else {
                    let nodes = self.list_nodes(Some(late_statements));
                    let mut stripped = Vec::with_capacity(nodes.len());
                    let mut changed = false;
                    for statement in &nodes {
                        let result = self.strip_export_modifiers(Some(*statement))?;
                        match result {
                            Some(result) => {
                                changed |= result != *statement;
                                stripped.extend(self.node_or_syntax_list_children(result));
                            }
                            None => changed = true,
                        }
                    }
                    if changed {
                        let loc = self.output.read_list(late_statements).loc();
                        late_statements = self.new_node_list(stripped);
                        self.output.set_list_location(late_statements, loc)?;
                    }
                }
            }
            let body = self
                .output
                .update_module_block(inner, Some(late_statements));
            self.needs_declare = save_needs_declare;
            self.needs_scope_fix_marker = old_needs_scope_fix;
            self.result_has_scope_marker = old_has_scope_fix;
            return Ok(self.output.update_module_declaration(
                input,
                mods,
                keyword,
                name,
                attributes,
                Some(body),
            ));
        }
        if let Some(inner) = inner {
            // trigger visit. ignore result (is deferred, so is just inner unless elided)
            self.visit(Some(inner))?;
            // eagerly transform nested namespaces (the nesting doesn't need any elision or painting done)
            let original = self.most_original(inner);
            let body = self
                .late_statement_replacement_map
                .remove(&original)
                .flatten();
            return Ok(self
                .output
                .update_module_declaration(input, mods, keyword, name, attributes, body));
        }
        Ok(self
            .output
            .update_module_declaration(input, mods, keyword, name, attributes, None))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.stripExportModifiers
    fn strip_export_modifiers(
        &mut self,
        statement: Option<NodeId>,
    ) -> Result<Option<NodeId>, R::Error> {
        let Some(statement) = statement else {
            return Ok(None);
        };
        let parse_node = self.parse_node(statement);
        let is_default = match parse_node {
            Some(parse_node) => {
                self.resolver
                    .effective_declaration_flags(parse_node, mf::DEFAULT)?
                    != 0
            }
            None => false,
        };
        if self.kind(statement) == K::ImportEqualsDeclaration
            || is_default
            || !tsr_ast::utilities::can_have_modifiers(&self.node(statement))
        {
            // `export import` statements should remain as-is, as imports are _not_ implicitly exported in an ambient namespace
            // Likewise, `export default` classes and the like and just be `default`, so we preserve their `export` modifiers, too
            return Ok(Some(statement));
        }
        let old_flags = self.combined_modifier_flags(statement)?;
        if old_flags & mf::EXPORT == 0 {
            return Ok(Some(statement));
        }
        let new_flags = old_flags & (mf::ALL ^ mf::EXPORT);
        let modifiers = self.create_modifiers_from_modifier_flags(new_flags);
        let modifiers = self.new_modifier_list(modifiers);
        Ok(Some(tsr_ast::utilities_class::replace_modifiers(
            &mut *self.output,
            statement,
            Some(modifiers),
        )))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformVariableStatement
    pub fn transform_variable_statement(
        &mut self,
        input: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let declaration_list = self
            .node(input)
            .as_variable_statement()
            .expect("variable statement payload")
            .declaration_list()
            .expect(NIL);
        let declarations = self.list_nodes(
            self.node(declaration_list)
                .as_variable_declaration_list()
                .expect("declaration list payload")
                .declarations(),
        );
        let mut visible = false;
        for &decl in &declarations {
            visible = util::get_binding_name_visible(self, decl)?;
            if visible {
                break;
            }
        }
        if !visible {
            return Ok(None);
        }
        let mut input_nodes = declarations;
        let mut extra_imports = Vec::new();
        if self
            .output
            .read_source_file(self.current_source_file)?
            .common_js_module_indicator()
            .is_some()
        {
            let mut normal_declarations = Vec::new();
            let mut imports = Vec::new();
            for n in input_nodes {
                if tsr_ast::is_variable_declaration_initialized_to_require(self.view(), n)? {
                    imports.push(n);
                } else {
                    normal_declarations.push(n);
                }
            }
            input_nodes = normal_declarations;
            extra_imports = self.visit_slice(&imports)?.0;
        }
        let nodes = self.visit_slice(&input_nodes)?.0;
        if nodes.is_empty() {
            if !extra_imports.is_empty() {
                return Ok(Some(self.new_syntax_list(extra_imports)));
            }
            return Ok(None);
        }
        let node_list = self.new_node_list(nodes);
        let modifiers = self.ensure_modifiers(input)?;
        let decl_list = if tsr_ast::utilities::is_var_using(self.view(), declaration_list)?
            || tsr_ast::utilities::is_var_await_using(self.view(), declaration_list)?
        {
            let decl_list = self
                .output
                .new_variable_declaration_list(Some(node_list), nf::CONST);
            self.emit.set_original(decl_list, declaration_list);
            let loc = {
                let read = self.node(declaration_list);
                tsr_core::TextRange::new(i64::from(read.pos()), i64::from(read.end()))
            };
            self.emit.set_comment_range(decl_list, loc);
            Factory::node_mut(&mut *self.output, decl_list).set_range(loc);
            decl_list
        } else {
            let flags = self.node(declaration_list).flags();
            self.output
                .update_variable_declaration_list(declaration_list, Some(node_list), flags)
        };
        let res = self
            .output
            .update_variable_statement(input, modifiers, Some(decl_list));
        if !extra_imports.is_empty() {
            extra_imports.push(res);
            return Ok(Some(self.new_syntax_list(extra_imports)));
        }
        Ok(Some(res))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformEnumDeclaration
    pub fn transform_enum_declaration(&mut self, input: NodeId) -> Result<NodeId, R::Error> {
        let (name, members) = {
            let read = self.node(input);
            let data = read.as_enum_declaration().expect("enum payload");
            (read.name(), data.members())
        };
        let modifiers = self.ensure_modifiers(input)?;
        let mut results = Vec::new();
        for m in self.list_nodes(members) {
            if self.should_strip_internal(Some(m))? {
                continue;
            }
            // Rewrite enum values to their constants, if available
            let enum_value = self.resolver.enum_member_value(m)?;
            let member_name = self.node(m).name();
            if self.isolated_declarations
                && self.node(m).initializer().is_some()
                && enum_value.has_external_references
                // This will be its own compiler error instead, so don't report.
                && member_name.is_none_or(|name| self.kind(name) != K::ComputedPropertyName)
            {
                self.add_diagnostic_for_node(
                    m,
                    tsr_diagnostics::Enum_member_initializers_must_be_computable_without_references_to_external_symbols_with_isolatedDeclarations,
                    vec![],
                )?;
            }
            let new_initializer = match enum_value.value {
                Some(ConstantValue::Number(value)) => Some(self.enum_number_initializer(value)),
                Some(ConstantValue::String(value)) => {
                    Some(self.output.new_string_literal(value, 0))
                }
                // nil
                None => None,
            };
            let result = self
                .output
                .update_enum_member(m, member_name, new_initializer);
            self.preserve_js_doc(result, m);
            results.push(result);
        }
        let members = self.new_node_list(results);
        Ok(self
            .output
            .update_enum_declaration(input, modifiers, name, Some(members)))
    }
    /// The `jsnum.Number` arm of `transformEnumDeclaration`.
    fn enum_number_initializer(&mut self, value: tsr_jsnum::Number) -> NodeId {
        let number = value.value();
        if number.is_infinite() {
            let infinity = self
                .output
                .new_identifier(JsString::from_bytes(b"Infinity".as_slice()));
            if number > 0.0 {
                infinity
            } else {
                self.output
                    .new_prefix_unary_expression(K::MinusToken.into(), Some(infinity))
            }
        } else if number.is_nan() {
            self.output
                .new_identifier(JsString::from_bytes(b"NaN".as_slice()))
        } else if number >= 0.0 {
            self.output
                .new_numeric_literal(JsString::from_bytes(value.to_string().as_bytes()), 0)
        } else {
            let negated = tsr_jsnum::Number::new(-number);
            let literal = self
                .output
                .new_numeric_literal(JsString::from_bytes(negated.to_string().as_bytes()), 0);
            self.output
                .new_prefix_unary_expression(K::MinusToken.into(), Some(literal))
        }
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.ensureModifiers
    pub fn ensure_modifiers(&mut self, node: NodeId) -> Result<Option<NodeListId>, R::Error> {
        let parse_node = self.parse_node(node).expect(NIL);
        let current_flags = self.combined_modifier_flags(parse_node)? & mf::ALL;
        let new_flags = self.ensure_modifier_flags(node)?;
        if current_flags == new_flags {
            // Elide decorators
            let Some(mods) = self.node(node).modifiers() else {
                return Ok(None);
            };
            let nodes = self.list_nodes(Some(mods));
            if util::can_reuse_modifier_nodes(self.view(), &nodes)? {
                let modifiers: Vec<_> = nodes
                    .into_iter()
                    .filter(|node| tsr_ast::utilities::is_modifier(&self.node(*node)))
                    .collect();
                return Ok(Some(self.new_modifier_list(modifiers)));
            }
        }
        let result = self.create_modifiers_from_modifier_flags(new_flags);
        if result.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.new_modifier_list(result)))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.ensureModifierFlags
    pub fn ensure_modifier_flags(&mut self, node: NodeId) -> Result<u32, R::Error> {
        let mut mask = mf::ALL ^ (mf::PUBLIC | mf::ASYNC | mf::OVERRIDE); // No async and override modifiers in declaration files
        let mut additions = mf::NONE;
        if self.needs_declare && !util::is_always_type(&self.node(node)) {
            additions = mf::AMBIENT;
        }
        let parent_is_file = self.parent_kind(node) == K::SourceFile;
        if !parent_is_file {
            mask ^= mf::AMBIENT;
            additions = mf::NONE;
        }
        if self.is_implicitly_exported_js_doc_declaration(node)? {
            additions |= mf::EXPORT;
        }
        Ok(util::mask_modifier_flags(
            self.view(),
            node,
            mask,
            additions,
        )?)
    }

    /// `ast.IsImplicitlyExportedJSDocDeclaration`. The file is read through
    /// the output builder, which selects the file's binding: the binder sets
    /// the CommonJS indicator, and the builder's own view does not carry it.
    fn is_implicitly_exported_js_doc_declaration(&self, node: NodeId) -> Result<bool, R::Error> {
        let parent = self.parent(node).expect(NIL);
        if self.kind(parent) != K::SourceFile
            || !tsr_ast::utilities::is_external_or_common_js_module(
                &self.output.read_source_file(parent)?,
            )
        {
            return Ok(false);
        }
        let read = self.node(node);
        // A reparsed ModuleDeclaration synthesized from a JSDoc @typedef/@callback
        // dotted name should also be treated as implicitly exported in modules.
        Ok(read.kind() == K::JSTypeAliasDeclaration
            || read.kind() == K::ModuleDeclaration
                && read.flags() & tsr_ast::node_flags::REPARSED != 0)
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformImportEqualsDeclaration
    pub fn transform_import_equals_declaration(
        &mut self,
        decl: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        if !self.resolver.is_declaration_visible(decl)? {
            return Ok(None);
        }
        let (modifiers, is_type_only, name, module_reference) = {
            let read = self.node(decl);
            let data = read
                .as_import_equals_declaration()
                .expect("import equals payload");
            (
                read.modifiers(),
                data.is_type_only(),
                read.name(),
                data.module_reference().expect(NIL),
            )
        };
        if self.kind(module_reference) == K::ExternalModuleReference {
            // Rewrite external module names if necessary
            let specifier =
                tsr_ast::utilities_modules::get_external_module_import_equals_declaration_expression(
                    self.view(),
                    decl,
                )?;
            let specifier = self.rewrite_module_specifier(decl, specifier);
            let reference = self
                .output
                .update_external_module_reference(module_reference, specifier);
            return Ok(Some(self.output.update_import_equals_declaration(
                decl,
                modifiers,
                is_type_only,
                name,
                Some(reference),
            )));
        }
        let old_diag = self.tracker.selector.clone();
        self.set_diagnostic_context_for_node(decl)?;
        let result =
            self.check_entity_name_visibility(module_reference, self.enclosing_declaration);
        self.tracker.selector = old_diag;
        result?;
        Ok(Some(decl))
    }

    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformImportDeclaration
    pub fn transform_import_declaration(
        &mut self,
        decl: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let (modifiers, import_clause, module_specifier, attributes) = {
            let read = self.node(decl);
            let data = read
                .as_import_declaration()
                .expect("import declaration payload");
            (
                read.modifiers(),
                data.import_clause(),
                data.module_specifier(),
                data.attributes(),
            )
        };
        let Some(import_clause) = import_clause else {
            // import "mod" - possibly needed for side effects? (global interface patches, module augmentations, etc)
            let specifier = self.rewrite_module_specifier(decl, module_specifier);
            return Ok(Some(self.output.update_import_declaration(
                decl, modifiers, None, specifier, attributes,
            )));
        };
        let (mut phase_modifier, clause_name, named_bindings) = {
            let read = self.node(import_clause);
            let data = read.as_import_clause().expect("import clause payload");
            (data.phase_modifier(), read.name(), data.named_bindings())
        };
        if phase_modifier == K::DeferKeyword {
            phase_modifier = K::Unknown.into();
        }
        // The `importClause` visibility corresponds to the default's visibility.
        let visible_default_binding =
            if clause_name.is_some() && self.resolver.is_declaration_visible(import_clause)? {
                clause_name
            } else {
                None
            };
        let Some(named_bindings) = named_bindings else {
            // No named bindings (either namespace or list), meaning the import is just default or should be elided
            if visible_default_binding.is_none() {
                return Ok(None);
            }
            let clause = self.output.update_import_clause(
                import_clause,
                phase_modifier,
                visible_default_binding,
                None, /*namedBindings*/
            );
            let specifier = self.rewrite_module_specifier(decl, module_specifier);
            return Ok(Some(self.output.update_import_declaration(
                decl,
                modifiers,
                Some(clause),
                specifier,
                attributes,
            )));
        };
        if self.kind(named_bindings) == K::NamespaceImport {
            // Namespace import (optionally with visible default)
            let named = if self.resolver.is_declaration_visible(named_bindings)? {
                Some(named_bindings)
            } else {
                None
            };
            if visible_default_binding.is_none() && named.is_none() {
                return Ok(None);
            }
            let clause = self.output.update_import_clause(
                import_clause,
                phase_modifier,
                visible_default_binding,
                named,
            );
            let specifier = self.rewrite_module_specifier(decl, module_specifier);
            return Ok(Some(self.output.update_import_declaration(
                decl,
                modifiers,
                Some(clause),
                specifier,
                attributes,
            )));
        }
        // Named imports (optionally with visible default)
        let mut binding_list = Vec::new();
        for b in self.list_nodes(self.node(named_bindings).element_list()) {
            if self.resolver.is_declaration_visible(b)? {
                binding_list.push(b);
            }
        }
        if !binding_list.is_empty() || visible_default_binding.is_some() {
            let named_imports = if binding_list.is_empty() {
                None
            } else {
                let list = self.new_node_list(binding_list);
                Some(self.output.update_named_imports(named_bindings, Some(list)))
            };
            let clause = self.output.update_import_clause(
                import_clause,
                phase_modifier,
                visible_default_binding,
                named_imports,
            );
            let specifier = self.rewrite_module_specifier(decl, module_specifier);
            return Ok(Some(self.output.update_import_declaration(
                decl,
                modifiers,
                Some(clause),
                specifier,
                attributes,
            )));
        }
        // Augmentation of export depends on import
        if self.resolver.import_required_by_augmentation(decl)? {
            if self.isolated_declarations {
                self.add_diagnostic_for_node(
                    decl,
                    tsr_diagnostics::Declaration_emit_for_this_file_requires_preserving_this_import_for_augmentations_This_is_not_supported_with_isolatedDeclarations,
                    vec![],
                )?;
            }
            let specifier = self.rewrite_module_specifier(decl, module_specifier);
            return Ok(Some(self.output.update_import_declaration(
                decl, modifiers, None, /*importClause*/
                specifier, attributes,
            )));
        }
        // Nothing visible
        Ok(None)
    }
}
