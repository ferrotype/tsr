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

#[test]
fn category_routing_defaults_and_minimum_capacity_match_native_observations() {
    let defaults = crate::session::SessionOptions::default();
    assert_eq!(defaults.query_checkers, 3);
    for queries in [1, defaults.query_checkers] {
        let (s, clock) = setup(queries);
        let s =
            CheckerScheduler::with_clock(s.pool.clone(), s.program.clone(), Duration::ZERO, clock);
        assert_eq!(s.idle_timeout, Duration::from_secs(30));
        assert_eq!(s.pool.query_slots, queries);
        // Native MaxCheckers counts diagnostics plus queries; Rust also stores API.
        assert_eq!(s.state.lock().unwrap().slots.len(), queries + 2);
        let diag = take(&s, CheckerLifetime::Diagnostics, &context(), "diag");
        let query = take(&s, CheckerLifetime::Temporary, &context(), "query");
        let api = take(&s, CheckerLifetime::Api, &Context::background(), "");
        assert_eq!(diag.0.index, 0);
        assert!((1..=queries).contains(&query.0.index));
        assert_eq!(api.0.index, queries + 1);
        assert_ne!(diag.owner().identity().id(), query.owner().identity().id());
        assert_ne!(diag.owner().identity().id(), api.owner().identity().id());
        assert_ne!(query.owner().identity().id(), api.owner().identity().id());
    }
}

#[test]
fn saturated_query_and_diagnostics_categories_wait_independently() {
    let (s, _) = setup(3);
    let ctx = context();
    let mut queries: Vec<_> = (0..3)
        .map(|i| take(&s, CheckerLifetime::Temporary, &ctx, &format!("q{i}")))
        .collect();
    for (i, a) in queries.iter().enumerate() {
        assert!(a.0.index > 0);
        for b in &queries[i + 1..] {
            assert_ne!(a.owner().identity().id(), b.owner().identity().id());
        }
    }
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| take(&s, CheckerLifetime::Temporary, &ctx, "fourth"));
        wait_until_contended(&s);
        let diag = take(&s, CheckerLifetime::Diagnostics, &ctx, "diag");
        drop(queries.pop());
        assert!(worker.join().unwrap().0.index > 0);
        drop(diag);
    });
    drop(queries);
    let diag = take(&s, CheckerLifetime::Diagnostics, &ctx, "first-diag");
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| take(&s, CheckerLifetime::Diagnostics, &ctx, "second-diag"));
        wait_until_contended(&s);
        let query = take(&s, CheckerLifetime::Temporary, &ctx, "independent-query");
        assert_ne!(diag.owner().identity().id(), query.owner().identity().id());
        drop(diag);
        assert_eq!(worker.join().unwrap().0.index, 0);
    });
    ctx.cancel();
}

#[test]
fn diagnostics_affinity_survives_release_and_lifetime_changes_ignore_it() {
    let (s, _) = setup(3);
    let ctx = context();
    let first = take(&s, CheckerLifetime::Diagnostics, &ctx, "same");
    let identity = first.owner().identity().id();
    drop(first);
    let again = take(&s, CheckerLifetime::Diagnostics, &ctx, "same");
    assert_eq!(again.owner().identity().id(), identity);
    drop(again);
    let distinct_request = take(&s, CheckerLifetime::Diagnostics, &ctx, "different");
    assert_eq!(distinct_request.owner().identity().id(), identity);
    drop(distinct_request);
    let query = take(&s, CheckerLifetime::Temporary, &ctx, "same");
    assert!(query.0.index > 0);
    assert_ne!(query.owner().identity().id(), identity);
    drop(query);
    for _ in 0..2 {
        drop(take(
            &s,
            CheckerLifetime::Temporary,
            &Context::background(),
            "",
        ));
    }
    ctx.cancel();
}

