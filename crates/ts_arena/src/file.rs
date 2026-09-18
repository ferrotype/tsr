use crate::{
    arena::Arena, bundle::Root, counters::Track, lazy::LazyArena, ArenaId, AuxId, AuxiliaryRead,
    CoreScopeMut, Counters, Error, FileId, NodeId, NodeParentRecord, NodeRecord, StorageHandle,
    StorageRead, StorageTransaction, SymbolId,
};
use hashbrown::HashMap;
use std::sync::{Arc, OnceLock};
use ts_jsstring::{PositionMap, SourceText};

/// Exclusive construction owns the same storage later transferred into a file.
/// Read views cannot retain this unpublished owner; mutation requires `&mut self`.
pub struct StorageBuilder<N: NodeRecord, S = ()> {
    owner: StorageOwner<N, S>,
    pub(crate) counters: Counters,
}

/// Checked exclusive core records, disjoint from immutable auxiliary/payload data.
/// This borrow cannot allocate, publish, or resolve imported and lazy records.
pub struct CoreNodesMut<'a, N>(&'a mut Arena<N>);
impl<N> CoreNodesMut<'_, N> {
    pub fn id(&self) -> ArenaId {
        self.0.id
    }
    pub fn get(&self, id: NodeId) -> Result<&N, Error> {
        self.0.get(id.arena(), id.slot())
    }
    pub fn get_mut(&mut self, id: NodeId) -> Result<&mut N, Error> {
        self.0.get_mut(id.arena(), id.slot())
    }
}

/// Read-only construction data paired with an exclusive core-record borrow.
pub struct CoreDataRead<'a, N: NodeRecord> {
    auxiliary: &'a Arena<N::CoreAux>,
    store: &'a N::Store,
    source: &'a SourceText,
}
impl<'a, N: NodeRecord> CoreDataRead<'a, N> {
    pub fn auxiliary_arena(&self) -> ArenaId {
        self.auxiliary.id
    }
    pub fn auxiliary(&self, id: AuxId) -> Result<&'a N::CoreAux, Error> {
        self.auxiliary.get(id.arena(), id.slot())
    }
    pub fn store(&self) -> &'a N::Store {
        self.store
    }
    pub fn source(&self) -> &'a SourceText {
        self.source
    }
}

