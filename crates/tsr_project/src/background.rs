//! Background work has an admission barrier separate from completion. Closing
//! prevents new work; waiting joins everything admitted, including nested work.
use std::sync::{Arc, Condvar, Mutex};
use tsr_core::CancellationToken;

#[derive(Default)]
struct State {
    closed: bool,
    active: usize,
    panics: Vec<Box<dyn std::any::Any + Send>>,
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    completed: Condvar,
}
#[derive(Clone, Default)]
pub struct Queue {
    shared: Arc<Shared>,
}
impl Queue {
    // port: tsc/internal/project/background/queue.go:NewQueue
    pub fn new() -> Self {
        Self::default()
    }
    // port: tsc/internal/project/background/queue.go:Queue.Enqueue
    pub fn enqueue(
        &self,
        token: CancellationToken,
        task: impl FnOnce(CancellationToken) + Send + 'static,
    ) -> bool {
        {
            let mut state = self.shared.state.lock().expect("background queue poisoned");
            if state.closed || token.is_canceled() {
                return false;
            }
            state.active += 1;
        }
        let shared = self.shared.clone();
        let result = std::thread::Builder::new()
            .name("tsr-project-background".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if !token.is_canceled() {
                        task(token);
                    }
                }));
                let mut state = shared.state.lock().expect("background queue poisoned");
                if let Err(panic) = result {
                    state.panics.push(panic);
                    state.closed = true;
                }
                state.active -= 1;
                shared.completed.notify_all();
            });
        if let Err(error) = result {
            let mut state = self.shared.state.lock().expect("background queue poisoned");
            state.active -= 1;
            self.shared.completed.notify_all();
            drop(state);
            panic!("could not start background worker: {error}");
        }
        true
    }
    // port: tsc/internal/project/background/queue.go:Queue.Wait
    pub fn wait(&self) {
        let mut state = self.shared.state.lock().expect("background queue poisoned");
        while state.active != 0 {
            state = self
                .shared
                .completed
                .wait(state)
                .expect("background queue poisoned");
        }
        if let Some(panic) = state.panics.pop() {
            drop(state);
            std::panic::resume_unwind(panic);
        }
    }
    // port: tsc/internal/project/background/queue.go:Queue.Close
    pub fn close(&self) {
        self.shared
            .state
            .lock()
            .expect("background queue poisoned")
            .closed = true;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    // source: tsc/internal/project/background/queue_test.go:TestQueue
    #[test]
    fn queue_runs_nested_work_waits_and_closes_admission() {
        let queue = Queue::new();
        let count = Arc::new(AtomicUsize::new(0));
        for _ in 0..10 {
            let (q, count) = (queue.clone(), count.clone());
            assert!(queue.enqueue(CancellationToken::new(), move |token| {
                count.fetch_add(1, Ordering::SeqCst);
                q.enqueue(token, move |_| {
                    count.fetch_add(1, Ordering::SeqCst);
                });
            }));
        }
        queue.wait();
        assert_eq!(count.load(Ordering::SeqCst), 20);
        queue.close();
        assert!(!queue.enqueue(CancellationToken::new(), |_| panic!("closed")));
    }
    #[test]
    fn cancellation_and_panic_do_not_strand_the_join() {
        let queue = Queue::new();
        let token = CancellationToken::new();
        token.cancel();
        assert!(!queue.enqueue(token, |_| panic!("canceled")));
        queue.enqueue(CancellationToken::new(), |_| {
            panic!("original worker failure")
        });
        let panic =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| queue.wait())).unwrap_err();
        assert_eq!(
            panic.downcast_ref::<&str>(),
            Some(&"original worker failure")
        );
        assert!(!queue.enqueue(CancellationToken::new(), |_| {}));
    }
}
