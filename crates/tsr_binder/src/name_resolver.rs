//! Lexical lookup with the pinned binder's optional checker callbacks.
//! The host retains every file/checker arena referenced by its checked IDs.
use std::ops::ControlFlow;
use ts_arena::{Error, SymbolId};
use ts_ast::{
    internal_symbol_names as names, modifier_flags as modifiers, node_flags, symbol_flags as flags,
    utilities as u, utilities_middle as middle, AstView, ChildVisitor, DeclarationRead, JsString,
    NodeBinding, NodeDataRead, NodeId, NodeKind, NodeListId, NodeRead, NodeSlice, SymbolFlags,
    SymbolRef, SymbolTableId, SymbolTableRead, SyntaxKind as K,
};
use ts_core::{ScriptTarget, Tristate};
use ts_diagnostics::{self as diagnostics, Message};

/// A present source callback returning nil differs from an absent callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hook<T> {
    Absent,
    Value(T),
}

/// Effective CompilerOptions values, projected by the actual options owner.
#[derive(Clone, Copy, Debug)]
pub struct ResolverOptions {
    pub emit_script_target: ScriptTarget,
    pub isolated_modules: bool,
    pub verbatim_module_syntax: bool,
    pub emit_standard_class_fields: bool,
}

/// Reads validate identities against the host's retained owner set. AST views
/// include completed binding overlays. A transient symbol belongs to the host's
/// operation/checker universe, never to an immutable file bind result.
pub trait ResolverHost {
    fn ast(&self, node: NodeId) -> Result<AstView<'_>, Error>;
    fn binding(&self, node: NodeId) -> Result<Option<NodeBinding>, Error>;
    fn symbol(&self, symbol: SymbolId) -> Result<SymbolRef<'_>, Error>;
    fn table(&self, table: SymbolTableId) -> Result<SymbolTableRead<'_>, Error>;
    fn declarations(&self, symbol: SymbolId) -> Result<DeclarationRead<'_>, Error>;
    fn new_transient_symbol(
        &mut self,
        flags: SymbolFlags,
        name: JsString,
    ) -> Result<SymbolId, Error>;
    fn node(&self, node: NodeId) -> Result<NodeRead<'_>, Error> {
        self.ast(node)?.node(node)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResolvedName {
    pub original_location: Option<NodeId>,
    pub symbol: SymbolId,
    pub meaning: SymbolFlags,
    pub last_location: Option<NodeId>,
    pub associated_declaration: Option<NodeId>,
    pub within_deferred_context: bool,
}

/// Checker callbacks return identities owned by the supplied ResolverHost.
/// Defaults preserve absent source callbacks, including cache misses and quiet
/// diagnostics. Errors propagate without running later callbacks.
pub trait NameResolverHooks {
    fn get_symbol_of_declaration(
        &mut self,
        _node: NodeId,
    ) -> Result<Hook<Option<SymbolId>>, Error> {
        Ok(Hook::Absent)
    }
    fn lookup(
        &mut self,
        _table: Option<SymbolTableId>,
        _name: &[u8],
        _meaning: SymbolFlags,
    ) -> Result<Hook<Option<SymbolId>>, Error> {
        Ok(Hook::Absent)
    }
    fn error(
        &mut self,
        _location: Option<NodeId>,
        _message: &'static Message,
        _args: &[JsString],
    ) -> Result<(), Error> {
        Ok(())
    }
    fn symbol_referenced(&mut self, _symbol: SymbolId, _meaning: SymbolFlags) -> Result<(), Error> {
        Ok(())
    }
    fn get_requires_scope_change_cache(&mut self, _node: NodeId) -> Result<Tristate, Error> {
        Ok(Tristate::UNKNOWN)
    }
    fn set_requires_scope_change_cache(
        &mut self,
        _node: NodeId,
        _value: Tristate,
    ) -> Result<(), Error> {
        Ok(())
    }
    fn on_property_with_invalid_initializer(
        &mut self,
        _location: Option<NodeId>,
        _name: &[u8],
        _declaration: NodeId,
        _result: Option<SymbolId>,
    ) -> Result<bool, Error> {
        Ok(false)
    }
    fn on_failed_to_resolve_symbol(
        &mut self,
        _location: Option<NodeId>,
        _name: &[u8],
        _meaning: SymbolFlags,
        _message: &'static Message,
    ) -> Result<(), Error> {
        Ok(())
    }
    fn on_successfully_resolved_symbol(&mut self, _resolution: ResolvedName) -> Result<(), Error> {
        Ok(())
    }
}
pub struct NoNameResolverHooks;
impl NameResolverHooks for NoNameResolverHooks {}

/// One resolver is attached to one retained host universe. Cached globals,
/// `arguments`, and `require` identities must remain valid in that universe.
pub struct NameResolver {
    pub options: ResolverOptions,
    pub globals: Option<SymbolTableId>,
    pub arguments_symbol: Option<SymbolId>,
    pub require_symbol: Option<SymbolId>,
}
impl NameResolver {
    pub fn new(options: ResolverOptions) -> Self {
        Self {
            options,
            globals: None,
            arguments_symbol: None,
            require_symbol: None,
        }
    }

