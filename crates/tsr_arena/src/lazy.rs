use crate::{
    arena::Arena,
    counters::Track,
    ids::{allocate_slot, next_arena},
    ArenaId, AuxId, AuxiliaryRead, Counters, Error, NodeId, NodeParentRecord, NodeRecord,
    StorageOwner, StorageRead,
};
use std::{
    collections::BTreeMap,
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    sync::{Arc, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

const PAGE_SIZE: usize = 256;
fn assert_not_initializing(id: ArenaId) {
    crate::InitializationGuard::assert_inactive(id, crate::InitializationDomain::Lazy, 0);
}

struct InitializerGuard {
    _guard: crate::InitializationGuard,
}
impl InitializerGuard {
    fn enter(id: ArenaId) -> Self {
        Self {
            _guard: crate::InitializationGuard::enter(id, crate::InitializationDomain::Lazy, 0),
        }
    }
}

struct Page<T> {
    slots: [OnceLock<T>; PAGE_SIZE],
    _allocation: Track,
}
/// Kept private: an exported read also borrows or retains its complete owner.
pub(crate) struct LazyRecord<T> {
    page: Arc<Page<T>>,
    offset: usize,
}
impl<T> Clone for LazyRecord<T> {
    fn clone(&self) -> Self {
        Self {
            page: self.page.clone(),
            offset: self.offset,
        }
    }
}
impl<T> LazyRecord<T> {
    pub(crate) fn get(&self) -> &T {
        self.page.slots[self.offset]
            .get()
            .expect("published lazy record")
    }
}
struct Pages<T> {
    id: ArenaId,
    reserved: usize,
    directory: Vec<Arc<Page<T>>>,
    counters: Counters,
}
impl<T> Pages<T> {
    fn new(counters: &Counters) -> Self {
        Self {
            id: next_arena(),
            reserved: 0,
            directory: Vec::new(),
            counters: counters.clone(),
        }
    }
    fn reserve(&mut self) -> u32 {
        let slot = allocate_slot(self.reserved);
        if self.reserved / PAGE_SIZE == self.directory.len() {
            self.directory.push(Arc::new(Page {
                slots: std::array::from_fn(|_| OnceLock::new()),
                _allocation: self.counters.allocation(),
            }));
        }
        self.reserved += 1;
        slot
    }
    fn initialize(&self, slot: u32, value: T) {
        let index = slot as usize - 1;
        // Initialize only the unused slot; never mutably borrow a published page.
        assert!(
            self.directory[index / PAGE_SIZE].slots[index % PAGE_SIZE]
                .set(value)
                .is_ok(),
            "lazy slots are initialized once"
        );
    }
    fn get_borrowed(&self, slot: u32) -> Result<&T, Error> {
        let index = slot.checked_sub(1).ok_or(Error::InvalidSlot)? as usize;
        if index >= self.reserved {
            return Err(Error::InvalidSlot);
        }
        self.directory[index / PAGE_SIZE].slots[index % PAGE_SIZE]
            .get()
            .ok_or(Error::InvalidSlot)
    }
    fn get(&self, slot: u32) -> Result<LazyRecord<T>, Error> {
        self.get_borrowed(slot)?;
        let index = slot as usize - 1;
        Ok(LazyRecord {
            page: self.directory[index / PAGE_SIZE].clone(),
            offset: index % PAGE_SIZE,
        })
    }
}
struct Staging<'a, T> {
    pages: &'a mut Pages<T>,
    base: usize,
    pending: Vec<T>,
}
impl<'a, T> Staging<'a, T> {
    fn new(pages: &'a mut Pages<T>) -> Self {
        Self {
            base: pages.reserved,
            pages,
            pending: Vec::new(),
        }
    }
    fn push(&mut self, value: T) -> u32 {
        let slot = self.pages.reserve();
        self.pending.push(value);
        slot
    }
    fn index(&self, slot: u32) -> Result<usize, Error> {
        (slot as usize)
            .checked_sub(self.base + 1)
            .filter(|&i| i < self.pending.len())
            .ok_or(Error::InvalidSlot)
    }
    fn get(&self, slot: u32) -> Result<&T, Error> {
        if slot as usize <= self.base {
            self.pages.get_borrowed(slot)
        } else {
            self.pending
                .get(self.index(slot)?)
                .ok_or(Error::InvalidSlot)
        }
    }
    fn get_mut(&mut self, slot: u32) -> Result<&mut T, Error> {
        let index = self.index(slot)?;
        self.pending.get_mut(index).ok_or(Error::InvalidSlot)
    }
    fn publish(self) {
        for (index, value) in self.pending.into_iter().enumerate() {
            self.pages.initialize((self.base + index + 1) as u32, value);
        }
    }
}

/// Node and auxiliary staging share the cache's single publication lock. Reads
/// resolve core, already published and staged records without taking that lock again.
/// Error and unwind burn both sets of identities and discard all pending values.
pub struct StorageTransaction<'a, N: NodeRecord> {
    nodes: Staging<'a, N>,
    auxiliary: Staging<'a, N::Aux>,
    core: &'a Arena<N>,
    core_auxiliary: &'a Arena<N::CoreAux>,
    source: &'a tsr_jsstring::SourceText,
    store: &'a N::Store,
}
impl<N: NodeRecord> StorageTransaction<'_, N> {
    /// Physical file owning both this transaction's core and lazy arenas.
    pub fn owner_id(&self) -> crate::FileId {
        crate::FileId(self.core.id)
    }
    /// The source is borrowed before initialization acquires its publication
    /// lock. Reading it here cannot reenter the lazy directory.
    pub fn source(&self) -> &tsr_jsstring::SourceText {
        self.source
    }
    /// Immutable core payload storage is borrowed before the lazy publication
    /// lock is acquired. Transaction reads do not route through that lock.
    pub fn store(&self) -> &N::Store {
        self.store
    }
    pub fn core_auxiliary_arena(&self) -> ArenaId {
        self.core_auxiliary.id
    }
    pub fn lazy_auxiliary_arena(&self) -> ArenaId {
        self.auxiliary.pages.id
    }
    pub fn staged_nodes(&self) -> impl Iterator<Item = &N> {
        self.nodes.pending.iter()
    }
    pub fn staged_aux(&self) -> impl Iterator<Item = &N::Aux> {
        self.auxiliary.pending.iter()
    }
    pub fn push(&mut self, node: N) -> NodeId {
        NodeId::new(self.nodes.pages.id, self.nodes.push(node))
    }
    pub fn push_aux(&mut self, value: N::Aux) -> AuxId {
        AuxId::new(self.auxiliary.pages.id, self.auxiliary.push(value))
    }
    pub fn node(&self, id: NodeId) -> Result<&N, Error> {
        if id.arena() == self.core.id {
            return self.core.get_slot(id.slot());
        }
        if id.arena() != self.nodes.pages.id {
            return Err(Error::WrongOwner);
        }
        self.nodes.get(id.slot())
    }
    /// Core records borrow the existing owner; staged and published lazy values
    /// borrow this transaction's already locked pages without locking again.
    ///
    /// ```compile_fail
    /// use tsr_arena::{AuxId, AuxiliaryRead, Node, StorageTransaction};
    /// fn escape<'a>(transaction: &StorageTransaction<'_, Node<()>>, id: AuxId) -> AuxiliaryRead<'a, Node<()>> {
    ///     transaction.aux(id).unwrap()
    /// }
    /// ```
    pub fn aux(&self, id: AuxId) -> Result<AuxiliaryRead<'_, N>, Error> {
        if id.arena() == self.core_auxiliary.id {
            return self
                .core_auxiliary
                .get_slot(id.slot())
                .map(AuxiliaryRead::Core);
        }
        if id.arena() != self.auxiliary.pages.id {
            return Err(Error::WrongOwner);
        }
        self.auxiliary
            .get(id.slot())
            .map(|value| AuxiliaryRead::Lazy(StorageRead::borrowed(value)))
    }
    pub fn node_mut(&mut self, id: NodeId) -> Result<&mut N, Error> {
        if id.arena() != self.nodes.pages.id {
            return Err(Error::WrongOwner);
        }
        self.nodes.get_mut(id.slot())
    }
    pub fn aux_mut(&mut self, id: AuxId) -> Result<&mut N::Aux, Error> {
        if id.arena() != self.auxiliary.pages.id {
            return Err(Error::WrongOwner);
        }
        self.auxiliary.get_mut(id.slot())
    }
    /// Split exclusive access to two records staged by this transaction. Core,
    /// previously published and failed-attempt records remain immutable.
    pub fn node_and_aux_mut(
        &mut self,
        node: NodeId,
        auxiliary: AuxId,
    ) -> Result<(&mut N, &mut N::Aux), Error> {
        if node.arena() != self.nodes.pages.id {
            return Err(Error::WrongOwner);
        }
        let node = self.nodes.get_mut(node.slot())?;
        if auxiliary.arena() != self.auxiliary.pages.id {
            return Err(Error::WrongOwner);
        }
        Ok((node, self.auxiliary.get_mut(auxiliary.slot())?))
    }
    fn publish(self, roots: &[NodeId]) -> Result<(), Error> {
        for id in roots {
            if id.arena() != self.nodes.pages.id || self.nodes.index(id.slot()).is_err() {
                return Err(Error::InvalidGraph);
            }
        }
        // No user callbacks or fallible operations after this publication point.
        self.auxiliary.publish();
        self.nodes.publish();
        Ok(())
    }
    fn publish_token(self, node: N) -> NodeId {
        let Staging {
            pages,
            base,
            pending,
        } = self.nodes;
        // Reserve before the publication point. Keeping the returned header
        // separate avoids allocating a staging Vec for full-record tokens.
        let slot = pages.reserve();
        self.auxiliary.publish();
        for (index, value) in pending.into_iter().enumerate() {
            pages.initialize((base + index + 1) as u32, value);
        }
        pages.initialize(slot, node);
        NodeId::new(pages.id, slot)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TokenKey {
    pub parent: NodeId,
    pub start: usize,
    pub end: usize,
}
#[derive(Clone, Copy)]
struct CachedToken {
    id: NodeId,
    kind: u32,
}
struct LazyState<N: NodeRecord> {
    pages: Pages<N>,
    auxiliary: Pages<N::Aux>,
    // None is the S04 file cache. Concrete ASTs can distinguish several logical
    // source files housed in the same storage owner without adding another lock.
    jsdoc: BTreeMap<(Option<NodeId>, NodeId), Arc<[NodeId]>>,
    tokens: BTreeMap<TokenKey, CachedToken>,
}
/// Node/auxiliary directories, reservations, publication and caches use one lock.
pub(crate) struct LazyArena<N: NodeRecord> {
    id: ArenaId,
    auxiliary_id: ArenaId,
    state: RwLock<LazyState<N>>,
}
impl<N: NodeRecord> LazyArena<N> {
    pub(crate) fn new(counters: &Counters) -> Self {
        let pages = Pages::new(counters);
        let auxiliary = Pages::new(counters);
        Self {
            id: pages.id,
            auxiliary_id: auxiliary.id,
            state: RwLock::new(LazyState {
                pages,
                auxiliary,
                jsdoc: BTreeMap::new(),
                tokens: BTreeMap::new(),
            }),
        }
    }
    pub(crate) fn id(&self) -> ArenaId {
        self.id
    }
    pub(crate) fn has_records(&self) -> bool {
        let state = self.read();
        state.pages.reserved != 0 || state.auxiliary.reserved != 0
    }
    pub(crate) fn auxiliary_id(&self) -> ArenaId {
        self.auxiliary_id
    }
    fn read(&self) -> RwLockReadGuard<'_, LazyState<N>> {
        assert_not_initializing(self.id);
        self.state
            .read()
            .expect("lazy publication lock is not poisoned")
    }
    fn write(&self) -> RwLockWriteGuard<'_, LazyState<N>> {
        assert_not_initializing(self.id);
        self.state
            .write()
            .expect("lazy publication lock is not poisoned")
    }
    pub(crate) fn node(&self, id: NodeId) -> Result<LazyRecord<N>, Error> {
        if id.arena() != self.id {
            return Err(Error::WrongOwner);
        }
        self.read().pages.get(id.slot())
    }
    pub(crate) fn aux(&self, id: AuxId) -> Result<LazyRecord<N::Aux>, Error> {
        if id.arena() != self.auxiliary_id {
            return Err(Error::WrongOwner);
        }
        self.read().auxiliary.get(id.slot())
    }
    pub(crate) fn eager_jsdoc(
        &self,
        source: Option<NodeId>,
        parent: NodeId,
    ) -> Option<Arc<[NodeId]>> {
        self.read().jsdoc.get(&(source, parent)).cloned()
    }
    pub(crate) fn seed_jsdoc(
        &mut self,
        source: Option<NodeId>,
        parent: NodeId,
        roots: Arc<[NodeId]>,
    ) -> Result<(), Error> {
        let state = self
            .state
            .get_mut()
            .expect("lazy publication lock is not poisoned");
        if state.jsdoc.contains_key(&(source, parent)) {
            return Err(Error::InvalidGraph);
        }
        state.jsdoc.insert((source, parent), roots);
        Ok(())
    }
    pub(crate) fn jsdoc<S>(
        &self,
        source: Option<NodeId>,
        parent: NodeId,
        owner: &StorageOwner<N, S>,
        initialize: impl FnOnce(&mut StorageTransaction<'_, N>) -> Result<Vec<NodeId>, Error>,
    ) -> Result<Arc<[NodeId]>, Error> {
        let source_text = owner.source_text();
        let store = owner.store();
        if let Some(ids) = self.read().jsdoc.get(&(source, parent)) {
            return Ok(ids.clone());
        }
        let mut state = self.write();
        if let Some(ids) = state.jsdoc.get(&(source, parent)) {
            return Ok(ids.clone());
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _initializer = InitializerGuard::enter(self.id);
            let LazyState {
                pages, auxiliary, ..
            } = &mut *state;
            let mut transaction = StorageTransaction {
                nodes: Staging::new(pages),
                auxiliary: Staging::new(auxiliary),
                core: &owner.core,
                core_auxiliary: &owner.auxiliary,
                source: source_text,
                store,
            };
            let roots = initialize(&mut transaction)?;
            transaction.publish(&roots)?;
            let roots: Arc<[NodeId]> = roots.into();
            state.jsdoc.insert((source, parent), roots.clone());
            Ok(roots)
        }));
        drop(state);
        match result {
            Ok(result) => result,
            Err(panic) => resume_unwind(panic),
        }
    }
    fn cached_token(
        state: &LazyState<N>,
        key: TokenKey,
        kind: u32,
    ) -> Result<Option<NodeId>, Error> {
        state
            .tokens
            .get(&key)
            .map(|cached| {
                if cached.kind == kind {
                    Ok(cached.id)
                } else {
                    Err(Error::TokenKindMismatch {
                        cached: cached.kind,
                        requested: kind,
                    })
                }
            })
            .transpose()
    }
    pub(crate) fn token<S>(
        &self,
        key: TokenKey,
        kind: u32,
        reparsed: bool,
        owner: &StorageOwner<N, S>,
        initialize: impl FnOnce() -> N,
    ) -> Result<NodeId, Error>
    where
        N: NodeParentRecord,
    {
        self.token_prepared_with(
            key,
            kind,
            reparsed,
            owner,
            |_, _| Ok(initialize()),
            |node| node.set_storage_parent(Some(key.parent)),
        )
    }
    pub(crate) fn token_prepared<S>(
        &self,
        key: TokenKey,
        kind: u32,
        reparsed: bool,
        owner: &StorageOwner<N, S>,
        initialize: impl FnOnce(&mut StorageTransaction<'_, N>, NodeId) -> Result<N, Error>,
    ) -> Result<NodeId, Error> {
        self.token_prepared_with(key, kind, reparsed, owner, initialize, |_| {})
    }
    fn token_prepared_with<S>(
        &self,
        key: TokenKey,
        kind: u32,
        reparsed: bool,
        owner: &StorageOwner<N, S>,
        initialize: impl FnOnce(&mut StorageTransaction<'_, N>, NodeId) -> Result<N, Error>,
        finish_header: impl FnOnce(&mut N),
    ) -> Result<NodeId, Error> {
        let source_text = owner.source_text();
        let store = owner.store();
        if let Some(id) = Self::cached_token(&self.read(), key, kind)? {
            return Ok(id);
        }
        let mut state = self.write();
        if let Some(id) = Self::cached_token(&state, key, kind)? {
            return Ok(id);
        }
        if reparsed {
            return Err(Error::ReparsedParent);
        }
        if key.start > key.end || key.end > owner.source().len() {
            return Err(Error::InvalidTokenRange);
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _initializer = InitializerGuard::enter(self.id);
            let LazyState {
                pages, auxiliary, ..
            } = &mut *state;
            let mut transaction = StorageTransaction {
                nodes: Staging::new(pages),
                auxiliary: Staging::new(auxiliary),
                core: &owner.core,
                core_auxiliary: &owner.auxiliary,
                source: source_text,
                store,
            };
            let mut node = initialize(&mut transaction, key.parent)?;
            if node.storage_kind() != kind {
                return Err(Error::TokenKindMismatch {
                    cached: node.storage_kind(),
                    requested: kind,
                });
            }
            // Full-record wrappers preserve kind validation before their parent
            // setter. Prepared compact headers already contain their link.
            finish_header(&mut node);
            let id = transaction.publish_token(node);
            state.tokens.insert(key, CachedToken { id, kind });
            Ok(id)
        }));
        drop(state);
        match result {
            Ok(result) => result,
            Err(panic) => resume_unwind(panic),
        }
    }
}

