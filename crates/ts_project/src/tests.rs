use super::*;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Barrier;
use ts_checker::{type_flags, RetainedType};

thread_local! {
    // Inject callback behavior into the real OnceLock initialization closure.
    // Taking the hook first permits a nested acquisition without borrowing TLS.
    static INITIALIZER_HOOK: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

pub(super) fn run_initializer_hook() {
    let hook = INITIALIZER_HOOK.with(|hook| hook.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

fn initializer_hook(hook: impl FnOnce() + 'static) {
    INITIALIZER_HOOK.with(|slot| {
        assert!(slot.borrow_mut().replace(Box::new(hook)).is_none());
    });
}

fn retain_literal(checker: &PooledChecker) -> RetainedType {
    let mut operation = checker.operation().unwrap();
    let literal = operation.string_literal_type(b"retained result").unwrap();
    operation.retain_type(literal).unwrap()
}

#[test]
fn same_slot_initializer_reentry_is_rejected_before_waiting() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let pool = CheckerPool::for_types(CheckerOptions::default(), &counters, 1);
    let invoked = std::rc::Rc::new(std::cell::Cell::new(false));
    let callback_pool = pool.clone();
    let callback_invoked = invoked.clone();
    initializer_hook(move || {
        assert!(matches!(
            callback_pool.acquire(CheckerSlot::Api),
            Err(Error::Reentry)
        ));
        callback_invoked.set(true);
    });

    let first = pool.acquire(CheckerSlot::Api).unwrap();
    assert!(invoked.get());
    assert!(first.operation().is_ok());
    // A stale TLS key would reject even the initialized-cell path. Exercise it
    // in release as well, where debug_assert cannot perform required cleanup.
    let second = pool.acquire(CheckerSlot::Api).unwrap();
    assert!(Arc::ptr_eq(first.owner(), second.owner()));
    drop(first);
    drop(second);
    drop(pool);
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn initializer_panic_retires_generation_and_prevents_retry() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let pool = CheckerPool::for_types(CheckerOptions::default(), &counters, 1);
    let weak = Arc::downgrade(&pool);
    initializer_hook(|| panic!("injected checker initializer panic"));

    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _checker = pool.acquire(CheckerSlot::Query(0)).unwrap();
    }))
    .unwrap_err();
    assert_eq!(
        panic.downcast_ref::<&str>(),
        Some(&"injected checker initializer panic")
    );
    assert_eq!(pool.generation().validate(), Err(ts_arena::Error::Retired));
    assert!(INITIALIZING.with(|active| active.borrow().is_empty()));
    assert!(matches!(
        pool.acquire(CheckerSlot::Query(0)),
        Err(Error::Arena(ts_arena::Error::Retired))
    ));
    assert!(matches!(
        pool.acquire(CheckerSlot::Api),
        Err(Error::Arena(ts_arena::Error::Retired))
    ));
    // OnceLock did not publish a partial owner; the reservation also unwound.
    let slots = pool.slots.lock().unwrap();
    assert!(slots[1].checker.get().is_none());
    assert_eq!(slots[1].checkouts, 0);
    drop(slots);
    drop(pool);
    assert!(weak.upgrade().is_none());
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn idle_replacement_retains_the_old_checker_but_rejects_its_types() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let pool = CheckerPool::for_types(CheckerOptions::default(), &counters, 1);
    let first = pool.acquire(CheckerSlot::Query(0)).unwrap();
    let old_identity = first.owner().identity().id();
    let old_owner = Arc::downgrade(first.owner());
    let retained = retain_literal(&first);
    let old_type = first.operation().unwrap().import_type(&retained).unwrap();

    assert!(!pool.evict_idle(CheckerSlot::Query(0)).unwrap());
    let second_checkout = pool.acquire(CheckerSlot::Query(0)).unwrap();
    assert!(Arc::ptr_eq(first.owner(), second_checkout.owner()));
    drop(first);
    assert!(!pool.evict_idle(CheckerSlot::Query(0)).unwrap());
    drop(second_checkout);
    assert!(pool.evict_idle(CheckerSlot::Query(0)).unwrap());
    assert!(old_owner.upgrade().is_some());

    let replacement = pool.acquire(CheckerSlot::Query(0)).unwrap();
    assert_ne!(replacement.owner().identity().id(), old_identity);
    {
        let operation = replacement.operation().unwrap();
        assert_eq!(
            operation.import_type(&retained),
            Err(Error::Arena(ts_arena::Error::WrongOwner))
        );
        assert_eq!(
            operation.type_flags(old_type),
            Err(Error::Arena(ts_arena::Error::WrongOwner))
        );
    }
    {
        let operation = retained.owner().operation().unwrap();
        let imported = operation.import_type(&retained).unwrap();
        assert_eq!(imported, old_type);
        assert_eq!(
            operation.type_flags(imported).unwrap(),
            type_flags::STRING_LITERAL
        );
    }

    drop(retained);
    assert!(old_owner.upgrade().is_none());
    drop(replacement);
    drop(pool);
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn api_checker_is_persistent_across_idle_eviction() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let pool = CheckerPool::for_types(CheckerOptions::default(), &counters, 1);
    let api = pool.acquire(CheckerSlot::Api).unwrap();
    let identity = api.owner().identity().id();
    let weak = Arc::downgrade(api.owner());
    assert!(!pool.evict_idle(CheckerSlot::Api).unwrap());
    drop(api);
    assert!(!pool.evict_idle(CheckerSlot::Api).unwrap());
    assert!(weak.upgrade().is_some());
    let api = pool.acquire(CheckerSlot::Api).unwrap();
    assert_eq!(api.owner().identity().id(), identity);
    drop(api);
    drop(pool);
    assert!(weak.upgrade().is_none());
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn snapshots_share_the_pool_until_the_last_root_is_dropped() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let pool = CheckerPool::for_types(CheckerOptions::default(), &counters, 1);
    let first = Snapshot::new(Project::new(pool.clone()));
    let second = Snapshot::new(first.project().clone());
    let pool_weak = Arc::downgrade(&pool);
    assert!(Arc::ptr_eq(first.project().pool(), second.project().pool()));
    let checkout = first.project().pool().acquire(CheckerSlot::Api).unwrap();
    let identity = checkout.owner().identity().id();
    let owner_weak = Arc::downgrade(checkout.owner());
    drop(checkout);
    drop(pool);
    drop(first);
    assert!(pool_weak.upgrade().is_some());
    assert!(owner_weak.upgrade().is_some());
    let checkout = second.project().pool().acquire(CheckerSlot::Api).unwrap();
    assert_eq!(checkout.owner().identity().id(), identity);
    assert!(checkout.operation().is_ok());
    drop(checkout);
    drop(second);
    assert!(pool_weak.upgrade().is_none());
    assert!(owner_weak.upgrade().is_none());
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn panic_retires_shared_snapshots_but_not_a_fresh_pool() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let pool = CheckerPool::for_types(CheckerOptions::default(), &counters, 1);
    let first = Snapshot::new(Project::new(pool.clone()));
    let second = Snapshot::new(first.project().clone());
    let api = first.project().pool().acquire(CheckerSlot::Api).unwrap();
    let query = second
        .project()
        .pool()
        .acquire(CheckerSlot::Query(0))
        .unwrap();
    let retained = retain_literal(&api);
    let old_owner = Arc::downgrade(api.owner());
    let fresh = CheckerPool::for_types(CheckerOptions::default(), &counters, 1);

    let panic = catch_unwind(AssertUnwindSafe(|| {
        let mut operation = api.operation().unwrap();
        operation
            .string_literal_type(b"partially completed work")
            .unwrap();
        panic!("injected checker operation panic");
    }))
    .unwrap_err();
    assert_eq!(
        panic.downcast_ref::<&str>(),
        Some(&"injected checker operation panic")
    );
    for snapshot in [&first, &second] {
        assert_eq!(
            snapshot.project().pool().generation().validate(),
            Err(ts_arena::Error::Retired)
        );
        for slot in [
            CheckerSlot::Diagnostics,
            CheckerSlot::Query(0),
            CheckerSlot::Api,
        ] {
            assert!(matches!(
                snapshot.project().pool().acquire(slot),
                Err(Error::Arena(ts_arena::Error::Retired))
            ));
        }
    }
    assert!(matches!(
        query.resume(),
        Err(Error::Arena(ts_arena::Error::Retired))
    ));
    assert!(matches!(
        retained.owner().operation(),
        Err(Error::Arena(ts_arena::Error::Retired))
    ));
    let fresh_api = fresh.acquire(CheckerSlot::Api).unwrap();
    assert!(fresh_api.operation().is_ok());
    assert_ne!(
        fresh_api.owner().identity().id(),
        api.owner().identity().id()
    );

    drop(query);
    drop(api);
    drop(first);
    drop(second);
    drop(pool);
    assert!(
        old_owner.upgrade().is_some(),
        "retirement must not dispose retained storage"
    );
    drop(retained);
    assert!(old_owner.upgrade().is_none());
    drop(fresh_api);
    drop(fresh);
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn callback_resume_rechecks_retirement_after_permit_release() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let pool = CheckerPool::for_types(CheckerOptions::default(), &counters, 1);
    let checker = pool.acquire(CheckerSlot::Diagnostics).unwrap();
    {
        let operation = checker.operation().unwrap();
        assert!(operation.builtin_type("stringType").is_some());
        assert!(matches!(checker.resume(), Err(Error::Reentry)));
    }
    // A callback may reacquire the checker only after the outer permit is gone.
    assert!(checker.resume().is_ok());
    pool.generation().retire();
    assert!(matches!(
        checker.resume(),
        Err(Error::Arena(ts_arena::Error::Retired))
    ));
    drop(checker);
    drop(pool);
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn final_retained_result_controls_checker_disposal() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let pool = CheckerPool::for_types(CheckerOptions::default(), &counters, 1);
    let snapshot = Snapshot::new(Project::new(pool.clone()));
    let checkout = pool.acquire(CheckerSlot::Query(0)).unwrap();
    let retained = retain_literal(&checkout);
    let owner_weak = Arc::downgrade(checkout.owner());
    let pool_weak = Arc::downgrade(&pool);
    drop(snapshot);
    drop(pool);
    assert!(
        pool_weak.upgrade().is_some(),
        "a checkout pins the pool slot"
    );
    drop(checkout);
    assert!(pool_weak.upgrade().is_none());
    assert!(owner_weak.upgrade().is_some());
    assert!(counters.snapshot().owners > baseline.owners);
    {
        let operation = retained.owner().operation().unwrap();
        let imported = operation.import_type(&retained).unwrap();
        assert_eq!(
            operation.type_flags(imported).unwrap(),
            type_flags::STRING_LITERAL
        );
    }
    drop(retained);
    assert!(owner_weak.upgrade().is_none());
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn lazy_initialization_and_same_slot_contention_share_one_owner() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let pool = CheckerPool::for_types(CheckerOptions::default(), &counters, 1);
    assert_eq!(
        counters.snapshot().owners,
        baseline.owners + 1,
        "empty slots retain only the generation"
    );
    let acquired = Barrier::new(3);
    std::thread::scope(|scope| {
        let acquire = || {
            let checkout = pool.acquire(CheckerSlot::Query(0)).unwrap();
            acquired.wait();
            checkout
        };
        let first = scope.spawn(acquire);
        let second = scope.spawn(acquire);
        acquired.wait();
        assert!(!pool.evict_idle(CheckerSlot::Query(0)).unwrap());
        let first = first.join().unwrap();
        let second = second.join().unwrap();
        assert!(Arc::ptr_eq(first.owner(), second.owner()));
        assert!(first.operation().is_ok());
        assert!(second.operation().is_ok());
    });
    assert!(pool.evict_idle(CheckerSlot::Query(0)).unwrap());
    assert_eq!(counters.snapshot().owners, baseline.owners + 1);
    drop(pool);
    assert_eq!(counters.snapshot(), baseline);
}
