use crate::{AstView, NodeIndexCache};
use std::{
    cell::RefCell,
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    sync::{Arc, Mutex, OnceLock},
};
use ts_arena::Error;

thread_local! { static INITIALIZING: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) }; }

/// `sync.Once.Do` completes even when its function panics. Its zero pointer
/// remains observable on the next read. Rust ownership errors instead return
/// without claiming a completed source operation and may be retried.
#[derive(Debug, Default)]
pub(crate) struct SourceNodeIndexCache {
    value: OnceLock<Option<Arc<NodeIndexCache>>>,
    initialize: Mutex<()>,
}

impl SourceNodeIndexCache {
    pub(crate) fn get_or_try_init(
        &self,
        view: AstView<'_>,
        build: impl FnOnce() -> Result<NodeIndexCache, Error>,
    ) -> Result<Option<&Arc<NodeIndexCache>>, Error> {
        if let Some(value) = self.value.get() {
            return Ok(value.as_ref());
        }
        let address = std::ptr::from_ref(self).addr();
        assert!(
            !INITIALIZING.with(|active| active.borrow().contains(&address)),
            "reentrant SourceFile node-index initialization"
        );
        let lock = self
            .initialize
            .lock()
            .expect("node-index initializer unlocks before unwinding");
        if let Some(value) = self.value.get() {
            return Ok(value.as_ref());
        }
        // The shared borrow fixes the cache's address for the initializer's
        // duration. No pointer is reconstructed or used to access storage.
        INITIALIZING.with(|active| active.borrow_mut().push(address));
        let outcome = catch_unwind(AssertUnwindSafe(build));
        INITIALIZING.with(|active| {
            assert_eq!(active.borrow_mut().pop(), Some(address));
        });
        match outcome {
            Ok(Ok(table)) => {
                for &node in table.nodes().iter().flatten() {
                    view.node(node)?;
                }
                self.value
                    .set(Some(Arc::new(table)))
                    .expect("serialized cache initialization");
            }
            Ok(Err(error)) => return Err(error),
            Err(payload) => {
                self.value
                    .set(None)
                    .expect("serialized cache initialization");
                drop(lock);
                resume_unwind(payload);
            }
        }
        Ok(self.value.get().expect("completed initialization").as_ref())
    }

    pub(crate) fn validate_references(&self, view: AstView<'_>) -> Result<(), Error> {
        if let Some(Some(table)) = self.value.get() {
            for &node in table.nodes().iter().flatten() {
                view.node(node)?;
            }
        }
        Ok(())
    }
}