    /// port: tsc/internal/binder/nameresolver.go:NameResolver.Resolve
    #[allow(clippy::too_many_arguments)]
    pub fn resolve(
        &mut self,
        host: &mut dyn ResolverHost,
        hooks: &mut dyn NameResolverHooks,
        mut location: Option<NodeId>,
        name: &[u8],
        meaning: SymbolFlags,
        name_not_found_message: Option<&'static Message>,
        is_use: bool,
        exclude_globals: bool,
    ) -> Result<Option<SymbolId>, Error> {
        let mut result = None;
        let mut last_location = None;
        let mut last_self_reference_location = None;
        let mut property_with_invalid_initializer = None;
        let mut associated_declaration = None;
        let mut within_deferred_context = false;
        let original_location = location;
        let name_is_const = name == b"const";
        'search: while let Some(mut current) = location {
            if name_is_const
                && middle::is_const_assertion(host.ast(current)?, &host.node(current)?)?
            {
                return Ok(None);
            }
            if matches!(
                kind(host, current)?.known(),
                Some(K::ModuleDeclaration | K::EnumDeclaration)
            ) && last_location.is_some()
                && host.node(current)?.name() == last_location
            {
                last_location = Some(current);
                current = required(host.node(current)?.parent());
            }
            let module_attributes = match host.node(current)?.data() {
                NodeDataRead::ModuleDeclaration(data) => {
                    data.attributes().is_some() && last_location == data.attributes()
                }
                _ => false,
            };
            let locals = host.binding(current)?.and_then(|binding| binding.locals);
            if locals.is_some() && !middle::is_global_source_file(host.ast(current)?, current)? {
                result = Self::lookup(host, hooks, locals, name, meaning)?;
                if let Some(symbol) = result {
                    let symbol_flags = host.symbol(symbol)?.flags();
                    let mut use_result = true;
                    if module_attributes {
                        use_result = false;
                    } else if u::is_function_like(Some(&host.node(current)?))
                        && last_location.is_some()
                        && last_location != host.node(current)?.body()
                    {
                        let last = required(last_location);
                        let last_kind = kind(host, last)?;
                        let synthesized = host.node(last)?.flags() & node_flags::SYNTHESIZED != 0;
                        if meaning & symbol_flags & flags::TYPE != 0 && last_kind != K::JSDoc {
                            use_result = symbol_flags & flags::TYPE_PARAMETER != 0
                                && (synthesized
                                    || Some(last) == host.node(current)?.type_node()
                                    || matches!(
                                        last_kind.known(),
                                        Some(
                                            K::Parameter
                                                | K::JSDocParameterTag
                                                | K::JSDocReturnTag
                                                | K::TypeParameter
                                        )
                                    ));
                        }
                        if meaning & symbol_flags & flags::VARIABLE != 0 {
                            if self.use_outer_variable_scope_in_parameter(
                                host, hooks, symbol, current, last,
                            )? {
                                use_result = false;
                            } else if symbol_flags & flags::FUNCTION_SCOPED_VARIABLE != 0 {
                                use_result = last_kind == K::Parameter
                                    || synthesized
                                    || Some(last) == host.node(current)?.type_node()
                                        && ancestor_kind(
                                            host,
                                            host.symbol(symbol)?.value_declaration(),
                                            K::Parameter,
                                        )?
                                        .is_some();
                            }
                        }
                    } else if kind(host, current)? == K::ConditionalType {
                        use_result = last_location
                            == host
                                .node(current)?
                                .data_source()
                                .as_conditional_type_node()
                                .expect("ConditionalType payload")
                                .true_type();
                    }
                    if use_result {
                        break 'search;
                    }
                    result = None;
                }
            }
            within_deferred_context =
                within_deferred_context || get_is_deferred_context(host, current, last_location)?;
            match kind(host, current)?.known() {
                Some(K::SourceFile | K::ModuleDeclaration) => {
                    if !(module_attributes
                        || kind(host, current)? == K::SourceFile
                            && middle::is_global_source_file(host.ast(current)?, current)?)
                    {
                        if let Some(module_symbol) =
                            Self::get_symbol_of_declaration(host, hooks, current)?
                        {
                            let exports = host.symbol(module_symbol)?.exports();
                            let external = kind(host, current)? == K::SourceFile
                                || host.node(current)?.flags() & node_flags::AMBIENT != 0
                                    && !u::is_global_scope_augmentation(&host.node(current)?);
                            let mut pure_alias = false;
                            if external {
                                result = table_entry(host, exports, names::DEFAULT)?;
                                if let Some(symbol) = result {
                                    if let Some(local) =
                                        get_local_symbol_for_export_default(host, Some(symbol))?
                                    {
                                        if host.symbol(symbol)?.flags() & meaning != 0
                                            && host.symbol(local)?.name_bytes() == name
                                        {
                                            break 'search;
                                        }
                                    }
                                    result = None;
                                }
                                if let Some(symbol) = table_entry(host, exports, name)? {
                                    pure_alias = host.symbol(symbol)?.flags() == flags::ALIAS
                                        && (declaration_of_kind(host, symbol, K::ExportSpecifier)?
                                            .is_some()
                                            || declaration_of_kind(
                                                host,
                                                symbol,
                                                K::NamespaceExport,
                                            )?
                                            .is_some());
                                }
                            }
                            if !pure_alias && name != names::DEFAULT {
                                result = Self::lookup(
                                    host,
                                    hooks,
                                    exports,
                                    name,
                                    meaning & flags::MODULE_MEMBER,
                                )?;
                                if let Some(symbol) = result {
                                    let common_js = kind(host, current)? == K::SourceFile
                                        && host
                                            .ast(current)?
                                            .source_file(current)?
                                            .common_js_module_indicator()
                                            .is_some();
                                    if common_js && host.symbol(symbol)?.flags() & flags::TYPE == 0
                                    {
                                        result = None;
                                    } else {
                                        break 'search;
                                    }
                                }
                            }
                        }
                    }
                }
                Some(K::EnumDeclaration) => {
                    if let Some(enum_symbol) =
                        Self::get_symbol_of_declaration(host, hooks, current)?
                    {
                        result = Self::lookup(
                            host,
                            hooks,
                            host.symbol(enum_symbol)?.exports(),
                            name,
                            meaning & flags::ENUM_MEMBER,
                        )?;
                        if let Some(symbol) = result {
                            if name_not_found_message.is_some()
                                && self.options.isolated_modules
                                && host.node(current)?.flags() & node_flags::AMBIENT == 0
                                && source_file(host, Some(current))?
                                    != source_file(host, host.symbol(symbol)?.value_declaration())?
                            {
                                let option = if self.options.verbatim_module_syntax {
                                    b"verbatimModuleSyntax".as_slice()
                                } else {
                                    b"isolatedModules"
                                };
                                let qualified =
                                    [host.symbol(enum_symbol)?.name_bytes(), b".", name].concat();
                                Self::error(hooks, original_location, diagnostics::Cannot_access_0_from_another_file_without_qualification_when_1_is_enabled_Use_2_instead,
                                    &[JsString::from_bytes(name), JsString::from_bytes(option), JsString::from_bytes(qualified)])?;
                            }
                            break 'search;
                        }
                    }
                }
                Some(K::PropertyDeclaration) => {
                    if !u::is_static(host.ast(current)?, current)? {
                        if let Some(constructor) = ts_ast::find_constructor_declaration(
                            host.ast(current)?,
                            required(host.node(current)?.parent()),
                        )? {
                            if let Some(locals) = host.binding(constructor)?.and_then(|b| b.locals)
                            {
                                if Self::lookup(
                                    host,
                                    hooks,
                                    Some(locals),
                                    name,
                                    meaning & flags::VALUE,
                                )?
                                .is_some()
                                {
                                    property_with_invalid_initializer = Some(current);
                                }
                            }
                        }
                    }
                }
                Some(K::ClassDeclaration | K::ClassExpression | K::InterfaceDeclaration) => {
                    let container_symbol =
                        required_symbol(Self::get_symbol_of_declaration(host, hooks, current)?);
                    result = Self::lookup(
                        host,
                        hooks,
                        host.symbol(container_symbol)?.members(),
                        name,
                        meaning & flags::TYPE,
                    )?;
                    if let Some(symbol) = result {
                        if is_type_parameter_symbol_declared_in_container(host, symbol, current)? {
                            if let Some(last) = last_location {
                                if u::is_static(host.ast(last)?, last)? {
                                    if name_not_found_message.is_some() {
                                        Self::error(hooks, original_location, diagnostics::Static_members_cannot_reference_class_type_parameters, &[])?;
                                    }
                                    return Ok(None);
                                }
                            }
                            break 'search;
                        }
                        result = None;
                    } else if kind(host, current)? == K::ClassExpression
                        && meaning & flags::CLASS != 0
                        && node_name_equals(host, current, name)?
                    {
                        result = node_symbol(host, current)?;
                        break 'search;
                    }
                }
                Some(K::ExpressionWithTypeArguments)
                    if last_location == host.node(current)?.expression() =>
                {
                    let parent = required(host.node(current)?.parent());
                    if kind(host, parent)? == K::HeritageClause
                        && host
                            .node(parent)?
                            .data_source()
                            .as_heritage_clause()
                            .expect("HeritageClause payload")
                            .token()
                            == K::ExtendsKeyword
                    {
                        let container = required(host.node(parent)?.parent());
                        if u::is_class_like(&host.node(container)?) {
                            let symbol = required_symbol(Self::get_symbol_of_declaration(
                                host, hooks, container,
                            )?);
                            if Self::lookup(
                                host,
                                hooks,
                                host.symbol(symbol)?.members(),
                                name,
                                meaning & flags::TYPE,
                            )?
                            .is_some()
                            {
                                if name_not_found_message.is_some() {
                                    Self::error(hooks, original_location, diagnostics::Base_class_expressions_cannot_reference_class_type_parameters, &[])?;
                                }
                                return Ok(None);
                            }
                        }
                    }
                }
                Some(K::ComputedPropertyName) => {
                    let parent = required(host.node(current)?.parent());
                    let grandparent = required(host.node(parent)?.parent());
                    if u::is_class_like(&host.node(grandparent)?)
                        || kind(host, grandparent)? == K::InterfaceDeclaration
                    {
                        let symbol = required_symbol(Self::get_symbol_of_declaration(
                            host,
                            hooks,
                            grandparent,
                        )?);
                        if Self::lookup(
                            host,
                            hooks,
                            host.symbol(symbol)?.members(),
                            name,
                            meaning & flags::TYPE,
                        )?
                        .is_some()
                        {
                            if name_not_found_message.is_some() {
                                Self::error(hooks, original_location, diagnostics::A_computed_property_name_cannot_reference_a_type_parameter_from_its_containing_type, &[])?;
                            }
                            return Ok(None);
                        }
                    }
                }
                Some(
                    K::MethodDeclaration
                    | K::Constructor
                    | K::GetAccessor
                    | K::SetAccessor
                    | K::FunctionDeclaration
                    | K::FunctionExpression,
                ) => {
                    if meaning & flags::VARIABLE != 0 && name == b"arguments" {
                        result = Some(self.arguments_symbol(host)?);
                        break 'search;
                    }
                    if kind(host, current)? == K::FunctionExpression
                        && meaning & flags::FUNCTION != 0
                        && node_name_equals(host, current, name)?
                    {
                        result = node_symbol(host, current)?;
                        break 'search;
                    }
                }
                Some(K::Decorator) => {
                    if let Some(parent) = host.node(current)?.parent() {
                        if kind(host, parent)? == K::Parameter {
                            current = parent;
                        }
                    }
                    if let Some(parent) = host.node(current)?.parent() {
                        if u::is_class_element(&host.node(parent)?)
                            || kind(host, parent)? == K::ClassDeclaration
                        {
                            current = parent;
                        }
                    }
                }
                Some(K::Parameter | K::BindingElement) => {
                    if let Some(last) = last_location {
                        let node = host.node(current)?;
                        if (Some(last) == node.initializer()
                            || Some(last) == node.name()
                                && u::is_binding_pattern(&host.node(last)?))
                            && (kind(host, current)? == K::Parameter
                                || u::is_part_of_parameter_declaration(
                                    host.ast(current)?,
                                    current,
                                )?)
                            && associated_declaration.is_none()
                        {
                            associated_declaration = Some(current);
                        }
                    }
                }
                Some(K::InferType) => {
                    if meaning & flags::TYPE_PARAMETER != 0 {
                        let parameter = required(
                            host.node(current)?
                                .data_source()
                                .as_infer_type_node()
                                .expect("InferType payload")
                                .type_parameter(),
                        );
                        if node_name_equals(host, parameter, name)? {
                            result = node_symbol(host, parameter)?;
                            break 'search;
                        }
                    }
                }
                Some(K::ExportSpecifier) => {
                    let property = host
                        .node(current)?
                        .data_source()
                        .as_export_specifier()
                        .expect("ExportSpecifier payload")
                        .property_name();
                    if last_location.is_some() && last_location == property {
                        let parent = required(host.node(current)?.parent());
                        let grandparent = required(host.node(parent)?.parent());
                        if host.node(grandparent)?.module_specifier().is_some() {
                            current = required(host.node(grandparent)?.parent());
                        }
                    }
                }
                _ => {}
            }
            if is_self_reference_location(host, current, last_location)? {
                last_self_reference_location = Some(current);
            }
            last_location = Some(current);
            location = host.node(current)?.parent();
        }
        if is_use {
            if let Some(symbol) = result {
                if last_self_reference_location
                    .map(|node| node_symbol(host, node))
                    .transpose()?
                    .flatten()
                    != Some(symbol)
                {
                    hooks.symbol_referenced(symbol, meaning)?;
                }
            }
        }
        if result.is_none() && !exclude_globals {
            result = Self::lookup(
                host,
                hooks,
                self.globals,
                name,
                meaning | flags::GLOBAL_LOOKUP,
            )?;
        }
        if result.is_none() {
            if let Some(original) = original_location {
                if u::is_in_js_file(Some(&host.node(original)?)) {
                    if let Some(parent) = host.node(original)?.parent() {
                        if middle::is_require_call(host.ast(parent)?, &host.node(parent)?, false)? {
                            return Ok(self.require_symbol);
                        }
                    }
                }
            }
        }
        if let Some(message) = name_not_found_message {
            if let Some(property) = property_with_invalid_initializer {
                if hooks.on_property_with_invalid_initializer(
                    original_location,
                    name,
                    property,
                    result,
                )? {
                    return Ok(None);
                }
            }
            if let Some(symbol) = result {
                hooks.on_successfully_resolved_symbol(ResolvedName {
                    original_location,
                    symbol,
                    meaning,
                    last_location,
                    associated_declaration,
                    within_deferred_context,
                })?;
            } else {
                hooks.on_failed_to_resolve_symbol(original_location, name, meaning, message)?;
            }
        }
        Ok(result)
    }

