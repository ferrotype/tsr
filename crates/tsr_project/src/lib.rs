//! Project ownership, immutable file snapshots and shared parse caches. The
//! checker slots retain ADR 0012 generation and panic-retirement semantics.
#![forbid(unsafe_code)]

pub mod background;
pub mod dirty;
pub mod file_change;
pub mod overlay;
pub mod parse_cache;
pub mod ref_count_cache;
pub mod snapshot_fs;
pub mod source_fs;

use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock};
use tsr_arena::{ArenaId, CheckerIdentity, Counters, Generation};
use tsr_checker::{CheckerHost, CheckerLifetime, CheckerOptions, CheckerOwner, Error, Operation};

/// The API checker is persistent; diagnostics and query slots can be evicted
/// only after their last checkout returns (project/checkerpool.go at the pin).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckerSlot {
    Diagnostics,
    Query(usize),
    Api,
}

type CheckerCell = Arc<OnceLock<Result<Arc<CheckerOwner>, Error>>>;

thread_local! {
    static INITIALIZING: RefCell<Vec<(ArenaId, usize)>> = const { RefCell::new(Vec::new()) };
}

struct Initializing((ArenaId, usize));
impl Drop for Initializing {
    fn drop(&mut self) {
        let _ = INITIALIZING.try_with(|active| {
            let popped = active.borrow_mut().pop();
            debug_assert_eq!(popped, Some(self.0));
        });
    }
}

#[derive(Default)]
struct Slot {
    checker: CheckerCell,
    checkouts: usize,
}

enum Source {
    Program(Arc<dyn CheckerHost>),
    /// Embedders can use the type-system operations without a source program.
    Types(CheckerOptions),
}

pub struct CheckerPool {
    generation: Generation,
    source: Source,
    counters: Counters,
    slots: Mutex<Vec<Slot>>,
    query_slots: usize,
}

impl CheckerPool {
    pub fn for_program(
        host: Arc<dyn CheckerHost>,
        counters: &Counters,
        query_slots: usize,
    ) -> Arc<Self> {
        Self::create(Source::Program(host), counters, query_slots)
    }

    pub fn for_types(
        options: CheckerOptions,
        counters: &Counters,
        query_slots: usize,
    ) -> Arc<Self> {
        Self::create(Source::Types(options), counters, query_slots)
    }

    fn create(source: Source, counters: &Counters, query_slots: usize) -> Arc<Self> {
        assert!(query_slots > 0, "a pool needs at least one query slot");
        let count = query_slots.checked_add(2).expect("checker slot overflow");
        Arc::new(Self {
            generation: Generation::new(counters),
            source,
            counters: counters.clone(),
            slots: Mutex::new((0..count).map(|_| Slot::default()).collect()),
            query_slots,
        })
    }

    pub fn generation(&self) -> &Generation {
        &self.generation
    }

    fn index(&self, slot: CheckerSlot) -> Result<usize, Error> {
        match slot {
            CheckerSlot::Diagnostics => Ok(0),
            CheckerSlot::Query(index) if index < self.query_slots => Ok(index + 1),
            CheckerSlot::Api => Ok(self.query_slots + 1),
            CheckerSlot::Query(_) => Err(tsr_arena::Error::InvalidSlot.into()),
        }
    }

    /// Pins a slot before initializing it. Initialization, which may call a
    /// program host, runs outside all generation/pool locks. A cell publishes
    /// only a complete checker; a failed or panicking generation never issues it.
    pub fn acquire(self: &Arc<Self>, slot: CheckerSlot) -> Result<PooledChecker, Error> {
        let index = self.index(slot)?;
        let cell = {
            let _gate = self.generation.enter()?;
            let mut slots = self.slots.lock().map_err(|_| tsr_arena::Error::Retired)?;
            slots[index].checkouts = slots[index]
                .checkouts
                .checked_add(1)
                .expect("checker checkout overflow");
            slots[index].checker.clone()
        };
        let reservation = Reservation {
            pool: self.clone(),
            index,
        };
        // Also retires if a host panics before CheckerOwner can create a lease.
        let _unwind = RetireOnUnwind(&self.generation);
        let key = (self.generation.id(), index);
        if INITIALIZING.with(|active| active.borrow().contains(&key)) {
            return Err(Error::Reentry);
        }
        let owner = cell
            .get_or_init(|| {
                // Must run before OnceLock releases waiting initializers on
                // panic; the outer guard alone would leave a retry window.
                let _unwind = RetireOnUnwind(&self.generation);
                self.generation.validate()?;
                INITIALIZING.with(|active| active.borrow_mut().push(key));
                let _initializing = Initializing(key);
                #[cfg(test)]
                tests::run_initializer_hook();
                let identity = CheckerIdentity::new(self.generation.clone(), &self.counters);
                let owner = match &self.source {
                    Source::Program(host) => {
                        CheckerOwner::for_program(identity, &self.counters, host.clone())
                    }
                    Source::Types(options) => CheckerOwner::new(identity, &self.counters, *options),
                };
                owner.map(Arc::new)
            })
            .clone()?;
        self.generation
            .enter()?
            .validate_checker(owner.identity())?;
        Ok(PooledChecker { owner, reservation })
    }

