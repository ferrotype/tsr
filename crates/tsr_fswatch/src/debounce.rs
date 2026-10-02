#![forbid(unsafe_code)]
use crate::{lock, watcher::DirWatch};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
#[derive(Default)]
struct State {
    signalled: bool,
    generation: u64,
    stop: bool,
    watches: Vec<Weak<DirWatch>>,
}
pub(crate) struct Debounce {
    shared: Arc<(Mutex<State>, Condvar)>,
    thread: Mutex<Option<JoinHandle<()>>>,
}
impl Debounce {
    // port: tsc/internal/fswatch/debounce.go:newDebounce
    pub(crate) fn new() -> Arc<Self> {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let worker = shared.clone();
        let thread = thread::Builder::new()
            .name("fswatch-debounce".into())
            .spawn(move || {
                let (mutex, ready) = &*worker;
                let mut last: Option<Instant> = None;
                loop {
                    let mut state = lock(mutex);
                    while !state.stop && !state.signalled {
                        state = ready
                            .wait(state)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                    if state.stop {
                        break;
                    }
                    if last.is_some_and(|last| last.elapsed() <= Duration::from_millis(500)) {
                        let generation = state.generation;
                        let (next, timeout) = ready
                            .wait_timeout(state, Duration::from_millis(50))
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        state = next;
                        if state.stop {
                            break;
                        }
                        if !timeout.timed_out() || generation != state.generation {
                            continue;
                        }
                    }
                    state.signalled = false;
                    let watches: Vec<_> = state.watches.iter().filter_map(Weak::upgrade).collect();
                    state.watches.retain(|w| w.strong_count() != 0);
                    last = Some(Instant::now());
                    drop(state);
                    for watch in watches {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            watch.trigger_callbacks()
                        }));
                    }
                }
            })
            .expect("create fswatch debounce thread");
        Arc::new(Self {
            shared,
            thread: Mutex::new(Some(thread)),
        })
    }
    // port: tsc/internal/fswatch/debounce.go:debounce.add
    pub(crate) fn add(&self, watch: &Arc<DirWatch>) {
        lock(&self.shared.0).watches.push(Arc::downgrade(watch));
    }
    // port: tsc/internal/fswatch/debounce.go:debounce.trigger
    pub(crate) fn trigger(&self) {
        let mut s = lock(&self.shared.0);
        s.signalled = true;
        s.generation = s.generation.wrapping_add(1);
        self.shared.1.notify_one();
    }
    pub(crate) fn shutdown(&self) {
        {
            let mut s = lock(&self.shared.0);
            s.stop = true;
            self.shared.1.notify_one();
        }
        if let Some(thread) = lock(&self.thread).take() {
            if thread.thread().id() != thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}
impl Drop for Debounce {
    fn drop(&mut self) {
        self.shutdown();
    }
}
