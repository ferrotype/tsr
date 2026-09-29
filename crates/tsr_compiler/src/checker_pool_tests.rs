//! E3's ownership scenarios against the compiler's checker pool
//! (docs/PHASE2-C6-plan.md, C6.6): shared-pool panic retirement, wrong-owner
//! rejection and release boundaries, over a program-backed pool of four
//! checkers on real pool threads. The S09 ownership lanes run them in debug,
//! release, Miri and AddressSanitizer (`data/s09/ownership-cases.json`).
use crate::{CheckedProgram, CompilerCheckerPool, FileCache, Program, ProgramOptions};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier, Mutex};
use tsr_arena::{Counters, NodeId};
use tsr_core::{CompilerOptions, Tristate};
use tsr_jsstring::JsString;
use tsr_tsoptions::ParsedCommandLine;
use tsr_vfs::MemoryBuilder;

/// Eight files in an import chain, so each of four checkers has files.
fn program(counters: &Counters) -> Arc<Program> {
    let mut host = MemoryBuilder::new(b"/", true);
    let mut roots = Vec::new();
    for index in 0..8 {
        let next = (index + 1) % 8;
        let name = format!("/f{index}.ts");
        let text = format!(
            "import {{ v{next} }} from \"./f{next}\";\nexport const v{index}: number = \"{index}\";\n"
        );
        host.insert_physical(name.as_bytes(), text.as_bytes());
        roots.push(JsString::from_bytes(name.as_bytes()));
    }
    let options = CompilerOptions {
        no_lib: Tristate::TRUE,
        ..CompilerOptions::default()
    };
    Arc::new(
        Program::load(
            ProgramOptions {
                config: ParsedCommandLine::new(options, roots),
                host: Arc::new(host.finish()),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(b"/no-default-lib".as_slice()),
                skip_module_resolution: false,
                single_threaded: Tristate::FALSE,
            },
            &mut FileCache::new(),
            counters,
        )
        .unwrap(),
    )
}

fn sources(program: &Program) -> Vec<NodeId> {
    program.files().iter().map(|file| file.source()).collect()
}

fn index_of(pool: &CompilerCheckerPool, operation: &tsr_checker::Operation<'_>) -> usize {
    pool.checkers()
        .unwrap()
        .iter()
        .position(|owner| Arc::ptr_eq(owner, operation.owner()))
        .unwrap()
}

#[test]
fn shared_pool_panic_retirement() {
    let counters = Counters::new();
    let program = program(&counters);
    let expected = CheckedProgram::new(program.clone(), &counters, None)
        .semantic_diagnostics(None)
        .unwrap();
    assert_eq!(expected.len(), 8, "every file reports its assignment error");
    let baseline = counters.snapshot();
    let pool = CompilerCheckerPool::new(program.clone(), &counters);
    assert_eq!(pool.checker_count(), 4);
    let files = sources(&program);
    let plan = pool.association_plan().unwrap().clone();
    let mut per_checker = [0; 4];
    for &checker in &plan.associations {
        per_checker[checker] += 1;
    }
    assert!(per_checker.iter().all(|&count| count > 0));
    let retained = {
        let mut operation = pool.checker_for_file_exclusive(files[0]).unwrap();
        let literal = operation.string_literal_type(b"retained result").unwrap();
        operation.retain_type(literal).unwrap()
    };
    let panicking = 1;

    // A commitment holds the generation gate while checker 1's thread
    // panics: its retirement observes the contended gate, then waits for it.
    let (contended, observed) = mpsc::channel();
    let (committed, commit) = mpsc::channel::<()>();
    let generation = pool.generation().clone();
    let holder = std::thread::spawn(move || {
        let guard = generation.enter().unwrap();
        committed.send(()).unwrap();
        observed.recv().unwrap();
        drop(guard);
    });
    commit.recv().unwrap();
    let started = Barrier::new(4);
    let visits = Mutex::new(Vec::new());
    let result = catch_unwind(AssertUnwindSafe(|| {
        pool.for_each_checker_group_do(&files, false, &|operation, position, _| {
            let checker = index_of(&pool, operation);
            let first = !visits.lock().unwrap().iter().any(|&(c, _)| c == checker);
            visits.lock().unwrap().push((checker, position));
            if !first {
                return;
            }
            started.wait();
            if checker == panicking {
                tsr_arena::observe_next_retirement_contention(contended.clone());
                operation
                    .string_literal_type(b"partially completed work")
                    .unwrap();
                panic!("injected checker operation panic");
            }
            // The others finish this file only once the generation retired;
            // their next file finds it retired and they stop.
            while pool.generation().validate().is_ok() {
                std::thread::yield_now();
            }
        })
    }));
    holder.join().unwrap();
    let payload = result.expect_err("the injected panic propagates from the group");
    assert_eq!(
        payload.downcast_ref::<&str>(),
        Some(&"injected checker operation panic")
    );
    let visits = visits.into_inner().unwrap();
    for checker in 0..4 {
        assert_eq!(
            visits.iter().filter(|&&(c, _)| c == checker).count(),
            1,
            "checker {checker} stopped at its next file"
        );
    }

    assert_eq!(pool.generation().validate(), Err(tsr_arena::Error::Retired));
    for owner in pool.checkers().unwrap() {
        assert!(matches!(
            owner.operation(),
            Err(tsr_checker::Error::Arena(tsr_arena::Error::Retired))
        ));
    }
    assert!(matches!(
        retained.owner().operation(),
        Err(tsr_checker::Error::Arena(tsr_arena::Error::Retired))
    ));
    assert!(matches!(
        pool.for_each_checker_group_do(&files, false, &|_, _, _| {}),
        Err(tsr_checker::Error::Arena(tsr_arena::Error::Retired))
    ));
    drop(retained);
    drop(pool);
    assert_eq!(counters.snapshot(), baseline);

    let fresh = CheckedProgram::new(program, &counters, None);
    assert_eq!(fresh.semantic_diagnostics(None).unwrap(), expected);
}

