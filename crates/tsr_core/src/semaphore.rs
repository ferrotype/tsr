//! A bounded permit whose release follows Rust ownership, including unwinding.
use std::sync::{Condvar, Mutex};
pub struct LimitedSemaphore {
    limit: usize,
    held: Mutex<usize>,
    available: Condvar,
}
pub struct Permit<'a> {
    semaphore: &'a LimitedSemaphore,
}
impl LimitedSemaphore {
    /// port: tsc/internal/core/semaphore.go:NewLimitedSemaphore
    pub const fn new(limit: usize) -> Self {
        assert!(limit > 0, "maxConcurrency must be positive");
        Self {
            limit,
            held: Mutex::new(0),
            available: Condvar::new(),
        }
    }
    /// port: tsc/internal/core/semaphore.go:LimitedSemaphore.Acquire
    pub fn acquire(&self) -> Permit<'_> {
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *held == self.limit {
            held = self
                .available
                .wait(held)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *held += 1;
        Permit { semaphore: self }
    }
}
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *self
            .semaphore
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) -= 1;
        self.semaphore.available.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::LimitedSemaphore;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    };
    #[test]
    fn permits_bound_concurrency_and_unwind_releases_them() {
        let sem = Arc::new(LimitedSemaphore::new(2));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(5));
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let (sem, active, peak, barrier) =
                    (sem.clone(), active.clone(), peak.clone(), barrier.clone());
                scope.spawn(move || {
                    barrier.wait();
                    for _ in 0..200 {
                        let _permit = sem.acquire();
                        let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(n, Ordering::SeqCst);
                        std::thread::yield_now();
                        active.fetch_sub(1, Ordering::SeqCst);
                    }
                });
            }
            barrier.wait();
        });
        assert!((1..=2).contains(&peak.load(Ordering::SeqCst)));
        assert_eq!(active.load(Ordering::SeqCst), 0);
        let result = std::panic::catch_unwind(|| {
            let _permit = sem.acquire();
            panic!("release on unwind");
        });
        assert!(result.is_err());
        let _first = sem.acquire();
        let _second = sem.acquire();
    }
}
