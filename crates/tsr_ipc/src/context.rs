//! The part of Go's `context.Context` the connections and the content-mapper
//! host use: cancellation that flows from parents to children, deadlines, and
//! functions to run when a context is canceled (`context.AfterFunc`).
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

/// Why a context is done, rendered as the pinned errors render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextError {
    Canceled,
    DeadlineExceeded,
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Canceled => "context canceled",
            Self::DeadlineExceeded => "context deadline exceeded",
        })
    }
}

impl std::error::Error for ContextError {}

type AfterFunc = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct State {
    canceled: Option<ContextError>,
    children: Vec<Weak<Inner>>,
    after: Vec<(u64, AfterFunc)>,
    next_after: u64,
}

struct Inner {
    parent: Option<Context>,
    deadline: Option<Instant>,
    state: Mutex<State>,
}

/// A cancellation scope. Cloning shares the scope.
#[derive(Clone)]
pub struct Context(Arc<Inner>);

impl Default for Context {
    fn default() -> Self {
        Self::background()
    }
}

impl Context {
    /// A context that is never done (`context.Background`).
    pub fn background() -> Self {
        Self(Arc::new(Inner {
            parent: None,
            deadline: None,
            state: Mutex::new(State::default()),
        }))
    }

    fn child(&self, deadline: Option<Instant>) -> Self {
        let inherited = self.deadline();
        // Only a deadline earlier than every ancestor's needs a timer of its
        // own; a later one ends the child through the ancestor's expiry
        // (`context.WithDeadline`).
        let timer = deadline.filter(|own| inherited.is_none_or(|inherited| *own < inherited));
        let deadline = match (deadline, inherited) {
            (Some(own), Some(parent)) => Some(own.min(parent)),
            (own, parent) => own.or(parent),
        };
        let child = Self(Arc::new(Inner {
            parent: Some(self.clone()),
            deadline,
            state: Mutex::new(State::default()),
        }));
        let parent_error = {
            let mut state = self.0.state.lock().expect("context lock");
            state.children.retain(|child| child.strong_count() > 0);
            state.children.push(Arc::downgrade(&child.0));
            state.canceled
        };
        if let Some(error) = parent_error.or_else(|| self.err()) {
            child.cancel_with(error);
        } else if let Some(deadline) = timer {
            if deadline <= Instant::now() {
                child.cancel_with(ContextError::DeadlineExceeded);
            } else {
                DEADLINES.schedule(deadline, &child.0);
            }
        }
        child
    }

    /// A child canceled by its cancel function or with its parent
    /// (`context.WithCancel`).
    #[must_use]
    pub fn with_cancel(&self) -> Self {
        self.child(None)
    }

    /// A child that is also done after `timeout` (`context.WithTimeout`): at
    /// the deadline it is canceled with `DeadlineExceeded`, running its
    /// after-functions and its descendants'.
    #[must_use]
    pub fn with_timeout(&self, timeout: Duration) -> Self {
        self.child(Some(Instant::now() + timeout))
    }

    /// The earliest deadline of this context and its ancestors.
    pub fn deadline(&self) -> Option<Instant> {
        self.0.deadline
    }

    /// Whether this scope has a cancellation lifetime (`ctx.Done() != nil`).
    /// Background contexts do not install per-request cleanup callbacks.
    pub fn can_cancel(&self) -> bool {
        self.0.parent.is_some()
    }

    /// Cancels this context and its descendants, running their after-functions.
    pub fn cancel(&self) {
        self.cancel_with(ContextError::Canceled);
    }

    /// Ends a scope whose caller-owned timer expired. Project timers can use
    /// a controllable clock while preserving the native deadline error.
    pub fn expire(&self) {
        self.cancel_with(ContextError::DeadlineExceeded);
    }

    /// Ends the context once, with the first reason it was done for: a
    /// deadline that has passed comes before a later cancellation even when
    /// its timer has not fired yet. The descendants end first, so a call
    /// waiting on one of them returns before an after-function that waits for
    /// that call runs; the pin runs each after-function on a goroutine of its
    /// own once the context is done.
    fn cancel_with(&self, error: ContextError) {
        let (children, after, error) = {
            let mut state = self.0.state.lock().expect("context lock");
            if state.canceled.is_some() {
                return;
            }
            let error = if self.expired() {
                ContextError::DeadlineExceeded
            } else {
                error
            };
            state.canceled = Some(error);
            (
                std::mem::take(&mut state.children),
                std::mem::take(&mut state.after),
                error,
            )
        };
        for child in children.iter().filter_map(Weak::upgrade) {
            Self(child).cancel_with(error);
        }
        for (_, function) in after {
            function();
        }
    }

