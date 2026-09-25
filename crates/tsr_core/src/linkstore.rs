//! `core.LinkStore` and `core.PagedLinkStore`.
//!
//! Ports of `tsc/internal/core/linkstore.go`, witnessed by the `core` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).
use std::collections::HashMap;
use std::hash::Hash;

/// Go's `LinkStore[K, V]`: a value per key, created on first `Get`.
#[derive(Debug)]
pub struct LinkStore<K, V> {
    entries: HashMap<K, V>,
}

impl<K, V> Default for LinkStore<K, V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash + Clone, V: Default> LinkStore<K, V> {
    /// Go's `Get`. The port marker is on the presence test, a site the
    /// mutation splicer can negate (a reference has no replacement value).
    pub fn get(&mut self, key: &K) -> &mut V {
        let present = self.entries.contains_key(key);
        // port: tsc/internal/core/linkstore.go:LinkStore.Get
        if !present {
            self.entries.insert(key.clone(), V::default());
        }
        self.entries.entry(key.clone()).or_default()
    }

    /// port: tsc/internal/core/linkstore.go:LinkStore.Has
    pub fn has(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// port: tsc/internal/core/linkstore.go:LinkStore.TryGet
    pub fn try_get(&self, key: &K) -> Option<&V> {
        self.entries.get(key)
    }
}

const PAGE_SHIFT: u64 = 8;
const PAGE_SIZE: usize = 1 << PAGE_SHIFT;
const PAGE_MASK: u64 = (1 << PAGE_SHIFT) - 1;
const MAX_PAGE_COUNT: u64 = 65536;

type Page<V> = Box<[V; PAGE_SIZE]>;

/// Go's `PagedLinkStore[V]`: values in pages of 256 keyed by `u64`, a page
/// list below 65536 pages and a page map above.
#[derive(Debug)]
pub struct PagedLinkStore<V> {
    page_map: HashMap<u64, Page<V>>,
    page_list: Vec<Option<Page<V>>>,
}

impl<V> Default for PagedLinkStore<V> {
    fn default() -> Self {
        Self {
            page_map: HashMap::new(),
            page_list: Vec::new(),
        }
    }
}

fn new_page<V: Default>() -> [V; PAGE_SIZE] {
    std::array::from_fn(|_| V::default())
}

impl<V: Default> PagedLinkStore<V> {
    /// Go's `Get`. The port marker is on the missing-page test, a site the
    /// mutation splicer can negate (a reference has no replacement value).
    pub fn get(&mut self, key: u64) -> &mut V {
        let page_index = key >> PAGE_SHIFT;
        let slot = usize::try_from(key & PAGE_MASK).expect("page slot");
        let page = if page_index < MAX_PAGE_COUNT {
            let index = usize::try_from(page_index).expect("page index");
            if index >= self.page_list.len() {
                self.page_list.resize_with(index + 1, || None);
            }
            &mut self.page_list[index]
        } else {
            let entry = self.page_map.entry(page_index);
            return match entry {
                std::collections::hash_map::Entry::Occupied(page) => &mut page.into_mut()[slot],
                std::collections::hash_map::Entry::Vacant(vacant) => {
                    &mut vacant.insert(Box::new(new_page()))[slot]
                }
            };
        };
        let missing = page.is_none();
        // port: tsc/internal/core/linkstore.go:PagedLinkStore.Get
        if missing {
            *page = Some(Box::new(new_page()));
        }
        &mut page.get_or_insert_with(|| Box::new(new_page()))[slot]
    }

    /// port: tsc/internal/core/linkstore.go:PagedLinkStore.Has
    pub fn has(&self, key: u64) -> bool {
        self.try_get(key).is_some()
    }

    /// port: tsc/internal/core/linkstore.go:PagedLinkStore.TryGet
    pub fn try_get(&self, key: u64) -> Option<&V> {
        let page_index = key >> PAGE_SHIFT;
        let page = if page_index < MAX_PAGE_COUNT {
            self.page_list
                .get(usize::try_from(page_index).ok()?)
                .and_then(Option::as_ref)
        } else {
            self.page_map.get(&page_index)
        };
        page.map(|page| &page[usize::try_from(key & PAGE_MASK).expect("page slot")])
    }
}