impl<T> Pages<T> {
    /// Reserved page bytes plus the directory's capacity.
    fn structural_bytes(&self) -> usize {
        // Each page is one `Arc` allocation: the strong and weak counts precede
        // the slots.
        self.directory.capacity() * size_of::<Arc<Page<T>>>()
            + self.directory.len() * (16 + size_of::<Page<T>>())
    }
}

impl<N: NodeRecord> LazyArena<N> {
    /// Known structural bytes and the count of B-tree entries whose
    /// allocation extent std does not expose.
    pub(crate) fn structural_bytes(&self) -> (usize, usize) {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            state.pages.structural_bytes() + state.auxiliary.structural_bytes(),
            state.jsdoc.len() + state.tokens.len(),
        )
    }
}

#[cfg(test)]
mod prepared_token_tests {
    use super::*;
    use crate::StorageBuilder;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Payload {
        value: u32,
        dropped: Arc<AtomicUsize>,
    }
    impl Drop for Payload {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    // Deliberately has no NodeParentRecord implementation: its parent is
    // prepared together with the auxiliary payload by the owner-aware callback.
    struct Header {
        kind: u32,
        parent: Option<NodeId>,
        payload: Option<AuxId>,
        reparsed: bool,
    }
    impl Header {
        fn new(kind: u32) -> Self {
            Self {
                kind,
                parent: None,
                payload: None,
                reparsed: false,
            }
        }
    }
    impl NodeRecord for Header {
        type CoreAux = u32;
        type Aux = Payload;
        type Store = Vec<u8>;
        fn storage_kind(&self) -> u32 {
            self.kind
        }
        fn storage_reparsed(&self) -> bool {
            self.reparsed
        }
    }

