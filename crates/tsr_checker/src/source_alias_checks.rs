//! Alias diagnostics follow the source language and emitted module format;
//! syntactic type-only declarations already carry their own JS grammar errors.
use crate::{CheckerState, Error};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    modifier_flags as mf, node_flags as nf, symbol_flags as sf, JsString, SyntaxKind as K,
};
use ts_core::ModuleKind;
use ts_diagnostics as d;
impl CheckerState {
    pub(crate) fn alias_property_name(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let read = self.node(node)?;
        Ok(
            if matches!(
                read.kind().known(),
                Some(K::ImportSpecifier | K::ExportSpecifier)
            ) {
                read.property_name().or(read.name())
            } else {
                read.name()
            },
        )
    }
    // port: tsc/internal/checker/checker.go:Checker.checkAliasSymbol
    pub(crate) fn check_js_type_alias_import(
        &mut self,
        node: NodeId,
        symbol: SymbolId,
        target: SymbolId,
        target_flags: u32,
        type_only: bool,
    ) -> Result<bool, Error> {
        let read = self.node(node)?;
        if read.flags() & nf::JAVA_SCRIPT_FILE == 0 || target_flags & sf::VALUE != 0 || type_only {
            return Ok(false);
        }
        let export = read.kind() == K::ExportSpecifier;
        let name = self.alias_property_name(node)?.unwrap_or(node);
        if export {
            let diagnostic = self.error_at(
                Some(name),
                d::Types_cannot_appear_in_export_declarations_in_JavaScript_files,
                vec![],
            )?;
            let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
                .ok_or(Error::MissingLink("alias source"))?;
            let source_symbol = self
                .program()?
                .bound(source)?
                .node_binding(source)?
                .and_then(|binding| binding.symbol);
            if let Some(source_symbol) = source_symbol {
                let text = self.node_text(name)?.into_js_string();
                if self.member_symbol(self.symbol(source_symbol)?.exports(), text.as_bytes())?
                    == Some(target)
                {
                    for declaration in self
                        .symbol_declarations(target)?
                        .to_vec()
                        .into_iter()
                        .flatten()
                    {
                        if ts_ast::is_js_type_alias_declaration(&self.node(declaration)?) {
                            if let Some(diagnostic) = diagnostic {
                                let text = self.symbol(target)?.name_to_owned();
                                let related = self.diagnostic_for_node(
                                    Some(declaration),
                                    d::X_0_is_automatically_exported_here,
                                    vec![text],
                                )?;
                                self.add_related_diagnostic(diagnostic, related)?;
                            }
                            break;
                        }
                    }
                }
            }
        } else {
            let text = if self.node(name)?.kind() == K::Identifier {
                self.node_text(name)?.into_js_string()
            } else {
                self.symbol(symbol)?.name_to_owned()
            };
            let specifier = self.module_specifier(node)?;
            let specifier = match specifier {
                Some(specifier) => self.node_text(specifier)?.into_js_string(),
                None => JsString::from_bytes(b"...".as_slice()),
            };
            let mut import = b"import(\"".to_vec();
            import.extend_from_slice(specifier.as_bytes());
            import.extend_from_slice(b"\")");
            if self.node(node)?.kind() == K::ImportSpecifier {
                import.push(b'.');
                import.extend_from_slice(text.as_bytes());
            }
            self.error_at(Some(name),d::X_0_is_a_type_and_cannot_be_imported_in_JavaScript_files_Use_1_in_a_JSDoc_type_annotation,vec![text,JsString::from_bytes(import)])?;
        }
        Ok(true)
    }
    // port: tsc/internal/checker/checker.go:Checker.addTypeOnlyDeclarationRelatedInfo
    pub(crate) fn alias_type_only_related(
        &mut self,
        diagnostic: Option<usize>,
        declaration: Option<NodeId>,
        name: JsString,
    ) -> Result<(), Error> {
        if let (Some(diagnostic), Some(declaration)) = (diagnostic, declaration) {
            let export = matches!(
                self.node(declaration)?.kind().known(),
                Some(K::ExportSpecifier | K::ExportDeclaration | K::NamespaceExport)
            );
            let related = self.diagnostic_for_node(
                Some(declaration),
                if export {
                    d::X_0_was_exported_here
                } else {
                    d::X_0_was_imported_here
                },
                vec![name],
            )?;
            self.add_related_diagnostic(diagnostic, related)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkAliasSymbol
    pub(crate) fn check_isolated_alias(
        &mut self,
        node: NodeId,
        symbol: SymbolId,
        target: SymbolId,
        target_flags: u32,
    ) -> Result<(), Error> {
        let kind = self.node(node)?.kind();
        let verbatim = self
            .program()?
            .host
            .options()
            .verbatim_module_syntax
            .is_true();
        let type_only = self.direct_type_only_alias_declaration(symbol)?;
        let is_type = target_flags & sf::VALUE == 0;
        if is_type || type_only.is_some() {
            let name = self
                .alias_property_name(node)?
                .ok_or(Error::MissingLink("isolated alias name"))?;
            let name = self.node_text(name)?.into_js_string();
            match kind.known() {
                Some(K::ImportClause | K::ImportSpecifier | K::ImportEqualsDeclaration) => {
                    if verbatim {
                        let internal = if kind == K::ImportEqualsDeclaration {
                            let reference = self
                                .ast(node)?
                                .node(node)?
                                .data_source()
                                .as_import_equals_declaration()
                                .and_then(|data| data.module_reference())
                                .ok_or(Error::MissingLink("import alias reference"))?;
                            self.node(reference)?.kind() != K::ExternalModuleReference
                        } else {
                            false
                        };
                        let message = if internal {
                            d::An_import_alias_cannot_resolve_to_a_type_or_type_only_declaration_when_verbatimModuleSyntax_is_enabled
                        } else if is_type {
                            d::X_0_is_a_type_and_must_be_imported_using_a_type_only_import_when_verbatimModuleSyntax_is_enabled
                        } else {
                            d::X_0_resolves_to_a_type_only_declaration_and_must_be_imported_using_a_type_only_import_when_verbatimModuleSyntax_is_enabled
                        };
                        let diagnostic = self.error_at(Some(node), message, vec![name.clone()])?;
                        self.alias_type_only_related(
                            diagnostic,
                            if is_type { None } else { type_only },
                            name.clone(),
                        )?;
                    }
                    if is_type
                        && kind == K::ImportEqualsDeclaration
                        && self.effective_declaration_flags(node, mf::EXPORT)? & mf::EXPORT != 0
                    {
                        let flag = self.module_isolated_flag_name();
                        self.error_at(Some(node),d::Cannot_use_export_import_on_a_type_or_type_only_namespace_when_0_is_enabled,vec![flag])?;
                    }
                }
                Some(K::ExportSpecifier) => {
                    let current =
                        ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?;
                    let origin = match type_only {
                        Some(declaration) => ts_ast::utilities::get_source_file_of_node(
                            self.ast(declaration)?,
                            Some(declaration),
                        )?,
                        None => None,
                    };
                    if verbatim || current != origin {
                        let flag = self.module_isolated_flag_name();
                        let diagnostic = if is_type {
                            self.error_at(
                                Some(node),
                                d::Re_exporting_a_type_when_0_is_enabled_requires_using_export_type,
                                vec![flag],
                            )?
                        } else {
                            self.error_at(Some(node),d::X_0_resolves_to_a_type_only_declaration_and_must_be_re_exported_using_a_type_only_re_export_when_1_is_enabled,vec![name.clone(),flag])?
                        };
                        self.alias_type_only_related(
                            diagnostic,
                            if is_type { None } else { type_only },
                            name,
                        )?;
                    }
                }
                _ => {}
            }
        }
        let format = self.module_emit_format(node)?;
        if verbatim
            && kind != K::ImportEqualsDeclaration
            && self.node(node)?.flags() & nf::JAVA_SCRIPT_FILE == 0
            && format == ModuleKind::COMMON_JS
        {
            let (_, file) = self.module_source(node)?;
            let message = if file.as_bytes().ends_with(b".cts")
                || file.as_bytes().ends_with(b".cjs")
            {
                d::ECMAScript_imports_and_exports_cannot_be_written_in_a_CommonJS_file_under_verbatimModuleSyntax
            } else {
                d::ECMAScript_imports_and_exports_cannot_be_written_in_a_CommonJS_file_under_verbatimModuleSyntax_Adjust_the_type_field_in_the_nearest_package_json_to_make_this_file_an_ECMAScript_module_or_adjust_your_verbatimModuleSyntax_module_and_moduleResolution_settings_in_TypeScript
            };
            self.error_at(Some(node), message, vec![])?;
        } else if self.program()?.host.options().emit_module_kind() == ModuleKind::PRESERVE
            && !matches!(
                kind.known(),
                Some(K::ImportEqualsDeclaration | K::VariableDeclaration | K::BindingElement)
            )
            && format == ModuleKind::COMMON_JS
        {
            self.error_at(Some(node),d::ECMAScript_module_syntax_is_not_allowed_in_a_CommonJS_module_when_module_is_set_to_preserve,vec![])?;
        }
        if verbatim && target_flags & sf::CONST_ENUM != 0 {
            let declaration = self
                .symbol(target)?
                .value_declaration()
                .ok_or(Error::MissingLink("const enum alias declaration"))?;
            let source = ts_ast::utilities::get_source_file_of_node(
                self.ast(declaration)?,
                Some(declaration),
            )?
            .ok_or(Error::MissingLink("const enum source"))?;
            let path = self
                .ast(source)?
                .source_file(source)?
                .parse_options()
                .path
                .clone();
            let redirect = self
                .program()?
                .host
                .get_project_reference_from_output_dts(path.as_bytes())?;
            if self.node(declaration)?.flags() & nf::AMBIENT != 0
                && redirect.is_none_or(|redirect| !redirect.options.should_preserve_const_enums())
            {
                let flag = self.module_isolated_flag_name();
                self.error_at(
                    Some(node),
                    d::Cannot_access_ambient_const_enums_when_0_is_enabled,
                    vec![flag],
                )?;
            }
        }
        Ok(())
    }
}
