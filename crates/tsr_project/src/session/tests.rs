mod resources;
use super::*;
use crate::project::{ProgramUpdateKind, INFERRED_PROJECT_NAME};
use tsr_core::{ScriptKind, Tristate};
use tsr_lsproto::TextDocumentContentChangeWholeDocument;
use tsr_vfs::{
    iovfs,
    vfstest::{self, InputFile, TestFs},
};
fn js(text: &str) -> JsString {
    JsString::from_bytes(text.as_bytes())
}
fn uri(name: &str) -> DocumentUri {
    DocumentUri::from_file_name(name.as_bytes())
}
fn setup(files: &[(&str, &str)], counters: &Counters) -> (Arc<TestFs>, Arc<Session>) {
    let fs = Arc::new(vfstest::from_map(
        &files
            .iter()
            .map(|(name, text)| {
                (
                    name.as_bytes().to_vec(),
                    InputFile::Text(text.as_bytes().to_vec()),
                )
            })
            .collect(),
        false,
    ));
    let session = Session::new(
        SessionOptions::default(),
        Arc::new(iovfs::from(fs.clone(), false)),
        counters,
    );
    (fs, session)
}
fn open(session: &Session, name: &str, content: &str) -> Snapshot {
    session
        .did_open_file(uri(name), 1, js(content), LanguageKind("typescript".into()))
        .unwrap()
}
fn edit(session: &Session, name: &str, content: &str) {
    session
        .did_change_file(
            uri(name),
            2,
            vec![TextDocumentContentChangePartialOrWholeDocument {
                whole_document: Some(Box::new(TextDocumentContentChangeWholeDocument {
                    text: content.into(),
                })),
                ..Default::default()
            }],
        )
        .unwrap();
}
fn text(snapshot: &Snapshot, file: &str) -> Vec<u8> {
    snapshot
        .project_for_file(file.as_bytes())
        .unwrap()
        .program()
        .unwrap()
        .source_file(file.as_bytes())
        .unwrap()
        .bound()
        .view()
        .source_file()
        .unwrap()
        .text()
        .as_bytes()
        .to_vec()
}

#[test]
fn insensitive_project_keys_preserve_config_and_directory_spelling() {
    let (_, session) = setup(
        &[
            (
                "/Mixed/Project/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true}}"#,
            ),
            ("/Mixed/Project/main.ts", "const value = 1;"),
        ],
        &Counters::new(),
    );
    let snapshot = open(&session, "/Mixed/Project/main.ts", "const value = 1;");
    let project = snapshot
        .project_by_path(b"/mixed/project/tsconfig.json")
        .unwrap()
        .data()
        .unwrap();
    assert_eq!(project.path.as_bytes(), b"/mixed/project/tsconfig.json");
    assert_eq!(project.name.as_bytes(), b"/Mixed/Project/tsconfig.json");
    assert_eq!(project.current_directory.as_bytes(), b"/Mixed/Project");
    session.close();
}