#[test]
fn wrong_owner_rejected() {
    let counters = Counters::new();
    let program = program(&counters);
    let pool = CompilerCheckerPool::new(program.clone(), &counters);
    let files = sources(&program);
    let plan = pool.association_plan().unwrap().clone();
    let first = files[0];
    let other = files
        .iter()
        .zip(&plan.associations)
        .find(|&(_, &checker)| checker != plan.associations[0])
        .map(|(&file, _)| file)
        .unwrap();
    let (retained, id) = {
        let mut operation = pool.checker_for_file_exclusive(first).unwrap();
        let literal = operation.string_literal_type(b"checker-local").unwrap();
        (operation.retain_type(literal).unwrap(), literal)
    };
    let operation = pool.checker_for_file_exclusive(other).unwrap();
    assert!(!Arc::ptr_eq(operation.owner(), retained.owner()));
    assert_eq!(
        operation.owner().identity().generation().id(),
        retained.owner().identity().generation().id(),
        "one pool, one generation"
    );
    assert_eq!(
        operation.import_type(&retained),
        Err(tsr_checker::Error::Arena(tsr_arena::Error::WrongOwner))
    );
    assert_eq!(
        operation.type_flags(id),
        Err(tsr_checker::Error::Arena(tsr_arena::Error::WrongOwner))
    );
    drop(operation);
    let operation = pool.checker_for_file_exclusive(first).unwrap();
    assert_eq!(operation.import_type(&retained).unwrap(), id);
}

#[test]
fn release_boundaries() {
    let counters = Counters::new();
    let program = program(&counters);
    let baseline = counters.snapshot();
    let pool = Arc::new(CompilerCheckerPool::new(program.clone(), &counters));
    let files = sources(&program);

    // One operation per checker: reentry on the holding thread is refused
    // before waiting, and a release lets another thread acquire.
    let held = pool.checker_for_file_exclusive(files[0]).unwrap();
    assert!(matches!(
        pool.checker_for_file_exclusive(files[0]),
        Err(tsr_checker::Error::Reentry)
    ));
    assert!(matches!(
        pool.checker_for_file_non_exclusive(files[0])
            .unwrap()
            .operation(),
        Err(tsr_checker::Error::Reentry)
    ));
    let acquired = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let waiter = scope.spawn(|| {
            let operation = pool.checker_for_file_exclusive(files[0]).unwrap();
            acquired.fetch_add(1, Ordering::SeqCst);
            drop(operation);
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(acquired.load(Ordering::SeqCst), 0, "held until released");
        drop(held);
        waiter.join().unwrap();
    });
    assert_eq!(acquired.load(Ordering::SeqCst), 1);

    // The final retained result, not the pool, controls its checker's
    // disposal; the pool's other checkers go with the pool.
    let retained = {
        let mut operation = pool.checker_for_file_exclusive(files[0]).unwrap();
        let literal = operation.string_literal_type(b"retained result").unwrap();
        operation.retain_type(literal).unwrap()
    };
    let owners: Vec<_> = pool
        .checkers()
        .unwrap()
        .iter()
        .map(Arc::downgrade)
        .collect();
    drop(pool);
    let live: Vec<bool> = owners
        .iter()
        .map(|owner| owner.upgrade().is_some())
        .collect();
    assert_eq!(live.iter().filter(|&&alive| alive).count(), 1);
    {
        let operation = retained.owner().operation().unwrap();
        let imported = operation.import_type(&retained).unwrap();
        assert_eq!(
            operation.type_flags(imported).unwrap(),
            tsr_checker::type_flags::STRING_LITERAL
        );
    }
    drop(retained);
    assert!(owners.iter().all(|owner| owner.upgrade().is_none()));
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn a_retirement_between_files_publishes_nothing() {
    let counters = Counters::new();
    let program = program(&counters);
    let checked = CheckedProgram::new(program.clone(), &counters, None);
    let pool = checked.compiler_checker_pool().unwrap().clone();
    let files = sources(&program);
    let plan = pool.association_plan().unwrap().clone();
    let mut per_checker = [0; 4];
    for &checker in &plan.associations {
        per_checker[checker] += 1;
    }
    let retiring = (0..4).max_by_key(|&checker| per_checker[checker]).unwrap();
    assert!(
        per_checker[retiring] > 1,
        "the retiring checker has a next file"
    );
    // Retired from outside any checker operation, without a panic, while
    // every task holds its checker: each stops at its next file and the group
    // fails instead of returning part of the program's results.
    let started = Barrier::new(4);
    let visits = Mutex::new(Vec::new());
    let result = pool.for_each_checker_group_do(&files, false, &|operation, _, _| {
        let checker = index_of(&pool, operation);
        let first = !visits.lock().unwrap().contains(&checker);
        visits.lock().unwrap().push(checker);
        if !first {
            return;
        }
        started.wait();
        if checker == retiring {
            pool.generation().retire();
        }
        while pool.generation().validate().is_ok() {
            std::thread::yield_now();
        }
    });
    assert!(matches!(
        result,
        Err(tsr_checker::Error::Arena(tsr_arena::Error::Retired))
    ));
    assert_eq!(
        visits.into_inner().unwrap().len(),
        4,
        "one file per checker"
    );
    assert!(checked.semantic_diagnostics(None).is_err());
}
