//! Concurrent map operations, including atomic load-or-store and callbacks that
//! can reenter the map. Range snapshots keys, then resolves each current value
//! outside the callback; deleted entries may disappear and new keys may be
//! omitted. It never holds a lock while calling user code.
use std::{borrow::Borrow, collections::HashMap, hash::Hash, sync::RwLock};

/// Source type: tsc/internal/collections/syncmap.go:SyncMap
#[derive(Debug)]
pub struct SyncMap<K, V> {
    entries: RwLock<HashMap<K, V>>,
}
impl<K, V> Default for SyncMap<K, V> {
    fn default() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }
}
impl<K, V> SyncMap<K, V> {
    /// port: tsc/internal/collections/syncmap.go:SyncMap.Size
    pub fn len(&self) -> usize {
        self.entries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// port: tsc/internal/collections/syncmap.go:SyncMap.Clear
    pub fn clear(&self) {
        self.entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}
impl<K: Eq + Hash, V> SyncMap<K, V> {
    /// port: tsc/internal/collections/syncmap.go:SyncMap.Store
    pub fn store(&self, key: K, value: V) {
        self.entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, value);
    }
    /// port: tsc/internal/collections/syncmap.go:SyncMap.Delete
    pub fn delete<Q: Hash + Eq + ?Sized>(&self, key: &Q)
    where
        K: Borrow<Q>,
    {
        self.entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key);
    }
}
impl<K: Eq + Hash, V: Clone> SyncMap<K, V> {
    /// The outer Option is presence. A nullable V retains a present nil value.
    /// port: tsc/internal/collections/syncmap.go:SyncMap.Load
    pub fn load<Q: Hash + Eq + ?Sized>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        self.entries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
    }
    /// port: tsc/internal/collections/syncmap.go:SyncMap.LoadOrStore
    pub fn load_or_store(&self, key: K, value: V) -> (V, bool) {
        use std::collections::hash_map::Entry;
        match self
            .entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(key)
        {
            Entry::Occupied(e) => (e.get().clone(), true),
            Entry::Vacant(e) => (e.insert(value).clone(), false),
        }
    }
}
impl<K: Eq + Hash + Clone, V: Clone> SyncMap<K, V> {
    /// port: tsc/internal/collections/syncmap.go:SyncMap.Keys
    pub fn keys(&self) -> Vec<K> {
        self.entries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }
    /// port: tsc/internal/collections/syncmap.go:SyncMap.Range
    pub fn range(&self, mut visit: impl FnMut(K, V) -> bool) {
        for key in self.keys() {
            if let Some(value) = self.load(&key) {
                if !visit(key, value) {
                    break;
                }
            }
        }
    }
    /// Rust typed values need no runtime assertion. Erased Go interface values
    /// use `try_to_map` to apply their assertion, including nil rejection.
    /// port: tsc/internal/collections/syncmap.go:SyncMap.ToMap
    pub fn to_map(&self) -> HashMap<K, V> {
        self.try_to_map::<std::convert::Infallible>(Ok).unwrap()
    }
    pub fn try_to_map<E>(
        &self,
        mut assert_value: impl FnMut(V) -> Result<V, E>,
    ) -> Result<HashMap<K, V>, E> {
        let mut out = HashMap::with_capacity(self.len());
        let mut error = None;
        self.range(|key, value| match assert_value(value) {
            Ok(value) => {
                out.insert(key, value);
                true
            }
            Err(e) => {
                error = Some(e);
                false
            }
        });
        error.map_or(Ok(out), Err)
    }
}
impl<K: Clone, V: Clone> Clone for SyncMap<K, V> {
    /// port: tsc/internal/collections/syncmap.go:SyncMap.Clone
    fn clone(&self) -> Self {
        Self {
            entries: RwLock::new(
                self.entries
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            ),
        }
    }
}
