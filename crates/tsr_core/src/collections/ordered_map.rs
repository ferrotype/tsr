//! Insertion order and live iteration from `collections/ordered_map.go`.
//!
//! Keys are stored separately from values, as in Go. A deque lets deletion at
//! either end avoid shifting all remaining keys. Middle deletion is linear.

use std::borrow::Borrow;
use std::collections::{hash_map::Entry, hash_map::RandomState, HashMap, VecDeque};
use std::hash::{BuildHasher, Hash};

/// Replacing a value keeps its position; deleting and reinserting a key moves
/// it to the end. Immutable iterators borrow entries. Use `visit_entries_mut`
/// when the iteration body needs to mutate the same map.
#[derive(Debug)]
pub struct OrderedMap<K, V, S = RandomState> {
    keys: VecDeque<K>,
    values: HashMap<K, V, S>,
}

impl<K: Clone, V: Clone, S: Clone> Clone for OrderedMap<K, V, S> {
    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.Clone
    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.clone
    fn clone(&self) -> Self {
        Self {
            keys: self.keys.clone(),
            values: self.values.clone(),
        }
    }
}

impl<K, V, S: Default> Default for OrderedMap<K, V, S> {
    fn default() -> Self {
        Self {
            keys: VecDeque::new(),
            values: HashMap::default(),
        }
    }
}

// Equality retains the sequence contract of config objects and path mappings:
// maps with the same entries in different orders can resolve patterns differently.
impl<K: Eq + Hash, V: PartialEq, S: BuildHasher> PartialEq for OrderedMap<K, V, S> {
    fn eq(&self, other: &Self) -> bool {
        self.keys == other.keys && self.values == other.values
    }
}
impl<K: Eq + Hash, V: Eq, S: BuildHasher> Eq for OrderedMap<K, V, S> {}

/// Borrowed entries in insertion order. Neither keys nor values are cloned.
pub struct Iter<'a, K, V, S> {
    keys: std::collections::vec_deque::Iter<'a, K>,
    values: &'a HashMap<K, V, S>,
}
impl<'a, K: Eq + Hash, V, S: BuildHasher> Iterator for Iter<'a, K, V, S> {
    type Item = (&'a K, &'a V);
    fn next(&mut self) -> Option<Self::Item> {
        self.keys.next().map(|key| (key, &self.values[key]))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.keys.size_hint()
    }
}
impl<K: Eq + Hash, V, S: BuildHasher> ExactSizeIterator for Iter<'_, K, V, S> {}
impl<'a, K: Eq + Hash, V, S: BuildHasher> IntoIterator for &'a OrderedMap<K, V, S> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V, S>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Owned entries in insertion order. Dropping this iterator drops the unvisited
/// values normally; consumption needs no intermediate vector or value clones.
pub struct IntoIter<K, V, S> {
    keys: std::collections::vec_deque::IntoIter<K>,
    values: HashMap<K, V, S>,
}
impl<K: Eq + Hash, V, S: BuildHasher> Iterator for IntoIter<K, V, S> {
    type Item = (K, V);
    fn next(&mut self) -> Option<Self::Item> {
        let key = self.keys.next()?;
        let value = self
            .values
            .remove(&key)
            .expect("every ordered key has one value");
        Some((key, value))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.keys.size_hint()
    }
}
impl<K: Eq + Hash, V, S: BuildHasher> ExactSizeIterator for IntoIter<K, V, S> {}
impl<K: Eq + Hash, V, S: BuildHasher> IntoIterator for OrderedMap<K, V, S> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V, S>;
    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            keys: self.keys.into_iter(),
            values: self.values,
        }
    }
}

/// One ordered diff notification. Added keys follow the new map's order;
/// modified and removed keys then follow the old map's order.
#[derive(Debug, PartialEq, Eq)]
pub enum MapChange<'a, K, V> {
    Added(&'a K, &'a V),
    Removed(&'a K, &'a V),
    Modified(&'a K, &'a V, &'a V),
}

impl<K, V, S> OrderedMap<K, V, S> {
    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.Size
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.Keys
    pub fn keys(&self) -> impl ExactSizeIterator<Item = &K> {
        self.keys.iter()
    }

    pub(super) fn key_at(&self, index: usize) -> Option<&K> {
        self.keys.get(index)
    }

    /// Clear without releasing capacity.
    ///
    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.Clear
    pub fn clear(&mut self) {
        self.keys.clear();
        self.values.clear();
    }
}

