use crate::{
    ast as a, checked, need,
    symbol_access::BindingSymbol,
    table_access::BindingTable,
    target::{target_payload, BindingNode},
    Binder,
};
use std::sync::Arc;
use ts_ast::{
    internal_symbol_names as names, modifier_flags as mf, symbol_flags as sf, JsString, NodeId,
    SymbolId, SymbolTableId, SyntaxKind as K,
};
use ts_diagnostics as d;

impl<'scope> Binder<'_, 'scope, '_> {
    // port: tsc/internal/binder/binder.go:Binder.declareSymbol
    pub fn declare_binding_symbol(
        &mut self,
        table: BindingTable<'scope>,
        parent: Option<BindingSymbol<'scope>>,
        node: BindingNode<'scope>,
        includes: u32,
        excludes: u32,
    ) -> BindingSymbol<'scope> {
        self.declare_binding_symbol_ex(table, parent, node, includes, excludes, false, false)
    }
    // port: tsc/internal/binder/binder.go:Binder.declareSymbolEx
    #[allow(
        clippy::too_many_arguments,
        reason = "preserves the pinned declaration conflict operation and independent computed/replaceable flags"
    )]
    pub fn declare_binding_symbol_ex(
        &mut self,
        table: BindingTable<'scope>,
        parent: Option<BindingSymbol<'scope>>,
        node: BindingNode<'scope>,
        includes: u32,
        excludes: u32,
        replaceable: bool,
        computed: bool,
    ) -> BindingSymbol<'scope> {
        assert!(computed || !self.target_has_dynamic_name(Some(node)));
        let default_export = self.target_has_syntactic_modifier(node, mf::DEFAULT)
            || self.node_kind(node) == K::ExportSpecifier
                && checked(a::module_export_name_is_default(
                    self.view(),
                    self.node_id(need(self.node_name(node))),
                ));
        let name = if computed {
            JsString::from_bytes(names::COMPUTED)
        } else if default_export && parent.is_some() {
            JsString::from_bytes(names::DEFAULT)
        } else {
            self.declaration_name(node)
        };
        let mut symbol;
        if name.as_bytes() == names::MISSING {
            symbol = self.new_binding_symbol(sf::NONE, JsString::from_bytes(names::MISSING));
        } else if let Some(existing) = self.binding_table_get(table, name.as_bytes()).flatten() {
            symbol = existing;
            let flags = self.s_binding(symbol).flags();
            if replaceable && flags & sf::REPLACEABLE_BY_METHOD == 0 {
                return symbol;
            }
            if flags & excludes != 0 {
                if flags & sf::REPLACEABLE_BY_METHOD != 0 {
                    symbol = self.new_binding_symbol(sf::NONE, name.clone());
                    self.binding_table_insert(table, name, Some(symbol));
                } else if !(includes & sf::VARIABLE != 0 && flags & sf::ASSIGNMENT != 0
                    || includes & sf::ASSIGNMENT != 0 && flags & sf::VARIABLE != 0)
                {
                    let mut message = if flags & sf::BLOCK_SCOPED_VARIABLE != 0 {
                        d::Cannot_redeclare_block_scoped_variable_0
                    } else {
                        d::Duplicate_identifier_0
                    };
                    let mut needs_name = true;
                    if (flags | includes) & sf::ENUM != 0 {
                        message = d::Enum_declarations_can_only_merge_with_namespace_or_other_enum_declarations;
                        needs_name = false;
                    }
                    let declarations = checked(
                        self.builder
                            .declarations()
                            .get(self.s_binding(symbol).declarations()),
                    )
                    .to_vec();
                    let multiple_defaults = !declarations.is_empty()
                        && (default_export
                            || self.node_kind(node) == K::ExportAssignment
                                && !target_payload!(self, node, as_export_assignment, "export assignment payload"; scalar: is_export_equals).0);
                    if multiple_defaults {
                        message = d::A_module_cannot_have_multiple_default_exports;
                        needs_name = false;
                    }
                    let declaration_name =
                        self.target_name_of_declaration(Some(node)).unwrap_or(node);
                    let args = if needs_name {
                        vec![self.display_name(node)]
                    } else {
                        Vec::new()
                    };
                    let mut diagnostic = self.create_diagnostic_for_node(
                        self.node_id(declaration_name),
                        message,
                        args,
                    );
                    if self.node_kind(node) == K::TypeAliasDeclaration
                        && self
                            .node_type(node)
                            .map(|id| self.n(self.node_id(id)))
                            .is_none_or(|node| a::node_is_missing(Some(&node)))
                        && self.target_has_syntactic_modifier(node, mf::EXPORT)
                        && flags & (sf::ALIAS | sf::TYPE | sf::NAMESPACE) != 0
                    {
                        let text = self.target_text(need(self.node_name(node)));
                        let suggestion = JsString::from_bytes(
                            [b"export type { ".as_slice(), text.as_bytes(), b" }"].concat(),
                        );
                        diagnostic.related_information.push(Arc::new(
                            self.create_diagnostic_for_node(
                                self.node_id(node),
                                d::Did_you_mean_0,
                                vec![suggestion],
                            ),
                        ));
                    }
                    for (index, declaration) in declarations.into_iter().enumerate() {
                        let declaration = self.binding_node(need(declaration));
                        let name_node = self
                            .target_name_of_declaration(Some(declaration))
                            .unwrap_or(declaration);
                        let args = if needs_name {
                            vec![self.display_name(declaration)]
                        } else {
                            Vec::new()
                        };
                        let mut previous =
                            self.create_diagnostic_for_node(self.node_id(name_node), message, args);
                        if multiple_defaults {
                            previous.related_information.push(Arc::new(
                                self.create_diagnostic_for_node(
                                    self.node_id(declaration_name),
                                    if index == 0 {
                                        d::Another_export_default_is_here
                                    } else {
                                        d::X_and_here
                                    },
                                    Vec::new(),
                                ),
                            ));
                        }
                        self.add_diagnostic(previous);
                        if multiple_defaults {
                            diagnostic.related_information.push(Arc::new(
                                self.create_diagnostic_for_node(
                                    self.node_id(name_node),
                                    d::The_first_export_default_is_here,
                                    Vec::new(),
                                ),
                            ));
                        }
                    }
                    self.add_diagnostic(diagnostic);
                    if flags & sf::ACCESSOR != 0 && flags & sf::ACCESSOR != includes & sf::ACCESSOR
                    {
                        *self.binding_symbol_flags_mut(symbol) |= sf::ACCESSOR;
                    }
                    symbol = self.new_binding_symbol(sf::NONE, name);
                }
            }
        } else {
            symbol = self.new_binding_symbol(sf::NONE, name.clone());
            self.binding_table_insert(table, name, Some(symbol));
            if replaceable {
                *self.binding_symbol_flags_mut(symbol) |= sf::REPLACEABLE_BY_METHOD;
            }
        }
        self.add_binding_declaration(symbol, node, includes);
        if self.s_binding(symbol).parent().is_none() {
            self.set_binding_symbol_parent(symbol, parent);
        } else if !self.same_symbol(self.binding_symbol_parent(symbol), parent) {
            // Retain the checked path's diagnostic values for a mismatch.
            assert_eq!(
                self.s_binding(symbol).parent(),
                parent.map(|parent| self.symbol_id(parent)),
                "Existing symbol parent should match new one"
            );
        }
        symbol
    }
    // port: tsc/internal/binder/binder.go:Binder.getDeclarationName
    pub fn declaration_name(&self, node: BindingNode<'scope>) -> JsString {
        if self.node_kind(node) == K::ExportAssignment {
            return JsString::from_bytes(
                if target_payload!(self, node, as_export_assignment, "export assignment payload"; scalar: is_export_equals).0 {
                    names::EXPORT_EQUALS
                } else {
                    names::DEFAULT
                },
            );
        }
        if let Some(name) = self.target_name_of_declaration(Some(node)) {
            if self.target_is_ambient_module(node) {
                let module_name = self.target_text(name);
                if a::is_global_scope_augmentation(&self.n(self.node_id(node))) {
                    return JsString::from_bytes(names::GLOBAL);
                }
                let pattern = ts_core::pattern::Pattern::parse(module_name.as_bytes());
                if pattern.is_valid() && pattern.star_index >= 0 {
                    if let Some(attributes) =
                        target_payload!(self, node, as_module_declaration, "module payload"; node: attributes).0
                    {
                        let id = a::runtime_node_id(&self.n(self.node_id(attributes)));
                        return JsString::from_bytes(
                            [
                                b"\xfe\"".as_slice(),
                                module_name.as_bytes(),
                                b"\"pattern@",
                                id.to_string().as_bytes(),
                            ]
                            .concat(),
                        );
                    }
                }
                return JsString::from_bytes(
                    [b"\"".as_slice(), module_name.as_bytes(), b"\""].concat(),
                );
            }
            if self.node_kind(name) == K::PrivateIdentifier {
                let Some(class) = checked(a::get_containing_class(self.view(), self.node_id(node)))
                else {
                    return JsString::from_bytes(names::MISSING);
                };
                return get_symbol_name_for_private_identifier(
                    &self.s_binding(need(self.node_binding_symbol(self.binding_node(class)))),
                    self.target_text(name).as_bytes(),
                );
            }
            if a::is_property_name_literal_kind(self.node_kind(name))
                || self.node_kind(name) == K::JsxNamespacedName
            {
                return self.target_text(name);
            }
            if self.node_kind(name) == K::ComputedPropertyName {
                let expression = need(self.node_expression(name));
                if a::is_string_or_numeric_literal_like_kind(self.node_kind(expression)) {
                    return self.target_text(expression);
                }
                if a::is_signed_numeric_literal(self.view(), self.node_id(expression))
                    .expect("computed literal graph")
                {
                    let (operator, operand) = target_payload!(self, expression, as_prefix_unary_expression, "prefix payload"; scalar: operator, node: operand);
                    let token = ts_scanner::token_to_string(operator.known().unwrap_or(K::Unknown));
                    return JsString::from_bytes(
                        [token.as_bytes(), self.target_text(need(operand)).as_bytes()].concat(),
                    );
                }
                panic!("Only computed properties with literal names have declaration names");
            }
            return JsString::from_bytes(names::MISSING);
        }
        JsString::from_bytes(match self.node_kind(node).known() {
            Some(K::Constructor) => names::CONSTRUCTOR,
            Some(K::FunctionType | K::CallSignature) => names::CALL,
            Some(K::ConstructorType | K::ConstructSignature) => names::NEW,
            Some(K::IndexSignature) => names::INDEX,
            Some(K::ExportDeclaration) => names::EXPORT_STAR,
            Some(K::SourceFile | K::BinaryExpression) => names::EXPORT_EQUALS,
            _ => names::MISSING,
        })
    }
    // port: tsc/internal/binder/binder.go:Binder.getDisplayName
    pub fn display_name(&self, node: BindingNode<'scope>) -> JsString {
        if let Some(name) = self.node_name(node) {
            return checked(ts_scanner::declaration_name_to_string(
                self.view(),
                Some(self.node_id(name)),
            ));
        }
        let name = self.declaration_name(node);
        if name.as_bytes() == names::MISSING {
            JsString::from_bytes(&b"(Missing)"[..])
        } else {
            name
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.addDeclarationToSymbol
    pub fn add_binding_declaration(
        &mut self,
        symbol: BindingSymbol<'scope>,
        node: BindingNode<'scope>,
        flags: u32,
    ) {
        *self.binding_symbol_flags_mut(symbol) |= flags;
        self.set_binding_node_symbol(node, Some(symbol));
        let declarations = self.s_binding(symbol).declarations();
        let raw_node = self.node_id(node);
        let declarations = if declarations.is_nil() {
            self.new_single_declaration(Some(raw_node))
        } else {
            checked(
                self.builder
                    .declarations_mut()
                    .append_if_unique(declarations, Some(raw_node)),
            )
        };
        self.set_binding_symbol_declarations(symbol, declarations);
        let existing = self.s_binding(symbol).flags();
        if existing & sf::CONST_ENUM_ONLY_MODULE != 0
            && existing & (sf::FUNCTION | sf::CLASS | sf::REGULAR_ENUM) != 0
        {
            *self.binding_symbol_flags_mut(symbol) &= !sf::CONST_ENUM_ONLY_MODULE;
            self.not_const_enum_only_modules
                .insert(self.symbol_id(symbol));
        }
        if flags & sf::VALUE != 0 {
            self.set_binding_value_declaration(symbol, node);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.newSingleDeclaration
    pub fn new_single_declaration(&mut self, node: Option<NodeId>) -> a::DeclarationSlice {
        checked(self.builder.declarations_mut().alloc_one(node))
    }
    // port: tsc/internal/binder/binder.go:SetValueDeclaration
    pub fn set_binding_value_declaration(
        &mut self,
        symbol: BindingSymbol<'scope>,
        node: BindingNode<'scope>,
    ) {
        let previous = self
            .s_binding(symbol)
            .value_declaration()
            .map(|node| self.binding_node(node));
        if previous.is_none_or(|previous| {
            is_assignment_declaration_kind(self.node_kind(previous))
                && !is_assignment_declaration_kind(self.node_kind(node))
                || self.node_kind(previous) != self.node_kind(node)
                    && is_effective_module_declaration_kind(self.node_kind(previous))
        }) {
            self.set_binding_symbol_value_declaration(symbol, Some(node));
        }
    }
}
// port: tsc/internal/binder/binder.go:GetSymbolNameForPrivateIdentifier
pub fn get_symbol_name_for_private_identifier(
    symbol: &(impl a::SymbolAccess + ?Sized),
    description: &[u8],
) -> JsString {
    let id = a::runtime_symbol_id(symbol) as isize;
    JsString::from_bytes(
        [
            b"\xfe#".as_slice(),
            id.to_string().as_bytes(),
            b"@",
            description,
        ]
        .concat(),
    )
}
// port: tsc/internal/binder/binder.go:isAssignmentDeclaration
fn is_assignment_declaration_kind(kind: a::NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::BinaryExpression
                | K::PropertyAccessExpression
                | K::ElementAccessExpression
                | K::Identifier
                | K::CallExpression
        )
    )
}
// port: tsc/internal/binder/binder.go:isEffectiveModuleDeclaration
fn is_effective_module_declaration_kind(kind: a::NodeKind) -> bool {
    matches!(kind.known(), Some(K::ModuleDeclaration | K::Identifier))
}

impl<'scope> Binder<'_, 'scope, '_> {
    // port: tsc/internal/binder/binder.go:Binder.declareModuleMember
    pub fn declare_binding_module_member(
        &mut self,
        node: BindingNode<'scope>,
        flags: u32,
        excludes: u32,
    ) -> BindingSymbol<'scope> {
        let container = self.binding_node(need(self.container));
        let exported = self.target_combined_modifier_flags(node) & mf::EXPORT != 0
            || checked(a::is_implicitly_exported_js_doc_declaration(
                self.view(),
                self.node_id(node),
            ));
        if flags & sf::ALIAS != 0 {
            if self.node_kind(node) == K::ExportSpecifier
                || self.node_kind(node) == K::ImportEqualsDeclaration && exported
            {
                let parent = need(self.node_binding_symbol(container));
                let table = self.ensure_binding_exports(parent);
                return self.declare_binding_symbol(table, Some(parent), node, flags, excludes);
            }
            let table = self.ensure_binding_locals(container);
            return self.declare_binding_symbol(table, None, node, flags, excludes);
        }
        if !self.target_is_ambient_module(node)
            && (exported || self.node_flags(container) & a::node_flags::EXPORT_CONTEXT != 0)
        {
            let parent = need(self.node_binding_symbol(container));
            let exports = self.ensure_binding_exports(parent);
            if !self.target_has_locals(container)
                || self.target_has_syntactic_modifier(node, mf::DEFAULT)
                    && self.declaration_name(node).as_bytes() == names::MISSING
            {
                return self.declare_binding_symbol(exports, Some(parent), node, flags, excludes);
            }
            let export_kind = if flags & sf::VALUE != 0 {
                sf::EXPORT_VALUE
            } else {
                sf::NONE
            };
            let table = self.ensure_binding_locals(container);
            let local = self.declare_binding_symbol(table, None, node, export_kind, excludes);
            let exported =
                self.declare_binding_symbol(exports, Some(parent), node, flags, excludes);
            self.set_binding_symbol_export_symbol(local, Some(exported));
            self.set_binding_node_local_symbol(node, Some(local));
            return local;
        }
        let table = self.ensure_binding_locals(container);
        self.declare_binding_symbol(table, None, node, flags, excludes)
    }
    // port: tsc/internal/binder/binder.go:Binder.declareClassMember
    pub fn declare_binding_class_member(
        &mut self,
        node: BindingNode<'scope>,
        flags: u32,
        excludes: u32,
    ) -> BindingSymbol<'scope> {
        let parent = need(self.node_binding_symbol(self.binding_node(need(self.container))));
        let table = if checked(a::is_static(self.view(), self.node_id(node))) {
            self.ensure_binding_exports(parent)
        } else {
            self.ensure_binding_members(parent)
        };
        self.declare_binding_symbol(table, Some(parent), node, flags, excludes)
    }
    // port: tsc/internal/binder/binder.go:Binder.declareSourceFileMember
    pub fn declare_binding_source_file_member(
        &mut self,
        node: BindingNode<'scope>,
        flags: u32,
        excludes: u32,
    ) -> BindingSymbol<'scope> {
        if checked(self.view().source_file(self.file))
            .external_module_indicator
            .is_some()
        {
            return self.declare_binding_module_member(node, flags, excludes);
        }
        let table = self.ensure_binding_locals(self.binding_node(self.file));
        self.declare_binding_symbol(table, None, node, flags, excludes)
    }
    // port: tsc/internal/binder/binder.go:Binder.declareSymbolAndAddToSymbolTable
    pub fn declare_target_symbol(
        &mut self,
        node: BindingNode<'scope>,
        flags: u32,
        excludes: u32,
    ) -> BindingSymbol<'scope> {
        let container = self.binding_node(need(self.container));
        match self.node_kind(container).known() {
            Some(K::ModuleDeclaration) => self.declare_binding_module_member(node, flags, excludes),
            Some(K::SourceFile) => self.declare_binding_source_file_member(node, flags, excludes),
            Some(K::ClassExpression | K::ClassDeclaration) => {
                self.declare_binding_class_member(node, flags, excludes)
            }
            Some(K::EnumDeclaration) => {
                let parent = need(self.node_binding_symbol(container));
                let table = self.ensure_binding_exports(parent);
                self.declare_binding_symbol(table, Some(parent), node, flags, excludes)
            }
            Some(
                K::TypeLiteral
                | K::ObjectLiteralExpression
                | K::InterfaceDeclaration
                | K::JsxAttributes,
            ) => {
                let parent = need(self.node_binding_symbol(container));
                let table = self.ensure_binding_members(parent);
                self.declare_binding_symbol(table, Some(parent), node, flags, excludes)
            }
            Some(
                K::FunctionType
                | K::ConstructorType
                | K::CallSignature
                | K::ConstructSignature
                | K::IndexSignature
                | K::MethodDeclaration
                | K::MethodSignature
                | K::Constructor
                | K::GetAccessor
                | K::SetAccessor
                | K::FunctionDeclaration
                | K::FunctionExpression
                | K::ArrowFunction
                | K::ClassStaticBlockDeclaration
                | K::TypeAliasDeclaration
                | K::JSTypeAliasDeclaration
                | K::MappedType,
            ) => {
                let table = self.ensure_binding_locals(container);
                self.declare_binding_symbol(table, None, node, flags, excludes)
            }
            _ => panic!("Unhandled case in declareSymbolAndAddToSymbolTable"),
        }
    }
}

