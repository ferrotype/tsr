//! Request scheduling for the project checker pool. Slot ownership lives in
//! `CheckerPool`; this layer selects slots and releases them at request barriers.
//! No scheduler lock spans checker construction, operations, or host callbacks.
use crate::{
    clock::{self, Clock, Timer},
    CheckerPool, CheckerSlot, PooledChecker,
};
use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex, Weak},
    thread::ThreadId,
    time::{Duration, Instant},
};
use tsr_arena::NodeId;
use tsr_ast::Diagnostic;
use tsr_checker::{CheckerLifetime, Error, Operation};
use tsr_compiler::Program;
use tsr_ipc::{AfterFuncStop, Context, ContextError};

#[derive(Debug)]
pub enum AcquireError {
    Checker(Error),
    Canceled(ContextError),
}
impl From<Error> for AcquireError {
    fn from(error: Error) -> Self {
        Self::Checker(error)
    }
}
impl std::fmt::Display for AcquireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Checker(e) => e.fmt(f),
            Self::Canceled(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for AcquireError {}

#[derive(Default)]
struct Slot {
    identity: Option<Arc<tsr_arena::CheckerIdentity>>,
    initialized: bool,
    held: bool,
    initializing: Option<ThreadId>,
    request: String,
    lease: Weak<Lease>,
    released: Option<Instant>,
    globals: usize,
}
struct Association {
    index: usize,
    serial: u64,
    cleanup: Option<AfterFuncStop>,
}
impl Drop for Association {
    fn drop(&mut self) {
        if let Some(stop) = &self.cleanup {
            stop.stop();
        }
    }
}
struct State {
    #[cfg(test)]
    waiters: usize,
    slots: Vec<Slot>,
    requests: HashMap<String, Association>,
    files: HashMap<NodeId, usize>,
    next_association: u64,
    discarded: bool,
    timer_epoch: u64,
    timer_deadline: Option<Instant>,
    timer: Option<Timer>,
    globals: Vec<Diagnostic>,
    globals_changed: bool,
    diagnostic_error: Option<Arc<tsr_compiler::Error>>,
}
impl State {
    fn clear_associations(&mut self, index: usize) {
        self.files.retain(|_, slot| *slot != index);
        self.requests.retain(|_, a| a.index != index);
    }
}

pub struct CheckerScheduler {
    pool: Arc<CheckerPool>,
    program: Arc<Program>,
    clock: Arc<dyn Clock>,
    idle_timeout: Duration,
    state: Mutex<State>,
    available: Condvar,
}
impl CheckerScheduler {
    pub fn new(pool: Arc<CheckerPool>, program: Arc<Program>, idle_timeout: Duration) -> Arc<Self> {
        Self::with_clock(pool, program, idle_timeout, clock::system())
    }
    fn with_clock(
        pool: Arc<CheckerPool>,
        program: Arc<Program>,
        idle_timeout: Duration,
        clock: Arc<dyn Clock>,
    ) -> Arc<Self> {
        let count = pool.query_slots + 2;
        Arc::new(Self {
            pool,
            program,
            clock,
            idle_timeout: if idle_timeout.is_zero() {
                Duration::from_secs(30)
            } else {
                idle_timeout
            },
            state: Mutex::new(State {
                #[cfg(test)]
                waiters: 0,
                slots: (0..count).map(|_| Slot::default()).collect(),
                requests: HashMap::new(),
                files: HashMap::new(),
                next_association: 0,
                discarded: false,
                timer_epoch: 0,
                timer_deadline: None,
                timer: None,
                globals: Vec::new(),
                globals_changed: false,
                diagnostic_error: None,
            }),
            available: Condvar::new(),
        })
    }
    fn checker_slot(&self, index: usize) -> CheckerSlot {
        if index == 0 {
            CheckerSlot::Diagnostics
        } else if index == self.pool.query_slots + 1 {
            CheckerSlot::Api
        } else {
            CheckerSlot::Query(index - 1)
        }
    }

    // port: tsc/internal/project/checkerpool.go:checkerPool.GetChecker
    pub fn acquire(
        self: &Arc<Self>,
        lifetime: CheckerLifetime,
        file: Option<NodeId>,
        context: &Context,
        request: &str,
    ) -> Result<ScheduledChecker, AcquireError> {
        let request = if context.can_cancel() && lifetime != CheckerLifetime::Api {
            request
        } else {
            ""
        };
        // Deliberate difference from checkerpool.go: Go's semaphore wait ignores
        // cancellation. Here a canceled waiter returns without claiming or
        // initializing a slot, so abandoned requests leave the wait promptly.
        // Later slot/type allocation order may therefore differ from Go.
        // Registration is outside the state lock: an already-done context calls
        // its after-function immediately.
        let weak = Arc::downgrade(self);
        let wake = context.can_cancel().then(|| {
            context.after_func(move || {
                if let Some(scheduler) = weak.upgrade() {
                    let _state = scheduler.state.lock().unwrap();
                    scheduler.available.notify_all();
                }
            })
        });
        struct Stop(Option<AfterFuncStop>);
        impl Drop for Stop {
            fn drop(&mut self) {
                if let Some(stop) = &self.0 {
                    stop.stop();
                }
            }
        }
        let _wake = Stop(wake);
        let mut state = self.state.lock().unwrap();
        let (index, associate_file) = loop {
            self.pool.generation().validate().map_err(Error::from)?;
            if let Some(error) = context.err() {
                return Err(AcquireError::Canceled(error));
            }
            let mut finishing_same_request = false;
            if let Some(association) = state.requests.get(request) {
                let index = association.index;
                if (lifetime == CheckerLifetime::Diagnostics) == (index == 0) {
                    let slot = &state.slots[index];
                    if slot.initialized && slot.request == request && slot.held {
                        if let Some(lease) = slot.lease.upgrade() {
                            return Ok(ScheduledChecker(lease));
                        }
                        if slot.initializing == Some(std::thread::current().id()) {
                            return Err(Error::Reentry.into());
                        }
                        finishing_same_request = true;
                    }
                    if slot.initialized && !slot.held {
                        break (index, false);
                    }
                } else {
                    state.requests.remove(request);
                }
            }
            let candidate = match lifetime {
                CheckerLifetime::Diagnostics => (!state.slots[0].held).then_some((0, false)),
                CheckerLifetime::Api => {
                    let i = self.pool.query_slots + 1;
                    (!state.slots[i].held).then_some((i, false))
                }
                CheckerLifetime::Temporary => file
                    .and_then(|file| state.files.get(&file).copied())
                    .filter(|&i| state.slots[i].initialized && !state.slots[i].held)
                    .map(|i| (i, false))
                    .or_else(|| {
                        (1..=self.pool.query_slots)
                            .find(|&i| state.slots[i].initialized && !state.slots[i].held)
                            .map(|i| (i, true))
                    })
                    .or_else(|| {
                        (1..=self.pool.query_slots)
                            .find(|&i| !state.slots[i].held)
                            .map(|i| (i, true))
                    }),
            };
            if let Some(selection) = candidate.filter(|_| !finishing_same_request) {
                break selection;
            }
            let relevant = match lifetime {
                CheckerLifetime::Diagnostics => 0..1,
                CheckerLifetime::Temporary => 1..self.pool.query_slots + 1,
                CheckerLifetime::Api => self.pool.query_slots + 1..self.pool.query_slots + 2,
            };
            if state.slots[relevant].iter().any(|slot| {
                slot.initializing == Some(std::thread::current().id())
                    || slot
                        .identity
                        .as_ref()
                        .is_some_and(|id| id.is_leased_on_current_thread())
            }) {
                return Err(Error::Reentry.into());
            }
            #[cfg(test)]
            {
                state.waiters += 1;
                self.available.notify_all();
            }
            state = self.available.wait(state).unwrap();
            #[cfg(test)]
            {
                state.waiters -= 1;
            }
        };
        let slot = &mut state.slots[index];
        slot.held = true;
        slot.initializing = Some(std::thread::current().id());
        slot.request = request.into();
        drop(state);
        let mut reservation = Reservation {
            scheduler: self.clone(),
            index,
            armed: true,
        };
        let checker = self.pool.acquire(self.checker_slot(index))?;
        let lease = Arc::new(Lease {
            scheduler: self.clone(),
            index,
            checker: Some(checker),
        });
        let registration = {
            let mut state = self.state.lock().unwrap();
            let slot = &mut state.slots[index];
            slot.initialized = true;
            slot.identity = Some(lease.checker.as_ref().unwrap().owner().identity().clone());
            slot.initializing = None;
            slot.lease = Arc::downgrade(&lease);
            // getQueryChecker only records a file on the find-or-create path.
            // Reacquiring by request must leave another file's affinity intact.
            if let Some(file) = file.filter(|_| associate_file) {
                state.files.insert(file, index);
            }
            let registration = if !request.is_empty() && !state.requests.contains_key(request) {
                let serial = state.next_association;
                state.next_association = serial
                    .checked_add(1)
                    .expect("request association exhausted");
                state.requests.insert(
                    request.into(),
                    Association {
                        index,
                        serial,
                        cleanup: None,
                    },
                );
                Some(serial)
            } else {
                None
            };
            reservation.armed = false;
            self.available.notify_all();
            registration
        };
        if let Some(serial) = registration {
            self.register_cleanup(context, request, serial);
        }
        Ok(ScheduledChecker(lease))
    }

    // port: tsc/internal/project/checkerpool.go:checkerPool.registerRequestCleanup
    fn register_cleanup(self: &Arc<Self>, context: &Context, request: &str, serial: u64) {
        let weak = Arc::downgrade(self);
        let name = request.to_owned();
        let stop = context.after_func(move || {
            if let Some(scheduler) = weak.upgrade() {
                let mut state = scheduler.state.lock().unwrap();
                if state
                    .requests
                    .get(&name)
                    .is_some_and(|a| a.serial == serial)
                {
                    state.requests.remove(&name);
                }
            }
        });
        let mut state = self.state.lock().unwrap();
        if let Some(association) = state
            .requests
            .get_mut(request)
            .filter(|a| a.serial == serial)
        {
            association.cleanup = Some(stop);
        } else {
            stop.stop();
        }
    }

    // port: tsc/internal/project/checkerpool.go:checkerPool.createRelease
    fn release(self: &Arc<Self>, index: usize, checker: PooledChecker) {
        let canceled = checker.owner().was_canceled();
        let globals = if !canceled && !std::thread::panicking() && index <= self.pool.query_slots {
            Some(
                checker
                    .operation()
                    .and_then(|mut operation| operation.global_diagnostics()),
            )
        } else {
            None
        };
        // The slot cannot be claimed until its owner reservation has returned.
        // Drop also removes a canceled checker from the underlying pool.
        drop(checker);
        {
            let mut state = self.state.lock().unwrap();
            if canceled {
                state.clear_associations(index);
                state.slots[index] = Slot::default();
            } else {
                if let Some(globals) = globals {
                    match globals {
                        Ok(globals) if globals.len() != state.slots[index].globals => {
                            state.slots[index].globals = globals.len();
                            let mut combined = state.globals.clone();
                            combined.extend(globals);
                            match self.program.sort_and_deduplicate_diagnostics(&combined) {
                                Ok(combined) => {
                                    state.globals_changed |= combined.len() != state.globals.len();
                                    state.globals = combined;
                                }
                                Err(error) => state.diagnostic_error = Some(Arc::new(error)),
                            }
                        }
                        Err(error) => state.diagnostic_error = Some(Arc::new(error.into())),
                        _ => {}
                    }
                }
                let slot = &mut state.slots[index];
                slot.held = false;
                slot.request.clear();
                slot.released = Some(self.clock.now());
            }
            self.available.notify_all();
        }
        self.schedule_cleanup();
    }

    // port: tsc/internal/project/checkerpool.go:checkerPool.scheduleCleanupLocked
    fn schedule_cleanup(self: &Arc<Self>) {
        let (deadline, epoch, previous) = {
            let mut state = self.state.lock().unwrap();
            let deadline = if state.discarded || self.pool.generation().validate().is_err() {
                None
            } else {
                state.slots[..=self.pool.query_slots]
                    .iter()
                    .filter(|slot| slot.initialized && !slot.held)
                    .filter_map(|slot| slot.released.map(|time| time + self.idle_timeout))
                    .min()
            };
            if state.timer_deadline == deadline {
                return;
            }
            state.timer_deadline = deadline;
            state.timer_epoch = state
                .timer_epoch
                .checked_add(1)
                .expect("timer epoch exhausted");
            (deadline, state.timer_epoch, state.timer.take())
        };
        drop(previous);
        let Some(deadline) = deadline else {
            return;
        };
        let weak = Arc::downgrade(self);
        let delay = deadline
            .saturating_duration_since(self.clock.now())
            .max(Duration::from_millis(1));
        let timer = self.clock.after(
            delay,
            Box::new(move || {
                if let Some(scheduler) = weak.upgrade() {
                    scheduler.cleanup(epoch);
                }
            }),
        );
        let displaced = {
            let mut state = self.state.lock().unwrap();
            if state.timer_epoch == epoch && !state.discarded {
                state.timer.replace(timer)
            } else {
                Some(timer)
            }
        };
        drop(displaced);
    }

    // port: tsc/internal/project/checkerpool.go:checkerPool.cleanupIdleCheckers
    fn cleanup(self: &Arc<Self>, epoch: u64) {
        let now = self.clock.now();
        let (displaced, timer) = {
            let mut state = self.state.lock().unwrap();
            if state.discarded || state.timer_epoch != epoch {
                return;
            }
            state.timer_deadline = None;
            let timer = state.timer.take();
            let idle: Vec<_> = (0..=self.pool.query_slots)
                .filter(|&i| {
                    let s = &state.slots[i];
                    s.initialized
                        && !s.held
                        && s.released.is_some_and(|time| {
                            now.saturating_duration_since(time) >= self.idle_timeout
                        })
                })
                .collect();
            let mut displaced = Vec::new();
            for index in idle {
                // Removal and Discard are serialized. Actual destruction is
                // deferred until the scheduler and owner locks are released.
                if let Ok(Some(cell)) = self.pool.take_idle(self.checker_slot(index)) {
                    displaced.push(cell);
                    state.clear_associations(index);
                    state.slots[index] = Slot::default();
                }
            }
            (displaced, timer)
        };
        drop(timer);
        drop(displaced);
        self.schedule_cleanup();
    }

    // port: tsc/internal/project/checkerpool.go:checkerPool.Discard
    pub fn discard(&self) {
        let timer = {
            let mut state = self.state.lock().unwrap();
            state.discarded = true;
            state.timer_epoch = state
                .timer_epoch
                .checked_add(1)
                .expect("timer epoch exhausted");
            state.timer_deadline = None;
            state.timer.take()
        };
        drop(timer);
    }
    // port: tsc/internal/project/checkerpool.go:checkerPool.GetGlobalDiagnostics
    pub fn global_diagnostics(&self) -> Result<Vec<Diagnostic>, Arc<tsr_compiler::Error>> {
        let state = self.state.lock().unwrap();
        if let Some(error) = &state.diagnostic_error {
            return Err(error.clone());
        }
        Ok(state.globals.clone())
    }
    // port: tsc/internal/project/checkerpool.go:checkerPool.TakeNewGlobalDiagnostics
    pub fn take_new_global_diagnostics(&self) -> bool {
        std::mem::take(&mut self.state.lock().unwrap().globals_changed)
    }
}
struct Reservation {
    scheduler: Arc<CheckerScheduler>,
    index: usize,
    armed: bool,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if self.armed {
            let mut state = self.scheduler.state.lock().unwrap();
            state.slots[self.index].held = false;
            state.slots[self.index].initializing = None;
            self.scheduler.available.notify_all();
        }
    }
}
struct Lease {
    scheduler: Arc<CheckerScheduler>,
    index: usize,
    checker: Option<PooledChecker>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(checker) = self.checker.take() {
            self.scheduler.release(self.index, checker);
        }
    }
}
/// Nested acquisitions by the same request share the reservation. Rust retains
/// it until the last nested handle is dropped, so a retained nested checkout
/// cannot be used concurrently with a new request after its outer one drops.
#[derive(Clone)]
pub struct ScheduledChecker(Arc<Lease>);
impl ScheduledChecker {
    pub fn owner(&self) -> &Arc<tsr_checker::CheckerOwner> {
        self.0.checker.as_ref().unwrap().owner()
    }
    pub fn operation(&self) -> Result<Operation<'_>, Error> {
        self.0.checker.as_ref().unwrap().operation()
    }
}

#[cfg(test)]
mod tests;
