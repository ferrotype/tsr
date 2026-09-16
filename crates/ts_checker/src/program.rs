//! Retained immutable program inputs. The index only routes a namespace to its
//! owner; each store still checks the slot. Ordinary reads borrow the retained
//! host and never clone a file or checker owner.

use crate::types::Map as HashMap;
use crate::{CheckerHost, CheckerState};
use std::sync::Arc;
use ts_arena::{ArenaId, Error, NodeId, SymbolId};
use ts_ast::{
    AstView, BoundView, DeclarationRead, DeclarationSlice, SymbolRef, SymbolTableId,
    SymbolTableRead,
};

pub(crate) struct ProgramContext {
    pub(crate) host: Arc<dyn CheckerHost>,
    /// Per file, the shared owner and binding: a view from here is two
    /// borrows, where the host's completed file routes through its handle on
    /// every call. `None` for a bundle member, which keeps the routed path.
    shared: Vec<Option<ts_ast::SharedBoundFile>>,
    nodes: HashMap<ArenaId, usize>,
    file_indices: HashMap<NodeId, usize>,
    symbols: HashMap<ArenaId, usize>,
    tables: HashMap<ArenaId, usize>,
    declarations: HashMap<ArenaId, usize>,
}

impl CheckerState {
    pub(crate) fn program(&self) -> Result<&ProgramContext, crate::Error> {
        self.program
            .as_ref()
            .ok_or(crate::Error::Unsupported("query without a checker program"))
    }

    pub(crate) fn ast(&self, node: NodeId) -> Result<AstView<'_>, crate::Error> {
        if self.factory.id().arena() == node.arena() {
            let view = self.factory.view();
            view.node(node)?;
            return Ok(view);
        }
        Ok(self.program()?.ast(node)?)
    }
}

impl ProgramContext {
    pub(crate) fn new(host: Arc<dyn CheckerHost>) -> Self {
        let shared = (0..host.source_file_count())
            .map(|index| host.source_file(index).shared())
            .collect();
        let mut result = Self {
            host,
            shared,
            nodes: HashMap::default(),
            file_indices: HashMap::default(),
            symbols: HashMap::default(),
            tables: HashMap::default(),
            declarations: HashMap::default(),
        };
        for index in 0..result.host.source_file_count() {
            let file = result.host.source_file(index);
            let view = file.view();
            result.file_indices.insert(file.source(), index);
            result.nodes.entry(file.source().arena()).or_insert(index);
            result.symbols.insert(view.result().symbols().id(), index);
            result.tables.insert(view.result().tables().id(), index);
            result
                .declarations
                .insert(view.result().declarations().id(), index);
        }
        result
    }

    /// The bound view of file `index`, from the shared form when there is one.
    fn file_view(&self, index: usize) -> BoundView<'_> {
        match &self.shared[index] {
            Some(shared) => shared.view(),
            None => self.host.source_file(index).view(),
        }
    }

    pub(crate) fn bound(&self, node: NodeId) -> Result<BoundView<'_>, Error> {
        if let Some(&index) = self.nodes.get(&node.arena()) {
            let view = self.file_view(index);
            view.node(node)?;
            return Ok(view);
        }
        // Lazy JSDoc and retained imported owners have separate namespaces.
        // The core index must not make these legitimate nodes look foreign.
        for index in 0..self.host.source_file_count() {
            let view = self.host.source_file(index).view();
            match view.ast().for_node_owner(node) {
                Ok(ast) => {
                    ast.node(node)?;
                    return Ok(view);
                }
                Err(Error::WrongOwner) => {}
                Err(error) => return Err(error),
            }
        }
        Err(Error::WrongOwner)
    }

    pub(crate) fn ast(&self, node: NodeId) -> Result<AstView<'_>, Error> {
        // `for_node_owner` validates the slot, so the core index needs no
        // second node lookup before it.
        if let Some(&index) = self.nodes.get(&node.arena()) {
            return self.file_view(index).ast().for_node_owner(node);
        }
        self.bound(node)?.ast().for_node_owner(node)
    }

    /// The file index of the file whose core arena holds `node`, or `None`
    /// when the node lives elsewhere (lazy, auxiliary or checker-owned arenas).
    pub(crate) fn core_file_index(&self, node: NodeId) -> Option<usize> {
        self.nodes.get(&node.arena()).copied()
    }

    pub(crate) fn file_index(&self, source: Option<NodeId>) -> usize {
        // Go's map lookup returns zero for a synthetic/nil source outside files.
        source
            .and_then(|source| self.file_indices.get(&source).copied())
            .unwrap_or(0)
    }

    pub(crate) fn symbol(&self, symbol: SymbolId) -> Result<SymbolRef<'_>, Error> {
        let &index = self.symbols.get(&symbol.arena()).ok_or(Error::WrongOwner)?;
        self.host
            .source_file(index)
            .view()
            .symbol(symbol)
            .map(SymbolRef::Stored)
    }

    pub(crate) fn table(&self, table: SymbolTableId) -> Result<SymbolTableRead<'_>, Error> {
        let &index = self.tables.get(&table.arena()).ok_or(Error::WrongOwner)?;
        self.host
            .source_file(index)
            .view()
            .result()
            .tables()
            .get(table)
    }

    pub(crate) fn declarations(
        &self,
        slice: DeclarationSlice,
    ) -> Result<DeclarationRead<'_>, Error> {
        let backing = slice.backing_id().ok_or(Error::InvalidSlot)?;
        let &index = self
            .declarations
            .get(&backing.arena())
            .ok_or(Error::WrongOwner)?;
        self.host
            .source_file(index)
            .view()
            .result()
            .declarations()
            .get(slice)
    }
}

#[cfg(any(test, feature = "storage-pilot"))]
impl ProgramContext {
    pub(crate) fn census(&self, census: &mut crate::census::Census) {
        // The retained program itself is bound input; these directories are
        // allocated by the checker and remain charged here.
        census.map("program_indices", &self.nodes);
        census.map("program_indices", &self.file_indices);
        census.map("program_indices", &self.symbols);
        census.map("program_indices", &self.tables);
        census.map("program_indices", &self.declarations);
    }
}
