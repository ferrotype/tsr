//! The project timers use the existing IPC deadline service. Tests drive the
//! same callbacks with a manual clock, without sleeps or shortened timeouts.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) struct Timer(Option<Box<dyn FnOnce() + Send>>);
impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(stop) = self.0.take() {
            stop();
        }
    }
}
pub(crate) trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn after(&self, delay: Duration, callback: Box<dyn FnOnce() + Send>) -> Timer;
}
pub(crate) struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn after(&self, delay: Duration, callback: Box<dyn FnOnce() + Send>) -> Timer {
        let context = tsr_ipc::Context::background().with_timeout(delay);
        let stop = context.after_func(callback);
        Timer(Some(Box::new(move || {
            stop.stop();
            context.cancel();
        })))
    }
}
pub(crate) fn system() -> Arc<dyn Clock> {
    Arc::new(SystemClock)
}

#[cfg(test)]
pub(crate) mod manual {
    use super::*;
    use std::{collections::BTreeMap, sync::Mutex};
    type Task = Box<dyn FnOnce() + Send>;
    struct State {
        now: Instant,
        next: u64,
        tasks: BTreeMap<(Instant, u64), Task>,
    }
    pub(crate) struct ManualClock(Arc<Mutex<State>>);
    impl ManualClock {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self(Arc::new(Mutex::new(State {
                now: Instant::now(),
                next: 0,
                tasks: BTreeMap::new(),
            }))))
        }
        pub(crate) fn advance(&self, duration: Duration) {
            self.0.lock().unwrap().now += duration;
            loop {
                let task = {
                    let mut state = self.0.lock().unwrap();
                    if state
                        .tasks
                        .first_key_value()
                        .is_none_or(|((due, _), _)| *due > state.now)
                    {
                        break;
                    }
                    state.tasks.pop_first().unwrap().1
                };
                task();
            }
        }
    }
    impl Clock for ManualClock {
        fn now(&self) -> Instant {
            self.0.lock().unwrap().now
        }
        fn after(&self, delay: Duration, callback: Task) -> Timer {
            let key = {
                let mut state = self.0.lock().unwrap();
                let key = (state.now + delay, state.next);
                state.next += 1;
                state.tasks.insert(key, callback);
                key
            };
            let weak = Arc::downgrade(&self.0);
            Timer(Some(Box::new(move || {
                if let Some(shared) = weak.upgrade() {
                    let task = shared.lock().unwrap().tasks.remove(&key);
                    drop(task);
                }
            })))
        }
    }
}