    fn auxiliary_value(value: AuxiliaryRead<'_, Header>) -> u32 {
        match value {
            AuxiliaryRead::Core(value) => *value,
            AuxiliaryRead::Lazy(value) => value.value,
        }
    }

    #[test]
    fn prepared_token_rolls_back_payloads_and_ids_before_retry() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let counters = Counters::new();
        let baseline = counters.snapshot();
        {
            let mut builder = StorageBuilder::<Header>::new(Arc::from(&b"abc"[..]), &counters);
            let parent = builder.push(Header::new(1));
            let owner = builder.finish();
            let key = TokenKey {
                parent,
                start: 0,
                end: 1,
            };
            let mut failed_ids = Vec::new();
            for failure in 0..3 {
                let mut staged = None;
                let result = catch_unwind(AssertUnwindSafe(|| {
                    owner.try_token_prepared(key, 2, |transaction, selected_parent| {
                        assert_eq!(selected_parent, parent);
                        let auxiliary = transaction.push_aux(Payload {
                            value: 5,
                            dropped: dropped.clone(),
                        });
                        let node = transaction.push(Header::new(3));
                        staged = Some((node, auxiliary));
                        match failure {
                            0 => Err(Error::InvalidGraph),
                            1 => panic!("partial token initializer"),
                            _ => Ok(Header::new(99)),
                        }
                    })
                }));
                match failure {
                    0 => assert!(matches!(result, Ok(Err(Error::InvalidGraph)))),
                    1 => assert!(result.is_err()),
                    _ => assert!(matches!(
                        result,
                        Ok(Err(Error::TokenKindMismatch {
                            cached: 99,
                            requested: 2
                        }))
                    )),
                }
                let (node, auxiliary) = staged.unwrap();
                assert!(matches!(owner.node(node), Err(Error::InvalidSlot)));
                assert!(matches!(
                    owner.view().aux(auxiliary),
                    Err(Error::InvalidSlot)
                ));
                failed_ids.push((node, auxiliary));
                assert_eq!(dropped.load(Ordering::Relaxed), failure + 1);
            }
            let token = owner
                .try_token_prepared(key, 2, |transaction, selected_parent| {
                    let auxiliary = transaction.push_aux(Payload {
                        value: 7,
                        dropped: dropped.clone(),
                    });
                    Ok(Header {
                        parent: Some(selected_parent),
                        payload: Some(auxiliary),
                        ..Header::new(2)
                    })
                })
                .unwrap()
                .retain();
            assert_eq!(token.parent, Some(parent));
            let auxiliary = token.payload.unwrap();
            assert_eq!(auxiliary_value(owner.view().aux(auxiliary).unwrap()), 7);
            for (node, failed_auxiliary) in failed_ids {
                assert_ne!(node, token.id());
                assert_ne!(failed_auxiliary, auxiliary);
                assert!(matches!(owner.node(node), Err(Error::InvalidSlot)));
                assert!(matches!(
                    owner.view().aux(failed_auxiliary),
                    Err(Error::InvalidSlot)
                ));
            }
            assert_eq!(
                owner
                    .try_token_prepared(key, 2, |_, _| panic!("cache hit"))
                    .unwrap()
                    .id(),
                token.id()
            );
            assert!(matches!(
                owner.try_token_prepared(key, 3, |_, _| panic!("kind checked before callback")),
                Err(Error::TokenKindMismatch {
                    cached: 2,
                    requested: 3
                })
            ));
            drop(owner);
            assert_eq!(
                auxiliary_value(token.owner().view().aux(auxiliary).unwrap()),
                7
            );
            assert_eq!(dropped.load(Ordering::Relaxed), 3);
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 4);
        assert_eq!(counters.snapshot(), baseline);
    }

