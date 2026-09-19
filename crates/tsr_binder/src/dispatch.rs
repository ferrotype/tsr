use crate::target::BindingNode;
use crate::{ast as a, checked, Binder};
use ts_ast::{
    internal_symbol_names as names, node_flags as nf, symbol_flags as sf, JsString, NodeId,
    SyntaxKind as K,
};

impl<'scope> Binder<'_, 'scope, '_> {
    // port: tsc/internal/binder/binder.go:Binder.bind
    pub fn bind(&mut self, node: Option<NodeId>) -> bool {
        let Some(node) = node else {
            return false;
        };
        self.bind_target(self.binding_node(node))
    }
    pub(crate) fn bind_target(&mut self, node: BindingNode<'scope>) -> bool {
        match node {
            BindingNode::Local(node) => self.bind_local_entry(node),
            BindingNode::Checked(_) => crate::recursion::guarded(|| self.bind_worker_target(node)),
        }
    }
    pub(crate) fn bind_worker_target(&mut self, node: BindingNode<'scope>) -> bool {
        let kind = self.bind_target_head(node);
        // Head binding may have changed the flags; read them after that phase.
        let mut has_error = self.node_flags(node) & nf::THIS_NODE_HAS_ERROR != 0;
        if kind.raw() > K::LastToken as i16 {
            let saved = self.seen_parse_error;
            self.seen_parse_error = false;
            let flags = self.container_flags(node);
            if flags.0 == 0 {
                self.bind_children_target(node);
            } else {
                self.bind_container_target(node, flags);
            }
            has_error |= self.seen_parse_error;
            self.seen_parse_error = saved;
        }
        self.bind_target_error(node, has_error);
        false
    }
    pub(crate) fn bind_target_error(&mut self, node: BindingNode<'scope>, has_error: bool) {
        if has_error {
            self.set_binding_flags(
                node,
                self.node_flags(node) | nf::THIS_NODE_OR_ANY_SUB_NODES_HAS_ERROR,
            );
            self.seen_parse_error = true;
        }
    }
    // Shared entry phase for ordinary binding and binary continuations.
    pub(crate) fn bind_target_head(&mut self, node: BindingNode<'scope>) -> ts_ast::NodeKind {
        let kind = self.node_kind(node);
        self.bind_node_head_known(node, kind);
        kind
    }
    fn bind_node_head_known(&mut self, target: BindingNode<'scope>, kind: ts_ast::NodeKind) {
        let node = self.node_id(target);
        match kind.known() {
            Some(K::Identifier) => {
                self.set_target_flow(target, self.current_flow);
                self.check_contextual_identifier(node);
            }
            Some(K::ThisKeyword | K::SuperKeyword) => {
                if kind == K::ThisKeyword {
                    self.seen_this_keyword = true;
                }
                self.set_target_flow(target, self.current_flow);
            }
            Some(K::QualifiedName) => {
                if self.current_flow.is_some()
                    && checked(a::is_part_of_type_query(self.view(), node))
                {
                    self.set_target_flow(target, self.current_flow);
                }
            }
            Some(K::MetaProperty) => self.set_target_flow(target, self.current_flow),
            Some(K::PrivateIdentifier) => self.check_private_identifier(node),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                if self.current_flow.is_some() && self.is_narrowable_reference(target) {
                    self.set_target_flow(target, self.current_flow);
                }
            }
            Some(K::BinaryExpression) => {
                match checked(a::get_assignment_declaration_kind(self.view(), node)) {
                    a::JSDeclarationKind::ModuleExports => {
                        self.bind_module_exports_assignment(node);
                    }
                    a::JSDeclarationKind::ExportsProperty => {
                        self.bind_exports_or_object_define_property(node);
                    }
                    a::JSDeclarationKind::Property => self.bind_expando_property_assignment(node),
                    a::JSDeclarationKind::ThisProperty => self.bind_this_property_assignment(node),
                    _ => {}
                }
                self.check_strict_mode_binary_expression(target);
            }
            Some(K::CatchClause) => self.check_strict_mode_catch_clause(target),
            Some(K::DeleteExpression) => self.check_strict_mode_delete_expression(target),
            Some(K::PostfixUnaryExpression) => {
                self.check_strict_mode_postfix_unary_expression(target);
            }
            Some(K::PrefixUnaryExpression) => {
                self.check_strict_mode_prefix_unary_expression(target);
            }
            Some(K::WithStatement) => self.check_strict_mode_with_statement(target),
            Some(K::LabeledStatement) => self.check_strict_mode_labeled_statement(target),
            Some(K::ThisType) => self.seen_this_keyword = true,
            Some(K::TypeParameter) => self.bind_type_parameter(target),
            Some(K::Parameter) => self.bind_parameter(target),
            Some(K::VariableDeclaration) => {
                self.bind_variable_declaration_or_binding_element(target);
            }
            Some(K::BindingElement) => {
                self.set_target_flow(target, self.current_flow);
                self.bind_variable_declaration_or_binding_element(target);
            }
            Some(K::PropertyDeclaration | K::PropertySignature) => {
                self.bind_property_worker(target);
            }
            Some(K::PropertyAssignment | K::ShorthandPropertyAssignment) => {
                self.bind_property_or_method_or_accessor(
                    target,
                    sf::PROPERTY,
                    sf::PROPERTY_EXCLUDES,
                );
            }
            Some(K::EnumMember) => self.bind_property_or_method_or_accessor(
                target,
                sf::ENUM_MEMBER,
                sf::ENUM_MEMBER_EXCLUDES,
            ),
            Some(K::CallSignature | K::ConstructSignature | K::IndexSignature) => {
                self.declare_target_symbol(target, sf::SIGNATURE, sf::NONE);
            }
            Some(K::MethodDeclaration | K::MethodSignature) => {
                let excludes = if checked(a::is_object_literal_method(self.view(), Some(node))) {
                    sf::VALUE
                } else {
                    sf::METHOD_EXCLUDES
                };
                self.bind_property_or_method_or_accessor(
                    target,
                    sf::METHOD | self.optional_symbol_flag(target),
                    excludes,
                );
            }
            Some(K::FunctionDeclaration) => self.bind_function_declaration(target),
            Some(K::Constructor) => {
                self.declare_target_symbol(target, sf::CONSTRUCTOR, sf::NONE);
            }
            Some(K::GetAccessor) => self.bind_property_or_method_or_accessor(
                target,
                sf::GET_ACCESSOR,
                sf::GET_ACCESSOR_EXCLUDES,
            ),
            Some(K::SetAccessor) => self.bind_property_or_method_or_accessor(
                target,
                sf::SET_ACCESSOR,
                sf::SET_ACCESSOR_EXCLUDES,
            ),
            Some(K::FunctionType | K::ConstructorType) => {
                self.bind_function_or_constructor_type(target);
            }
            Some(K::TypeLiteral | K::MappedType) => self.bind_anonymous_target(
                target,
                sf::TYPE_LITERAL,
                JsString::from_bytes(names::TYPE),
            ),
            Some(K::ObjectLiteralExpression) => self.bind_anonymous_target(
                target,
                sf::OBJECT_LITERAL,
                JsString::from_bytes(names::OBJECT),
            ),
            Some(K::FunctionExpression | K::ArrowFunction) => self.bind_function_expression(target),
            Some(K::ClassExpression | K::ClassDeclaration) => {
                self.bind_class_like_declaration(target);
            }
            Some(K::InterfaceDeclaration) => {
                self.bind_block_scoped_target(target, sf::INTERFACE, sf::INTERFACE_EXCLUDES);
            }
            Some(K::CallExpression) => {
                match checked(a::get_assignment_declaration_kind(self.view(), node)) {
                    a::JSDeclarationKind::ObjectDefinePropertyValue => {
                        self.bind_expando_property_assignment(node);
                    }
                    a::JSDeclarationKind::ObjectDefinePropertyExports => {
                        self.bind_exports_or_object_define_property(node);
                    }
                    _ => {}
                }
                if a::is_in_js_file(Some(&self.n(node))) {
                    self.bind_call_expression(node);
                }
            }
            Some(K::TypeAliasDeclaration) => {
                self.bind_block_scoped_target(target, sf::TYPE_ALIAS, sf::TYPE_ALIAS_EXCLUDES);
            }
            Some(K::JSTypeAliasDeclaration) => {
                if self
                    .block_scope_container
                    .is_none_or(|id| self.n(id).kind() != K::SourceFile)
                {
                    self.bind_block_scoped_target(target, sf::TYPE_ALIAS, sf::TYPE_ALIAS_EXCLUDES);
                }
            }
            Some(K::EnumDeclaration) => self.bind_enum_declaration(target),
            Some(K::ModuleDeclaration) => self.bind_module_declaration(node),
            Some(
                K::ImportEqualsDeclaration
                | K::NamespaceImport
                | K::ImportSpecifier
                | K::ExportSpecifier,
            ) => {
                self.declare_target_symbol(target, sf::ALIAS, sf::ALIAS_EXCLUDES);
            }
            Some(K::NamespaceExportDeclaration) => self.bind_namespace_export_declaration(node),
            Some(K::ImportClause) => self.bind_import_clause(node),
            Some(K::ExportDeclaration) => self.bind_export_declaration(node),
            Some(K::ExportAssignment) => self.bind_export_assignment(node),
            Some(K::SourceFile) => self.bind_source_file_if_external_module(),
            Some(K::JsxAttributes) => self.bind_jsx_attributes(target),
            Some(K::JsxAttribute) => {
                self.bind_jsx_attribute(target, sf::PROPERTY, sf::PROPERTY_EXCLUDES);
            }
            _ => {}
        }
    }
}