    /// port: tsc/internal/binder/nameresolver.go:NameResolver.useOuterVariableScopeInParameter
    fn use_outer_variable_scope_in_parameter(
        &self,
        host: &dyn ResolverHost,
        hooks: &mut dyn NameResolverHooks,
        result: SymbolId,
        location: NodeId,
        last: NodeId,
    ) -> Result<bool, Error> {
        if kind(host, last)? == K::Parameter {
            if let (Some(body), Some(value)) = (
                host.node(location)?.body(),
                host.symbol(result)?.value_declaration(),
            ) {
                if host.node(value)?.pos() >= host.node(body)?.pos()
                    && host.node(value)?.end() <= host.node(body)?.end()
                {
                    let mut state = hooks.get_requires_scope_change_cache(location)?;
                    if state == Tristate::UNKNOWN {
                        let view = host.ast(location)?;
                        let parameters = host.node(location)?.parameters(view)?;
                        let mut requires = false;
                        for parameter in view.node_slice(parameters)?.iter() {
                            if self.requires_scope_change(host, required(parameter))? {
                                requires = true;
                                break;
                            }
                        }
                        state = Tristate::from(requires);
                        hooks.set_requires_scope_change_cache(location, state)?;
                    }
                    return Ok(state != Tristate::TRUE);
                }
            }
        }
        Ok(false)
    }
    /// port: tsc/internal/binder/nameresolver.go:NameResolver.requiresScopeChange
    fn requires_scope_change(&self, host: &dyn ResolverHost, node: NodeId) -> Result<bool, Error> {
        let (name, initializer) = {
            let n = host.node(node)?;
            let d = n
                .data_source()
                .as_parameter_declaration()
                .expect("ParameterDeclaration payload");
            (d.name(), d.initializer())
        };
        Ok(self.requires_scope_change_worker(host, required(name))?
            || initializer
                .map(|id| self.requires_scope_change_worker(host, id))
                .transpose()?
                .unwrap_or(false))
    }
    /// port: tsc/internal/binder/nameresolver.go:NameResolver.requiresScopeChangeWorker
    fn requires_scope_change_worker(
        &self,
        host: &dyn ResolverHost,
        id: NodeId,
    ) -> Result<bool, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let node = host.node(id)?;
            match node.kind().known() {
                Some(
                    K::ArrowFunction
                    | K::FunctionExpression
                    | K::FunctionDeclaration
                    | K::Constructor,
                ) => Ok(false),
                Some(
                    K::MethodDeclaration | K::GetAccessor | K::SetAccessor | K::PropertyAssignment,
                ) => self.requires_scope_change_worker(host, required(node.name())),
                Some(K::PropertyDeclaration) => {
                    if u::has_static_modifier(host.ast(id)?, id)? {
                        Ok(!self.options.emit_standard_class_fields)
                    } else {
                        self.requires_scope_change_worker(host, required(node.name()))
                    }
                }
                _ => {
                    if u::is_nullish_coalesce(host.ast(id)?, id)? || u::is_optional_chain(&node) {
                        return Ok(self.options.emit_script_target < ScriptTarget::ES2020);
                    }
                    if node.kind() == K::BindingElement
                        && node
                            .data_source()
                            .as_binding_element()
                            .expect("BindingElement payload")
                            .dot_dot_dot_token()
                            .is_some()
                        && kind(host, required(node.parent()))? == K::ObjectBindingPattern
                    {
                        return Ok(self.options.emit_script_target < ScriptTarget::ES2017);
                    }
                    if u::is_type_node(&node) {
                        return Ok(false);
                    }
                    let mut visitor = ScopeVisitor {
                        resolver: self,
                        host,
                        view: host.ast(id)?,
                        result: Ok(false),
                    };
                    let _ = node.for_each_child(&mut visitor);
                    visitor.result
                }
            }
        })
    }
    /// port: tsc/internal/binder/nameresolver.go:NameResolver.error
    fn error(
        hooks: &mut dyn NameResolverHooks,
        location: Option<NodeId>,
        message: &'static Message,
        args: &[JsString],
    ) -> Result<(), Error> {
        hooks.error(location, message, args)
    }
    /// port: tsc/internal/binder/nameresolver.go:NameResolver.getSymbolOfDeclaration
    fn get_symbol_of_declaration(
        host: &dyn ResolverHost,
        hooks: &mut dyn NameResolverHooks,
        node: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        match hooks.get_symbol_of_declaration(node)? {
            Hook::Absent => node_symbol(host, node),
            Hook::Value(value) => Ok(value),
        }
    }
    /// port: tsc/internal/binder/nameresolver.go:NameResolver.lookup
    fn lookup(
        host: &dyn ResolverHost,
        hooks: &mut dyn NameResolverHooks,
        table: Option<SymbolTableId>,
        name: &[u8],
        meaning: SymbolFlags,
    ) -> Result<Option<SymbolId>, Error> {
        if let Hook::Value(value) = hooks.lookup(table, name, meaning)? {
            return Ok(value);
        }
        if meaning != 0 {
            if let Some(symbol) = table_entry(host, table, name)? {
                if host.symbol(symbol)?.flags() & meaning != 0 {
                    return Ok(Some(symbol));
                }
            }
        }
        Ok(None)
    }
    /// port: tsc/internal/binder/nameresolver.go:NameResolver.argumentsSymbol
    fn arguments_symbol(&mut self, host: &mut dyn ResolverHost) -> Result<SymbolId, Error> {
        if let Some(symbol) = self.arguments_symbol {
            return Ok(symbol);
        }
        let symbol = host.new_transient_symbol(
            flags::PROPERTY | flags::TRANSIENT,
            JsString::from_bytes(b"arguments".as_slice()),
        )?;
        self.arguments_symbol = Some(symbol);
        Ok(symbol)
    }
}

