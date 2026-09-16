//! Import and re-export aliases preserve the immediate symbol when requested;
//! pure alias chains are collapsed by the existing shared resolution stack.
use crate::{CheckerState, Error};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{internal_symbol_names as names, symbol_flags as sf, SyntaxKind as K};
use ts_core::ModuleKind;
use ts_diagnostics as d;
impl CheckerState {
    pub(crate) fn shorthand_ambient_module(&self, symbol: SymbolId) -> Result<bool, Error> {
        let Some(declaration) = self.symbol(symbol)?.value_declaration() else {
            return Ok(false);
        };
        let read = self.ast(declaration)?.node(declaration)?;
        Ok(read.kind() == K::ModuleDeclaration && read.body().is_none())
    }
    pub(crate) fn mark_module_alias_type_only(
        &mut self,
        node: NodeId,
        star: Option<NodeId>,
    ) -> Result<(), Error> {
        let Some(symbol) = self.get_symbol_of_declaration(node)? else {
            return Ok(());
        };
        if !self.module_aliases.type_only.contains_key(&symbol) {
            if self.local_type_only_alias_declaration(node)?.is_some() {
                self.module_aliases.type_only.insert(symbol, node);
            } else if let Some(star) = star {
                self.module_aliases.type_only.insert(symbol, star);
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.getTargetOfImportClause
    // port: tsc/internal/checker/checker.go:Checker.getTargetOfNamespaceImport
    // port: tsc/internal/checker/checker.go:Checker.getTargetOfNamespaceExport
    pub(crate) fn target_of_external_alias(
        &mut self,
        node: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let read = self.ast(node)?.node(node)?;
        let kind = read.kind();
        let specifier = match self.require_module_specifier(node)? {
            Some(specifier) => specifier,
            None => self
                .module_specifier(node)?
                .ok_or(Error::MissingLink("external alias specifier"))?,
        };
        self.prepare_module_attributes(specifier)?;
        let Some(module) = self.resolve_external_module_name(node, specifier, false)? else {
            return Ok(None);
        };
        let result = if kind == K::ImportClause {
            self.target_of_module_default(module, node, true)?
        } else if matches!(kind.known(), Some(K::NamespaceImport | K::NamespaceExport)) {
            self.resolve_es_module_symbol(module, node, specifier)?
        } else if kind == K::ImportEqualsDeclaration {
            let result = self.resolve_external_module_symbol(Some(module), true)?;
            if (ModuleKind::NODE20..=ModuleKind::NODE_NEXT)
                .contains(&self.program()?.host.options().emit_module_kind())
            {
                if let Some(result) = result {
                    if let Some(export) =
                        self.module_export_member(result, names::MODULE_EXPORTS, node, true)?
                    {
                        return Ok(Some(export));
                    }
                }
            }
            result
        } else {
            self.external_module_member(module, node, specifier, true)?
        };
        self.mark_module_alias_type_only(node, None)?;
        Ok(result)
    }
    pub(crate) fn prepare_module_attributes(&mut self, specifier: NodeId) -> Result<(), Error> {
        self.import_attributes_type_for_specifier(specifier)?;
        Ok(())
    }
    /// The `require("...")` argument of the node's root declaration, which
    /// upstream reads before falling back to a module specifier. A binding
    /// element resolves through its root variable declaration.
    // port: tsc/internal/checker/utilities.go:getExternalModuleRequireArgument
    // port: tsc/internal/ast/utilities.go:IsVariableDeclarationInitializedToRequire
    pub(crate) fn require_module_specifier(
        &mut self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let root = if self.ast(node)?.node(node)?.kind() == K::BindingElement {
            ts_ast::utilities::get_root_declaration(self.ast(node)?, node)?
        } else {
            node
        };
        let read = self.ast(root)?.node(root)?;
        if read.kind() != K::VariableDeclaration
            || !ts_ast::utilities::is_in_js_file(Some(&read))
            || read.type_node().is_some()
        {
            return Ok(None);
        }
        let Some(initializer) = read.initializer() else {
            return Ok(None);
        };
        let Some(list) = read.parent() else {
            return Ok(None);
        };
        let Some(statement) = self.ast(list)?.node(list)?.parent() else {
            return Ok(None);
        };
        let view = self.ast(statement)?;
        if view.node(statement)?.modifier_flags(view)? & ts_ast::modifier_flags::EXPORT != 0 {
            return Ok(None);
        }
        let view = self.ast(initializer)?;
        if !ts_ast::utilities_middle::is_require_call(view, &view.node(initializer)?, true)? {
            return Ok(None);
        }
        let arguments = self.source_list(
            initializer,
            self.ast(initializer)?.node(initializer)?.argument_list(),
        )?;
        Ok(arguments.first().copied())
    }
    // port: tsc/internal/checker/checker.go:Checker.getExportOfModule
    pub(crate) fn module_export_member(
        &mut self,
        module: SymbolId,
        name: &[u8],
        source: NodeId,
        dont_resolve_alias: bool,
    ) -> Result<Option<SymbolId>, Error> {
        if self.symbol(module)?.flags() & sf::MODULE == 0 {
            return Ok(None);
        }
        let exports = self.module_exports(module)?;
        let symbol = self.table(exports)?.get(name).flatten();
        let result = self.resolve_module_symbol(symbol, dont_resolve_alias)?;
        let star = self
            .module_aliases
            .type_only_exports
            .get(&module)
            .and_then(|table| table.get(name))
            .copied();
        self.mark_module_alias_type_only(source, star)?;
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.resolveExportByName
    fn module_export_by_name(
        &mut self,
        module: SymbolId,
        name: &[u8],
        source: Option<NodeId>,
        dont_resolve_alias: bool,
    ) -> Result<Option<SymbolId>, Error> {
        let exports = self.symbol(module)?.exports();
        let symbol = if let Some(export) = self.member_symbol(exports, names::EXPORT_EQUALS)? {
            let ty = self.get_type_of_symbol(export)?;
            self.constituent_property_ex(ty, name, true, false)?
        } else {
            self.member_symbol(exports, name)?
        };
        let resolved = self.resolve_module_symbol(symbol, dont_resolve_alias)?;
        if let Some(source) = source {
            self.mark_module_alias_type_only(source, None)?;
        }
        Ok(resolved)
    }
    fn module_source_declaration(&self, module: SymbolId) -> Result<Option<NodeId>, Error> {
        for node in self
            .symbol_declarations(module)?
            .to_vec()
            .into_iter()
            .flatten()
        {
            if self.ast(node)?.node(node)?.kind() == K::SourceFile {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }
    // port: tsc/internal/checker/checker.go:Checker.isOnlyImportableAsDefault
    pub(crate) fn module_only_importable_as_default(
        &mut self,
        usage: NodeId,
        mut module: Option<SymbolId>,
    ) -> Result<bool, Error> {
        let kind = self.program()?.host.options().emit_module_kind();
        if !(ModuleKind::NODE16..=ModuleKind::NODE_NEXT).contains(&kind) {
            return Ok(false);
        }
        let (_, file) = self.module_source(usage)?;
        if self
            .program()?
            .host
            .get_emit_syntax_for_usage_location(file.as_bytes(), usage)?
            .0
            != ModuleKind::ESNEXT.0
        {
            return Ok(false);
        }
        if module.is_none() {
            module = self.resolve_external_module_name(usage, usage, true)?;
        }
        let Some(module) = module else {
            return Ok(false);
        };
        let Some(file) = self.module_source_declaration(module)? else {
            return Ok(false);
        };
        let state = self.ast(file)?.source_file(file)?;
        Ok(ts_ast::utilities::is_json_source_file(&state)
            || state.file_name().ends_with(b".d.json.ts"))
    }
    // port: tsc/internal/checker/checker.go:Checker.canHaveSyntheticDefault
    pub(crate) fn module_can_have_synthetic_default(
        &mut self,
        module: SymbolId,
        usage: NodeId,
        dont_resolve_alias: bool,
    ) -> Result<bool, Error> {
        let file = self.module_source_declaration(module)?;
        if let Some(file) = file {
            let (_, source_name) = self.module_source(usage)?;
            let file_name = self
                .ast(file)?
                .source_file(file)?
                .parse_options()
                .file_name
                .clone();
            let mode = self
                .program()?
                .host
                .get_emit_syntax_for_usage_location(source_name.as_bytes(), usage)?;
            if mode != ModuleKind::NONE {
                let target_mode = self
                    .program()?
                    .host
                    .get_implied_node_format_for_emit(file_name.as_bytes())?;
                if mode == ModuleKind::ESNEXT
                    && target_mode == ModuleKind::COMMON_JS
                    && (ModuleKind::NODE16..=ModuleKind::NODE_NEXT)
                        .contains(&self.program()?.host.options().emit_module_kind())
                {
                    return Ok(true);
                }
                if mode == ModuleKind::ESNEXT && target_mode == ModuleKind::ESNEXT {
                    return Ok(false);
                }
                if target_mode == ModuleKind::NONE
                    && self.ast(file)?.source_file(file)?.is_declaration_file
                    && (self
                        .program()?
                        .host
                        .get_redirect_for_resolution(file_name.as_bytes())?
                        .is_some()
                        || self
                            .program()?
                            .host
                            .get_project_reference_from_output_dts(file_name.as_bytes())?
                            .is_some())
                {
                    return Err(Error::Unsupported(
                        "canHaveSyntheticDefault: project reference module format",
                    ));
                }
            }
        }
        if match file {
            Some(file) => self.ast(file)?.source_file(file)?.is_declaration_file,
            None => true,
        } {
            if let Some(default) = self.module_export_by_name(module, names::DEFAULT, None, true)? {
                for node in self
                    .symbol_declarations(default)?
                    .to_vec()
                    .into_iter()
                    .flatten()
                {
                    if self.syntactic_module_default(node)? {
                        return Ok(false);
                    }
                }
            }
            return Ok(self
                .module_export_by_name(module, b"__esModule", None, dont_resolve_alias)?
                .is_none());
        }
        let file = file.ok_or(Error::MissingLink("synthetic module file"))?;
        if self.ast(file)?.node(file)?.flags() & ts_ast::node_flags::JAVA_SCRIPT_FILE == 0 {
            return Ok(self
                .member_symbol(self.symbol(module)?.exports(), names::EXPORT_EQUALS)?
                .is_some());
        }
        let indicator = self.ast(file)?.source_file(file)?.external_module_indicator;
        Ok((indicator.is_none() || indicator == Some(file))
            && self
                .module_export_by_name(module, b"__esModule", None, dont_resolve_alias)?
                .is_none())
    }
    // port: tsc/internal/checker/utilities.go:isSyntacticDefault
    fn syntactic_module_default(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.kind() == K::ExportAssignment {
            return Ok(!read
                .data_source()
                .as_export_assignment()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .is_export_equals());
        }
        if matches!(
            read.kind().known(),
            Some(K::ExportSpecifier | K::NamespaceExport)
        ) {
            return Ok(true);
        }
        Ok(read.modifier_flags(self.ast(node)?)? & ts_ast::modifier_flags::DEFAULT != 0)
    }
    // port: tsc/internal/checker/checker.go:Checker.getTargetOfModuleDefault
    fn target_of_module_default(
        &mut self,
        module: SymbolId,
        node: NodeId,
        dont_resolve_alias: bool,
    ) -> Result<Option<SymbolId>, Error> {
        let specifier = self.module_specifier(node)?;
        if !self.shorthand_ambient_module(module)?
            && (ModuleKind::NODE20..=ModuleKind::NODE_NEXT)
                .contains(&self.program()?.host.options().emit_module_kind())
        {
            if let (Some(file), Some(specifier)) =
                (self.module_source_declaration(module)?, specifier)
            {
                let (_, usage_file) = self.module_source(specifier)?;
                let mode = self
                    .program()?
                    .host
                    .get_emit_syntax_for_usage_location(usage_file.as_bytes(), specifier)?;
                let file_name = self
                    .ast(file)?
                    .source_file(file)?
                    .parse_options()
                    .file_name
                    .clone();
                if mode == ModuleKind::COMMON_JS
                    && self
                        .program()?
                        .host
                        .get_implied_node_format_for_emit(file_name.as_bytes())?
                        == ModuleKind::ESNEXT
                {
                    if let Some(export) = self.module_export_by_name(
                        module,
                        names::MODULE_EXPORTS,
                        Some(node),
                        dont_resolve_alias,
                    )? {
                        self.mark_module_alias_type_only(node, None)?;
                        return Ok(Some(export));
                    }
                }
            }
        }
        let default =
            self.module_export_by_name(module, names::DEFAULT, Some(node), dont_resolve_alias)?;
        let Some(specifier) = specifier else {
            return Ok(default);
        };
        let synthetic =
            self.module_can_have_synthetic_default(module, specifier, dont_resolve_alias)?;
        if synthetic || self.module_only_importable_as_default(specifier, Some(module))? {
            let resolved = self.resolve_external_module_symbol(Some(module), dont_resolve_alias)?;
            return self.resolve_module_symbol(resolved, dont_resolve_alias);
        }
        if default.is_none() {
            if self.ast(node)?.node(node)?.kind() == K::ImportClause {
                let name = self
                    .ast(node)?
                    .node(node)?
                    .name()
                    .ok_or(Error::MissingLink("default import name"))?;
                let name_text = self.ast(name)?.node_text(name)?.into_js_string();
                let module_text = self.symbol_to_string(module)?;
                if self
                    .member_symbol(self.symbol(module)?.exports(), name_text.as_bytes())?
                    .is_some()
                {
                    self.error_at(Some(node),d::Module_0_has_no_default_export_Did_you_mean_to_use_import_1_from_0_instead,vec![module_text,name_text])?;
                } else {
                    let diagnostic = self.error_at(
                        Some(name),
                        d::Module_0_has_no_default_export,
                        vec![module_text],
                    )?;
                    if let Some(diagnostic) = diagnostic {
                        if let Some(related) = self.export_star_default_declaration(module)? {
                            let related = self.diagnostic_for_node(
                                Some(related),
                                d::X_export_Asterisk_does_not_re_export_a_default,
                                vec![],
                            )?;
                            self.add_related_diagnostic(diagnostic, related)?;
                        }
                    }
                }
            } else {
                let read = self.ast(node)?.node(node)?;
                let name = read
                    .property_name()
                    .or(read.name())
                    .ok_or(Error::MissingLink("default specifier name"))?;
                self.error_no_module_member(module, module, node, name)?;
            }
        }
        Ok(default)
    }
    // port: tsc/internal/checker/checker.go:Checker.resolveESModuleSymbol
    fn resolve_es_module_symbol(
        &mut self,
        module: SymbolId,
        node: NodeId,
        specifier: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let Some(mut symbol) = self.resolve_external_module_symbol(Some(module), true)? else {
            return Ok(None);
        };
        if ts_ast::is_non_local_alias(
            Some(&self.symbol(symbol)?),
            sf::VALUE | sf::TYPE | sf::NAMESPACE,
        ) {
            let source = self
                .get_symbol_of_declaration(node)?
                .ok_or(Error::MissingLink("namespace alias source"))?;
            let target = self.resolve_alias(symbol)?;
            if let Some(&declaration) = self.module_aliases.type_only.get(&symbol) {
                self.module_aliases
                    .type_only
                    .entry(source)
                    .or_insert(declaration);
            }
            symbol = self.get_merged_symbol(target);
        }
        let parent = self
            .ast(specifier)?
            .node(specifier)?
            .parent()
            .ok_or(Error::MissingLink("module reference parent"))?;
        let namespace_import = if self.ast(parent)?.node(parent)?.kind() == K::ImportDeclaration {
            ts_ast::utilities_middle::get_namespace_declaration_node(self.ast(parent)?, parent)?
        } else {
            None
        };
        let import_call = self.ast(parent)?.node(parent)?.kind() == K::CallExpression;
        if namespace_import.is_some() || import_call {
            let ty = self.get_type_of_symbol(symbol)?;
            if let Some(default_only) =
                self.module_default_only_type(ty, symbol, module, specifier)?
            {
                return self
                    .clone_type_as_module_type(symbol, default_only, parent)
                    .map(Some);
            }
            let source = self.module_source_declaration(module)?;
            let (_, import_name) = self.module_source(specifier)?;
            let usage = self
                .program()?
                .host
                .get_emit_syntax_for_usage_location(import_name.as_bytes(), specifier)?;
            let target_mode = if let Some(source) = source {
                let name = self
                    .ast(source)?
                    .source_file(source)?
                    .parse_options()
                    .file_name
                    .clone();
                self.program()?
                    .host
                    .get_implied_node_format_for_emit(name.as_bytes())?
            } else {
                ModuleKind::NONE
            };
            if let Some(namespace) = namespace_import {
                if source.is_some()
                    && (ModuleKind::NODE20..=ModuleKind::NODE_NEXT)
                        .contains(&self.program()?.host.options().emit_module_kind())
                    && usage == ModuleKind::COMMON_JS
                    && target_mode == ModuleKind::ESNEXT
                {
                    if let Some(export) =
                        self.module_export_member(symbol, names::MODULE_EXPORTS, namespace, true)?
                    {
                        return if self.module_has_signatures(ty)? {
                            self.clone_type_as_module_type(export, ty, parent).map(Some)
                        } else {
                            Ok(Some(export))
                        };
                    }
                }
            }
            if self.module_has_signatures(ty)?
                || self
                    .constituent_property_ex(ty, names::DEFAULT, true, false)?
                    .is_some()
                || source.is_some()
                    && usage == ModuleKind::ESNEXT
                    && target_mode == ModuleKind::COMMON_JS
            {
                let module_type = if self.types.flags(ty)? & crate::type_flags::STRUCTURED_TYPE != 0
                {
                    self.module_synthetic_default_type(ty, symbol, module, specifier)?
                } else {
                    let parent = self.symbol(symbol)?.parent();
                    self.default_property_wrapper_for_module(symbol, parent, None)?
                };
                return self
                    .clone_type_as_module_type(symbol, module_type, parent)
                    .map(Some);
            }
        }
        Ok(Some(symbol))
    }
    // port: tsc/internal/checker/checker.go:Checker.getExternalModuleMember
    fn external_module_member(
        &mut self,
        module: SymbolId,
        node: NodeId,
        specifier: NodeId,
        dont_resolve_alias: bool,
    ) -> Result<Option<SymbolId>, Error> {
        let read = self.ast(node)?.node(node)?;
        let name = read
            .property_name()
            .or(read.name())
            .ok_or(Error::MissingLink("import/export member name"))?;
        let read = self.ast(name)?.node(name)?;
        if !matches!(read.kind().known(), Some(K::Identifier | K::StringLiteral)) {
            return Ok(None);
        }
        let text = self.ast(name)?.node_text(name)?.into_js_string();
        if text.is_empty() && read.kind() != K::StringLiteral {
            return Ok(None);
        }
        if text.as_bytes() == names::DEFAULT {
            return self.target_of_module_default(module, node, dont_resolve_alias);
        }
        if self.shorthand_ambient_module(module)? {
            return Ok(Some(module));
        }
        let Some(target) = self.resolve_es_module_symbol(module, node, specifier)? else {
            return Ok(None);
        };
        let export_equals = self
            .member_symbol(self.symbol(module)?.exports(), names::EXPORT_EQUALS)?
            .is_some();
        let from_variable = if export_equals {
            let ty = self.get_type_of_symbol(target)?;
            self.constituent_property_ex(ty, text.as_bytes(), true, false)?
        } else if self.symbol(target)?.flags() & sf::VARIABLE != 0 {
            let declaration = self
                .symbol(target)?
                .value_declaration()
                .ok_or(Error::MissingLink("module variable declaration"))?;
            match self.ast(declaration)?.node(declaration)?.type_node() {
                Some(annotation) => {
                    let ty = self.get_type_from_type_node(annotation)?;
                    self.constituent_property(ty, text.as_bytes(), false)?
                }
                None => None,
            }
        } else {
            None
        };
        let from_variable = self.resolve_module_symbol(from_variable, dont_resolve_alias)?;
        let from_module = self.module_export_member(
            if export_equals { module } else { target },
            text.as_bytes(),
            node,
            dont_resolve_alias,
        )?;
        let result = match (from_variable, from_module) {
            (Some(value), Some(ty)) if value != ty => {
                Some(self.combine_value_and_type_symbols(value, ty)?)
            }
            (_, Some(value)) | (Some(value), _) => Some(value),
            _ => None,
        };
        let specifier_node = matches!(
            self.ast(node)?.node(node)?.kind().known(),
            Some(K::ImportSpecifier | K::ExportSpecifier)
        );
        if specifier_node
            && self.module_only_importable_as_default(specifier, Some(module))?
            && text.as_bytes() != names::DEFAULT
        {
            let kind = self.program()?.host.options().emit_module_kind();
            self.error_at(
                Some(name),
                d::Named_imports_from_a_JSON_file_into_an_ECMAScript_module_are_not_allowed_when_module_is_set_to_0,
                vec![ts_ast::JsString::from_bytes(crate::emit_checks::module_kind_text(kind))],
            )?;
        } else if result.is_none() {
            self.error_no_module_member(module, target, node, name)?;
        }
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.errorNoModuleMemberSymbol
    fn error_no_module_member(
        &mut self,
        module: SymbolId,
        target: SymbolId,
        node: NodeId,
        name: NodeId,
    ) -> Result<(), Error> {
        if self.program()?.host.options().no_check.is_true() {
            return Ok(());
        }
        let module_name = self.fully_qualified_name(module, Some(node))?;
        let name_text = self.ast(name)?.node_text(name)?.into_js_string();
        let declaration_name = ts_scanner::declaration_name_to_string(self.ast(name)?, Some(name))?;
        if self.ast(name)?.node(name)?.kind() == K::Identifier {
            let suggestion = self.suggested_module_member(name, target)?;
            if let Some(suggestion) = suggestion {
                let display = self.symbol_to_string(suggestion)?;
                if let Some(diagnostic) = self.error_at(
                    Some(name),
                    d::X_0_has_no_exported_member_named_1_Did_you_mean_2,
                    vec![module_name, declaration_name, display.clone()],
                )? {
                    if let Some(declaration) = self.symbol(suggestion)?.value_declaration() {
                        let related = self.diagnostic_for_node(
                            Some(declaration),
                            d::X_0_is_declared_here,
                            vec![display],
                        )?;
                        self.add_related_diagnostic(diagnostic, related)?;
                    }
                }
                return Ok(());
            }
        }
        if self
            .member_symbol(self.symbol(module)?.exports(), names::DEFAULT)?
            .is_some()
        {
            self.error_at(
                Some(name),
                d::Module_0_has_no_exported_member_1_Did_you_mean_to_use_import_1_from_0_instead,
                vec![module_name, declaration_name],
            )?;
        } else {
            let declaration = self.symbol(module)?.value_declaration();
            let local = if let Some(declaration) = declaration {
                self.program()?
                    .bound(declaration)?
                    .node_binding(declaration)?
                    .and_then(|binding| binding.locals)
            } else {
                None
            };
            if let Some(local) = self.member_symbol(local, name_text.as_bytes())? {
                let exports = self.symbol(module)?.exports();
                if let Some(export) = self.member_symbol(exports, names::EXPORT_EQUALS)? {
                    if self.module_symbols_same_reference(export, local)? {
                        let kind = self.program()?.host.options().emit_module_kind();
                        if kind >= ModuleKind::ES2015 {
                            self.error_at(
                                Some(name),
                                d::X_0_can_only_be_imported_by_using_a_default_import,
                                vec![declaration_name],
                            )?;
                        } else if self.ast(name)?.node(name)?.flags()
                            & ts_ast::node_flags::JAVA_SCRIPT_FILE
                            != 0
                        {
                            self.error_at(Some(name),d::X_0_can_only_be_imported_by_using_a_require_call_or_by_using_a_default_import,vec![declaration_name])?;
                        } else {
                            self.error_at(Some(name),d::X_0_can_only_be_imported_by_using_import_1_require_2_or_a_default_import,vec![declaration_name.clone(),declaration_name,module_name])?;
                        }
                        return Ok(());
                    }
                } else {
                    let mut renamed = None;
                    if let Some(exports) = exports {
                        for (_, export) in self.module_table_entries(exports)? {
                            if let Some(export) = export {
                                if self.module_symbols_same_reference(export, local)? {
                                    renamed = Some(export);
                                    break;
                                }
                            }
                        }
                    }
                    let diagnostic = if let Some(export) = renamed {
                        let export_name = self.symbol_to_string(export)?;
                        self.error_at(
                            Some(name),
                            d::Module_0_declares_1_locally_but_it_is_exported_as_2,
                            vec![module_name, declaration_name.clone(), export_name],
                        )?
                    } else {
                        self.error_at(
                            Some(name),
                            d::Module_0_declares_1_locally_but_it_is_not_exported,
                            vec![module_name, declaration_name.clone()],
                        )?
                    };
                    if let Some(diagnostic) = diagnostic {
                        for (index, declaration) in self
                            .symbol_declarations(local)?
                            .to_vec()
                            .into_iter()
                            .flatten()
                            .enumerate()
                        {
                            let related = self.diagnostic_for_node(
                                Some(declaration),
                                if index == 0 {
                                    d::X_0_is_declared_here
                                } else {
                                    d::X_and_here
                                },
                                vec![declaration_name.clone()],
                            )?;
                            self.add_related_diagnostic(diagnostic, related)?;
                        }
                    }
                    return Ok(());
                }
            }
            self.error_at(
                Some(name),
                d::Module_0_has_no_exported_member_1,
                vec![module_name, declaration_name],
            )?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.getSymbolIfSameReference
    pub(crate) fn module_symbols_same_reference(
        &mut self,
        left: SymbolId,
        right: SymbolId,
    ) -> Result<bool, Error> {
        let left = self
            .resolve_module_symbol(Some(self.get_merged_symbol(left)), false)?
            .ok_or(Error::MissingLink("left module reference"))?;
        let right = self
            .resolve_module_symbol(Some(self.get_merged_symbol(right)), false)?
            .ok_or(Error::MissingLink("right module reference"))?;
        Ok(self.get_merged_symbol(left) == self.get_merged_symbol(right))
    }

    // port: tsc/internal/checker/checker.go:Checker.combineValueAndTypeSymbols
    fn combine_value_and_type_symbols(
        &mut self,
        value_symbol: SymbolId,
        type_symbol: SymbolId,
    ) -> Result<SymbolId, Error> {
        if value_symbol == self.builtins.unknown_symbol
            && type_symbol == self.builtins.unknown_symbol
        {
            return Ok(self.builtins.unknown_symbol);
        }
        let value = self.symbol(value_symbol)?;
        let ty = self.symbol(type_symbol)?;
        if ty.flags() & sf::VALUE != 0 {
            return Ok(type_symbol);
        }
        if value.flags() & (sf::TYPE | sf::NAMESPACE) != 0 {
            return Ok(value_symbol);
        }
        let flags = value.flags() | ty.flags();
        let name = value.name_to_owned();
        let parent = value.parent().or(ty.parent());
        let value_declaration = value.value_declaration();
        let members = ty.members();
        let exports = value.exports();
        let mut declarations: Vec<Option<ts_arena::NodeId>> = Vec::new();
        for declaration in self
            .symbol_declarations(value_symbol)?
            .iter()
            .chain(self.symbol_declarations(type_symbol)?.iter())
        {
            declarations.push(declaration);
        }
        // Go's slices.Compact removes adjacent duplicates only.
        declarations.dedup();
        if declarations.is_empty() {
            return Err(Error::MissingLink("combined symbol declarations"));
        }
        let result = self.new_symbol(flags, name)?;
        let declarations = self.declarations.alloc(declarations)?;
        let members = self.clone_symbol_table(members)?;
        let exports = self.clone_symbol_table(exports)?;
        let target = self.symbol_mut(result)?;
        target.declarations = declarations;
        target.parent = parent;
        target.value_declaration = value_declaration;
        target.members = members;
        target.exports = exports;
        Ok(result)
    }

    /// The `export *` declaration whose target module has a default export,
    /// used as related information by reportNonDefaultExport.
    // port: tsc/internal/checker/checker.go:Checker.reportNonDefaultExport
    fn export_star_default_declaration(
        &mut self,
        module: SymbolId,
    ) -> Result<Option<NodeId>, Error> {
        let Some(export_star) =
            self.member_symbol(self.symbol(module)?.exports(), names::EXPORT_STAR)?
        else {
            return Ok(None);
        };
        for declaration in self
            .symbol_declarations(export_star)?
            .to_vec()
            .into_iter()
            .flatten()
        {
            let read = self.ast(declaration)?.node(declaration)?;
            if read.kind() != K::ExportDeclaration {
                continue;
            }
            let Some(specifier) = read.module_specifier() else {
                continue;
            };
            let Some(resolved) =
                self.resolve_external_module_name(declaration, specifier, false)?
            else {
                continue;
            };
            if self
                .member_symbol(self.symbol(resolved)?.exports(), names::DEFAULT)?
                .is_some()
            {
                return Ok(Some(declaration));
            }
        }
        Ok(None)
    }
}
