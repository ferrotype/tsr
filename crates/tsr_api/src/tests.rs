use super::*;
use std::cell::RefCell;
use std::sync::mpsc;
use ts_arena::{Counters, Counts};
use ts_checker::CheckerOptions;
use ts_project::{CheckerPool, Project};

thread_local! {
    static PUBLICATION_PAUSE: RefCell<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>> = const { RefCell::new(None) };
}

// The production commit reaches this point with its gate, registry and queue
// locked. This one-shot test rendezvous injects an attempted retirement before
// the actual queue insertion; it never substitutes for the real commit path.
pub(super) fn before_response_queue_publication() {
    let pause = PUBLICATION_PAUSE.with(|pause| pause.borrow_mut().take());
    if let Some((ready, resume)) = pause {
        ready.send(()).unwrap();
        resume.recv().unwrap();
    }
}

fn pool(counters: &Counters) -> Arc<CheckerPool> {
    CheckerPool::for_types(CheckerOptions::default(), counters, 1)
}
fn snapshot(pool: &Arc<CheckerPool>) -> Snapshot {
    Snapshot::new(ts_project::Snapshot::new(Project::new(pool.clone()))).unwrap()
}
fn prepared(snapshot: &Snapshot, bytes: &[u8]) -> (u32, PreparedResponse) {
    let operation = snapshot.checker().operation().unwrap();
    let ty = operation.builtin_type("stringType").unwrap();
    (
        ty.id(),
        snapshot.prepare(&operation, &[ty], bytes.to_vec()).unwrap(),
    )
}
fn retired<T>(result: Result<T, Error>) {
    assert_eq!(
        result.err(),
        Some(Error::Checker(ts_checker::Error::Arena(
            ts_arena::Error::Retired
        )))
    );
}
fn panic_in_query(snapshot: &Snapshot) {
    let result = snapshot.request::<()>(|| {
        let slot = snapshot
            .0
            .project
            .project()
            .pool()
            .acquire(CheckerSlot::Query(0))?;
        let _operation = slot.operation()?;
        panic!("injected checker failure");
    });
    assert_eq!(
        result,
        Err(Error::Panicked("injected checker failure".into()))
    );
}