// Only the imported capability requires shared payloads. Keeping that bound on
// this private object preserves Send-only exclusive builders with no imports.
trait RetainedImport<N: NodeRecord, S>: Send + Sync {
    fn owner(&self) -> &StorageOwner<N, S>;
    fn handle(&self) -> StorageHandle<N, S>;
    fn borrowed_handle(&self) -> &StorageHandle<N, S>;
}
impl<N, S> RetainedImport<N, S> for StorageHandle<N, S>
where
    N: NodeRecord + Send + Sync,
    N::Aux: Send + Sync,
    N::CoreAux: Send + Sync,
    N::Store: Send + Sync,
    S: Send + Sync,
{
    fn owner(&self) -> &StorageOwner<N, S> {
        self
    }
    fn handle(&self) -> StorageHandle<N, S> {
        self.clone()
    }
    fn borrowed_handle(&self) -> &StorageHandle<N, S> {
        self
    }
}
impl<N: NodeRecord, S> std::fmt::Debug for StorageBuilder<N, S> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("StorageBuilder")
            .field("id", &self.id())
            .finish_non_exhaustive()
    }
}
impl<N: NodeRecord, S> StorageBuilder<N, S> {
    pub fn new(bytes: Arc<[u8]>, counters: &Counters) -> Self {
        Self::from_source_text(SourceText::from_bytes(bytes), counters)
    }
    pub fn from_source_text(source: SourceText, counters: &Counters) -> Self {
        Self {
            owner: StorageOwner {
                core: Arena::new(counters),
                symbols: Arena::new(counters),
                auxiliary: Arena::new(counters),
                store: N::Store::default(),
                lazy: LazyArena::new(counters),
                source,
                position_map: OnceLock::new(),
                canonical: None,
                supplemental: Vec::new(),
                metadata: None,
                imports: Vec::new(),
                imported_arenas: HashMap::default(),
                _owner: counters.owner(),
            },
            counters: counters.clone(),
        }
    }
    pub fn id(&self) -> FileId {
        self.owner.id()
    }
    pub fn view(&self) -> StorageView<'_, N, S> {
        self.owner.view()
    }
    pub fn store(&self) -> &N::Store {
        &self.owner.store
    }
    pub fn store_mut(&mut self) -> &mut N::Store {
        &mut self.owner.store
    }
    pub fn store_and_source_mut(&mut self) -> (&mut N::Store, &SourceText) {
        (&mut self.owner.store, &self.owner.source)
    }
    /// Give a storage that was built without text the text of the file
    /// constructed in it, as when a decoded tree is printed and then wrapped in
    /// a source file over the printed text. Only an empty source can be
    /// replaced: nothing built so far can refer into it. The builder is
    /// exclusive and unpublished, so no reader has seen the old text either.
    pub fn adopt_source(&mut self, source: SourceText) -> Result<(), Error> {
        if !self.owner.source.as_bytes().is_empty() {
            return Err(Error::InvalidGraph);
        }
        self.owner.source = source;
        self.owner.position_map = OnceLock::new();
        Ok(())
    }
    /// Open this exclusive core with a fresh invariant scope. Checked local
    /// handles cannot escape or be used with any other scope, even on this same
    /// builder. The callback does not grant growth or publication capabilities.
    /// Writes are not rolled back on error or unwind; the caller must decide
    /// whether a failed operation permits retaining the exclusive builder.
    ///
    /// ```compile_fail
    /// use ts_arena::{Counters, Node, StorageBuilder};
    /// let mut builder = StorageBuilder::<Node<()>>::new(std::sync::Arc::from(&b""[..]), &Counters::new());
    /// let id = builder.push(Node::new(0, ()));
    /// let escaped = builder.with_core_scope(|scope| scope.check(id).unwrap());
    /// ```
    ///
    /// ```compile_fail
    /// use ts_arena::{Counters, Node, StorageBuilder};
    /// let mut first = StorageBuilder::<Node<()>>::new(std::sync::Arc::from(&b""[..]), &Counters::new());
    /// let mut second = StorageBuilder::<Node<()>>::new(std::sync::Arc::from(&b""[..]), &Counters::new());
    /// let id = first.push(Node::new(0, ()));
    /// second.push(Node::new(0, ()));
    /// first.with_core_scope(|first| {
    ///     let local = first.check(id).unwrap();
    ///     second.with_core_scope(|mut second| second.get_mut(local).kind = 1);
    /// });
    /// ```
    ///
    /// ```compile_fail
    /// use ts_arena::{Counters, Node, StorageBuilder};
    /// let mut builder = StorageBuilder::<Node<()>>::new(std::sync::Arc::from(&b""[..]), &Counters::new());
    /// let id = builder.push(Node::new(0, ()));
    /// builder.with_core_scope(|mut scope| {
    ///     let local = scope.check(id).unwrap();
    ///     let read = scope.get(local);
    ///     scope.get_mut(local).kind = 1;
    ///     assert_eq!(read.kind, 0);
    /// });
    /// ```
    ///
    /// ```compile_fail
    /// use ts_arena::{Counters, Node, StorageBuilder};
    /// let mut builder = StorageBuilder::<Node<()>>::new(std::sync::Arc::from(&b""[..]), &Counters::new());
    /// let id = builder.push(Node::new(0, ()));
    /// builder.with_core_scope(|mut scope| {
    ///     let local = scope.check(id).unwrap();
    ///     let view = scope.view();
    ///     scope.get_mut(local).kind = 1;
    ///     assert_eq!(view.core_node(id).unwrap().kind, 0);
    /// });
    /// ```
    ///
    /// ```compile_fail
    /// use ts_arena::{Counters, Node, StorageBuilder};
    /// let mut builder = StorageBuilder::<Node<()>>::new(std::sync::Arc::from(&b""[..]), &Counters::new());
    /// builder.with_core_scope(|scope| {
    ///     builder.push(Node::new(0, ()));
    ///     let _ = scope.source();
    /// });
    /// ```
    #[inline]
    pub fn with_core_scope<R>(
        &mut self,
        operation: impl for<'brand> FnOnce(CoreScopeMut<'brand, '_, N, S>) -> R,
    ) -> R {
        operation(CoreScopeMut::new(self))
    }

    #[inline]
    pub(crate) fn core_slot(&self, slot: u32) -> Result<&N, Error> {
        self.owner.core.get_slot(slot)
    }

    #[inline]
    pub(crate) fn core_slot_mut(&mut self, slot: u32) -> Result<&mut N, Error> {
        self.owner.core.get_slot_mut(slot)
    }

    #[inline]
    pub(crate) fn core_auxiliary_slot(&self, slot: u32) -> Result<&N::CoreAux, Error> {
        self.owner.auxiliary.get_slot(slot)
    }

    #[inline]
    pub(crate) fn core_slot_store_and_source_mut(
        &mut self,
        slot: u32,
    ) -> Result<(&mut N, &mut N::Store, &SourceText), Error> {
        Ok((
            self.owner.core.get_slot_mut(slot)?,
            &mut self.owner.store,
            &self.owner.source,
        ))
    }
    /// Split exclusive core records from immutable construction data. Both
    /// halves borrow this builder; neither can grow storage or escape its owner.
    ///
    /// ```compile_fail
    /// use ts_arena::{Node, NodeId, StorageBuilder};
    /// fn grow(builder: &mut StorageBuilder<Node<()>>, id: NodeId) {
    ///     let (mut nodes, data) = builder.split_core_mut();
    ///     let node = nodes.get_mut(id).unwrap();
    ///     builder.push(Node::new(0, ()));
    ///     node.kind = data.source().len() as u32;
    /// }
    /// ```
    ///
    /// ```compile_fail
    /// use ts_arena::{Node, StorageBuilder};
    /// fn dispose(mut builder: StorageBuilder<Node<()>>) {
    ///     let (_, data) = builder.split_core_mut();
    ///     let source = data.source();
    ///     drop(builder);
    ///     let _ = source.as_bytes();
    /// }
    /// ```
    pub fn split_core_mut(&mut self) -> (CoreNodesMut<'_, N>, CoreDataRead<'_, N>) {
        (
            CoreNodesMut(&mut self.owner.core),
            CoreDataRead {
                auxiliary: &self.owner.auxiliary,
                store: &self.owner.store,
                source: &self.owner.source,
            },
        )
    }
    /// Borrow a checked core header and its payload storage exclusively together.
    pub fn node_and_store_mut(&mut self, id: NodeId) -> Result<(&mut N, &mut N::Store), Error> {
        Ok((
            self.owner.core.get_mut(id.arena(), id.slot())?,
            &mut self.owner.store,
        ))
    }
    pub fn node_store_and_source_mut(
        &mut self,
        id: NodeId,
    ) -> Result<(&mut N, &mut N::Store, &SourceText), Error> {
        Ok((
            self.owner.core.get_mut(id.arena(), id.slot())?,
            &mut self.owner.store,
            &self.owner.source,
        ))
    }
    /// Resolve a core auxiliary record before exposing either mutable part.
    /// Failed owner or slot checks leave the payload store unborrowed.
    pub fn aux_and_store_mut(
        &mut self,
        id: AuxId,
    ) -> Result<(&mut N::CoreAux, &mut N::Store), Error> {
        Ok((
            self.owner.auxiliary.get_mut(id.arena(), id.slot())?,
            &mut self.owner.store,
        ))
    }
    /// Retain a previously published file and its complete mapped bundle.
    /// Imports are flattened and deduplicated at this explicit ownership boundary;
    /// ordinary lookup only borrows the selected owner. Published-only inputs
    /// make dependencies acyclic by publication order.
    pub fn retain_file(&mut self, file: StorageHandle<N, S>)
    where
        N: Send + Sync + 'static,
        N::Aux: Send + Sync + 'static,
        N::CoreAux: Send + Sync + 'static,
        N::Store: Send + Sync + 'static,
        S: Send + Sync + 'static,
    {
        if self.owner.imported_arenas.contains_key(&file.core.id) {
            return;
        }
        for member in file.into_members() {
            for dependency in &member.imports {
                self.owner.retain_import(dependency.handle());
            }
            self.owner.retain_import(member);
        }
    }
    /// Borrow all prospective bundle owners while validating their cross-file
    /// identities. The operation cannot return a view borrowing this group table.
    ///
    /// ```compile_fail
    /// use ts_arena::{Counters, Node, StorageBuilder};
    /// let builder = StorageBuilder::<Node<()>>::new(std::sync::Arc::from(&b""[..]), &Counters::new());
    /// let escaped = StorageBuilder::with_group_views(&[&builder], |views| views[0]);
    /// println!("{:?}", escaped.id());
    /// ```
    pub fn with_group_views<T>(
        builders: &[&Self],
        operation: impl FnOnce(&[StorageView<'_, N, S>]) -> T,
    ) -> T {
        let owners: Vec<_> = builders.iter().map(|builder| &builder.owner).collect();
        let views: Vec<_> = owners
            .iter()
            .map(|&owner| StorageView {
                owner,
                retention: ViewRetention::Group(&owners),
            })
            .collect();
        operation(&views)
    }
    pub fn push(&mut self, node: N) -> NodeId {
        NodeId::new(self.owner.core.id, self.owner.core.push(node))
    }
    pub fn push_aux(&mut self, value: N::CoreAux) -> AuxId {
        AuxId::new(self.owner.auxiliary.id, self.owner.auxiliary.push(value))
    }
    pub fn push_symbol(&mut self, symbol: S) -> SymbolId {
        SymbolId::new(self.owner.symbols.id, self.owner.symbols.push(symbol))
    }
    /// Whether all existing graph storage belongs to this exclusive core owner.
    /// Eager JSDoc cache entries may refer to core nodes without lazy allocations.
    pub fn is_core_only(&self) -> bool {
        self.owner.imports.is_empty() && !self.owner.lazy.has_records()
    }
    /// Checked core access without imported-owner or lazy-arena routing.
    pub fn core_node(&self, id: NodeId) -> Result<&N, Error> {
        self.owner.core.get(id.arena(), id.slot())
    }
    pub fn node_mut(&mut self, id: NodeId) -> Result<&mut N, Error> {
        self.owner.core.get_mut(id.arena(), id.slot())
    }
    pub fn aux_mut(&mut self, id: AuxId) -> Result<&mut N::CoreAux, Error> {
        self.owner.auxiliary.get_mut(id.arena(), id.slot())
    }
    pub fn symbol_mut(&mut self, id: SymbolId) -> Result<&mut S, Error> {
        self.owner.symbols.get_mut(id.arena(), id.slot())
    }

    /// Select a core auxiliary record containing runtime-defined file metadata.
    pub fn set_metadata(&mut self, id: AuxId) -> Result<(), Error> {
        self.owner.auxiliary.get(id.arena(), id.slot())?;
        self.owner.metadata = Some(id);
        Ok(())
    }
    /// Seed eager roots only from validated core storage. A populated cache is not overwritten.
    pub fn seed_jsdoc(&mut self, parent: NodeId, roots: Vec<NodeId>) -> Result<(), Error> {
        self.owner.core.get(parent.arena(), parent.slot())?;
        for id in &roots {
            self.owner.core.get(id.arena(), id.slot())?;
        }
        self.owner.lazy.seed_jsdoc(None, parent, roots.into())
    }
    /// Seed a logical source's cache, independently of other sources sharing
    /// its storage owner. Only validated core references can be seeded.
    pub fn seed_source_jsdoc(
        &mut self,
        source: NodeId,
        parent: NodeId,
        roots: Vec<NodeId>,
    ) -> Result<(), Error> {
        self.owner.core.get(source.arena(), source.slot())?;
        self.owner.core.get(parent.arena(), parent.slot())?;
        for id in &roots {
            self.owner.core.get(id.arena(), id.slot())?;
        }
        self.owner
            .lazy
            .seed_jsdoc(Some(source), parent, roots.into())
    }
    pub fn finish(self) -> StorageHandle<N, S> {
        StorageHandle {
            root: Root::File(Arc::new(self.publish(None, Vec::new()))),
        }
    }
    pub(crate) fn publish(
        mut self,
        canonical: Option<FileId>,
        supplemental: Vec<FileId>,
    ) -> StorageOwner<N, S> {
        self.owner.canonical = canonical;
        self.owner.supplemental = supplemental;
        self.owner
    }
}

/// Owns core and lazy records, auxiliary data, symbols and source bytes together.
/// Shared access has no mutable core path; the exclusive builder contains this
/// same owner before binding and publication.
pub struct StorageOwner<N: NodeRecord, S = ()> {
    pub(crate) core: Arena<N>,
    pub(crate) symbols: Arena<S>,
    pub(crate) auxiliary: Arena<N::CoreAux>,
    pub(crate) store: N::Store,
    pub(crate) lazy: LazyArena<N>,
    source: SourceText,
    position_map: OnceLock<PositionMap>,
    canonical: Option<FileId>,
    supplemental: Vec<FileId>,
    metadata: Option<AuxId>,
    imports: Vec<Box<dyn RetainedImport<N, S>>>,
    imported_arenas: HashMap<ArenaId, usize, crate::hash::FastState>,
    _owner: Track,
}
impl<N: NodeRecord, S> std::fmt::Debug for StorageOwner<N, S> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("StorageOwner")
            .field("id", &self.id())
            .field("lazy_arena", &self.lazy_arena())
            .field("canonical", &self.canonical)
            .field("supplemental", &self.supplemental)
            .finish_non_exhaustive()
    }
}
impl<N: NodeRecord, S> StorageOwner<N, S> {
    pub(crate) fn imported_view(&self, arena: ArenaId) -> Option<StorageView<'_, N, S>> {
        let import = self.imports.get(*self.imported_arenas.get(&arena)?)?;
        import.borrowed_handle().view().for_arena(arena).ok()
    }
    fn retain_import(&mut self, file: StorageHandle<N, S>)
    where
        N: Send + Sync + 'static,
        N::Aux: Send + Sync + 'static,
        N::CoreAux: Send + Sync + 'static,
        N::Store: Send + Sync + 'static,
        S: Send + Sync + 'static,
    {
        if self.imported_arenas.contains_key(&file.core.id) {
            return;
        }
        let index = self.imports.len();
        for arena in [
            file.core.id,
            file.lazy_arena(),
            file.auxiliary_arena(),
            file.lazy_auxiliary_arena(),
        ] {
            self.imported_arenas.insert(arena, index);
        }
        self.imports.push(Box::new(file));
    }
    pub(crate) fn imported_owner(&self, arena: ArenaId) -> Option<&StorageOwner<N, S>> {
        self.imported_arenas
            .get(&arena)
            .map(|&index| self.imports[index].owner())
    }
    pub(crate) fn imported_file(&self, id: FileId) -> Option<StorageHandle<N, S>> {
        self.imported_arenas
            .get(&id.0)
            .map(|&index| self.imports[index].handle())
    }
    pub fn id(&self) -> FileId {
        FileId(self.core.id)
    }
    pub fn symbol_arena(&self) -> ArenaId {
        self.symbols.id
    }
    pub fn auxiliary_arena(&self) -> ArenaId {
        self.auxiliary.id
    }
    pub fn lazy_arena(&self) -> ArenaId {
        self.lazy.id()
    }
    pub fn lazy_auxiliary_arena(&self) -> ArenaId {
        self.lazy.auxiliary_id()
    }
    pub fn source(&self) -> &[u8] {
        self.source.as_bytes()
    }
    pub fn source_text(&self) -> &SourceText {
        &self.source
    }
    pub fn store(&self) -> &N::Store {
        &self.store
    }
    pub fn position_map(&self) -> &PositionMap {
        self.position_map
            .get_or_init(|| PositionMap::new(self.source.as_bytes()))
    }
    pub fn canonical(&self) -> Option<FileId> {
        self.canonical
    }
    pub fn supplemental(&self) -> &[FileId] {
        &self.supplemental
    }
    pub fn metadata(&self) -> Option<AuxId> {
        self.metadata
    }
    pub fn view(&self) -> StorageView<'_, N, S> {
        StorageView {
            owner: self,
            retention: ViewRetention::Owner(self),
        }
    }
}

