//! Scope tables for name serialization. A table descriptor keeps its provenance
//! when filtering class type parameters or exposing a class-expression name.
use super::{NameTable, NameTableId};
use crate::node_builder::NodeBuilder;
use crate::Error;
use ts_arena::{NodeId, SymbolId};
use ts_ast::{symbol_flags as sf, SymbolTableId, SyntaxKind as K};

impl NodeBuilder<'_> {
    fn name_table(id: NameTableId, table: Option<SymbolTableId>) -> NameTable {
        NameTable {
            id,
            table,
            singleton: None,
        }
    }

    // port: tsc/internal/checker/symbolaccessibility.go:Checker.someSymbolTableInScope
    pub(super) fn some_name_scope(
        &mut self,
        enclosing: Option<NodeId>,
        mut callback: impl FnMut(&mut Self, NameTable, Option<NodeId>) -> Result<bool, Error>,
    ) -> Result<bool, Error> {
        let mut location = enclosing;
        while let Some(node) = location {
            let read = self.checker.ast(node)?.node(node)?;
            let kind = read.kind();
            let parent = read.parent();
            let global_source = kind == K::SourceFile
                && !ts_ast::utilities::is_external_or_common_js_module(
                    &self.checker.ast(node)?.source_file(node)?,
                );
            let locals = self
                .checker
                .checker_node_binding(node)?
                .and_then(|binding| binding.locals);
            if locals.is_some() && !global_source {
                let table = Self::name_table(NameTableId::Locals(node), locals);
                if callback(self, table, Some(node))? {
                    return Ok(true);
                }
            }
            match kind.known() {
                Some(K::SourceFile | K::ModuleDeclaration) if !global_source => {
                    if self.checker.ast(node)?.node(node)?.flags() & ts_ast::node_flags::REPARSED
                        != 0
                    {
                        return Err(Error::Unsupported(
                            "someSymbolTableInScope: reparsed module",
                        ));
                    }
                    let symbol = self
                        .checker
                        .get_symbol_of_declaration(node)?
                        .ok_or(Error::MissingLink("name scope module symbol"))?;
                    let table = Self::name_table(
                        NameTableId::Exports(symbol),
                        self.checker.symbol(symbol)?.exports(),
                    );
                    if callback(self, table, Some(node))? {
                        return Ok(true);
                    }
                }
                Some(K::ClassDeclaration | K::ClassExpression | K::InterfaceDeclaration) => {
                    let symbol = self
                        .checker
                        .get_symbol_of_declaration(node)?
                        .ok_or(Error::MissingLink("name scope class symbol"))?;
                    let table = Self::name_table(
                        NameTableId::Members(symbol),
                        self.checker.symbol(symbol)?.members(),
                    );
                    if !self.name_table_symbols(&table)?.is_empty()
                        && callback(self, table, Some(node))?
                    {
                        return Ok(true);
                    }
                    if kind == K::ClassExpression {
                        if let Some(name) = self.checker.ast(node)?.node(node)?.name() {
                            let name = self.checker.ast(name)?.node_text(name)?.into_js_string();
                            if !name.is_empty() {
                                let table = NameTable {
                                    id: NameTableId::Locals(node),
                                    table: None,
                                    singleton: Some((name, symbol)),
                                };
                                if callback(self, table, Some(node))? {
                                    return Ok(true);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            location = parent;
        }
        let table = Self::name_table(NameTableId::Globals, self.checker.builtins.globals);
        callback(self, table, None)
    }

    pub(super) fn name_table_lookup(
        &self,
        table: &NameTable,
        name: &[u8],
    ) -> Result<Option<SymbolId>, Error> {
        let value = if let Some((key, value)) = &table.singleton {
            (key.as_bytes() == name).then_some(*value)
        } else {
            table
                .table
                .map(|table| {
                    self.checker
                        .table(table)
                        .map(|read| read.get(name).flatten())
                })
                .transpose()?
                .flatten()
        };
        if let Some(value) = value {
            if matches!(table.id, NameTableId::Members(_))
                && self.checker.symbol(value)?.flags() & (sf::TYPE & !sf::ASSIGNMENT) == 0
            {
                return Ok(None);
            }
        }
        Ok(value)
    }

    /// The symbols of a name table in table order (a members table keeps only
    /// its type members). No caller needs the names, and copying every name of
    /// the globals table per lookup was most of the display path's allocation.
    pub(super) fn name_table_symbols(&self, table: &NameTable) -> Result<Vec<SymbolId>, Error> {
        if let Some((_, value)) = &table.singleton {
            return Ok(vec![*value]);
        }
        let Some(id) = table.table else {
            return Ok(vec![]);
        };
        let read = self.checker.table(id)?;
        let mut result = Vec::with_capacity(read.len());
        for (_, value) in read {
            if let Some(value) = value {
                if matches!(table.id, NameTableId::Members(_))
                    && self.checker.symbol(value)?.flags() & (sf::TYPE & !sf::ASSIGNMENT) == 0
                {
                    continue;
                }
                result.push(value);
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/symbolaccessibility.go:Checker.needsQualification
    pub(super) fn name_needs_qualification(
        &mut self,
        symbol: SymbolId,
        enclosing: Option<NodeId>,
        meaning: u32,
    ) -> Result<bool, Error> {
        let name = self.checker.symbol(symbol)?.name_to_owned();
        let mut qualify = false;
        self.some_name_scope(enclosing, |this, table, _| {
            let Some(found) = this.name_table_lookup(&table, name.as_bytes())? else {
                return Ok(false);
            };
            let mut found = this.checker.get_merged_symbol(found);
            if found == symbol {
                return Ok(true);
            }
            let alias = this.checker.symbol(found)?.flags() & sf::ALIAS != 0
                && !this.name_has_declaration_kind(found, K::ExportSpecifier)?;
            let mut flags = this.checker.symbol(found)?.flags();
            if alias {
                found = this.checker.resolve_alias(found)?;
                flags = this.checker.module_symbol_flags(found, false, false)?;
            }
            if flags & meaning != 0 {
                qualify = true;
                return Ok(true);
            }
            Ok(false)
        })?;
        Ok(qualify)
    }
}