// Pinned TestSnapshot and TestProjectProgramUpdateKind: edits clone one program,
// unrelated projects keep exact identity, and older hosts are frozen only once.
#[test]
fn real_sessions_reuse_programs_and_leave_retained_snapshots_unchanged() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let (_, session) = setup(
        &[
            ("/p1/tsconfig.json", r#"{"compilerOptions":{"noLib":true}}"#),
            ("/p1/main.ts", "export const value = 1;"),
            ("/p2/tsconfig.json", r#"{"compilerOptions":{"noLib":true}}"#),
            ("/p2/main.ts", "export const other = 2;"),
        ],
        &counters,
    );
    open(&session, "/p1/main.ts", "export const value = 1;");
    let first = open(&session, "/p2/main.ts", "export const other = 2;");
    assert_eq!(first.projects().len(), 2);
    let unchanged = first
        .project_by_path(b"/p2/tsconfig.json")
        .unwrap()
        .data()
        .unwrap()
        .clone();
    edit(&session, "/p1/main.ts", "export const value = 3;");
    assert_eq!(
        session.snapshot().unwrap().id(),
        first.id(),
        "changes wait for the request barrier"
    );
    let next = session.snapshot_for_file(&uri("/p1/main.ts")).unwrap();
    assert_eq!(next.parent_id(), first.id());
    assert_eq!(
        next.project_by_path(b"/p1/tsconfig.json")
            .unwrap()
            .data()
            .unwrap()
            .update_kind,
        ProgramUpdateKind::Cloned
    );
    assert!(Arc::ptr_eq(
        &unchanged,
        next.project_by_path(b"/p2/tsconfig.json")
            .unwrap()
            .data()
            .unwrap()
    ));
    assert_eq!(text(&first, "/p1/main.ts"), b"export const value = 1;");
    assert_eq!(text(&next, "/p1/main.ts"), b"export const value = 3;");
    assert_eq!(session.program_counter().len(), 3);
    session.close();
    assert!(matches!(session.snapshot(), Err(Error::Closed)));
    assert_eq!(text(&first, "/p2/main.ts"), b"export const other = 2;");
    drop(unchanged);
    drop(first);
    drop(next);
    assert!(session.parse_cache().is_empty());
    assert!(session.program_counter().is_empty());
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn a_rebuilt_program_releases_its_previous_filesystem_root() {
    let (_, session) = setup(
        &[("/tsconfig.json", r#"{"compilerOptions":{"noLib":true}}"#)],
        &Counters::new(),
    );
    let first = open(&session, "/main.ts", "export const x = 1;");
    let old_fs = Arc::downgrade(first.filesystem().unwrap());
    edit(&session, "/main.ts", "export const x = 2;");
    let second = session.snapshot_for_file(&uri("/main.ts")).unwrap();
    drop(first);
    assert!(
        old_fs.upgrade().is_none(),
        "the new compiler host must not retain its construction builder"
    );
    assert_eq!(text(&second, "/main.ts"), b"export const x = 2;");
}

#[test]
fn deleting_a_watched_directory_removes_its_imported_files() {
    let main = "import { value } from './dir/value'; export { value };";
    let (fs, session) = setup(
        &[
            (
                "/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true},"files":["main.ts"]}"#,
            ),
            ("/main.ts", main),
            ("/dir/value.ts", "export const value = 1;"),
        ],
        &Counters::new(),
    );
    let first = open(&session, "/main.ts", main);
    assert!(first.project().contains_file(b"/dir/value.ts"));
    fs.remove(b"dir").unwrap();
    session
        .enqueue(FileChange::new(FileChangeKind::WatchDelete, uri("/dir")))
        .unwrap();
    let next = session.snapshot_for_file(&uri("/main.ts")).unwrap();
    assert!(!next.project().contains_file(b"/dir/value.ts"));
    assert_eq!(text(&first, "/dir/value.ts"), b"export const value = 1;");
}

#[test]
fn retained_closed_projects_keep_watch_changes_until_requested() {
    let (fs, session) = setup(
        &[
            ("/a/tsconfig.json", r#"{"compilerOptions":{"noLib":true}}"#),
            ("/a/main.ts", "const first = 1;"),
            ("/a/other.ts", "const other = 2;"),
            ("/b/tsconfig.json", r#"{"compilerOptions":{"noLib":true}}"#),
            ("/b/main.ts", "const second = 2;"),
        ],
        &Counters::new(),
    );
    open(&session, "/a/main.ts", "const first = 1;");
    let first = open(&session, "/b/main.ts", "const second = 2;");
    session.did_close_file(uri("/a/main.ts")).unwrap();
    session.flush(None).unwrap();

    // Two distinct updates are consumed without selecting the closed project.
    // Neither may be forgotten, or reduced to a clone of just the last file.
    for (path, updated) in [
        ("/a/main.ts", "const first = 99;"),
        ("/a/other.ts", "const other = 100;"),
    ] {
        fs.write_file(&path.as_bytes()[1..], updated.as_bytes(), 0o644)
            .unwrap();
        session
            .enqueue(FileChange::new(FileChangeKind::WatchChange, uri(path)))
            .unwrap();
        let pending = session.flush(None).unwrap();
        assert!(
            pending
                .project_by_path(b"/a/tsconfig.json")
                .unwrap()
                .data()
                .unwrap()
                .dirty
        );
        assert_eq!(text(&pending, "/a/main.ts"), b"const first = 1;");
        assert_eq!(text(&pending, "/a/other.ts"), b"const other = 2;");
    }
    let next = session.snapshot_for_file(&uri("/a/main.ts")).unwrap();
    assert_eq!(text(&next, "/a/main.ts"), b"const first = 99;");
    assert_eq!(text(&next, "/a/other.ts"), b"const other = 100;");
    assert!(
        !next
            .project_by_path(b"/a/tsconfig.json")
            .unwrap()
            .data()
            .unwrap()
            .dirty
    );
    assert_eq!(text(&first, "/a/main.ts"), b"const first = 1;");
    assert_eq!(text(&first, "/a/other.ts"), b"const other = 2;");
    assert!(Arc::ptr_eq(
        first
            .project_by_path(b"/b/tsconfig.json")
            .unwrap()
            .data()
            .unwrap(),
        next.project_by_path(b"/b/tsconfig.json")
            .unwrap()
            .data()
            .unwrap(),
    ));
}

#[test]
fn retained_closed_projects_keep_config_changes_until_requested() {
    let (fs, session) = setup(
        &[
            ("/a/tsconfig.json", r#"{"compilerOptions":{"noLib":true}}"#),
            ("/a/main.ts", "const first = 1;"),
            ("/b/tsconfig.json", r#"{"compilerOptions":{"noLib":true}}"#),
            ("/b/main.ts", "const second = 2;"),
        ],
        &Counters::new(),
    );
    open(&session, "/a/main.ts", "const first = 1;");
    let first = open(&session, "/b/main.ts", "const second = 2;");
    session.did_close_file(uri("/a/main.ts")).unwrap();
    session.flush(None).unwrap();
    fs.write_file(
        b"a/tsconfig.json",
        br#"{"compilerOptions":{"noLib":true,"strict":true}}"#,
        0o644,
    )
    .unwrap();
    session
        .enqueue(FileChange::new(
            FileChangeKind::WatchChange,
            uri("/a/tsconfig.json"),
        ))
        .unwrap();
    session.flush(None).unwrap();
    let next = session.snapshot_for_file(&uri("/a/main.ts")).unwrap();
    assert!(next
        .project_for_file(b"/a/main.ts")
        .unwrap()
        .program()
        .unwrap()
        .options()
        .strict
        .is_true());
    assert!(!first
        .project_for_file(b"/a/main.ts")
        .unwrap()
        .program()
        .unwrap()
        .options()
        .strict
        .is_true());
}

#[test]
fn checker_panic_retires_only_the_affected_real_project() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let (_, session) = setup(
        &[
            ("/a/tsconfig.json", r#"{"compilerOptions":{"noLib":true}}"#),
            ("/b/tsconfig.json", r#"{"compilerOptions":{"noLib":true}}"#),
        ],
        &counters,
    );
    open(&session, "/a/main.ts", "export const a = 1;");
    let snapshot = open(&session, "/b/main.ts", "export const b = 2;");
    let a = snapshot.project_for_file(b"/a/main.ts").unwrap();
    let b = snapshot.project_for_file(b"/b/main.ts").unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let checkout = a.pool().acquire(crate::CheckerSlot::Query(0)).unwrap();
        let _operation = checkout.operation().unwrap();
        panic!("request failure");
    }));
    assert!(panic.is_err());
    assert!(a.pool().acquire(crate::CheckerSlot::Query(0)).is_err());
    {
        let checkout = b.pool().acquire(crate::CheckerSlot::Query(0)).unwrap();
        let _operation = checkout.operation().unwrap();
    }
    session.close();
    drop(snapshot);
    assert_eq!(counters.snapshot(), baseline);
}

// Pinned TestConfigFileChanges and TestProjectProgramUpdateKind: config changes
// replace a project's command line; inferred options never mutate old programs.
#[test]
fn config_changes_and_inferred_options_reselect_and_rebuild() {
    let counters = Counters::new();
    let (fs, session) = setup(&[("/src/main.ts", "export const x = 1;")], &counters);
    session
        .set_inferred_options(CompilerOptions {
            no_lib: Tristate::TRUE,
            ..Default::default()
        })
        .unwrap();
    let first = open(&session, "/src/main.ts", "export const x = 1;");
    assert!(first.project_by_path(INFERRED_PROJECT_NAME).is_some());
    session
        .set_inferred_options(CompilerOptions {
            no_lib: Tristate::TRUE,
            strict: Tristate::TRUE,
            ..Default::default()
        })
        .unwrap();
    let second = session.snapshot_for_file(&uri("/src/main.ts")).unwrap();
    assert!(second
        .project()
        .program()
        .unwrap()
        .options()
        .strict
        .is_true());
    assert!(!first
        .project()
        .program()
        .unwrap()
        .options()
        .strict
        .is_true());
    fs.write_file(
        b"src/tsconfig.json",
        br#"{"compilerOptions":{"noLib":true},"files":["main.ts"]}"#,
        0o644,
    )
    .unwrap();
    session
        .enqueue(FileChange::new(
            FileChangeKind::WatchCreate,
            uri("/src/tsconfig.json"),
        ))
        .unwrap();
    let third = session.snapshot_for_file(&uri("/src/main.ts")).unwrap();
    assert!(third.project_by_path(INFERRED_PROJECT_NAME).is_none());
    assert!(third.project_by_path(b"/src/tsconfig.json").is_some());
    fs.write_file(
        b"src/tsconfig.json",
        br#"{"compilerOptions":{"noLib":true},"files":[]}"#,
        0o644,
    )
    .unwrap();
    session
        .enqueue(FileChange::new(
            FileChangeKind::WatchChange,
            uri("/src/tsconfig.json"),
        ))
        .unwrap();
    let fourth = session.snapshot_for_file(&uri("/src/main.ts")).unwrap();
    assert!(fourth.project_by_path(INFERRED_PROJECT_NAME).is_some());
    assert!(fourth
        .project_by_path(b"/src/tsconfig.json")
        .unwrap()
        .data()
        .unwrap()
        .command_line
        .root_file_names
        .is_empty());
}

// Pinned TestSnapshot disk cache cleanup: closing one project drops only its
// files; retained snapshots and the other project's imported file remain usable.
#[test]
fn closing_projects_releases_extended_configs_and_disk_cache() {
    let counters = Counters::new();
    let (_, session) = setup(
        &[
            ("/shared.json", r#"{"compilerOptions":{"noLib":true}}"#),
            ("/a/tsconfig.json", r#"{"extends":"../shared.json"}"#),
            ("/a/main.ts", "import { x } from './dep'; export { x };"),
            ("/a/dep.ts", "export const x = 1;"),
            ("/b/tsconfig.json", r#"{"compilerOptions":{"noLib":true}}"#),
            ("/b/main.ts", "export const y = 2;"),
        ],
        &counters,
    );
    open(
        &session,
        "/a/main.ts",
        "import { x } from './dep'; export { x };",
    );
    let first = open(&session, "/b/main.ts", "export const y = 2;");
    assert_eq!(session.extended_config_cache().len(), 1);
    session.did_close_file(uri("/a/main.ts")).unwrap();
    session
        .set_inferred_options(CompilerOptions {
            no_lib: Tristate::TRUE,
            ..Default::default()
        })
        .unwrap();
    let next = open(&session, "/other.ts", "export const z = 0;");
    assert_eq!(next.projects().len(), 2);
    assert!(
        next.filesystem().unwrap().cached_file_count()
            < first.filesystem().unwrap().cached_file_count()
    );
    assert_eq!(
        text(&first, "/a/main.ts"),
        b"import { x } from './dep'; export { x };"
    );
    drop(first);
    assert!(session.extended_config_cache().is_empty());
}

#[test]
fn overlay_language_kind_reaches_the_production_parser() {
    let counters = Counters::new();
    let (_, session) = setup(&[], &counters);
    session
        .set_inferred_options(CompilerOptions {
            no_lib: Tristate::TRUE,
            allow_non_ts_extensions: Tristate::TRUE,
            ..Default::default()
        })
        .unwrap();
    let snapshot = open(&session, "/editor.js", "const n: number = 1;");
    let file = snapshot
        .project()
        .program()
        .unwrap()
        .source_file(b"/editor.js")
        .unwrap();
    assert_eq!(
        file.bound().view().source_file().unwrap().script_kind,
        ScriptKind::TS
    );
}

#[test]
fn published_snapshots_drive_deduplicated_watches_without_holding_the_session_lock() {
    use crate::watch::{WatchClient, Watcher};
    use tsr_ipc::Context;
    struct Client {
        session: Mutex<std::sync::Weak<Session>>,
        calls: Mutex<Vec<(JsString, JsString)>>,
    }
    impl WatchClient for Client {
        fn watch_files(&self, _: &Context, id: &JsString, watcher: &Watcher) -> Result<(), String> {
            let session = self.session.lock().unwrap().upgrade().unwrap();
            let snapshot = session.snapshot().unwrap();
            assert!(snapshot.project_for_file(b"/main.ts").is_some());
            self.calls
                .lock()
                .unwrap()
                .push((id.clone(), watcher.glob_string()));
            Ok(())
        }
        fn unwatch_files(&self, _: &Context, _: &JsString) -> Result<(), String> {
            Ok(())
        }
    }
    let client = Arc::new(Client {
        session: Mutex::default(),
        calls: Mutex::default(),
    });
    let (_, session) = setup(
        &[("/tsconfig.json", r#"{"compilerOptions":{"noLib":true}}"#)],
        &Counters::new(),
    );
    let session = session.with_watch_client(client.clone());
    *client.session.lock().unwrap() = Arc::downgrade(&session);
    let first = open(&session, "/main.ts", "export const x = 1;");
    session.wait_for_background_tasks();
    assert_eq!(
        client.calls.lock().unwrap().len(),
        1,
        "root and program globs share a registration"
    );
    let first_watch = first
        .project()
        .data()
        .unwrap()
        .program_files_watch
        .id()
        .clone();
    edit(&session, "/main.ts", "export const x = 2;");
    let next = session.snapshot_for_file(&uri("/main.ts")).unwrap();
    session.wait_for_background_tasks();
    assert_eq!(
        next.project().data().unwrap().program_files_watch.id(),
        &first_watch
    );
    assert_eq!(
        client.calls.lock().unwrap().len(),
        1,
        "a text edit must not register an unchanged watch again"
    );
    assert!(session.take_watch_errors().is_empty());
    session.close();
}

fn timed_session() -> (
    Arc<TestFs>,
    Arc<Session>,
    Arc<crate::clock::manual::ManualClock>,
) {
    let clock = crate::clock::manual::ManualClock::new();
    let (fs, _) = setup(
        &[("/tsconfig.json", r#"{"compilerOptions":{"noLib":true}}"#)],
        &Counters::new(),
    );
    let session = Session::with_clock(
        SessionOptions {
            debounce_delay: std::time::Duration::from_secs(1),
            ..Default::default()
        },
        Arc::new(iovfs::from(fs.clone(), false)),
        &Counters::new(),
        Arc::new(ParseCache::new(RefCountCacheOptions::default())),
        Arc::new(ContentMappedParseCache::new(RefCountCacheOptions::default())),
        clock.clone(),
    );
    (fs, session, clock)
}

fn watch(session: &Session, name: &str, kind: u32) {
    session
        .did_change_watched_files([tsr_lsproto::FileEvent {
            uri: uri(name),
            r#type: tsr_lsproto::FileChangeType(kind),
        }])
        .unwrap();
}
fn refreshes(events: &std::sync::mpsc::Receiver<SessionEvent>) -> Vec<tsr_core::CancellationToken> {
    events
        .try_iter()
        .filter_map(|event| match event {
            SessionEvent::DiagnosticsRefresh { cancellation } => Some(cancellation),
            _ => None,
        })
        .collect()
}

#[test]
fn watched_diagnostics_coalesce_and_edits_and_close_cancel_pending_delivery() {
    use std::time::Duration;
    let (_, session, clock) = timed_session();
    open(&session, "/main.ts", "const x = 1;");
    let events = session.subscribe();
    watch(&session, "/dependency.ts", 2);
    clock.advance(Duration::from_millis(900));
    watch(&session, "/dependency.ts", 2);
    clock.advance(Duration::from_millis(900));
    session.wait_for_background_tasks();
    assert!(refreshes(&events).is_empty());
    clock.advance(Duration::from_millis(100));
    session.wait_for_background_tasks();
    let delivered = refreshes(&events);
    assert_eq!(delivered.len(), 1);
    assert!(!delivered[0].is_canceled());

    watch(&session, "/dependency.ts", 2);
    edit(&session, "/main.ts", "const x = 2;");
    clock.advance(Duration::from_secs(1));
    session.wait_for_background_tasks();
    assert!(refreshes(&events).is_empty());
    assert!(delivered[0].is_canceled());

    watch(&session, "/dependency.ts", 2);
    clock.advance(Duration::from_secs(1));
    session.wait_for_background_tasks();
    let queued = refreshes(&events);
    assert_eq!(queued.len(), 1);
    session.close();
    assert!(
        queued[0].is_canceled(),
        "a queued refresh must not escape shutdown"
    );
    clock.advance(Duration::from_secs(30));
    session.wait_for_background_tasks();
    assert!(refreshes(&events).is_empty());
}

#[test]
fn watched_diagnostics_filter_extensions_and_distinguish_deleted_directories() {
    use std::time::Duration;
    let (fs, session, clock) = timed_session();
    fs.mkdir_all(b"dir", 0o755).unwrap();
    fs.write_file(b"dir/dep.ts", b"export const x = 1;", 0)
        .unwrap();
    open(&session, "/main.ts", "import { x } from './dir/dep';");
    let events = session.subscribe();
    for name in ["/readme.md", "/LICENSE", "/some.dir/file", "/missing"] {
        watch(&session, name, 3);
    }
    watch(&session, "/dep.ts", 99);
    clock.advance(Duration::from_secs(1));
    session.wait_for_background_tasks();
    assert!(refreshes(&events).is_empty());
    for (name, kind) in [
        ("/dir/", 3),
        ("/node_modules/pkg", 3),
        ("/dir", 1),
        ("/dep.json", 2),
    ] {
        watch(&session, name, kind);
        clock.advance(Duration::from_secs(1));
        session.wait_for_background_tasks();
        assert_eq!(refreshes(&events).len(), 1, "{name}: {kind}");
    }
    assert!(session.take_background_errors().is_empty());
}

// The native close/config debounce and getSnapshot cancellation barrier. No
// shortened wall-clock delays: the same callbacks run under a controlled clock.
#[test]
fn config_notifications_coalesce_and_a_request_cancels_the_old_timer() {
    use std::time::Duration;
    let (fs, session, clock) = timed_session();
    let first = open(&session, "/main.ts", "const x = 1;");
    let change = || FileChange::new(FileChangeKind::WatchChange, uri("/tsconfig.json"));
    fs.write_file(
        b"tsconfig.json",
        br#"{"compilerOptions":{"noLib":true,"strict":true}}"#,
        0,
    )
    .unwrap();
    session.enqueue(change()).unwrap();
    clock.advance(Duration::from_millis(900));
    session.enqueue(change()).unwrap();
    clock.advance(Duration::from_millis(900));
    session.wait_for_background_tasks();
    assert_eq!(session.snapshot().unwrap().id(), first.id());
    clock.advance(Duration::from_millis(100));
    session.wait_for_background_tasks();
    let next = session.snapshot().unwrap();
    assert_eq!(next.parent_id(), first.id());
    assert!(next.project().program().unwrap().options().strict.is_true());

    session.did_close_file(uri("/main.ts")).unwrap();
    let requested = session.flush(None).unwrap();
    clock.advance(Duration::from_secs(1));
    session.wait_for_background_tasks();
    assert_eq!(session.snapshot().unwrap().id(), requested.id());
    assert!(session.take_background_errors().is_empty());
}

#[test]
fn idle_timer_flushes_changes_after_thirty_seconds_from_the_last_notification() {
    use std::time::Duration;
    let (_, session, clock) = timed_session();
    let first = open(&session, "/main.ts", "const x = 1;");
    clock.advance(Duration::from_secs(29));
    edit(&session, "/main.ts", "const x = 2;");
    clock.advance(Duration::from_secs(29));
    session.wait_for_background_tasks();
    assert_eq!(session.snapshot().unwrap().id(), first.id());
    clock.advance(Duration::from_secs(1));
    session.wait_for_background_tasks();
    let next = session.snapshot().unwrap();
    assert_eq!(next.parent_id(), first.id());
    assert_eq!(text(&next, "/main.ts"), b"const x = 2;");
    assert_eq!(text(&first, "/main.ts"), b"const x = 1;");
    clock.advance(Duration::from_secs(30));
    session.wait_for_background_tasks();
    assert_eq!(
        session.snapshot().unwrap().id(),
        next.id(),
        "cleanup is one-shot"
    );
    session.did_close_file(uri("/main.ts")).unwrap();
    session.close();
    clock.advance(Duration::from_secs(30));
    session.wait_for_background_tasks();
    assert!(matches!(session.snapshot(), Err(Error::Closed)));
    assert!(session.take_background_errors().is_empty());
}

#[test]
fn inferred_options_response_is_a_snapshot_barrier() {
    let (_, session) = setup(&[], &Counters::new());
    session
        .set_inferred_options(CompilerOptions {
            no_lib: Tristate::TRUE,
            ..Default::default()
        })
        .unwrap();
    let first = open(&session, "/main.ts", "const x = 1;");
    session
        .set_inferred_options(CompilerOptions {
            no_lib: Tristate::TRUE,
            strict: Tristate::TRUE,
            ..Default::default()
        })
        .unwrap();
    let next = session.snapshot().unwrap();
    assert_ne!(first.id(), next.id());
    assert!(next.project().program().unwrap().options().strict.is_true());
    assert!(!first
        .project()
        .program()
        .unwrap()
        .options()
        .strict
        .is_true());
}

// source: tsc/internal/project/customconfigfilename_test.go:TestCustomConfigFileName
#[test]
fn custom_config_preferences_reselect_open_files_and_remove_inferred_roots() {
    let (_, session) = setup(
        &[
            (
                "/src/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"strict":false}}"#,
            ),
            (
                "/src/tsconfig.all.json",
                r#"{"compilerOptions":{"noLib":true,"strict":true}}"#,
            ),
            ("/src/main.ts", "const x = 1;"),
        ],
        &Counters::new(),
    );
    let original = open(&session, "/src/main.ts", "const x = 1;");
    for (custom, expected, strict) in [
        ("tsconfig.all.json", "/src/tsconfig.all.json", true),
        ("", "/src/tsconfig.json", false),
        ("missing.json", "/src/tsconfig.json", false),
    ] {
        session.set_custom_config_file_name(js(custom)).unwrap();
        let snapshot = session.snapshot_for_file(&uri("/src/main.ts")).unwrap();
        let project = snapshot.project_for_file(b"/src/main.ts").unwrap();
        assert_eq!(project.data().unwrap().name, js(expected));
        assert_eq!(
            project.program().unwrap().options().strict.is_true(),
            strict
        );
    }
    assert!(!original
        .project()
        .program()
        .unwrap()
        .options()
        .strict
        .is_true());
    let (_, other) = setup(
        &[
            (
                "/src/tsconfig.all.json",
                r#"{"compilerOptions":{"noLib":true},"include":["./**/*"]}"#,
            ),
            ("/src/main.ts", "const x = 1;"),
        ],
        &Counters::new(),
    );
    let first = open(&other, "/src/main.ts", "const x = 1;");
    assert!(first.project_by_path(INFERRED_PROJECT_NAME).is_some());
    other
        .set_custom_config_file_name(js("tsconfig.all.json"))
        .unwrap();
    let next = other.snapshot_for_file(&uri("/src/main.ts")).unwrap();
    assert!(next.project_by_path(INFERRED_PROJECT_NAME).is_none());
    assert_eq!(next.projects().len(), 1);
    assert!(first.project_by_path(INFERRED_PROJECT_NAME).is_some());
}

// source: tsc/internal/project/untitled_test.go:TestUntitledFileInInferredProject
// source: tsc/internal/project/project_test.go:TestDisplayName
// References and returned URI conversion are exercised with the L3 service.
#[test]
fn untitled_overlays_form_an_inferred_program_and_project_names_follow_go() {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/home/projects", true);
    fs.insert_loaded(
        b"/home/projects/sub/tsconfig.json",
        br#"{"compilerOptions":{"noLib":true}}"#.as_slice(),
    );
    fs.insert_loaded(b"/home/projects/sub/main.ts", b"const y = 1;".as_slice());
    let session = Session::new(
        SessionOptions {
            current_directory: js("/home/projects"),
            ..Default::default()
        },
        Arc::new(fs.finish()),
        &Counters::new(),
    );
    session
        .set_inferred_options(CompilerOptions {
            no_lib: Tristate::TRUE,
            allow_non_ts_extensions: Tristate::TRUE,
            ..Default::default()
        })
        .unwrap();
    let first = DocumentUri("untitled:Untitled-1".into());
    let second = DocumentUri("untitled:Untitled-2".into());
    session
        .did_open_file(
            first.clone(),
            1,
            js("x\n\n"),
            LanguageKind("typescript".into()),
        )
        .unwrap();
    let snapshot = session
        .did_open_file(
            second.clone(),
            1,
            js("let x = 42;\nx;"),
            LanguageKind("typescript".into()),
        )
        .unwrap();
    let project = snapshot
        .project_for_file(second.file_name().as_bytes())
        .unwrap();
    assert_eq!(project.program().unwrap().files().len(), 2);
    assert!(project
        .program()
        .unwrap()
        .source_file(first.file_name().as_bytes())
        .is_some());
    assert_eq!(
        project
            .program()
            .unwrap()
            .source_file(second.file_name().as_bytes())
            .unwrap()
            .bound()
            .view()
            .source_file()
            .unwrap()
            .text()
            .as_bytes(),
        b"let x = 42;\nx;"
    );
    assert_eq!(project.display_name(b"/home"), Some(js("projects")));
    let configured = open(&session, "/home/projects/sub/main.ts", "const y = 1;");
    let project = configured
        .project_for_file(b"/home/projects/sub/main.ts")
        .unwrap();
    assert_eq!(
        project.display_name(b"/home/projects"),
        Some(js("sub/tsconfig.json"))
    );
    assert_eq!(
        project.display_name(b"/home/projects/sub"),
        Some(js("tsconfig.json"))
    );
}

#[test]
fn unopened_dependency_uses_its_containing_project_without_inferred_options() {
    let (_, session) = setup(
        &[
            (
                "/src/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true,"strict":true},"files":["main.ts"]}"#,
            ),
            ("/src/main.ts", "import {value} from 'pkg'; value;"),
            (
                "/src/node_modules/pkg/index.d.ts",
                "export declare const value: number;",
            ),
        ],
        &Counters::new(),
    );
    let first = open(
        &session,
        "/src/main.ts",
        "import {value} from 'pkg'; value;",
    );
    let next = session
        .snapshot_for_file(&uri("/src/node_modules/pkg/index.d.ts"))
        .unwrap();
    assert_eq!(first.id(), next.id());
    assert!(next.project_by_path(INFERRED_PROJECT_NAME).is_none());
    let project = next
        .project_for_file(b"/src/node_modules/pkg/index.d.ts")
        .unwrap();
    assert_eq!(project.data().unwrap().name, js("/src/tsconfig.json"));
    assert!(project.program().unwrap().options().no_lib.is_true());
    assert!(project
        .program()
        .unwrap()
        .source_file(b"/src/main.ts")
        .is_some());
}

#[test]
fn auxiliary_package_changes_retire_only_the_current_completion_cache() {
    let (_, session) = setup(
        &[
            (
                "/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true},"files":["main.ts"]}"#,
            ),
            ("/main.ts", "export {}"),
            ("/node_modules/pkg/index.d.ts", "export const Before = 1;"),
        ],
        &Counters::new(),
    );
    let old = open(&session, "/main.ts", "export {}");
    let project = old.project_for_file(b"/main.ts").unwrap();
    let program = project.program().unwrap().clone();
    let cache = project.auto_import_cache().unwrap();
    let mut registry = tsr_autoimport::Registry::default();
    registry
        .dependencies
        .files
        .insert(js("/node_modules/pkg/index.d.ts"));
    cache.publish(registry);
    session
        .did_change_watched_files([tsr_lsproto::FileEvent {
            uri: uri("/node_modules/pkg/index.d.ts"),
            r#type: tsr_lsproto::FileChangeType::CHANGED,
        }])
        .unwrap();
    let current = session.snapshot_for_file(&uri("/main.ts")).unwrap();
    let project = current.project_for_file(b"/main.ts").unwrap();
    assert!(Arc::ptr_eq(&program, project.program().unwrap()));
    assert!(cache.is_prepared(), "the retained snapshot was mutated");
    assert!(!project.auto_import_cache().unwrap().is_prepared());
    session.close();
}

#[test]
fn consumed_config_preference_does_not_block_clean_program_inclusion_reuse() {
    let (_, session) = setup(
        &[
            (
                "/src/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true},"files":["main.ts"]}"#,
            ),
            (
                "/src/custom.json",
                r#"{"compilerOptions":{"noLib":true,"strict":true},"files":["main.ts"]}"#,
            ),
            ("/src/main.ts", "const value = 1;"),
        ],
        &Counters::new(),
    );
    let first = session.snapshot_for_file(&uri("/src/main.ts")).unwrap();
    session
        .set_custom_config_file_name(js("custom.json"))
        .unwrap();
    // Like an update timer, consume newConfig without requesting this closed
    // document. Go retains its clean project and later GetDefaultProject may
    // select it by program inclusion, independent of the new config filename.
    let updated = session.flush(None).unwrap();
    assert_ne!(first.id(), updated.id());
    let reused = session.snapshot_for_file(&uri("/src/main.ts")).unwrap();
    assert_eq!(updated.id(), reused.id());
    assert_eq!(
        reused
            .project_for_file(b"/src/main.ts")
            .unwrap()
            .data()
            .unwrap()
            .name,
        js("/src/tsconfig.json")
    );
}

#[test]
fn auto_import_watches_cover_open_projects_with_one_unescaped_directory_set() {
    struct Client;
    impl crate::watch::WatchClient for Client {
        fn watch_files(
            &self,
            _: &tsr_ipc::Context,
            _: &JsString,
            _: &crate::watch::Watcher,
        ) -> Result<(), String> {
            Ok(())
        }
        fn unwatch_files(&self, _: &tsr_ipc::Context, _: &JsString) -> Result<(), String> {
            Ok(())
        }
    }
    let (_, session) = setup(
        &[
            (
                "/a[one]/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true},"files":["main.ts"]}"#,
            ),
            ("/a[one]/main.ts", ""),
            (
                "/a[one]/node_modules/unused/index.d.ts",
                "export const value: number;",
            ),
            (
                "/b/tsconfig.json",
                r#"{"compilerOptions":{"noLib":true},"files":["main.ts"]}"#,
            ),
            ("/b/main.ts", ""),
            (
                "/b/node_modules/unused/index.d.ts",
                "export const other: number;",
            ),
        ],
        &Counters::new(),
    );
    let session = session.with_watch_client(Arc::new(Client));
    open(&session, "/a[one]/main.ts", "");
    open(&session, "/b/main.ts", "");
    let patterns = || {
        let watches = session.auto_import_watches.lock().unwrap();
        assert_eq!(watches.len(), 1);
        let watchers = watches.get(b"auto-import".as_slice()).unwrap().watchers();
        assert!(watchers.id.as_bytes().starts_with(b"auto-import watcher "));
        watchers
            .iter()
            .map(|w| (w.pattern.clone(), w.kind))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        patterns(),
        vec![
            (js("/a[one]/node_modules/**/*"), 7),
            (js("/b/node_modules/**/*"), 7)
        ]
    );
    session.did_close_file(uri("/a[one]/main.ts")).unwrap();
    session.snapshot_for_file(&uri("/b/main.ts")).unwrap();
    assert_eq!(patterns(), vec![(js("/b/node_modules/**/*"), 7)]);
    session.close();
}
