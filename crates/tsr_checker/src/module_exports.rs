//! Export-star traversal has query-local cycle tracking. Published export maps
//! and type-only provenance belong to the checker, never to bound symbols.
use crate::{CheckerState, Error};
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{
    internal_symbol_names as names, symbol_flags as sf, JsString, SymbolTable, SymbolTableId,
};

#[derive(Default)]
struct ExportTraversal {
    visited: crate::types::Set<SymbolId>,
    /// The export tables visited other than through a type-only star; their
    /// names cancel type-only provenance. Go collects the names into a set
    /// up front; the tables are immutable, so reading them at the end when
    /// (rarely) a type-only star recorded anything is the same set.
    non_type_only: Vec<SymbolTableId>,
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
            && tsr_ast::is_non_local_alias(
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
        if let Some(result) = self.module_aliases.resolved_exports.get(&symbol).cloned() {
            return result;
        }
        if !self.module_aliases.resolving_exports.insert(symbol) {
            return Err(Error::Unsupported(
                "getExportsOfModule: recursive alias-backed export map",
            ));
        }
        let result = self.module_exports_worker(symbol);
        self.module_aliases.resolving_exports.remove(&symbol);
        self.module_aliases
            .resolved_exports
            .insert(symbol, result.clone());
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
        let exports = match resolved {
            Some(resolved) => self.visit_module_exports(resolved, None, false, &mut traversal)?,
            None => None,
        };
        let exports = match exports {
            Some(exports) => exports,
            None => self.tables.alloc(SymbolTable::default()),
        };
        if let Some(original) = original {
            if let Some(table) = self.symbol(original)?.exports() {
                let (bytes, entries) = self.collect_table_entries(table)?;
                if entries.len() > 1 {
                    for (range, current) in entries {
                        let Some(current) = current else { continue };
                        let name = &bytes[range];
                        if name == names::EXPORT_EQUALS || name == names::EXPORT_STAR {
                            continue;
                        }
                        let flags = self.module_symbol_flags(current, false, false)?;
                        if flags & (sf::TYPE | sf::NAMESPACE) != 0
                            && flags & sf::VALUE == 0
                            && self.table(exports)?.get(name).flatten().is_none()
                        {
                            self.tables
                                .get_mut(exports)?
                                .insert_bytes(name, Some(current));
                        }
                    }
                }
            }
        }
        if !traversal.type_only.is_empty() {
            for table in traversal.non_type_only {
                for (name, _) in self.table(table)? {
                    traversal.type_only.remove(name);
                }
            }
        }
        self.module_aliases
            .type_only_exports
            .insert(symbol, traversal.type_only);
        Ok(exports)
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
    ) -> Result<Option<SymbolTableId>, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let table = self.symbol(symbol)?.exports();
            if !is_type_only {
                if let Some(table) = table {
                    traversal.non_type_only.push(table);
                }
            }
            let Some(table) = table else { return Ok(None) };
            if !traversal.visited.insert(symbol) {
                return Ok(None);
            }
            // Go's `maps.Clone(symbol.Exports)`: a checker-owned copy the
            // star exports extend.
            let symbols = self
                .clone_symbol_table(Some(table))?
                .expect("exports table cloned");
            if let Some(stars) = self.table(symbols)?.get(names::EXPORT_STAR).flatten() {
                let nested = self.tables.alloc(SymbolTable::default());
                let mut collisions = crate::types::Map::default();
                for declaration in self
                    .symbol_declarations(stars)?
                    .to_vec()
                    .into_iter()
                    .flatten()
                {
                    let read = self.node(declaration)?;
                    let data = read
                        .data_source()
                        .as_export_declaration()
                        .ok_or(tsr_arena::Error::InvalidGraph)?;
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
                                nested,
                                exported,
                                Some((&mut collisions, declaration)),
                            )?;
                        }
                    }
                }
                for (name, collision) in collisions {
                    if name.as_bytes() == names::EXPORT_EQUALS
                        || collision.duplicates.is_empty()
                        || self
                            .table(symbols)?
                            .get(name.as_bytes())
                            .flatten()
                            .is_some()
                    {
                        continue;
                    }
                    for declaration in collision.duplicates {
                        self.error_at(Some(declaration),tsr_diagnostics::Module_0_has_already_exported_a_member_named_1_Consider_explicitly_re_exporting_to_resolve_the_ambiguity,vec![collision.specifier.clone(),name.clone()])?;
                    }
                }
                self.extend_module_exports(symbols, nested, None)?;
            }
            if let Some(star) = export_star {
                if self.node(star)?.is_type_only() {
                    for (name, _) in self.table(symbols)? {
                        traversal.type_only.insert(JsString::from_bytes(name), star);
                    }
                }
            }
            Ok(Some(symbols))
        })
    }
    // port: tsc/internal/checker/checker.go:Checker.extendExportSymbols
    fn extend_module_exports(
        &mut self,
        target: SymbolTableId,
        source: SymbolTableId,
        mut collision: Option<(&mut crate::types::Map<JsString, ExportCollision>, NodeId)>,
    ) -> Result<(), Error> {
        let (bytes, entries) = self.collect_table_entries(source)?;
        for (range, value) in entries {
            let name = &bytes[range];
            if name == names::DEFAULT {
                continue;
            }
            let current = self.table(target)?.get(name).flatten();
            if current.is_none() {
                self.tables.get_mut(target)?.insert_bytes(name, value);
                if let Some((table, node)) = &mut collision {
                    let specifier = self
                        .module_specifier(*node)?
                        .ok_or(Error::MissingLink("collision export specifier"))?;
                    table.insert(
                        JsString::from_bytes(name),
                        ExportCollision {
                            specifier: tsr_scanner::get_text_of_node(
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
                        .get_mut(name)
                        .ok_or(Error::MissingLink("export collision entry"))?
                        .duplicates
                        .push(*node);
                }
            }
        }
        Ok(())
    }
}
