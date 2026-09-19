//! Checker-local side tables (`tsc/internal/checker/links.go`).
//!
//! Upstream keys link stores by node or symbol id and stores values in pages of
//! 256 (`core.PagedLinkStore`), either inline (`nodeLinkStore`) or through an
//! arena pointer for large values (`symbolArenaLinkStore`). Here one store
//! serves both uses: the key carries the owning arena and slot, pages are
//! allocated on first use per arena, and whether `V` is boxed is the caller's
//! choice. Upstream's `Get` creates a default entry; `TryGet` and `Has` do not.
//!
//! The per-arena directory is a hash map for now. Plan P1 measures whether the
//! handful of arenas a checker touches justifies something flatter; the count
//! of allocated pages is exposed for that census.

use crate::types::FastState;
use std::marker::PhantomData;
use ts_arena::{ArenaId, NodeId, SymbolId};

const PAGE_SIZE: usize = 256;

/// An arena-qualified slot identity that can key a link store.
pub trait LinkKey: Copy {
    fn arena(self) -> ArenaId;
    /// The one-based published slot; zero is never live.
    fn slot(self) -> u32;
}

impl LinkKey for NodeId {
    fn arena(self) -> ArenaId {
        NodeId::arena(self)
    }
    fn slot(self) -> u32 {
        NodeId::slot(self)
    }
}

impl LinkKey for SymbolId {
    fn arena(self) -> ArenaId {
        SymbolId::arena(self)
    }
    fn slot(self) -> u32 {
        SymbolId::slot(self)
    }
}

type Page<V> = Box<[Option<V>]>;

pub struct LinkStore<K: LinkKey, V> {
    arenas: hashbrown::HashMap<ArenaId, Vec<Option<Page<V>>>, FastState>,
    len: usize,
    _key: PhantomData<K>,
}

impl<K: LinkKey, V> Default for LinkStore<K, V> {
    fn default() -> Self {
        Self {
            arenas: hashbrown::HashMap::default(),
            len: 0,
            _key: PhantomData,
        }
    }
}

fn position(key: impl LinkKey) -> (usize, usize) {
    let index = key.slot() as usize - 1;
    (index / PAGE_SIZE, index % PAGE_SIZE)
}

impl<K: LinkKey, V> LinkStore<K, V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Entries with a value.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[cfg(any(test, feature = "storage-pilot"))]
    pub(crate) fn values(&self) -> impl Iterator<Item = &V> {
        self.arenas
            .values()
            .flat_map(|directory| directory.iter().flatten())
            .flat_map(|page| page.iter().flatten())
    }

    /// Bytes held by the pages, the per-arena directories and the arena map,
    /// for storage censuses.
    pub fn structural_bytes(&self) -> usize {
        self.arenas.allocation_size()
            + self
                .arenas
                .values()
                .map(|directory| {
                    directory.capacity() * size_of::<Option<Page<V>>>()
                        + directory.iter().flatten().count() * PAGE_SIZE * size_of::<Option<V>>()
                })
                .sum::<usize>()
    }

    /// Allocated pages across all arenas, for capacity accounting.
    pub fn pages(&self) -> usize {
        self.arenas
            .values()
            .map(|directory| directory.iter().filter(|page| page.is_some()).count())
            .sum()
    }

    // port: tsc/internal/checker/links.go:nodeLinkStore.TryGet
    // port: tsc/internal/checker/links.go:symbolArenaLinkStore.TryGet
    pub fn try_get(&self, key: K) -> Option<&V> {
        let (page, offset) = position(key);
        self.arenas
            .get(&key.arena())?
            .get(page)?
            .as_ref()?
            .get(offset)?
            .as_ref()
    }

    pub fn try_get_mut(&mut self, key: K) -> Option<&mut V> {
        let (page, offset) = position(key);
        self.arenas
            .get_mut(&key.arena())?
            .get_mut(page)?
            .as_mut()?
            .get_mut(offset)?
            .as_mut()
    }

    // port: tsc/internal/checker/links.go:nodeLinkStore.Has
    // port: tsc/internal/checker/links.go:symbolArenaLinkStore.Has
    pub fn has(&self, key: K) -> bool {
        self.try_get(key).is_some()
    }

    /// Returns the entry, creating `V::default()` on first use. The page and the
    /// arena directory are allocated here, not when the store is created.
    // port: tsc/internal/checker/links.go:nodeLinkStore.Get
    // port: tsc/internal/checker/links.go:symbolArenaLinkStore.Get
    pub fn get_or_default(&mut self, key: K) -> &mut V
    where
        V: Default,
    {
        let (page, offset) = position(key);
        let directory = self.arenas.entry(key.arena()).or_default();
        if directory.len() <= page {
            directory.resize_with(page + 1, || None);
        }
        let entries = directory[page]
            .get_or_insert_with(|| (0..PAGE_SIZE).map(|_| None).collect::<Vec<_>>().into());
        let entry = &mut entries[offset];
        if entry.is_none() {
            *entry = Some(V::default());
            self.len += 1;
        }
        entry.as_mut().expect("entry was just filled")
    }
}
