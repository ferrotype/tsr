//! Source checks for internal import aliases and local named exports.
use crate::{CheckerState, Error};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    modifier_flags as mf, node_flags as nf, symbol_flags as sf, JsString, SyntaxKind as K,
};
use ts_diagnostics as d;
fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(name))
}
impl CheckerState {
    /// The CommonJS branch of checkVariableLikeDeclaration precedes ordinary
    /// variable type/initializer checks, including for destructured aliases.
    pub(crate) fn check_require_alias_declaration(
        &mut self,
        node: NodeId,
        symbol: SymbolId,
    ) -> Result<bool, Error> {
        if self.symbol(symbol)?.flags() & sf::ALIAS != 0
            && ts_ast::is_variable_declaration_initialized_to_require(self.ast(node)?, node)?
        {
            self.check_source_alias_symbol(node)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkExportDeclaration
    pub(crate) fn check_export_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let javascript = read.flags() & nf::JAVA_SCRIPT_FILE != 0;
        let ambient = read.flags() & nf::AMBIENT != 0;
        let parent = required(read.parent(), "export parent")?;
        let modifiers = read.modifiers();
        let data = read
            .data_source()
            .as_export_declaration()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let specifier = data.module_specifier();
        let clause = data.export_clause();
        let type_only = data.is_type_only();
        let message = if javascript {
            d::An_export_declaration_can_only_be_used_at_the_top_level_of_a_module
        } else {
            d::An_export_declaration_can_only_be_used_at_the_top_level_of_a_namespace_or_module
        };
        if self.check_grammar_module_context(node, message)? {
            self.check_external_module_name_in_global_scope(node)?;
            return Ok(());
        }
        if !self.check_grammar_modifiers(node)? && modifiers.is_some() {
            self.grammar_error_first_token(
                node,
                d::An_export_declaration_cannot_have_modifiers,
                vec![],
            )?;
        }
        if type_only {
            if let Some(clause) = clause {
                if self.ast(clause)?.node(clause)?.kind() == K::NamedExports {
                    for element in
                        self.source_list(clause, self.ast(clause)?.node(clause)?.element_list())?
                    {
                        if self.ast(element)?.node(element)?.is_type_only() {
                            self.grammar_error_first_token(element,d::The_type_modifier_cannot_be_used_on_a_named_export_when_export_type_is_used_on_its_export_statement,vec![])?;
                            break;
                        }
                    }
                }
            }
        }
        if specifier.is_none() || self.check_external_import_or_export(node)? {
            if let Some(clause) = clause {
                if self.ast(clause)?.node(clause)?.kind() == K::NamespaceExport {
                    self.check_export_star(node, Some(clause), specifier)?;
                } else {
                    for element in
                        self.source_list(clause, self.ast(clause)?.node(clause)?.element_list())?
                    {
                        self.check_export_specifier(element)?;
                    }
                    let in_block = self.ast(parent)?.node(parent)?.kind() == K::ModuleBlock;
                    let external = in_block
                        && ts_ast::is_ambient_module(
                            self.ast(parent)?,
                            required(
                                self.ast(parent)?.node(parent)?.parent(),
                                "export module parent",
                            )?,
                        )?;
                    let ambient_namespace = !external && in_block && specifier.is_none() && ambient;
                    if self.ast(parent)?.node(parent)?.kind() != K::SourceFile
                        && !external
                        && !ambient_namespace
                    {
                        self.error_at(
                            Some(node),
                            d::Export_declarations_are_not_permitted_in_a_namespace,
                            vec![],
                        )?;
                    }
                }
            } else {
                self.check_export_star(node, None, specifier)?;
            }
        }
        self.check_import_attributes(node)
    }
    fn check_export_star(
        &mut self,
        node: NodeId,
        clause: Option<NodeId>,
        specifier: Option<NodeId>,
    ) -> Result<(), Error> {
        let Some(specifier) = specifier else {
            return Ok(());
        };
        let module = self.resolve_external_module_name(node, specifier, false)?;
        let export_equals = if let Some(module) = module {
            self.member_symbol(
                self.symbol(module)?.exports(),
                ts_ast::internal_symbol_names::EXPORT_EQUALS,
            )?
            .is_some()
        } else {
            false
        };
        if export_equals {
            let text = self.symbol_to_string(module.ok_or(Error::MissingLink("star module"))?)?;
            self.error_at(
                Some(specifier),
                d::Module_0_uses_export_and_cannot_be_used_with_export_Asterisk,
                vec![text],
            )?;
        } else if let Some(clause) = clause {
            self.check_source_alias_symbol(clause)?;
            self.check_module_export_name(self.ast(clause)?.node(clause)?.name(), true)?;
        }
        if self.module_emit_format(node)? == ts_core::ModuleKind::COMMON_JS {
            self.check_external_emit_helpers(
                node,
                if clause.is_some() {
                    // export * as ns from "foo";
                    crate::external_emit_helpers::IMPORT_STAR
                } else {
                    // export * from "foo"
                    crate::external_emit_helpers::EXPORT_STAR
                },
            )?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkExportAssignment
    pub(crate) fn check_export_assignment(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let expression = required(read.expression(), "export assignment expression")?;
        let export_equals = read
            .data_source()
            .as_export_assignment()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .is_export_equals();
        let ambient = read.flags() & nf::AMBIENT != 0;
        let javascript = read.flags() & nf::JAVA_SCRIPT_FILE != 0;
        let expression_type = self.check_expression_cached(expression)?;
        let context = if export_equals {
            d::An_export_assignment_must_be_at_the_top_level_of_a_file_or_module_declaration
        } else {
            d::A_default_export_must_be_at_the_top_level_of_a_file_or_module_declaration
        };
        if self.check_grammar_module_context(node, context)? {
            return Ok(());
        }
        if self
            .program()?
            .host
            .options()
            .erasable_syntax_only
            .is_true()
            && !javascript
            && export_equals
            && !ambient
        {
            self.error_at(
                Some(node),
                d::This_syntax_is_not_allowed_when_erasableSyntaxOnly_is_enabled,
                vec![],
            )?;
        }
        if self.contained_by_namespace(node)? {
            self.error_at(
                Some(node),
                if export_equals {
                    d::An_export_assignment_cannot_be_used_in_a_namespace
                } else {
                    d::A_default_export_can_only_be_used_in_an_ECMAScript_style_module
                },
                vec![],
            )?;
            return Ok(());
        }
        if !self.check_grammar_modifiers(node)? && self.ast(node)?.node(node)?.modifiers().is_some()
        {
            self.grammar_error_first_token(
                node,
                d::An_export_assignment_cannot_have_modifiers,
                vec![],
            )?;
        }
        let options = self.program()?.host.options();
        let verbatim = options.verbatim_module_syntax.is_true();
        let isolated = options.isolated_modules();
        let module_kind = options.emit_module_kind();
        let illegal_default = !export_equals
            && !ambient
            && verbatim
            && self.module_emit_format(node)? == ts_core::ModuleKind::COMMON_JS;
        if self.ast(expression)?.node(expression)?.kind() == K::Identifier {
            if let Some(symbol) =
                self.resolve_entity_name_at(expression, sf::ALL, true, true, Some(node))?
            {
                let symbol = self.get_export_symbol_of_value_symbol_if_exported(symbol)?;
                self.mark_module_export_referenced(node)?;
                let type_only = self.module_type_only_alias(symbol, sf::VALUE)?;
                let text = self
                    .ast(expression)?
                    .node_text(expression)?
                    .into_js_string();
                if self.module_symbol_flags(symbol, false, false)? & sf::VALUE != 0 {
                    if !illegal_default && !ambient && verbatim && type_only.is_some() {
                        self.error_at(Some(expression),if export_equals{d::An_export_declaration_must_reference_a_real_value_when_verbatimModuleSyntax_is_enabled_but_0_resolves_to_a_type_only_declaration}else{d::An_export_default_must_reference_a_real_value_when_verbatimModuleSyntax_is_enabled_but_0_resolves_to_a_type_only_declaration},vec![text.clone()])?;
                    }
                } else if !illegal_default && !ambient && verbatim {
                    self.error_at(Some(expression),if export_equals{d::An_export_declaration_must_reference_a_value_when_verbatimModuleSyntax_is_enabled_but_0_only_refers_to_a_type}else{d::An_export_default_must_reference_a_value_when_verbatimModuleSyntax_is_enabled_but_0_only_refers_to_a_type},vec![text.clone()])?;
                }
                if !illegal_default
                    && !ambient
                    && isolated
                    && self.symbol(symbol)?.flags() & sf::VALUE == 0
                {
                    let other_file = if let Some(declaration) = type_only {
                        self.module_source(declaration)?.0 != self.module_source(node)?.0
                    } else {
                        true
                    };
                    let meanings = self.module_symbol_flags(symbol, false, true)?;
                    if self.symbol(symbol)?.flags() & sf::ALIAS != 0
                        && meanings & sf::TYPE != 0
                        && meanings & sf::VALUE == 0
                        && other_file
                    {
                        self.error_at(Some(expression),if export_equals{d::X_0_resolves_to_a_type_and_must_be_marked_type_only_in_this_file_before_re_exporting_when_1_is_enabled_Consider_using_import_type_where_0_is_imported}else{d::X_0_resolves_to_a_type_and_must_be_marked_type_only_in_this_file_before_re_exporting_when_1_is_enabled_Consider_using_export_type_0_as_default},vec![text.clone(),self.module_isolated_flag_name()])?;
                    } else if type_only.is_some() && other_file {
                        // import { SomeTypeOnlyValue } from "./someModule";
                        // export default SomeTypeOnlyValue; OR
                        // export = SomeTypeOnlyValue;
                        let index = self.error_at(
                            Some(expression),
                            if export_equals {
                                d::X_0_resolves_to_a_type_only_declaration_and_must_be_marked_type_only_in_this_file_before_re_exporting_when_1_is_enabled_Consider_using_import_type_where_0_is_imported
                            } else {
                                d::X_0_resolves_to_a_type_only_declaration_and_must_be_marked_type_only_in_this_file_before_re_exporting_when_1_is_enabled_Consider_using_export_type_0_as_default
                            },
                            vec![text.clone(), self.module_isolated_flag_name()],
                        )?;
                        self.alias_type_only_related(index, type_only, text.clone())?;
                    }
                }
            }
        }
        if illegal_default {
            let (_, file) = self.module_source(node)?;
            self.error_at(Some(node),if file.as_bytes().ends_with(b".cts")||file.as_bytes().ends_with(b".cjs"){d::ECMAScript_imports_and_exports_cannot_be_written_in_a_CommonJS_file_under_verbatimModuleSyntax}else{d::ECMAScript_imports_and_exports_cannot_be_written_in_a_CommonJS_file_under_verbatimModuleSyntax_Adjust_the_type_field_in_the_nearest_package_json_to_make_this_file_an_ECMAScript_module_or_adjust_your_verbatimModuleSyntax_module_and_moduleResolution_settings_in_TypeScript},vec![])?;
        }
        let mut container = required(
            self.ast(node)?.node(node)?.parent(),
            "export assignment container",
        )?;
        if self.ast(container)?.node(container)?.kind() != K::SourceFile {
            container = required(
                self.ast(container)?.node(container)?.parent(),
                "export assignment module",
            )?;
        }
        self.check_external_module_exports(container)?;
        if let Some(annotation) = self.ast(node)?.node(node)?.type_node() {
            let ty = self.get_type_from_type_node(annotation)?;
            self.check_assignable_at(expression_type, ty, expression)?;
        }
        if ambient && !ts_ast::is_entity_name_expression(self.ast(expression)?, expression)? {
            self.grammar_error_node(expression,d::The_expression_of_an_export_assignment_must_be_an_identifier_or_qualified_name_in_an_ambient_context,vec![])?;
        }
        if export_equals {
            let (_, file) = self.module_source(node)?;
            let implied = self
                .program()?
                .host
                .get_implied_node_format_for_emit(file.as_bytes())?;
            if module_kind >= ts_core::ModuleKind::ES2015
                && module_kind != ts_core::ModuleKind::PRESERVE
                && ((ambient && implied == ts_core::ModuleKind::ESNEXT)
                    || (!ambient && implied != ts_core::ModuleKind::COMMON_JS))
            {
                self.grammar_error_node(node,d::Export_assignment_cannot_be_used_when_targeting_ECMAScript_modules_Consider_using_export_default_or_another_module_format_instead,vec![])?;
            } else if module_kind == ts_core::ModuleKind::SYSTEM && !ambient {
                self.grammar_error_node(
                    node,
                    d::Export_assignment_is_not_supported_when_module_flag_is_system,
                    vec![],
                )?;
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkExportSpecifier
    pub(crate) fn check_export_specifier(&mut self, node: NodeId) -> Result<(), Error> {
        self.check_source_alias_symbol(node)?;
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_export_specifier()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let name = required(data.name(), "export specifier name")?;
        let property = data.property_name();
        let type_only = data.is_type_only();
        let ambient = read.flags() & nf::AMBIENT != 0;
        let parent = required(read.parent(), "export list")?;
        let declaration = required(
            self.ast(parent)?.node(parent)?.parent(),
            "export declaration",
        )?;
        let read = self.ast(declaration)?.node(declaration)?;
        let declaration_data = read
            .data_source()
            .as_export_declaration()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let external = declaration_data.module_specifier().is_some();
        let declaration_type_only = declaration_data.is_type_only();
        self.check_module_export_name(property, external)?;
        self.check_module_export_name(Some(name), true)?;
        if external {
            if self.module_emit_format(node)? == ts_core::ModuleKind::COMMON_JS
                && self
                    .ast(property.unwrap_or(name))?
                    .node_text(property.unwrap_or(name))?
                    .as_bytes()
                    == b"default"
            {
                self.check_external_emit_helpers(
                    node,
                    crate::external_emit_helpers::IMPORT_DEFAULT,
                )?;
            }
            return Ok(());
        }
        let exported_name = property.unwrap_or(name);
        if self.ast(exported_name)?.node(exported_name)?.kind() == K::StringLiteral {
            return Ok(());
        }
        let text = self
            .ast(exported_name)?
            .node_text(exported_name)?
            .into_js_string();
        let symbol = self.resolve_name(
            Some(exported_name),
            text.as_bytes(),
            sf::VALUE | sf::TYPE | sf::NAMESPACE | sf::ALIAS,
            None,
            true,
        )?;
        let nonlocal = if let Some(symbol) = symbol {
            if symbol == self.builtins.undefined_symbol
                || symbol == self.builtins.global_this_symbol
            {
                true
            } else {
                let first = self.symbol_declarations(symbol)?.iter().flatten().next();
                if let Some(first) = first {
                    if let Some(container) =
                        ts_ast::get_declaration_container(self.ast(first)?, first)?
                    {
                        ts_ast::utilities_middle::is_global_source_file(
                            self.ast(container)?,
                            container,
                        )?
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        } else {
            false
        };
        if nonlocal {
            self.error_at(
                Some(exported_name),
                d::Cannot_export_0_Only_local_declarations_can_be_exported_from_a_module,
                vec![text],
            )?;
        } else if !ambient
            && !type_only
            && !declaration_type_only
            && !self
                .program()?
                .host
                .options()
                .verbatim_module_syntax
                .is_true()
        {
            let target = if let Some(symbol) = symbol {
                Some(if self.symbol(symbol)?.flags() & sf::ALIAS != 0 {
                    self.resolve_alias(symbol)?
                } else {
                    symbol
                })
            } else {
                None
            };
            if target.is_none()
                || self.module_symbol_flags(
                    target.expect("target is present in right operand"),
                    false,
                    false,
                )? & sf::VALUE
                    != 0
            {
                self.mark_module_export_referenced(node)?;
                if let Some(symbol) = symbol {
                    if ts_ast::is_non_local_alias(Some(&self.symbol(symbol)?), sf::VALUE)
                        && !self.module_aliases.type_only.contains_key(&symbol)
                    {
                        self.mark_module_alias_referenced(symbol)?;
                    }
                }
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkModuleExportName
    pub(crate) fn check_module_export_name(
        &mut self,
        name: Option<NodeId>,
        allow_string: bool,
    ) -> Result<(), Error> {
        let Some(name) = name else {
            return Ok(());
        };
        if self.ast(name)?.node(name)?.kind() != K::StringLiteral {
            return Ok(());
        }
        if !allow_string {
            self.grammar_error_node(name, d::Identifier_expected, vec![])?;
        } else if matches!(
            self.program()?.host.options().emit_module_kind(),
            ts_core::ModuleKind::ES2015 | ts_core::ModuleKind::ES2020
        ) {
            let source = required(
                ts_ast::utilities::get_source_file_of_node(self.ast(name)?, Some(name))?,
                "export-name source",
            )?;
            if !self.ast(source)?.source_file(source)?.is_declaration_file {
                self.grammar_error_node(name, d::String_literal_import_and_export_names_are_not_supported_when_the_module_flag_is_set_to_es2015_or_es2020, vec![])?;
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkImportEqualsDeclaration
    pub(crate) fn check_import_equals_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let javascript = read.flags() & nf::JAVA_SCRIPT_FILE != 0;
        let ambient = read.flags() & nf::AMBIENT != 0;
        let data = read
            .data_source()
            .as_import_equals_declaration()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let reference = required(data.module_reference(), "import equals reference")?;
        let name = required(data.name(), "import equals name")?;
        let type_only = data.is_type_only();
        let context = if javascript {
            d::An_import_declaration_can_only_be_used_at_the_top_level_of_a_module
        } else {
            d::An_import_declaration_can_only_be_used_at_the_top_level_of_a_namespace_or_module
        };
        if self.check_grammar_module_context(node, context)? {
            self.check_external_module_name_in_global_scope(node)?;
            return Ok(());
        }
        self.check_grammar_modifiers(node)?;
        if self
            .program()?
            .host
            .options()
            .erasable_syntax_only
            .is_true()
            && !javascript
            && !ambient
        {
            self.error_at(
                Some(node),
                d::This_syntax_is_not_allowed_when_erasableSyntaxOnly_is_enabled,
                vec![],
            )?;
        }
        if self.ast(reference)?.node(reference)?.kind() == K::ExternalModuleReference {
            if self.check_external_import_or_export(node)? {
                self.check_import_binding(node)?;
                if self
                    .ast(node)?
                    .node(node)?
                    .modifier_flags(self.ast(node)?)?
                    & mf::EXPORT
                    != 0
                {
                    self.mark_module_export_referenced(node)?;
                }
                if (ts_core::ModuleKind::ES2015..=ts_core::ModuleKind::ESNEXT)
                    .contains(&self.program()?.host.options().emit_module_kind())
                    && !type_only
                    && !ambient
                {
                    self.grammar_error_node(node,d::Import_assignment_cannot_be_used_when_targeting_ECMAScript_modules_Consider_using_import_Asterisk_as_ns_from_mod_import_a_from_mod_import_d_from_mod_or_another_module_format_instead,vec![])?;
                }
            }
            return Ok(());
        }
        self.check_module_name_collision(node, name)?;
        self.check_source_alias_symbol(node)?;
        if self
            .ast(node)?
            .node(node)?
            .modifier_flags(self.ast(node)?)?
            & mf::EXPORT
            != 0
        {
            self.mark_module_export_referenced(node)?;
        }
        let symbol = required(self.get_symbol_of_declaration(node)?, "import alias symbol")?;
        let target = self.resolve_alias(symbol)?;
        if target != self.builtins.unknown_symbol {
            let flags = self.module_symbol_flags(target, false, false)?;
            if flags & sf::VALUE != 0 {
                let first = ts_ast::utilities_middle::get_first_identifier(
                    self.ast(reference)?,
                    reference,
                )?;
                if let Some(resolved) =
                    self.resolve_entity_name(first, sf::VALUE | sf::NAMESPACE, false)?
                {
                    if self.symbol(resolved)?.flags() & sf::NAMESPACE == 0 {
                        let text =
                            ts_scanner::declaration_name_to_string(self.ast(first)?, Some(first))?;
                        self.error_at(
                            Some(first),
                            d::Module_0_is_hidden_by_a_local_declaration_with_the_same_name,
                            vec![text],
                        )?;
                    }
                }
            }
            if flags & sf::TYPE != 0 {
                self.check_module_reserved_type_name(name, d::Import_name_cannot_be_0)?;
            }
        }
        if type_only {
            self.grammar_error_node(node, d::An_import_alias_cannot_use_import_type, vec![])?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkAliasSymbol
    pub(crate) fn check_source_alias_symbol(&mut self, node: NodeId) -> Result<(), Error> {
        let symbol = required(self.get_symbol_of_declaration(node)?, "source alias symbol")?;
        let target = self.resolve_alias(symbol)?;
        if target == self.builtins.unknown_symbol {
            return Ok(());
        }
        let symbol = self.get_merged_symbol(self.symbol(symbol)?.export_symbol().unwrap_or(symbol));
        let local_flags = self.symbol(symbol)?.flags();
        let target_flags = self.module_symbol_flags(target, false, false)?;
        let read = self.ast(node)?.node(node)?;
        let export = read.kind() == K::ExportSpecifier;
        let ambient = read.flags() & nf::AMBIENT != 0;
        let type_only = self.local_type_only_alias_declaration(node)?.is_some();
        if self.check_js_type_alias_import(node, symbol, target, target_flags, type_only)? {
            return Ok(());
        }
        let excluded = if local_flags & (sf::VALUE | sf::EXPORT_VALUE) != 0 {
            sf::VALUE
        } else {
            0
        } | if local_flags & sf::TYPE != 0 {
            sf::TYPE
        } else {
            0
        } | if local_flags & sf::NAMESPACE != 0 {
            sf::NAMESPACE
        } else {
            0
        };
        if target_flags & excluded != 0 {
            let name = self.symbol_to_string(symbol)?;
            self.error_at(
                Some(node),
                if export {
                    d::Export_declaration_conflicts_with_exported_declaration_of_0
                } else {
                    d::Import_declaration_conflicts_with_local_declaration_of_0
                },
                vec![name],
            )?;
        } else if !export
            && self.program()?.host.options().isolated_modules.is_true()
            && !type_only
            && local_flags & (sf::VALUE | sf::EXPORT_VALUE) != 0
        {
            let name = self.symbol_to_string(symbol)?;
            let flag = self.module_isolated_flag_name();
            self.error_at(Some(node),d::Import_0_conflicts_with_local_value_so_must_be_declared_with_a_type_only_import_when_isolatedModules_is_enabled,vec![name,flag])?;
        }
        if self.program()?.host.options().isolated_modules() && !type_only && !ambient {
            self.check_isolated_alias(node, symbol, target, target_flags)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkTypeNameIsReserved
    pub(crate) fn check_module_reserved_type_name(
        &mut self,
        name: NodeId,
        message: &'static d::Message,
    ) -> Result<(), Error> {
        let text = self.ast(name)?.node_text(name)?.into_js_string();
        if matches!(
            text.as_bytes(),
            b"any"
                | b"unknown"
                | b"never"
                | b"number"
                | b"bigint"
                | b"boolean"
                | b"string"
                | b"symbol"
                | b"void"
                | b"object"
                | b"undefined"
        ) {
            self.error_at(Some(name), message, vec![text])?;
        }
        Ok(())
    }
    pub(crate) fn module_isolated_flag_name(&self) -> JsString {
        JsString::from_bytes(
            if self
                .program
                .as_ref()
                .is_some_and(|p| p.host.options().verbatim_module_syntax.is_true())
            {
                &b"verbatimModuleSyntax"[..]
            } else {
                &b"isolatedModules"[..]
            },
        )
    }
    // port: tsc/internal/checker/checker.go:Checker.markExportAsReferenced
    fn mark_module_export_referenced(&mut self, node: NodeId) -> Result<(), Error> {
        if self
            .program()?
            .host
            .options()
            .verbatim_module_syntax
            .is_true()
            || self.ast(node)?.node(node)?.flags() & nf::AMBIENT != 0
        {
            return Ok(());
        }
        let symbol = required(
            self.get_symbol_of_declaration(node)?,
            "referenced export alias",
        )?;
        let target = self.resolve_alias(symbol)?;
        let flags = self.symbol(target)?.flags();
        if target == self.builtins.unknown_symbol
            || self.module_symbol_flags(symbol, true, false)? & sf::VALUE != 0
                && flags & sf::CONST_ENUM == 0
                && flags & sf::CONST_ENUM_ONLY_MODULE == 0
        {
            self.mark_module_alias_referenced(symbol)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.markAliasSymbolAsReferenced
    pub(crate) fn mark_module_alias_referenced(&mut self, symbol: SymbolId) -> Result<(), Error> {
        if !self.module_aliases.referenced.insert(symbol) {
            return Ok(());
        }
        let declaration = self.alias_declaration(symbol)?;
        let read = self.ast(declaration)?.node(declaration)?;
        if read.kind() == K::ImportEqualsDeclaration {
            let reference = required(
                read.data_source()
                    .as_import_equals_declaration()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .module_reference(),
                "referenced import alias target",
            )?;
            if self.ast(reference)?.node(reference)?.kind() != K::ExternalModuleReference {
                let target = self.resolve_alias(symbol)?;
                if self.symbol(target)?.flags() & sf::VALUE != 0 {
                    let first = ts_ast::utilities_middle::get_first_identifier(
                        self.ast(reference)?,
                        reference,
                    )?;
                    // markIdentifierAliasReferenced resolves through getResolvedSymbol,
                    // which reports unresolved names and namespaces used as values.
                    let alias = self.resolved_value_symbol(first)?;
                    if alias != self.builtins.unknown_symbol
                        && alias != self.builtins.arguments_symbol
                        && self.symbol(alias)?.flags() & sf::ALIAS != 0
                        && !self.module_aliases.type_only.contains_key(&alias)
                    {
                        self.mark_module_alias_referenced(alias)?;
                    }
                }
            }
        }
        Ok(())
    }
}
