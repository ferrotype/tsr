use super::*;
use crate::clock::manual::ManualClock;
use tsr_arena::Counters;
use tsr_compiler::{FileCache, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, Tristate};
use tsr_jsstring::JsString;
use tsr_vfs::MemoryBuilder;

fn setup(queries: usize) -> (Arc<CheckerScheduler>, Arc<ManualClock>) {
    let mut fs = MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/a.ts", &b"let a: number = 'bad';"[..]);
    fs.insert_loaded(b"/b.ts", &b"let b = 1;"[..]);
    let counters = Counters::new();
    let program = Program::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                CompilerOptions {
                    no_lib: Tristate::TRUE,
                    ..Default::default()
                },
                vec![
                    JsString::from_bytes(b"/a.ts".as_slice()),
                    JsString::from_bytes(b"/b.ts".as_slice()),
                ],
            ),
            host: Arc::new(fs.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/".as_slice()),
            skip_module_resolution: false,
            single_threaded: Tristate::TRUE,
        },
        &mut FileCache::default(),
        &counters,
    )
    .unwrap();
    let program = Arc::new(program);
    let pool = CheckerPool::for_program(
        Arc::new(ProgramCheckerHost::new(program.clone())),
        &counters,
        queries,
    );
    let clock = ManualClock::new();
    (
        CheckerScheduler::with_clock(pool, program, Duration::from_secs(10), clock.clone()),
        clock,
    )
}
fn context() -> Context {
    Context::background().with_cancel()
}
fn take(
    s: &Arc<CheckerScheduler>,
    purpose: CheckerLifetime,
    ctx: &Context,
    id: &str,
) -> ScheduledChecker {
    s.acquire(purpose, None, ctx, id).unwrap()
}
fn wait_until_contended(s: &CheckerScheduler) {
    let state = s.state.lock().unwrap();
    let (state, timeout) = s
        .available
        .wait_timeout_while(state, Duration::from_secs(5), |s| s.waiters == 0)
        .unwrap();
    assert!(
        !timeout.timed_out() && state.waiters > 0,
        "worker must reach actual slot contention"
    );
}
// source: tsc/internal/project/checkerpool_test.go:TestCheckerPoolRequestAffinity
// source: tsc/internal/project/checkerpool_test.go:TestCheckerPoolLifetimeMismatchIgnoresAssociation
#[test]
fn requests_reuse_across_nested_calls_and_releases_but_not_categories() {
    let (s, _) = setup(2);
    let ctx = context();
    let first = take(&s, CheckerLifetime::Temporary, &ctx, "request");
    let identity = first.owner().identity().id();
    let nested = take(&s, CheckerLifetime::Temporary, &ctx, "request");
    assert_eq!(nested.owner().identity().id(), identity);
    drop(nested);
    drop(first);
    let again = take(&s, CheckerLifetime::Temporary, &ctx, "request");
    assert_eq!(again.owner().identity().id(), identity);
    let diagnostics = take(&s, CheckerLifetime::Diagnostics, &ctx, "request");
    assert_ne!(diagnostics.owner().identity().id(), identity);
    drop(diagnostics);
    drop(again);
    assert!(s.state.lock().unwrap().requests.contains_key("request"));
    ctx.cancel();
    assert!(s.state.lock().unwrap().requests.is_empty());
    let unnamed = take(
        &s,
        CheckerLifetime::Temporary,
        &Context::background(),
        "ignored-name",
    );
    drop(unnamed);
    assert!(s.state.lock().unwrap().requests.is_empty());
}
// source: tsc/internal/project/checkerpool_test.go:TestCheckerPoolFileAffinity
// source: tsc/internal/project/checkerpool_test.go:TestCheckerPoolMultipleConcurrentQueryCheckers
#[test]
fn queries_prefer_file_affinity_and_then_an_existing_idle_checker() {
    let (s, _) = setup(3);
    let a = s.program.files()[0].source();
    let b = s.program.files()[1].source();
    let ctx = context();
    let first = s
        .acquire(CheckerLifetime::Temporary, Some(a), &ctx, "one")
        .unwrap();
    let second = s
        .acquire(CheckerLifetime::Temporary, Some(b), &ctx, "two")
        .unwrap();
    let identity = second.owner().identity().id();
    assert_ne!(first.owner().identity().id(), identity);
    drop(first);
    drop(second);
    let again = s
        .acquire(CheckerLifetime::Temporary, Some(b), &ctx, "three")
        .unwrap();
    assert_eq!(again.owner().identity().id(), identity);
    let next = take(&s, CheckerLifetime::Temporary, &ctx, "four");
    assert_eq!(
        next.0.index, 1,
        "existing idle slot precedes the empty third slot"
    );
    ctx.cancel();
}