/// A non-retaining view shared by exclusive and published file storage. Core
/// reads borrow directly; lazy reads lock the directory and retain a stable page.
pub struct StorageView<'a, N: NodeRecord, S = ()> {
    owner: &'a StorageOwner<N, S>,
    retention: ViewRetention<'a, N, S>,
}
enum ViewRetention<'a, N: NodeRecord, S> {
    Owner(&'a StorageOwner<N, S>),
    Handle(&'a StorageHandle<N, S>),
    Group(&'a [&'a StorageOwner<N, S>]),
}
impl<N: NodeRecord, S> Copy for ViewRetention<'_, N, S> {}
impl<N: NodeRecord, S> Clone for ViewRetention<'_, N, S> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<N: NodeRecord, S> Copy for StorageView<'_, N, S> {}
impl<N: NodeRecord, S> Clone for StorageView<'_, N, S> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<N: NodeRecord, S> std::fmt::Debug for StorageView<'_, N, S> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.owner.fmt(out)
    }
}
impl<'a, N: NodeRecord, S> StorageView<'a, N, S> {
    pub(crate) fn retained(handle: &'a StorageHandle<N, S>) -> Self {
        Self {
            owner: handle,
            retention: ViewRetention::Handle(handle),
        }
    }
    /// Select an arena's physical owner within the retained graph, without
    /// reading a node or acquiring the lazy directory lock. This establishes
    /// retention only: it does not validate any record slot or publication.
    pub fn for_arena(self, arena: ArenaId) -> Result<Self, Error> {
        let contains = |owner: &&StorageOwner<N, S>| {
            owner.core.id == arena
                || owner.lazy_arena() == arena
                || owner.auxiliary_arena() == arena
                || owner.lazy_auxiliary_arena() == arena
        };
        if contains(&self.owner) {
            return Ok(self);
        }
        let owner = match self.retention {
            ViewRetention::Owner(owner) => Some(owner)
                .filter(contains)
                .or_else(|| owner.imported_owner(arena)),
            ViewRetention::Handle(handle) => handle.retained_owner(arena),
            ViewRetention::Group(owners) => owners
                .iter()
                .copied()
                .find(contains)
                .or_else(|| owners.iter().find_map(|owner| owner.imported_owner(arena))),
        }
        .ok_or(Error::WrongOwner)?;
        Ok(Self {
            owner,
            retention: self.retention,
        })
    }
    pub fn for_node_owner(self, node: NodeId) -> Result<Self, Error> {
        let view = self.for_arena(node.arena())?;
        view.node_here(node)?;
        Ok(view)
    }
    /// Narrow a selected owner's service to the storage that owner will retain
    /// independently. Ordinary traversal keeps its caller's broader context;
    /// persistent caches cannot borrow an importing owner's extra capabilities.
    pub fn owner_retention(self) -> Result<Self, Error> {
        let arena = self.owner.core.id;
        match self.retention {
            ViewRetention::Owner(owner) => {
                if owner.core.id == arena {
                    Ok(self)
                } else {
                    owner.imported_view(arena).ok_or(Error::WrongOwner)
                }
            }
            ViewRetention::Handle(handle) => handle.owner_retention(arena).ok_or(Error::WrongOwner),
            ViewRetention::Group(owners) => {
                if owners.iter().any(|owner| owner.core.id == arena) {
                    Ok(self)
                } else {
                    owners
                        .iter()
                        .find_map(|owner| owner.imported_view(arena))
                        .ok_or(Error::WrongOwner)
                }
            }
        }
    }
    pub fn core_nodes(self) -> impl Iterator<Item = &'a N> {
        self.owner.core.values()
    }
    pub fn core_auxiliary(self) -> impl Iterator<Item = &'a N::CoreAux> {
        self.owner.auxiliary.values()
    }
    pub fn id(self) -> FileId {
        self.owner.id()
    }
    /// Borrow the selected physical owner without cloning a retaining handle.
    /// The borrow keeps this view's lifetime; it does not inherit the caller's
    /// wider routing capability for other bundle members or imported owners.
    pub fn physical_owner(self) -> &'a StorageOwner<N, S> {
        self.owner
    }
    pub fn metadata(self) -> Option<AuxId> {
        self.owner.metadata
    }
    pub fn counters(self) -> &'a Counters {
        self.owner.core.counters()
    }

    pub fn source(self) -> &'a SourceText {
        self.owner.source_text()
    }
    pub fn store(self) -> &'a N::Store {
        &self.owner.store
    }
    pub fn position_map(self) -> &'a PositionMap {
        self.owner.position_map()
    }
    pub fn lazy_arena(self) -> ArenaId {
        self.owner.lazy_arena()
    }
    pub fn lazy_auxiliary_arena(self) -> ArenaId {
        self.owner.lazy_auxiliary_arena()
    }
    pub fn auxiliary_arena(self) -> ArenaId {
        self.owner.auxiliary_arena()
    }
    /// Borrow only the selected owner's core record. Retained imports and lazy
    /// identities require general routing through `node` instead.
    ///
    /// ```compile_fail
    /// use ts_arena::{Counters, Node, StorageBuilder};
    /// fn escaped() -> &'static Node<()> {
    ///     let mut builder = StorageBuilder::<Node<()>>::new(std::sync::Arc::from(&b""[..]), &Counters::new());
    ///     let id = builder.push(Node::new(0, ()));
    ///     builder.view().core_node(id).unwrap()
    /// }
    /// ```
    #[inline]
    pub fn core_node(self, id: NodeId) -> Result<&'a N, Error> {
        self.owner.core.get(id.arena(), id.slot())
    }
    pub fn node(self, id: NodeId) -> Result<StorageRead<'a, N>, Error> {
        self.for_arena(id.arena())?.node_here(id)
    }
    /// Resolve a node and its physical owner together. The selected view keeps
    /// the caller's retention context; this performs the slot read only once.
    pub fn node_with_owner(self, id: NodeId) -> Result<(StorageRead<'a, N>, Self), Error> {
        let owner = self.for_arena(id.arena())?;
        Ok((owner.node_here(id)?, owner))
    }
    fn node_here(self, id: NodeId) -> Result<StorageRead<'a, N>, Error> {
        if id.arena() == self.owner.core.id {
            Ok(StorageRead::borrowed(self.owner.core.get_slot(id.slot())?))
        } else {
            Ok(StorageRead::lazy(self.owner.lazy.node(id)?))
        }
    }
    pub fn aux(self, id: AuxId) -> Result<AuxiliaryRead<'a, N>, Error> {
        self.for_arena(id.arena())?.aux_here(id)
    }
    /// Resolve an auxiliary record and its physical owner in one routing step.
    /// The returned view borrows the original retention context.
    pub fn aux_with_owner(self, id: AuxId) -> Result<(AuxiliaryRead<'a, N>, Self), Error> {
        let owner = self.for_arena(id.arena())?;
        Ok((owner.aux_here(id)?, owner))
    }
    fn aux_here(self, id: AuxId) -> Result<AuxiliaryRead<'a, N>, Error> {
        if id.arena() == self.owner.auxiliary.id {
            Ok(AuxiliaryRead::Core(
                self.owner.auxiliary.get_slot(id.slot())?,
            ))
        } else {
            Ok(AuxiliaryRead::Lazy(StorageRead::lazy(
                self.owner.lazy.aux(id)?,
            )))
        }
    }
    /// Cache-only lookup. It never invokes a parser or allocates nodes.
    pub fn eager_jsdoc(self, parent: NodeId) -> Result<Option<Arc<[NodeId]>>, Error> {
        let selected = self.for_node_owner(parent)?;
        Ok(selected.owner.lazy.eager_jsdoc(None, parent))
    }
    /// Cache lookup for one logical source, including imported source owners.
    pub fn source_eager_jsdoc(
        self,
        source: NodeId,
        parent: NodeId,
    ) -> Result<Option<Arc<[NodeId]>>, Error> {
        let selected = self.for_node_owner(source)?;
        selected.node_here(parent)?;
        Ok(selected.owner.lazy.eager_jsdoc(Some(source), parent))
    }
    /// The caller must retain this borrow for the operation. Dispatch the whole
    /// operation to any worker before entering this lock, never the initializer.
    pub fn jsdoc(
        self,
        parent: NodeId,
        initialize: impl FnOnce(&mut StorageTransaction<'_, N>) -> Result<Vec<NodeId>, Error>,
    ) -> Result<Arc<[NodeId]>, Error> {
        let selected = self.for_node_owner(parent)?;
        selected
            .owner
            .lazy
            .jsdoc(None, parent, selected.owner, initialize)
    }
    /// Initialize in the selected source's storage on the calling thread. The
    /// parent must belong to that owner; the source key is independent of it.
    pub fn source_jsdoc(
        self,
        source: NodeId,
        parent: NodeId,
        initialize: impl FnOnce(&mut StorageTransaction<'_, N>) -> Result<Vec<NodeId>, Error>,
    ) -> Result<Arc<[NodeId]>, Error> {
        let selected = self.for_node_owner(source)?;
        selected.node_here(parent)?;
        selected
            .owner
            .lazy
            .jsdoc(Some(source), parent, selected.owner, initialize)
    }
    pub fn try_token_record(
        self,
        key: crate::TokenKey,
        kind: u32,
        initialize: impl FnOnce() -> N,
    ) -> Result<StorageRead<'a, N>, Error>
    where
        N: NodeParentRecord,
    {
        let selected = self.for_node_owner(key.parent)?;
        let parent = selected.node(key.parent)?;
        let id = selected.owner.lazy.token(
            key,
            kind,
            parent.storage_reparsed(),
            selected.owner,
            initialize,
        )?;
        self.node(id)
    }
    /// Initialize a token and its auxiliary payload under the same publication
    /// lock. The callback prepares the parent link with its owner context; a
    /// cache hit skips it, and an error or panic publishes none of its staging.
    pub fn try_token_prepared(
        self,
        key: crate::TokenKey,
        kind: u32,
        initialize: impl FnOnce(&mut StorageTransaction<'_, N>, NodeId) -> Result<N, Error>,
    ) -> Result<StorageRead<'a, N>, Error> {
        let id = self.try_token_prepared_id(key, kind, initialize)?;
        self.node(id)
    }
    /// The id form of [`Self::try_token_prepared`], for a caller that resolves
    /// the token through a typed view of its own.
    pub fn try_token_prepared_id(
        self,
        key: crate::TokenKey,
        kind: u32,
        initialize: impl FnOnce(&mut StorageTransaction<'_, N>, NodeId) -> Result<N, Error>,
    ) -> Result<NodeId, Error> {
        let selected = self.for_node_owner(key.parent)?;
        let parent = selected.node(key.parent)?;
        selected.owner.lazy.token_prepared(
            key,
            kind,
            parent.storage_reparsed(),
            selected.owner,
            initialize,
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Node;
    use std::sync::Barrier;

    #[test]
    fn core_split_keeps_auxiliary_borrows_live_during_checked_header_mutation() {
        let counters = Counters::new();
        let mut builder =
            StorageBuilder::<Node<()>>::new(Arc::from(b"source".as_slice()), &counters);
        let id = builder.push(Node::new(1, ()));
        let auxiliary = builder.push_aux(());
        let mut foreign = StorageBuilder::<Node<()>>::new(Arc::from([]), &counters);
        let foreign_id = foreign.push(Node::new(2, ()));
        let missing = NodeId::from_parts(id.arena(), u32::MAX).unwrap();
        let missing_foreign = NodeId::from_parts(foreign_id.arena(), u32::MAX).unwrap();
        {
            let (mut nodes, data) = builder.split_core_mut();
            let record = data.auxiliary(auxiliary).unwrap();
            nodes.get_mut(id).unwrap().kind = 3;
            assert_eq!(nodes.get(id).unwrap().kind, 3);
            assert!(std::ptr::eq(record, data.auxiliary(auxiliary).unwrap()));
            assert_eq!(data.source().as_bytes(), b"source");
            assert_eq!(nodes.get_mut(missing), Err(Error::InvalidSlot));
            assert_eq!(nodes.get_mut(missing_foreign), Err(Error::WrongOwner));
            assert_eq!(nodes.get(foreign_id), Err(Error::WrongOwner));
        }
        assert_eq!(builder.core_node(id).unwrap().kind, 3);
    }

    #[test]
    fn position_map_is_initialized_on_demand_and_shared_between_threads() {
        let file =
            StorageBuilder::<Node<()>>::new(Arc::from("\u{1f600}x".as_bytes()), &Counters::new())
                .finish();
        assert!(file.position_map.get().is_none());
        assert_eq!(file.source(), "\u{1f600}x".as_bytes());
        assert!(file.position_map.get().is_none());
        let barrier = Barrier::new(4);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        file.position_map()
                    })
                })
                .collect();
            let maps: Vec<_> = handles
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect();
            assert!(maps.windows(2).all(|pair| std::ptr::eq(pair[0], pair[1])));
            assert_eq!(maps[0].utf8_to_utf16(4), 2);
        });
        assert!(std::ptr::eq(
            file.position_map.get().unwrap(),
            file.position_map()
        ));
    }
}

