//! Source namespace and local alias checks. External module resolution and
//! augmentation remain explicit boundaries until their host closure is ported.
use crate::{CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    modifier_flags as mf, node_flags as nf, symbol_flags as sf, JsString, SyntaxKind as K,
};
use ts_diagnostics as d;
fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(name))
}
impl CheckerState {
    pub(crate) fn check_module_block(&mut self, node: NodeId) -> Result<(), Error> {
        for statement in self.source_list(node, self.ast(node)?.node(node)?.statement_list())? {
            self.check_source_element(statement)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarModuleElementContext
    pub(crate) fn check_grammar_module_context(
        &mut self,
        node: NodeId,
        message: &'static d::Message,
    ) -> Result<bool, Error> {
        let parent = required(
            self.ast(node)?.node(node)?.parent(),
            "module element parent",
        )?;
        let invalid = !matches!(
            self.ast(parent)?.node(parent)?.kind().known(),
            Some(K::SourceFile | K::ModuleBlock | K::ModuleDeclaration)
        );
        if invalid {
            self.grammar_error_first_token(node, message, vec![])?;
        }
        Ok(invalid)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkModuleDeclaration
    pub(crate) fn check_module_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let body = read.body();
        let global = ts_ast::utilities::is_global_scope_augmentation(&read);
        let ambient = read.flags() & nf::AMBIENT != 0;
        let name = required(read.name(), "namespace name")?;
        let data = read
            .data_source()
            .as_module_declaration()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let keyword = data.keyword();
        let attributes = data.attributes();
        if let Some(body) = body {
            self.check_source_element(body)?;
            if !global {
                self.register_for_unused_identifiers_check(node)?;
            }
        }
        if global && !ambient {
            self.error_at(Some(name),d::Augmentations_for_the_global_scope_should_have_declare_modifier_unless_they_appear_in_already_ambient_context,vec![])?;
        }
        if let Some(attributes) = attributes {
            self.check_import_attributes_type(attributes)?;
        }
        let external = ts_ast::is_ambient_module(self.ast(node)?, node)?;
        let context = if external {
            d::An_ambient_module_declaration_is_only_allowed_at_the_top_level_in_a_file
        } else {
            d::A_namespace_declaration_is_only_allowed_at_the_top_level_of_a_namespace_or_module
        };
        if self.check_grammar_module_context(node, context)? {
            return Ok(());
        }
        if !self.check_grammar_modifiers(node)?
            && !ambient
            && self.ast(name)?.node(name)?.kind() == K::StringLiteral
        {
            self.grammar_error_node(name, d::Only_ambient_modules_can_use_quoted_names, vec![])?;
        }
        if self.ast(name)?.node(name)?.kind() == K::Identifier {
            self.check_module_name_collision(node, name)?;
            if keyword == K::ModuleKeyword {
                self.error_at(Some(name),d::A_namespace_declaration_should_not_be_declared_using_the_module_keyword_Please_use_the_namespace_keyword_instead,vec![])?;
            }
        }
        self.check_exports_on_merged_declarations(node)?;
        let symbol = required(self.get_symbol_of_declaration(node)?, "namespace symbol")?;
        if self.symbol(symbol)?.flags() & sf::VALUE_MODULE != 0
            && !ambient
            && self.instantiated_module(node)?
        {
            let options = self.program()?.host.options();
            let erasable = options.erasable_syntax_only.is_true()
                && self.ast(node)?.node(node)?.flags() & nf::JAVA_SCRIPT_FILE == 0;
            let isolated = options.isolated_modules();
            let verbatim = options.verbatim_module_syntax.is_true();
            if erasable {
                self.error_at(
                    Some(node),
                    d::This_syntax_is_not_allowed_when_erasableSyntaxOnly_is_enabled,
                    vec![],
                )?;
            }
            let source = required(
                ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?,
                "namespace source",
            )?;
            if isolated
                && self
                    .ast(source)?
                    .source_file(source)?
                    .external_module_indicator
                    .is_none()
            {
                self.error_at(Some(name),d::Namespaces_are_not_allowed_in_global_script_files_when_0_is_enabled_If_this_file_is_not_intended_to_be_a_global_script_set_moduleDetection_to_force_or_add_an_empty_export_statement,vec![JsString::from_bytes(if verbatim{&b"verbatimModuleSyntax"[..]}else{&b"isolatedModules"[..]})])?;
            }
            for declaration in self
                .symbol_declarations(symbol)?
                .to_vec()
                .into_iter()
                .flatten()
            {
                let read = self.ast(declaration)?.node(declaration)?;
                let real_class_or_function = read.kind() == K::ClassDeclaration
                    || read.kind() == K::FunctionDeclaration && read.body().is_some();
                if real_class_or_function && read.flags() & nf::AMBIENT == 0 {
                    let declaration_source = ts_ast::utilities::get_source_file_of_node(
                        self.ast(declaration)?,
                        Some(declaration),
                    )?;
                    if Some(source) != declaration_source {
                        self.error_at(Some(name),d::A_namespace_declaration_cannot_be_in_a_different_file_from_a_class_or_function_with_which_it_is_merged,vec![])?;
                    } else if self.ast(node)?.node(node)?.pos() < read.pos() {
                        self.error_at(Some(name),d::A_namespace_declaration_cannot_be_located_prior_to_a_class_or_function_with_which_it_is_merged,vec![])?;
                    }
                    break;
                }
            }
            if verbatim
                && self
                    .ast(node)?
                    .node(node)?
                    .modifier_flags(self.ast(node)?)?
                    & mf::EXPORT
                    != 0
                && self.module_emit_format(node)? == ts_core::ModuleKind::COMMON_JS
            {
                let modifiers = self.source_list(node, self.ast(node)?.node(node)?.modifiers())?;
                let mut export_modifier = None;
                for modifier in modifiers {
                    if self.ast(modifier)?.node(modifier)?.kind() == K::ExportKeyword {
                        export_modifier = Some(modifier);
                        break;
                    }
                }
                self.error_at(
                    export_modifier,
                    d::A_top_level_export_modifier_cannot_be_used_on_value_declarations_in_a_CommonJS_module_when_verbatimModuleSyntax_is_enabled,
                    vec![],
                )?;
            }
        }
        if external {
            if ts_ast::is_module_augmentation_external(self.ast(node)?, node)? {
                if let Some(attributes) = attributes {
                    self.error_at(
                        Some(attributes),
                        d::Import_attributes_are_not_allowed_on_a_module_augmentation,
                        vec![],
                    )?;
                }
                if global || self.symbol(symbol)?.flags() & sf::TRANSIENT != 0 {
                    if let Some(body) = body {
                        for statement in
                            self.source_list(body, self.ast(body)?.node(body)?.statement_list())?
                        {
                            self.check_module_augmentation_element(statement)?;
                        }
                    }
                }
            } else {
                let parent = required(
                    self.ast(node)?.node(node)?.parent(),
                    "ambient module parent",
                )?;
                if ts_ast::utilities_middle::is_global_source_file(self.ast(parent)?, parent)? {
                    if global {
                        self.error_at(Some(name),d::Augmentations_for_the_global_scope_can_only_be_directly_nested_in_external_modules_or_ambient_module_declarations,vec![])?;
                    } else if ts_module::is_relative(self.ast(name)?.node_text(name)?.as_bytes()) {
                        self.error_at(
                            Some(name),
                            d::Ambient_module_declaration_cannot_specify_relative_module_name,
                            vec![],
                        )?;
                    }
                } else {
                    self.error_at(Some(name),if global{d::Augmentations_for_the_global_scope_can_only_be_directly_nested_in_external_modules_or_ambient_module_declarations}else{d::Ambient_modules_cannot_be_nested_in_other_modules_or_namespaces},vec![])?;
                }
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:isInstantiatedModule
    fn instantiated_module(&self, node: NodeId) -> Result<bool, Error> {
        let state = ts_ast::get_module_instance_state(self.ast(node)?, node)?;
        Ok(state == ts_ast::ModuleInstanceState::Instantiated
            || state == ts_ast::ModuleInstanceState::ConstEnumOnly
                && self.program()?.host.options().should_preserve_const_enums())
    }
    pub(crate) fn check_module_name_collision(
        &mut self,
        node: NodeId,
        _name: NodeId,
    ) -> Result<(), Error> {
        self.check_collisions_for_declaration_name(node)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkExportsOnMergedDeclarations
    pub(crate) fn check_exports_on_merged_declarations(
        &mut self,
        node: NodeId,
    ) -> Result<(), Error> {
        let local = self
            .program()?
            .bound(node)?
            .node_binding(node)?
            .and_then(|binding| binding.local_symbol);
        let symbol = if let Some(local) = local {
            local
        } else {
            let symbol = required(
                self.get_symbol_of_declaration(node)?,
                "merged declaration symbol",
            )?;
            if self.symbol(symbol)?.export_symbol().is_none() {
                return Ok(());
            }
            symbol
        };
        let kind = self.ast(node)?.node(node)?.kind();
        let declarations = self.symbol_declarations(symbol)?.to_vec();
        let mut first = None;
        for declaration in declarations.iter().flatten() {
            if self.ast(*declaration)?.node(*declaration)?.kind() == kind {
                first = Some(*declaration);
                break;
            }
        }
        if first != Some(node) {
            return Ok(());
        }
        let (mut exported, mut local, mut default) = (0, 0, 0);
        for declaration in declarations.iter().flatten() {
            let spaces = self.declaration_spaces(*declaration)?;
            let flags = self.effective_declaration_flags(*declaration, mf::EXPORT | mf::DEFAULT)?;
            if flags & mf::EXPORT != 0 {
                if flags & mf::DEFAULT != 0 {
                    default |= spaces;
                } else {
                    exported |= spaces;
                }
            } else {
                local |= spaces;
            }
        }
        let common = exported & local;
        let common_default = default & (exported | local);
        if common != 0 || common_default != 0 {
            for declaration in declarations.into_iter().flatten() {
                let spaces = self.declaration_spaces(declaration)?;
                let name = self.ast(declaration)?.node(declaration)?.name();
                let message = if spaces & common_default != 0 {
                    Some(d::Merged_declaration_0_cannot_include_a_default_export_declaration_Consider_adding_a_separate_export_default_0_declaration_instead)
                } else if spaces & common != 0 {
                    Some(d::Individual_declarations_in_merged_declaration_0_must_be_all_exported_or_all_local)
                } else {
                    None
                };
                if let Some(message) = message {
                    let text =
                        ts_scanner::declaration_name_to_string(self.ast(declaration)?, name)?;
                    self.error_at(name, message, vec![text])?;
                }
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.getDeclarationSpaces
    fn declaration_spaces(&mut self, node: NodeId) -> Result<u8, Error> {
        let read = self.ast(node)?.node(node)?;
        match read.kind().known() {
            Some(
                K::InterfaceDeclaration
                | K::TypeAliasDeclaration
                | K::JSTypeAliasDeclaration
                | K::JSDocTypedefTag
                | K::JSDocCallbackTag
                | K::MethodSignature
                | K::PropertySignature,
            ) => Ok(1),
            Some(K::ModuleDeclaration) => Ok(
                if ts_ast::is_ambient_module(self.ast(node)?, node)?
                    || ts_ast::get_module_instance_state(self.ast(node)?, node)?
                        != ts_ast::ModuleInstanceState::NonInstantiated
                {
                    2 | 4
                } else {
                    4
                },
            ),
            Some(K::ClassDeclaration | K::EnumDeclaration | K::EnumMember) => Ok(1 | 2),
            Some(K::SourceFile) => Ok(1 | 2 | 4),
            Some(
                K::VariableDeclaration
                | K::BindingElement
                | K::FunctionDeclaration
                | K::ImportSpecifier,
            ) => Ok(2),
            Some(K::ExportAssignment | K::BinaryExpression)
                if !self.export_assignment_declares_alias(node)? =>
            {
                Ok(2)
            }
            // Export assigned entity name expressions act as aliases and should fall through, otherwise they export values.
            Some(
                K::ExportAssignment
                | K::BinaryExpression
                | K::ImportEqualsDeclaration
                | K::NamespaceImport
                | K::ImportClause,
            ) => {
                let symbol = required(
                    self.get_symbol_of_declaration(node)?,
                    "declaration spaces alias",
                )?;
                let target = self.resolve_alias(symbol)?;
                let mut spaces = 0;
                for declaration in self
                    .symbol_declarations(target)?
                    .to_vec()
                    .into_iter()
                    .flatten()
                {
                    spaces |= self.declaration_spaces(declaration)?;
                }
                Ok(spaces)
            }

            _ => Err(Error::Unsupported(
                "getDeclarationSpaces: declaration family",
            )),
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.checkExternalModuleExports
    pub(crate) fn check_external_module_exports(&mut self, node: NodeId) -> Result<(), Error> {
        let symbol = required(
            self.get_symbol_of_declaration(node)?,
            "external module symbol",
        )?;
        if self.module_aliases.exports_checked.contains(&symbol) {
            return Ok(());
        }
        let exports = self.symbol(symbol)?.exports();
        if let Some(export_equals) =
            self.member_symbol(exports, ts_ast::internal_symbol_names::EXPORT_EQUALS)?
        {
            let shadowed = if self.symbol(export_equals)?.flags()
                & (sf::NAMESPACE_MODULE | sf::ALIAS)
                == (sf::NAMESPACE_MODULE | sf::ALIAS)
            {
                let target = self.resolve_alias(export_equals)?;
                self.symbol(target)?.flags() & sf::NAMESPACE != 0
                    && self.module_has_exported_kind(target, sf::TYPE | sf::NAMESPACE)?
            } else {
                false
            };
            if self.module_has_exported_kind(symbol, sf::VALUE)? || shadowed {
                let declaration = self
                    .alias_declaration_or_none(export_equals)?
                    .or(self.symbol(export_equals)?.value_declaration());
                if let Some(declaration) = declaration {
                    if !self.top_level_module_augmentation(declaration)? {
                        self.error_at(Some(declaration),d::An_export_assignment_cannot_be_used_in_a_module_with_other_exported_elements,vec![])?;
                    }
                }
            }
        }
        let exports = self.module_exports(symbol)?;
        let entries = self.module_table_entries(exports)?;
        for (name, symbol) in entries {
            let Some(symbol) = symbol else { continue };
            if name.as_bytes() == ts_ast::internal_symbol_names::EXPORT_STAR {
                continue;
            }
            let flags = self.symbol(symbol)?.flags();
            if flags & (sf::NAMESPACE | sf::ENUM) != 0 {
                continue;
            }
            let declarations = self.symbol_declarations(symbol)?.to_vec();
            let mut count = 0;
            for &declaration in declarations.iter().flatten() {
                let read = self.ast(declaration)?.node(declaration)?;
                if self.module_not_overload(declaration)?
                    && !matches!(
                        read.kind().known(),
                        Some(K::GetAccessor | K::SetAccessor | K::InterfaceDeclaration)
                    )
                {
                    count += 1;
                }
            }
            if flags & sf::TYPE_ALIAS != 0 && count <= 2 {
                continue;
            }
            if count > 1 {
                let mut all_exports_properties = true;
                for &declaration in declarations.iter().flatten() {
                    if ts_ast::get_assignment_declaration_kind(self.ast(declaration)?, declaration)?
                        != ts_ast::JSDeclarationKind::ExportsProperty
                    {
                        all_exports_properties = false;
                        break;
                    }
                }
                if all_exports_properties {
                    continue;
                }
                for declaration in declarations.into_iter().flatten() {
                    if self.module_not_overload(declaration)? {
                        self.error_at(
                            Some(declaration),
                            d::Cannot_redeclare_exported_variable_0,
                            vec![name.clone()],
                        )?;
                    }
                }
            }
        }
        self.module_aliases.exports_checked.insert(symbol);
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.hasExportedMembersOfKind
    fn module_has_exported_kind(&mut self, module: SymbolId, kind: u32) -> Result<bool, Error> {
        let Some(exports) = self.symbol(module)?.exports() else {
            return Ok(false);
        };
        for (_, symbol) in self.module_table_entries(exports)? {
            if let Some(symbol) = symbol {
                if self.symbol(symbol)?.name_bytes() != ts_ast::internal_symbol_names::EXPORT_EQUALS
                    && self.module_symbol_flags(symbol, false, false)? & kind != 0
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    fn module_not_overload(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        Ok(!matches!(
            read.kind().known(),
            Some(K::FunctionDeclaration | K::MethodDeclaration)
        ) || read.body().is_some())
    }
    // port: tsc/internal/checker/checker.go:Checker.resolveAnonymousTypeMembers
    pub(crate) fn resolve_namespace_type_members(
        &mut self,
        ty: TypeId,
        symbol: SymbolId,
    ) -> Result<(), Error> {
        let mut exports = self.module_exports(symbol)?;
        if symbol == self.builtins.global_this_symbol {
            // resolveAnonymousTypeMembers exposes runtime globals, excluding
            // lexical declarations and modules declared only by ambient names.
            let mut members = ts_ast::SymbolTable::new();
            for (_, property) in self.table(exports)? {
                let Some(property) = property else { continue };
                let read = self.symbol(property)?;
                if read.flags() & sf::BLOCK_SCOPED != 0 {
                    continue;
                }
                let declarations = self.symbol_declarations(property)?;
                let mut ambient_only =
                    read.flags() & sf::VALUE_MODULE != 0 && !declarations.is_empty();
                if ambient_only {
                    for declaration in declarations.iter() {
                        if !declaration
                            .map(|node| {
                                ts_ast::is_ambient_module(self.ast(node)?, node)
                                    .map_err(Error::from)
                            })
                            .transpose()?
                            .unwrap_or(false)
                        {
                            ambient_only = false;
                            break;
                        }
                    }
                }
                if !ambient_only {
                    members.insert(read.name_to_owned(), Some(property));
                }
            }
            exports = self.alloc_symbol_table(members);
        }
        self.set_structured_type_members(ty, Some(exports), &[], &[], &[])
    }

    /// The alias half of `getDeclarationSpaces` for export assignments: an
    /// entity-name expression whose symbol is an alias.
    // port: tsc/internal/checker/checker.go:Checker.getDeclarationSpaces
    fn export_assignment_declares_alias(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        let expression = if read.kind() == K::ExportAssignment {
            read.expression()
        } else {
            read.data_source()
                .as_binary_expression()
                .and_then(|data| data.right())
        };
        let Some(expression) = expression else {
            return Ok(false);
        };
        if !ts_ast::is_entity_name_expression(self.ast(expression)?, expression)? {
            return Ok(false);
        }
        let Some(symbol) = self.get_symbol_of_declaration(node)? else {
            return Ok(false);
        };
        Ok(self.symbol(symbol)?.flags() & sf::ALIAS != 0)
    }
}
