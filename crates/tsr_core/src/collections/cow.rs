//! Nested name scopes from `internal/collections/cow.go`.
//!
//! Empty maps allocate nothing. A fork or scope retains the inherited backing;
//! reads borrow directly and the first write clones it. `Arc` keeps these
//! collections Send/Sync when their contents are, without locking lookups.

use std::borrow::Borrow;
use std::collections::{hash_map::RandomState, HashMap};
use std::hash::{BuildHasher, Hash};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

/// A map with independent writes and shared inherited storage.
///
/// Cloning retains the backing in O(1); either copy clones its entries on its
/// first write while the other remains alive. Values are shallow-cloned, so
/// shared objects stored as values retain their own identity, as in Go.
/// `enter_scope` additionally restores the parent on normal return or unwind.
#[derive(Debug)]
pub struct CopyOnWriteMap<K, V, S = RandomState> {
    backing: Option<Arc<HashMap<K, V, S>>>,
}

impl<K, V, S> Default for CopyOnWriteMap<K, V, S> {
    fn default() -> Self {
        Self { backing: None }
    }
}

// No K/V/S Clone bound: sharing the allocation does not clone its entries.
impl<K, V, S> Clone for CopyOnWriteMap<K, V, S> {
    fn clone(&self) -> Self {
        Self {
            backing: self.backing.clone(),
        }
    }
}

impl<K, V, S> CopyOnWriteMap<K, V, S> {
    /// Save this map until the returned scope is dropped. Mutate through the
    /// scope; the exclusive borrow prevents out-of-order restoration or use
    /// of the parent while a child is active. Nested scopes restore one level
    /// at a time. As with other RAII guards, forgetting it skips restoration.
    ///
    /// port: tsc/internal/collections/cow.go:CopyOnWriteMap.EnterScope
    pub fn enter_scope(&mut self) -> CopyOnWriteMapScope<'_, K, V, S> {
        let saved = self.clone();
        CopyOnWriteMapScope {
            current: self,
            saved,
        }
    }
}

impl<K: Eq + Hash, V, S: BuildHasher> CopyOnWriteMap<K, V, S> {
    /// port: tsc/internal/collections/cow.go:CopyOnWriteMap.Get
    pub fn get<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.backing.as_ref()?.get(key)
    }

    /// port: tsc/internal/collections/cow.go:CopyOnWriteMap.Has
    pub fn contains_key<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.get(key).is_some()
    }
}

impl<K: Eq + Hash + Clone, V: Clone, S: BuildHasher + Clone + Default> CopyOnWriteMap<K, V, S> {
    /// port: tsc/internal/collections/cow.go:CopyOnWriteMap.ensureOwned
    fn ensure_owned(&mut self) -> &mut HashMap<K, V, S> {
        Arc::make_mut(
            self.backing
                .get_or_insert_with(|| Arc::new(HashMap::default())),
        )
    }

    /// Set a value, returning the previous value if it was present. Inherited
    /// entries are cloned even when this overwrites an equal value, as in Go.
    ///
    /// port: tsc/internal/collections/cow.go:CopyOnWriteMap.Set
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.ensure_owned().insert(key, value)
    }
}

/// An exclusive child scope. Dropping it restores the saved backing.
pub struct CopyOnWriteMapScope<'a, K, V, S = RandomState> {
    current: &'a mut CopyOnWriteMap<K, V, S>,
    saved: CopyOnWriteMap<K, V, S>,
}

impl<K, V, S> Deref for CopyOnWriteMapScope<'_, K, V, S> {
    type Target = CopyOnWriteMap<K, V, S>;
    fn deref(&self) -> &Self::Target {
        self.current
    }
}

impl<K, V, S> DerefMut for CopyOnWriteMapScope<'_, K, V, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.current
    }
}

impl<K, V, S> Drop for CopyOnWriteMapScope<'_, K, V, S> {
    fn drop(&mut self) {
        self.current.backing = self.saved.backing.take();
    }
}

/// A scoped set with the same sharing and restoration rules as its map.
#[derive(Debug)]
pub struct CopyOnWriteSet<K, S = RandomState> {
    map: CopyOnWriteMap<K, (), S>,
}

impl<K, S> Default for CopyOnWriteSet<K, S> {
    fn default() -> Self {
        Self {
            map: CopyOnWriteMap::default(),
        }
    }
}

impl<K, S> Clone for CopyOnWriteSet<K, S> {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
        }
    }
}

impl<K, S> CopyOnWriteSet<K, S> {
    /// port: tsc/internal/collections/cow.go:CopyOnWriteSet.EnterScope
    pub fn enter_scope(&mut self) -> CopyOnWriteSetScope<'_, K, S> {
        let saved = self.clone();
        CopyOnWriteSetScope {
            current: self,
            saved,
        }
    }
}

impl<K: Eq + Hash, S: BuildHasher> CopyOnWriteSet<K, S> {
    /// port: tsc/internal/collections/cow.go:CopyOnWriteSet.Has
    pub fn contains<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.map.contains_key(key)
    }
}

impl<K: Eq + Hash + Clone, S: BuildHasher + Clone + Default> CopyOnWriteSet<K, S> {
    /// port: tsc/internal/collections/cow.go:CopyOnWriteSet.Add
    pub fn insert(&mut self, key: K) -> bool {
        self.map.insert(key, ()).is_none()
    }
}

/// An exclusive child set scope, restored on drop.
pub struct CopyOnWriteSetScope<'a, K, S = RandomState> {
    current: &'a mut CopyOnWriteSet<K, S>,
    saved: CopyOnWriteSet<K, S>,
}

impl<K, S> Deref for CopyOnWriteSetScope<'_, K, S> {
    type Target = CopyOnWriteSet<K, S>;
    fn deref(&self) -> &Self::Target {
        self.current
    }
}

impl<K, S> DerefMut for CopyOnWriteSetScope<'_, K, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.current
    }
}

impl<K, S> Drop for CopyOnWriteSetScope<'_, K, S> {
    fn drop(&mut self) {
        self.current.map.backing = self.saved.map.backing.take();
    }
}

#[cfg(test)]
mod tests;