#[cfg(test)]
mod construction_tests {
    use super::*;
    use crate::Node;

    #[test]
    fn selected_core_reads_borrow_and_reject_other_namespaces_before_slots() {
        let counters = Counters::new();
        let mut imported = StorageBuilder::<Node<()>>::new(Arc::from(&b"imported"[..]), &counters);
        let foreign = imported.push(Node::new(1, ()));
        let mut builder = StorageBuilder::<Node<()>>::new(Arc::from(&b"local"[..]), &counters);
        let local = builder.push(Node::new(2, ()));
        builder.retain_file(imported.finish());
        let view = builder.view();
        assert!(std::ptr::eq(
            view.core_node(local).unwrap(),
            builder.core_node(local).unwrap(),
        ));
        let missing = NodeId::from_parts(local.arena(), u32::MAX).unwrap();
        assert!(matches!(view.core_node(missing), Err(Error::InvalidSlot)));
        assert!(matches!(view.core_node(foreign), Err(Error::WrongOwner)));
        let missing_foreign = NodeId::from_parts(foreign.arena(), u32::MAX).unwrap();
        assert!(matches!(
            view.core_node(missing_foreign),
            Err(Error::WrongOwner)
        ));
        let lazy = NodeId::from_parts(view.lazy_arena(), 1).unwrap();
        assert!(matches!(view.core_node(lazy), Err(Error::WrongOwner)));
        assert_eq!(view.node(foreign).unwrap().kind, 1);
        let selected = view.for_arena(foreign.arena()).unwrap();
        assert_eq!(selected.core_node(foreign).unwrap().kind, 1);
        assert!(matches!(selected.core_node(local), Err(Error::WrongOwner)));
    }

