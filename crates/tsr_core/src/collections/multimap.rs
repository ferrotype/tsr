//! Grouped values retain Go slice-header semantics: a captured view keeps its
//! own length, while removal shifts the shared backing without clearing its
//! old tail. Reallocation detaches future appends from captured views.
use crate::slices::{GoSliceElement, SharedSlice, SliceRead};
use std::{collections::HashMap, hash::Hash};
pub type Values<V> = SharedSlice<V>;

/// Source type: tsc/internal/collections/multimap.go:MultiMap
#[derive(Debug)]
pub struct MultiMap<K, V> {
    entries: HashMap<K, Values<V>>,
}
impl<K, V> Default for MultiMap<K, V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}
impl<K, V> MultiMap<K, V> {
    /// port: tsc/internal/collections/multimap.go:NewMultiMapWithSizeHint
    pub fn with_capacity(size: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(size),
        }
    }
    /// port: tsc/internal/collections/multimap.go:MultiMap.Len
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// port: tsc/internal/collections/multimap.go:MultiMap.Keys
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.keys()
    }
    /// port: tsc/internal/collections/multimap.go:MultiMap.Values
    pub fn values(&self) -> impl Iterator<Item = SliceRead<'_, V>> {
        self.entries.values().map(Values::read)
    }
    /// port: tsc/internal/collections/multimap.go:MultiMap.Clear
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
impl<K: Hash + Eq, V> MultiMap<K, V> {
    /// port: tsc/internal/collections/multimap.go:MultiMap.Has
    pub fn contains_key<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> bool
    where
        K: std::borrow::Borrow<Q>,
    {
        self.entries.contains_key(key)
    }
    /// port: tsc/internal/collections/multimap.go:MultiMap.Get
    pub fn get<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> Option<SliceRead<'_, V>>
    where
        K: std::borrow::Borrow<Q>,
    {
        self.entries.get(key).map(Values::read)
    }
    pub fn retain_values<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> Option<Values<V>>
    where
        K: std::borrow::Borrow<Q>,
    {
        self.entries.get(key).cloned()
    }
    /// port: tsc/internal/collections/multimap.go:MultiMap.RemoveAll
    pub fn remove_all<Q: Eq + Hash + ?Sized>(&mut self, key: &Q)
    where
        K: std::borrow::Borrow<Q>,
    {
        self.entries.remove(key);
    }
}
impl<K: Hash + Eq, V: GoSliceElement + Eq> MultiMap<K, V> {
    /// port: tsc/internal/collections/multimap.go:MultiMap.Add
    pub fn add(&mut self, key: K, value: V) {
        use std::collections::hash_map::Entry;
        match self.entries.entry(key) {
            Entry::Occupied(mut e) => e.get_mut().push(value),
            Entry::Vacant(e) => {
                e.insert(Values::from_vec(vec![value]));
            }
        }
    }
    /// port: tsc/internal/collections/multimap.go:MultiMap.Remove
    pub fn remove<Q: Eq + Hash + ?Sized>(&mut self, key: &Q, value: &V)
    where
        K: std::borrow::Borrow<Q>,
    {
        if let Some(values) = self.entries.get_mut(key) {
            let index = values.read().iter().position(|v| v == value);
            if let Some(index) = index {
                if values.len() == 1 {
                    self.entries.remove(key);
                } else {
                    values.remove_preserving_tail(index);
                }
            }
        }
    }
    /// port: tsc/internal/collections/multimap.go:GroupBy
    pub fn group_by(items: impl IntoIterator<Item = V>, mut key_of: impl FnMut(&V) -> K) -> Self {
        let mut result = Self::default();
        for item in items {
            result.add(key_of(&item), item);
        }
        result
    }
}
