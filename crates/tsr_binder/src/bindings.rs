use crate::{
    ast as a, checked, need,
    target::{target_payload, BindingNode},
    Binder,
};
use ts_ast::{
    internal_symbol_names as names, node_flags as nf, symbol_flags as sf, JsString, NodeId,
    SyntaxKind as K,
};
use ts_diagnostics as d;

impl<'scope> Binder<'_, 'scope, '_> {
    // port: tsc/internal/binder/binder.go:Binder.bindPropertyWorker
    pub fn bind_property_worker(&mut self, node: BindingNode<'scope>) {
        let accessor = checked(a::is_auto_accessor_property_declaration(
            self.view(),
            self.node_id(node),
        ));
        self.bind_property_or_method_or_accessor(
            node,
            if accessor { sf::ACCESSOR } else { sf::PROPERTY } | self.optional_symbol_flag(node),
            if accessor {
                sf::ACCESSOR_EXCLUDES
            } else {
                sf::PROPERTY_EXCLUDES
            },
        );
    }
    // port: tsc/internal/binder/binder.go:Binder.bindPropertyOrMethodOrAccessor
    pub fn bind_property_or_method_or_accessor(
        &mut self,
        node: BindingNode<'scope>,
        flags: u32,
        excludes: u32,
    ) {
        self.record_async_function(node);
        if self.current_flow.is_some()
            && checked(a::is_object_literal_or_class_expression_method_or_accessor(
                self.view(),
                self.node_id(node),
            ))
        {
            self.set_flow_node(self.node_id(node), self.current_flow);
        }
        if self.target_has_dynamic_name(Some(node)) {
            self.bind_anonymous_target(node, flags, JsString::from_bytes(names::COMPUTED));
        } else {
            self.declare_target_symbol(node, flags, excludes);
        }
    }
    fn record_async_function(&mut self, node: BindingNode<'scope>) {
        if !checked(self.view().source_file(self.file)).is_declaration_file
            && self.node_flags(node) & nf::AMBIENT == 0
            && checked(a::is_async_function(self.view(), self.node_id(node)))
        {
            self.emit_flags |= nf::HAS_ASYNC_FUNCTIONS;
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindFunctionExpression
    pub fn bind_function_expression(&mut self, node: BindingNode<'scope>) {
        self.record_async_function(node);
        self.set_flow_node(self.node_id(node), self.current_flow);
        let name =
            if self.node_kind(node) == K::FunctionExpression && self.node_name(node).is_some() {
                self.check_strict_mode_function_name(node);
                self.target_text(need(self.node_name(node)))
            } else {
                JsString::from_bytes(names::FUNCTION)
            };
        self.bind_anonymous_target(node, sf::FUNCTION, name);
    }
    // port: tsc/internal/binder/binder.go:Binder.bindClassLikeDeclaration
    pub fn bind_class_like_declaration(&mut self, node: BindingNode<'scope>) {
        if self.node_kind(node) == K::ClassDeclaration {
            self.bind_block_scoped_target(node, sf::CLASS, sf::CLASS_EXCLUDES);
        } else if self.node_kind(node) == K::ClassExpression {
            let name = self.node_name(node).map_or_else(
                || JsString::from_bytes(names::CLASS),
                |name| self.target_text(name),
            );
            self.bind_anonymous_target(node, sf::CLASS, name);
        }
        let symbol = need(self.node_binding_symbol(node));
        let prototype = self.new_binding_symbol(
            sf::PROPERTY | sf::PROTOTYPE,
            JsString::from_bytes(&b"prototype"[..]),
        );
        let table = self.ensure_binding_exports(symbol);
        if let Some(previous) = self.binding_table_get(table, b"prototype").flatten() {
            let declaration = need(
                checked(
                    self.builder
                        .declarations()
                        .get(self.s_binding(previous).declarations()),
                )
                .at(0),
            );
            self.error_on_node(
                declaration,
                d::Duplicate_identifier_0,
                vec![self.s_binding(prototype).name_to_owned()],
            );
        }
        self.binding_table_insert(
            table,
            JsString::from_bytes(&b"prototype"[..]),
            Some(prototype),
        );
        self.set_binding_symbol_parent(prototype, Some(symbol));
    }
    // port: tsc/internal/binder/binder.go:Binder.bindFunctionOrConstructorType
    pub fn bind_function_or_constructor_type(&mut self, node: BindingNode<'scope>) {
        let symbol = self.new_binding_symbol(sf::SIGNATURE, self.declaration_name(node));
        self.add_binding_declaration(symbol, node, sf::SIGNATURE);
        let type_literal =
            self.new_binding_symbol(sf::TYPE_LITERAL, JsString::from_bytes(names::TYPE));
        self.add_binding_declaration(type_literal, node, sf::TYPE_LITERAL);
        let name = self.s_binding(symbol).name_to_owned();
        let table = self.new_binding_table();
        self.binding_table_insert(table, name, Some(symbol));
        self.set_binding_symbol_members(type_literal, Some(table));
    }
    // port: tsc/internal/binder/binder.go:Binder.bindEnumDeclaration
    pub fn bind_enum_declaration(&mut self, node: BindingNode<'scope>) {
        if checked(a::is_enum_const(self.view(), self.node_id(node))) {
            self.bind_block_scoped_target(node, sf::CONST_ENUM, sf::CONST_ENUM_EXCLUDES);
        } else {
            self.bind_block_scoped_target(node, sf::REGULAR_ENUM, sf::REGULAR_ENUM_EXCLUDES);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindVariableDeclarationOrBindingElement
    pub fn bind_variable_declaration_or_binding_element(&mut self, node: BindingNode<'scope>) {
        self.check_strict_mode_eval_or_arguments(node, self.node_name(node));
        if self
            .node_name(node)
            .is_some_and(|name| !a::is_binding_pattern_kind(self.node_kind(name)))
        {
            if checked(a::is_variable_declaration_initialized_to_require(
                self.view(),
                self.node_id(node),
            )) {
                self.declare_target_symbol(node, sf::ALIAS, sf::ALIAS_EXCLUDES);
            } else if checked(a::is_block_or_catch_scoped(self.view(), self.node_id(node))) {
                self.bind_block_scoped_target(
                    node,
                    sf::BLOCK_SCOPED_VARIABLE,
                    sf::BLOCK_SCOPED_VARIABLE_EXCLUDES,
                );
            } else if checked(a::is_part_of_parameter_declaration(
                self.view(),
                self.node_id(node),
            )) {
                self.declare_target_symbol(
                    node,
                    sf::FUNCTION_SCOPED_VARIABLE,
                    sf::PARAMETER_EXCLUDES,
                );
            } else {
                self.declare_target_symbol(
                    node,
                    sf::FUNCTION_SCOPED_VARIABLE,
                    sf::FUNCTION_SCOPED_VARIABLE_EXCLUDES,
                );
            }
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindParameter
    pub fn bind_parameter(&mut self, node: BindingNode<'scope>) {
        if self.node_flags(node) & nf::AMBIENT == 0 {
            self.check_strict_mode_eval_or_arguments(node, self.node_name(node));
        }
        let parent = need(self.node_parent(node));
        if a::is_binding_pattern_kind(self.node_kind(need(self.node_name(node)))) {
            let parameters = self.target_list_edges(self.node_parameter_list(parent));
            let index = (0..parameters.len())
                .position(|index| self.same_node(self.edge(parameters, index), Some(node)))
                .map_or(-1, |index| index as isize);
            self.bind_anonymous_target(
                node,
                sf::FUNCTION_SCOPED_VARIABLE,
                JsString::from_bytes(format!("__{index}").into_bytes()),
            );
        } else {
            self.declare_target_symbol(node, sf::FUNCTION_SCOPED_VARIABLE, sf::PARAMETER_EXCLUDES);
        }
        if checked(a::is_parameter_property_declaration(
            self.view(),
            self.node_id(node),
            self.node_id(parent),
        )) {
            let class = need(self.node_parent(parent));
            let symbol = need(self.node_binding_symbol(class));
            let flags = sf::PROPERTY
                | if target_payload!(self, node, as_parameter_declaration, "parameter payload"; node: question_token)
                    .0
                    .is_some()
                {
                    sf::OPTIONAL
                } else {
                    0
                };
            let table = self.ensure_binding_members(symbol);
            self.declare_binding_symbol(table, Some(symbol), node, flags, sf::PROPERTY_EXCLUDES);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindFunctionDeclaration
    pub fn bind_function_declaration(&mut self, node: BindingNode<'scope>) {
        self.record_async_function(node);
        self.check_strict_mode_function_name(node);
        self.bind_block_scoped_target(node, sf::FUNCTION, sf::FUNCTION_EXCLUDES);
    }
    // port: tsc/internal/binder/binder.go:Binder.getInferTypeContainer
    pub fn get_infer_type_container(
        &self,
        node: BindingNode<'scope>,
    ) -> Option<BindingNode<'scope>> {
        let mut current = Some(node);
        while let Some(node) = current {
            let parent = self.node_parent(node);
            if parent.is_some_and(|parent| {
                self.node_kind(parent) == K::ConditionalType
                    && self.same_node(
                        target_payload!(self, parent, as_conditional_type_node, "conditional type payload"; node: extends_type)
                            .0,
                        Some(node),
                    )
            }) {
                return parent;
            }
            current = parent;
        }
        None
    }
    // port: tsc/internal/binder/binder.go:Binder.bindAnonymousDeclaration
    pub fn bind_anonymous_target(&mut self, node: BindingNode<'scope>, flags: u32, name: JsString) {
        let symbol = self.new_binding_symbol(flags, name);
        if flags & (sf::ENUM_MEMBER | sf::CLASS_MEMBER) != 0 {
            self.set_binding_symbol_parent(
                symbol,
                self.node_binding_symbol(self.binding_node(need(self.container))),
            );
        }
        self.add_binding_declaration(symbol, node, flags);
    }
    // port: tsc/internal/binder/binder.go:Binder.bindBlockScopedDeclaration
    pub fn bind_block_scoped_target(
        &mut self,
        node: BindingNode<'scope>,
        flags: u32,
        excludes: u32,
    ) {
        let block = self.binding_node(need(self.block_scope_container));
        if self.node_kind(block) == K::ModuleDeclaration
            || self.node_kind(block) == K::SourceFile
                && a::is_external_or_common_js_module(&checked(
                    self.view().source_file(need(self.container)),
                ))
        {
            self.declare_binding_module_member(node, flags, excludes);
        } else {
            let table = self.ensure_binding_locals(block);
            self.declare_binding_symbol(table, None, node, flags, excludes);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindTypeParameter
    pub fn bind_type_parameter(&mut self, node: BindingNode<'scope>) {
        let parent = need(self.node_parent(node));
        if self.node_kind(parent) == K::InferType {
            if let Some(container) = self.get_infer_type_container(parent) {
                let table = self.ensure_binding_locals(container);
                self.declare_binding_symbol(
                    table,
                    None,
                    node,
                    sf::TYPE_PARAMETER,
                    sf::TYPE_PARAMETER_EXCLUDES,
                );
            } else {
                self.bind_anonymous_target(node, sf::TYPE_PARAMETER, self.declaration_name(node));
            }
        } else {
            self.declare_target_symbol(node, sf::TYPE_PARAMETER, sf::TYPE_PARAMETER_EXCLUDES);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindJsxAttributes
    pub fn bind_jsx_attributes(&mut self, node: BindingNode<'scope>) {
        self.bind_anonymous_target(
            node,
            sf::OBJECT_LITERAL,
            JsString::from_bytes(names::JSX_ATTRIBUTES),
        );
    }
    // port: tsc/internal/binder/binder.go:Binder.bindJsxAttribute
    pub fn bind_jsx_attribute(&mut self, node: BindingNode<'scope>, flags: u32, excludes: u32) {
        self.declare_target_symbol(node, flags, excludes);
    }
    // port: tsc/internal/binder/binder.go:getOptionalSymbolFlagForNode
    pub fn optional_symbol_flag(&self, node: BindingNode<'scope>) -> u32 {
        if self
            .node_postfix_token(node)
            .is_some_and(|token| self.node_kind(token) == K::QuestionToken)
        {
            sf::OPTIONAL
        } else {
            sf::NONE
        }
    }
}

impl Binder<'_, '_, '_> {
    pub fn bind_anonymous_declaration(&mut self, node: NodeId, flags: u32, name: JsString) {
        self.bind_anonymous_target(self.binding_node(node), flags, name);
    }
    pub fn bind_block_scoped_declaration(&mut self, node: NodeId, flags: u32, excludes: u32) {
        self.bind_block_scoped_target(self.binding_node(node), flags, excludes);
    }
}
