//! `core.ThrottleGroup` over scoped threads with a bounded permit count.
//!
//! Ports of `tsc/internal/core/workgroup.go`, witnessed by the `concurrency` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).
use crate::semaphore::LimitedSemaphore;
use std::sync::{Arc, Mutex};

type Task<'a, E> = Box<dyn FnOnce() -> Result<(), E> + Send + 'a>;

/// Go's `ThrottleGroup`: functions run on their own threads, at most the
/// semaphore's limit at a time, and `Wait` returns the first error. Go starts
/// each goroutine in `Go`; here the tasks start in `wait`, inside a thread
/// scope that joins them all, which no caller can tell apart (the group is
/// only observed through `Wait`).
pub struct ThrottleGroup<'a, E> {
    semaphore: Arc<LimitedSemaphore>,
    tasks: Vec<Task<'a, E>>,
}

impl<'a, E: Send + 'a> ThrottleGroup<'a, E> {
    /// port: tsc/internal/core/workgroup.go:NewThrottleGroup
    pub fn new(semaphore: Arc<LimitedSemaphore>) -> Self {
        Self {
            semaphore,
            tasks: Vec::new(),
        }
    }

    /// port: tsc/internal/core/workgroup.go:ThrottleGroup.Go
    pub fn go(&mut self, task: impl FnOnce() -> Result<(), E> + Send + 'a) {
        self.tasks.push(Box::new(task));
    }

    /// port: tsc/internal/core/workgroup.go:ThrottleGroup.Wait
    pub fn wait(self) -> Result<(), E> {
        let first_error: Mutex<Option<E>> = Mutex::new(None);
        let semaphore = &self.semaphore;
        std::thread::scope(|scope| {
            for task in self.tasks {
                let first_error = &first_error;
                scope.spawn(move || {
                    let _permit = semaphore.acquire();
                    if let Err(error) = task() {
                        first_error
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .get_or_insert(error);
                    }
                });
            }
        });
        match first_error
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