struct ScopeVisitor<'a> {
    resolver: &'a NameResolver,
    host: &'a dyn ResolverHost,
    view: AstView<'a>,
    result: Result<bool, Error>,
}
impl ChildVisitor for ScopeVisitor<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.result = self.resolver.requires_scope_change_worker(self.host, node);
        if matches!(self.result, Ok(false)) {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(())
        }
    }
    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        match self.view.list(list) {
            Ok(list) => self.visit_node_slice(list.nodes()),
            Err(error) => {
                self.result = Err(error);
                ControlFlow::Break(())
            }
        }
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        match self.view.node_slice(nodes) {
            Ok(nodes) => {
                for node in nodes.iter().flatten() {
                    self.visit_node(node)?;
                }
                ControlFlow::Continue(())
            }
            Err(error) => {
                self.result = Err(error);
                ControlFlow::Break(())
            }
        }
    }
}

/// port: tsc/internal/binder/nameresolver.go:GetLocalSymbolForExportDefault
pub fn get_local_symbol_for_export_default(
    host: &dyn ResolverHost,
    symbol: Option<SymbolId>,
) -> Result<Option<SymbolId>, Error> {
    if !is_export_default_symbol(host, symbol)? {
        return Ok(None);
    }
    for declaration in host.declarations(required_symbol(symbol))?.iter() {
        if let Some(local) = host
            .binding(required(declaration))?
            .and_then(|b| b.local_symbol)
        {
            return Ok(Some(local));
        }
    }
    Ok(None)
}
/// port: tsc/internal/binder/nameresolver.go:isExportDefaultSymbol
fn is_export_default_symbol(
    host: &dyn ResolverHost,
    symbol: Option<SymbolId>,
) -> Result<bool, Error> {
    let Some(symbol) = symbol else {
        return Ok(false);
    };
    let Some(first) = host.declarations(symbol)?.first() else {
        return Ok(false);
    };
    let first = required(first);
    Ok(host.node(first)?.modifier_flags(host.ast(first)?)? & modifiers::DEFAULT != 0)
}
/// port: tsc/internal/binder/nameresolver.go:getIsDeferredContext
fn get_is_deferred_context(
    host: &dyn ResolverHost,
    location: NodeId,
    last: Option<NodeId>,
) -> Result<bool, Error> {
    let node = host.node(location)?;
    if !matches!(
        node.kind().known(),
        Some(K::ArrowFunction | K::FunctionExpression)
    ) {
        return Ok(node.kind() == K::TypeQuery
            || (u::is_function_like_declaration(Some(&node))
                || node.kind() == K::PropertyDeclaration
                    && !u::is_static(host.ast(location)?, location)?)
                && (last.is_none() || last != node.name()));
    }
    if last.is_some() && last == node.name() {
        return Ok(false);
    }
    let asterisk = match node.data() {
        NodeDataRead::ArrowFunction(data) => data.asterisk_token(),
        NodeDataRead::FunctionExpression(data) => data.asterisk_token(),
        _ => unreachable!(),
    };
    if asterisk.is_some() || node.modifier_flags(host.ast(location)?)? & modifiers::ASYNC != 0 {
        return Ok(true);
    }
    Ok(
        ts_ast::get_immediately_invoked_function_expression(host.ast(location)?, location)?
            .is_none(),
    )
}
/// port: tsc/internal/binder/nameresolver.go:isTypeParameterSymbolDeclaredInContainer
fn is_type_parameter_symbol_declared_in_container(
    host: &dyn ResolverHost,
    symbol: SymbolId,
    container: NodeId,
) -> Result<bool, Error> {
    for declaration in host.declarations(symbol)?.iter() {
        let declaration = required(declaration);
        if kind(host, declaration)? == K::TypeParameter
            && host.node(declaration)?.parent() == Some(container)
        {
            return Ok(true);
        }
    }
    Ok(false)
}
/// port: tsc/internal/binder/nameresolver.go:isSelfReferenceLocation
fn is_self_reference_location(
    host: &dyn ResolverHost,
    node: NodeId,
    last: Option<NodeId>,
) -> Result<bool, Error> {
    Ok(match kind(host, node)?.known() {
        Some(K::Parameter) => last.is_some() && last == host.node(node)?.name(),
        Some(
            K::FunctionDeclaration
            | K::ClassDeclaration
            | K::InterfaceDeclaration
            | K::EnumDeclaration
            | K::TypeAliasDeclaration
            | K::JSTypeAliasDeclaration
            | K::ModuleDeclaration,
        ) => true,
        _ => false,
    })
}
pub(crate) fn required(node: Option<NodeId>) -> NodeId {
    node.expect("runtime error: invalid memory address or nil pointer dereference")
}
pub(crate) fn required_symbol(symbol: Option<SymbolId>) -> SymbolId {
    symbol.expect("runtime error: invalid memory address or nil pointer dereference")
}
pub(crate) fn kind(host: &dyn ResolverHost, node: NodeId) -> Result<NodeKind, Error> {
    Ok(host.node(node)?.kind())
}
pub(crate) fn node_symbol(
    host: &dyn ResolverHost,
    node: NodeId,
) -> Result<Option<SymbolId>, Error> {
    Ok(host.binding(node)?.and_then(|b| b.symbol))
}
pub(crate) fn source_file(
    host: &dyn ResolverHost,
    node: Option<NodeId>,
) -> Result<Option<NodeId>, Error> {
    node.map(|id| u::get_source_file_of_node(host.ast(id)?, Some(id)))
        .transpose()
        .map(Option::flatten)
}
fn node_name_equals(host: &dyn ResolverHost, node: NodeId, name: &[u8]) -> Result<bool, Error> {
    host.node(node)?
        .name()
        .map(|id| Ok(host.ast(id)?.node_text(id)?.as_bytes() == name))
        .unwrap_or(Ok(false))
}
fn table_entry(
    host: &dyn ResolverHost,
    table: Option<SymbolTableId>,
    name: &[u8],
) -> Result<Option<SymbolId>, Error> {
    let Some(table) = table else {
        return Ok(None);
    };
    Ok(host.table(table)?.get(name).flatten())
}
fn declaration_of_kind(
    host: &dyn ResolverHost,
    symbol: SymbolId,
    expected: K,
) -> Result<Option<NodeId>, Error> {
    for declaration in host.declarations(symbol)?.iter() {
        let id = required(declaration);
        if kind(host, id)? == expected {
            return Ok(Some(id));
        }
    }
    Ok(None)
}
fn ancestor_kind(
    host: &dyn ResolverHost,
    mut node: Option<NodeId>,
    expected: K,
) -> Result<Option<NodeId>, Error> {
    while let Some(id) = node {
        if kind(host, id)? == expected {
            return Ok(Some(id));
        }
        node = host.node(id)?.parent();
    }
    Ok(None)
}

#[cfg(test)]
#[path = "resolver_tests.rs"]
mod tests;
