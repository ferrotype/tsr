//! `core.ThrottleGroup` over scoped threads with a bounded permit count.
//!
//! Ports of `tsc/internal/core/workgroup.go`, witnessed by the `concurrency` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).
use crate::semaphore::LimitedSemaphore;
use std::{
    sync::{Arc, Mutex},
    thread::{Scope, ScopedJoinHandle},
};

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
