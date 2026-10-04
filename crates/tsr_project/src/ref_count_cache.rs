//! Per-key initialization and explicit program references. Values publish only
//! after successful construction; failures and unwinds wake waiters to retry.
use std::{
    collections::HashMap,
    hash::Hash,
    sync::{Arc, Condvar, Mutex},
    thread::ThreadId,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct RefCountCacheOptions {
    pub disable_deletion: bool,
}

enum State<V> {
    Initializing(ThreadId),
    Ready { value: V, references: isize },
    Removed,
}
struct Entry<V> {
    state: Mutex<State<V>>,
    ready: Condvar,
}

pub struct RefCountCache<K, V> {
    entries: Mutex<HashMap<K, Arc<Entry<V>>>>,
    options: RefCountCacheOptions,
}
impl<K: Eq + Hash + Clone, V: Clone> RefCountCache<K, V> {
    // port: tsc/internal/project/refcountcache.go:NewRefCountCache
    pub fn new(options: RefCountCacheOptions) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            options,
        }
    }
    // port: tsc/internal/project/refcountcache.go:RefCountCache.Acquire
    pub fn acquire(&self, key: &K, produce: impl FnOnce() -> V) -> V {
        self.acquire_or_error(key, || Ok::<_, std::convert::Infallible>(produce()))
            .unwrap()
    }
    // port: tsc/internal/project/refcountcache.go:RefCountCache.AcquireOrError
    pub fn acquire_or_error<E>(
        &self,
        key: &K,
        produce: impl FnOnce() -> Result<V, E>,
    ) -> Result<V, E> {
        let mut produce = Some(produce);
        loop {
            let (entry, created) = {
                let mut entries = self.entries.lock().expect("cache index poisoned");
                match entries.entry(key.clone()) {
                    std::collections::hash_map::Entry::Occupied(entry) => {
                        (entry.get().clone(), false)
                    }
                    std::collections::hash_map::Entry::Vacant(slot) => (
                        slot.insert(Arc::new(Entry {
                            state: Mutex::new(State::Initializing(std::thread::current().id())),
                            ready: Condvar::new(),
                        }))
                        .clone(),
                        true,
                    ),
                }
            };
            if created {
                // User construction never runs under the index or entry lock.
                // The guard removes a provisional entry on both error and panic.
                let mut guard = Initialization {
                    cache: self,
                    key,
                    entry: &entry,
                    committed: false,
                };
                let value = produce
                    .take()
                    .expect("only a cache winner calls the producer")(
                )?;
                let cached = value.clone();
                *entry.state.lock().expect("cache entry poisoned") = State::Ready {
                    value: cached,
                    references: 1,
                };
                guard.committed = true;
                entry.ready.notify_all();
                return Ok(value);
            }
            let mut state = entry.state.lock().expect("cache entry poisoned");
            loop {
                match &mut *state {
                    State::Initializing(owner) => {
                        if *owner == std::thread::current().id() {
                            // Drop before panicking: waiters may retry after the
                            // outer initialization guard removes this entry.
                            drop(state);
                            panic!("reentrant cache initialization");
                        }
                        state = entry.ready.wait(state).expect("cache entry poisoned");
                    }
                    State::Ready { value, references } => {
                        *references = references
                            .checked_add(1)
                            .expect("cache references exhausted");
                        return Ok(value.clone());
                    }
                    State::Removed => break,
                }
            }
        }
    }
    // port: tsc/internal/project/refcountcache.go:RefCountCache.Has
    pub fn has(&self, key: &K) -> bool {
        self.entries
            .lock()
            .expect("cache index poisoned")
            .contains_key(key)
    }
    pub fn len(&self) -> usize {
        self.entries.lock().expect("cache index poisoned").len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn reference_count(&self, key: &K) -> Option<isize> {
        let entry = self
            .entries
            .lock()
            .expect("cache index poisoned")
            .get(key)
            .cloned()?;
        let result = match &*entry.state.lock().expect("cache entry poisoned") {
            State::Ready { references, .. } => Some(*references),
            State::Initializing(_) => Some(1),
            State::Removed => None,
        };
        result
    }
    // port: tsc/internal/project/refcountcache.go:RefCountCache.Deref
    pub fn release(&self, key: &K) {
        let Some(entry) = self
            .entries
            .lock()
            .expect("cache index poisoned")
            .get(key)
            .cloned()
        else {
            return;
        };
        let removed = {
            let mut state = entry.state.lock().expect("cache entry poisoned");
            let State::Ready { references, .. } = &mut *state else {
                return;
            };
            *references -= 1;
            if *references > 0 || self.options.disable_deletion {
                return;
            }
            let removed = std::mem::replace(&mut *state, State::Removed);
            self.remove_entry(key, &entry);
            removed
        };
        entry.ready.notify_all();
        // V may own caller objects; destructors must run outside our locks.
        drop(removed);
    }
    fn remove_entry(&self, key: &K, entry: &Arc<Entry<V>>) {
        let mut entries = self.entries.lock().expect("cache index poisoned");
        if entries
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            entries.remove(key);
        }
    }
}
struct Initialization<'a, K: Eq + Hash + Clone, V: Clone> {
    cache: &'a RefCountCache<K, V>,
    key: &'a K,
    entry: &'a Arc<Entry<V>>,
    committed: bool,
}
impl<K: Eq + Hash + Clone, V: Clone> Drop for Initialization<'_, K, V> {
    fn drop(&mut self) {
        if !self.committed {
            self.cache.remove_entry(self.key, self.entry);
            *self.entry.state.lock().expect("cache entry poisoned") = State::Removed;
            self.entry.ready.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn references_dispose_only_after_the_last_program() {
        let cache = RefCountCache::new(RefCountCacheOptions::default());
        let a = cache.acquire(&"a", || Arc::new("value"));
        let b = cache.acquire(&"a", || panic!("reuse"));
        assert!(Arc::ptr_eq(&a, &b));
        cache.acquire(&"a", || panic!("existing entry must be reused"));
        assert_eq!(cache.reference_count(&"a"), Some(3));
        cache.release(&"a");
        cache.release(&"a");
        assert!(cache.has(&"a"));
        cache.release(&"a");
        assert!(!cache.has(&"a"));
        assert_eq!(
            *a, "value",
            "escaped storage is independent of cache membership"
        );
    }
    #[test]
    fn failures_panics_and_same_thread_reentry_leave_no_provisional_entry() {
        let cache = RefCountCache::<_, usize>::new(RefCountCacheOptions::default());
        assert_eq!(
            cache.acquire_or_error(&"a", || Err("failed")),
            Err("failed")
        );
        assert!(!cache.has(&"a"));
        let panic = std::panic::catch_unwind(|| cache.acquire(&"a", || cache.acquire(&"a", || 2)));
        assert!(panic.is_err());
        assert!(!cache.has(&"a"));
        assert_eq!(cache.acquire(&"a", || 3), 3);
    }
    #[test]
    fn independent_keys_initialize_concurrently_and_same_key_initializes_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let cache = Arc::new(RefCountCache::new(RefCountCacheOptions::default()));
        let count = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let (cache, count, barrier) = (cache.clone(), count.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    cache.acquire(&1, || {
                        count.fetch_add(1, Ordering::SeqCst);
                        Arc::new(7)
                    })
                })
            })
            .collect();
        let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(results.iter().all(|r| Arc::ptr_eq(r, &results[0])));
        assert_eq!(cache.reference_count(&1), Some(8));
        assert_eq!(
            cache.acquire(&2, || cache.acquire(&3, || Arc::new(9))),
            Arc::new(9)
        );
    }
    #[test]
    fn test_retention_is_an_explicit_policy() {
        let cache = RefCountCache::new(RefCountCacheOptions {
            disable_deletion: true,
        });
        let value = cache.acquire(&0, || Arc::new(1));
        cache.release(&0);
        assert_eq!(cache.reference_count(&0), Some(0));
        assert!(Arc::ptr_eq(
            &value,
            &cache.acquire(&0, || panic!("retain across tests"))
        ));
    }
}
