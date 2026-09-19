//! One checker owner, one operation at a time (plan §4.1; ADR 0007, ADR 0012).
//!
//! `CheckerOwner` holds the exact checker identity and every piece of mutable
//! checker state behind it. An [`Operation`] is the only way to reach that
//! state: it holds the identity's exclusive permit and the state lock together,
//! and drops both together. Inside an operation the checker is a plain
//! single-threaded `&mut` state machine; no lock is taken per node, type,
//! symbol or relation edge.
//!
//! Reentry on the same thread is a bug in the caller, not contention, and is
//! reported before waiting: the thread-local set of active identity leases
//! answers it without touching the lock. Contention from another thread waits
//! normally. A panic inside an operation retires the shared generation before
//! the permit is released (`CheckerLease`'s drop does this); the poisoned state
//! lock is a backstop, never the recovery mechanism.
//!
//! Two lock acquisitions per operation is a known cost of reusing the `ts_arena`
//! permit unchanged. Operations are per query, not per node; P1 measures it and
//! folds the state into the permit if it shows.

use crate::{CheckerOptions, CheckerState, Error};
use std::sync::{Arc, Mutex, MutexGuard};
use ts_arena::{CheckerIdentity, CheckerLease, Counters};

pub struct CheckerOwner {
    identity: Arc<CheckerIdentity>,
    state: Mutex<CheckerState>,
}

impl CheckerOwner {
    /// Adopts the identity's reserved arena number as the checker symbol arena
    /// and creates the types every checker starts with. Each identity can back
    /// one owner; a second attempt fails with `IdentityAdopted` rather than
    /// minting a second, inconsistent identity.
    pub fn new(
        identity: Arc<CheckerIdentity>,
        counters: &Counters,
        options: CheckerOptions,
    ) -> Result<Self, Error> {
        let state = CheckerState::new(&identity, counters, options)?;
        Ok(Self {
            identity,
            state: Mutex::new(state),
        })
    }

    pub fn identity(&self) -> &Arc<CheckerIdentity> {
        &self.identity
    }

    /// Owns the program for every query and retained result. Its source files
    /// are already bound; initialization writes only checker-owned state.
    pub fn for_program(
        identity: Arc<CheckerIdentity>,
        counters: &Counters,
        host: Arc<dyn crate::CheckerHost>,
    ) -> Result<Self, Error> {
        let options = host.options();
        let options = CheckerOptions {
            strict_null_checks: options.strict_option_value(options.strict_null_checks),
            exact_optional_property_types: options.exact_optional_property_types.is_true(),
        };
        let mut state = CheckerState::new(&identity, counters, options)?;
        state.program = Some(crate::program::ProgramContext::new(host));
        state.initialize_program()?;
        Ok(Self {
            identity,
            state: Mutex::new(state),
        })
    }

    /// Begins an exclusive operation: validates the generation, takes the permit,
    /// then the state. Fails with [`Error::Reentry`] if this thread already holds
    /// an operation on this owner, and with `Retired` once the generation is gone.
    ///
    /// The owner is shared through an `Arc` so results can retain it
    /// (`Operation::retain_type` and friends).
    pub fn operation(self: &Arc<Self>) -> Result<Operation<'_>, Error> {
        let lease = self.identity.lease()?;
        let state = self.state.lock().map_err(|_| {
            self.identity.generation().retire();
            Error::Arena(ts_arena::Error::Retired)
        })?;
        Ok(Operation {
            owner: self,
            lease,
            state,
        })
    }
}

/// An exclusive operation scope. Mutable storage never escapes this crate.
///
/// This compile-fail case checks that mutable checker state is crate-private.
///
/// ```compile_fail
/// use ts_checker::Operation;
/// fn swap_checkers(a: &mut Operation<'_>, b: &mut Operation<'_>) {
///     std::mem::swap(a.state_mut(), b.state_mut());
/// }
/// ```
///
/// Direct store lookup is also crate-private. This proves API privacy, not
/// compile-time owner branding: public P1 references use exact-owner checks
/// when an operation imports or resolves them.
///
/// ```compile_fail
/// use ts_checker::{TypeStore, TypeId};
/// fn private_store_lookup(store: &TypeStore, id: TypeId) { store.get(id); }
/// ```
///
/// Field order is the drop order: the lease goes
/// first so a panicking unwind retires the generation before the state unlocks.
pub struct Operation<'a> {
    owner: &'a Arc<CheckerOwner>,
    lease: CheckerLease<'a>,
    state: MutexGuard<'a, CheckerState>,
}

impl Operation<'_> {
    pub fn owner(&self) -> &Arc<CheckerOwner> {
        self.owner
    }
    pub fn lease(&self) -> &CheckerLease<'_> {
        &self.lease
    }
    pub(crate) fn state(&self) -> &CheckerState {
        &self.state
    }
    pub(crate) fn state_mut(&mut self) -> &mut CheckerState {
        &mut self.state
    }
}
