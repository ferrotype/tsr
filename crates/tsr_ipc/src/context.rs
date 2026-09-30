//! The part of Go's `context.Context` the connections and the content-mapper
//! host use: cancellation that flows from parents to children, deadlines, and
//! functions to run when a context is canceled (`context.AfterFunc`).
use std::sync::{Arc, Mutex, Weak};
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
        let deadline = match (deadline, self.deadline()) {
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
        }
        child
    }

    /// A child canceled by its cancel function or with its parent
    /// (`context.WithCancel`).
    #[must_use]
    pub fn with_cancel(&self) -> Self {
        self.child(None)
    }

    /// A child that is also done after `timeout` (`context.WithTimeout`).
    #[must_use]
    pub fn with_timeout(&self, timeout: Duration) -> Self {
        self.child(Some(Instant::now() + timeout))
    }

    /// The earliest deadline of this context and its ancestors.
    pub fn deadline(&self) -> Option<Instant> {
        self.0.deadline
    }

    /// Cancels this context and its descendants, running their after-functions.
    pub fn cancel(&self) {
        self.cancel_with(ContextError::Canceled);
    }

    fn cancel_with(&self, error: ContextError) {
        let (children, after) = {
            let mut state = self.0.state.lock().expect("context lock");
            if state.canceled.is_some() {
                return;
            }
            state.canceled = Some(error);
            (
                std::mem::take(&mut state.children),
                std::mem::take(&mut state.after),
            )
        };
        for (_, function) in after {
            function();
        }
        for child in children.iter().filter_map(Weak::upgrade) {
            Self(child).cancel_with(error);
        }
    }

    /// Why the context is done, or `None` while it is live (`ctx.Err()`).
    pub fn err(&self) -> Option<ContextError> {
        if let Some(error) = self.0.state.lock().expect("context lock").canceled {
            return Some(error);
        }
        if self
            .0
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Some(ContextError::DeadlineExceeded);
        }
        self.0.parent.as_ref().and_then(Self::err)
    }

    /// Runs `function` once the context is canceled, on the canceling thread,
    /// or at once when it already is (`context.AfterFunc`). The returned stop
    /// function unregisters it and reports whether it did so before it ran.
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
}
