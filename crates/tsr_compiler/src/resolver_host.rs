//! Borrowed program views plus one explicit operation-owned transient arena.
use crate::{Program, ProgramFile};
use std::collections::HashMap;
use ts_arena::{ArenaId, Counters, Error, SymbolArena, SymbolId};
use ts_ast::{
    AstView, DeclarationRead, NodeBinding, NodeId, Symbol, SymbolFlags, SymbolRef, SymbolTableId,
    SymbolTableRead,
};
use ts_binder::name_resolver::ResolverHost;
use ts_jsstring::JsString;
#[derive(Default)]
pub(crate) struct OwnerIndex {
    nodes: HashMap<ArenaId, usize>,
    symbols: HashMap<ArenaId, usize>,
    tables: HashMap<ArenaId, usize>,
}
impl OwnerIndex {
    /// Finds the retained file owning a node without creating a resolver scope.
    pub(crate) fn node_file_index(&self, node: NodeId) -> Option<usize> {
        self.nodes.get(&node.arena()).copied()
    }

    pub(crate) fn from_files(files: &[std::sync::Arc<ProgramFile>]) -> Self {
        let mut index = Self::default();
        for (i, file) in files.iter().enumerate() {
            let view = file.bound().view();
            index.nodes.insert(file.source().arena(), i);
            index.symbols.insert(view.result().symbols().id(), i);
            index.tables.insert(view.result().tables().id(), i);
        }
        index
    }
}
/// Borrows its program, so none of its reads clone a file owner. Only explicitly
/// requested transient symbols allocate in this scope; they cannot mutate a
/// file's published BindResult. This is an environment, not a checker substitute.
pub struct ProgramResolverHost<'a> {
    program: &'a Program,
    transient: SymbolArena<Symbol>,
}
impl Program {
    pub fn resolver_host(&self, counters: &Counters) -> ProgramResolverHost<'_> {
        ProgramResolverHost {
            program: self,
            transient: SymbolArena::new(counters),
        }
    }
}
impl ResolverHost for ProgramResolverHost<'_> {
    fn ast(&self, node: NodeId) -> Result<AstView<'_>, Error> {
        let &i = self
            .program
            .owners
            .nodes
            .get(&node.arena())
            .ok_or(Error::WrongOwner)?;
        let view = self.program.files()[i].bound().view().ast();
        view.node(node)?;
        Ok(view)
    }
    fn binding(&self, node: NodeId) -> Result<Option<NodeBinding>, Error> {
        let &i = self
            .program
            .owners
            .nodes
            .get(&node.arena())
            .ok_or(Error::WrongOwner)?;
        self.program.files()[i].bound().view().node_binding(node)
    }
    fn symbol(&self, symbol: SymbolId) -> Result<SymbolRef<'_>, Error> {
        if symbol.arena() == self.transient.id() {
            return self.transient.get(symbol).map(SymbolRef::Owned);
        }
        let &i = self
            .program
            .owners
            .symbols
            .get(&symbol.arena())
            .ok_or(Error::WrongOwner)?;
        self.program.files()[i]
            .bound()
            .view()
            .symbol(symbol)
            .map(SymbolRef::Stored)
    }
    fn table(&self, table: SymbolTableId) -> Result<SymbolTableRead<'_>, Error> {
        let &i = self
            .program
            .owners
            .tables
            .get(&table.arena())
            .ok_or(Error::WrongOwner)?;
        self.program.files()[i]
            .bound()
            .view()
            .result()
            .tables()
            .get(table)
    }
    fn declarations(&self, symbol: SymbolId) -> Result<DeclarationRead<'_>, Error> {
        if symbol.arena() == self.transient.id() {
            self.transient.get(symbol)?;
            return Ok(DeclarationRead::empty(self.transient.id()));
        }
        let &i = self
            .program
            .owners
            .symbols
            .get(&symbol.arena())
            .ok_or(Error::WrongOwner)?;
        let view = self.program.files()[i].bound().view();
        view.result()
            .declarations()
            .get(view.symbol(symbol)?.declarations())
    }
    fn new_transient_symbol(
        &mut self,
        flags: SymbolFlags,
        name: JsString,
    ) -> Result<SymbolId, Error> {
        Ok(self.transient.push(Symbol::new(flags, name)))
    }
}
