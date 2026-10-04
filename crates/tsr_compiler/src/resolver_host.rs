//! Borrowed program views plus one explicit operation-owned transient arena.
use crate::{Program, ProgramFile};
use std::collections::HashMap;
use tsr_arena::{ArenaId, Counters, Error, SymbolArena, SymbolId};
use tsr_ast::{
    AstView, DeclarationRead, NodeBinding, NodeId, Symbol, SymbolFlags, SymbolRef, SymbolTableId,
    SymbolTableRead,
};
use tsr_binder::name_resolver::ResolverHost;
use tsr_jsstring::JsString;
#[derive(Default)]
pub(crate) struct OwnerIndex {
    nodes: HashMap<ArenaId, usize>,
    // Lazy tokens/JSDoc nodes can be created after program publication.
    // Each retained owner has fixed arena namespaces, bounding this cache;
    // core reads remain lock-free.
    secondary_nodes: std::sync::RwLock<SecondaryNodes>,
    symbols: HashMap<ArenaId, usize>,
    tables: HashMap<ArenaId, usize>,
}
#[derive(Default)]
struct SecondaryNodes {
    owners: HashMap<ArenaId, usize>,
    // One negative entry avoids repeated scans for a transient display owner
    // without retaining an unbounded set of foreign IDs supplied by callers.
    last_miss: Option<ArenaId>,
}
impl OwnerIndex {
    /// Finds the retained file owning a node without creating a resolver scope.
    pub(crate) fn node_file_index(&self, node: NodeId) -> Option<usize> {
        self.nodes.get(&node.arena()).copied()
    }

    pub(crate) fn retained_node_file_index(
        &self,
        files: &[std::sync::Arc<ProgramFile>],
        node: NodeId,
    ) -> Option<usize> {
        if let Some(index) = self.node_file_index(node) {
            return Some(index);
        }
        {
            let secondary = self
                .secondary_nodes
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(&index) = secondary.owners.get(&node.arena()) {
                return Some(index);
            }
            if secondary.last_miss == Some(node.arena()) {
                return None;
            }
        }
        let index = files
            .iter()
            .position(|file| file.bound().view().ast().retains_arena(node.arena()));
        let mut secondary = self
            .secondary_nodes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = index {
            secondary.owners.insert(node.arena(), index);
        } else {
            // Retained file owners are fixed for this program. Lazy node IDs
            // only escape after publication into their owner's lazy namespace.
            secondary.last_miss = Some(node.arena());
        }
        index
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
        let file = self.program.file_of_node(node).ok_or(Error::WrongOwner)?;
        let view = file.bound().view().ast().for_node_owner(node)?;
        view.node(node)?;
        Ok(view)
    }
    fn binding(&self, node: NodeId) -> Result<Option<NodeBinding>, Error> {
        let file = self.program.file_of_node(node).ok_or(Error::WrongOwner)?;
        file.bound().view().node_binding(node)
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
