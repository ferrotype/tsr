use crate::{
    lazy::LazyRecord, Error, Node, NodeId, StorageHandle, StorageTransaction, SymbolId, TokenKey,
};
use crate::{NodeParentRecord, NodeRecord};
use std::{fmt, ops::Deref, sync::Arc};

enum Location<N: NodeRecord> {
    Core(u32),
    ImportedCore(crate::ArenaId, u32),
    Lazy(LazyRecord<N>),
}

impl<N: NodeRecord> Clone for Location<N> {
    fn clone(&self) -> Self {
        match self {
            Self::Core(slot) => Self::Core(*slot),
            Self::ImportedCore(arena, slot) => Self::ImportedCore(*arena, *slot),
            Self::Lazy(node) => Self::Lazy(node.clone()),
        }
    }
}

impl<N: NodeRecord> Location<N> {
    fn get<'a, S>(&'a self, owner: &'a StorageHandle<N, S>) -> &'a N {
        match self {
            Self::Core(slot) => owner.core.get_slot(*slot).expect("validated core slot"),
            Self::ImportedCore(arena, slot) => owner
                .retained_owner(*arena)
                .expect("validated retained owner")
                .core
                .get_slot(*slot)
                .expect("validated imported core slot"),
            Self::Lazy(node) => node.get(),
        }
    }
}

/// A checked borrow of a node. Core lookup does not clone an Arc or acquire a lock.
pub struct RecordRef<'a, N: NodeRecord, S = ()> {
    owner: &'a StorageHandle<N, S>,
    id: NodeId,
    location: Location<N>,
}

impl<N: NodeRecord, S> RecordRef<'_, N, S> {
    pub fn id(&self) -> NodeId {
        self.id
    }
    pub fn owner(&self) -> &StorageHandle<N, S> {
        self.owner
    }

    /// Explicitly retain the enclosing file or mapped bundle for an escaped result.
    pub fn retain(self) -> RetainedRecord<N, S> {
        RetainedRecord {
            owner: self.owner.clone(),
            id: self.id,
            location: self.location,
        }
    }
}

impl<N: NodeRecord, S> Deref for RecordRef<'_, N, S> {
    type Target = N;
    fn deref(&self) -> &Self::Target {
        self.location.get(self.owner)
    }
}

impl<N: NodeRecord + fmt::Debug, S> fmt::Debug for RecordRef<'_, N, S> {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(output)
    }
}

/// An escaped node plus its complete retention root, including mapped siblings.
pub struct RetainedRecord<N: NodeRecord, S = ()> {
    owner: StorageHandle<N, S>,
    id: NodeId,
    location: Location<N>,
}

impl<N: NodeRecord + fmt::Debug, S> fmt::Debug for RetainedRecord<N, S> {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(output)
    }
}

impl<N: NodeRecord, S> RetainedRecord<N, S> {
    pub fn id(&self) -> NodeId {
        self.id
    }
    pub fn owner(&self) -> &StorageHandle<N, S> {
        &self.owner
    }
}

impl<N: NodeRecord, S> Clone for RetainedRecord<N, S> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            id: self.id,
            location: self.location.clone(),
        }
    }
}

impl<N: NodeRecord, S> Deref for RetainedRecord<N, S> {
    type Target = N;
    fn deref(&self) -> &Self::Target {
        self.location.get(&self.owner)
    }
}

/// Checked symbol access borrows the scope; `retain` opts into an escaped owner root.
pub struct StorageSymbolRef<'a, N: NodeRecord, S> {
    owner: &'a StorageHandle<N, S>,
    id: SymbolId,
}

impl<N: NodeRecord, S> fmt::Debug for StorageSymbolRef<'_, N, S> {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output
            .debug_tuple("StorageSymbolRef")
            .field(&self.id)
            .finish()
    }
}

impl<N: NodeRecord, S> StorageSymbolRef<'_, N, S> {
    pub fn id(&self) -> SymbolId {
        self.id
    }
    pub fn retain(self) -> RetainedStorageSymbol<N, S> {
        RetainedStorageSymbol {
            owner: self.owner.clone(),
            id: self.id,
        }
    }
}

impl<N: NodeRecord, S> Deref for StorageSymbolRef<'_, N, S> {
    type Target = S;
    fn deref(&self) -> &S {
        self.owner
            .symbols
            .get_slot(self.id.slot())
            .expect("validated symbol slot")
    }
}

