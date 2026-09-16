//! Name serialization uses lexical tables and qualified symbol chains. The
//! recursion guard is keyed by both symbol and table provenance, as upstream.
use crate::node_builder::NodeBuilder;
use crate::{
    types::{Map, Set},
    Error,
};
use std::cmp::Ordering;
use ts_arena::{NodeId, SymbolId};
use ts_ast::{symbol_flags as sf, JsString, SymbolTableId, SyntaxKind as K};

#[path = "node_builder_imports.rs"]
mod imports;

#[path = "node_builder_containers.rs"]
mod containers;
#[path = "node_builder_scope.rs"]
mod scope;

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub(super) enum NameTableId {
    Locals(NodeId),
    Exports(SymbolId),
    Members(SymbolId),
    Globals,
    ResolvedExports(SymbolId),
}
#[derive(Clone)]
pub(super) struct NameTable {
    id: NameTableId,
    table: Option<SymbolTableId>,
    singleton: Option<(JsString, SymbolId)>,
}
#[derive(Clone, Copy)]
struct NameQuery {
    symbol: SymbolId,
    enclosing: Option<NodeId>,
    meaning: u32,
    external_only: bool,
}
#[derive(Default)]
pub(super) struct NameAccess {
    chains: Map<(SymbolId, bool, Option<NodeId>, u32), Vec<SymbolId>>,
    aliases: Map<NameTableId, Vec<SymbolId>>,
    visited: Set<(SymbolId, NameTableId)>,
    extended: Map<SymbolId, Vec<SymbolId>>,
    extended_by_file: Map<(SymbolId, NodeId), Vec<SymbolId>>,
}
fn left_meaning(meaning: u32) -> u32 {
    if meaning == sf::VALUE {
        sf::VALUE
    } else {
        sf::NAMESPACE
    }
}

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.symbolToNode
    pub(crate) fn symbol_display_node(
        &mut self,
        symbol: SymbolId,
        meaning: u32,
        allow_any_node: bool,
    ) -> Result<NodeId, Error> {
        use ts_ast::FactoryMethods;
        if !allow_any_node {
            let chain = self.display_name_chain(symbol, self.enclosing, meaning)?;
            return self.entity_name_from_symbol_chain(&chain);
        }
        if self.internal_flags & ts_nodebuilder::internal_flags::WRITE_COMPUTED_PROPS != 0 {
            if let Some(declaration) = self.checker.symbol(symbol)?.value_declaration() {
                let view = self.checker.ast(declaration)?;
                if let Some(name) = view.node(declaration)?.name() {
                    if view.node(name)?.kind() == K::ComputedPropertyName {
                        self.retain_source_node(name)?;
                        return Ok(name);
                    }
                }
            }
            if let Some(ty) = self
                .checker
                .value_symbol_links
                .try_get(symbol)
                .and_then(|l| l.name_type)
            {
                if self.checker.types.flags(ty)?
                    & (crate::type_flags::ENUM_LITERAL | crate::type_flags::UNIQUE_ES_SYMBOL)
                    != 0
                {
                    let target = self
                        .checker
                        .types
                        .get(ty)?
                        .symbol
                        .ok_or(Error::MissingLink("computed name symbol"))?;
                    let old = self.enclosing;
                    self.enclosing = self.checker.symbol(target)?.value_declaration();
                    let expression =
                        self.symbol_expression_with_meaning(target, self.enclosing, meaning);
                    self.enclosing = old;
                    return Ok(self.ast.new_computed_property_name(Some(expression?)));
                }
            }
        }
        self.symbol_expression_with_meaning(symbol, self.enclosing, meaning)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.createEntityNameFromSymbolChain
    fn entity_name_from_symbol_chain(&mut self, chain: &[SymbolId]) -> Result<NodeId, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.entity_name_from_symbol_chain_worker(chain)
        })
    }

    fn entity_name_from_symbol_chain_worker(
        &mut self,
        chain: &[SymbolId],
    ) -> Result<NodeId, Error> {
        use ts_ast::FactoryMethods;
        let (&symbol, prefix) = chain
            .split_last()
            .ok_or(Error::MissingLink("entity name chain"))?;
        if prefix.is_empty() {
            self.flags |= ts_nodebuilder::flags::IN_INITIAL_ENTITY_NAME;
        }
        let name = self.symbol_name(symbol);
        if prefix.is_empty() {
            self.flags ^= ts_nodebuilder::flags::IN_INITIAL_ENTITY_NAME;
        }
        let identifier = self.ast.new_identifier(name?);
        self.emit
            .add_emit_flags(identifier, ts_printer::emit_flags::NO_ASCII_ESCAPING);
        if prefix.is_empty() {
            return Ok(identifier);
        }
        let left = self.entity_name_from_symbol_chain(prefix)?;
        Ok(self.ast.new_qualified_name(Some(left), Some(identifier)))
    }

    pub(super) fn accessibility_chain(
        &mut self,
        symbol: SymbolId,
        enclosing: NodeId,
        meaning: u32,
    ) -> Result<Vec<SymbolId>, Error> {
        self.accessible_name_chain(NameQuery {
            symbol,
            enclosing: Some(enclosing),
            meaning,
            external_only: false,
        })
    }
    pub(super) fn accessibility_containers(
        &mut self,
        symbol: SymbolId,
        enclosing: NodeId,
        meaning: u32,
    ) -> Result<Vec<SymbolId>, Error> {
        self.name_containers(NameQuery {
            symbol,
            enclosing: Some(enclosing),
            meaning,
            external_only: false,
        })
    }
    pub(super) fn accessibility_external_container(
        &mut self,
        node: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        self.external_name_container(node)
    }
    pub(crate) fn symbol_expression_with_meaning(
        &mut self,
        symbol: SymbolId,
        enclosing: Option<NodeId>,
        meaning: u32,
    ) -> Result<NodeId, Error> {
        let previous = std::mem::replace(&mut self.enclosing, enclosing);
        let tracked = self.track_symbol(symbol, meaning);
        self.enclosing = previous;
        tracked?;
        let chain = self.display_name_chain(symbol, enclosing, meaning)?;
        self.expression_from_name_chain(
            &chain,
            chain
                .len()
                .checked_sub(1)
                .ok_or(Error::MissingLink("symbol expression chain"))?,
        )
    }
    fn name_has_declaration_kind(&self, symbol: SymbolId, kind: K) -> Result<bool, Error> {
        for node in self.checker.symbol_declarations(symbol)?.iter().flatten() {
            if self.checker.ast(node)?.node(node)?.kind() == kind {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(super) fn name_external_module(&self, symbol: SymbolId) -> Result<bool, Error> {
        for node in self.checker.symbol_declarations(symbol)?.iter().flatten() {
            let view = self.checker.ast(node)?;
            let read = view.node(node)?;
            if read.kind() == K::SourceFile
                && ts_ast::utilities::is_external_or_common_js_module(&view.source_file(node)?)
            {
                return Ok(true);
            }
            if read.kind() == K::ModuleDeclaration {
                if let Some(name) = read.name() {
                    if view.node(name)?.kind() == K::StringLiteral {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.getSpecifierForModuleSymbol
    fn module_file_declaration(&mut self, symbol: SymbolId) -> Result<Option<NodeId>, Error> {
        let declarations: Vec<_> = self
            .checker
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect();
        let mut file = None;
        for &declaration in &declarations {
            if self.checker.ast(declaration)?.node(declaration)?.kind() == K::SourceFile {
                file = Some(declaration);
                break;
            }
        }
        if file.is_none() {
            for &declaration in &declarations {
                if let Some(container) = self.external_name_container(declaration)? {
                    if let Some(exports) = self.checker.symbol(container)?.exports() {
                        if let Some(export) = self
                            .checker
                            .table(exports)?
                            .get(ts_ast::internal_symbol_names::EXPORT_EQUALS)
                            .flatten()
                        {
                            if self.checker.module_symbols_same_reference(export, symbol)? {
                                for node in self
                                    .checker
                                    .symbol_declarations(container)?
                                    .iter()
                                    .flatten()
                                {
                                    if self.checker.ast(node)?.node(node)?.kind() == K::SourceFile {
                                        file = Some(node);
                                        break;
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }
        Ok(file)
    }

    // The diagnostic symbolToString API has no enclosing declaration or file.
    // Keep that native branch separate from declaration-emit specifier ranking.
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.getSpecifierForModuleSymbol
    pub(super) fn context_free_module_specifier(
        &mut self,
        symbol: SymbolId,
    ) -> Result<JsString, Error> {
        let declarations: Vec<_> = self
            .checker
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect();
        let file = self.module_file_declaration(symbol)?;
        if file.is_none() {
            for &declaration in &declarations {
                let view = self.checker.ast(declaration)?;
                let read = view.node(declaration)?;
                if read.kind() == K::ModuleDeclaration {
                    if let Some(name) = read.name() {
                        if view.node(name)?.kind() == K::StringLiteral {
                            return Ok(JsString::from_bytes(view.node_text(name)?.as_bytes()));
                        }
                    }
                }
            }
        }
        if let Some(specifier) = ts_ast::try_get_ambient_module_name_from_symbol_name(
            self.checker.symbol(symbol)?.name_bytes(),
        ) {
            return Ok(JsString::from_bytes(specifier));
        }
        let source = self
            .module_source_file(symbol)?
            .ok_or(Error::MissingLink("module source file"))?;
        Ok(JsString::from_bytes(
            self.checker.ast(source)?.source_file(source)?.file_name(),
        ))
    }

    // port: tsc/internal/ast/utilities.go:GetSourceFileOfModule
    fn module_source_file(&self, symbol: SymbolId) -> Result<Option<NodeId>, Error> {
        let mut declaration = self.checker.symbol(symbol)?.value_declaration();
        if declaration.is_none() {
            for candidate in self.checker.symbol_declarations(symbol)?.iter().flatten() {
                let view = self.checker.ast(candidate)?;
                let read = view.node(candidate)?;
                let external_augmentation = read.kind() == K::ModuleDeclaration
                    && ts_ast::is_ambient_module(view, candidate)?
                    && ts_ast::is_module_augmentation_external(view, candidate)?;
                if !external_augmentation && !ts_ast::utilities::is_global_scope_augmentation(&read)
                {
                    declaration = Some(candidate);
                    break;
                }
            }
        }
        match declaration {
            Some(node) => Ok(ts_ast::utilities::get_source_file_of_node(
                self.checker.ast(node)?,
                Some(node),
            )?),
            None => Ok(None),
        }
    }

    // Keep symbol/container selection here; generation uses the immutable
    // program host, including retained import modes and package identity.
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.getSpecifierForModuleSymbol
    pub(super) fn module_specifier_with_context(
        &mut self,
        symbol: SymbolId,
        enclosing: Option<NodeId>,
    ) -> Result<JsString, Error> {
        self.module_specifier_with_context_and_mode(
            symbol,
            enclosing,
            ts_core::ResolutionMode::NONE,
        )
    }

    pub(super) fn module_specifier_with_context_and_mode(
        &mut self,
        symbol: SymbolId,
        enclosing: Option<NodeId>,
        mode: ts_core::ResolutionMode,
    ) -> Result<JsString, Error> {
        let Some(enclosing) = enclosing else {
            return self.context_free_module_specifier(symbol);
        };
        let declarations: Vec<_> = self
            .checker
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect();
        let source = self.module_file_declaration(symbol)?;
        if source.is_none() {
            // Ambient declarations precede file-specifier generation.
            for &declaration in &declarations {
                let view = self.checker.ast(declaration)?;
                let read = view.node(declaration)?;
                if read.kind() == K::ModuleDeclaration {
                    if let Some(name) = read.name() {
                        if view.node(name)?.kind() == K::StringLiteral {
                            return Ok(view.node_text(name)?.into_js_string());
                        }
                    }
                }
            }
            if let Some(name) = ts_ast::try_get_ambient_module_name_from_symbol_name(
                self.checker.symbol(symbol)?.name_bytes(),
            ) {
                return Ok(JsString::from_bytes(name));
            }
        }
        let source = source.ok_or(Error::MissingLink("module source"))?;
        let target = self
            .checker
            .ast(source)?
            .source_file(source)?
            .file_name()
            .to_vec();
        let (importer, importer_name) = self.checker.module_source(enclosing)?;
        let host = self.checker.program()?.host.clone();
        let preferred_mode = if mode == ts_core::ResolutionMode::NONE {
            match self.original_module_specifier(enclosing)? {
                Some(original) => {
                    host.get_mode_for_usage_location(importer_name.as_bytes(), original)?
                }
                None => host.get_default_resolution_mode_for_file(importer_name.as_bytes())?,
            }
        } else {
            mode
        };
        let key = super::cache::SpecifierKey {
            symbol,
            file: importer,
            mode: preferred_mode,
        };
        if let Some(specifier) = self.cached_module_specifier(key) {
            return Ok(specifier);
        }
        let specifier = crate::module_specifiers::generate(
            host.as_ref(),
            importer,
            importer_name.as_bytes(),
            &target,
            mode,
            preferred_mode == ts_core::ResolutionMode::ESNEXT,
        )?;
        self.cache_module_specifier(key, specifier.clone());
        Ok(specifier)
    }

    pub(crate) fn symbol_expression_without_chain(
        &mut self,
        symbol: SymbolId,
        enclosing: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        let name = self.symbol_name(symbol)?;
        if name
            .as_bytes()
            .first()
            .is_some_and(|b| matches!(b, b'\'' | b'"'))
            && self.name_external_module(symbol)?
        {
            let specifier = self.module_specifier_with_context(symbol, enclosing)?;
            self.approximate_length += specifier.len() + 2;
            return Ok(self.string_literal(specifier));
        }
        self.symbol_node(symbol)
    }

    // port: tsc/internal/checker/symbolaccessibility.go:Checker.getAccessibleSymbolChainEx
    fn accessible_name_chain(&mut self, query: NameQuery) -> Result<Vec<SymbolId>, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.accessible_name_chain_worker(query)
        })
    }

    fn accessible_name_chain_worker(&mut self, query: NameQuery) -> Result<Vec<SymbolId>, Error> {
        let declarations: Vec<_> = self
            .checker
            .symbol_declarations(query.symbol)?
            .iter()
            .flatten()
            .collect();
        if !declarations.is_empty() {
            let mut property = true;
            for node in declarations {
                if !matches!(
                    self.checker.ast(node)?.node(node)?.kind().known(),
                    Some(
                        K::PropertyDeclaration
                            | K::MethodDeclaration
                            | K::GetAccessor
                            | K::SetAccessor
                    )
                ) {
                    property = false;
                    break;
                }
            }
            if property {
                return Ok(vec![]);
            }
        }
        let mut first = None;
        self.some_name_scope(query.enclosing, |_, _, node| {
            first = node;
            Ok(true)
        })?;
        let key = (query.symbol, query.external_only, first, query.meaning);
        if let Some(result) = self.name_access.chains.get(&key) {
            return Ok(result.clone());
        }
        let mut result = vec![];
        self.some_name_scope(query.enclosing, |this, table, _| {
            let local = !matches!(table.id, NameTableId::Members(_));
            result = this.name_chain_from_table(query, &table, false, local)?;
            Ok(!result.is_empty())
        })?;
        self.name_access.chains.insert(key, result.clone());
        Ok(result)
    }

    // port: tsc/internal/checker/symbolaccessibility.go:Checker.getAccessibleSymbolChainFromSymbolTable
    fn name_chain_from_table(
        &mut self,
        query: NameQuery,
        table: &NameTable,
        ignore_qualification: bool,
        local: bool,
    ) -> Result<Vec<SymbolId>, Error> {
        let key = (query.symbol, table.id);
        if !self.name_access.visited.insert(key) {
            return Ok(vec![]);
        }
        let result = self.try_name_table(query, table, ignore_qualification, local);
        self.name_access.visited.remove(&key);
        result
    }

    // port: tsc/internal/checker/symbolaccessibility.go:Checker.getSymbolTableAliases
    fn name_table_aliases(&mut self, table: &NameTable) -> Result<Vec<SymbolId>, Error> {
        if matches!(table.id, NameTableId::Members(_)) {
            return Ok(vec![]);
        }
        let cached = !matches!(table.id, NameTableId::Locals(_));
        if cached {
            if let Some(values) = self.name_access.aliases.get(&table.id) {
                return Ok(values.clone());
            }
        }
        let mut aliases = vec![];
        for (_, symbol) in self.name_table_entries(table)? {
            if self.checker.symbol(symbol)?.flags() & sf::ALIAS != 0 {
                aliases.push(symbol);
            }
        }
        if cached {
            self.name_access.aliases.insert(table.id, aliases.clone());
        }
        Ok(aliases)
    }

    // port: tsc/internal/checker/symbolaccessibility.go:Checker.trySymbolTable
    fn try_name_table(
        &mut self,
        query: NameQuery,
        table: &NameTable,
        ignore_qualification: bool,
        local: bool,
    ) -> Result<Vec<SymbolId>, Error> {
        let name = self.checker.symbol(query.symbol)?.name_to_owned();
        let found = self.name_table_lookup(table, name.as_bytes())?;
        if let Some(found) = found {
            if self.name_is_accessible(query, found, None, ignore_qualification)? {
                return Ok(vec![query.symbol]);
            }
        }
        let mut candidates = vec![];
        if let Some(found) = found {
            if let Some(export) = self.checker.symbol(found)?.export_symbol() {
                let export = self.checker.get_merged_symbol(export);
                if self.name_is_accessible(query, export, None, ignore_qualification)? {
                    candidates.push(vec![query.symbol]);
                }
            }
        }
        for alias in self.name_table_aliases(table)? {
            let name = self.checker.symbol(alias)?.name_to_owned();
            if matches!(
                name.as_bytes(),
                ts_ast::internal_symbol_names::EXPORT_EQUALS
                    | ts_ast::internal_symbol_names::DEFAULT
            ) {
                continue;
            }
            if self.name_has_declaration_kind(alias, K::NamespaceExportDeclaration)? {
                if let Some(enclosing) = query.enclosing {
                    let file = ts_ast::utilities::get_source_file_of_node(
                        self.checker.ast(enclosing)?,
                        Some(enclosing),
                    )?
                    .ok_or(Error::MissingLink("name scope source"))?;
                    if self
                        .checker
                        .ast(file)?
                        .source_file(file)?
                        .external_module_indicator
                        .is_some()
                    {
                        continue;
                    }
                }
            }
            if query.external_only && !self.name_has_external_import_equals(alias)? {
                continue;
            }
            if local && self.name_has_namespace_reexport(alias)? {
                continue;
            }
            if !ignore_qualification && self.name_has_declaration_kind(alias, K::ExportSpecifier)? {
                continue;
            }
            let resolved = self.checker.resolve_alias(alias)?;
            let candidate = self.name_candidate(query, alias, resolved, ignore_qualification)?;
            if !candidate.is_empty() {
                candidates.push(candidate);
            }
        }
        if !candidates.is_empty() {
            let mut failure = None;
            candidates.sort_by(|a, b| match self.compare_name_chains(a, b) {
                Ok(order) => order,
                Err(error) => {
                    failure = Some(error);
                    Ordering::Equal
                }
            });
            if let Some(error) = failure {
                return Err(error);
            }
            return Ok(candidates.remove(0));
        }
        if table.id == NameTableId::Globals {
            let global = self.checker.builtins.global_this_symbol;
            return self.name_candidate(query, global, global, ignore_qualification);
        }
        Ok(vec![])
    }

    fn name_has_external_import_equals(&self, symbol: SymbolId) -> Result<bool, Error> {
        for node in self.checker.symbol_declarations(symbol)?.iter().flatten() {
            let read = self.checker.ast(node)?.node(node)?;
            if let Some(import) = read.data_source().as_import_equals_declaration() {
                if let Some(reference) = import.module_reference() {
                    if self.checker.ast(reference)?.node(reference)?.kind()
                        == K::ExternalModuleReference
                    {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }
    fn name_has_namespace_reexport(&self, symbol: SymbolId) -> Result<bool, Error> {
        for node in self.checker.symbol_declarations(symbol)?.iter().flatten() {
            let read = self.checker.ast(node)?.node(node)?;
            if read.kind() == K::NamespaceExport {
                let parent = read
                    .parent()
                    .ok_or(Error::MissingLink("namespace export parent"))?;
                if self.checker.module_specifier(parent)?.is_some() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/symbolaccessibility.go:Checker.compareSymbolChainsWorker
    fn compare_name_chains(&self, a: &[SymbolId], b: &[SymbolId]) -> Result<Ordering, Error> {
        let order = a.len().cmp(&b.len());
        if order != Ordering::Equal {
            return Ok(order);
        }
        for (&a, &b) in a.iter().zip(b) {
            let order = self.checker.compare_symbols(Some(a), Some(b))?;
            if order != Ordering::Equal {
                return Ok(order);
            }
        }
        Ok(Ordering::Equal)
    }

    // port: tsc/internal/checker/symbolaccessibility.go:Checker.getCandidateListForSymbol
    fn name_candidate(
        &mut self,
        query: NameQuery,
        symbol: SymbolId,
        resolved: SymbolId,
        ignore_qualification: bool,
    ) -> Result<Vec<SymbolId>, Error> {
        if self.name_is_accessible(query, symbol, Some(resolved), ignore_qualification)? {
            return Ok(vec![symbol]);
        }
        let Some(table) = self.checker.module_exports_of_symbol(resolved)? else {
            return Ok(vec![]);
        };
        let table = NameTable {
            id: NameTableId::ResolvedExports(resolved),
            table: Some(table),
            singleton: None,
        };
        let result = self.name_chain_from_table(query, &table, true, false)?;
        if result.is_empty()
            || !self.name_can_qualify(query, symbol, left_meaning(query.meaning))?
        {
            return Ok(vec![]);
        }
        let mut chain = vec![symbol];
        chain.extend(result);
        Ok(chain)
    }

    // port: tsc/internal/checker/symbolaccessibility.go:Checker.isAccessible
    fn name_is_accessible(
        &mut self,
        query: NameQuery,
        symbol: SymbolId,
        alias: Option<SymbolId>,
        ignore_qualification: bool,
    ) -> Result<bool, Error> {
        let merged = self.checker.get_merged_symbol(query.symbol);
        if query.symbol != symbol
            && Some(query.symbol) != alias
            && merged != self.checker.get_merged_symbol(symbol)
            && Some(merged) != alias.map(|symbol| self.checker.get_merged_symbol(symbol))
        {
            return Ok(false);
        }
        Ok(!self.name_external_module(symbol)?
            && (ignore_qualification
                || self.name_can_qualify(
                    query,
                    self.checker.get_merged_symbol(symbol),
                    query.meaning,
                )?))
    }
    // port: tsc/internal/checker/symbolaccessibility.go:Checker.canQualifySymbol
    fn name_can_qualify(
        &mut self,
        query: NameQuery,
        symbol: SymbolId,
        meaning: u32,
    ) -> Result<bool, Error> {
        if !self.name_needs_qualification(symbol, query.enclosing, meaning)? {
            return Ok(true);
        }
        let Some(parent) = self.checker.symbol(symbol)?.parent() else {
            return Ok(false);
        };
        Ok(!self
            .accessible_name_chain(NameQuery {
                symbol: parent,
                meaning: left_meaning(meaning),
                ..query
            })?
            .is_empty())
    }
}

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.lookupSymbolChainWorker
    fn display_name_chain(
        &mut self,
        symbol: SymbolId,
        enclosing: Option<NodeId>,
        meaning: u32,
    ) -> Result<Vec<SymbolId>, Error> {
        self.display_name_chain_with_module(symbol, enclosing, meaning, false)
    }

    fn display_name_chain_with_module(
        &mut self,
        symbol: SymbolId,
        enclosing: Option<NodeId>,
        meaning: u32,
        yield_module: bool,
    ) -> Result<Vec<SymbolId>, Error> {
        if self.checker.symbol(symbol)?.flags() & sf::TYPE_PARAMETER != 0
            || self.internal_flags & ts_nodebuilder::internal_flags::DO_NOT_INCLUDE_SYMBOL_CHAIN
                != 0
            || enclosing.is_none()
                && self.flags & ts_nodebuilder::flags::USE_FULLY_QUALIFIED_TYPE == 0
        {
            return Ok(vec![symbol]);
        }
        self.qualified_name_chain(
            NameQuery {
                symbol,
                enclosing,
                meaning,
                external_only: self.flags & ts_nodebuilder::flags::USE_ONLY_EXTERNAL_ALIASING != 0,
            },
            true,
            yield_module,
        )
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.getSymbolChain
    fn qualified_name_chain(
        &mut self,
        query: NameQuery,
        end_of_chain: bool,
        yield_module: bool,
    ) -> Result<Vec<SymbolId>, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.qualified_name_chain_worker(query, end_of_chain, yield_module)
        })
    }

    fn qualified_name_chain_worker(
        &mut self,
        query: NameQuery,
        end_of_chain: bool,
        yield_module: bool,
    ) -> Result<Vec<SymbolId>, Error> {
        let mut chain = self.accessible_name_chain(query)?;
        let qualifier_meaning = if chain.len() > 1 {
            left_meaning(query.meaning)
        } else {
            query.meaning
        };
        if chain.is_empty()
            || self.name_needs_qualification(chain[0], query.enclosing, qualifier_meaning)?
        {
            let root = chain.first().copied().unwrap_or(query.symbol);
            let parents = self.name_containers(NameQuery {
                symbol: root,
                ..query
            })?;
            // External containers are ranked by their module specifiers: fewer
            // path components first, non-relative before relative, then by
            // symbol order (sortByBestName).
            let mut ranked: Vec<(SymbolId, JsString)> = Vec::with_capacity(parents.len());
            for &parent in &parents {
                let specifier = if self.name_external_module(parent)? {
                    self.module_specifier_with_context(parent, query.enclosing)?
                } else {
                    JsString::default()
                };
                ranked.push((parent, specifier));
            }
            let failure = std::cell::Cell::new(None);
            ranked.sort_by(|a, b| {
                let (specifier_a, specifier_b) = (a.1.as_bytes(), b.1.as_bytes());
                if !specifier_a.is_empty() && !specifier_b.is_empty() {
                    let b_relative = path_is_relative(specifier_b);
                    if path_is_relative(specifier_a) == b_relative {
                        // Both relative or both non-relative, sort by number of parts
                        return count_path_components(specifier_a)
                            .cmp(&count_path_components(specifier_b));
                    }
                    // A non-relative specifier is preferred over a relative one
                    return if b_relative {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    };
                }
                // must sort symbols for stable ordering
                match self.checker.compare_symbols(Some(a.0), Some(b.0)) {
                    Ok(order) => order,
                    Err(error) => {
                        failure.set(Some(error));
                        Ordering::Equal
                    }
                }
            });
            if let Some(error) = failure.into_inner() {
                return Err(error);
            }
            for (parent, _) in ranked {
                let mut parent_chain = self.qualified_name_chain(
                    NameQuery {
                        symbol: parent,
                        meaning: left_meaning(query.meaning),
                        ..query
                    },
                    false,
                    yield_module,
                )?;
                if parent_chain.is_empty() {
                    continue;
                }
                if let Some(exports) = self.checker.symbol(parent)?.exports() {
                    if let Some(export) = self
                        .checker
                        .table(exports)?
                        .get(ts_ast::internal_symbol_names::EXPORT_EQUALS)
                        .flatten()
                    {
                        if self
                            .checker
                            .module_symbols_same_reference(export, query.symbol)?
                        {
                            chain = parent_chain;
                            break;
                        }
                    }
                }
                if chain.is_empty() {
                    chain.push(
                        self.name_alias_in_container(parent, query.symbol)?
                            .unwrap_or(query.symbol),
                    );
                }
                parent_chain.extend(chain);
                chain = parent_chain;
                break;
            }
        }
        if !chain.is_empty() {
            return Ok(chain);
        }
        if end_of_chain
            || self.checker.symbol(query.symbol)?.flags() & (sf::TYPE_LITERAL | sf::OBJECT_LITERAL)
                == 0
        {
            if !end_of_chain && !yield_module && self.name_external_module(query.symbol)? {
                return Ok(vec![]);
            }
            return Ok(vec![query.symbol]);
        }
        Ok(vec![])
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.symbolToExpression
    pub(super) fn symbol_expression(
        &mut self,
        symbol: SymbolId,
        enclosing: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        let previous = std::mem::replace(&mut self.enclosing, enclosing);
        let tracked = self.track_symbol(symbol, sf::VALUE);
        self.enclosing = previous;
        tracked?;
        let chain = self.display_name_chain(symbol, enclosing, sf::VALUE)?;
        self.expression_from_name_chain(
            &chain,
            chain
                .len()
                .checked_sub(1)
                .ok_or(Error::MissingLink("expression name chain"))?,
        )
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.createExpressionFromSymbolChain
    fn expression_from_name_chain(
        &mut self,
        chain: &[SymbolId],
        index: usize,
    ) -> Result<NodeId, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.expression_from_name_chain_worker(chain, index)
        })
    }

    fn expression_from_name_chain_worker(
        &mut self,
        chain: &[SymbolId],
        index: usize,
    ) -> Result<NodeId, Error> {
        use ts_ast::FactoryMethods;
        let symbol = chain[index];
        if self.flags & ts_nodebuilder::flags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME != 0
            && index + 1 < chain.len()
        {
            for declaration in self.checker.symbol_declarations(symbol)?.iter().flatten() {
                if self
                    .checker
                    .ast(declaration)?
                    .node(declaration)?
                    .type_parameter_list()
                    .is_some()
                {
                    return Err(Error::Unsupported(
                        "lookupExpressionChainTypeArgumentNodes: generic qualified value",
                    ));
                }
            }
        }
        if index == 0 {
            self.flags |= ts_nodebuilder::flags::IN_INITIAL_ENTITY_NAME;
        }
        let name = self.symbol_name(symbol);
        if index == 0 {
            self.flags ^= ts_nodebuilder::flags::IN_INITIAL_ENTITY_NAME;
        }
        let mut name = name?;
        if name
            .as_bytes()
            .first()
            .is_some_and(|b| matches!(b, b'\'' | b'"'))
            && self.name_external_module(symbol)?
        {
            let specifier = self.module_specifier_with_context(symbol, self.enclosing)?;
            self.approximate_length += specifier.len() + 2;
            return Ok(self.string_literal(specifier));
        }
        let can_access = if name.as_bytes().starts_with(b"#") {
            name.len() > 1
                && ts_scanner::is_identifier_text(
                    &name.as_bytes()[1..],
                    ts_core::LanguageVariant::STANDARD,
                )
        } else {
            ts_scanner::is_identifier_text(name.as_bytes(), ts_core::LanguageVariant::STANDARD)
        };
        if index == 0 || can_access {
            let identifier = self.ast.new_identifier(name.clone());
            self.emit
                .add_emit_flags(identifier, ts_printer::emit_flags::NO_ASCII_ESCAPING);
            self.approximate_length += name.len() + 1;
            if index > 0 {
                let left = self.expression_from_name_chain(chain, index - 1)?;
                let node =
                    self.ast
                        .new_property_access_expression(Some(left), None, Some(identifier), 0);
                self.emit
                    .add_emit_flags(node, ts_printer::emit_flags::NO_INDENTATION);
                return Ok(node);
            }
            return Ok(identifier);
        }
        if name.as_bytes().starts_with(b"[") {
            name = JsString::from_bytes(&name.as_bytes()[1..name.len() - 1]);
        }
        let expression = if name
            .as_bytes()
            .first()
            .is_some_and(|b| matches!(b, b'\'' | b'"'))
            && self.checker.symbol(symbol)?.flags() & sf::ENUM_MEMBER == 0
        {
            let single = name.as_bytes()[0] == b'\'';
            let text = unquote_name(name.as_bytes());
            self.approximate_length += text.len() + 2;
            self.ast.new_string_literal(
                text,
                if single {
                    ts_ast::token_flags::SINGLE_QUOTE
                } else {
                    0
                },
            )
        } else if ts_jsnum::from_string(name.as_bytes())
            .to_string()
            .as_bytes()
            == name.as_bytes()
        {
            self.approximate_length += name.len();
            self.ast.new_numeric_literal(name, 0)
        } else {
            self.approximate_length += name.len();
            let node = self.ast.new_identifier(name);
            self.emit
                .add_emit_flags(node, ts_printer::emit_flags::NO_ASCII_ESCAPING);
            node
        };
        self.approximate_length += 2;
        let left = self.expression_from_name_chain(chain, index - 1)?;
        Ok(self
            .ast
            .new_element_access_expression(Some(left), None, Some(expression), 0))
    }
}

// port: tsc/internal/stringutil/util.go:UnquoteString
fn unquote_name(mut name: &[u8]) -> JsString {
    if name.len() >= 2 && name.first() == name.last() && matches!(name[0], b'\'' | b'"' | b'`') {
        name = &name[1..name.len() - 1];
    }
    let mut result = Vec::with_capacity(name.len());
    let mut i = 0;
    while i < name.len() {
        if name[i] == b'\\' && i + 1 < name.len() && name[i + 1] != b'\n' {
            i += 1;
        }
        result.push(name[i]);
        i += 1;
    }
    JsString::from_bytes(result)
}

// port: tsc/internal/checker/utilities.go:isLateBoundName
pub(super) fn is_late_bound_name(name: &[u8]) -> bool {
    name.len() >= 2 && name[0] == 0xFE && name[1] == b'@'
}

impl NodeBuilder<'_> {
    /// `lookupSymbolChain` for type-node construction, preserving a module root
    /// when alias policy requires an import type.
    pub(super) fn type_symbol_chain(
        &mut self,
        symbol: SymbolId,
        meaning: u32,
    ) -> Result<Vec<SymbolId>, Error> {
        let chain = self.display_name_chain_with_module(
            symbol,
            self.enclosing,
            meaning,
            self.flags & ts_nodebuilder::flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE == 0,
        )?;
        if chain.is_empty() {
            return Err(Error::MissingLink("symbol type node chain"));
        }
        Ok(chain)
    }

    /// Resolve aliases before deciding between an entity name and an import type.
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.symbolToTypeNode
    pub(super) fn symbol_type_node_from_chain(
        &mut self,
        symbol: SymbolId,
        meaning: u32,
        type_arguments: Option<ts_ast::NodeListId>,
    ) -> Result<NodeId, Error> {
        let chain = self.type_symbol_chain(symbol, meaning)?;
        self.symbol_type_node_from_resolved_chain(&chain, meaning, type_arguments)
    }

    fn symbol_type_node_from_resolved_chain(
        &mut self,
        chain: &[SymbolId],
        meaning: u32,
        type_arguments: Option<ts_ast::NodeListId>,
    ) -> Result<NodeId, Error> {
        use ts_ast::FactoryMethods;
        let is_type_of = meaning == sf::VALUE;
        if self.name_external_module(chain[0])? {
            return self.import_type_from_symbol_chain(chain, is_type_of, type_arguments);
        }
        let entity_name =
            self.access_from_symbol_chain(chain, chain.len() - 1, 0, type_arguments)?;
        let kind = self.ast.view().node(entity_name)?.kind();
        if kind == K::IndexedAccessType {
            // Indexed accesses can never be `typeof`
            return Ok(entity_name);
        }
        if matches!(kind.known(), Some(K::Identifier | K::QualifiedName)) {
            return Ok(if is_type_of {
                self.ast.new_type_query_node(Some(entity_name), None)
            } else {
                self.ast
                    .new_type_reference_node(Some(entity_name), type_arguments)
            });
        }
        Err(Error::Unsupported(
            "symbolToTypeNode: expression with type arguments",
        ))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.symbolToTypeNode
    fn import_type_from_symbol_chain(
        &mut self,
        chain: &[SymbolId],
        is_type_of: bool,
        type_arguments: Option<ts_ast::NodeListId>,
    ) -> Result<NodeId, Error> {
        use ts_ast::FactoryMethods;
        let qualifier = if chain.len() > 1 {
            Some(self.access_from_symbol_chain(chain, chain.len() - 1, 1, type_arguments)?)
        } else {
            None
        };
        let arguments = match type_arguments {
            Some(arguments) => Some(arguments),
            None => self.qualified_type_parameter_nodes(chain, 0)?,
        };
        let (specifier, attributes) =
            self.import_type_specifier(chain[0], *chain.last().unwrap())?;
        self.approximate_length += specifier.len() + 10;
        let literal = self.string_literal(specifier);
        let argument = self.ast.new_literal_type_node(Some(literal));
        if let Some(mut node) = qualifier {
            if self.ast.view().node(node)?.kind() == K::IndexedAccessType {
                // getTopmostIndexedAccessType follows the object side to its
                // first indexed access. Preserve that exact pinned extraction.
                loop {
                    let object = self
                        .ast
                        .view()
                        .node(node)?
                        .as_indexed_access_type_node()
                        .unwrap()
                        .object_type()
                        .unwrap();
                    if self.ast.view().node(object)?.kind() != K::IndexedAccessType {
                        break;
                    }
                    node = object;
                }
                let indexed = self
                    .ast
                    .view()
                    .node(node)?
                    .data_source()
                    .as_indexed_access_type_node()
                    .unwrap()
                    .to_owned();
                let qualifier = self
                    .ast
                    .view()
                    .node(indexed.object_type.unwrap())?
                    .as_type_reference_node()
                    .unwrap()
                    .type_name();
                let imported = self.ast.new_import_type_node(
                    is_type_of,
                    Some(argument),
                    attributes,
                    qualifier,
                    arguments,
                );
                return Ok(self
                    .ast
                    .new_indexed_access_type_node(Some(imported), indexed.index_type));
            }
        }
        Ok(self.ast.new_import_type_node(
            is_type_of,
            Some(argument),
            attributes,
            qualifier,
            arguments,
        ))
    }

    /// Type arguments written for a non-final chain component.
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.lookupTypeParameterNodes
    fn qualified_type_parameter_nodes(
        &mut self,
        chain: &[SymbolId],
        index: usize,
    ) -> Result<Option<ts_ast::NodeListId>, Error> {
        if self.flags & ts_nodebuilder::flags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME == 0
            || index + 1 >= chain.len()
        {
            return Ok(None);
        }
        for declaration in self
            .checker
            .symbol_declarations(chain[index])?
            .iter()
            .flatten()
        {
            if self
                .checker
                .ast(declaration)?
                .node(declaration)?
                .type_parameter_list()
                .is_some()
            {
                return Err(Error::Unsupported(
                    "lookupTypeParameterNodes: generic qualified type name",
                ));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.createAccessFromSymbolChain
    pub(super) fn access_from_symbol_chain(
        &mut self,
        chain: &[SymbolId],
        index: usize,
        stopper: usize,
        override_type_arguments: Option<ts_ast::NodeListId>,
    ) -> Result<NodeId, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.access_from_symbol_chain_worker(chain, index, stopper, override_type_arguments)
        })
    }

    fn access_from_symbol_chain_worker(
        &mut self,
        chain: &[SymbolId],
        index: usize,
        stopper: usize,
        override_type_arguments: Option<ts_ast::NodeListId>,
    ) -> Result<NodeId, Error> {
        use ts_ast::FactoryMethods;
        let type_parameter_nodes = if index + 1 == chain.len() {
            override_type_arguments
        } else {
            self.qualified_type_parameter_nodes(chain, index)?
        };
        let symbol = chain[index];
        let parent = (index > 0).then(|| chain[index - 1]);
        let raw_name = self.checker.symbol(symbol)?.name_to_owned();
        let mut symbol_name: Option<JsString> = None;
        if index == 0 {
            self.flags |= ts_nodebuilder::flags::IN_INITIAL_ENTITY_NAME;
            let name = self.symbol_name(symbol);
            self.flags ^= ts_nodebuilder::flags::IN_INITIAL_ENTITY_NAME;
            let name = name?;
            self.approximate_length += name.len() + 1;
            symbol_name = Some(name);
        } else if let Some(parent) = parent {
            // lookup a ref to symbol within parent to handle export aliases
            if let Some(exports) = self.checker.module_exports_of_symbol(parent)? {
                let direct = self
                    .checker
                    .table(exports)?
                    .get(raw_name.as_bytes())
                    .flatten();
                let export_equals =
                    raw_name.as_bytes() == ts_ast::internal_symbol_names::EXPORT_EQUALS;
                let same_direct = match direct {
                    Some(direct) if !export_equals && !is_late_bound_name(raw_name.as_bytes()) => {
                        self.checker.module_symbols_same_reference(direct, symbol)?
                    }
                    _ => false,
                };
                if same_direct {
                    symbol_name = Some(raw_name.clone());
                } else {
                    let entries: Vec<(JsString, SymbolId)> = self
                        .checker
                        .table(exports)?
                        .iter()
                        .filter_map(|(name, export)| {
                            export.map(|e| (JsString::from_bytes(name), e))
                        })
                        .collect();
                    // must collect all results and sort them - exports are randomly iterated
                    let mut results: Vec<(SymbolId, JsString)> = Vec::new();
                    for (name, export) in entries {
                        if !is_late_bound_name(name.as_bytes())
                            && name.as_bytes() != ts_ast::internal_symbol_names::EXPORT_EQUALS
                            && self.checker.module_symbols_same_reference(export, symbol)?
                        {
                            results.push((export, name));
                        }
                    }
                    if !results.is_empty() {
                        let mut symbols: Vec<SymbolId> = results.iter().map(|r| r.0).collect();
                        self.checker.sort_symbols(&mut symbols)?;
                        symbol_name = results
                            .iter()
                            .find(|r| r.0 == symbols[0])
                            .map(|r| r.1.clone());
                    }
                }
            }
        }
        let symbol_name = if let Some(name) = symbol_name {
            name
        } else {
            let mut declared_name = None;
            for declaration in self.checker.symbol_declarations(symbol)?.iter().flatten() {
                let view = self.checker.ast(declaration)?;
                if let Some(name) = ts_ast::get_name_of_declaration(view, Some(declaration))? {
                    declared_name = Some((view, name));
                    break;
                }
            }
            if let Some((view, name)) = declared_name {
                let read = view.node(name)?;
                if read.kind() == K::ComputedPropertyName
                    && read
                        .expression()
                        .map(|expression| {
                            Ok::<_, Error>(ts_ast::utilities::is_entity_name(
                                &view.node(expression)?,
                            ))
                        })
                        .transpose()?
                        .unwrap_or(false)
                {
                    return Err(Error::Unsupported(
                        "createAccessFromSymbolChain: computed entity-name member",
                    ));
                }
            }
            self.symbol_name(symbol)?
        };
        self.approximate_length += symbol_name.len() + 1;

        if self.flags & ts_nodebuilder::flags::FORBID_INDEXED_ACCESS_SYMBOL_REFERENCES == 0 {
            if let Some(parent) = parent {
                if let Some(members) = self.checker.members_of_symbol(parent)? {
                    let member = self
                        .checker
                        .table(members)?
                        .get(raw_name.as_bytes())
                        .flatten();
                    let same = match member {
                        Some(member) => {
                            self.checker.module_symbols_same_reference(member, symbol)?
                        }
                        None => false,
                    };
                    if same {
                        // Should use an indexed access
                        let lhs = self.access_from_symbol_chain(
                            chain,
                            index - 1,
                            stopper,
                            override_type_arguments,
                        )?;
                        let literal = self.string_literal(symbol_name);
                        let literal = self.ast.new_literal_type_node(Some(literal));
                        if self.ast.view().node(lhs)?.kind() == K::IndexedAccessType {
                            return Ok(self
                                .ast
                                .new_indexed_access_type_node(Some(lhs), Some(literal)));
                        }
                        let object = self
                            .ast
                            .new_type_reference_node(Some(lhs), type_parameter_nodes);
                        return Ok(self
                            .ast
                            .new_indexed_access_type_node(Some(object), Some(literal)));
                    }
                }
            }
        }

        let identifier = self.ast.new_identifier(symbol_name);
        self.emit
            .add_emit_flags(identifier, ts_printer::emit_flags::NO_ASCII_ESCAPING);
        if index > stopper {
            let lhs =
                self.access_from_symbol_chain(chain, index - 1, stopper, override_type_arguments)?;
            let lhs_kind = self.ast.view().node(lhs)?.kind();
            let entity = matches!(lhs_kind.known(), Some(K::Identifier | K::QualifiedName));
            let bare = type_parameter_nodes
                .map(|list| {
                    Ok::<_, Error>(
                        self.ast
                            .view()
                            .node_slice(self.ast.view().list(list)?.nodes())?
                            .is_empty(),
                    )
                })
                .transpose()?
                .unwrap_or(true);
            if self.flags & ts_nodebuilder::flags::USE_INSTANTIATION_EXPRESSIONS == 0
                || entity && bare
            {
                return Ok(self.ast.new_qualified_name(Some(lhs), Some(identifier)));
            }
            return Err(Error::Unsupported(
                "createAccessFromSymbolChain: instantiation expression access",
            ));
        }
        Ok(identifier)
    }
}

// port: tsc/internal/tspath/path.go:PathIsRelative
fn path_is_relative(path: &[u8]) -> bool {
    path == b"."
        || path == b".."
        || path.len() >= 2 && path[0] == b'.' && matches!(path[1], b'/' | b'\\')
        || path.len() >= 3 && path[0] == b'.' && path[1] == b'.' && matches!(path[2], b'/' | b'\\')
}

// port: tsc/internal/modulespecifiers/compare.go:CountPathComponents
fn count_path_components(path: &[u8]) -> usize {
    let rest = path.strip_prefix(b"./").unwrap_or(path);
    rest.iter()
        .fold(0, |count, &byte| count + usize::from(byte == b'/'))
}
