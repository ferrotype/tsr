//! Source imports use retained loader resolutions; grammar failures preserve the
//! native early-return boundaries before resolving individual aliases.
use crate::{CheckerState, Error};
use ts_arena::NodeId;
use ts_ast::{node_flags as nf, JsString, SyntaxKind as K};
use ts_core::ModuleKind;
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkImportDeclaration
    pub(crate) fn check_import_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let javascript = read.flags() & nf::JAVA_SCRIPT_FILE != 0;
        let modifiers = read.modifiers();
        let data = read
            .data_source()
            .as_import_declaration()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let clause = data.import_clause();
        let specifier = data
            .module_specifier()
            .ok_or(Error::MissingLink("import module specifier"))?;
        if self.check_grammar_module_context(
            node,
            if javascript {
                d::An_import_declaration_can_only_be_used_at_the_top_level_of_a_module
            } else {
                d::An_import_declaration_can_only_be_used_at_the_top_level_of_a_namespace_or_module
            },
        )? {
            self.check_external_module_name_in_global_scope(node)?;
            return Ok(());
        }
        if !self.check_grammar_modifiers(node)? && modifiers.is_some() {
            self.grammar_error_first_token(
                node,
                d::An_import_declaration_cannot_have_modifiers,
                vec![],
            )?;
        }
        if self.check_external_import_or_export(node)? {
            if let Some(clause) = clause {
                if !self.check_grammar_import_clause(clause)? {
                    let read = self.ast(clause)?.node(clause)?;
                    let data = read
                        .data_source()
                        .as_import_clause()
                        .ok_or(ts_arena::Error::InvalidGraph)?;
                    let default = data.name().is_some();
                    let named = data.named_bindings();
                    let type_only = data.phase_modifier() == K::TypeKeyword;
                    let mut needs_star = false;
                    let mut resolved = None;
                    if default {
                        self.check_import_binding(clause)?;
                    }
                    if let Some(named) = named {
                        if self.ast(named)?.node(named)?.kind() == K::NamespaceImport {
                            self.check_import_binding(named)?;
                            if self.module_emit_format(node)? == ModuleKind::COMMON_JS {
                                needs_star = true;
                                self.check_external_emit_helpers(
                                    node,
                                    crate::external_emit_helpers::IMPORT_STAR,
                                )?;
                            }
                        } else {
                            resolved = self.resolve_external_module_name(node, specifier, false)?;
                            if resolved.is_some() {
                                for binding in self.source_list(
                                    named,
                                    self.ast(named)?.node(named)?.element_list(),
                                )? {
                                    self.check_import_binding(binding)?;
                                }
                            }
                        }
                    }
                    if default
                        && !needs_star
                        && self.module_emit_format(node)? == ModuleKind::COMMON_JS
                    {
                        self.check_external_emit_helpers(
                            node,
                            crate::external_emit_helpers::IMPORT_DEFAULT,
                        )?;
                    }
                    let kind = self.program()?.host.options().emit_module_kind();
                    if !type_only
                        && (ModuleKind::NODE18..=ModuleKind::NODE_NEXT).contains(&kind)
                        && self.module_only_importable_as_default(specifier, resolved)?
                        && !self.has_type_json_import_attribute(node)?
                    {
                        self.error_at(Some(specifier),d::Importing_a_JSON_file_into_an_ECMAScript_module_requires_a_type_Colon_json_import_attribute_when_module_is_set_to_0,vec![JsString::from_bytes(match kind {ModuleKind::NODE18=>b"node18".as_slice(),ModuleKind::NODE20=>b"node20".as_slice(),_=>b"nodenext".as_slice()})])?;
                    }
                }
            } else if self
                .program()?
                .host
                .options()
                .no_unchecked_side_effect_imports
                .is_true_or_unknown()
            {
                let ignore = self.program()?.host.options().no_check.is_true();
                self.resolve_external_module_name_with_error(
                    node,
                    specifier,
                    ignore,
                    (!ignore).then_some(
                        d::Cannot_find_module_or_type_declarations_for_side_effect_import_of_0,
                    ),
                    false,
                )?;
            }
        }
        self.check_import_attributes(node)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkExternalImportOrExportDeclaration
    pub(crate) fn check_external_import_or_export(&mut self, node: NodeId) -> Result<bool, Error> {
        let Some(name) = self.module_specifier(node)? else {
            return Ok(false);
        };
        let read = self.ast(name)?.node(name)?;
        if read.pos() == read.end() {
            return Ok(false);
        }
        if read.kind() != K::StringLiteral {
            self.error_at(Some(name), d::String_literal_expected, vec![])?;
            return Ok(false);
        }
        let parent = self
            .ast(node)?
            .node(node)?
            .parent()
            .ok_or(Error::MissingLink("external declaration parent"))?;
        let parent_read = self.ast(parent)?.node(parent)?;
        let ambient = parent_read.kind() == K::ModuleBlock
            && if let Some(module) = parent_read.parent() {
                ts_ast::is_ambient_module(self.ast(module)?, module)?
            } else {
                false
            };
        if parent_read.kind() != K::SourceFile && !ambient {
            self.error_at(
                Some(name),
                if self.ast(node)?.node(node)?.kind() == K::ExportDeclaration {
                    d::Export_declarations_are_not_permitted_in_a_namespace
                } else {
                    d::Import_declarations_in_a_namespace_cannot_reference_a_module
                },
                vec![],
            )?;
            return Ok(false);
        }
        if ambient
            && ts_module::is_relative(self.ast(name)?.node_text(name)?.as_bytes())
            && !self.top_level_module_augmentation(node)?
        {
            self.error_at(Some(node),d::Import_or_export_declaration_in_an_ambient_module_declaration_cannot_reference_module_through_relative_module_name,vec![])?;
            return Ok(false);
        }
        let read = self.ast(node)?.node(node)?;
        let attributes = match read.kind().known() {
            Some(K::ImportDeclaration) => read
                .data_source()
                .as_import_declaration()
                .and_then(|d| d.attributes()),
            Some(K::ExportDeclaration) => read
                .data_source()
                .as_export_declaration()
                .and_then(|d| d.attributes()),
            _ => None,
        };
        if let Some(attributes) = attributes {
            if self.check_grammar_import_attribute_values(attributes)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    // port: tsc/internal/checker/utilities.go:isTopLevelInExternalModuleAugmentation
    pub(crate) fn top_level_module_augmentation(&self, node: NodeId) -> Result<bool, Error> {
        let Some(parent) = self.ast(node)?.node(node)?.parent() else {
            return Ok(false);
        };
        let read = self.ast(parent)?.node(parent)?;
        if read.kind() != K::ModuleBlock {
            return Ok(false);
        }
        let Some(module) = read.parent() else {
            return Ok(false);
        };
        Ok(ts_ast::is_ambient_module(self.ast(module)?, module)?
            && ts_ast::is_module_augmentation_external(self.ast(module)?, module)?)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkExternalModuleNameInGlobalScope
    pub(crate) fn check_external_module_name_in_global_scope(
        &mut self,
        node: NodeId,
    ) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        if read.kind() == K::ImportDeclaration
            && read
                .data_source()
                .as_import_declaration()
                .and_then(|d| d.import_clause())
                .is_none()
        {
            return Ok(());
        }
        let mut parent = read.parent();
        while let Some(current) = parent {
            if ts_binder::get_container_flags(self.ast(current)?, current)?.0
                & ts_binder::ContainerFlags::IS_CONTAINER
                != 0
            {
                if self.ast(current)?.node(current)?.kind() == K::SourceFile {
                    if let Some(name) = self.module_specifier(node)? {
                        self.resolve_external_module_name(node, name, false)?;
                    }
                }
                return Ok(());
            }
            parent = self.ast(current)?.node(current)?.parent();
        }
        Ok(())
    }
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarImportClause
    fn check_grammar_import_clause(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        let jsdoc = read.flags() & nf::JS_DOC != 0;
        let data = read
            .data_source()
            .as_import_clause()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let phase = data.phase_modifier();
        let name = data.name();
        let bindings = data.named_bindings();
        if phase == K::TypeKeyword {
            if !jsdoc && name.is_some() && bindings.is_some() {
                return self.grammar_error_node(node,d::A_type_only_import_can_specify_a_default_import_or_named_bindings_but_not_both,vec![]);
            }
            if let Some(bindings) = bindings {
                if self.ast(bindings)?.node(bindings)?.kind() == K::NamedImports {
                    for element in self
                        .source_list(bindings, self.ast(bindings)?.node(bindings)?.element_list())?
                    {
                        if self.ast(element)?.node(element)?.is_type_only() {
                            return self.grammar_error_first_token(element,d::The_type_modifier_cannot_be_used_on_a_named_import_when_import_type_is_used_on_its_import_statement,vec![]);
                        }
                    }
                }
            }
        } else if phase == K::DeferKeyword {
            if name.is_some() {
                return self.grammar_error_node(
                    node,
                    d::Default_imports_are_not_allowed_in_a_deferred_import,
                    vec![],
                );
            }
            if let Some(bindings) = bindings {
                if self.ast(bindings)?.node(bindings)?.kind() == K::NamedImports {
                    return self.grammar_error_node(
                        node,
                        d::Named_imports_are_not_allowed_in_a_deferred_import,
                        vec![],
                    );
                }
            }
            if !matches!(
                self.program()?.host.options().emit_module_kind(),
                ModuleKind::ESNEXT | ModuleKind::PRESERVE
            ) {
                return self.grammar_error_node(node,d::Deferred_imports_are_only_supported_when_the_module_flag_is_set_to_esnext_or_preserve,vec![]);
            }
        }
        Ok(false)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkImportBinding
    pub(crate) fn check_import_binding(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let name = read
            .name()
            .ok_or(Error::MissingLink("import binding name"))?;
        let import = read.kind() == K::ImportSpecifier;
        self.check_module_name_collision(node, name)?;
        self.check_source_alias_symbol(node)?;
        if import {
            let read = self.ast(node)?.node(node)?;
            let property = read.property_name();
            self.check_module_export_name(property, true)?;
            if self
                .ast(property.unwrap_or(name))?
                .node_text(property.unwrap_or(name))?
                .as_bytes()
                == b"default"
                && self.module_emit_format(node)? == ModuleKind::COMMON_JS
            {
                self.check_external_emit_helpers(
                    node,
                    crate::external_emit_helpers::IMPORT_DEFAULT,
                )?;
            }
        }
        Ok(())
    }
    pub(crate) fn module_emit_format(&self, node: NodeId) -> Result<ModuleKind, Error> {
        let (_, file) = self.module_source(node)?;
        self.program()?
            .host
            .get_emit_module_format_of_file(file.as_bytes())
    }
}
