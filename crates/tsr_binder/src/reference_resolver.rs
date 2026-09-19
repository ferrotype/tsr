//! Reference lookup preserves optional-hook precedence and alias/value rules.
use crate::name_resolver::{
    kind, node_symbol, required, required_symbol, source_file, Hook, NameResolver,
    NoNameResolverHooks, ResolverHost, ResolverOptions,
};
use ts_arena::{Error, SymbolId};
use ts_ast::{symbol_flags as flags, JsString, NodeId, SymbolFlags, SyntaxKind as K};
use ts_diagnostics::Message;

/// Optional checker callbacks. Returned identities belong to ResolverHost;
/// Hook::Value(None) is authoritative where the source callback returns nil.
pub trait ReferenceResolverHooks {
    #[allow(clippy::too_many_arguments)]
    fn resolve_name(
        &mut self,
        _location: Option<NodeId>,
        _name: &[u8],
        _meaning: SymbolFlags,
        _message: Option<&'static Message>,
        _is_use: bool,
        _exclude_globals: bool,
    ) -> Result<Hook<Option<SymbolId>>, Error> {
        Ok(Hook::Absent)
    }
    fn get_resolved_symbol(&mut self, _node: NodeId) -> Result<Hook<Option<SymbolId>>, Error> {
        Ok(Hook::Absent)
    }
    fn get_merged_symbol(&mut self, _symbol: SymbolId) -> Result<Hook<Option<SymbolId>>, Error> {
        Ok(Hook::Absent)
    }
    fn get_parent_of_symbol(&mut self, _symbol: SymbolId) -> Result<Hook<Option<SymbolId>>, Error> {
        Ok(Hook::Absent)
    }
    fn get_symbol_of_declaration(
        &mut self,
        _node: NodeId,
    ) -> Result<Hook<Option<SymbolId>>, Error> {
        Ok(Hook::Absent)
    }
    fn get_type_only_alias_declaration(
        &mut self,
        _symbol: SymbolId,
        _include: SymbolFlags,
    ) -> Result<Hook<Option<NodeId>>, Error> {
        Ok(Hook::Absent)
    }
    fn get_export_symbol_of_value_symbol_if_exported(
        &mut self,
        _symbol: SymbolId,
    ) -> Result<Hook<Option<SymbolId>>, Error> {
        Ok(Hook::Absent)
    }
    /// Value(None) preserves a present source hook returning ok=false.
    fn get_element_access_expression_name(
        &mut self,
        _expression: NodeId,
    ) -> Result<Hook<Option<JsString>>, Error> {
        Ok(Hook::Absent)
    }
}
pub struct NoReferenceResolverHooks;
impl ReferenceResolverHooks for NoReferenceResolverHooks {}

pub struct ReferenceResolver {
    resolver: Option<NameResolver>,
    options: ResolverOptions,
}
impl ReferenceResolver {
    /// port: tsc/internal/binder/referenceresolver.go:NewReferenceResolver
    pub fn new(options: ResolverOptions) -> Self {
        Self {
            resolver: None,
            options,
        }
    }

