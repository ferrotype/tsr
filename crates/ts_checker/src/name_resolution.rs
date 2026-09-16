//! The binder's lexical walk with checker lookup and side effects. The resolver
//! is scoped to one operation; cached checker links are updated after the walk.

use crate::{CheckerState, Error};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    symbol_flags as sf, AstView, DeclarationRead, JsString, NodeBinding, SymbolFlags, SymbolRef,
    SymbolTableId, SymbolTableRead,
};
use ts_binder::name_resolver::{
    Hook, NameResolver, NameResolverHooks, ResolvedName, ResolverHost, ResolverOptions,
};
use ts_core::Tristate;
use ts_diagnostics::Message;

struct ReadHost<'a>(&'a CheckerState);
fn arena(error: Error) -> ts_arena::Error {
    match error {
        Error::Arena(error) => error,
        _ => ts_arena::Error::InvalidGraph,
    }
}
impl ResolverHost for ReadHost<'_> {
    fn ast(&self, node: NodeId) -> Result<AstView<'_>, ts_arena::Error> {
        self.0.ast(node).map_err(arena)
    }
    fn binding(&self, node: NodeId) -> Result<Option<NodeBinding>, ts_arena::Error> {
        self.0.checker_node_binding(node).map_err(arena)
    }
    fn symbol(&self, id: SymbolId) -> Result<SymbolRef<'_>, ts_arena::Error> {
        self.0.symbol(id).map_err(arena)
    }
    fn table(&self, id: SymbolTableId) -> Result<SymbolTableRead<'_>, ts_arena::Error> {
        self.0.table(id).map_err(arena)
    }
    fn declarations(&self, id: SymbolId) -> Result<DeclarationRead<'_>, ts_arena::Error> {
        self.0.symbol_declarations(id).map_err(arena)
    }
    fn new_transient_symbol(
        &mut self,
        _: SymbolFlags,
        _: JsString,
    ) -> Result<SymbolId, ts_arena::Error> {
        // The binder creates only a missing arguments symbol. NewChecker
        // always installs its own before this resolver can run.
        unreachable!("initialized checker resolver already has argumentsSymbol")
    }
}

