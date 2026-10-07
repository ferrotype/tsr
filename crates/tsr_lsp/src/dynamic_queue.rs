//! Cancellation-aware FIFO. No caller or transport code runs under its lock.
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use tsr_ipc::{AfterFuncStop, Context, ContextError};

struct Inner<T> {
    items: Mutex<VecDeque<T>>,
    changed: Condvar,
    capacity: Option<usize>,
}

// The pin's dynamicQueue state, with Condvar wakeups for cancellation.
pub struct DynamicQueue<T>(Arc<Inner<T>>);
impl<T> Clone for DynamicQueue<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T: Send + 'static> Default for DynamicQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T: Send + 'static> DynamicQueue<T> {
    // port: tsc/internal/lsp/dynamic_queue.go:newDynamicQueue
    pub fn new() -> Self {
        Self::with_capacity(None)
    }
    pub(crate) fn bounded(capacity: usize) -> Self {
        assert!(capacity > 0, "a channel must have room for an event");
        Self::with_capacity(Some(capacity))
    }
    fn with_capacity(capacity: Option<usize>) -> Self {
        Self(Arc::new(Inner {
            items: Mutex::new(VecDeque::new()),
            changed: Condvar::new(),
            capacity,
        }))
    }
    fn wake_on_cancel(&self, context: &Context) -> CancelWake {
        let queue = Arc::downgrade(&self.0);
        CancelWake(context.after_func(move || {
            if let Some(queue) = queue.upgrade() {
                // Pair with wait's mutex: cancellation between its predicate
                // and sleeping cannot lose this notification.
                let _items = queue.items.lock().expect("queue lock");
                queue.changed.notify_all();
            }
        }))
    }
    // port: tsc/internal/lsp/dynamic_queue.go:dynamicQueue.Put
    pub fn put(&self, context: &Context, item: T) -> Result<(), ContextError> {
        if let Some(error) = context.err() {
            return Err(error);
        }
        let _wake = self.wake_on_cancel(context);
        let mut items = self.0.items.lock().expect("queue lock");
        loop {
            if let Some(error) = context.err() {
                return Err(error);
            }
            if self.0.capacity.is_none_or(|cap| items.len() < cap) {
                items.push_back(item);
                self.0.changed.notify_all();
                return Ok(());
            }
            items = self.0.changed.wait(items).expect("queue lock");
        }
    }
    // port: tsc/internal/lsp/dynamic_queue.go:dynamicQueue.Get
    pub fn get(&self, context: &Context) -> Result<T, ContextError> {
        if let Some(error) = context.err() {
            return Err(error);
        }
        let _wake = self.wake_on_cancel(context);
        let mut items = self.0.items.lock().expect("queue lock");
        loop {
            if let Some(error) = context.err() {
                return Err(error);
            }
            if let Some(item) = items.pop_front() {
                if items.is_empty() {
                    *items = VecDeque::new();
                }
                self.0.changed.notify_all();
                return Ok(item);
            }
            items = self.0.changed.wait(items).expect("queue lock");
        }
    }
}
struct CancelWake(AfterFuncStop);
impl Drop for CancelWake {
    fn drop(&mut self) {
        self.0.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn fifo_and_canceled_operations_preserve_remaining_items() {
        let queue = DynamicQueue::new();
        let live = Context::background();
        let canceled = live.with_cancel();
        canceled.cancel();
        assert_eq!(queue.get(&canceled), Err(ContextError::Canceled));
        assert_eq!(queue.put(&canceled, 99), Err(ContextError::Canceled));
        for value in 0..1000 {
            queue.put(&live, value).unwrap();
        }
        for value in 0..1000 {
            assert_eq!(queue.get(&live), Ok(value));
        }
    }

    // Source: TestDynamicQueuePutCancellationWhileStateUnavailable.
    #[test]
    fn canceled_put_does_not_acquire_unavailable_state_or_publish_an_item() {
        let queue = DynamicQueue::new();
        let context = Context::background().with_cancel();
        context.cancel();
        // The native test removes the queue state from its channel. Holding
        // the Rust state mutex represents the same unavailable state.
        let state = queue.0.items.lock().unwrap();
        assert_eq!(queue.put(&context, 1), Err(ContextError::Canceled));
        drop(state);
        queue.put(&Context::background(), 2).unwrap();
        assert_eq!(queue.get(&Context::background()), Ok(2));
    }

    #[test]
    fn cancellation_unblocks_an_empty_reader_and_a_full_writer() {
        for put in [false, true] {
            let queue = DynamicQueue::bounded(1);
            let ctx = Context::background().with_cancel();
            if put {
                queue.put(&ctx, 1).unwrap();
            }
            let worker_queue = queue.clone();
            let worker_ctx = ctx.clone();
            let (started, start) = std::sync::mpsc::channel();
            let (finished, finish) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                started.send(()).unwrap();
                let result = if put {
                    worker_queue.put(&worker_ctx, 2)
                } else {
                    worker_queue.get(&worker_ctx).map(|_| ())
                };
                finished.send(result).unwrap();
            });
            start.recv().unwrap();
            // Cancellation is valid before or after the waiter is asleep.
            ctx.cancel();
            assert_eq!(
                finish.recv_timeout(Duration::from_secs(2)).unwrap(),
                Err(ContextError::Canceled)
            );
            worker.join().unwrap();
            if put {
                assert_eq!(queue.get(&Context::background()), Ok(1));
            }
        }
    }
}