    #[test]
    fn construction_transfers_requested_map_and_lazy_arena_without_reinitializing() {
        let builder = StorageBuilder::<Node<()>>::new(Arc::from(&b"x\ny"[..]), &Counters::new());
        let lazy_id = builder.view().lazy_arena();
        let auxiliary_id = builder.view().lazy_auxiliary_arena();
        assert!(builder.owner.position_map.get().is_none());
        assert_eq!(builder.view().position_map().utf8_to_utf16(3), 3);
        assert!(builder.owner.position_map.get().is_some());
        let file = builder.finish();
        assert!(file.position_map.get().is_some());
        assert_eq!(file.lazy_arena(), lazy_id);
        assert_eq!(file.lazy_auxiliary_arena(), auxiliary_id);
    }

    #[test]
    fn exclusive_worker_transfer_does_not_require_sync_symbol_payloads() {
        let mut builder = StorageBuilder::<Node<()>, std::cell::Cell<u32>>::new(
            Arc::from(&b"x"[..]),
            &Counters::new(),
        );
        let id = builder.push_symbol(std::cell::Cell::new(1));
        let mut builder = std::thread::spawn(move || {
            builder.symbol_mut(id).unwrap().set(2);
            builder
        })
        .join()
        .unwrap();
        assert_eq!(builder.symbol_mut(id).unwrap().get(), 2);
    }
}