impl<K: Eq + Hash, V, S: BuildHasher> OrderedMap<K, V, S> {
    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.Get
    pub fn get<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.values.get(key)
    }

    /// Mutate a value without changing its key or insertion position.
    pub fn get_mut<Q: Eq + Hash + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        self.values.get_mut(key)
    }

    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.Has
    pub fn contains_key<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.values.contains_key(key)
    }

    /// The signed index retains Go's negative-index miss behavior.
    ///
    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.EntryAt
    pub fn entry_at(&self, index: isize) -> Option<(&K, &V)> {
        let key = self.keys.get(usize::try_from(index).ok()?)?;
        Some((key, &self.values[key]))
    }

    pub fn iter(&self) -> Iter<'_, K, V, S> {
        Iter {
            keys: self.keys.iter(),
            values: &self.values,
        }
    }

    /// Mutate values in insertion order without exposing mutable keys. The
    /// callback cannot structurally change this map during the traversal.
    pub fn for_each_value_mut(&mut self, mut visit: impl FnMut(&K, &mut V)) {
        for key in &self.keys {
            visit(
                key,
                self.values
                    .get_mut(key)
                    .expect("every ordered key has one value"),
            );
        }
    }

    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.Entries
    pub fn entries(&self) -> impl ExactSizeIterator<Item = (&K, &V)> {
        self.keys.iter().map(|key| (key, &self.values[key]))
    }

    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.Values
    pub fn values(&self) -> impl ExactSizeIterator<Item = &V> {
        self.entries().map(|(_, value)| value)
    }

    /// Move values out in insertion order, without cloning their contents.
    pub fn into_values(self) -> impl ExactSizeIterator<Item = V> {
        let mut values = self.values;
        self.keys.into_iter().map(move |key| {
            values
                .remove(&key)
                .expect("every ordered key has one value")
        })
    }

    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.Delete
    pub fn remove<Q: Eq + Hash + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        let value = self.values.remove(key)?;
        let index = self
            .keys
            .iter()
            .position(|item| item.borrow() == key)
            .expect("every ordered-map value has an ordered key");
        self.keys.remove(index);
        Some(value)
    }

    /// Borrowing callbacks cannot mutate either diff operand. This is the
    /// contract used by the pin's watcher/project consumers.
    ///
    /// port: tsc/internal/collections/ordered_map.go:DiffOrderedMapsFunc
    pub fn diff_by<'a>(
        &'a self,
        other: &'a Self,
        mut equal: impl FnMut(&V, &V) -> bool,
        mut emit: impl FnMut(MapChange<'a, K, V>),
    ) {
        for (key, value) in other.entries() {
            if !self.contains_key(key) {
                emit(MapChange::Added(key, value));
            }
        }
        for (key, value) in self.entries() {
            match other.get(key) {
                Some(new) if !equal(value, new) => emit(MapChange::Modified(key, value, new)),
                None => emit(MapChange::Removed(key, value)),
                Some(_) => {}
            }
        }
    }

    /// port: tsc/internal/collections/ordered_map.go:DiffOrderedMaps
    pub fn diff<'a>(&'a self, other: &'a Self, emit: impl FnMut(MapChange<'a, K, V>))
    where
        V: PartialEq,
    {
        self.diff_by(other, PartialEq::eq, emit);
    }
}

impl<K: Eq + Hash + Clone, V, S: BuildHasher> OrderedMap<K, V, S> {
    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.Set
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        match self.values.entry(key) {
            Entry::Occupied(mut entry) => Some(entry.insert(value)),
            Entry::Vacant(entry) => {
                self.keys.push_back(entry.key().clone());
                entry.insert(value);
                None
            }
        }
    }

    /// Re-read the current length and key after every callback, including
    /// entries appended by the callback. Deletion shifts subsequent indices
    /// exactly as in the pinned indexed loop; it may skip the shifted key.
    /// Returning false stops iteration. Only the current key is cloned; the
    /// callback can borrow its value with `get` or mutate it with `get_mut`.
    pub fn visit_keys_mut(&mut self, mut visit: impl FnMut(&mut Self, K) -> bool) {
        let mut index = 0;
        while let Some(key) = self.keys.get(index).cloned() {
            if !visit(self, key) {
                break;
            }
            index += 1;
        }
    }

    /// Live iteration with an owned key/value pair for each callback. This
    /// clones each visited value; use `visit_keys_mut` to avoid that cost or to
    /// visit values that do not implement `Clone`.
    pub fn visit_entries_mut(&mut self, mut visit: impl FnMut(&mut Self, K, V) -> bool)
    where
        V: Clone,
    {
        self.visit_keys_mut(|map, key| {
            let value = map.values[&key].clone();
            visit(map, key, value)
        });
    }
}

impl<K: Eq + Hash + Clone, V: Clone + Default, S: BuildHasher> OrderedMap<K, V, S> {
    /// Go returns a value here; ordinary lookups should use the borrowed `get`.
    ///
    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.GetOrZero
    pub fn get_or_default<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> V
    where
        K: Borrow<Q>,
    {
        self.get(key).cloned().unwrap_or_default()
    }
}

impl<K, V, S: BuildHasher + Default> OrderedMap<K, V, S> {
    /// port: tsc/internal/collections/ordered_map.go:NewOrderedMapWithSizeHint
    /// port: tsc/internal/collections/ordered_map.go:newMapWithSizeHint
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            keys: VecDeque::with_capacity(capacity),
            values: HashMap::with_capacity_and_hasher(capacity, S::default()),
        }
    }
}

impl<K: Eq + Hash + Clone, V, S: BuildHasher + Default> FromIterator<(K, V)>
    for OrderedMap<K, V, S>
{
    /// port: tsc/internal/collections/ordered_map.go:NewOrderedMapFromList
    fn from_iter<T: IntoIterator<Item = (K, V)>>(items: T) -> Self {
        let items = items.into_iter();
        let mut map = Self::with_capacity(items.size_hint().0);
        for (key, value) in items {
            map.insert(key, value);
        }
        map
    }
}
