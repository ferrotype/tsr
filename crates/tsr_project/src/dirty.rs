//! Snapshot maps retain the base allocation until a change is published. Values
//! are copied only when changed; an unchanged finalization preserves identity.
use std::{collections::BTreeMap, sync::Arc};

pub struct Map<K, V> {
    base: Arc<BTreeMap<K, V>>,
    changes: BTreeMap<K, Option<V>>,
}
impl<K: Ord + Clone, V: Clone> Map<K, V> {
    pub fn new(base: Arc<BTreeMap<K, V>>) -> Self {
        Self {
            base,
            changes: BTreeMap::new(),
        }
    }
    pub fn original(&self, key: &K) -> Option<&V> {
        self.base.get(key)
    }
    pub fn get(&self, key: &K) -> Option<&V> {
        self.changes
            .get(key)
            .map_or_else(|| self.base.get(key), Option::as_ref)
    }
    pub fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }
    pub fn insert(&mut self, key: K, value: V) {
        self.changes.insert(key, Some(value));
    }
    pub fn remove(&mut self, key: &K) {
        if self.base.contains_key(key) || self.changes.contains_key(key) {
            self.changes.insert(key.clone(), None);
        }
    }
    pub fn keys(&self) -> Vec<K> {
        self.base
            .keys()
            .chain(self.changes.keys())
            .filter(|k| self.contains_key(k))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn finalize(self) -> (Arc<BTreeMap<K, V>>, bool) {
        if self.changes.is_empty() {
            return (self.base, false);
        }
        let mut result = self.base;
        let values = Arc::make_mut(&mut result);
        for (key, value) in self.changes {
            if let Some(value) = value {
                values.insert(key, value);
            } else {
                values.remove(&key);
            }
        }
        (result, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_maps_and_values_keep_identity() {
        let value = Arc::new(7);
        let base = Arc::new(BTreeMap::from([(1, value.clone())]));
        let (same, changed) = Map::new(base.clone()).finalize();
        assert!(!changed);
        assert!(Arc::ptr_eq(&same, &base));
        let mut map = Map::new(base.clone());
        map.insert(2, Arc::new(9));
        let (next, changed) = map.finalize();
        assert!(changed);
        assert_eq!(base.len(), 1);
        assert!(Arc::ptr_eq(&next[&1], &value));
        let mut map = Map::new(base.clone());
        map.insert(2, Arc::new(9));
        map.remove(&2);
        let (next, changed) = map.finalize();
        assert!(changed);
        assert!(!Arc::ptr_eq(&next, &base));
        assert_eq!(*next, *base);
    }
}