/// Structural storage of one owner for allocation censuses: reserved arena
/// pages and directories, the lazy arena, the record store (measured by the
/// caller's store-specific closure) and the owner's own directories. Returns
/// known bytes and the number of entries whose allocation extent is not
/// exposed (a census reports those as unavailable, never as zero).
impl<N: NodeRecord, S> StorageOwner<N, S> {
    pub fn structural_bytes(&self, store: impl Fn(&N::Store) -> (usize, usize)) -> (usize, usize) {
        self.structural_bytes_with(
            &|store_value, _| store(store_value),
            &mut crate::StorageCensus::default(),
        )
    }
    pub(crate) fn structural_bytes_with(
        &self,
        store: &impl Fn(&N::Store, &mut crate::StorageCensus) -> (usize, usize),
        census: &mut crate::StorageCensus,
    ) -> (usize, usize) {
        let (store_known, store_unmeasured) = store(&self.store, census);
        let (lazy_known, lazy_unmeasured) = self.lazy.structural_bytes();
        let mut known = self.core.structural_bytes()
            + self.symbols.structural_bytes()
            + self.auxiliary.structural_bytes()
            + store_known
            + lazy_known
            + self.supplemental.capacity() * size_of::<FileId>()
            + self.imports.capacity() * size_of::<Box<dyn RetainedImport<N, S>>>()
            + self.imported_arenas.allocation_size()
            + census.text(self.source.backing_bytes())
            + self
                .position_map
                .get()
                .map_or(0, PositionMap::structural_bytes);
        let mut unmeasured = store_unmeasured + lazy_unmeasured;
        for import in &self.imports {
            // The box retains a handle; its referenced owner may be a bound
            // program file or a previously published checker-created frame.
            known += size_of_val(&**import);
            let (bytes, unknown) = import
                .borrowed_handle()
                .structural_bytes_with(store, census);
            known += bytes;
            unmeasured += unknown;
        }
        (known, unmeasured)
    }
}
impl<N: NodeRecord, S> StorageBuilder<N, S> {
    pub fn structural_bytes(&self, store: impl Fn(&N::Store) -> (usize, usize)) -> (usize, usize) {
        self.structural_bytes_with(
            &|value, _| store(value),
            &mut crate::StorageCensus::default(),
        )
    }
    pub fn structural_bytes_with(
        &self,
        store: &impl Fn(&N::Store, &mut crate::StorageCensus) -> (usize, usize),
        census: &mut crate::StorageCensus,
    ) -> (usize, usize) {
        if !census.owner(self.id()) {
            return (0, 0);
        }
        self.owner.structural_bytes_with(store, census)
    }
}
