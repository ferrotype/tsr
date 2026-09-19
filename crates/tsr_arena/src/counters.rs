use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

/// Live tracked owners and storage allocations in one observation domain.
/// Allocation counts include owner objects, core slabs and lazy pages; they are
/// not malloc-call counts or RSS (directory/cache bookkeeping is not included).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub owners: usize,
    pub allocations: usize,
}

#[derive(Default)]
struct State {
    owners: AtomicUsize,
    allocations: AtomicUsize,
}

/// Share this observation domain among related owners. Independent domains make
/// concurrent programs/tests measurable without resetting global counters.
#[derive(Clone, Default)]
pub struct Counters(Arc<State>);
impl Counters {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn snapshot(&self) -> Counts {
        Counts {
            owners: self.0.owners.load(Ordering::SeqCst),
            allocations: self.0.allocations.load(Ordering::SeqCst),
        }
    }
    pub(crate) fn owner(&self) -> Track {
        self.track(true)
    }
    pub(crate) fn allocation(&self) -> Track {
        self.track(false)
    }
    fn track(&self, owner: bool) -> Track {
        if owner {
            self.0.owners.fetch_add(1, Ordering::SeqCst);
        }
        self.0.allocations.fetch_add(1, Ordering::SeqCst);
        Track {
            counters: self.clone(),
            owner,
        }
    }
}
pub(crate) struct Track {
    counters: Counters,
    owner: bool,
}
impl Drop for Track {
    fn drop(&mut self) {
        if self.owner {
            self.counters.0.owners.fetch_sub(1, Ordering::SeqCst);
        }
        self.counters.0.allocations.fetch_sub(1, Ordering::SeqCst);
    }
}