#[test]
fn idle_disposal_removes_file_associations_and_staggered_slots_survive_before_deadline() {
    let (s, clock) = setup(3);
    let ctx = context();
    let file = s.program.files()[0].source();
    let first = s
        .acquire(CheckerLifetime::Temporary, Some(file), &ctx, "first")
        .unwrap();
    let second = take(&s, CheckerLifetime::Temporary, &ctx, "second");
    let a = first.0.index;
    let b = second.0.index;
    assert_eq!(s.state.lock().unwrap().files.get(&file), Some(&a));
    drop(first);
    clock.advance(Duration::from_secs(6));
    drop(second);
    {
        let state = s.state.lock().unwrap();
        assert!(state.slots[a].initialized && state.slots[b].initialized);
    }
    clock.advance(Duration::from_secs(11));
    let state = s.state.lock().unwrap();
    assert!(!state.slots[a].initialized && !state.slots[b].initialized);
    assert!(!state.files.contains_key(&file));
    drop(state);
    ctx.cancel();
}

#[test]
fn discard_keeps_idle_and_api_checkers_and_never_rearms_cleanup() {
    let (s, clock) = setup(3);
    let ctx = context();
    let mut identities = Vec::new();
    for purpose in [
        CheckerLifetime::Diagnostics,
        CheckerLifetime::Temporary,
        CheckerLifetime::Api,
    ] {
        let checker = take(&s, purpose, &ctx, "existing");
        identities.push((purpose, checker.0.index, checker.owner().identity().id()));
        drop(checker);
    }
    let epoch = s.state.lock().unwrap().timer_epoch;
    assert!(s.state.lock().unwrap().timer.is_some());
    s.discard();
    s.discard();
    s.cleanup(epoch);
    {
        let state = s.state.lock().unwrap();
        assert!(state.timer.is_none());
        assert!(state.timer_deadline.is_none());
        for &(_, index, id) in &identities {
            assert!(state.slots[index].initialized);
            assert_eq!(state.slots[index].identity.as_ref().unwrap().id(), id);
        }
    }
    clock.advance(Duration::from_mins(1));
    for (purpose, _, id) in identities {
        let checker = take(&s, purpose, &ctx, "reacquired");
        assert_eq!(checker.owner().identity().id(), id);
        drop(checker);
    }
    assert!(s.state.lock().unwrap().timer.is_none());
    ctx.cancel();

    let (fresh, _) = setup(1);
    fresh.discard();
    let first = take(
        &fresh,
        CheckerLifetime::Temporary,
        &Context::background(),
        "",
    );
    let id = first.owner().identity().id();
    assert_eq!(first.0.index, 1);
    drop(first);
    assert_eq!(
        take(
            &fresh,
            CheckerLifetime::Temporary,
            &Context::background(),
            ""
        )
        .owner()
        .identity()
        .id(),
        id
    );
}

#[test]
fn canceled_api_release_clears_slot_and_nested_handles_release_once() {
    let (s, _) = setup(1);
    let api = take(&s, CheckerLifetime::Api, &Context::background(), "");
    let index = api.0.index;
    let token = tsr_core::CancellationToken::new();
    token.cancel();
    api.operation()
        .unwrap()
        .semantic_diagnostics_cancellable(s.program.files()[0].source(), &token)
        .unwrap();
    assert!(api.owner().was_canceled());
    drop(api);
    assert!(!s.state.lock().unwrap().slots[index].initialized);
    assert!(s.state.lock().unwrap().slots[index].identity.is_none());
    let first = take(&s, CheckerLifetime::Temporary, &Context::background(), "");
    let nested = first.clone();
    let index = first.0.index;
    drop(first);
    assert!(s.state.lock().unwrap().slots[index].held);
    drop(nested);
    assert!(!s.state.lock().unwrap().slots[index].held);
    drop(take(
        &s,
        CheckerLifetime::Temporary,
        &Context::background(),
        "",
    ));
}

#[test]
fn query_diagnostics_merge_globals_once_across_distinct_requests() {
    let (s, _) = setup(3);
    let ctx = context();
    assert!(!s.take_new_global_diagnostics());
    let file = s.program.files()[0].source();
    for (request, changed) in [("first", true), ("second", false)] {
        let checker = s
            .acquire(CheckerLifetime::Temporary, Some(file), &ctx, request)
            .unwrap();
        checker
            .operation()
            .unwrap()
            .semantic_diagnostics(file)
            .unwrap();
        drop(checker);
        assert_eq!(s.take_new_global_diagnostics(), changed);
        assert!(!s.take_new_global_diagnostics());
    }
    ctx.cancel();
}
