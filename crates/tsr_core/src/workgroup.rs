//! `core.WorkGroup` and `core.ThrottleGroup` over scoped threads.
//!
//! Ports of `tsc/internal/core/workgroup.go`. The throttle group is witnessed by
//! the `concurrency` group of the Phase 1 operation tables
//! (`docs/PHASE1-mutation-witnesses.md`, section 9); the work group by the C6
//! contracts (`docs/PHASE2-C6-plan.md`).
use crate::semaphore::LimitedSemaphore;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, PoisonError,
    },
    thread::{Scope, ScopedJoinHandle},
};

/// The stack a parallel work group reserves for each of its threads (ADR 0011:
/// parsing, checking and emit run on large reserved stacks, virtual and
/// committed lazily; the deepest recursive paths also grow on demand).
pub const RESERVED_STACK: usize = 256 << 20;

type Task<'env> = Box<dyn FnOnce() + Send + 'env>;

/// Go's `WorkGroup`: queue functions, then run them all and wait. A parallel
/// group runs each function on its own thread with [`RESERVED_STACK`]; a
/// single-threaded group runs them on the caller's thread, last queued first,
/// as the pin's does, which is observable wherever the functions' order is.
///
/// The pin's parallel `Queue` starts its goroutine at once; here the threads
/// start in `run_and_wait`, which the interface allows ("It may be invoked
/// immediately, or deferred until RunAndWait"). Functions borrow from the
/// caller for `'env`, so, unlike the pin's, they cannot queue more work on
/// the group that runs them; no pinned caller does. A panicking function
/// propagates from `run_and_wait` after every other function has finished.
pub struct WorkGroup<'env> {
    single_threaded: bool,
    done: AtomicBool,
    tasks: Mutex<Vec<Task<'env>>>,
}

impl<'env> WorkGroup<'env> {
    /// port: tsc/internal/core/workgroup.go:NewWorkGroup
    pub fn new(single_threaded: bool) -> Self {
        Self {
            single_threaded,
            done: AtomicBool::new(false),
            tasks: Mutex::new(Vec::new()),
        }
    }

    /// port: tsc/internal/core/workgroup.go:parallelWorkGroup.Queue
    /// port: tsc/internal/core/workgroup.go:singleThreadedWorkGroup.Queue
    pub fn queue(&self, task: impl FnOnce() + Send + 'env) {
        assert!(
            !self.done.load(Ordering::Acquire),
            "Queue called after RunAndWait returned"
        );
        self.tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::new(task));
    }

    /// port: tsc/internal/core/workgroup.go:parallelWorkGroup.RunAndWait
    /// port: tsc/internal/core/workgroup.go:singleThreadedWorkGroup.RunAndWait
    pub fn run_and_wait(&self) {
        struct Done<'a>(&'a AtomicBool);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let _done = Done(&self.done);
        if self.single_threaded {
            while let Some(task) = self.pop() {
                task();
            }
            return;
        }
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap_or_else(PoisonError::into_inner));
        std::thread::scope(|scope| {
            let handles: Vec<_> = tasks
                .into_iter()
                .map(|task| {
                    std::thread::Builder::new()
                        .name("tsr-work".into())
                        .stack_size(RESERVED_STACK)
                        .spawn_scoped(scope, task)
                        .expect("could not start a work group thread")
                })
                .collect();
            let mut first_panic = None;
            for handle in handles {
                if let Err(panic) = handle.join() {
                    first_panic.get_or_insert(panic);
                }
            }
            if let Some(panic) = first_panic {
                std::panic::resume_unwind(panic);
            }
        });
    }

    /// port: tsc/internal/core/workgroup.go:singleThreadedWorkGroup.pop
    fn pop(&self) -> Option<Task<'env>> {
        self.tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop()
    }
}

/// Go's `ThrottleGroup`: functions run on their own threads, at most the
/// semaphore's limit at a time, and `Wait` returns the first error. `go` starts
/// each task immediately; the caller's thread scope keeps borrowed inputs alive
/// even if the group is dropped without calling `wait`.
pub struct ThrottleGroup<'scope, 'env, E> {
    scope: &'scope Scope<'scope, 'env>,
    semaphore: Arc<LimitedSemaphore>,
    tasks: Vec<ScopedJoinHandle<'scope, ()>>,
    first_error: Arc<Mutex<Option<E>>>,
}