#[test]
fn retirement_before_commit_suppresses_registry_shared_result_and_success() {
    let counters = Counters::new();
    {
        let pool = pool(&counters);
        let first = snapshot(&pool);
        let second = snapshot(&pool);
        let queue = ResponseQueue::new(1);
        let (id, response) = prepared(&first, b"uncommitted");
        let (ready, ready_rx) = mpsc::channel();
        let (resume, resume_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            // Disconnect on a failing assertion before scope joins its worker.
            let resume_commit = resume;
            let first = &first;
            let queue = &queue;
            let worker = scope.spawn(move || {
                let _operation = first.checker().operation().unwrap();
                ready.send(()).unwrap();
                resume_rx.recv().unwrap();
                retired(first.commit(response, queue));
            });
            ready_rx.recv().unwrap();
            panic_in_query(&second);
            resume_commit.send(()).unwrap();
            worker.join().unwrap();
        });
        assert!(queue.pop().is_none());
        assert!(first.0.registry.lock().unwrap().types.is_empty());
        assert!(first.0.registry.lock().unwrap().latest.is_none());
        retired(first.with_type(id, |_, _| Ok(())));
        retired(second.latest());
        assert!(matches!(
            pool.acquire(CheckerSlot::Diagnostics),
            Err(ts_checker::Error::Arena(ts_arena::Error::Retired))
        ));
    }
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn commit_holds_generation_gate_through_queue_publication() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    {
        let pool = pool(&counters);
        let snapshot = snapshot(&pool);
        let queue = ResponseQueue::new(1);
        let (id, response) = prepared(&snapshot, b"ordered commitment");
        let (ready, ready_rx) = mpsc::channel();
        let (resume, resume_rx) = mpsc::channel();
        let (attempted, attempted_rx) = mpsc::channel();
        let (retired_tx, retired_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            // On a failed contention assertion, disconnect the publication
            // waiter before scope joins it; the regression must fail, not hang.
            let resume_on_drop = resume;
            let snapshot = &snapshot;
            let queue = &queue;
            let committing = scope.spawn(move || {
                PUBLICATION_PAUSE.with(|pause| {
                    *pause.borrow_mut() = Some((ready, resume_rx));
                });
                snapshot.commit(response, queue).unwrap();
            });
            ready_rx.recv().unwrap();
            let retiring = scope.spawn(|| {
                ts_arena::observe_next_retirement_contention(attempted);
                pool.generation().retire();
                retired_tx.send(()).unwrap();
            });
            // This arrives only after retirement's actual gate try_lock
            // returned WouldBlock. Replacing commit's held guard with an
            // earlier validate() would fail that observation.
            attempted_rx.recv().unwrap();
            assert!(matches!(
                retired_rx.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
            assert_eq!(pool.generation().validate(), Ok(()));
            resume_on_drop.send(()).unwrap();
            committing.join().unwrap();
            retired_rx.recv().unwrap();
            retiring.join().unwrap();
        });
        let registry = snapshot.0.registry.lock().unwrap();
        assert_eq!(registry.types.get(&id).unwrap().id(), id);
        assert_eq!(
            registry.latest.as_ref().unwrap().bytes(),
            b"ordered commitment"
        );
        drop(registry);
        assert_eq!(queue.pop().unwrap().bytes(), b"ordered commitment");
        assert!(queue.pop().is_none());
        retired(snapshot.with_type(id, |_, _| Ok(())));
        retired(snapshot.latest());
    }
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn commitment_before_retirement_survives_delivery_but_handles_are_invalidated() {
    let counters = Counters::new();
    {
        let pool = pool(&counters);
        let first = snapshot(&pool);
        let second = snapshot(&pool);
        let queue = ResponseQueue::new(1);
        let (id, response) = prepared(&first, b"committed");
        first.commit(response, &queue).unwrap();
        let shared = first.latest().unwrap().unwrap();
        assert_eq!(shared.type_ids().collect::<Vec<_>>(), [id]);
        first
            .with_type(id, |op, ty| {
                assert_eq!(op.type_kind(ty).unwrap(), ts_checker::TypeKind::Intrinsic);
                Ok(())
            })
            .unwrap();
        // Same pool, different snapshot: registration was not copied.
        assert!(matches!(
            second.with_type(id, |_, _| Ok(())),
            Err(Error::UnknownType)
        ));
        panic_in_query(&second);
        assert_eq!(queue.pop().unwrap().bytes(), b"committed");
        assert_eq!(shared.bytes(), b"committed");
        retired(first.with_type(id, |_, _| Ok(())));
        retired(first.latest());
    }
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn lookup_releases_registry_and_gate_before_permit_acquisition_and_revalidates() {
    let counters = Counters::new();
    {
        let pool = pool(&counters);
        let first = snapshot(&pool);
        let other = snapshot(&pool);
        let queue = ResponseQueue::new(1);
        let (id, response) = prepared(&first, b"before");
        first.commit(response, &queue).unwrap();
        let held = first.checker().operation().unwrap();
        let (attempted, attempted_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                ts_arena::observe_next_lease_contention(attempted);
                retired::<()>(
                    first.with_type(id, |_, _| panic!("retired callback must not execute")),
                );
            });
            attempted_rx.recv().unwrap();
            // The reader has attempted the actual checker permit and observed
            // WouldBlock. Neither publication lock survives into that wait;
            // sibling retirement must complete before this permit is released.
            assert!(first.0.registry.try_lock().is_ok());
            panic_in_query(&other);
            drop(held);
            reader.join().unwrap();
        });
    }
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn callback_reentry_requires_releasing_the_operation_and_rechecks_retirement() {
    let counters = Counters::new();
    {
        let pool = pool(&counters);
        let snapshot = snapshot(&pool);
        let slot = pool.acquire(CheckerSlot::Api).unwrap();
        let operation = slot.operation().unwrap();
        assert!(matches!(slot.resume(), Err(ts_checker::Error::Reentry)));
        drop(operation);
        snapshot
            .request(|| {
                let _nested = slot.resume()?;
                Ok(())
            })
            .unwrap();
        let outcome = snapshot.request(|| {
            panic_in_query(&snapshot);
            Ok(())
        });
        retired(outcome);
        assert!(matches!(
            slot.resume(),
            Err(ts_checker::Error::Arena(ts_arena::Error::Retired))
        ));
    }
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn serialization_panic_without_a_lease_retires_all_snapshots() {
    let counters = Counters::new();
    {
        let pool = pool(&counters);
        let first = snapshot(&pool);
        let second = snapshot(&pool);
        assert_eq!(
            first.request::<()>(|| panic!("serialization failed")),
            Err(Error::Panicked("serialization failed".into()))
        );
        retired(second.latest());
    }
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn equal_numeric_ids_cannot_cross_checkers_snapshots_or_replacement_generations() {
    let counters = Counters::new();
    {
        let old_pool = pool(&counters);
        let old = snapshot(&old_pool);
        let same_pool_snapshot = snapshot(&old_pool);
        let queue = ResponseQueue::new(2);
        let (id, response) = prepared(&old, b"old");
        assert_eq!(
            same_pool_snapshot.commit(response, &queue),
            Err(Error::WrongSnapshot)
        );
        let query = old_pool.acquire(CheckerSlot::Query(0)).unwrap();
        let operation = query.operation().unwrap();
        let foreign = operation.builtin_type("stringType").unwrap();
        assert_eq!(foreign.id(), id);
        assert!(matches!(
            old.prepare(&operation, &[foreign], vec![]),
            Err(Error::Checker(ts_checker::Error::Arena(
                ts_arena::Error::WrongOwner
            )))
        ));
        drop(operation);
        let (_, response) = prepared(&old, b"old");
        old.commit(response, &queue).unwrap();
        old_pool.generation().retire();
        let replacement_pool = pool(&counters);
        let replacement = snapshot(&replacement_pool);
        let (new_id, response) = prepared(&replacement, b"new");
        assert_eq!(id, new_id);
        assert_ne!(
            old.checker().identity().id(),
            replacement.checker().identity().id()
        );
        replacement.commit(response, &queue).unwrap();
        replacement.with_type(id, |_, _| Ok(())).unwrap();
        retired(old.with_type(id, |_, _| Ok(())));
    }
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn queue_backpressure_leaves_no_partial_registration_or_shared_publication() {
    let counters = Counters::new();
    {
        let pool = pool(&counters);
        let snapshot = snapshot(&pool);
        let queue = ResponseQueue::new(1);
        let (id, response) = prepared(&snapshot, b"first");
        snapshot.commit(response, &queue).unwrap();
        let operation = snapshot.checker().operation().unwrap();
        let ty = operation.builtin_type("numberType").unwrap();
        let second = snapshot
            .prepare(&operation, &[ty], b"second".to_vec())
            .unwrap();
        assert_eq!(snapshot.commit(second, &queue), Err(Error::QueueFull));
        assert!(matches!(
            snapshot.retained_type(ty.id()),
            Err(Error::UnknownType)
        ));
        assert_eq!(snapshot.latest().unwrap().unwrap().bytes(), b"first");
        assert_eq!(snapshot.retained_type(id).unwrap().id(), id);
        assert_eq!(queue.pop().unwrap().bytes(), b"first");
        let second = snapshot
            .prepare(&operation, &[ty], b"second".to_vec())
            .unwrap();
        snapshot.commit(second, &queue).unwrap();
        assert_eq!(queue.pop().unwrap().bytes(), b"second");
    }
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn registry_shared_result_and_queue_retain_storage_until_their_final_roots_drop() {
    let counters = Counters::new();
    let weak;
    let response;
    {
        let pool = pool(&counters);
        let first = snapshot(&pool);
        let clone = first.clone();
        weak = Arc::downgrade(first.checker());
        let queue = ResponseQueue::new(1);
        let (id, prepared) = prepared(&first, b"owned");
        first.commit(prepared, &queue).unwrap();
        drop(first);
        clone.with_type(id, |_, _| Ok(())).unwrap();
        response = clone.latest().unwrap().unwrap();
        pool.generation().retire();
        assert!(weak.upgrade().is_some());
        drop(clone);
        drop(pool);
        assert_eq!(queue.pop().unwrap().bytes(), b"owned");
    }
    assert!(weak.upgrade().is_some());
    assert!(counters.snapshot().owners > 0);
    drop(response);
    assert!(weak.upgrade().is_none());
    assert_eq!(counters.snapshot(), Counts::default());
}
