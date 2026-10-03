//! Native watch-session races and invalidation contracts, using the real driver.
use phase4_tsctests::{
    execute::{
        command_line,
        fswatch::{Event, EventKind},
        tsc::{ExitStatus, Watcher},
    },
    runner::TscInput,
    sys::{new_test_sys, TestSys},
};
use std::{any::Any, sync::Arc, time::Duration};
use tsr_ipc::Context;
use tsr_jsstring::JsString;
use tsr_vfs::vfstest::InputFile;

const ROOT: &str = "/home/src/workspaces/project/";
fn args(values: &[&str]) -> Vec<JsString> {
    values
        .iter()
        .map(|s| JsString::from_bytes(s.as_bytes()))
        .collect()
}
fn system(files: &[(&str, &str)]) -> Arc<TestSys> {
    new_test_sys(
        &TscInput {
            files: files
                .iter()
                .map(|(p, t)| {
                    (
                        [ROOT.as_bytes(), p.as_bytes()].concat(),
                        InputFile::Text(t.as_bytes().to_vec()),
                    )
                })
                .collect(),
            ..Default::default()
        },
        false,
    )
}
fn start(files: &[(&str, &str)]) -> (Arc<dyn Watcher>, Arc<TestSys>) {
    let sys = system(files);
    let result = command_line(
        &Context::background(),
        sys.clone(),
        &args(&["--watch", "--pretty", "false"]),
        Some(sys.clone()),
    )
    .expect("command completes");
    (result.watcher.expect("watch session"), sys)
}
fn minimal() -> (Arc<dyn Watcher>, Arc<TestSys>) {
    start(&[
        ("a.ts", "const a: number = 1;"),
        ("b.ts", "import { a } from './a'; export const b = a;"),
        ("tsconfig.json", "{}"),
    ])
}
fn write(sys: &TestSys, path: &str, text: &str) {
    sys.fs_from_file_map()
        .write_file(
            [ROOT.as_bytes(), path.as_bytes()].concat().as_slice(),
            text.as_bytes(),
        )
        .unwrap();
}
fn event(sys: &TestSys, paths: &[&str]) {
    sys.mock_watch_backend.send_events(
        &paths
            .iter()
            .map(|p| Event {
                kind: EventKind::EventUpdate,
                path: [ROOT.as_bytes(), p.as_bytes()].concat(),
            })
            .collect::<Vec<_>>(),
    );
}
fn output(sys: &TestSys) -> String {
    String::from_utf8(sys.current_write().string()).unwrap()
}
fn counts(w: &dyn Watcher) -> (usize, usize) {
    let w = (w as &dyn Any)
        .downcast_ref::<tsr_execute::tsc::watcher::CompilerWatcher>()
        .unwrap();
    (w.fast_path_builds(), w.full_builds())
}