// getQueryChecker returns immediately after tryReacquireForRequest; a query of
// another file in that request must not overwrite the file's own association.
#[test]
fn request_reacquisition_preserves_the_other_files_checker_affinity() {
    let (s, _) = setup(2);
    let a = s.program.files()[0].source();
    let b = s.program.files()[1].source();
    let ctx = context();
    let first = s
        .acquire(CheckerLifetime::Temporary, Some(a), &ctx, "one")
        .unwrap();
    let second = s
        .acquire(CheckerLifetime::Temporary, Some(b), &ctx, "two")
        .unwrap();
    let first_id = first.owner().identity().id();
    let second_id = second.owner().identity().id();
    assert_ne!(first_id, second_id);
    drop(first);
    drop(second);

    let reacquired = s
        .acquire(CheckerLifetime::Temporary, Some(b), &ctx, "one")
        .unwrap();
    assert_eq!(reacquired.owner().identity().id(), first_id);
    drop(reacquired);
    let later = s
        .acquire(CheckerLifetime::Temporary, Some(b), &ctx, "three")
        .unwrap();
    assert_eq!(later.owner().identity().id(), second_id);
    ctx.cancel();
}
// source: tsc/internal/project/checkerpool_test.go:TestCheckerPoolStaggeredIdleCleanup
// source: tsc/internal/project/checkerpool_test.go:TestCheckerPoolAPICheckerStableIdentity
#[test]
fn deadlines_evict_idle_slots_in_order_and_never_evict_api_or_held_slots() {
    let (s, clock) = setup(2);
    let ctx = context();
    let diag = take(&s, CheckerLifetime::Diagnostics, &ctx, "diag");
    let diag_id = diag.owner().identity().id();
    drop(diag);
    clock.advance(Duration::from_secs(6));
    let query = take(&s, CheckerLifetime::Temporary, &ctx, "query");
    let query_id = query.owner().identity().id();
    drop(query);
    let held = take(&s, CheckerLifetime::Temporary, &ctx, "held");
    let api = take(&s, CheckerLifetime::Api, &ctx, "");
    let api_id = api.owner().identity().id();
    drop(api);
    clock.advance(Duration::from_secs(4));
    assert!(!s.state.lock().unwrap().slots[0].initialized);
    assert!(
        s.state.lock().unwrap().slots[1].initialized,
        "held query must survive its old deadline"
    );
    let diag = take(&s, CheckerLifetime::Diagnostics, &ctx, "diag");
    assert_ne!(diag.owner().identity().id(), diag_id);
    drop(diag);
    drop(held);
    clock.advance(Duration::from_secs(10));
    let query = take(&s, CheckerLifetime::Temporary, &ctx, "query");
    assert_ne!(query.owner().identity().id(), query_id);
    let api = take(&s, CheckerLifetime::Api, &ctx, "");
    assert_eq!(api.owner().identity().id(), api_id);
    ctx.cancel();
}
// source: tsc/internal/project/checkerpool_test.go:TestCheckerPoolDiscardHeldCheckerSurvivesRelease
// source: tsc/internal/project/checkerpool_test.go:TestCheckerPoolDiscardStillFunctional
#[test]
fn discarded_pools_survive_timer_callbacks_and_remain_fully_usable() {
    let (s, clock) = setup(1);
    let ctx = context();
    let diag = take(&s, CheckerLifetime::Diagnostics, &ctx, "diag");
    let diag_id = diag.owner().identity().id();
    drop(diag);
    let epoch = s.state.lock().unwrap().timer_epoch;
    let query = take(&s, CheckerLifetime::Temporary, &ctx, "query");
    let query_id = query.owner().identity().id();
    s.discard();
    s.discard();
    s.cleanup(epoch); // a callback already dispatched when Discard stopped its timer
    drop(query);
    clock.advance(Duration::from_secs(100));
    assert_eq!(
        take(&s, CheckerLifetime::Diagnostics, &ctx, "diag")
            .owner()
            .identity()
            .id(),
        diag_id
    );
    assert_eq!(
        take(&s, CheckerLifetime::Temporary, &ctx, "query")
            .owner()
            .identity()
            .id(),
        query_id
    );
    ctx.cancel();
}
// source: tsc/internal/project/checkerpool_test.go:TestCheckerPoolCrossReleaseAffinityWithContention
#[test]
fn contention_waits_for_release_but_other_categories_proceed() {
    let (s, _) = setup(1);
    let ctx = context();
    let first = take(&s, CheckerLifetime::Temporary, &ctx, "a");
    let id = first.owner().identity().id();
    drop(first);
    let other = take(&s, CheckerLifetime::Temporary, &context(), "b");
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| take(&s, CheckerLifetime::Temporary, &ctx, "a"));
        wait_until_contended(&s);
        let diagnostics = take(&s, CheckerLifetime::Diagnostics, &ctx, "diag");
        assert_ne!(diagnostics.owner().identity().id(), id);
        drop(other);
        assert_eq!(worker.join().unwrap().owner().identity().id(), id);
    });
    ctx.cancel();
}
#[test]
fn cancellation_wakes_a_worker_which_has_not_acquired_a_slot() {
    let (s, _) = setup(1);
    let held = take(&s, CheckerLifetime::Temporary, &Context::background(), "");
    let ctx = context();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| s.acquire(CheckerLifetime::Temporary, None, &ctx, "waiter"));
        wait_until_contended(&s);
        ctx.cancel();
        assert!(matches!(
            worker.join().unwrap(),
            Err(AcquireError::Canceled(ContextError::Canceled))
        ));
        assert!(s.state.lock().unwrap().requests.is_empty());
    });
    drop(held);
}
// source: tsc/internal/project/checkerpool_test.go:TestCheckerPoolCanceledCheckerDisposal
// source: tsc/internal/project/checkerpool_test.go:TestCheckerPoolAPICheckerDisposedOnCancel
#[test]
fn canceled_checkers_and_their_associations_are_disposed_in_all_categories() {
    for purpose in [
        CheckerLifetime::Diagnostics,
        CheckerLifetime::Temporary,
        CheckerLifetime::Api,
    ] {
        let (s, _) = setup(1);
        let ctx = context();
        let checkout = take(&s, purpose, &ctx, "canceled");
        let old = checkout.owner().identity().id();
        let token = tsr_core::CancellationToken::new();
        token.cancel();
        assert!(checkout
            .operation()
            .unwrap()
            .semantic_diagnostics_cancellable(s.program.files()[0].source(), &token)
            .unwrap()
            .is_empty());
        assert!(checkout.owner().was_canceled());
        drop(checkout);
        assert!(s.state.lock().unwrap().requests.is_empty());
        let fresh = take(&s, purpose, &ctx, "fresh");
        assert_ne!(fresh.owner().identity().id(), old);
        assert!(fresh.operation().is_ok());
        ctx.cancel();
    }
}
// source: tsc/internal/project/checkerpool_test.go:TestCheckerPoolTakeNewGlobalDiagnostics
#[test]
fn globals_accumulate_once_and_survive_disposal() {
    let (s, clock) = setup(1);
    assert!(s.global_diagnostics().unwrap().is_empty());
    assert!(!s.take_new_global_diagnostics());
    drop(take(
        &s,
        CheckerLifetime::Diagnostics,
        &Context::background(),
        "",
    ));
    let globals = s.global_diagnostics().unwrap();
    assert!(
        !globals.is_empty(),
        "noLib leaves the required global types unresolved"
    );
    assert!(s.take_new_global_diagnostics());
    assert!(!s.take_new_global_diagnostics());
    drop(take(
        &s,
        CheckerLifetime::Temporary,
        &Context::background(),
        "",
    ));
    assert_eq!(s.global_diagnostics().unwrap(), globals);
    assert!(!s.take_new_global_diagnostics());
    clock.advance(Duration::from_secs(10));
    assert_eq!(s.global_diagnostics().unwrap(), globals);
}

