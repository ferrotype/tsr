//! A cancellation token for checker work (docs/PHASE2-C6-plan.md, C6.5, owner
//! decision 5).
//!
//! The pin passes a `context.Context` to the checker's `checkSourceFile` and
//! polls `ctx.Err()` at a few fixed sites. The only property the checker reads
//! is whether the context is done, so the Rust counterpart is a shared flag:
//! `Send + Sync`, set once by whoever cancels, and polled without a lock.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A shared cancellation flag. Clones observe the same flag.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation; every clone observes it at its next poll.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// `ctx.Err() != nil`: whether cancellation was requested.
    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::CancellationToken;

    #[test]
    fn clones_share_one_flag_across_threads() {
        let token = CancellationToken::new();
        let clone = token.clone();
        assert!(!clone.is_canceled());
        std::thread::spawn(move || token.cancel()).join().unwrap();
        assert!(clone.is_canceled());
    }
}
