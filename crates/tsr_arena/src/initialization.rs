//! Same-thread reentry detection, separate from synchronization for contenders.
use crate::ArenaId;
use std::{cell::RefCell, marker::PhantomData, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitializationDomain {
    Lazy,
    Binding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key(ArenaId, InitializationDomain, u64);

thread_local! {
    static ACTIVE: RefCell<Vec<Key>> = const { RefCell::new(Vec::new()) };
}

/// Enter on the actual execution thread, before waiting on the operation's lock.
/// A binding operation may initialize JSDoc, but cannot recursively bind itself.
/// This diagnoses same-thread cycles; callers must also exclude cross-thread
/// dependencies that wait back on an active initializer.
pub struct InitializationGuard {
    key: Key,
    _thread: PhantomData<Rc<()>>,
}

impl InitializationGuard {
    pub fn assert_inactive(arena: ArenaId, domain: InitializationDomain, operation: u64) {
        let active = ACTIVE.with(|active| active.borrow().contains(&Key(arena, domain, operation)));
        assert!(
            !active,
            "{}",
            match domain {
                InitializationDomain::Lazy =>
                    "ts_arena: lazy initializer reentered its file's lazy storage",
                InitializationDomain::Binding =>
                    "ts_ast: reentrant binding of the same source file",
            }
        );
    }

    pub fn enter(arena: ArenaId, domain: InitializationDomain, operation: u64) -> Self {
        Self::assert_inactive(arena, domain, operation);
        let key = Key(arena, domain, operation);
        ACTIVE.with(|active| active.borrow_mut().push(key));
        Self {
            key,
            _thread: PhantomData,
        }
    }
}

impl Drop for InitializationGuard {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            let popped = active.borrow_mut().pop();
            debug_assert_eq!(popped, Some(self.key));
        });
    }
}