/// source: tsc/internal/execute/tsctests/watcher_race_test.go:TestWatcherConcurrentDoCycle
#[test]
fn concurrent_cycles_with_source_writes() {
    let (w, sys) = minimal();
    std::thread::scope(|s| {
        for i in 0..8 {
            let (w, sys) = (&w, &sys);
            s.spawn(move || {
                for j in 0..10 {
                    write(sys, "a.ts", &format!("const a: number = {};", i * 10 + j));
                    w.do_cycle().expect("watch cycle completes");
                }
            });
        }
    });
}
/// source: tsc/internal/execute/tsctests/watcher_race_test.go:TestWatcherDoCycleWithConcurrentStateReads
#[test]
fn concurrent_cycles_and_state_reads() {
    let (w, sys) = minimal();
    std::thread::scope(|s| {
        for i in 0..4 {
            let (w, sys) = (&w, &sys);
            s.spawn(move || {
                for j in 0..15 {
                    write(sys, "a.ts", &format!("const a: number = {};", i * 15 + j));
                    w.do_cycle().expect("watch cycle completes");
                }
            });
        }
        for _ in 0..8 {
            let w = &w;
            s.spawn(move || {
                for _ in 0..50 {
                    for _ in 0..4 {
                        w.do_cycle().expect("watch cycle completes");
                    }
                }
            });
        }
    });
}
/// source: tsc/internal/execute/tsctests/watcher_race_test.go:TestWatcherConcurrentFileChangesAndDoCycle
#[test]
fn create_delete_and_cycle_concurrently() {
    let (w, sys) = minimal();
    std::thread::scope(|s| {
        for i in 0..4 {
            let sys = &sys;
            s.spawn(move || {
                for j in 0..20 {
                    write(
                        sys,
                        &format!("gen_{i}_{j}.ts"),
                        &format!("export const x{i}_{j} = {j};"),
                    );
                }
            });
        }
        let sys = &sys;
        s.spawn(move || {
            for j in 0..20 {
                let _ = sys
                    .fs_from_file_map()
                    .remove(format!("{ROOT}gen_0_{j}.ts").as_bytes());
            }
        });
        for _ in 0..4 {
            let w = &w;
            s.spawn(move || {
                for _ in 0..10 {
                    w.do_cycle().expect("watch cycle completes");
                }
            });
        }
    });
}
/// source: tsc/internal/execute/tsctests/watcher_race_test.go:TestWatcherRapidConfigChanges
#[test]
fn rapid_config_and_source_edits() {
    let (w, sys) = minimal();
    const CONFIGS: [&str; 4] = [
        "{}",
        r#"{"compilerOptions":{"strict":true}}"#,
        r#"{"compilerOptions":{"target":"ES2020"}}"#,
        r#"{"compilerOptions":{"noEmit":true}}"#,
    ];
    std::thread::scope(|s| {
        for i in 0..3 {
            let (w, sys) = (&w, &sys);
            s.spawn(move || {
                for j in 0..10 {
                    write(sys, "tsconfig.json", CONFIGS[(i + j) % 4]);
                    w.do_cycle().expect("watch cycle completes");
                }
            });
        }
        for i in 0..2 {
            let (w, sys) = (&w, &sys);
            s.spawn(move || {
                for j in 0..15 {
                    write(sys, "a.ts", &format!("const a: number = {};", i * 15 + j));
                    w.do_cycle().expect("watch cycle completes");
                }
            });
        }
        for _ in 0..4 {
            let w = &w;
            s.spawn(move || {
                for _ in 0..30 {
                    w.do_cycle().expect("watch cycle completes");
                    w.do_cycle().expect("watch cycle completes");
                }
            });
        }
    });
}
/// source: tsc/internal/execute/tsctests/watcher_race_test.go:TestWatcherConcurrentDoCycleNoChanges
#[test]
fn concurrent_cycles_without_changes() {
    let (w, _sys) = minimal();
    std::thread::scope(|s| {
        for _ in 0..16 {
            let w = &w;
            s.spawn(move || {
                for _ in 0..50 {
                    w.do_cycle().expect("watch cycle completes");
                }
            });
        }
    });
}
/// source: tsc/internal/execute/tsctests/watcher_race_test.go:TestWatcherAlternatingModifyAndDoCycle
#[test]
fn alternating_writes_and_cycles() {
    let (w, sys) = minimal();
    std::thread::scope(|s| {
        let sys = &sys;
        s.spawn(move || {
            for j in 0..100 {
                write(sys, "a.ts", &format!("const a: number = {j};"));
            }
        });
        for count in [25, 25, 25, 25, 100, 100, 100, 100] {
            let w = &w;
            s.spawn(move || {
                for _ in 0..count {
                    w.do_cycle().expect("watch cycle completes");
                }
            });
        }
    });
}
/// source: tsc/internal/execute/tsctests/watcher_race_test.go:TestBuildWatchStopsWhenContextIsCancelled
#[test]
fn cancelled_build_watch_returns_without_waiting_for_interval() {
    let sys = system(&[
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"composite":true},"files":["index.ts"]}"#,
        ),
        ("index.ts", "export const x = 1;"),
    ]);
    let context = Context::background().with_cancel();
    context.cancel();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        tx.send(
            command_line(
                &context,
                sys.clone(),
                &args(&["--build", "--watch", "--watchInterval", "60000"]),
                Some(sys),
            )
            .expect("command completes"),
        )
        .unwrap();
    });
    let result = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("cancelled watch stops promptly");
    worker.join().unwrap();
    assert_eq!(result.status, ExitStatus::Success);
    assert!(result.watcher.is_some());
}
/// source: tsc/internal/execute/tsctests/watcher_race_test.go:TestWatcherStartsFromExistingBuildInfo
#[test]
fn watch_reads_existing_build_info() {
    let sys = system(&[
        ("index.ts", "export const x: number = 1;"),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"composite":true},"files":["index.ts"]}"#,
        ),
    ]);
    let r = command_line(
        &Context::background(),
        sys.clone(),
        &args(&["-p", "tsconfig.json", "--pretty", "false"]),
        Some(sys.clone()),
    )
    .expect("command completes");
    assert_eq!(r.status, ExitStatus::Success);
    assert!(sys
        .fs_from_file_map()
        .file_exists(format!("{ROOT}tsconfig.tsbuildinfo").as_bytes()));
    sys.clear_output();
    let r = command_line(
        &Context::background(),
        sys.clone(),
        &args(&["--watch", "--noEmit", "--pretty", "false"]),
        Some(sys),
    )
    .expect("command completes");
    assert_eq!(r.status, ExitStatus::Success);
    assert!(r.watcher.is_some());
}
/// source: tsc/internal/execute/tsctests/watcher_race_test.go:TestWatcherRebuildsWhenJsxImportSourcePragmaChanges
#[test]
fn changed_jsx_pragma_resolves_the_new_runtime() {
    let (w, sys) = start(&[
        (
            "index.tsx",
            "/** @jsxImportSource foo */\nexport const x = <div />;",
        ),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"jsx":"react-jsx","module":"esnext","moduleResolution":"bundler","noEmit":true},"files":["index.tsx"]}"#,
        ),
    ]);
    sys.current_write().reset();
    write(
        &sys,
        "index.tsx",
        "/** @jsxImportSource bar */\nexport const x = <div />;",
    );
    event(&sys, &["index.tsx"]);
    w.do_cycle().expect("watch cycle completes");
    let text = output(&sys);
    assert!(text.contains("bar/jsx-runtime"), "{text}");
    assert!(!text.contains("foo/jsx-runtime"), "{text}");
}
/// source: tsc/internal/execute/tsctests/watcher_race_test.go:TestWatcherUpdateProgramFastPath
#[test]
fn body_only_edits_reuse_program_and_import_changes_rebuild() {
    let (w, sys) = start(&[
        ("a.ts", "export const a: number = 1;"),
        ("b.ts", "import { a } from './a'; export const b = a;"),
        ("c.ts", "export const c: number = 10;"),
        ("tsconfig.json", "{}"),
    ]);
    for (path, text, fast, errors) in [
        ("a.ts", "export const a: number = 2;", true, false),
        (
            "a.ts",
            "export const a: number = 'not a number';",
            true,
            true,
        ),
        ("a.ts", "export const a: number = 3;", true, false),
        (
            "b.ts",
            "import { c } from './c'; export const b = c;",
            false,
            false,
        ),
        (
            "b.ts",
            "import { c } from './c'; export const b = c + 1;",
            true,
            false,
        ),
    ] {
        let before = counts(w.as_ref());
        sys.current_write().reset();
        write(&sys, path, text);
        event(&sys, &[path]);
        w.do_cycle().expect("watch cycle completes");
        let out = output(&sys);
        assert_eq!(out.contains("Found 0 errors"), !errors, "{out}");
        assert_eq!(
            counts(w.as_ref()),
            (before.0 + usize::from(fast), before.1 + usize::from(!fast))
        );
    }
}
fn missing_dependency() -> (Arc<dyn Watcher>, Arc<TestSys>) {
    start(&[
        (
            "index.ts",
            "import { dep } from './dep'; export const x = dep;",
        ),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"noLib":true,"moduleResolution":"bundler","module":"esnext","outDir":"out"},"files":["index.ts"]}"#,
        ),
    ])
}
/// source: tsc/internal/execute/tsctests/watcher_race_test.go:TestWatcherOverflowForcesFullRebuild
#[test]
fn overflow_rediscovers_previously_missing_dependency() {
    let (w, sys) = missing_dependency();
    let dep = format!("{ROOT}out/dep.js");
    assert!(!sys.fs_from_file_map().file_exists(dep.as_bytes()));
    write(&sys, "dep.ts", "export const dep: number = 1;");
    let before = counts(w.as_ref());
    sys.mock_watch_backend.send_overflow();
    w.do_cycle().expect("watch cycle completes");
    assert_eq!(counts(w.as_ref()).1, before.1 + 1);
    assert!(sys.fs_from_file_map().file_exists(dep.as_bytes()));
}
/// source: tsc/internal/execute/tsctests/watcher_race_test.go:TestWatcherNonSourceDependencyForcesFullRebuild
#[test]
fn non_source_dependency_in_edit_batch_forces_rebuild() {
    let (w, sys) = missing_dependency();
    let dep = format!("{ROOT}out/dep.js");
    assert!(!sys.fs_from_file_map().file_exists(dep.as_bytes()));
    write(
        &sys,
        "index.ts",
        "import { dep } from './dep'; export const x = dep + 0;",
    );
    write(&sys, "dep.ts", "export const dep: number = 1;");
    let before = counts(w.as_ref());
    event(&sys, &["index.ts", "dep.ts"]);
    w.do_cycle().expect("watch cycle completes");
    assert_eq!(counts(w.as_ref()).1, before.1 + 1);
    assert!(sys.fs_from_file_map().file_exists(dep.as_bytes()));
}