impl<'scope, 'env, E: Send + 'scope> ThrottleGroup<'scope, 'env, E> {
    /// port: tsc/internal/core/workgroup.go:NewThrottleGroup
    pub fn new(scope: &'scope Scope<'scope, 'env>, semaphore: Arc<LimitedSemaphore>) -> Self {
        Self {
            scope,
            semaphore,
            tasks: Vec::new(),
            first_error: Arc::new(Mutex::new(None)),
        }
    }

    /// port: tsc/internal/core/workgroup.go:ThrottleGroup.Go
    pub fn go(&mut self, task: impl FnOnce() -> Result<(), E> + Send + 'scope) {
        let semaphore = self.semaphore.clone();
        let first_error = self.first_error.clone();
        self.tasks.push(self.scope.spawn(move || {
            let _permit = semaphore.acquire();
            if let Err(error) = task() {
                first_error
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get_or_insert(error);
            }
        }));
    }

    /// port: tsc/internal/core/workgroup.go:ThrottleGroup.Wait
    pub fn wait(self) -> Result<(), E> {
        for task in self.tasks {
            if let Err(panic) = task.join() {
                std::panic::resume_unwind(panic);
            }
        }
        match self
            .first_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod work_group_tests {
    use super::WorkGroup;
    use std::sync::Mutex;

    #[test]
    fn a_single_threaded_group_runs_last_queued_first_on_the_caller() {
        let caller = std::thread::current().id();
        let order = Mutex::new(Vec::new());
        let group = WorkGroup::new(true);
        for value in 0..4 {
            let order = &order;
            group.queue(move || {
                assert_eq!(std::thread::current().id(), caller);
                order.lock().unwrap().push(value);
            });
        }
        group.run_and_wait();
        assert_eq!(*order.lock().unwrap(), [3, 2, 1, 0]);
    }

    #[test]
    fn a_parallel_group_runs_every_task_on_its_own_thread() {
        let caller = std::thread::current().id();
        let seen = Mutex::new(Vec::new());
        let group = WorkGroup::new(false);
        for value in 0..4 {
            let seen = &seen;
            group.queue(move || {
                assert_ne!(std::thread::current().id(), caller);
                seen.lock().unwrap().push(value);
            });
        }
        group.run_and_wait();
        let mut seen = seen.lock().unwrap();
        seen.sort_unstable();
        assert_eq!(*seen, [0, 1, 2, 3]);
    }

    #[test]
    fn a_panicking_task_propagates_after_the_others_finish() {
        let finished = Mutex::new(0);
        let result = std::panic::catch_unwind(|| {
            let group = WorkGroup::new(false);
            group.queue(|| panic!("task failed"));
            for _ in 0..3 {
                group.queue(|| *finished.lock().unwrap() += 1);
            }
            group.run_and_wait();
        });
        assert!(result.is_err());
        assert_eq!(*finished.lock().unwrap(), 3);
    }

    #[test]
    #[should_panic(expected = "Queue called after RunAndWait returned")]
    fn queueing_after_run_and_wait_panics() {
        let group = WorkGroup::new(true);
        group.run_and_wait();
        group.queue(|| {});
    }
}

#[cfg(test)]
mod tests {
    use super::ThrottleGroup;
    use crate::semaphore::LimitedSemaphore;
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc, Mutex,
        },
        thread,
        time::Duration,
    };

    #[test]
    fn go_runs_borrowing_tasks_before_wait() {
        let finished = AtomicBool::new(false);
        let (started, receiver) = mpsc::channel();
        thread::scope(|scope| {
            let mut group = ThrottleGroup::<()>::new(scope, Arc::new(LimitedSemaphore::new(1)));
            group.go(|| {
                finished.store(true, Ordering::SeqCst);
                started.send(()).unwrap();
                Ok(())
            });
            // Pinned ThrottleGroup.Go starts work independently of Wait.
            receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(finished.load(Ordering::SeqCst));
            group.wait().unwrap();
        });
    }

    #[test]
    fn wait_joins_successful_tasks_even_after_an_error() {
        let values = Mutex::new(Vec::new());
        thread::scope(|scope| {
            let mut group = ThrottleGroup::new(scope, Arc::new(LimitedSemaphore::new(2)));
            for value in 0..4 {
                let values = &values;
                group.go(move || {
                    values.lock().unwrap().push(value);
                    if value == 0 {
                        Err("first error")
                    } else {
                        Ok(())
                    }
                });
            }
            assert_eq!(group.wait(), Err("first error"));
            let mut values = values.lock().unwrap();
            values.sort_unstable();
            assert_eq!(*values, [0, 1, 2, 3]);
        });
    }

    #[test]
    fn scope_joins_tasks_if_the_group_is_dropped() {
        let finished = AtomicBool::new(false);
        thread::scope(|scope| {
            let mut group = ThrottleGroup::<()>::new(scope, Arc::new(LimitedSemaphore::new(1)));
            group.go(|| {
                finished.store(true, Ordering::SeqCst);
                Ok(())
            });
        });
        assert!(finished.load(Ordering::SeqCst));
    }

    #[test]
    fn panicking_task_releases_its_permit_and_propagates() {
        let semaphore = Arc::new(LimitedSemaphore::new(1));
        let result = std::panic::catch_unwind(|| {
            thread::scope(|scope| {
                let mut group = ThrottleGroup::<()>::new(scope, semaphore.clone());
                group.go(|| panic!("task failed"));
                group.wait().unwrap();
            });
        });
        assert!(result.is_err());
        let _permit = semaphore.acquire();
    }
}