/// An escaped symbol retaining its file or mapped bundle.
pub struct RetainedStorageSymbol<N: NodeRecord, S> {
    owner: StorageHandle<N, S>,
    id: SymbolId,
}

impl<N: NodeRecord, S> fmt::Debug for RetainedStorageSymbol<N, S> {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output
            .debug_tuple("RetainedStorageSymbol")
            .field(&self.id)
            .finish()
    }
}

impl<N: NodeRecord, S> RetainedStorageSymbol<N, S> {
    pub fn id(&self) -> SymbolId {
        self.id
    }
}

impl<N: NodeRecord, S> Deref for RetainedStorageSymbol<N, S> {
    type Target = S;
    fn deref(&self) -> &S {
        self.owner
            .symbols
            .get_slot(self.id.slot())
            .expect("validated symbol slot")
    }
}

/// Cached ids are shared without copying the list. Only returned lists retain a file.
pub struct CachedNodes<N: NodeRecord, S = ()> {
    owner: StorageHandle<N, S>,
    ids: Arc<[NodeId]>,
}

impl<N: NodeRecord, S> fmt::Debug for CachedNodes<N, S> {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.debug_tuple("CachedNodes").field(&self.ids).finish()
    }
}

impl<N: NodeRecord, S> CachedNodes<N, S> {
    pub fn ids(&self) -> &[NodeId] {
        &self.ids
    }
    pub fn node(&self, index: usize) -> Result<RecordRef<'_, N, S>, Error> {
        self.owner
            .node(*self.ids.get(index).ok_or(Error::InvalidSlot)?)
    }
}

impl<N: NodeRecord, S> StorageHandle<N, S> {
    /// Borrow a node anywhere in this explicit retention graph, including mapped
    /// siblings and published imports. `node` retains the S04 owner-local API.
    pub fn resolved_node(&self, id: NodeId) -> Result<RecordRef<'_, N, S>, Error> {
        if id.arena() == self.core.id || id.arena() == self.lazy_arena() {
            return self.node(id);
        }
        let owner = self.retained_owner(id.arena()).ok_or(Error::WrongOwner)?;
        let location = if id.arena() == owner.core.id {
            owner.core.get_slot(id.slot())?;
            Location::ImportedCore(id.arena(), id.slot())
        } else if id.arena() == owner.lazy_arena() {
            Location::Lazy(owner.lazy.node(id)?)
        } else {
            return Err(Error::WrongOwner);
        };
        Ok(RecordRef {
            owner: self,
            id,
            location,
        })
    }
    pub fn node(&self, id: NodeId) -> Result<RecordRef<'_, N, S>, Error> {
        let location = if id.arena() == self.core.id {
            self.core.get_slot(id.slot())?;
            Location::Core(id.slot())
        } else if id.arena() == self.lazy_arena() {
            Location::Lazy(self.lazy.node(id)?)
        } else {
            return Err(Error::WrongOwner);
        };
        Ok(RecordRef {
            owner: self,
            id,
            location,
        })
    }

    pub fn retain_node(&self, id: NodeId) -> Result<RetainedRecord<N, S>, Error> {
        self.node(id).map(RecordRef::retain)
    }

    pub fn symbol(&self, id: SymbolId) -> Result<StorageSymbolRef<'_, N, S>, Error> {
        self.symbols.get(id.arena(), id.slot())?;
        Ok(StorageSymbolRef { owner: self, id })
    }

    /// Runs the winning initializer under this file's lazy write lock. The callback
    /// panics on same-thread reentry to this file's lazy APIs. The callback must not
    /// wait for another thread to use those APIs. Failure burns every provisional id.
    pub fn jsdoc(
        &self,
        parent: NodeId,
        initialize: impl FnOnce(&mut StorageTransaction<'_, N>) -> Result<Vec<NodeId>, Error>,
    ) -> Result<CachedNodes<N, S>, Error> {
        self.node(parent)?;
        let ids = self.lazy.jsdoc(None, parent, self, initialize)?;
        Ok(CachedNodes {
            owner: self.clone(),
            ids,
        })
    }

    /// Source-token access with upstream invariant panics. The initializer must
    /// not wait for another thread to use this file's lazy APIs; same-thread reentry
    /// panics before locking. A cache hit never calls the initializer.
    pub fn token_record(
        &self,
        key: TokenKey,
        kind: u32,
        initialize: impl FnOnce() -> N,
    ) -> Result<RecordRef<'_, N, S>, Error>
    where
        N: NodeParentRecord,
    {
        match self.try_token_record(key, kind, initialize) {
            Err(
                error @ (Error::TokenKindMismatch { .. }
                | Error::ReparsedParent
                | Error::InvalidTokenRange),
            ) => panic!("{error}"),
            result => result,
        }
    }

    /// Fallible invariant checks; the parent flags always come from retained storage.
    pub fn try_token_record(
        &self,
        key: TokenKey,
        kind: u32,
        initialize: impl FnOnce() -> N,
    ) -> Result<RecordRef<'_, N, S>, Error>
    where
        N: NodeParentRecord,
    {
        let parent = self.node(key.parent)?;
        let id = self
            .lazy
            .token(key, kind, parent.storage_reparsed(), self, initialize)?;
        self.node(id)
    }

    /// Fallible token initialization with owner-aware auxiliary staging. The
    /// callback must prepare the parent link and must not reenter this file's
    /// lazy APIs. All staged records publish together with the cache entry.
    pub fn try_token_prepared(
        &self,
        key: TokenKey,
        kind: u32,
        initialize: impl FnOnce(&mut StorageTransaction<'_, N>, NodeId) -> Result<N, Error>,
    ) -> Result<RecordRef<'_, N, S>, Error> {
        let parent = self.node(key.parent)?;
        let id =
            self.lazy
                .token_prepared(key, kind, parent.storage_reparsed(), self, initialize)?;
        self.node(id)
    }
}

