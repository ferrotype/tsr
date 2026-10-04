//! Entries are retained by distinct snapshot owners, not by acquisition count.
use std::{
    collections::{BTreeSet, HashMap},
    hash::Hash,
    sync::{Arc, Condvar, Mutex},
    thread::ThreadId,
};

struct State<V> {
    value: Option<V>,
    owners: BTreeSet<u64>,
    busy: Option<ThreadId>,
    removed: bool,
}
struct Entry<V> {
    state: Mutex<State<V>>,
    ready: Condvar,
}
pub struct OwnerCache<K, V> {
    entries: Mutex<HashMap<K, Arc<Entry<V>>>>,
}
impl<K, V> Default for OwnerCache<K, V> {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}
impl<K: Eq + Hash + Clone, V: Clone> OwnerCache<K, V> {
    // port: tsc/internal/project/ownercache.go:OwnerCache.LoadAndAcquire
    pub fn load_and_acquire<E>(
        &self,
        key: &K,
        owner: u64,
        expired: impl FnOnce(&V) -> Result<bool, E>,
        parse: impl FnOnce() -> Result<V, E>,
    ) -> Result<V, E> {
        let entry = loop {
            let entry = self
                .entries
                .lock()
                .expect("owner cache index")
                .entry(key.clone())
                .or_insert_with(|| {
                    Arc::new(Entry {
                        state: Mutex::new(State {
                            value: None,
                            owners: BTreeSet::new(),
                            busy: None,
                            removed: false,
                        }),
                        ready: Condvar::new(),
                    })
                })
                .clone();
            let mut state = wait(&entry);
            if state.removed {
                continue;
            }
            state.busy = Some(std::thread::current().id());
            drop(state);
            break entry;
        };
        let _finish = Finish {
            cache: self,
            key,
            entry: &entry,
        };
        let previous = entry.state.lock().expect("owner cache entry").value.clone();
        let value = match previous {
            Some(value) if !expired(&value)? => value,
            _ => parse()?,
        };
        let mut state = entry.state.lock().expect("owner cache entry");
        let displaced = state.value.replace(value.clone());
        state.owners.insert(owner);
        drop(state);
        drop(displaced);
        Ok(value)
    }

    // port: tsc/internal/project/ownercache.go:OwnerCache.Acquire
    pub fn acquire(&self, key: &K, owner: u64, value: V) {
        let result: Result<_, std::convert::Infallible> =
            self.load_and_acquire(key, owner, |_| Ok(false), || Ok(value));
        result.unwrap();
    }

    // port: tsc/internal/project/ownercache.go:OwnerCache.AddOwner
    pub fn add_owner(&self, key: &K, owner: u64) {
        assert!(
            self.try_add_owner(key, owner),
            "OwnerCache.AddOwner: entry has no owners"
        );
    }
    pub fn try_add_owner(&self, key: &K, owner: u64) -> bool {
        let entry = self
            .entries
            .lock()
            .expect("owner cache index")
            .get(key)
            .cloned();
        let Some(entry) = entry else {
            return false;
        };
        let mut state = wait(&entry);
        if state.removed || state.owners.is_empty() {
            return false;
        }
        state.owners.insert(owner);
        true
    }

    // port: tsc/internal/project/ownercache.go:OwnerCache.Release
    pub fn release(&self, key: &K, owner: u64) {
        let entry = self
            .entries
            .lock()
            .expect("owner cache index")
            .get(key)
            .cloned();
        let Some(entry) = entry else {
            return;
        };
        let mut state = wait(&entry);
        if state.removed {
            return;
        }
        state.owners.remove(&owner);
        if state.owners.is_empty() {
            self.remove(key, &entry, &mut state);
        }
    }

    fn remove(&self, key: &K, entry: &Arc<Entry<V>>, state: &mut State<V>) {
        state.removed = true;
        let mut entries = self.entries.lock().expect("owner cache index");
        if entries
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            entries.remove(key);
        }
    }

    // port: tsc/internal/project/ownercache.go:OwnerCache.Has
    pub fn has(&self, key: &K) -> bool {
        self.entries
            .lock()
            .expect("owner cache index")
            .contains_key(key)
    }
    pub fn len(&self) -> usize {
        self.entries.lock().expect("owner cache index").len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
fn wait<V>(entry: &Entry<V>) -> std::sync::MutexGuard<'_, State<V>> {
    let mut state = entry.state.lock().expect("owner cache entry");
    while let Some(thread) = state.busy {
        if thread == std::thread::current().id() {
            drop(state);
            panic!("OwnerCache: initializer reentered its own entry");
        }
        state = entry.ready.wait(state).expect("owner cache entry");
    }
    state
}
struct Finish<'a, K: Eq + Hash + Clone, V: Clone> {
    cache: &'a OwnerCache<K, V>,
    key: &'a K,
    entry: &'a Arc<Entry<V>>,
}
impl<K: Eq + Hash + Clone, V: Clone> Drop for Finish<'_, K, V> {
    fn drop(&mut self) {
        let mut state = self.entry.state.lock().expect("owner cache entry");
        state.busy = None;
        if state.owners.is_empty() {
            self.cache.remove(self.key, self.entry, &mut state);
        }
        self.entry.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_acquisitions_and_replacement_keep_snapshot_ownership() {
        let cache = OwnerCache::default();
        let first = Arc::new(10);
        cache.acquire(&1, 1, first.clone());
        cache.acquire(&1, 1, Arc::new(11));
        cache.add_owner(&1, 2);
        let current: Result<_, ()> =
            cache.load_and_acquire(&1, 3, |_| Ok(true), || Ok(Arc::new(12)));
        assert_eq!(*current.unwrap(), 12);
        assert_eq!(*first, 10);
        cache.release(&1, 1);
        cache.release(&1, 2);
        assert!(cache.has(&1));
        cache.release(&1, 3);
        assert!(cache.is_empty());
    }
    #[test]
    fn failure_and_reentry_remove_only_provisional_entries() {
        let cache = OwnerCache::<i32, Arc<i32>>::default();
        let fail: Result<_, ()> = cache.load_and_acquire(&1, 1, |_| Ok(false), || Err(()));
        assert!(fail.is_err());
        assert!(cache.is_empty());
        assert!(std::panic::catch_unwind(|| {
            let _: Result<_, ()> = cache.load_and_acquire(
                &1,
                1,
                |_| Ok(false),
                || {
                    cache.acquire(&1, 1, Arc::new(0));
                    Ok(Arc::new(0))
                },
            );
        })
        .is_err());
        cache.acquire(&1, 1, Arc::new(4));
        let fail: Result<_, ()> = cache.load_and_acquire(&1, 2, |_| Ok(true), || Err(()));
        assert!(fail.is_err());
        cache.release(&1, 1);
        assert!(cache.is_empty());
    }
}
