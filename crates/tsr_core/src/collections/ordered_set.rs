use super::OrderedMap;
use std::borrow::Borrow;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};

/// An insertion-ordered set, with map-equivalent delete/reinsert and cloning.
#[derive(Debug)]
pub struct OrderedSet<K, S = RandomState> {
    map: OrderedMap<K, (), S>,
}

impl<K: Clone, S: Clone> Clone for OrderedSet<K, S> {
    /// port: tsc/internal/collections/ordered_set.go:OrderedSet.Clone
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
        }
    }
}

impl<K, S: Default> Default for OrderedSet<K, S> {
    fn default() -> Self {
        Self {
            map: OrderedMap::default(),
        }
    }
}

impl<K, S> OrderedSet<K, S> {
    /// port: tsc/internal/collections/ordered_set.go:OrderedSet.Size
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    /// port: tsc/internal/collections/ordered_set.go:OrderedSet.Values
    pub fn values(&self) -> impl ExactSizeIterator<Item = &K> {
        self.map.keys()
    }
    /// port: tsc/internal/collections/ordered_set.go:OrderedSet.Clear
    pub fn clear(&mut self) {
        self.map.clear();
    }
}

impl<K, S: BuildHasher + Default> OrderedSet<K, S> {
    /// port: tsc/internal/collections/ordered_set.go:NewOrderedSetWithSizeHint
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            map: OrderedMap::with_capacity(capacity),
        }
    }
}

impl<K: Eq + Hash, S: BuildHasher> OrderedSet<K, S> {
    /// port: tsc/internal/collections/ordered_set.go:OrderedSet.Has
    pub fn contains<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.map.contains_key(key)
    }
    /// port: tsc/internal/collections/ordered_set.go:OrderedSet.Delete
    pub fn remove<Q: Eq + Hash + ?Sized>(&mut self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.map.remove(key).is_some()
    }
}

impl<K: Eq + Hash + Clone, S: BuildHasher> OrderedSet<K, S> {
    /// port: tsc/internal/collections/ordered_set.go:OrderedSet.Add
    pub fn insert(&mut self, key: K) -> bool {
        self.map.insert(key, ()).is_none()
    }

    /// Mutate through the callback while retaining the native live-iteration rule.
    pub fn visit_values_mut(&mut self, mut visit: impl FnMut(&mut Self, K) -> bool) {
        let mut index = 0;
        while let Some(key) = self.map.key_at(index).cloned() {
            if !visit(self, key) {
                break;
            }
            index += 1;
        }
    }
}