    /// port: tsc/internal/binder/referenceresolver.go:referenceResolver.getResolvedSymbol
    fn get_resolved_symbol(
        hooks: &mut dyn ReferenceResolverHooks,
        node: Option<NodeId>,
    ) -> Result<Option<SymbolId>, Error> {
        if let Some(node) = node {
            if let Hook::Value(symbol) = hooks.get_resolved_symbol(node)? {
                return Ok(symbol);
            }
        }
        Ok(None)
    }
    /// port: tsc/internal/binder/referenceresolver.go:referenceResolver.getMergedSymbol
    fn get_merged_symbol(
        hooks: &mut dyn ReferenceResolverHooks,
        symbol: Option<SymbolId>,
    ) -> Result<Option<SymbolId>, Error> {
        let Some(symbol) = symbol else {
            return Ok(None);
        };
        match hooks.get_merged_symbol(symbol)? {
            Hook::Absent => Ok(Some(symbol)),
            Hook::Value(value) => Ok(value),
        }
    }
    /// port: tsc/internal/binder/referenceresolver.go:referenceResolver.getParentOfSymbol
    fn get_parent_of_symbol(
        host: &dyn ResolverHost,
        hooks: &mut dyn ReferenceResolverHooks,
        symbol: Option<SymbolId>,
    ) -> Result<Option<SymbolId>, Error> {
        let Some(symbol) = symbol else {
            return Ok(None);
        };
        match hooks.get_parent_of_symbol(symbol)? {
            Hook::Absent => Ok(host.symbol(symbol)?.parent()),
            Hook::Value(value) => Ok(value),
        }
    }
    /// port: tsc/internal/binder/referenceresolver.go:referenceResolver.getSymbolOfDeclaration
    fn get_symbol_of_declaration(
        host: &dyn ResolverHost,
        hooks: &mut dyn ReferenceResolverHooks,
        node: Option<NodeId>,
    ) -> Result<Option<SymbolId>, Error> {
        let Some(node) = node else {
            return Ok(None);
        };
        match hooks.get_symbol_of_declaration(node)? {
            Hook::Absent => node_symbol(host, node),
            Hook::Value(value) => Ok(value),
        }
    }
    /// port: tsc/internal/binder/referenceresolver.go:referenceResolver.getReferencedValueSymbol
    fn get_referenced_value_symbol(
        &mut self,
        host: &mut dyn ResolverHost,
        hooks: &mut dyn ReferenceResolverHooks,
        reference: NodeId,
        start_in_declaration_container: bool,
    ) -> Result<Option<SymbolId>, Error> {
        if let Some(symbol) = Self::get_resolved_symbol(hooks, Some(reference))? {
            return Ok(Some(symbol));
        }
        let mut location = Some(reference);
        if start_in_declaration_container {
            if let Some(parent) = host.node(reference)?.parent() {
                if ts_ast::is_declaration(&host.node(parent)?)
                    && host.node(parent)?.name() == Some(reference)
                {
                    location = ts_ast::get_declaration_container(host.ast(parent)?, parent)?;
                }
            }
        }
        let name = host.ast(reference)?.node_text(reference)?;
        let meaning = flags::EXPORT_VALUE | flags::VALUE | flags::ALIAS;
        if let Hook::Value(value) =
            hooks.resolve_name(location, name.as_bytes(), meaning, None, false, false)?
        {
            return Ok(value);
        }
        // The fallback can allocate `arguments` in the mutable host. Retain the
        // text backing once across that call; a supplied ResolveName hook uses
        // the borrowed text above and performs no reference-count operation.
        let name = name.into_js_string();
        let resolver = self
            .resolver
            .get_or_insert_with(|| NameResolver::new(self.options));
        resolver.resolve(
            host,
            &mut NoNameResolverHooks,
            location,
            name.as_bytes(),
            meaning,
            None,
            false,
            false,
        )
    }
    /// port: tsc/internal/binder/referenceresolver.go:referenceResolver.isTypeOnlyAliasDeclaration
    fn is_type_only_alias_declaration(
        host: &dyn ResolverHost,
        hooks: &mut dyn ReferenceResolverHooks,
        symbol: Option<SymbolId>,
    ) -> Result<bool, Error> {
        let Some(symbol) = symbol else {
            return Ok(false);
        };
        if let Hook::Value(declaration) =
            hooks.get_type_only_alias_declaration(symbol, flags::VALUE)?
        {
            return Ok(declaration.is_some());
        }
        let mut node = Self::get_declaration_of_alias_symbol(host, symbol)?;
        while let Some(id) = node {
            match kind(host, id)?.known() {
                Some(K::ImportEqualsDeclaration | K::ExportDeclaration) => {
                    return Ok(host.node(id)?.is_type_only())
                }
                Some(K::ImportClause | K::ImportSpecifier | K::ExportSpecifier) => {
                    if host.node(id)?.is_type_only() {
                        return Ok(true);
                    }
                    node = host.node(id)?.parent();
                }
                Some(K::NamedImports | K::NamedExports) => node = host.node(id)?.parent(),
                _ => break,
            }
        }
        Ok(false)
    }
    /// port: tsc/internal/binder/referenceresolver.go:referenceResolver.getDeclarationOfAliasSymbol
    fn get_declaration_of_alias_symbol(
        host: &dyn ResolverHost,
        symbol: SymbolId,
    ) -> Result<Option<NodeId>, Error> {
        for declaration in host.declarations(symbol)?.iter().rev() {
            let node = required(declaration);
            if ts_ast::is_alias_symbol_declaration(host.ast(node)?, node)? {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }
    /// port: tsc/internal/binder/referenceresolver.go:referenceResolver.getExportSymbolOfValueSymbolIfExported
    fn get_export_symbol_of_value_symbol_if_exported(
        host: &dyn ResolverHost,
        hooks: &mut dyn ReferenceResolverHooks,
        symbol: Option<SymbolId>,
    ) -> Result<Option<SymbolId>, Error> {
        let Some(mut symbol) = symbol else {
            return Ok(None);
        };
        if let Hook::Value(value) = hooks.get_export_symbol_of_value_symbol_if_exported(symbol)? {
            return Ok(value);
        }
        let data = host.symbol(symbol)?;
        if data.flags() & flags::EXPORT_VALUE != 0 {
            if let Some(exported) = data.export_symbol() {
                symbol = exported;
            }
        }
        Self::get_merged_symbol(hooks, Some(symbol))
    }
    /// port: tsc/internal/binder/referenceresolver.go:referenceResolver.GetReferencedExportContainer
    pub fn get_referenced_export_container(
        &mut self,
        host: &mut dyn ResolverHost,
        hooks: &mut dyn ReferenceResolverHooks,
        node: NodeId,
        prefix_locals: bool,
    ) -> Result<Option<NodeId>, Error> {
        let parent = host.node(node)?.parent();
        let start_in_container = match parent {
            Some(parent) => {
                matches!(
                    kind(host, parent)?.known(),
                    Some(K::ModuleDeclaration | K::EnumDeclaration)
                ) && host.node(parent)?.name() == Some(node)
            }
            None => false,
        };
        if let Some(mut symbol) =
            self.get_referenced_value_symbol(host, hooks, node, start_in_container)?
        {
            if host.symbol(symbol)?.flags() & flags::EXPORT_VALUE != 0 {
                let exported =
                    Self::get_merged_symbol(hooks, host.symbol(symbol)?.export_symbol())?;
                // The source only dereferences the optional merged export when
                // prefixLocals is false. A present hook may return nil.
                if !prefix_locals {
                    let exported_flags = host.symbol(required_symbol(exported))?.flags();
                    if exported_flags & flags::EXPORT_HAS_LOCAL != 0
                        && exported_flags & flags::VARIABLE == 0
                    {
                        return Ok(None);
                    }
                }
                let Some(exported) = exported else {
                    return Ok(None);
                };
                symbol = exported;
            }
            if let Some(parent_symbol) = Self::get_parent_of_symbol(host, hooks, Some(symbol))? {
                let parent_data = host.symbol(parent_symbol)?;
                if parent_data.flags() & flags::VALUE_MODULE != 0 {
                    if let Some(declaration) = parent_data.value_declaration() {
                        if kind(host, declaration)? == K::SourceFile {
                            return Ok((Some(declaration) == source_file(host, Some(node))?)
                                .then_some(declaration));
                        }
                    }
                }
                let mut ancestor = parent;
                while let Some(id) = ancestor {
                    if matches!(
                        kind(host, id)?.known(),
                        Some(K::ModuleDeclaration | K::EnumDeclaration)
                    ) && Self::get_symbol_of_declaration(host, hooks, Some(id))?
                        == Some(parent_symbol)
                    {
                        return Ok(Some(id));
                    }
                    ancestor = host.node(id)?.parent();
                }
            }
        }
        Ok(None)
    }
    /// port: tsc/internal/binder/referenceresolver.go:referenceResolver.GetReferencedImportDeclaration
    pub fn get_referenced_import_declaration(
        &mut self,
        host: &mut dyn ResolverHost,
        hooks: &mut dyn ReferenceResolverHooks,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        if let Some(symbol) = self.get_referenced_value_symbol(host, hooks, node, false)? {
            if ts_ast::is_non_local_alias(Some(&host.symbol(symbol)?), flags::VALUE)
                && !Self::is_type_only_alias_declaration(host, hooks, Some(symbol))?
            {
                return Self::get_declaration_of_alias_symbol(host, symbol);
            }
        }
        Ok(None)
    }
    /// port: tsc/internal/binder/referenceresolver.go:referenceResolver.GetReferencedValueDeclaration
    pub fn get_referenced_value_declaration(
        &mut self,
        host: &mut dyn ResolverHost,
        hooks: &mut dyn ReferenceResolverHooks,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        if let Some(symbol) = self.get_referenced_value_symbol(host, hooks, node, false)? {
            let symbol = required_symbol(Self::get_export_symbol_of_value_symbol_if_exported(
                host,
                hooks,
                Some(symbol),
            )?);
            return Ok(host.symbol(symbol)?.value_declaration());
        }
        Ok(None)
    }
    /// The source returns nil if no qualifying declaration is appended.
    /// port: tsc/internal/binder/referenceresolver.go:referenceResolver.GetReferencedValueDeclarations
    pub fn get_referenced_value_declarations(
        &mut self,
        host: &mut dyn ResolverHost,
        hooks: &mut dyn ReferenceResolverHooks,
        node: NodeId,
    ) -> Result<Option<Vec<NodeId>>, Error> {
        let mut declarations = None;
        if let Some(symbol) = self.get_referenced_value_symbol(host, hooks, node, false)? {
            let symbol = required_symbol(Self::get_export_symbol_of_value_symbol_if_exported(
                host,
                hooks,
                Some(symbol),
            )?);
            for declaration in host.declarations(symbol)?.iter() {
                let id = required(declaration);
                if matches!(
                    kind(host, id)?.known(),
                    Some(
                        K::VariableDeclaration
                            | K::Parameter
                            | K::BindingElement
                            | K::PropertyDeclaration
                            | K::PropertyAssignment
                            | K::ShorthandPropertyAssignment
                            | K::EnumMember
                            | K::ObjectLiteralExpression
                            | K::FunctionDeclaration
                            | K::FunctionExpression
                            | K::ArrowFunction
                            | K::ClassDeclaration
                            | K::ClassExpression
                            | K::EnumDeclaration
                            | K::MethodDeclaration
                            | K::GetAccessor
                            | K::SetAccessor
                            | K::ModuleDeclaration
                    )
                ) {
                    declarations.get_or_insert_with(Vec::new).push(id);
                }
            }
        }
        Ok(declarations)
    }
    /// port: tsc/internal/binder/referenceresolver.go:referenceResolver.GetElementAccessExpressionName
    pub fn get_element_access_expression_name(
        &self,
        hooks: &mut dyn ReferenceResolverHooks,
        expression: Option<NodeId>,
    ) -> Result<JsString, Error> {
        if let Some(expression) = expression {
            if let Hook::Value(Some(name)) = hooks.get_element_access_expression_name(expression)? {
                return Ok(name);
            }
        }
        Ok(JsString::default())
    }
    /// port: tsc/internal/binder/referenceresolver.go:referenceResolver.GetReferencedMemberValueDeclaration
    pub fn get_referenced_member_value_declaration(
        &self,
        host: &dyn ResolverHost,
        hooks: &mut dyn ReferenceResolverHooks,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let mut symbol = Self::get_resolved_symbol(hooks, Some(node))?;
        if symbol.is_none() {
            if let Some(declared) = node_symbol(host, node)? {
                symbol = Self::get_merged_symbol(hooks, Some(declared))?;
            }
        }
        let Some(symbol) = symbol else {
            return Ok(None);
        };
        let symbol = required_symbol(Self::get_export_symbol_of_value_symbol_if_exported(
            host,
            hooks,
            Some(symbol),
        )?);
        Ok(host.symbol(symbol)?.value_declaration())
    }
}
