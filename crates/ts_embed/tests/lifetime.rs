use std::sync::Arc;
use ts_arena::{Counters, Counts};
use ts_checker::Error;
use ts_core::{CompilerOptions, Tristate};
use ts_embed::{FileCache, ProgramOptions, Session};
use ts_jsstring::JsString;

fn session(counters: &Counters) -> Session {
    let mut host = ts_vfs::MemoryBuilder::new(b"/", true);
    host.insert_loaded(b"/main.ts", b"const value: string = 1;".as_slice());
    Session::load(
        ProgramOptions {
            config: ts_tsoptions::ParsedCommandLine::new(
                CompilerOptions {
                    no_lib: Tristate::TRUE,
                    strict: Tristate::TRUE,
                    ..Default::default()
                },
                vec![JsString::from_bytes(b"/main.ts".as_slice())],
            ),
            host: Arc::new(host.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/lib".as_slice()),
            skip_module_resolution: false,
        },
        &mut FileCache::new(),
        counters,
    )
    .unwrap()
}

#[test]
fn repeated_create_query_drop_returns_tracked_storage_to_baseline() {
    let counters = Counters::new();
    for _ in 0..8 {
        let session = session(&counters);
        let weak_program = Arc::downgrade(session.program());
        let weak_checker = Arc::downgrade(session.checker().unwrap());
        {
            let mut operation = session.operation().unwrap();
            let file = session.program().file(b"/main.ts").unwrap();
            let diagnostics = session
                .program()
                .semantic_diagnostics_with_checker(&mut operation, file)
                .unwrap();
            assert_eq!(
                diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
                [2322]
            );
        }
        drop(session);
        assert!(weak_program.upgrade().is_none());
        assert!(weak_checker.upgrade().is_none());
        assert_eq!(counters.snapshot(), Counts::default());
    }
}

#[test]
fn retained_type_keeps_its_program_alive_and_rejects_other_sessions() {
    let counters = Counters::new();
    let first = session(&counters);
    let weak_program = Arc::downgrade(first.program());
    let retained = {
        let operation = first.operation().unwrap();
        operation
            .retain_type(operation.builtin_type("stringType").unwrap())
            .unwrap()
    };
    let second = Session::from_program(first.program().clone(), &counters);
    assert_eq!(
        second.operation().unwrap().import_type(&retained),
        Err(Error::Arena(ts_arena::Error::WrongOwner))
    );
    drop(second);
    drop(first);
    assert!(weak_program.upgrade().is_some());
    {
        let operation = retained.owner().operation().unwrap();
        let typ = operation.import_type(&retained).unwrap();
        assert_eq!(
            operation.intrinsic_type_name(typ).unwrap().as_bytes(),
            b"string"
        );
    }
    drop(retained);
    assert!(weak_program.upgrade().is_none());
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn reentry_is_an_error_and_retirement_invalidates_retained_results() {
    let counters = Counters::new();
    let session = session(&counters);
    let retained = {
        let operation = session.operation().unwrap();
        assert!(matches!(session.operation(), Err(Error::Reentry)));
        operation
            .retain_type(operation.builtin_type("numberType").unwrap())
            .unwrap()
    };
    session.retire();
    assert!(matches!(
        session.operation(),
        Err(Error::Arena(ts_arena::Error::Retired))
    ));
    assert!(matches!(
        retained.owner().operation(),
        Err(Error::Arena(ts_arena::Error::Retired))
    ));
    drop(session);
    drop(retained);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn retire_before_initialization_cannot_create_a_checker() {
    let counters = Counters::new();
    let session = session(&counters);
    session.retire();
    assert!(matches!(
        session.checker(),
        Err(Error::Arena(ts_arena::Error::Retired))
    ));
    drop(session);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn native_unwind_retires_the_session_and_preserves_the_payload() {
    let counters = Counters::new();
    let session = session(&counters);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _operation = session.operation().unwrap();
        std::panic::panic_any(42_u32);
    }));
    assert_eq!(*result.unwrap_err().downcast::<u32>().unwrap(), 42);
    assert!(matches!(
        session.operation(),
        Err(Error::Arena(ts_arena::Error::Retired))
    ));
    drop(session);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn concurrent_first_queries_share_one_checker_identity() {
    let counters = Counters::new();
    let session = Arc::new(session(&counters));
    let barrier = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    let operation = session.operation().unwrap();
                    operation.owner().identity().id()
                })
            })
            .collect();
        let ids: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(ids.iter().all(|id| *id == ids[0]));
    });
    drop(session);
    assert_eq!(counters.snapshot(), Counts::default());
}