/// A checked, non-retaining owner borrow. The lazy variant owns only a page and
/// remains lifetime-bound to the enclosing view; escaping requires a file handle.
pub struct StorageRead<'a, T> {
    value: ReadLocation<'a, T>,
}

/// A scoped auxiliary read preserves the distinction between compact core data
/// and full lazy values. Consumers decode core records using their selected
/// physical owner's store; a lazy guard keeps its own published page alive.
///
/// ```compile_fail
/// use ts_arena::{AuxId, AuxiliaryRead, Node, StorageBuilder};
/// fn escape(builder: StorageBuilder<Node<()>>, id: AuxId) -> AuxiliaryRead<'static, Node<()>> {
///     let owner = builder.finish();
///     owner.view().aux(id).unwrap()
/// }
/// ```
pub enum AuxiliaryRead<'a, N: NodeRecord> {
    Core(&'a N::CoreAux),
    Lazy(StorageRead<'a, N::Aux>),
}
enum ReadLocation<'a, T> {
    Borrowed(&'a T),
    Lazy(LazyRecord<T>),
}
impl<'a, T> StorageRead<'a, T> {
    /// Core records borrow the owner directly. Lazy page reads instead retain
    /// their page and cannot extend a borrow beyond this guard.
    pub fn as_borrowed(&self) -> Option<&'a T> {
        match &self.value {
            ReadLocation::Borrowed(value) => Some(*value),
            ReadLocation::Lazy(_) => None,
        }
    }

    pub fn borrowed(value: &'a T) -> Self {
        Self {
            value: ReadLocation::Borrowed(value),
        }
    }
    pub(crate) fn lazy(value: LazyRecord<T>) -> Self {
        Self {
            value: ReadLocation::Lazy(value),
        }
    }
}
impl<T> Deref for StorageRead<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        match &self.value {
            ReadLocation::Borrowed(value) => value,
            ReadLocation::Lazy(value) => value.get(),
        }
    }
}
impl<T: fmt::Debug> fmt::Debug for StorageRead<'_, T> {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(output)
    }
}

impl<N, S> StorageHandle<Node<N>, S> {
    /// S04 payload adapter. Concrete runtime records use `token_record`.
    pub fn token(
        &self,
        key: TokenKey,
        kind: u32,
        initialize: impl FnOnce() -> N,
    ) -> Result<RecordRef<'_, Node<N>, S>, Error> {
        self.token_record(key, kind, || Node::new(kind, initialize()))
    }
    pub fn try_token(
        &self,
        key: TokenKey,
        kind: u32,
        initialize: impl FnOnce() -> N,
    ) -> Result<RecordRef<'_, Node<N>, S>, Error> {
        self.try_token_record(key, kind, || Node::new(kind, initialize()))
    }
}