    #[test]
    fn prepared_token_context_selects_imported_owner_and_checks_staged_mutation() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let counters = Counters::new();
        let mut builder = StorageBuilder::<Header>::new(Arc::from(&b"abc"[..]), &counters);
        let parent = builder.push(Header::new(1));
        let reparsed = builder.push(Header {
            reparsed: true,
            ..Header::new(1)
        });
        let core_aux = builder.push_aux(1);
        let (core_value, store) = builder.aux_and_store_mut(core_aux).unwrap();
        *core_value = 7;
        assert!(store.is_empty());
        let missing_core_aux = AuxId::from_parts(core_aux.arena(), u32::MAX).unwrap();
        assert!(matches!(
            builder.aux_and_store_mut(missing_core_aux),
            Err(Error::InvalidSlot)
        ));
        let (store, source) = builder.store_and_source_mut();
        store.extend_from_slice(source.as_bytes());
        let (node, store, source) = builder.node_store_and_source_mut(parent).unwrap();
        node.payload = Some(core_aux);
        assert_eq!(store, source.as_bytes());
        let foreign_builder = StorageBuilder::<Header>::new(Arc::from(&b"x"[..]), &counters);
        let foreign = NodeId::from_parts(foreign_builder.id().arena(), parent.slot()).unwrap();
        let foreign_aux =
            AuxId::from_parts(foreign_builder.view().auxiliary_arena(), u32::MAX).unwrap();
        assert!(matches!(
            builder.aux_and_store_mut(foreign_aux),
            Err(Error::WrongOwner)
        ));
        assert!(matches!(
            builder.node_store_and_source_mut(foreign),
            Err(Error::WrongOwner)
        ));
        let owner = builder.finish();
        let mut importer = StorageBuilder::<Header>::new(Arc::from(&b""[..]), &counters);
        importer.retain_file(owner.clone());
        let importer = importer.finish();
        let (read, selected) = importer.view().aux_with_owner(core_aux).unwrap();
        assert!(matches!(read, AuxiliaryRead::Core(&7)));
        assert_eq!(selected.id(), owner.id());
        assert!(matches!(
            importer.view().aux(missing_core_aux),
            Err(Error::InvalidSlot)
        ));
        assert!(matches!(
            importer.view().aux(foreign_aux),
            Err(Error::WrongOwner)
        ));
        assert!(matches!(
            importer.view().try_token_prepared(
                TokenKey {
                    parent: foreign,
                    start: 0,
                    end: 0
                },
                2,
                |_, _| panic!("foreign parent rejected before callback")
            ),
            Err(Error::WrongOwner)
        ));
        assert!(matches!(
            importer.view().try_token_prepared(
                TokenKey {
                    parent,
                    start: 0,
                    end: 4
                },
                2,
                |_, _| panic!("range rejected before callback")
            ),
            Err(Error::InvalidTokenRange)
        ));
        assert!(matches!(
            importer.view().try_token_prepared(
                TokenKey {
                    parent: reparsed,
                    start: 0,
                    end: 4
                },
                2,
                |_, _| panic!("reparsed checked before range and callback")
            ),
            Err(Error::ReparsedParent)
        ));
        let mut staged_node = None;
        let token = importer
            .view()
            .try_token_prepared(
                TokenKey {
                    parent,
                    start: 1,
                    end: 3,
                },
                2,
                |transaction, selected_parent| {
                    assert_eq!(selected_parent, parent);
                    assert_eq!(transaction.owner_id(), owner.id());
                    assert_eq!(transaction.source().as_bytes(), b"abc");
                    assert_eq!(transaction.store(), b"abc");
                    assert_eq!(transaction.core_auxiliary_arena(), core_aux.arena());
                    assert!(matches!(
                        transaction.aux(core_aux)?,
                        AuxiliaryRead::Core(&7)
                    ));
                    // Ordinary core reads remain available under the lazy lock.
                    assert!(matches!(
                        owner.view().aux(core_aux)?,
                        AuxiliaryRead::Core(&7)
                    ));
                    assert!(matches!(
                        transaction.aux(missing_core_aux),
                        Err(Error::InvalidSlot)
                    ));
                    assert!(matches!(
                        transaction.aux(foreign_aux),
                        Err(Error::WrongOwner)
                    ));
                    assert_eq!(
                        transaction.lazy_auxiliary_arena(),
                        owner.lazy_auxiliary_arena()
                    );
                    let auxiliary = transaction.push_aux(Payload {
                        value: 2,
                        dropped: dropped.clone(),
                    });
                    let node = transaction.push(Header::new(4));
                    staged_node = Some(node);
                    let missing_node = NodeId::from_parts(node.arena(), u32::MAX).unwrap();
                    let missing_aux = AuxId::from_parts(auxiliary.arena(), u32::MAX).unwrap();
                    assert!(matches!(
                        transaction.node_and_aux_mut(missing_node, auxiliary),
                        Err(Error::InvalidSlot)
                    ));
                    assert!(matches!(
                        transaction.node_and_aux_mut(node, missing_aux),
                        Err(Error::InvalidSlot)
                    ));
                    assert!(matches!(
                        transaction.node_and_aux_mut(parent, auxiliary),
                        Err(Error::WrongOwner)
                    ));
                    assert!(matches!(
                        transaction.node_and_aux_mut(node, core_aux),
                        Err(Error::WrongOwner)
                    ));
                    let (header, payload) = transaction.node_and_aux_mut(node, auxiliary)?;
                    header.parent = Some(parent);
                    header.payload = Some(auxiliary);
                    payload.value = 9;
                    assert_eq!(transaction.node(node)?.parent, Some(parent));
                    match transaction.aux(auxiliary)? {
                        AuxiliaryRead::Lazy(value) => {
                            assert_eq!(value.value, 9);
                            assert!(value.as_borrowed().is_some());
                        }
                        AuxiliaryRead::Core(_) => panic!("staged auxiliary uses full lazy storage"),
                    }
                    Ok(Header {
                        parent: Some(parent),
                        payload: Some(auxiliary),
                        ..Header::new(2)
                    })
                },
            )
            .unwrap();
        assert_eq!(token.parent, Some(parent));
        let (read, selected) = importer
            .view()
            .aux_with_owner(token.payload.unwrap())
            .unwrap();
        match read {
            AuxiliaryRead::Lazy(value) => {
                assert_eq!(value.value, 9);
                assert!(value.as_borrowed().is_none());
            }
            AuxiliaryRead::Core(_) => panic!("published lazy auxiliary keeps its page guard"),
        }
        assert_eq!(selected.id(), owner.id());
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        let prior_aux = token.payload.unwrap();
        owner
            .try_token_prepared(
                TokenKey {
                    parent,
                    start: 0,
                    end: 1,
                },
                2,
                |transaction, _| {
                    match transaction.aux(prior_aux)? {
                        AuxiliaryRead::Lazy(value) => {
                            assert_eq!(value.value, 9);
                            assert!(value.as_borrowed().is_some());
                        }
                        AuxiliaryRead::Core(_) => {
                            panic!("prior lazy auxiliary remains full storage")
                        }
                    }
                    let auxiliary = transaction.push_aux(Payload {
                        value: 3,
                        dropped: dropped.clone(),
                    });
                    let node = transaction.push(Header::new(4));
                    assert!(matches!(
                        transaction.node_and_aux_mut(staged_node.unwrap(), auxiliary),
                        Err(Error::InvalidSlot)
                    ));
                    assert!(matches!(
                        transaction.node_and_aux_mut(node, prior_aux),
                        Err(Error::InvalidSlot)
                    ));
                    Ok(Header::new(2))
                },
            )
            .unwrap();
    }

    #[test]
    fn full_record_token_checks_kind_before_assigning_parent() {
        struct Full {
            kind: u32,
            writes: Arc<AtomicUsize>,
        }
        impl NodeRecord for Full {
            type CoreAux = ();
            type Aux = ();
            type Store = ();
            fn storage_kind(&self) -> u32 {
                self.kind
            }
            fn storage_reparsed(&self) -> bool {
                false
            }
        }
        impl NodeParentRecord for Full {
            fn set_storage_parent(&mut self, parent: Option<NodeId>) {
                assert!(parent.is_some());
                self.writes.fetch_add(1, Ordering::Relaxed);
            }
        }
        let writes = Arc::new(AtomicUsize::new(0));
        let mut builder = StorageBuilder::<Full>::new(Arc::from(&b"x"[..]), &Counters::new());
        let parent = builder.push(Full {
            kind: 1,
            writes: writes.clone(),
        });
        let owner = builder.finish();
        let key = TokenKey {
            parent,
            start: 0,
            end: 1,
        };
        assert!(matches!(
            owner.try_token_record(key, 2, || Full {
                kind: 3,
                writes: writes.clone()
            }),
            Err(Error::TokenKindMismatch {
                cached: 3,
                requested: 2
            })
        ));
        assert_eq!(writes.load(Ordering::Relaxed), 0);
        owner
            .try_token_record(key, 2, || Full {
                kind: 2,
                writes: writes.clone(),
            })
            .unwrap();
        assert_eq!(writes.load(Ordering::Relaxed), 1);
        owner.try_token_record(key, 2, || panic!("cached")).unwrap();
        assert_eq!(writes.load(Ordering::Relaxed), 1);
    }
}