enum Effect {
    Error(Option<NodeId>, &'static Message, Vec<JsString>),
    Referenced(SymbolId, SymbolFlags),
    Scope(NodeId, Tristate),
    Resolved(ResolvedName),
    Failed(Option<NodeId>, JsString, SymbolFlags, &'static Message),
    InvalidInitializer(Option<NodeId>, JsString, NodeId, Option<SymbolId>),
}
enum PendingAlias {
    Lookup(SymbolId),
    SpellingCandidate(SymbolId),
}
struct Hooks<'a> {
    state: &'a CheckerState,
    effects: Vec<Effect>,
    failure: Option<Error>,
    alias_flags: &'a crate::types::Map<SymbolId, SymbolFlags>,
    spelling_alias_flags: &'a crate::types::Map<SymbolId, SymbolFlags>,
    pending_alias: Option<PendingAlias>,
    suggestion: bool,
}
impl Hooks<'_> {
    fn suggest_lookup(
        &mut self,
        table: Option<SymbolTableId>,
        name: &[u8],
        meaning: SymbolFlags,
    ) -> Result<Option<SymbolId>, ts_arena::Error> {
        let result = (|| {
            let Some(table) = table else { return Ok(None) };
            let table_id = table;
            let table = self.state.table(table_id)?;
            let mut candidates = Vec::new();
            for (_, candidate) in table {
                let Some(candidate) = candidate else { continue };
                let read = self.state.symbol(candidate)?;
                let text = read.name_to_owned();
                if text.is_empty() || matches!(text.as_bytes()[0], b'"' | 0xfe) {
                    continue;
                }
                let mut flags = read.flags();
                if flags & meaning == 0 && flags & sf::ALIAS != 0 {
                    // port: tsc/internal/checker/checker.go:Checker.getSpellingSuggestionForName
                    // Suggestions use the first resolved target's flags, not
                    // getSymbolFlags' accumulated meanings (or ALL for unknown).
                    if let Some(&known) = self.spelling_alias_flags.get(&candidate) {
                        flags = known;
                    } else if let Some(&target) = self.state.module_aliases.targets.get(&candidate)
                    {
                        flags = self.state.symbol(target?)?.flags();
                    } else {
                        self.pending_alias = Some(PendingAlias::SpellingCandidate(candidate));
                        return Err(Error::Unsupported("getSymbolFlags: alias resolution"));
                    }
                }
                if flags & meaning != 0 {
                    candidates.push((text, candidate));
                }
            }
            if meaning & sf::GLOBAL_LOOKUP != 0 {
                // port: tsc/internal/checker/checker.go:getPrimitiveTypeAliasSuggestions
                let table = self.state.table(table_id)?;
                for &(builtin, suggestion) in &self.state.builtins.primitive_alias_suggestions {
                    if table.get(builtin).is_some() {
                        let text = self.state.symbol(suggestion)?.name_to_owned();
                        candidates.push((text, suggestion));
                    }
                }
            }
            let failure = std::cell::Cell::new(None);
            let result = ts_scanner::get_spelling_suggestion(
                name,
                candidates.iter(),
                |entry| entry.0.as_bytes(),
                |a, b| match self.state.compare_symbols(Some(a.1), Some(b.1)) {
                    Ok(order) => order,
                    Err(error) => {
                        failure.set(Some(error));
                        std::cmp::Ordering::Equal
                    }
                },
                0,
            )
            .map(|entry| entry.1);
            if let Some(error) = failure.get() {
                return Err(error);
            }
            Ok(result)
        })();
        self.capture(result)
    }
    fn capture<T>(&mut self, result: Result<T, Error>) -> Result<T, ts_arena::Error> {
        result.map_err(|error| {
            self.failure = Some(error);
            arena(error)
        })
    }
}
impl NameResolverHooks for Hooks<'_> {
    fn get_symbol_of_declaration(
        &mut self,
        node: NodeId,
    ) -> Result<Hook<Option<SymbolId>>, ts_arena::Error> {
        let result = (|| {
            let Some(raw) = self.state.raw_declaration_symbol(node)? else {
                return Ok(Hook::Value(None));
            };
            let symbol = self.state.symbol(raw)?;
            let symbol = if symbol.flags() & sf::CLASS_MEMBER != 0
                && symbol.name_bytes() == ts_ast::internal_symbol_names::COMPUTED
            {
                self.state
                    .late_members
                    .symbols
                    .try_get(raw)
                    .copied()
                    .flatten()
                    .ok_or(Error::Unsupported(
                        "createNameResolver: unresolved late enclosing symbol",
                    ))?
            } else {
                raw
            };
            Ok(Hook::Value(Some(self.state.get_merged_symbol(symbol))))
        })();
        self.capture(result)
    }
    fn lookup(
        &mut self,
        table: Option<SymbolTableId>,
        name: &[u8],
        meaning: SymbolFlags,
    ) -> Result<Hook<Option<SymbolId>>, ts_arena::Error> {
        let result = self.state.lookup_symbol(table, name, meaning);
        if matches!(
            result,
            Err(Error::Unsupported("getSymbolFlags: alias resolution"))
        ) {
            if let Some(table) = table {
                if let Some(symbol) = self.state.table(table).map_err(arena)?.get(name).flatten() {
                    let symbol = self.state.get_merged_symbol(symbol);
                    if let Some(&flags) = self.alias_flags.get(&symbol) {
                        if flags & meaning == 0 && self.suggestion {
                            return self
                                .suggest_lookup(Some(table), name, meaning)
                                .map(Hook::Value);
                        }
                        return Ok(Hook::Value((flags & meaning != 0).then_some(symbol)));
                    }
                    self.pending_alias = Some(PendingAlias::Lookup(symbol));
                }
            }
        }
        if self.suggestion && matches!(result, Ok(None)) {
            return self.suggest_lookup(table, name, meaning).map(Hook::Value);
        }
        self.capture(result.map(Hook::Value))
    }
    fn error(
        &mut self,
        location: Option<NodeId>,
        message: &'static Message,
        args: &[JsString],
    ) -> Result<(), ts_arena::Error> {
        self.effects
            .push(Effect::Error(location, message, args.to_vec()));
        Ok(())
    }
    fn symbol_referenced(
        &mut self,
        symbol: SymbolId,
        meaning: SymbolFlags,
    ) -> Result<(), ts_arena::Error> {
        self.effects.push(Effect::Referenced(symbol, meaning));
        Ok(())
    }
    fn get_requires_scope_change_cache(
        &mut self,
        node: NodeId,
    ) -> Result<Tristate, ts_arena::Error> {
        for effect in self.effects.iter().rev() {
            if let Effect::Scope(id, value) = effect {
                if *id == node {
                    return Ok(*value);
                }
            }
        }
        Ok(self
            .state
            .query
            .scope_changes
            .try_get(node)
            .copied()
            .unwrap_or(Tristate::UNKNOWN))
    }
    fn set_requires_scope_change_cache(
        &mut self,
        node: NodeId,
        value: Tristate,
    ) -> Result<(), ts_arena::Error> {
        self.effects.push(Effect::Scope(node, value));
        Ok(())
    }
    fn on_property_with_invalid_initializer(
        &mut self,
        location: Option<NodeId>,
        name: &[u8],
        property: NodeId,
        result: Option<SymbolId>,
    ) -> Result<bool, ts_arena::Error> {
        if self.suggestion {
            return Ok(false);
        }
        if self.capture(
            self.state
                .program()
                .map(|program| program.host.options().emit_standard_class_fields()),
        )? {
            return Ok(false);
        }
        self.effects.push(Effect::InvalidInitializer(
            location,
            JsString::from_bytes(name),
            property,
            result,
        ));
        Ok(true)
    }
    fn on_failed_to_resolve_symbol(
        &mut self,
        location: Option<NodeId>,
        name: &[u8],
        meaning: SymbolFlags,
        message: &'static Message,
    ) -> Result<(), ts_arena::Error> {
        self.effects.push(Effect::Failed(
            location,
            JsString::from_bytes(name),
            meaning,
            message,
        ));
        Ok(())
    }

    fn on_successfully_resolved_symbol(
        &mut self,
        resolution: ResolvedName,
    ) -> Result<(), ts_arena::Error> {
        if !self.suggestion {
            self.effects.push(Effect::Resolved(resolution));
        }
        Ok(())
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.resolveEntityName
    // port: tsc/internal/checker/checker.go:Checker.resolveQualifiedName
    pub(crate) fn resolve_entity_name(
        &mut self,
        name: NodeId,
        meaning: SymbolFlags,
        ignore_errors: bool,
    ) -> Result<Option<SymbolId>, Error> {
        self.resolve_entity_name_ex(name, meaning, ignore_errors, false)
    }

    pub(crate) fn resolve_entity_name_ex(
        &mut self,
        name: NodeId,
        meaning: SymbolFlags,
        ignore_errors: bool,
        dont_resolve_alias: bool,
    ) -> Result<Option<SymbolId>, Error> {
        self.resolve_entity_name_at(name, meaning, ignore_errors, dont_resolve_alias, None)
    }

    pub(crate) fn resolve_entity_name_at(
        &mut self,
        name: NodeId,
        meaning: SymbolFlags,
        ignore_errors: bool,
        dont_resolve_alias: bool,
        location: Option<NodeId>,
    ) -> Result<Option<SymbolId>, Error> {
        use ts_ast::{internal_symbol_names as names, SyntaxKind as K};
        let read = self.ast(name)?.node(name)?;
        if read.pos() == read.end() {
            return Ok(None);
        }
        let symbol = if read.kind() == K::Identifier {
            let text = self.ast(name)?.node_text(name)?.into_js_string();
            let message = if meaning == sf::NAMESPACE || read.pos() < 0 {
                ts_diagnostics::Cannot_find_namespace_0
            } else {
                self.cannot_find_name_diagnostic(name)?
            };
            let symbol = self.resolve_name(
                Some(location.unwrap_or(name)),
                text.as_bytes(),
                meaning,
                if meaning == sf::NAMESPACE {
                    None
                } else {
                    (!ignore_errors).then_some(message)
                },
                true,
            )?;
            if symbol.is_none() && meaning == sf::NAMESPACE {
                if let Some(alias) = self.resolve_name(
                    Some(location.unwrap_or(name)),
                    text.as_bytes(),
                    sf::ALIAS,
                    None,
                    true,
                )? {
                    if self.symbol(alias)?.name_bytes() == names::EXPORT_EQUALS {
                        return Ok(self.symbol(alias)?.parent());
                    }
                }
                if !ignore_errors {
                    return self.resolve_name(
                        Some(location.unwrap_or(name)),
                        text.as_bytes(),
                        meaning,
                        Some(message),
                        true,
                    );
                }
            }
            symbol
        } else {
            let (left, right) = match read.kind().known() {
                Some(K::QualifiedName) => {
                    let data = read
                        .data_source()
                        .as_qualified_name()
                        .ok_or(Error::MissingLink("qualified name"))?;
                    (data.left(), data.right())
                }
                Some(K::PropertyAccessExpression) => {
                    let data = read
                        .data_source()
                        .as_property_access_expression()
                        .ok_or(Error::MissingLink("qualified property access"))?;
                    (data.expression(), read.name())
                }
                _ => {
                    return Err(Error::Unsupported(
                        "resolveEntityName: unsupported entity syntax",
                    ))
                }
            };
            let left = left.ok_or(Error::MissingLink("qualified name left"))?;
            let right = right.ok_or(Error::MissingLink("qualified name right"))?;
            let Some(namespace) =
                self.resolve_entity_name_at(left, sf::NAMESPACE, ignore_errors, false, location)?
            else {
                return Ok(None);
            };
            if namespace == self.builtins.unknown_symbol {
                return Ok(Some(namespace));
            }
            if self.ast(right)?.node(right)?.pos() == self.ast(right)?.node(right)?.end() {
                return Ok(None);
            }
            let namespace = self.resolve_common_js_namespace(namespace)?;
            let exports = self.module_exports_of_symbol(namespace)?;
            let text = self.ast(right)?.node_text(right)?.into_js_string();
            let mut symbol = self.lookup_symbol_resolving(exports, text.as_bytes(), meaning)?;
            if symbol.is_none() && self.symbol(namespace)?.flags() & sf::ALIAS != 0 {
                let target = self.resolve_alias(namespace)?;
                let exports = self.module_exports_of_symbol(target)?;
                symbol = self.lookup_symbol_resolving(exports, text.as_bytes(), meaning)?;
            }
            if symbol.is_none() && !ignore_errors {
                self.report_missing_qualified_name(name, right, namespace, meaning)?;
            }
            symbol
        };
        if let Some(symbol) = symbol {
            let symbol = self.get_merged_symbol(symbol);
            if self.symbol(symbol)?.flags() & sf::ALIAS != 0
                && self.symbol(symbol)?.flags() & meaning == 0
                && !dont_resolve_alias
            {
                return self.resolve_alias(symbol).map(Some);
            }
            return Ok(Some(symbol));
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getSymbol
    pub(crate) fn lookup_symbol(
        &self,
        table: Option<SymbolTableId>,
        name: &[u8],
        meaning: SymbolFlags,
    ) -> Result<Option<SymbolId>, Error> {
        if meaning & sf::ALL == 0 {
            return Ok(None);
        }
        let Some(table) = table else { return Ok(None) };
        let Some(symbol) = self.table(table)?.get(name).flatten() else {
            return Ok(None);
        };
        let symbol = self.get_merged_symbol(symbol);
        let flags = self.symbol(symbol)?.flags();
        if flags & meaning != 0 {
            return Ok(Some(symbol));
        }
        if flags & sf::ALIAS != 0 {
            if let Some(flags) = self.cached_module_symbol_flags(symbol)? {
                return Ok((flags & meaning != 0).then_some(symbol));
            }
            return Err(Error::Unsupported("getSymbolFlags: alias resolution"));
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.createNameResolver
    pub(crate) fn resolve_name(
        &mut self,
        location: Option<NodeId>,
        name: &[u8],
        meaning: SymbolFlags,
        message: Option<&'static Message>,
        is_use: bool,
    ) -> Result<Option<SymbolId>, Error> {
        self.resolve_name_ex(location, name, meaning, message, is_use, false)
    }

    pub(crate) fn resolve_name_ex(
        &mut self,
        location: Option<NodeId>,
        name: &[u8],
        meaning: SymbolFlags,
        message: Option<&'static Message>,
        is_use: bool,
        exclude_globals: bool,
    ) -> Result<Option<SymbolId>, Error> {
        self.resolve_name_mode(
            location,
            name,
            meaning,
            message,
            is_use,
            exclude_globals,
            false,
        )
    }
    pub(crate) fn resolve_name_suggestion(
        &mut self,
        location: Option<NodeId>,
        name: &[u8],
        meaning: SymbolFlags,
    ) -> Result<Option<SymbolId>, Error> {
        self.ensure_primitive_alias_suggestions();
        self.resolve_name_mode(location, name, meaning, None, false, false, true)
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "Parameters preserve the upstream operation and its independently selected checking modes"
    )]
    fn resolve_name_mode(
        &mut self,
        location: Option<NodeId>,
        name: &[u8],
        meaning: SymbolFlags,
        message: Option<&'static Message>,
        is_use: bool,
        exclude_globals: bool,
        suggestion: bool,
    ) -> Result<Option<SymbolId>, Error> {
        let mut alias_flags = crate::types::Map::default();
        let mut spelling_alias_flags = crate::types::Map::default();
        loop {
            let options = self.program()?.host.options();
            let mut resolver = NameResolver {
                options: ResolverOptions {
                    emit_script_target: options.emit_script_target(),
                    isolated_modules: options.isolated_modules(),
                    verbatim_module_syntax: options.verbatim_module_syntax.is_true(),
                    emit_standard_class_fields: options.emit_standard_class_fields(),
                },
                globals: self.builtins.globals,
                arguments_symbol: Some(self.builtins.arguments_symbol),
                require_symbol: Some(self.builtins.require_symbol),
            };
            let mut hooks = Hooks {
                state: self,
                effects: Vec::new(),
                failure: None,
                alias_flags: &alias_flags,
                spelling_alias_flags: &spelling_alias_flags,
                pending_alias: None,
                suggestion,
            };
            let result = resolver.resolve(
                &mut ReadHost(self),
                &mut hooks,
                location,
                name,
                meaning,
                message,
                is_use,
                exclude_globals,
            );
            let pending_alias = hooks.pending_alias;
            let failure = hooks.failure;
            let effects = hooks.effects;
            if let Some(alias) = pending_alias {
                drop(effects);
                match alias {
                    PendingAlias::Lookup(alias) => {
                        let flags = self.module_symbol_flags(alias, false, false)?;
                        alias_flags.insert(alias, flags);
                    }
                    PendingAlias::SpellingCandidate(alias) => {
                        let flags = match self.try_resolve_alias(alias)? {
                            Some(target) => self.symbol(target)?.flags(),
                            None => 0,
                        };
                        // A skipped in-progress alias is local to this lookup;
                        // do not cache an unknown target on the alias itself.
                        spelling_alias_flags.insert(alias, flags);
                    }
                }
                continue;
            }
            for effect in effects {
                match effect {
                    Effect::Error(location, message, args) => {
                        self.error_at(location, message, args)?;
                    }
                    Effect::Referenced(symbol, meaning) => {
                        *self.query.references.get_or_default(symbol) |= meaning;
                    }
                    Effect::Scope(node, value) => {
                        *self.query.scope_changes.get_or_default(node) = value;
                    }
                    Effect::Resolved(resolution) => self.on_source_symbol_resolved(resolution)?,
                    Effect::Failed(location, name, meaning, message) => {
                        self.on_failed_source_name(location, name.as_bytes(), meaning, message)?;
                    }
                    Effect::InvalidInitializer(location, name, property, result) => self
                        .report_invalid_initializer(location, name.as_bytes(), property, result)?,
                }
            }
            if let Some(error) = failure {
                return Err(error);
            }
            return Ok(result?);
        }
    }

    pub(crate) fn lookup_symbol_resolving(
        &mut self,
        table: Option<SymbolTableId>,
        name: &[u8],
        meaning: SymbolFlags,
    ) -> Result<Option<SymbolId>, Error> {
        match self.lookup_symbol(table, name, meaning) {
            Err(Error::Unsupported("getSymbolFlags: alias resolution")) => {
                let table = table.ok_or(ts_arena::Error::InvalidGraph)?;
                let symbol = self
                    .table(table)?
                    .get(name)
                    .flatten()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                let symbol = self.get_merged_symbol(symbol);
                let flags = self.module_symbol_flags(symbol, false, false)?;
                Ok((flags & meaning != 0).then_some(symbol))
            }
            result => result,
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.onSuccessfullyResolvedSymbol
    fn on_source_symbol_resolved(&mut self, resolution: ResolvedName) -> Result<(), Error> {
        let Some(location) = resolution.original_location else {
            return Ok(());
        };
        let meaning = resolution.meaning;
        // TYPE and VALUE overlap (classes/enums). Testing for any overlap
        // would incorrectly run value-only checks on an ordinary type name.
        if meaning & sf::BLOCK_SCOPED_VARIABLE != 0
            || meaning & (sf::CLASS | sf::ENUM) != 0 && meaning & sf::VALUE == sf::VALUE
        {
            let exported = self.get_export_symbol_of_value_symbol_if_exported(resolution.symbol)?;
            if self.symbol(exported)?.flags() & (sf::BLOCK_SCOPED_VARIABLE | sf::CLASS | sf::ENUM)
                != 0
            {
                self.check_resolved_block_scoped_variable(exported, location)?;
            }
        }
        if meaning & sf::VALUE == sf::VALUE {
            if let Some(last) = resolution.last_location {
                let view = self.ast(last)?;
                if view.node(last)?.kind() == ts_ast::SyntaxKind::SourceFile
                    && ts_ast::utilities::is_external_or_common_js_module(&view.source_file(last)?)
                {
                    if self.ast(location)?.node(location)?.flags() & ts_ast::node_flags::JS_DOC == 0
                    {
                        let merged = self.get_merged_symbol(resolution.symbol);
                        let declarations = self.symbol_declarations(merged)?.to_vec();
                        let mut umd = !declarations.is_empty();
                        for declaration in declarations.into_iter().flatten() {
                            let kind = self.ast(declaration)?.node(declaration)?.kind();
                            if kind != ts_ast::SyntaxKind::NamespaceExportDeclaration
                                && !(kind == ts_ast::SyntaxKind::SourceFile
                                    && self
                                        .program()?
                                        .bound(declaration)?
                                        .result()
                                        .global_exports()
                                        .is_some())
                            {
                                umd = false;
                                break;
                            }
                        }
                        if umd {
                            let name = self.symbol(resolution.symbol)?.name_to_owned();
                            let diagnostic = self.diagnostic_for_node(Some(location), ts_diagnostics::X_0_refers_to_a_UMD_global_but_the_current_file_is_a_module_Consider_adding_an_import_instead, vec![name])?;
                            let error = !self
                                .program()?
                                .host
                                .options()
                                .allow_umd_global_access
                                .is_true();
                            self.variable_error_or_suggestion(error, diagnostic)?;
                        }
                    }
                    if self.program()?.host.options().isolated_modules.is_true() {
                        let name = self.symbol(resolution.symbol)?.name_to_owned();
                        if self.lookup_symbol_resolving(
                            self.builtins.globals,
                            name.as_bytes(),
                            meaning,
                        )? == Some(resolution.symbol)
                        {
                            let locals = self
                                .program()?
                                .bound(last)?
                                .node_binding(last)?
                                .and_then(|binding| binding.locals);
                            if let Some(symbol) =
                                self.lookup_symbol_resolving(locals, name.as_bytes(), !sf::VALUE)?
                            {
                                for declaration in self
                                    .symbol_declarations(symbol)?
                                    .to_vec()
                                    .into_iter()
                                    .flatten()
                                {
                                    if matches!(
                                        self.ast(declaration)?.node(declaration)?.kind().known(),
                                        Some(
                                            ts_ast::SyntaxKind::ImportSpecifier
                                                | ts_ast::SyntaxKind::ImportClause
                                                | ts_ast::SyntaxKind::NamespaceImport
                                                | ts_ast::SyntaxKind::ImportEqualsDeclaration
                                        )
                                    ) {
                                        let view = self.ast(declaration)?;
                                        if !crate::external_resolution::type_only_import_or_export_declaration(
                                            view,
                                            &view.node(declaration)?,
                                        )? {
                                            self.error_at(
                                                Some(declaration),
                                                ts_diagnostics::Import_0_conflicts_with_global_value_used_in_this_file_so_must_be_declared_with_a_type_only_import_when_isolatedModules_is_enabled,
                                                vec![name.clone()],
                                            )?;
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !resolution.within_deferred_context {
                if let Some(declaration) = resolution.associated_declaration {
                    self.check_parameter_initializer_name(
                        location,
                        resolution.symbol,
                        meaning,
                        declaration,
                    )?;
                }
            }
        }
        let flags = self.symbol(resolution.symbol)?.flags();
        if meaning & sf::VALUE != 0
            && flags & sf::ALIAS != 0
            && flags & sf::VALUE == 0
            && !self.valid_type_only_alias_use_site(location)?
        {
            if let Some(declaration) = self.module_type_only_alias(resolution.symbol, sf::VALUE)? {
                let exported = matches!(
                    self.ast(declaration)?.node(declaration)?.kind().known(),
                    Some(
                        ts_ast::SyntaxKind::ExportSpecifier
                            | ts_ast::SyntaxKind::ExportDeclaration
                            | ts_ast::SyntaxKind::NamespaceExport
                    )
                );
                let name = self.symbol(resolution.symbol)?.name_to_owned();
                let primary = self.error_at(Some(location), if exported {ts_diagnostics::X_0_cannot_be_used_as_a_value_because_it_was_exported_using_export_type} else {ts_diagnostics::X_0_cannot_be_used_as_a_value_because_it_was_imported_using_import_type},vec![name.clone()])?;
                if let Some(primary) = primary {
                    let related = self.diagnostic_for_node(
                        Some(declaration),
                        if exported {
                            ts_diagnostics::X_0_was_exported_here
                        } else {
                            ts_diagnostics::X_0_was_imported_here
                        },
                        vec![name],
                    )?;
                    self.add_related_diagnostic(primary, related)?;
                }
            }
        }
        Ok(())
    }
}

// First library of every feature-map key at the pin. Property-specific entries
// belong to the later missing-property diagnostic path.
// port: tsc/internal/checker/checker.go:Checker.getSuggestedLibForNonExistentName
// Source table: getFeatureMap variable initializer in tsc/internal/checker/utilities.go.
pub(crate) fn suggested_library(name: &[u8]) -> Option<&'static [u8]> {
    match name {
        b"Array"
        | b"Iterator"
        | b"AsyncIterator"
        | b"RegExp"
        | b"Reflect"
        | b"ArrayConstructor"
        | b"ObjectConstructor"
        | b"NumberConstructor"
        | b"Math"
        | b"Map"
        | b"Set"
        | b"PromiseConstructor"
        | b"Symbol"
        | b"WeakMap"
        | b"WeakSet"
        | b"String"
        | b"StringConstructor"
        | b"Promise" => Some(b"es2015"),
        b"ArrayBuffer" | b"MapConstructor" => Some(b"es2024"),
        b"Atomics" | b"SharedArrayBuffer" | b"DateTimeFormat" => Some(b"es2017"),
        b"AsyncIterable"
        | b"AsyncIterableIterator"
        | b"AsyncGenerator"
        | b"AsyncGeneratorFunction"
        | b"RegExpMatchArray"
        | b"RegExpExecArray"
        | b"Intl"
        | b"NumberFormat" => Some(b"es2018"),
        b"RegExpConstructor" | b"Float16Array" => Some(b"es2025"),
        b"SymbolConstructor"
        | b"DataView"
        | b"BigInt"
        | b"RelativeTimeFormat"
        | b"BigInt64Array"
        | b"BigUint64Array" => Some(b"es2020"),
        b"Int8Array" | b"Uint8Array" | b"Uint8ClampedArray" | b"Int16Array" | b"Uint16Array"
        | b"Int32Array" | b"Uint32Array" | b"Float32Array" | b"Float64Array" | b"Error" => {
            Some(b"es2022")
        }
        b"ErrorConstructor"
        | b"Uint8ArrayConstructor"
        | b"DisposableStack"
        | b"AsyncDisposableStack"
        | b"Date" => Some(b"esnext"),
        _ => None,
    }
}