    /// The context's own or inherited deadline has passed.
    fn expired(&self) -> bool {
        self.0
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    /// Why the context is done, or `None` while it is live (`ctx.Err()`).
    pub fn err(&self) -> Option<ContextError> {
        if let Some(error) = self.0.state.lock().expect("context lock").canceled {
            return Some(error);
        }
        if self.expired() {
            return Some(ContextError::DeadlineExceeded);
        }
        self.0.parent.as_ref().and_then(Self::err)
    }

    /// Runs `function` once the context is done, on the thread that ends it
    /// (a thread of the deadline timer's for an expiry), or at once when it
    /// already is (`context.AfterFunc`). The returned stop function
    /// unregisters it and reports whether it did so before it ran.
    pub fn after_func(&self, function: impl FnOnce() + Send + 'static) -> AfterFuncStop {
        let mut state = self.0.state.lock().expect("context lock");
        if state.canceled.is_some() {
            drop(state);
            function();
            return AfterFuncStop {
                context: self.clone(),
                id: None,
            };
        }
        let id = state.next_after;
        state.next_after += 1;
        state.after.push((id, Box::new(function)));
        AfterFuncStop {
            context: self.clone(),
            id: Some(id),
        }
    }
}

/// The pending deadlines of timed contexts, kept by one thread for the
/// process as the pin's runtime timers serve `context.WithDeadline`. A
/// context that is dropped before its deadline leaves only a dead entry.
struct Deadlines {
    pending: Mutex<Pending>,
    wake: Condvar,
}

struct Pending {
    /// By deadline, then by scheduling order.
    timers: BTreeMap<(Instant, u64), Weak<Inner>>,
    next: u64,
    running: bool,
}

static DEADLINES: Deadlines = Deadlines {
    pending: Mutex::new(Pending {
        timers: BTreeMap::new(),
        next: 0,
        running: false,
    }),
    wake: Condvar::new(),
};

impl Deadlines {
    fn schedule(&'static self, deadline: Instant, context: &Arc<Inner>) {
        let mut pending = self.pending.lock().expect("deadline lock");
        let sequence = pending.next;
        pending.next += 1;
        pending
            .timers
            .insert((deadline, sequence), Arc::downgrade(context));
        if !pending.running {
            pending.running = true;
            std::thread::Builder::new()
                .name("context deadlines".into())
                .spawn(|| DEADLINES.expire())
                .expect("the context deadline thread starts");
        }
        drop(pending);
        self.wake.notify_one();
    }

    /// Ends each timed context at its deadline, each on a thread of its own:
    /// its after-functions may block, and the next deadline must not wait.
    fn expire(&self) {
        let mut pending = self.pending.lock().expect("deadline lock");
        loop {
            let now = Instant::now();
            match pending.timers.first_key_value().map(|(&(due, _), _)| due) {
                None => pending = self.wake.wait(pending).expect("deadline lock"),
                Some(due) if due > now => {
                    pending = self
                        .wake
                        .wait_timeout(pending, due - now)
                        .expect("deadline lock")
                        .0;
                }
                Some(_) => {
                    let (_, context) = pending.timers.pop_first().expect("a due deadline");
                    drop(pending);
                    if let Some(context) = context.upgrade().map(Context) {
                        let ending = context.clone();
                        let started = std::thread::Builder::new()
                            .spawn(move || ending.cancel_with(ContextError::DeadlineExceeded));
                        if started.is_err() {
                            context.cancel_with(ContextError::DeadlineExceeded);
                        }
                    }
                    pending = self.pending.lock().expect("deadline lock");
                }
            }
        }
    }
}

/// Unregisters an after-function (the `stop` of `context.AfterFunc`).
pub struct AfterFuncStop {
    context: Context,
    id: Option<u64>,
}

impl AfterFuncStop {
    /// True when the function was unregistered before it ran.
    pub fn stop(&self) -> bool {
        let Some(id) = self.id else {
            return false;
        };
        let mut state = self.context.0.state.lock().expect("context lock");
        let before = state.after.len();
        state.after.retain(|(entry, _)| *entry != id);
        state.after.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn cancellation_flows_to_children_and_runs_after_functions_once() {
        let root = Context::background();
        let parent = root.with_cancel();
        let child = parent.with_cancel();
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = ran.clone();
        let stop = child.after_func(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(child.err(), None);
        parent.cancel();
        assert_eq!(child.err(), Some(ContextError::Canceled));
        assert_eq!(root.err(), None);
        parent.cancel();
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        assert!(!stop.stop());
        let late = parent.with_cancel();
        assert_eq!(late.err(), Some(ContextError::Canceled));
    }

    #[test]
    fn deadlines_expire_and_stopped_functions_never_run() {
        let root = Context::background();
        let short = root.with_timeout(Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(short.err(), Some(ContextError::DeadlineExceeded));
        assert_eq!(
            short.err().unwrap().to_string(),
            "context deadline exceeded"
        );
        let live = root.with_cancel();
        let stop = live.after_func(|| panic!("stopped"));
        assert!(stop.stop());
        live.cancel();
    }

    #[test]
    fn an_expiring_deadline_cancels_its_context_and_runs_after_functions() {
        let root = Context::background();
        let timed = root.with_timeout(Duration::from_millis(5));
        let child = timed.with_cancel();
        let later = timed.with_timeout(Duration::from_hours(1));
        let (ran, done) = std::sync::mpsc::channel();
        for (name, context) in [("timed", &timed), ("child", &child), ("later", &later)] {
            let ran = ran.clone();
            let _ = context.after_func(move || ran.send(name).unwrap());
        }
        let mut names: Vec<_> = (0..3)
            .map(|_| done.recv_timeout(Duration::from_secs(10)).unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["child", "later", "timed"]);
        for context in [&timed, &child, &later] {
            assert_eq!(context.err(), Some(ContextError::DeadlineExceeded));
            context.cancel();
            assert_eq!(context.err(), Some(ContextError::DeadlineExceeded));
        }
        // After the expiry a new function runs at once, and a new child is done.
        let _ = timed.after_func(move || ran.send("late").unwrap());
        assert_eq!(done.try_recv(), Ok("late"));
        assert_eq!(
            timed.with_cancel().err(),
            Some(ContextError::DeadlineExceeded)
        );
        assert_eq!(root.err(), None);
    }

    #[test]
    fn a_passed_deadline_is_the_first_reason() {
        let expired = Context::background().with_timeout(Duration::ZERO);
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = ran.clone();
        let _ = expired.after_func(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        expired.cancel();
        assert_eq!(expired.err(), Some(ContextError::DeadlineExceeded));
        // Canceled before its deadline, a context stays canceled.
        let canceled = Context::background().with_timeout(Duration::from_millis(5));
        canceled.cancel();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(canceled.err(), Some(ContextError::Canceled));
    }
}
