use crate::{ast as a, checked, need, Binder};
use tsr_ast::{
    internal_symbol_names as names, modifier_flags as mf, node_flags as nf, symbol_flags as sf,
    JsString, NodeId, SymbolId, SymbolTable, SyntaxKind as K,
};
use tsr_diagnostics as d;

impl Binder<'_, '_, '_> {
    // port: tsc/internal/binder/binder.go:Binder.bindSourceFileIfExternalModule
    pub fn bind_source_file_if_external_module(&mut self) {
        self.set_export_context_flag(self.file);
        let file = checked(self.view().source_file(self.file));
        if a::is_external_or_common_js_module(&file) {
            self.bind_source_file_as_external_module();
        } else if a::is_json_source_file(&file) {
            self.bind_source_file_as_external_module();
            let original = need(self.symbol(self.file));
            let exports = self.ensure_exports(original);
            self.declare_symbol(exports, Some(original), self.file, sf::PROPERTY, sf::ALL);
            self.set_node_symbol(self.file, Some(original));
        }
    }
    /// `bindSourceFileAsExternalModule`; the marker is on its statement site.
    pub fn bind_source_file_as_external_module(&mut self) {
        let file = checked(self.view().source_file(self.file));
        let stem = tsr_core::path::remove_file_extension(file.file_name());
        // `"\"" + RemoveFileExtension(fileName) + "\""`. The closing quote is
        // the skip-statement site: a mutant changes the module symbol's name
        // instead of dropping the declaration, which only crashes downstream.
        let mut name = Vec::with_capacity(stem.len() + 2);
        name.push(b'"');
        name.extend_from_slice(stem);
        // port: tsc/internal/binder/binder.go:Binder.bindSourceFileAsExternalModule
        name.push(b'"');
        self.bind_anonymous_declaration(self.file, sf::VALUE_MODULE, JsString::from_bytes(name));
    }
    // port: tsc/internal/binder/binder.go:Binder.bindModuleDeclaration
    pub fn bind_module_declaration(&mut self, node: NodeId) {
        self.set_export_context_flag(node);
        if self.target_is_ambient_module(self.binding_node(node)) {
            if self.target_has_syntactic_modifier(self.binding_node(node), mf::EXPORT) {
                self.error_on_first_token(node, d::X_export_modifier_cannot_be_applied_to_ambient_modules_and_module_augmentations_since_they_are_always_visible, Vec::new());
            }
            if checked(a::is_module_augmentation_external(self.view(), node)) {
                self.declare_module_symbol(node);
            } else {
                let name = need(self.n(node).name());
                let symbol = self.declare_symbol_and_add_to_symbol_table(
                    node,
                    sf::VALUE_MODULE,
                    sf::VALUE_MODULE_EXCLUDES,
                );
                if self.n(name).kind() == K::StringLiteral {
                    let pattern = tsr_core::pattern::Pattern::parse(self.text(name).as_bytes());
                    if !pattern.is_valid() {
                        self.error_on_first_token(
                            name,
                            d::Pattern_0_can_have_at_most_one_Asterisk_character,
                            vec![self.text(name)],
                        );
                    } else if pattern.star_index >= 0 {
                        self.builder
                            .pattern_ambient_modules_mut()
                            .push(a::PatternAmbientModule {
                                pattern,
                                symbol: Some(symbol),
                            });
                    } else if self
                        .n(node)
                        .data_source()
                        .as_module_declaration()
                        .expect("module payload")
                        .attributes()
                        .is_some()
                    {
                        self.error_on_node(name, d::An_ambient_module_declaration_with_import_attributes_must_use_a_pattern_name_with_an_Asterisk_character, Vec::new());
                    }
                }
            }
        } else {
            let state = self.declare_module_symbol(node);
            if state != a::ModuleInstanceState::NonInstantiated {
                let symbol = need(self.symbol(node));
                let constant_only =
                    self.s(symbol).flags() & (sf::FUNCTION | sf::CLASS | sf::REGULAR_ENUM) == 0
                        && state == a::ModuleInstanceState::ConstEnumOnly
                        && !self.not_const_enum_only_modules.contains(&symbol);
                if constant_only {
                    *self.symbol_flags_mut(symbol) |= sf::CONST_ENUM_ONLY_MODULE;
                } else {
                    *self.symbol_flags_mut(symbol) &= !sf::CONST_ENUM_ONLY_MODULE;
                    self.not_const_enum_only_modules.insert(symbol);
                }
            }
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.declareModuleSymbol
    pub fn declare_module_symbol(&mut self, node: NodeId) -> a::ModuleInstanceState {
        let state = checked(a::get_module_instance_state(self.view(), node));
        let instantiated = state != a::ModuleInstanceState::NonInstantiated;
        self.declare_symbol_and_add_to_symbol_table(
            node,
            if instantiated {
                sf::VALUE_MODULE
            } else {
                sf::NAMESPACE_MODULE
            },
            if instantiated {
                sf::VALUE_MODULE_EXCLUDES
            } else {
                sf::NAMESPACE_MODULE_EXCLUDES
            },
        );
        state
    }
    // port: tsc/internal/binder/binder.go:Binder.bindNamespaceExportDeclaration
    pub fn bind_namespace_export_declaration(&mut self, node: NodeId) {
        if self.n(node).modifiers().is_some() {
            self.error_on_node(node, d::Modifiers_cannot_appear_here, Vec::new());
        }
        let parent = need(self.n(node).parent());
        if self.n(parent).kind() != K::SourceFile {
            self.error_on_node(
                node,
                d::Global_module_exports_may_only_appear_at_top_level,
                Vec::new(),
            );
        } else if checked(self.view().source_file(parent))
            .external_module_indicator
            .is_none()
        {
            self.error_on_node(
                node,
                d::Global_module_exports_may_only_appear_in_module_files,
                Vec::new(),
            );
        } else if !checked(self.view().source_file(parent)).is_declaration_file {
            self.error_on_node(
                node,
                d::Global_module_exports_may_only_appear_in_declaration_files,
                Vec::new(),
            );
        } else {
            let table = if let Some(table) = self.builder.result().global_exports() {
                table
            } else {
                let table = self.builder.alloc_table(SymbolTable::default());
                self.builder.set_global_exports(Some(table));
                table
            };
            self.declare_symbol(
                table,
                self.symbol(self.file),
                node,
                sf::ALIAS,
                sf::ALIAS_EXCLUDES,
            );
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindImportClause
    pub fn bind_import_clause(&mut self, node: NodeId) {
        if self.n(node).name().is_some() {
            self.declare_symbol_and_add_to_symbol_table(node, sf::ALIAS, sf::ALIAS_EXCLUDES);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindExportDeclaration
    pub fn bind_export_declaration(&mut self, node: NodeId) {
        let clause = self
            .n(node)
            .data_source()
            .as_export_declaration()
            .expect("export declaration payload")
            .export_clause();
        if let Some(parent) = self.symbol(need(self.container)) {
            if clause.is_none() {
                let table = self.ensure_exports(parent);
                self.declare_symbol(table, Some(parent), node, sf::EXPORT_STAR, sf::NONE);
            } else if self.n(need(clause)).kind() == K::NamespaceExport {
                let table = self.ensure_exports(parent);
                self.declare_symbol(
                    table,
                    Some(parent),
                    need(clause),
                    sf::ALIAS,
                    sf::ALIAS_EXCLUDES,
                );
            }
        } else {
            self.bind_anonymous_declaration(node, sf::EXPORT_STAR, self.get_declaration_name(node));
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindExportAssignment
    pub fn bind_export_assignment(&mut self, node: NodeId) {
        if self.symbol(need(self.container)).is_none() && self.n(node).kind() == K::ExportAssignment
        {
            self.bind_anonymous_declaration(node, sf::VALUE, self.get_declaration_name(node));
        } else {
            let parent = need(self.symbol(need(self.container)));
            let flags = if checked(a::expression_is_alias(
                self.view(),
                need(self.n(node).expression()),
            )) {
                sf::ALIAS
            } else {
                sf::PROPERTY
            };
            let table = self.ensure_exports(parent);
            let symbol = self.declare_symbol(table, Some(parent), node, flags, sf::ALL);
            if self
                .n(node)
                .data_source()
                .as_export_assignment()
                .expect("export assignment payload")
                .is_export_equals()
            {
                self.set_value_declaration(symbol, node);
            }
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.setExportContextFlag
    pub fn set_export_context_flag(&mut self, node: NodeId) {
        let flags = self.n(node).flags();
        let flags = if flags & nf::AMBIENT != 0 && !self.has_export_declarations(node) {
            flags | nf::EXPORT_CONTEXT
        } else {
            flags & !nf::EXPORT_CONTEXT
        };
        self.set_flags(node, flags);
    }
    // port: tsc/internal/binder/binder.go:Binder.hasExportDeclarations
    pub fn has_export_declarations(&self, node: NodeId) -> bool {
        let body = if self.n(node).kind() == K::SourceFile {
            Some(node)
        } else if self.n(node).kind() == K::ModuleDeclaration {
            self.n(node)
                .body()
                .filter(|&body| self.n(body).kind() == K::ModuleBlock)
        } else {
            None
        };
        body.is_some_and(|body| {
            checked(
                self.view()
                    .node_slice(checked(self.n(body).statements(self.view()))),
            )
            .iter()
            .any(|node| {
                matches!(
                    self.n(need(node)).kind().known(),
                    Some(K::ExportDeclaration | K::ExportAssignment)
                )
            })
        })
    }
    // port: tsc/internal/binder/binder.go:Binder.bindCallExpression
    pub fn bind_call_expression(&mut self, node: NodeId) {
        if checked(self.view().source_file(self.file))
            .common_js_module_indicator()
            .is_none()
            && checked(a::is_require_call(self.view(), &self.n(node), false))
        {
            self.set_common_js_module_indicator(node);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.setCommonJSModuleIndicator
    pub fn set_common_js_module_indicator(&mut self, node: NodeId) -> bool {
        let external = checked(self.view().source_file(self.file)).external_module_indicator;
        if external.is_some() && external != Some(self.file) {
            return false;
        }
        if checked(self.view().source_file(self.file))
            .common_js_module_indicator()
            .is_none()
        {
            self.builder.set_common_js_module_indicator(Some(node));
            if external.is_none() {
                self.bind_source_file_as_external_module();
            }
        }
        true
    }
    // port: tsc/internal/binder/binder.go:Binder.bindCommonJSTypeExports
    pub fn bind_common_js_type_exports(&mut self, module: SymbolId) {
        let Some(exports) = self.s(module).exports() else {
            return;
        };
        let Some(export_equals) = self.table(exports).get(names::EXPORT_EQUALS).flatten() else {
            return;
        };
        let values: Vec<_> = self.table(exports).iter().map(|(_, value)| value).collect();
        for symbol in values {
            let symbol = need(symbol);
            if self.s(symbol).name_bytes() != names::EXPORT_EQUALS
                && self.s(symbol).flags() & (sf::TYPE | sf::NAMESPACE) != 0
            {
                let name = self.s(symbol).name_to_owned();
                let target = self.ensure_exports(export_equals);
                self.table_mut(target).insert(name, Some(symbol));
                *self.symbol_flags_mut(export_equals) |= sf::NAMESPACE_MODULE;
            }
        }
    }
}
