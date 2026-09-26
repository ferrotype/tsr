use super::{transform::Transformer, util};
use tsr_ast::{
    modifier_flags as mf, node_flags as nf, Factory, FactoryMethods, JsString, NodeId, NodeListId,
    RuntimeFactory, SyntaxKind as K,
};
use tsr_printer::emit_resolver::{ConstantValue, DeclarationEmitResolver};

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformSourceFile
    pub fn source_file(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        self.collect_nested_exports(node, true)?;
        self.collect_nested_exports(node, false)?;
        let data = self
            .node(node)
            .data_source()
            .as_source_file()
            .unwrap()
            .to_owned();
        let statements = self.visit_list_result(data.statements)?;
        let statements = self.late_statements(statements)?;
        let mut combined = Vec::new();
        if let Some(assignment) = self.cjs.assignment {
            self.append_flat(assignment, &mut combined);
        }
        for member in &self.cjs.members {
            self.append_flat(*member, &mut combined);
        }
        combined.extend(self.list_nodes(statements));
        let statements = Some(self.new_list(combined));
        let file = self.output.read_source_file(node)?;
        let external =
            file.external_module_indicator.is_some() || file.common_js_module_indicator().is_some();
        if external && file.is_js() {
            if let Some(symbol) = self.resolver.bound_symbol_of_declaration(node)? {
                if let Some(export) = self
                    .resolver
                    .symbol_export(symbol, tsr_ast::internal_symbol_names::EXPORT_EQUALS)?
                {
                    let declarations = self.resolver.symbol_declarations(export)?;
                    if declarations.len() > 1 {
                        for declaration in declarations {
                            self.diagnostic(declaration, tsr_diagnostics::Multiple_module_exports_assignments_cannot_be_serialized_for_declaration_emit, vec![])?;
                        }
                    }
                }
            }
        }
        let statements = if external
            && (!self.external_indicator || (self.needs_scope_marker && !self.has_scope_marker))
        {
            let mut nodes = self.list_nodes(statements);
            let marker = self.empty_exports();
            nodes.push(marker);
            Some(self.new_list(nodes))
        } else {
            statements
        };
        let result = self
            .output
            .update_source(node, statements, data.end_of_file_token);
        let result = if result == node {
            self.output.clone_source(node)
        } else {
            result
        };
        self.output.mut_source_file(result)?.is_declaration_file = true;
        // Source references are preserved on UpdateSourceFile. Path relocation
        // is a separate output-host operation and is never needed by diagnostics.
        Ok(result)
    }
    pub fn empty_exports(&mut self) -> NodeId {
        let list = self.new_list(Vec::new());
        let exports = self.output.new_named_exports(Some(list));
        self.output
            .new_export_declaration(None, false, Some(exports), None, None)
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visitDeclarationStatements
    pub fn statement(&mut self, node: NodeId) -> Result<Option<NodeId>, R::Error> {
        if self.strip_internal(node)? {
            return Ok(None);
        }
        match self.node(node).kind().known() {
            Some(K::ExportDeclaration) => {
                if self
                    .node(node)
                    .parent()
                    .is_some_and(|p| self.node(p).kind() == K::SourceFile)
                {
                    self.external_indicator = true;
                }
                self.has_scope_marker = true;
                Ok(Some(node))
            }
            Some(K::ExportAssignment) => self.export_assignment(node).map(Some),
            _ => {
                if !self.replacements.contains_key(&node) {
                    let result = self.top_level(node)?;
                    self.replacements.insert(node, result);
                }
                Ok(Some(node))
            }
        }
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformAndReplaceLatePaintedStatements
    pub fn late_statements(
        &mut self,
        statements: Option<NodeListId>,
    ) -> Result<Option<NodeListId>, R::Error> {
        while let Some(node) = self.tracker.late_marked.first().copied() {
            self.tracker.late_marked.remove(0);
            // Accessibility can queue a declaration from a merged namespace
            // in another file. Its syntax must outlive the emitted replacement.
            if self.output.view().node(node).is_err() {
                let source = tsr_ast::utilities::get_source_file_of_node(
                    self.resolver.ast(node)?,
                    Some(node),
                )?
                .ok_or(tsr_arena::Error::InvalidGraph)?;
                self.resolver.retain_source(source, self.output)?;
            }
            let old = self.needs_declare;
            self.needs_declare = self
                .node(node)
                .parent()
                .is_some_and(|p| self.node(p).kind() == K::SourceFile);
            let result = self.top_level(node);
            self.needs_declare = old;
            self.replacements.insert(node, result?);
        }
        let mut result = Vec::new();
        for statement in self.list_nodes(statements) {
            let replacement = self
                .replacements
                .remove(&statement)
                .unwrap_or(Some(statement));
            if let Some(node) = replacement {
                let mut flattened = Vec::new();
                self.append_flat(node, &mut flattened);
                for node in flattened {
                    if self.scope_marker_needed(node)? {
                        self.needs_scope_marker = true;
                    }
                    if self
                        .node(statement)
                        .parent()
                        .is_some_and(|p| self.node(p).kind() == K::SourceFile)
                        && self.external_module_indicator(node)?
                    {
                        self.external_indicator = true;
                    }
                    result.push(node);
                }
            }
        }
        let list = self.new_list(result);
        if let Some(original) = statements {
            self.output
                .set_list_location(list, self.output.read_list(original).loc())?;
        }
        Ok(Some(list))
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformTopLevelDeclaration
    pub fn top_level(&mut self, node: NodeId) -> Result<Option<NodeId>, R::Error> {
        self.tracker.late_marked.retain(|n| *n != node);
        if self.strip_internal(node)? {
            return Ok(None);
        }
        match self.node(node).kind().known() {
            Some(K::ImportEqualsDeclaration) => return self.import_equals(node),
            Some(K::ImportDeclaration | K::JSImportDeclaration) => {
                return self.import_declaration(node)
            }
            _ => {}
        }
        if self.declaration_not_visible(node)? {
            return Ok(None);
        }
        if tsr_ast::utilities::is_function_like(Some(&self.node(node)))
            && self.resolver.implementation_of_overload(node)?
        {
            return Ok(None);
        }
        let original = self.emit.most_original(node);
        if self.cjs.expando_hosts.contains_key(&original)
            || self.cjs.deferred_expando.contains_key(&original)
        {
            return self.full_expando_block(original);
        }
        let old_enclosing = self.enclosing;
        let old_selector = self.tracker.selector.clone();
        let old_name = self.tracker.error_name;
        let old_declare = self.needs_declare;
        if util::is_enclosing_declaration(&self.node(node)) {
            self.enclosing = node;
        }
        if util::can_produce_diagnostics(&self.node(node)) {
            self.select_context(node, false)?;
        }
        let result = self.top_level_worker(node);
        self.enclosing = old_enclosing;
        self.tracker.selector = old_selector;
        self.tracker.error_name = old_name;
        self.needs_declare = old_declare;
        result
    }
    fn top_level_worker(&mut self, node: NodeId) -> Result<Option<NodeId>, R::Error> {
        match self.node(node).kind().known() {
            Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration) => {
                self.needs_declare = false;
                let data = self
                    .node(node)
                    .data_source()
                    .as_type_alias_declaration()
                    .unwrap()
                    .to_owned();
                let modifiers = self.modifiers(node)?;
                let parameters = self.visit_list_result(data.type_parameters)?;
                let ty = self.visit(data.r#type)?;
                Ok(Some(self.output.update_type_alias_declaration(
                    node, modifiers, data.name, parameters, ty,
                )))
            }
            Some(K::InterfaceDeclaration) => {
                let data = self
                    .node(node)
                    .data_source()
                    .as_interface_declaration()
                    .unwrap()
                    .to_owned();
                let modifiers = self.modifiers(node)?;
                let parameters = self.visit_list_result(data.type_parameters)?;
                let heritage = self.visit_list_result(data.heritage_clauses)?;
                let members = self.visit_list_result(data.members)?;
                Ok(Some(self.output.update_interface_declaration(
                    node, modifiers, data.name, parameters, heritage, members,
                )))
            }
            Some(K::FunctionDeclaration) => {
                if self.resolver.expando_function_declaration(node)? {
                    self.expando_errors(node)?;
                }
                let name = self.node(node).name();
                let modifiers = self.modifiers(node)?;
                let parameters = self.parameters(node)?;
                let types = self.type_parameters(node)?;
                let ty = self.ensure_type(node, false)?;
                Ok(Some(self.output.update_function_declaration(
                    node, modifiers, None, name, types, parameters, ty, None, None,
                )))
            }
            Some(K::VariableStatement) => self.variable_statement(node),
            Some(K::EnumDeclaration) => self.enum_declaration(node).map(Some),
            Some(K::ModuleDeclaration) => self.module_declaration(node).map(Some),
            Some(K::ClassDeclaration) => self.class_declaration(node).map(Some),
            _ => Self::unsupported("declaration emit: top-level declaration kind"),
        }
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.isInternalDeclaration
    pub fn strip_internal(&self, node: NodeId) -> Result<bool, R::Error> {
        if !self.options.strip_internal || self.node(node).flags() & nf::SYNTHESIZED != 0 {
            return Ok(false);
        }
        let source = self.output.read_source_file(self.source)?;
        let text = source.text().as_bytes();
        let pos = i64::from(self.node(node).pos());
        let has_internal = |range: tsr_scanner::CommentRange| -> Result<bool, R::Error> {
            let start =
                usize::try_from(range.loc.pos()).map_err(|_| tsr_arena::Error::InvalidGraph)?;
            let end =
                usize::try_from(range.loc.end()).map_err(|_| tsr_arena::Error::InvalidGraph)?;
            let bytes = text.get(start..end).ok_or(tsr_arena::Error::InvalidGraph)?;
            Ok(bytes
                .windows(b"@internal".len())
                .any(|window| window == b"@internal"))
        };
        if self.node(node).kind() == K::Parameter {
            let parent = Self::required(self.node(node).parent())?;
            let parameters = self.list_nodes(self.node(parent).parameter_list());
            let index = parameters.iter().position(|parameter| *parameter == node);
            let previous = index
                .and_then(|index| index.checked_sub(1))
                .and_then(|index| parameters.get(index))
                .copied();
            let stop = tsr_scanner::SkipTriviaOptions {
                stop_at_comments: true,
                ..Default::default()
            };
            let start = previous.map_or(pos, |previous| i64::from(self.node(previous).end()) + 1);
            let start = tsr_scanner::skip_trivia_ex(text, start, Some(&stop));
            let mut ranges: Vec<_> =
                tsr_scanner::get_trailing_comment_ranges(text, start).collect();
            if previous.is_some() {
                ranges.extend(tsr_scanner::get_leading_comment_ranges(text, pos));
            }
            return ranges.pop().map(has_internal).unwrap_or(Ok(false));
        }
        if self.node(node).kind() != K::JsxText {
            for range in tsr_scanner::get_leading_comment_ranges(text, pos) {
                if has_internal(range)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    pub fn declaration_not_visible(&mut self, node: NodeId) -> Result<bool, R::Error> {
        match self.node(node).kind().known() {
            Some(
                K::FunctionDeclaration
                | K::ModuleDeclaration
                | K::InterfaceDeclaration
                | K::ClassDeclaration
                | K::TypeAliasDeclaration
                | K::JSTypeAliasDeclaration
                | K::EnumDeclaration,
            ) => Ok(!self.resolver.is_declaration_visible(node)?),
            Some(K::VariableDeclaration) => Ok(!self.binding_visible(node)?),
            Some(K::ClassStaticBlockDeclaration) => Ok(true),
            _ => Ok(false),
        }
    }
    pub fn binding_visible(&mut self, node: NodeId) -> Result<bool, R::Error> {
        if self.node(node).kind() == K::OmittedExpression {
            return Ok(false);
        }
        let Some(name) = self.node(node).name() else {
            return Ok(false);
        };
        if matches!(
            self.node(name).kind().known(),
            Some(K::ArrayBindingPattern | K::ObjectBindingPattern)
        ) {
            for element in self.list_nodes(self.node(name).element_list()) {
                if self.binding_visible(element)? {
                    return Ok(true);
                }
            }
            Ok(false)
        } else {
            self.resolver.is_declaration_visible(node)
        }
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.ensureModifierFlags
    pub fn modifiers(&mut self, node: NodeId) -> Result<Option<NodeListId>, R::Error> {
        let old =
            tsr_ast::utilities::get_combined_modifier_flags(self.output.view(), node)? & mf::ALL;
        let mut mask = mf::ALL ^ (mf::PUBLIC | mf::ASYNC | mf::OVERRIDE);
        let always_type = matches!(
            self.node(node).kind().known(),
            Some(K::InterfaceDeclaration | K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
        );
        let mut additions = if self.needs_declare && !always_type {
            mf::AMBIENT
        } else {
            0
        };
        if self
            .node(node)
            .parent()
            .is_none_or(|p| self.node(p).kind() != K::SourceFile)
        {
            mask ^= mf::AMBIENT;
            additions = 0;
        }
        let flags = util::mask_modifier_flags(old, mask, additions);
        if flags == old {
            if let Some(modifiers) = self.node(node).modifiers() {
                let nodes: Vec<_> = self
                    .list_nodes(Some(modifiers))
                    .into_iter()
                    .filter(|n| self.node(*n).kind() != K::Decorator)
                    .collect();
                if nodes
                    .iter()
                    .all(|n| self.node(*n).flags() & nf::REPARSED == 0)
                {
                    let nodes = self
                        .output
                        .alloc_nodes(nodes.into_iter().map(Some).collect());
                    return Ok(Some(self.output.new_modifier_list(nodes)));
                }
            } else {
                return Ok(None);
            }
        }
        Ok(self.modifiers_from_flags(flags))
    }
    fn modifiers_from_flags(&mut self, flags: u32) -> Option<NodeListId> {
        let nodes =
            tsr_ast::utilities_middle::create_modifiers_from_modifier_flags(flags, |kind| {
                Some(self.output.new_modifier(kind))
            })?;
        let nodes = self.output.alloc_nodes(nodes);
        Some(self.output.new_modifier_list(nodes))
    }
    fn variable_statement(&mut self, node: NodeId) -> Result<Option<NodeId>, R::Error> {
        let data = self
            .node(node)
            .data_source()
            .as_variable_statement()
            .unwrap()
            .to_owned();
        let declaration_list = Self::required(data.declaration_list)?;
        let declarations = self
            .node(declaration_list)
            .as_variable_declaration_list()
            .unwrap()
            .declarations();
        let mut visible = false;
        for declaration in self.list_nodes(declarations) {
            if self.binding_visible(declaration)? {
                visible = true;
                break;
            }
        }
        if !visible {
            return Ok(None);
        }
        let mut imports = Vec::new();
        let mut ordinary = Vec::new();
        let common_js = self
            .output
            .read_source_file(self.source)?
            .common_js_module_indicator()
            .is_some();
        for declaration in self.list_nodes(declarations) {
            if common_js
                && tsr_ast::is_variable_declaration_initialized_to_require(
                    self.output.view(),
                    declaration,
                )?
            {
                imports.push(declaration);
            } else {
                ordinary.push(declaration);
            }
        }
        let mut extra_imports = Vec::new();
        for import in imports {
            if let Some(result) = self.visit(Some(import))? {
                self.append_flat(result, &mut extra_imports);
            }
        }
        let mut nodes = Vec::new();
        for declaration in ordinary {
            if let Some(result) = self.visit(Some(declaration))? {
                self.append_flat(result, &mut nodes);
            }
        }
        if nodes.is_empty() {
            return Ok((!extra_imports.is_empty()).then(|| self.syntax_list(extra_imports)));
        }
        let declarations = Some(self.new_list(nodes));
        let modifiers = self.modifiers(node)?;
        let flags = self.node(declaration_list).flags();
        let flags = if flags & (nf::USING | nf::AWAIT_USING) != 0 {
            nf::CONST
        } else {
            flags
        };
        let declarations =
            self.output
                .update_variable_declaration_list(declaration_list, declarations, flags);
        let statement = self
            .output
            .update_variable_statement(node, modifiers, Some(declarations));
        if extra_imports.is_empty() {
            Ok(Some(statement))
        } else {
            extra_imports.push(statement);
            Ok(Some(self.syntax_list(extra_imports)))
        }
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformCjsRequireVariableDeclaration
    pub(super) fn cjs_require_variable(
        &mut self,
        node: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let initializer = Self::required(self.node(node).initializer())?;
        let arguments = self.list_nodes(self.node(initializer).argument_list());
        let specifier = Self::required(arguments.first().copied())?;
        // Declaration diagnostics have no output relocation; the original
        // module literal is the native rewriteModuleSpecifier result here.
        let name = Self::required(self.node(node).name())?;
        match self.node(name).kind().known() {
            Some(K::Identifier) => {
                let reference = self.output.new_external_module_reference(Some(specifier));
                Ok(Some(self.output.new_import_equals_declaration(
                    None,
                    false,
                    Some(name),
                    Some(reference),
                )))
            }
            Some(K::ArrayBindingPattern) => Ok(None),
            Some(K::ObjectBindingPattern) => {
                let mut imports = Vec::new();
                for element in self.list_nodes(self.node(name).element_list()) {
                    let Some(name) = self.node(element).name() else {
                        continue;
                    };
                    if self.node(name).kind() != K::Identifier {
                        continue;
                    }
                    let property = self
                        .node(element)
                        .as_binding_element()
                        .unwrap()
                        .property_name();
                    imports.push(
                        self.output
                            .new_import_specifier(false, property, Some(name)),
                    );
                }
                let imports = self.new_list(imports);
                let named = self.output.new_named_imports(Some(imports));
                let clause = self
                    .output
                    .new_import_clause(K::Unknown.into(), None, Some(named));
                Ok(Some(self.output.new_import_declaration(
                    None,
                    Some(clause),
                    Some(specifier),
                    None,
                )))
            }
            _ => Err(tsr_arena::Error::InvalidGraph.into()),
        }
    }
    fn enum_declaration(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        let data = self
            .node(node)
            .data_source()
            .as_enum_declaration()
            .unwrap()
            .to_owned();
        let modifiers = self.modifiers(node)?;
        let mut members = Vec::new();
        for member in self.list_nodes(data.members) {
            if self.strip_internal(member)? {
                continue;
            }
            let value = self.resolver.enum_member_value(member)?;
            let name = self.node(member).name();
            if self.options.isolated_declarations
                && self.node(member).initializer().is_some()
                && value.has_external_references
                && name.is_none_or(|n| self.node(n).kind() != K::ComputedPropertyName)
            {
                self.diagnostic(member, tsr_diagnostics::Enum_member_initializers_must_be_computable_without_references_to_external_symbols_with_isolatedDeclarations, vec![])?;
            }
            let initializer = match value.value {
                None => None,
                Some(ConstantValue::String(value)) => {
                    Some(self.output.new_string_literal(value, 0))
                }
                Some(ConstantValue::Number(value)) => {
                    let number = value.value();
                    let inner = if number.is_infinite() {
                        self.output
                            .new_identifier(JsString::from_bytes(b"Infinity".as_slice()))
                    } else if number.is_nan() {
                        self.output
                            .new_identifier(JsString::from_bytes(b"NaN".as_slice()))
                    } else {
                        self.output.new_numeric_literal(
                            JsString::from_bytes(
                                tsr_jsnum::Number::new(number.abs()).to_string().as_bytes(),
                            ),
                            0,
                        )
                    };
                    Some(if number < 0.0 {
                        self.output
                            .new_prefix_unary_expression(K::MinusToken.into(), Some(inner))
                    } else {
                        inner
                    })
                }
            };
            members.push(self.output.update_enum_member(member, name, initializer));
        }
        let members = self.new_list(members);
        Ok(self
            .output
            .update_enum_declaration(node, modifiers, data.name, Some(members)))
    }
    fn import_equals(&mut self, node: NodeId) -> Result<Option<NodeId>, R::Error> {
        if !self.resolver.is_declaration_visible(node)? {
            return Ok(None);
        }
        let data = self
            .node(node)
            .data_source()
            .as_import_equals_declaration()
            .unwrap()
            .to_owned();
        let reference = Self::required(data.module_reference)?;
        if self.node(reference).kind() == K::ExternalModuleReference {
            self.external_indicator = true;
            return Ok(Some(node));
        }
        let old = self.tracker.selector.clone();
        self.select_context(node, false)?;
        let result = self.entity_visible(reference);
        self.tracker.selector = old;
        result?;
        Ok(Some(node))
    }
    fn import_declaration(&mut self, node: NodeId) -> Result<Option<NodeId>, R::Error> {
        let data = self
            .node(node)
            .data_source()
            .as_import_declaration()
            .unwrap()
            .to_owned();
        let Some(clause) = data.import_clause else {
            self.external_indicator = true;
            return Ok(Some(node));
        };
        let clause_data = self
            .node(clause)
            .data_source()
            .as_import_clause()
            .unwrap()
            .to_owned();
        let phase = if clause_data.phase_modifier == K::DeferKeyword {
            K::Unknown.into()
        } else {
            clause_data.phase_modifier
        };
        let default =
            if clause_data.name.is_some() && self.resolver.is_declaration_visible(clause)? {
                clause_data.name
            } else {
                None
            };
        let named = if let Some(bindings) = clause_data.named_bindings {
            if self.node(bindings).kind() == K::NamespaceImport {
                self.resolver
                    .is_declaration_visible(bindings)?
                    .then_some(bindings)
            } else {
                let mut kept = Vec::new();
                for import in self.list_nodes(self.node(bindings).element_list()) {
                    if self.resolver.is_declaration_visible(import)? {
                        kept.push(import);
                    }
                }
                if kept.is_empty() {
                    None
                } else {
                    let list = self.new_list(kept);
                    Some(self.output.update_named_imports(bindings, Some(list)))
                }
            }
        } else {
            None
        };
        if default.is_some() || named.is_some() {
            let clause = self
                .output
                .update_import_clause(clause, phase, default, named);
            self.external_indicator = true;
            return Ok(Some(self.output.update_import_declaration(
                node,
                data.modifiers,
                Some(clause),
                data.module_specifier,
                data.attributes,
            )));
        }
        if clause_data
            .named_bindings
            .is_some_and(|n| self.node(n).kind() == K::NamedImports)
            && self.resolver.import_required_by_augmentation(node)?
        {
            if self.options.isolated_declarations {
                self.diagnostic(node, tsr_diagnostics::Declaration_emit_for_this_file_requires_preserving_this_import_for_augmentations_This_is_not_supported_with_isolatedDeclarations, vec![])?;
            }
            self.external_indicator = true;
            return Ok(Some(self.output.update_import_declaration(
                node,
                data.modifiers,
                None,
                data.module_specifier,
                data.attributes,
            )));
        }
        Ok(None)
    }
    fn module_declaration(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        let data = self
            .node(node)
            .data_source()
            .as_module_declaration()
            .unwrap()
            .to_owned();
        let modifiers = self.modifiers(node)?;
        let old_declare = self.needs_declare;
        self.needs_declare = false;
        let keyword = if data.keyword != K::GlobalKeyword
            && data
                .name
                .is_none_or(|n| self.node(n).kind() != K::StringLiteral)
        {
            K::NamespaceKeyword.into()
        } else {
            data.keyword
        };
        let attributes = self.visit(data.attributes)?;
        let body = if let Some(inner) = data.body {
            if self.node(inner).kind() == K::ModuleBlock {
                let old_needed = self.needs_scope_marker;
                let old_has = self.has_scope_marker;
                self.needs_scope_marker = false;
                self.has_scope_marker = false;
                let statements = self.visit_list_result(self.node(inner).statement_list())?;
                let mut statements = self.late_statements(statements)?;
                if self.node(node).flags() & nf::AMBIENT != 0 {
                    self.needs_scope_marker = false;
                }
                if !tsr_ast::utilities::is_global_scope_augmentation(&self.node(node))
                    && !self.has_scope_marker
                {
                    if self.needs_scope_marker {
                        let mut nodes = self.list_nodes(statements);
                        let marker = self.empty_exports();
                        nodes.push(marker);
                        statements = Some(self.new_list(nodes));
                    } else {
                        let mut nodes = Vec::new();
                        for statement in self.list_nodes(statements) {
                            nodes.push(self.strip_exports(statement)?);
                        }
                        statements = Some(self.new_list(nodes));
                    }
                }
                let body = self.output.update_module_block(inner, statements);
                self.needs_scope_marker = old_needed;
                self.has_scope_marker = old_has;
                Some(body)
            } else {
                let _ = self.visit(Some(inner))?;
                self.replacements.remove(&inner).flatten()
            }
        } else {
            None
        };
        self.needs_declare = old_declare;
        Ok(self
            .output
            .update_module_declaration(node, modifiers, keyword, data.name, attributes, body))
    }
    fn strip_exports(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        if self.node(node).kind() == K::ImportEqualsDeclaration {
            return Ok(node);
        }
        let flags = tsr_ast::utilities::get_combined_modifier_flags(self.output.view(), node)?;
        if flags & mf::DEFAULT != 0 || flags & mf::EXPORT == 0 {
            return Ok(node);
        }
        let modifiers = self.modifiers_from_flags(flags & !mf::EXPORT);
        self.replace_top_level_modifiers(node, modifiers)
    }
    fn scope_marker_needed(&self, node: NodeId) -> Result<bool, R::Error> {
        if matches!(
            self.node(node).kind().known(),
            Some(
                K::ImportDeclaration
                    | K::JSImportDeclaration
                    | K::ImportEqualsDeclaration
                    | K::ExportDeclaration
                    | K::ExportAssignment
            )
        ) {
            return Ok(false);
        }
        if self.node(node).kind() == K::ModuleDeclaration
            && self
                .node(node)
                .name()
                .is_some_and(|n| self.node(n).kind() == K::StringLiteral)
        {
            return Ok(false);
        }
        Ok(
            tsr_ast::utilities::get_combined_modifier_flags(self.output.view(), node)? & mf::EXPORT
                == 0,
        )
    }
    fn external_module_indicator(&self, node: NodeId) -> Result<bool, R::Error> {
        Ok(matches!(
            self.node(node).kind().known(),
            Some(
                K::ImportDeclaration
                    | K::JSImportDeclaration
                    | K::ExportDeclaration
                    | K::ExportAssignment
            )
        ) || (self.node(node).kind() == K::ImportEqualsDeclaration
            && self
                .node(node)
                .as_import_equals_declaration()
                .unwrap()
                .module_reference()
                .is_some_and(|r| self.node(r).kind() == K::ExternalModuleReference))
            || tsr_ast::utilities::get_combined_modifier_flags(self.output.view(), node)?
                & mf::EXPORT
                != 0)
    }
}
