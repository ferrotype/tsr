use crate::state::ExpandoAssignmentInfo;
use crate::{ast as a, checked, need, Binder};
use tsr_ast::{
    internal_symbol_names as names, node_flags as nf, symbol_flags as sf, JsString, NodeId,
    SymbolId, SymbolTable, SymbolTableId, SyntaxKind as K,
};

impl Binder<'_, '_, '_> {
    // port: tsc/internal/binder/binder.go:Binder.addLateBoundAssignmentDeclarationToSymbol
    pub fn add_late_bound_assignment_declaration_to_symbol(
        &mut self,
        node: NodeId,
        symbol: SymbolId,
    ) {
        let exports = self.ensure_exports(symbol);
        let assignment = self
            .table(exports)
            .get(names::ASSIGNMENT_DECLARATION)
            .flatten()
            .unwrap_or_else(|| {
                let symbol = self.new_symbol(
                    sf::NONE,
                    JsString::from_bytes(names::ASSIGNMENT_DECLARATION),
                );
                self.table_mut(exports).insert(
                    JsString::from_bytes(names::ASSIGNMENT_DECLARATION),
                    Some(symbol),
                );
                symbol
            });
        let declarations = self.s(assignment).declarations();
        let declarations = checked(
            self.builder
                .declarations_mut()
                .append(declarations, Some(node)),
        );
        self.set_symbol_declarations(assignment, declarations);
    }
    // port: tsc/internal/binder/binder.go:Binder.bindModuleExportsAssignment
    pub fn bind_module_exports_assignment(&mut self, node: NodeId) {
        if self.set_common_js_module_indicator(node) {
            let container = need(self.symbol(self.file));
            let right = need(
                self.n(node)
                    .data_source()
                    .as_binary_expression()
                    .expect("binary payload")
                    .right(),
            );
            let flags = if checked(a::expression_is_alias(self.view(), right)) {
                sf::ALIAS
            } else {
                sf::PROPERTY
            };
            let table = self.ensure_exports(container);
            let symbol = self.declare_symbol(table, Some(container), node, flags, 0);
            self.set_value_declaration(symbol, node);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindExpandoPropertyAssignment
    pub fn bind_expando_property_assignment(&mut self, node: NodeId) {
        self.expando_assignments.push(ExpandoAssignmentInfo {
            node,
            container: self.container,
            block_scope_container: self.block_scope_container,
        });
    }
    // port: tsc/internal/binder/binder.go:Binder.bindDeferredExpandoAssignments
    pub fn bind_deferred_expando_assignments(&mut self) {
        for index in 0..self.expando_assignments.len() {
            let info = self.expando_assignments[index];
            self.container = info.container;
            self.block_scope_container = info.block_scope_container;
            self.bind_deferred_expando_assignment(info.node);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindDeferredExpandoAssignment
    pub fn bind_deferred_expando_assignment(&mut self, node: NodeId) {
        let parent = self.get_parent_of_property_assignment(node);
        let symbol = self
            .lookup_entity(parent, self.block_scope_container)
            .or_else(|| self.lookup_entity(parent, self.container));
        if let Some(symbol) = self.get_initializer_symbol(symbol) {
            if checked(a::has_dynamic_name(self.view(), Some(node))) {
                self.bind_anonymous_declaration(
                    node,
                    sf::PROPERTY | sf::ASSIGNMENT,
                    JsString::from_bytes(names::COMPUTED),
                );
                self.add_late_bound_assignment_declaration_to_symbol(node, symbol);
            } else {
                let table = self.ensure_exports(symbol);
                let name = self.get_declaration_name(node);
                if self
                    .table(table)
                    .get(name.as_bytes())
                    .flatten()
                    .is_none_or(|existing| self.s(existing).flags() & sf::ASSIGNMENT != 0)
                {
                    self.declare_symbol(
                        table,
                        Some(symbol),
                        node,
                        sf::PROPERTY | sf::ASSIGNMENT,
                        sf::PROPERTY_EXCLUDES,
                    );
                }
            }
        }
    }
    // port: tsc/internal/binder/binder.go:getParentOfPropertyAssignment
    pub fn get_parent_of_property_assignment(&self, node: NodeId) -> NodeId {
        match self.n(node).kind().known() {
            Some(K::BinaryExpression) => need(
                self.n(need(
                    self.n(node)
                        .data_source()
                        .as_binary_expression()
                        .expect("binary payload")
                        .left(),
                ))
                .expression(),
            ),
            Some(K::CallExpression) => need(
                checked(
                    self.view()
                        .node_slice(checked(self.n(node).arguments(self.view()))),
                )
                .at(0),
            ),
            _ => panic!("Unhandled case in getParentOfPropertyAssignment"),
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindExportsOrObjectDefineProperty
    pub fn bind_exports_or_object_define_property(&mut self, node: NodeId) {
        if self.set_common_js_module_indicator(node) {
            let container = need(self.symbol(self.file));
            let flags = if self.n(node).kind() == K::BinaryExpression
                && checked(a::expression_is_alias(
                    self.view(),
                    need(
                        self.n(node)
                            .data_source()
                            .as_binary_expression()
                            .expect("binary payload")
                            .right(),
                    ),
                )) {
                sf::ALIAS
            } else {
                sf::FUNCTION_SCOPED_VARIABLE
            };
            let table = self.ensure_exports(container);
            self.declare_symbol(
                table,
                Some(container),
                node,
                flags,
                sf::FUNCTION_SCOPED_VARIABLE_EXCLUDES,
            );
        }
    }
    // port: tsc/internal/binder/binder.go:getInitializerSymbol
    pub fn get_initializer_symbol(&self, symbol: Option<SymbolId>) -> Option<SymbolId> {
        let symbol = symbol?;
        let declaration = self.s(symbol).value_declaration()?;
        let kind = self.n(declaration).kind();
        if kind == K::FunctionDeclaration
            || a::is_in_js_file(Some(&self.n(declaration))) && kind == K::ClassDeclaration
        {
            return Some(symbol);
        }
        let initializer = if kind == K::VariableDeclaration
            && (self.n(need(self.n(declaration).parent())).flags() & nf::CONST != 0
                || a::is_in_js_file(Some(&self.n(declaration))))
        {
            self.n(declaration).initializer()
        } else if kind == K::BinaryExpression && a::is_in_js_file(Some(&self.n(declaration))) {
            self.n(declaration)
                .data_source()
                .as_binary_expression()
                .expect("binary payload")
                .right()
        } else {
            return None;
        };
        if checked(a::is_expando_initializer(
            self.view(),
            declaration,
            initializer,
        )) {
            self.symbol(need(initializer))
        } else {
            None
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindThisPropertyAssignment
    pub fn bind_this_property_assignment(&mut self, node: NodeId) {
        if !a::is_in_js_file(Some(&self.n(node))) {
            return;
        }
        let left = need(
            self.n(node)
                .data_source()
                .as_binary_expression()
                .expect("binary payload")
                .left(),
        );
        if self.n(left).kind() == K::PropertyAccessExpression
            && self.n(need(self.n(left).name())).kind() == K::PrivateIdentifier
            || self.this_container.is_none()
        {
            return;
        }
        let (class, table) = self.get_this_class_and_symbol_table();
        if let Some(table) = table {
            if checked(a::has_dynamic_name(self.view(), Some(node))) {
                self.declare_symbol_ex(table, class, node, sf::PROPERTY, sf::NONE, true, true);
                self.add_late_bound_assignment_declaration_to_symbol(node, need(class));
            } else {
                self.declare_symbol_ex(
                    table,
                    class,
                    node,
                    sf::PROPERTY | sf::ASSIGNMENT,
                    sf::NONE,
                    true,
                    false,
                );
            }
        } else if !matches!(
            self.n(need(self.this_container)).kind().known(),
            Some(K::FunctionDeclaration | K::FunctionExpression)
        ) {
            panic!(
                "Unhandled case in bindThisPropertyAssignment: {}",
                self.n(need(self.this_container)).kind()
            );
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.getThisClassAndSymbolTable
    pub fn get_this_class_and_symbol_table(&mut self) -> (Option<SymbolId>, Option<SymbolTableId>) {
        let Some(container) = self.this_container else {
            return (None, None);
        };
        match self.n(container).kind().known() {
            Some(
                K::Constructor
                | K::PropertyDeclaration
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor
                | K::ClassStaticBlockDeclaration,
            ) => {
                let class = need(self.symbol(need(self.n(container).parent())));
                let table = if checked(a::is_static(self.view(), container)) {
                    self.ensure_exports(class)
                } else {
                    self.ensure_members(class)
                };
                (Some(class), Some(table))
            }
            _ => (None, None),
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.lookupEntity
    pub fn lookup_entity(&mut self, node: NodeId, container: Option<NodeId>) -> Option<SymbolId> {
        if self.n(node).kind() == K::Identifier {
            return self.lookup_name(self.text(node).as_bytes(), need(container));
        }
        let expression = need(self.n(node).expression());
        if self.n(expression).kind() == K::ThisKeyword {
            if let (_, Some(table)) = self.get_this_class_and_symbol_table() {
                if let Some(name) =
                    checked(a::get_element_or_property_access_name(self.view(), node))
                {
                    return self.table(table).get(self.text(name).as_bytes()).flatten();
                }
            }
            return None;
        }
        let symbol = self.lookup_entity(expression, container);
        if let Some(symbol) = self.get_initializer_symbol(symbol) {
            if let Some(exports) = self.s(symbol).exports() {
                if let Some(name) =
                    checked(a::get_element_or_property_access_name(self.view(), node))
                {
                    return self
                        .table(exports)
                        .get(self.text(name).as_bytes())
                        .flatten();
                }
            }
        }
        None
    }
    // port: tsc/internal/binder/binder.go:Binder.lookupName
    pub fn lookup_name(&self, name: &[u8], container: NodeId) -> Option<SymbolId> {
        if let Some(table) = self.locals(container) {
            if let Some(local) = self.table(table).get(name).flatten() {
                return self.s(local).export_symbol().or(Some(local));
            }
        }
        self.symbol(container)
            .and_then(|symbol| self.s(symbol).exports())
            .and_then(|table| self.table(table).get(name).flatten())
    }
    // port: tsc/internal/binder/binder.go:Binder.declareCommonJSVariable
    pub fn declare_common_js_variable(&mut self, name: JsString) {
        let locals = self.ensure_locals(self.file);
        if self.table(locals).get(name.as_bytes()).flatten().is_some() {
            return;
        }
        let symbol = self.new_symbol(
            sf::FUNCTION_SCOPED_VARIABLE | sf::MODULE_EXPORTS,
            name.clone(),
        );
        let declarations = self.new_single_declaration(Some(self.file));
        let source = self.file;
        self.set_symbol_declarations(symbol, declarations);
        self.set_symbol_value_declaration(symbol, Some(source));
        if name.as_bytes() == b"module" {
            let exports = self.new_symbol(
                sf::MODULE_EXPORTS | sf::PROPERTY,
                JsString::from_bytes(&b"exports"[..]),
            );
            self.set_symbol_declarations(exports, declarations);
            self.set_symbol_value_declaration(exports, Some(source));
            self.set_symbol_parent(exports, Some(symbol));
            let table = self.builder.alloc_table(SymbolTable::from_iter([(
                JsString::from_bytes(&b"exports"[..]),
                Some(exports),
            )]));
            self.set_symbol_members(symbol, Some(table));
        }
        self.table_mut(locals).insert(name, Some(symbol));
    }
}