// Checked entry points share the scoped declaration algorithm. Imported handles
// remain local throughout a declaration; conversion back is explicit here.
impl Binder<'_, '_, '_> {
    pub fn declare_symbol(
        &mut self,
        table: SymbolTableId,
        parent: Option<SymbolId>,
        node: NodeId,
        includes: u32,
        excludes: u32,
    ) -> SymbolId {
        let value = self.declare_binding_symbol(
            self.binding_table(table),
            parent.map(|id| self.binding_symbol(id)),
            self.binding_node(node),
            includes,
            excludes,
        );
        self.symbol_id(value)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn declare_symbol_ex(
        &mut self,
        table: SymbolTableId,
        parent: Option<SymbolId>,
        node: NodeId,
        includes: u32,
        excludes: u32,
        replaceable: bool,
        computed: bool,
    ) -> SymbolId {
        let value = self.declare_binding_symbol_ex(
            self.binding_table(table),
            parent.map(|id| self.binding_symbol(id)),
            self.binding_node(node),
            includes,
            excludes,
            replaceable,
            computed,
        );
        self.symbol_id(value)
    }
    pub fn get_declaration_name(&self, node: NodeId) -> JsString {
        self.declaration_name(self.binding_node(node))
    }
    pub fn set_value_declaration(&mut self, symbol: SymbolId, node: NodeId) {
        self.set_binding_value_declaration(self.binding_symbol(symbol), self.binding_node(node));
    }
    pub fn declare_symbol_and_add_to_symbol_table(
        &mut self,
        node: NodeId,
        flags: u32,
        excludes: u32,
    ) -> SymbolId {
        let symbol = self.declare_target_symbol(self.binding_node(node), flags, excludes);
        self.symbol_id(symbol)
    }
}
