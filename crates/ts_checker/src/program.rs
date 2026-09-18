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

/// The last (arena, file index) a directory resolved, packed in one word so the
/// context stays `Sync`: consecutive lookups almost always hit the same file,
/// and a compare beats a hash probe on every node and symbol read.
#[derive(Default)]
struct LastHit(std::sync::atomic::AtomicU64);
impl LastHit {
    #[inline]
    fn get(&self, arena: ArenaId) -> Option<usize> {
        let word = self.0.load(std::sync::atomic::Ordering::Relaxed);
        (word != 0 && (word >> 32) as u32 == arena.get()).then_some((word & 0xffff_ffff) as usize)
    }
    #[inline]
    fn set(&self, arena: ArenaId, index: usize) {
        if let Ok(index) = u32::try_from(index) {
            self.0.store(
                (u64::from(arena.get()) << 32) | u64::from(index),
                std::sync::atomic::Ordering::Relaxed,
            );
        }
    }
}

pub(crate) struct ProgramContext {
    pub(crate) host: Arc<dyn CheckerHost>,
    last_node: LastHit,
    last_symbol: LastHit,
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

    /// `ast(node)?.node(node)?` in one step: the current core file's header
    /// directly, with one slot validation instead of two and no owner
    /// selection in between.
    #[inline]
    pub(crate) fn node(&self, node: NodeId) -> Result<ts_ast::NodeRead<'_>, crate::Error> {
        if self.factory.id().arena() == node.arena() {
            return Ok(self.factory.view().node(node)?);
        }
        Ok(self.program()?.node(node)?)
    }

    /// `ast(node)?.node_text(node)?` without the view's own validation of
    /// `node`: the text read validates the id itself.
    #[inline]
    pub(crate) fn node_text(&self, node: NodeId) -> Result<ts_ast::NodeText<'_>, crate::Error> {
        if self.factory.id().arena() == node.arena() {
            return Ok(self.factory.view().node_text(node)?);
        }
        Ok(self.program()?.ast(node)?.node_text(node)?)
    }

    /// `ast(source)?.source_file(source)?` by the directory alone; the
    /// source-file read validates the id itself.
    #[inline]
    pub(crate) fn source_file_read(
        &self,
        source: NodeId,
    ) -> Result<ts_ast::SourceFileRead<'_>, crate::Error> {
        if self.factory.id().arena() == source.arena() {
            return Ok(self.factory.view().source_file(source)?);
        }
        Ok(self.program()?.ast(source)?.source_file(source)?)
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
            last_node: LastHit::default(),
            last_symbol: LastHit::default(),
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

    /// The binding of the file whose core arena holds `node`, without reading
    /// the node: flow and binding lookups validate their own ids.
    pub(crate) fn bind_result(&self, node: NodeId) -> Result<&ts_ast::BindResult, Error> {
        if let Some(index) = self.core_file_index(node) {
            if let Some(shared) = &self.shared[index] {
                return Ok(shared.result());
            }
        }
        self.bound(node).map(ts_ast::BoundView::result)
    }

    /// One node read by the shortest path; see `SharedBoundFile::node`.
    #[inline]
    pub(crate) fn node(&self, node: NodeId) -> Result<ts_ast::NodeRead<'_>, Error> {
        if let Some(index) = self.core_file_index(node) {
            if let Some(shared) = &self.shared[index] {
                return shared.node(node);
            }
        }
        self.ast(node)?.node(node)
    }

    /// The view of the file whose core arena holds `node`. A directory hit
    /// selects the owner without validating `node`: every caller reads the
    /// node (or another of the same file) through the view, and those reads
    /// validate their own ids, so the selection paid twice for nothing.
    /// Other arenas take the routed, validating path.
    #[inline]
    pub(crate) fn ast(&self, node: NodeId) -> Result<AstView<'_>, Error> {
        if let Some(index) = self.core_file_index(node) {
            return Ok(self.file_view(index).ast());
        }
        self.bound(node)?.ast().for_node_owner(node)
    }

    /// The file index of the file whose core arena holds `node`, or `None`
    /// when the node lives elsewhere (lazy, auxiliary or checker-owned arenas).
    pub(crate) fn core_file_index(&self, node: NodeId) -> Option<usize> {
        let arena = node.arena();
        if let Some(index) = self.last_node.get(arena) {
            return Some(index);
        }
        let index = self.nodes.get(&arena).copied()?;
        self.last_node.set(arena, index);
        Some(index)
    }

    pub(crate) fn file_index(&self, source: Option<NodeId>) -> usize {
        // Go's map lookup returns zero for a synthetic/nil source outside files.
        source
            .and_then(|source| self.file_indices.get(&source).copied())
            .unwrap_or(0)
    }

    // Symbol, table and declaration lookups take the shared view too: the
    // display path asks for a symbol on nearly every name-chain step.
    pub(crate) fn symbol(&self, symbol: SymbolId) -> Result<SymbolRef<'_>, Error> {
        let arena = symbol.arena();
        let index = if let Some(index) = self.last_symbol.get(arena) {
            index
        } else {
            let &index = self.symbols.get(&arena).ok_or(Error::WrongOwner)?;
            self.last_symbol.set(arena, index);
            index
        };
        self.file_view(index).symbol(symbol).map(SymbolRef::Stored)
    }

    pub(crate) fn table(&self, table: SymbolTableId) -> Result<SymbolTableRead<'_>, Error> {
        let &index = self.tables.get(&table.arena()).ok_or(Error::WrongOwner)?;
        self.file_view(index).result().tables().get(table)
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
        self.file_view(index).result().declarations().get(slice)
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
