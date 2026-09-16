//! Export-star traversal has query-local cycle tracking. Published export maps
//! and type-only provenance belong to the checker, never to bound symbols.
use crate::{CheckerState, Error};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    internal_symbol_names as names, symbol_flags as sf, JsString, SymbolTable, SymbolTableId,
};

#[derive(Default)]
struct ExportTraversal {
    visited: crate::types::Set<SymbolId>,
    non_type_only: crate::types::Set<JsString>,
    type_only: crate::types::Map<JsString, NodeId>,
}
#[derive(Default)]
struct ExportCollision {
    specifier: JsString,
    duplicates: Vec<NodeId>,
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.resolveSymbolEx
    pub(crate) fn resolve_module_symbol(
        &mut self,
        symbol: Option<SymbolId>,
        dont_resolve_alias: bool,
    ) -> Result<Option<SymbolId>, Error> {
        let Some(symbol) = symbol else {
            return Ok(None);
        };
        if !dont_resolve_alias
            && ts_ast::is_non_local_alias(
                Some(&self.symbol(symbol)?),
                sf::VALUE | sf::TYPE | sf::NAMESPACE,
            )
        {
            self.resolve_alias(symbol).map(Some)
        } else {
            Ok(Some(symbol))
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.resolveExternalModuleSymbol
    pub(crate) fn resolve_external_module_symbol(
        &mut self,
        module: Option<SymbolId>,
        dont_resolve_alias: bool,
    ) -> Result<Option<SymbolId>, Error> {
        let Some(module) = module else {
            return Ok(None);
        };
        let export = self.member_symbol(self.symbol(module)?.exports(), names::EXPORT_EQUALS)?;
        if let Some(export) = self.resolve_module_symbol(export, dont_resolve_alias)? {
            return Ok(Some(self.get_merged_symbol(export)));
        }
        Ok(Some(module))
    }
    // port: tsc/internal/checker/checker.go:Checker.getExportsOfSymbol
    pub(crate) fn module_exports_of_symbol(
        &mut self,
        symbol: SymbolId,
    ) -> Result<Option<SymbolTableId>, Error> {
        if self.symbol(symbol)?.flags() & sf::LATE_BINDING_CONTAINER != 0 {
            self.resolved_members_or_exports(symbol, true)
        } else if self.symbol(symbol)?.flags() & sf::MODULE != 0 {
            self.module_exports(symbol).map(Some)
        } else {
            Ok(self.symbol(symbol)?.exports())
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.getExportsOfModule
    pub(crate) fn module_exports(&mut self, symbol: SymbolId) -> Result<SymbolTableId, Error> {
        if let Some(&result) = self.module_aliases.resolved_exports.get(&symbol) {
            return result;
        }
        if !self.module_aliases.resolving_exports.insert(symbol) {
            return Err(Error::Unsupported(
                "getExportsOfModule: recursive alias-backed export map",
            ));
        }
        let result = self.module_exports_worker(symbol);
        self.module_aliases.resolving_exports.remove(&symbol);
        self.module_aliases.resolved_exports.insert(symbol, result);
        result
    }
    // port: tsc/internal/checker/checker.go:Checker.getExportsOfModuleWorker
    pub(crate) fn module_exports_worker(
        &mut self,
        symbol: SymbolId,
    ) -> Result<SymbolTableId, Error> {
        let mut traversal = ExportTraversal::default();
        let raw = self.symbol(symbol)?.exports();
        let export_equals = self.member_symbol(raw, names::EXPORT_EQUALS)?;
        let original = self
            .resolve_module_symbol(export_equals, false)?
            .map(|_| symbol);
        let resolved = self.resolve_external_module_symbol(Some(symbol), false)?;
        let mut exports = match resolved {
            Some(resolved) => self
                .visit_module_exports(resolved, None, false, &mut traversal)?
                .unwrap_or_default(),
            None => SymbolTable::default(),
        };
        if let Some(original) = original {
            if let Some(table) = self.symbol(original)?.exports() {
                let entries = self.module_table_entries(table)?;
                if entries.len() > 1 {
                    for (_, current) in entries {
                        let Some(current) = current else { continue };
                        let name = self.symbol(current)?.name_to_owned();
                        if name.as_bytes() == names::EXPORT_EQUALS
                            || name.as_bytes() == names::EXPORT_STAR
                        {
                            continue;
                        }
                        let flags = self.module_symbol_flags(current, false, false)?;
                        if flags & (sf::TYPE | sf::NAMESPACE) != 0
                            && flags & sf::VALUE == 0
                            && exports.get(name.as_bytes()).copied().flatten().is_none()
                        {
                            exports.insert(name, Some(current));
                        }
                    }
                }
            }
        }
        for name in traversal.non_type_only {
            traversal.type_only.remove(&name);
        }
        self.module_aliases
            .type_only_exports
            .insert(symbol, traversal.type_only);
        Ok(self.alloc_symbol_table(exports))
    }
    pub(crate) fn module_table_entries(
        &self,
        table: SymbolTableId,
    ) -> Result<Vec<(JsString, Option<SymbolId>)>, Error> {
        Ok(self
            .table(table)?
            .into_iter()
            .map(|(name, value)| (JsString::from_bytes(name), value))
            .collect())
    }
    fn visit_module_exports(
        &mut self,
        symbol: SymbolId,
        export_star: Option<NodeId>,
        is_type_only: bool,
        traversal: &mut ExportTraversal,
    ) -> Result<Option<SymbolTable>, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let table = self.symbol(symbol)?.exports();
            if !is_type_only {
                if let Some(table) = table {
                    for (name, _) in self.table(table)? {
                        traversal.non_type_only.insert(JsString::from_bytes(name));
                    }
                }
            }
            let Some(table) = table else { return Ok(None) };
            if !traversal.visited.insert(symbol) {
                return Ok(None);
            }
            let mut symbols: SymbolTable = self
                .table(table)?
                .into_iter()
                .map(|(name, value)| (JsString::from_bytes(name), value))
                .collect();
            if let Some(stars) = symbols.get(names::EXPORT_STAR).copied().flatten() {
                let mut nested = SymbolTable::default();
                let mut collisions = crate::types::Map::default();
                for declaration in self
                    .symbol_declarations(stars)?
                    .to_vec()
                    .into_iter()
                    .flatten()
                {
                    let read = self.ast(declaration)?.node(declaration)?;
                    let data = read
                        .data_source()
                        .as_export_declaration()
                        .ok_or(ts_arena::Error::InvalidGraph)?;
                    let name = data
                        .module_specifier()
                        .ok_or(Error::MissingLink("export-star module specifier"))?;
                    let type_only = data.is_type_only();
                    // The attributes type travels with the specifier (`import_attributes_type_for_specifier`).
                    let resolved = self.resolve_external_module_name(declaration, name, false)?;
                    if let Some(resolved) = resolved {
                        if let Some(exported) = self.visit_module_exports(
                            resolved,
                            Some(declaration),
                            is_type_only || type_only,
                            traversal,
                        )? {
                            self.extend_module_exports(
                                &mut nested,
                                exported,
                                Some((&mut collisions, declaration)),
                            )?;
                        }
                    }
                }
                for (name, collision) in collisions {
                    if name.as_bytes() == names::EXPORT_EQUALS
                        || collision.duplicates.is_empty()
                        || symbols.get(name.as_bytes()).copied().flatten().is_some()
                    {
                        continue;
                    }
                    for declaration in collision.duplicates {
                        self.error_at(Some(declaration),ts_diagnostics::Module_0_has_already_exported_a_member_named_1_Consider_explicitly_re_exporting_to_resolve_the_ambiguity,vec![collision.specifier.clone(),name.clone()])?;
                    }
                }
                self.extend_module_exports(&mut symbols, nested, None)?;
            }
            if let Some(star) = export_star {
                if self.ast(star)?.node(star)?.is_type_only() {
                    for name in symbols.keys() {
                        traversal.type_only.insert(name.clone(), star);
                    }
                }
            }
            Ok(Some(symbols))
        })
    }
    // port: tsc/internal/checker/checker.go:Checker.extendExportSymbols
    fn extend_module_exports(
        &mut self,
        target: &mut SymbolTable,
        source: SymbolTable,
        mut collision: Option<(&mut crate::types::Map<JsString, ExportCollision>, NodeId)>,
    ) -> Result<(), Error> {
        for (name, value) in source {
            if name.as_bytes() == names::DEFAULT {
                continue;
            }
            let current = target.get(name.as_bytes()).copied().flatten();
            if current.is_none() {
                target.insert(name.clone(), value);
                if let Some((table, node)) = &mut collision {
                    let specifier = self
                        .module_specifier(*node)?
                        .ok_or(Error::MissingLink("collision export specifier"))?;
                    table.insert(
                        name,
                        ExportCollision {
                            specifier: ts_scanner::get_text_of_node(
                                self.ast(specifier)?,
                                specifier,
                            )?,
                            duplicates: Vec::new(),
                        },
                    );
                }
            } else if let Some((table, node)) = &mut collision {
                if self.resolve_module_symbol(current, false)?
                    != self.resolve_module_symbol(value, false)?
                {
                    table
                        .get_mut(&name)
                        .ok_or(Error::MissingLink("export collision entry"))?
                        .duplicates
                        .push(*node);
                }
            }
        }
        Ok(())
    }
}