#[test]
fn unscoped_reentry_is_an_error_before_waiting_for_its_own_slot() {
    let (s, _) = setup(1);
    for purpose in [
        CheckerLifetime::Diagnostics,
        CheckerLifetime::Temporary,
        CheckerLifetime::Api,
    ] {
        let checker = take(&s, purpose, &Context::background(), "");
        let _operation = checker.operation().unwrap();
        assert!(matches!(
            s.acquire(purpose, None, &Context::background(), ""),
            Err(AcquireError::Checker(Error::Reentry))
        ));
    }
}

// source: tsc/internal/project/checkerpool_test.go:TestCheckerPoolStaggeredIdleCleanup
#[test]
fn a_later_release_does_not_postpone_an_earlier_deadline() {
    let (s, clock) = setup(2);
    let ctx = context();
    let first = take(&s, CheckerLifetime::Temporary, &ctx, "first");
    let second = take(&s, CheckerLifetime::Temporary, &ctx, "second");
    drop(first);
    clock.advance(Duration::from_secs(6));
    drop(second);
    clock.advance(Duration::from_secs(4));
    {
        let state = s.state.lock().unwrap();
        assert!(!state.slots[1].initialized);
        assert!(state.slots[2].initialized);
        assert!(!state.requests.contains_key("first"));
        assert!(state.requests.contains_key("second"));
    }
    clock.advance(Duration::from_secs(6));
    assert!(!s.state.lock().unwrap().slots[2].initialized);
    ctx.cancel();
}
