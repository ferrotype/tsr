//! Alias identities resolve lazily through the checker's shared resolution
//! stack. Failed port operations are terminal; a resolution cycle uses Go's
//! unknown-symbol result without turning an incomplete alias into a cache hit.
use crate::{CheckerState, Error, TypeId, TypeSystemPropertyName};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{symbol_flags as sf, SyntaxKind as K};

#[derive(Default)]
pub(crate) struct ModuleAliasState {
    pub(crate) global_import_attributes: Option<TypeId>,
    pub(crate) global_import_call_options: [Option<TypeId>; 2],
    pub(crate) attributes_types: crate::types::Map<SymbolId, TypeId>,
    pub(crate) referenced: crate::types::Set<SymbolId>,
    pub(crate) exports_checked: crate::types::Set<SymbolId>,
    pub(crate) targets: crate::types::Map<SymbolId, Result<SymbolId, Error>>,
    pub(crate) immediate_targets: crate::types::Map<SymbolId, SymbolId>,
    pub(crate) type_only: crate::types::Map<SymbolId, NodeId>,
    pub(crate) resolved_exports: crate::types::Map<SymbolId, Result<ts_ast::SymbolTableId, Error>>,
    pub(crate) resolving_exports: crate::types::Set<SymbolId>,
    pub(crate) type_only_exports:
        crate::types::Map<SymbolId, crate::types::Map<ts_ast::JsString, NodeId>>,
    pub(crate) patterns: Vec<ts_ast::PatternAmbientModule>,
    pub(crate) default_only_types: crate::types::Map<TypeId, TypeId>,
    pub(crate) synthetic_types: crate::types::Map<TypeId, TypeId>,
    pub(crate) export_types: crate::types::Map<SymbolId, (SymbolId, NodeId)>,
    pub(crate) pattern_augmentations: crate::types::Map<ts_ast::JsString, SymbolId>,
    pub(crate) pattern_targets: crate::types::Map<ts_ast::JsString, SymbolId>,
}
impl ModuleAliasState {
    #[cfg(any(test, feature = "storage-pilot"))]
    pub(crate) fn type_roots(&self) -> impl Iterator<Item = TypeId> + '_ {
        self.default_only_types
            .iter()
            .chain(self.synthetic_types.iter())
            .flat_map(|(&key, &value)| [key, value])
            .chain(self.attributes_types.values().copied())
            .chain(self.global_import_attributes)
            .chain(self.global_import_call_options.into_iter().flatten())
    }
    #[cfg(any(test, feature = "storage-pilot"))]
    pub(crate) fn census(&self, census: &mut crate::census::Census) {
        census.add(
            "module_aliases",
            self.referenced.len(),
            self.referenced.allocation_size(),
        );
        census.add(
            "module_aliases",
            self.exports_checked.len(),
            self.exports_checked.allocation_size(),
        );
        census.map("module_aliases", &self.default_only_types);
        census.map("module_aliases", &self.attributes_types);
        census.map("module_aliases", &self.synthetic_types);
        census.map("module_aliases", &self.export_types);
        census.map("module_aliases", &self.targets);
        census.map("module_aliases", &self.immediate_targets);
        census.map("module_aliases", &self.type_only);
        census.map("module_aliases", &self.resolved_exports);
        census.add(
            "module_aliases",
            self.resolving_exports.len(),
            self.resolving_exports.allocation_size(),
        );
        census.map("module_aliases", &self.type_only_exports);
        census.vec_capacity("module_aliases", &self.patterns, self.patterns.capacity());
        census.map("module_aliases", &self.pattern_augmentations);
        census.map("module_aliases", &self.pattern_targets);
        for name in self
            .pattern_augmentations
            .keys()
            .chain(self.pattern_targets.keys())
        {
            census.add("module_aliases", 1, name.len());
        }
        for pattern in &self.patterns {
            census.add("module_aliases", 1, pattern.pattern.text.len());
        }
        for table in self.type_only_exports.values() {
            census.map("module_aliases", table);
            for name in table.keys() {
                census.add("module_aliases", 1, name.len());
            }
        }
    }
}
impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getExportSymbolOfValueSymbolIfExported
    pub(crate) fn get_export_symbol_of_value_symbol_if_exported(
        &self,
        symbol: SymbolId,
    ) -> Result<SymbolId, Error> {
        let read = self.symbol(symbol)?;
        let symbol = if read.flags() & sf::EXPORT_VALUE != 0 {
            read.export_symbol().unwrap_or(symbol)
        } else {
            symbol
        };
        Ok(self.get_merged_symbol(symbol))
    }
    // port: tsc/internal/checker/checker.go:Checker.getTypeOnlyAliasDeclaration
    pub(crate) fn direct_type_only_alias_declaration(
        &mut self,
        symbol: SymbolId,
    ) -> Result<Option<NodeId>, Error> {
        if self.symbol(symbol)?.flags() & sf::ALIAS == 0 {
            return Ok(None);
        }
        self.resolve_alias(symbol)?;
        Ok(self.module_aliases.type_only.get(&symbol).copied())
    }
    // port: tsc/internal/checker/checker.go:Checker.getTypeOnlyAliasDeclarationEx
    pub(crate) fn module_type_only_alias(
        &mut self,
        mut symbol: SymbolId,
        meaning: u32,
    ) -> Result<Option<NodeId>, Error> {
        while self.symbol(symbol)?.flags() & sf::ALIAS != 0
            && self.symbol(symbol)?.flags() & meaning == 0
        {
            let target = self.resolve_alias(symbol)?;
            if let Some(&declaration) = self.module_aliases.type_only.get(&symbol) {
                return Ok(Some(declaration));
            }
            symbol = target;
        }
        Ok(None)
    }
    pub(crate) fn cached_module_symbol_flags(
        &self,
        mut symbol: SymbolId,
    ) -> Result<Option<u32>, Error> {
        let mut flags = self.symbol(symbol)?.flags();
        let mut seen = crate::types::Set::default();
        while self.symbol(symbol)?.flags() & sf::ALIAS != 0 {
            let Some(&target) = self.module_aliases.targets.get(&symbol) else {
                return Ok(None);
            };
            let target = self.get_export_symbol_of_value_symbol_if_exported(target?)?;
            if target == self.builtins.unknown_symbol {
                return Ok(Some(sf::ALL));
            }
            if self.symbol(target)?.flags() & sf::ALIAS != 0 {
                if target == symbol || seen.contains(&target) {
                    break;
                }
                if seen.is_empty() {
                    seen.insert(symbol);
                }
                seen.insert(target);
            }
            flags |= self.symbol(target)?.flags();
            symbol = target;
        }
        Ok(Some(flags))
    }
    // port: tsc/internal/checker/checker.go:Checker.getSymbolFlagsEx
    pub(crate) fn module_symbol_flags(
        &mut self,
        mut symbol: SymbolId,
        exclude_type_only: bool,
        exclude_local: bool,
    ) -> Result<u32, Error> {
        let mut seen = crate::types::Set::default();
        let mut flags = if exclude_local {
            0
        } else {
            self.symbol(symbol)?.flags()
        };
        while self.symbol(symbol)?.flags() & sf::ALIAS != 0 {
            // Resolving also establishes type-only provenance for local exports.
            let resolved = self.resolve_alias(symbol)?;
            if exclude_type_only && self.module_aliases.type_only.contains_key(&symbol) {
                break;
            }
            let target = self.get_export_symbol_of_value_symbol_if_exported(resolved)?;
            if target == self.builtins.unknown_symbol {
                return Ok(sf::ALL);
            }
            let target_flags = self.symbol(target)?.flags();
            if target_flags & sf::ALIAS != 0 {
                if target == symbol || seen.contains(&target) {
                    break;
                }
                if seen.is_empty() {
                    seen.insert(symbol);
                }
                seen.insert(target);
            }
            flags |= target_flags;
            symbol = target;
        }
        Ok(flags)
    }
    // port: tsc/internal/checker/checker.go:Checker.getDeclarationOfAliasSymbol
    pub(crate) fn alias_declaration(&self, symbol: SymbolId) -> Result<NodeId, Error> {
        self.alias_declaration_or_none(symbol)?
            .ok_or(Error::MissingLink("alias declaration"))
    }
    pub(crate) fn alias_declaration_or_none(
        &self,
        symbol: SymbolId,
    ) -> Result<Option<NodeId>, Error> {
        let declarations: Vec<_> = self.symbol_declarations(symbol)?.iter().flatten().collect();
        for declaration in declarations.into_iter().rev() {
            if ts_ast::is_alias_symbol_declaration(self.ast(declaration)?, declaration)? {
                return Ok(Some(declaration));
            }
        }
        Ok(None)
    }
    // port: tsc/internal/checker/checker.go:Checker.getImmediateAliasedSymbol
    pub(crate) fn immediate_aliased_symbol(
        &mut self,
        symbol: SymbolId,
    ) -> Result<Option<SymbolId>, Error> {
        if self.symbol(symbol)?.flags() & sf::ALIAS == 0 {
            return Err(ts_arena::Error::InvalidGraph.into());
        }
        if let Some(&target) = self.module_aliases.immediate_targets.get(&symbol) {
            return Ok(Some(target));
        }
        let node = self.alias_declaration(symbol)?;
        let target = self.target_of_alias_declaration(node)?;
        if let Some(target) = target {
            self.module_aliases.immediate_targets.insert(symbol, target);
        }
        Ok(target)
    }

    // port: tsc/internal/checker/checker.go:Checker.tryResolveAlias
    pub(crate) fn try_resolve_alias(
        &mut self,
        symbol: SymbolId,
    ) -> Result<Option<SymbolId>, Error> {
        if self.module_aliases.targets.contains_key(&symbol)
            || self
                .resolution
                .find_resolution_cycle_start_index(
                    crate::TypeSystemEntity::Symbol(symbol),
                    TypeSystemPropertyName::AliasTarget,
                    |entry| self.type_resolution_has_property(entry),
                )
                .is_none()
        {
            self.resolve_alias(symbol).map(Some)
        } else {
            // Unlike pushTypeResolution, the speculative probe must not mark
            // the active chain as circular merely to reject a suggestion.
            Ok(None)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveAlias
    pub(crate) fn resolve_alias(&mut self, symbol: SymbolId) -> Result<SymbolId, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.resolve_alias_worker(symbol)
        })
    }

    fn resolve_alias_worker(&mut self, symbol: SymbolId) -> Result<SymbolId, Error> {
        if self.symbol(symbol)?.flags() & sf::ALIAS == 0 {
            return Err(ts_arena::Error::InvalidGraph.into());
        }
        if let Some(&target) = self.module_aliases.targets.get(&symbol) {
            return target;
        }
        if !self.push_source_resolution(symbol, TypeSystemPropertyName::AliasTarget) {
            return Ok(self.builtins.unknown_symbol);
        }
        let result = (|| {
            let declaration = self.alias_declaration(symbol)?;
            // Internal import-equals reports an illegal `import type` modifier
            // but does not mark that declaration as type-only. Native marking
            // belongs to the external import/export target resolvers.
            if self.ast(declaration)?.node(declaration)?.kind() != K::ImportEqualsDeclaration
                && self
                    .local_type_only_alias_declaration(declaration)?
                    .is_some()
            {
                self.module_aliases
                    .type_only
                    .entry(symbol)
                    .or_insert(declaration);
            }
            let mut target = self
                .target_of_alias_declaration(declaration)?
                .unwrap_or(self.builtins.unknown_symbol);
            if ts_ast::is_non_local_alias(
                Some(&self.symbol(target)?),
                sf::VALUE | sf::TYPE | sf::NAMESPACE,
            ) {
                let alias = target;
                let resolved = self.resolve_alias(alias)?;
                target = self.get_merged_symbol(resolved);
                if let Some(&type_only) = self.module_aliases.type_only.get(&alias) {
                    self.module_aliases
                        .type_only
                        .entry(symbol)
                        .or_insert(type_only);
                }
            }
            Ok((declaration, target))
        })();
        if let Ok((_, target)) = result {
            self.module_aliases.targets.insert(symbol, Ok(target));
        }
        let complete = self.resolution.pop();
        let target = match result {
            Ok((declaration, target)) => {
                if complete {
                    Ok(target)
                } else {
                    self.module_aliases
                        .targets
                        .insert(symbol, Ok(self.builtins.unknown_symbol));
                    let text = self.symbol_to_string(symbol)?;
                    self.error_at(
                        Some(declaration),
                        ts_diagnostics::Circular_definition_of_import_alias_0,
                        vec![text],
                    )?;
                    Ok(self.builtins.unknown_symbol)
                }
            }
            Err(error) => Err(error),
        };
        self.module_aliases.targets.insert(symbol, target);
        target
    }
    pub(crate) fn local_type_only_alias_declaration(
        &self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        match read.kind().known() {
            Some(K::ImportEqualsDeclaration) => Ok(read
                .data_source()
                .as_import_equals_declaration()
                .ok_or(ts_arena::Error::InvalidGraph)?
                .is_type_only()
                .then_some(node)),
            Some(K::ImportClause) => Ok(read.is_type_only().then_some(node)),
            Some(K::NamespaceImport | K::ImportSpecifier) => {
                if read.kind() == K::ImportSpecifier && read.is_type_only() {
                    return Ok(Some(node));
                }
                let parent = read
                    .parent()
                    .ok_or(Error::MissingLink("type-only import parent"))?;
                let clause = if self.ast(parent)?.node(parent)?.kind() == K::ImportClause {
                    parent
                } else {
                    self.ast(parent)?
                        .node(parent)?
                        .parent()
                        .ok_or(Error::MissingLink("type-only import clause"))?
                };
                Ok(self
                    .ast(clause)?
                    .node(clause)?
                    .is_type_only()
                    .then_some(node))
            }
            Some(K::NamespaceExport) => {
                let parent = read
                    .parent()
                    .ok_or(Error::MissingLink("type-only namespace export"))?;
                Ok(self
                    .ast(parent)?
                    .node(parent)?
                    .is_type_only()
                    .then_some(node))
            }
            Some(K::ExportSpecifier) => {
                if read
                    .data_source()
                    .as_export_specifier()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .is_type_only()
                {
                    return Ok(Some(node));
                }
                let list = read
                    .parent()
                    .ok_or(Error::MissingLink("type-only export list"))?;
                let declaration = self
                    .ast(list)?
                    .node(list)?
                    .parent()
                    .ok_or(Error::MissingLink("type-only export declaration"))?;
                Ok(self
                    .ast(declaration)?
                    .node(declaration)?
                    .data_source()
                    .as_export_declaration()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .is_type_only()
                    .then_some(declaration))
            }
            _ => Ok(None),
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.checkAndReportErrorForResolvingImportAliasToTypeOnlySymbol
    fn check_import_alias_type_only_reference(&mut self, reference: NodeId) -> Result<(), Error> {
        let mut name = reference;
        loop {
            let target = self.resolve_entity_name_ex(
                name,
                sf::VALUE | sf::TYPE | sf::NAMESPACE,
                true,
                true,
            )?;
            let declaration = if let Some(symbol) = target {
                if self.symbol(symbol)?.flags() & sf::ALIAS != 0 {
                    self.resolve_alias(symbol)?;
                    self.module_aliases.type_only.get(&symbol).copied()
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(declaration) = declaration {
                let read = self.ast(declaration)?.node(declaration)?;
                let export = matches!(
                    read.kind().known(),
                    Some(K::ExportSpecifier | K::ExportDeclaration)
                );
                let text = if read.kind() == K::ExportDeclaration {
                    ts_ast::JsString::from_bytes(b"*".as_slice())
                } else {
                    let name = read
                        .name()
                        .ok_or(Error::MissingLink("type-only import target name"))?;
                    self.ast(name)?.node_text(name)?.into_js_string()
                };
                if let Some(diagnostic) = self.error_at(Some(reference), if export {
                    ts_diagnostics::An_import_alias_cannot_reference_a_declaration_that_was_exported_using_export_type
                } else {
                    ts_diagnostics::An_import_alias_cannot_reference_a_declaration_that_was_imported_using_import_type
                }, vec![])? {
                    let related = self.diagnostic_for_node(Some(declaration), if export {
                        ts_diagnostics::X_0_was_exported_here
                    } else {
                        ts_diagnostics::X_0_was_imported_here
                    }, vec![text])?;
                    self.add_related_diagnostic(diagnostic, related)?;
                }
                return Ok(());
            }
            let read = self.ast(name)?.node(name)?;
            if read.kind() == K::Identifier {
                return Ok(());
            }
            name = read
                .data_source()
                .as_qualified_name()
                .and_then(|data| data.left())
                .ok_or(Error::MissingLink("type-only import qualifier"))?;
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.getTargetOfAliasDeclaration
    pub(crate) fn target_of_alias_declaration(
        &mut self,
        node: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        // A binding element is an alias only when its root declaration is
        // initialized to `require`; upstream routes both through the import
        // specifier path. Decide that before borrowing the node view.
        let binding_require = self.ast(node)?.node(node)?.kind() == K::BindingElement
            && self.require_module_specifier(node)?.is_some();
        let read = self.ast(node)?.node(node)?;
        match read.kind().known() {
            Some(
                K::ImportClause | K::NamespaceImport | K::NamespaceExport | K::ImportSpecifier,
            ) => self.target_of_external_alias(node),
            // Other destructured shapes keep the named boundary below.
            Some(K::BindingElement) if binding_require => self.target_of_external_alias(node),
            Some(K::ImportEqualsDeclaration) => {
                let reference = read
                    .data_source()
                    .as_import_equals_declaration()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .module_reference()
                    .ok_or(Error::MissingLink("import alias reference"))?;
                if self.ast(reference)?.node(reference)?.kind() == K::ExternalModuleReference {
                    return self.target_of_external_alias(node);
                }
                let meaning = if self.ast(reference)?.node(reference)?.kind() == K::Identifier {
                    sf::NAMESPACE
                } else {
                    sf::VALUE | sf::TYPE | sf::NAMESPACE
                };
                let target = self.resolve_entity_name_ex(reference, meaning, false, true)?;
                self.check_import_alias_type_only_reference(reference)?;
                Ok(target)
            }
            Some(K::ExportSpecifier) => {
                let data = read
                    .data_source()
                    .as_export_specifier()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                let name = data
                    .property_name()
                    .or(data.name())
                    .ok_or(Error::MissingLink("export alias name"))?;
                let type_only = data.is_type_only();
                let parent = read
                    .parent()
                    .ok_or(Error::MissingLink("named export parent"))?;
                let declaration = self
                    .ast(parent)?
                    .node(parent)?
                    .parent()
                    .ok_or(Error::MissingLink("export declaration"))?;
                let declaration_read = self.ast(declaration)?.node(declaration)?;
                let declaration_data = declaration_read
                    .data_source()
                    .as_export_declaration()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                if declaration_data.module_specifier().is_some() {
                    return self.target_of_external_alias(node);
                }
                let declaration_type_only = declaration_data.is_type_only();
                let target = if self.ast(name)?.node(name)?.kind() == K::StringLiteral {
                    None
                } else {
                    self.resolve_entity_name_ex(
                        name,
                        sf::VALUE | sf::TYPE | sf::NAMESPACE,
                        false,
                        true,
                    )?
                };
                if type_only || declaration_type_only {
                    let symbol = self
                        .get_symbol_of_declaration(node)?
                        .ok_or(Error::MissingLink("type-only export symbol"))?;
                    self.module_aliases.type_only.entry(symbol).or_insert(node);
                }
                Ok(target)
            }
            Some(K::ExportAssignment) => {
                if self.contained_by_namespace(node)? {
                    return Ok(None);
                }
                let expression = read
                    .expression()
                    .ok_or(Error::MissingLink("export assignment expression"))?;
                let target = self.target_of_alias_like_expression(expression)?;
                self.mark_module_alias_type_only(node, None)?;
                Ok(target)
            }
            Some(K::BinaryExpression) => {
                let expression = read
                    .data_source()
                    .as_binary_expression()
                    .and_then(|data| data.right())
                    .ok_or(Error::MissingLink("alias assignment right"))?;
                let target = self.target_of_alias_like_expression(expression)?;
                self.mark_module_alias_type_only(node, None)?;
                Ok(target)
            }
            Some(K::PropertyAssignment) => self.target_of_alias_like_expression(
                read.initializer()
                    .ok_or(Error::MissingLink("alias property initializer"))?,
            ),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                self.target_of_alias_access(node)
            }
            Some(K::NamespaceExportDeclaration) => {
                self.target_of_namespace_export_declaration(node)
            }
            Some(K::VariableDeclaration) => self.target_of_require_variable(node),
            Some(K::ShorthandPropertyAssignment) => self.resolve_entity_name_ex(
                read.name()
                    .ok_or(Error::MissingLink("shorthand alias name"))?,
                sf::VALUE | sf::TYPE | sf::NAMESPACE,
                true,
                true,
            ),
            _ => Err(Error::Unsupported(
                "getTargetOfAliasDeclaration: external or CommonJS alias",
            )),
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.getTypeOfAlias
    pub(crate) fn type_of_alias(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        if let Some(ty) = self
            .value_symbol_links
            .try_get(symbol)
            .and_then(|links| links.resolved_type)
        {
            return Ok(ty);
        }
        if !self.push_source_resolution(symbol, TypeSystemPropertyName::Type) {
            return Ok(self.builtins.error_type);
        }
        // Recursive alias queries must not overwrite this invocation's target
        // for reportCircularityError (exportSymbol is a local upstream).
        let mut export = None;
        let result: Result<TypeId, Error> = (|| {
            let target = self.resolve_alias(symbol)?;
            export = match self.alias_declaration_or_none(symbol)? {
                Some(declaration) => self.target_of_alias_declaration(declaration)?,
                None => None,
            };
            if let Some(ty) = self
                .value_symbol_links
                .try_get(symbol)
                .and_then(|links| links.resolved_type)
            {
                return Ok(ty);
            }
            let ty = if self.module_symbol_flags(target, false, false)? & sf::VALUE != 0 {
                self.get_type_of_symbol(target)?
            } else {
                self.builtins.error_type
            };
            self.value_symbol_links.get_or_default(symbol).resolved_type = Some(ty);
            Ok(ty)
        })();
        let complete = self.resolution.pop();
        let ty = result?;
        if !complete {
            self.report_symbol_circularity(export.unwrap_or(symbol))?;
            let links = self.value_symbol_links.get_or_default(symbol);
            if links.resolved_type.is_none() {
                links.resolved_type = Some(self.builtins.error_type);
            }
            return Ok(links.resolved_type.unwrap_or(ty));
        }
        Ok(ty)
    }
    // port: tsc/internal/checker/checker.go:isContainedByNamespace
    pub(crate) fn contained_by_namespace(&self, node: NodeId) -> Result<bool, Error> {
        let mut container = self
            .ast(node)?
            .node(node)?
            .parent()
            .ok_or(Error::MissingLink("module container"))?;
        if self.ast(container)?.node(container)?.kind() != K::SourceFile {
            container = self
                .ast(container)?
                .node(container)?
                .parent()
                .ok_or(Error::MissingLink("module declaration container"))?;
        }
        Ok(
            self.ast(container)?.node(container)?.kind() == K::ModuleDeclaration
                && !ts_ast::is_ambient_module(self.ast(container)?, container)?,
        )
    }
}
