use super::SyncMap;
use std::hash::Hash;

/// Source type: tsc/internal/collections/syncset.go:SyncSet
#[derive(Debug)]
pub struct SyncSet<T> {
    map: SyncMap<T, ()>,
}
impl<T> Default for SyncSet<T> {
    fn default() -> Self {
        Self {
            map: SyncMap::default(),
        }
    }
}
impl<T: Hash + Eq + Clone> SyncSet<T> {
    /// port: tsc/internal/collections/syncset.go:SyncSet.Has
    pub fn contains(&self, key: &T) -> bool {
        self.map.load(key).is_some()
    }
    /// port: tsc/internal/collections/syncset.go:SyncSet.Add
    pub fn insert(&self, key: T) {
        self.insert_if_absent(key);
    }
    /// port: tsc/internal/collections/syncset.go:SyncSet.AddIfAbsent
    pub fn insert_if_absent(&self, key: T) -> bool {
        !self.map.load_or_store(key, ()).1
    }
    /// port: tsc/internal/collections/syncset.go:SyncSet.Delete
    pub fn remove(&self, key: &T) {
        self.map.delete(key);
    }
    /// port: tsc/internal/collections/syncset.go:SyncSet.Range
    pub fn range(&self, mut visit: impl FnMut(T) -> bool) {
        self.map.range(|key, ()| visit(key));
    }
    /// port: tsc/internal/collections/syncset.go:SyncSet.Size
    pub fn len(&self) -> usize {
        self.map.len()
    }
    /// port: tsc/internal/collections/syncset.go:SyncSet.IsEmpty
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    /// port: tsc/internal/collections/syncset.go:SyncSet.ToSlice
    pub fn to_vec(&self) -> Vec<T> {
        self.map.keys()
    }
    /// port: tsc/internal/collections/syncset.go:SyncSet.Keys
    pub fn keys(&self) -> Vec<T> {
        self.map.keys()
    }
}