    /// Drops only the idle slot's root. Retained results still name and retain
    /// their exact old checker; a later checkout adopts a fresh identity.
    pub fn evict_idle(&self, slot: CheckerSlot) -> Result<bool, Error> {
        let index = self.index(slot)?;
        let displaced = {
            let _gate = self.generation.enter()?;
            let mut slots = self.slots.lock().map_err(|_| tsr_arena::Error::Retired)?;
            if slot == CheckerSlot::Api || slots[index].checkouts != 0 {
                return Ok(false);
            }
            std::mem::take(&mut slots[index].checker)
        };
        // A checker can retain a caller-owned host; do not run its destructor
        // while holding the gate or pool lock.
        drop(displaced);
        Ok(true)
    }
}

impl CheckerPool {
    /// Disposes a canceled checker when its checkout is released, as the pin's
    /// release does (`WasCanceled`): the slot's next acquisition creates a
    /// fresh checker. Retained results keep the canceled owner alive, and its
    /// generation is not retired.
    fn dispose_canceled(&self, index: usize, owner: &Arc<CheckerOwner>) {
        let displaced = {
            let Ok(mut slots) = self.slots.lock() else {
                return;
            };
            let current = slots[index]
                .checker
                .get()
                .and_then(|result| result.as_ref().ok());
            if !current.is_some_and(|current| Arc::ptr_eq(current, owner)) {
                return;
            }
            std::mem::take(&mut slots[index].checker)
        };
        // As in evict_idle, a checker's destructor runs outside the pool lock.
        drop(displaced);
    }
}

/// The pin's `GetChecker` picks the checker by the request's lifetime: the
/// diagnostics checker, the persistent API checker, or a query checker. The
/// pin's query acquisition finds or creates an idle query checker and keeps
/// per-request affinity (`findOrCreateQueryCheckerLocked`), which is the
/// project system's scheduling (Phase 5); this pool serves its first query
/// slot. The file hint is unused, as in the pin. Releasing a canceled checker
/// disposes it ([`PooledChecker`]'s drop). A project's pool serves the
/// programs of its snapshots, so the project implements the interface.
impl tsr_checker::CheckerPool for Project {
    fn with_checker(
        &self,
        lifetime: CheckerLifetime,
        _file: Option<tsr_arena::NodeId>,
        task: &mut dyn FnMut(&mut Operation<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let slot = match lifetime {
            CheckerLifetime::Diagnostics => CheckerSlot::Diagnostics,
            CheckerLifetime::Api => CheckerSlot::Api,
            CheckerLifetime::Temporary => CheckerSlot::Query(0),
        };
        let checkout = self.pool.acquire(slot)?;
        let mut operation = checkout.operation()?;
        task(&mut operation)
    }
}

struct RetireOnUnwind<'a>(&'a Generation);
impl Drop for RetireOnUnwind<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.retire();
        }
    }
}

struct Reservation {
    pool: Arc<CheckerPool>,
    index: usize,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut slots = self
            .pool
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slots[self.index].checkouts -= 1;
    }
}

/// The checkout owns the slot reservation. An operation borrows it, so the slot
/// cannot become idle before the permit is released.
pub struct PooledChecker {
    owner: Arc<CheckerOwner>,
    reservation: Reservation,
}
impl Drop for PooledChecker {
    fn drop(&mut self) {
        if self.owner.was_canceled() {
            self.reservation
                .pool
                .dispose_canceled(self.reservation.index, &self.owner);
        }
    }
}
impl PooledChecker {
    pub fn owner(&self) -> &Arc<CheckerOwner> {
        &self.owner
    }

    pub fn operation(&self) -> Result<Operation<'_>, Error> {
        self.owner.operation()
    }

    /// Call after returning from a callback, before resuming checker work.
    /// Callbacks must run after dropping the previous operation/permit.
    pub fn resume(&self) -> Result<Operation<'_>, Error> {
        self.reservation.pool.generation.validate()?;
        self.operation()
    }
}

/// An immutable project version. Cloning into a later snapshot shares the pool;
/// editing/rebuilding creates a new Project with a separate pool generation.
#[derive(Clone)]
pub struct Project {
    pool: Arc<CheckerPool>,
}
impl Project {
    pub fn new(pool: Arc<CheckerPool>) -> Self {
        Self { pool }
    }
    pub fn pool(&self) -> &Arc<CheckerPool> {
        &self.pool
    }
}

#[derive(Clone)]
pub struct Snapshot {
    project: Project,
}
impl Snapshot {
    pub fn new(project: Project) -> Self {
        Self { project }
    }
    pub fn project(&self) -> &Project {
        &self.project
    }
}

#[cfg(test)]
mod retention;
#[cfg(test)]
mod tests;
