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
fn setup(files: &[(&str, &str)], counters: &Counters) -> (Arc<TestFs>, Session) {
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
    let session = Arc::new(session.with_watch_client(client.clone()));
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
