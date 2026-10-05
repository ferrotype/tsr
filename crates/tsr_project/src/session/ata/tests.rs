use super::*;
use crate::ata::{NpmError, NpmExecutor};
use crate::session::SessionOptions;
use tsr_arena::Counters;
use tsr_lsproto::{DocumentUri, LanguageKind};
use tsr_vfs::{
    iovfs,
    vfstest::{self, InputFile},
    FileSystem,
};

fn js(text: &str) -> JsString {
    JsString::from_bytes(text.as_bytes())
}
fn uri(name: &str) -> DocumentUri {
    DocumentUri::from_file_name(name.as_bytes())
}
struct Npm {
    fs: Arc<dyn FileSystem>,
    calls: Mutex<Vec<Vec<JsString>>>,
}
impl NpmExecutor for Npm {
    fn npm_install(&self, _: &Context, cwd: &[u8], args: &[JsString]) -> Result<Vec<u8>, NpmError> {
        assert_eq!(cwd, b"/cache");
        self.calls.lock().unwrap().push(args.to_vec());
        if args[2].as_bytes() == b"types-registry@latest" {
            self.fs.write_file(b"/cache/node_modules/types-registry/index.json", br#"{"entries":{"jquery":{"latest":"1.3.0"},"config":{"latest":"1.3.0"},"commander":{"latest":"1.3.0"},"node":{"latest":"1.3.0"},"ember__component":{"latest":"1.3.0"}}}"#).unwrap();
        } else {
            for arg in &args[2..args.len() - 2] {
                let name = arg
                    .as_bytes()
                    .strip_prefix(b"@types/")
                    .unwrap()
                    .strip_suffix(b"@latest")
                    .unwrap();
                self.fs
                    .write_file(
                        &[b"/cache/node_modules/@types/", name, b"/index.d.ts"].concat(),
                        b"export const value: number;",
                    )
                    .unwrap();
            }
        }
        Ok(vec![])
    }
}
fn setup(files: &[(&str, &str)]) -> (Arc<Session>, Arc<Npm>) {
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
    let fs: Arc<dyn FileSystem> = Arc::new(iovfs::from(fs, false));
    let npm = Arc::new(Npm {
        fs: fs.clone(),
        calls: Mutex::default(),
    });
    let session = Session::new(
        SessionOptions {
            current_directory: js("/project"),
            typings_location: js("/cache"),
            npm_executor: Some(npm.clone()),
            ..Default::default()
        },
        fs,
        &Counters::default(),
    );
    (session, npm)
}
fn open(session: &Session, content: &str) {
    session
        .did_open_file(
            uri("/project/app.js"),
            1,
            js(content),
            LanguageKind("javascript".into()),
        )
        .unwrap();
    session.wait_for_background_tasks();
}

#[test]
fn inferred_ata_roots_rebuild_and_resolve_imports_from_global_cache() {
    let (session, npm) = setup(&[("/project/app.js", "const x = require('jquery');")]);
    open(&session, "const x = require('jquery');");
    let snapshot = session.snapshot_for_file(&uri("/project/app.js")).unwrap();
    let data = snapshot
        .project_for_file(b"/project/app.js")
        .unwrap()
        .data()
        .unwrap();
    assert!(data
        .program
        .source_file(b"/cache/node_modules/@types/jquery/index.d.ts")
        .is_some());
    assert!(data
        .program
        .resolutions()
        .iter()
        .any(|r| r.name.as_bytes() == b"jquery"
            && r.result.resolved_file_name.as_bytes()
                == b"/cache/node_modules/@types/jquery/index.d.ts"));
    assert_eq!(
        data.command_line.root_file_names,
        vec![js("/project/app.js")]
    );
    assert_eq!(
        data.typings_watch
            .as_ref()
            .unwrap()
            .watchers()
            .iter()
            .map(crate::watch::Watcher::glob_string)
            .collect::<Vec<_>>(),
        vec![js("/project/**/*")]
    );
    session.wait_for_background_tasks();
    assert_eq!(npm.calls.lock().unwrap().len(), 2);
    session.close();
}

#[test]
fn configured_ata_follows_type_acquisition_and_global_disable_reenable() {
    let (session, npm) = setup(&[
        ("/project/app.js", ""),
        (
            "/project/jsconfig.json",
            r#"{"compilerOptions":{"noLib":true},"typeAcquisition":{"enable":true},"files":["app.js"]}"#,
        ),
        (
            "/project/package.json",
            r#"{"dependencies":{"jquery":"*"}}"#,
        ),
    ]);
    let events = session.subscribe();
    session
        .set_disable_automatic_type_acquisition(true)
        .unwrap();
    open(&session, "");
    assert!(npm.calls.lock().unwrap().is_empty());
    let _ = events.try_iter().collect::<Vec<_>>();
    session
        .set_disable_automatic_type_acquisition(false)
        .unwrap();
    session.wait_for_background_tasks();
    assert!(events
        .try_iter()
        .any(|event| matches!(event, SessionEvent::DiagnosticsRefresh { .. })));
    let snapshot = session.snapshot_for_file(&uri("/project/app.js")).unwrap();
    assert!(snapshot
        .project_for_file(b"/project/app.js")
        .unwrap()
        .contains_file(b"/cache/node_modules/@types/jquery/index.d.ts"));
    session
        .set_disable_automatic_type_acquisition(true)
        .unwrap();
    assert!(!session
        .snapshot()
        .unwrap()
        .project_for_file(b"/project/app.js")
        .unwrap()
        .contains_file(b"/cache/node_modules/@types/jquery/index.d.ts"));
    session.close();
}

#[test]
fn relative_local_import_does_not_acquire_types() {
    let (session, npm) = setup(&[
        ("/project/app.js", "const c = require('./config');"),
        ("/project/config.js", "export let x = 1;"),
    ]);
    open(&session, "const c = require('./config');");
    assert_eq!(npm.calls.lock().unwrap().len(), 1);
    assert!(session
        .snapshot()
        .unwrap()
        .project_for_file(b"/project/app.js")
        .unwrap()
        .program()
        .unwrap()
        .unresolved_imports()
        .iter()
        .all(|name| name.as_bytes() != b"config" && name.as_bytes() != b"./config"));
    session.close();
}

struct BlockingNpm(std::sync::mpsc::Sender<()>);
impl NpmExecutor for BlockingNpm {
    fn npm_install(
        &self,
        context: &Context,
        _: &[u8],
        _: &[JsString],
    ) -> Result<Vec<u8>, NpmError> {
        self.0.send(()).unwrap();
        while context.err().is_none() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Err(NpmError {
            message: "canceled".into(),
            output: vec![],
        })
    }
}
#[test]
fn dropping_session_cancels_and_joins_running_ata() {
    let fs: Arc<dyn FileSystem> = Arc::new(tsr_vfs::MemoryBuilder::new(b"/", false).finish());
    let (started, receive) = std::sync::mpsc::channel();
    let session = Session::new(
        SessionOptions {
            typings_location: js("/cache"),
            npm_executor: Some(Arc::new(BlockingNpm(started))),
            ..Default::default()
        },
        fs,
        &Counters::default(),
    );
    session
        .did_open_file(uri("/app.js"), 1, js(""), LanguageKind("javascript".into()))
        .unwrap();
    receive
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let start = std::time::Instant::now();
    drop(session);
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn close_cancels_before_waiting_for_update_and_finishes_ata_progress() {
    let fs = Arc::new(vfstest::from_map(&BTreeMap::new(), false));
    let fs: Arc<dyn FileSystem> = Arc::new(iovfs::from(fs, false));
    let (started, receive) = std::sync::mpsc::channel();
    let session = Session::new(
        SessionOptions {
            typings_location: js("/cache"),
            npm_executor: Some(Arc::new(BlockingNpm(started))),
            ..Default::default()
        },
        fs,
        &Counters::default(),
    );
    let events = session.subscribe();
    session
        .did_open_file(uri("/app.js"), 1, js(""), LanguageKind("javascript".into()))
        .unwrap();
    receive
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let (canceled, canceled_receive) = std::sync::mpsc::channel();
    let _on_cancel = session.context.after_func(move || {
        canceled.send(()).unwrap();
    });
    let update = session.update.lock().unwrap();
    let closing = session.clone();
    let worker = std::thread::spawn(move || closing.close());
    let cancellation = canceled_receive.recv_timeout(std::time::Duration::from_secs(2));
    drop(update);
    worker.join().unwrap();
    cancellation.unwrap();
    let events = events.try_iter().collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::InstallingTypes { finished, .. } => Some(*finished),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [false, true]
    );
    assert!(matches!(events.last(), Some(SessionEvent::Closed)));
}

#[test]
fn pinned_node_modules_types_option_matrix() {
    for (scenario, config, source, installs) in [
        ("discover from node_modules", "{}", "", 2),
        (
            "discover from node_modules empty types",
            r#"{"compilerOptions":{"types":[]}}"#,
            "",
            1,
        ),
        (
            "discover from node_modules explicit types",
            r#"{"compilerOptions":{"types":["jquery"]}}"#,
            "",
            1,
        ),
        (
            "discover from node_modules empty types has import",
            r#"{"compilerOptions":{"types":[]}}"#,
            "import 'jquery';",
            2,
        ),
    ] {
        let (session, npm) = setup(&[
            ("/project/app.js", source),
            ("/project/jsconfig.json", config),
            (
                "/project/package.json",
                r#"{"dependencies":{"jquery":"1.0.0"}}"#,
            ),
            ("/project/node_modules/commander/index.js", ""),
            (
                "/project/node_modules/commander/package.json",
                r#"{"name":"commander"}"#,
            ),
            ("/project/node_modules/jquery/index.js", ""),
            (
                "/project/node_modules/jquery/package.json",
                r#"{"name":"jquery"}"#,
            ),
            (
                "/project/node_modules/jquery/nested/package.json",
                r#"{"name":"nested"}"#,
            ),
        ]);
        open(&session, source);
        assert_eq!(npm.calls.lock().unwrap().len(), installs, "{scenario}");
        if installs == 2 {
            assert_eq!(
                npm.calls.lock().unwrap()[1][2].as_bytes(),
                b"@types/jquery@latest",
                "{scenario}"
            );
        }
        session.close();
    }
}

#[test]
fn pinned_bower_and_inferred_manifest_discovery() {
    for (scenario, extra) in [
        (
            "discover from bower_components",
            vec![
                ("/project/jsconfig.json", "{}"),
                ("/project/bower_components/jquery/index.js", ""),
                (
                    "/project/bower_components/jquery/bower.json",
                    r#"{"name":"jquery"}"#,
                ),
            ],
        ),
        (
            "discover from bower.json",
            vec![
                ("/project/jsconfig.json", "{}"),
                (
                    "/project/bower.json",
                    r#"{"dependencies":{"jquery":"^3.1.0"}}"#,
                ),
            ],
        ),
        (
            "inferred projects",
            vec![(
                "/project/package.json",
                r#"{"dependencies":{"jquery":"^3.1.0"}}"#,
            )],
        ),
    ] {
        let mut files = vec![("/project/app.js", "")];
        files.extend(extra);
        let (session, npm) = setup(&files);
        open(&session, "");
        assert_eq!(npm.calls.lock().unwrap().len(), 2, "{scenario}");
        let snapshot = session.snapshot_for_file(&uri("/project/app.js")).unwrap();
        assert!(
            snapshot
                .project_for_file(b"/project/app.js")
                .unwrap()
                .contains_file(b"/cache/node_modules/@types/jquery/index.d.ts"),
            "{scenario}"
        );
        // No WatchClient is installed: this also retains the pinned WatchEnabled=false regression.
        session.close();
    }
}

#[test]
fn pinned_disabled_filename_inference() {
    let (session, npm) = setup(&[
        ("/project/jquery.js", ""),
        (
            "/project/tsconfig.json",
            r#"{"compilerOptions":{"allowJs":true},"typeAcquisition":{"enable":true,"disableFilenameBasedTypeAcquisition":true}}"#,
        ),
    ]);
    session
        .did_open_file(
            uri("/project/jquery.js"),
            1,
            js(""),
            LanguageKind("javascript".into()),
        )
        .unwrap();
    session.wait_for_background_tasks();
    assert_eq!(npm.calls.lock().unwrap().len(), 1);
    session.close();
}

#[test]
fn pinned_malformed_manifest_fix_triggers_acquisition() {
    let (session, npm) = setup(&[
        ("/project/app.js", ""),
        ("/project/package.json", r#"{"dependencies":{"co } }"#),
    ]);
    open(&session, "");
    assert_eq!(npm.calls.lock().unwrap().len(), 1);
    npm.fs
        .write_file(
            b"/project/package.json",
            br#"{"dependencies":{"commander":"0.0.2"}}"#,
        )
        .unwrap();
    session
        .did_change_watched_files([tsr_lsproto::FileEvent {
            uri: uri("/project/package.json"),
            r#type: tsr_lsproto::FileChangeType(2),
        }])
        .unwrap();
    session.snapshot_for_file(&uri("/project/app.js")).unwrap();
    session.wait_for_background_tasks();
    assert_eq!(npm.calls.lock().unwrap().len(), 2);
    let snapshot = session.snapshot_for_file(&uri("/project/app.js")).unwrap();
    assert!(snapshot
        .project_for_file(b"/project/app.js")
        .unwrap()
        .contains_file(b"/cache/node_modules/@types/commander/index.d.ts"));
    session.close();
}

#[test]
fn pinned_js_resolution_is_replaced_after_acquisition() {
    let source = "import * as commander from 'commander';";
    let (session, npm) = setup(&[
        ("/project/app.js", source),
        ("/node_modules/commander/index.js", "module.exports = 0;"),
    ]);
    open(&session, source);
    assert_eq!(npm.calls.lock().unwrap().len(), 2);
    let snapshot = session.snapshot_for_file(&uri("/project/app.js")).unwrap();
    let project = snapshot.project_for_file(b"/project/app.js").unwrap();
    assert!(project.contains_file(b"/cache/node_modules/@types/commander/index.d.ts"));
    assert!(!project.contains_file(b"/node_modules/commander/index.js"));
    session.close();
}

#[test]
fn pinned_legacy_and_v3_cache_versions_produce_expected_program_text() {
    for lock_kind in ["dependencies", "packages"] {
        for version in ["1.0.0", "1.3.0"] {
            let key = if lock_kind == "packages" {
                "node_modules/@types/jquery"
            } else {
                "@types/jquery"
            };
            let lock = format!(r#"{{"{lock_kind}":{{"{key}":{{"version":"{version}"}}}}}}"#);
            let (session, npm) = setup(&[
                ("/project/app.js", ""),
                (
                    "/project/package.json",
                    r#"{"dependencies":{"jquery":"^3.1.0"}}"#,
                ),
                (
                    "/cache/node_modules/@types/jquery/index.d.ts",
                    "export const x = 10;",
                ),
                (
                    "/cache/package.json",
                    r#"{"devDependencies":{"@types/jquery":"^1.0.0"}}"#,
                ),
                ("/cache/package-lock.json", &lock),
            ]);
            open(&session, "");
            let snapshot = session.snapshot_for_file(&uri("/project/app.js")).unwrap();
            let source = snapshot
                .project_for_file(b"/project/app.js")
                .unwrap()
                .program()
                .unwrap()
                .source_file(b"/cache/node_modules/@types/jquery/index.d.ts")
                .unwrap();
            assert_eq!(
                source
                    .bound()
                    .view()
                    .source_file()
                    .unwrap()
                    .text()
                    .as_bytes(),
                if version == "1.3.0" {
                    b"export const x = 10;".as_slice()
                } else {
                    b"export const value: number;"
                }
            );
            assert_eq!(
                npm.calls.lock().unwrap().len(),
                if version == "1.3.0" { 1 } else { 2 }
            );
            session.close();
        }
    }
}

#[test]
fn pinned_unresolved_node_and_scoped_imports_are_installed_together() {
    let source = "import * as fs from 'fs'; import * as commander from 'commander'; import * as component from '@ember/component';";
    let (session, npm) = setup(&[("/project/app.js", source)]);
    open(&session, source);
    let calls = npm.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    for name in [
        "@types/node@latest",
        "@types/commander@latest",
        "@types/ember__component@latest",
    ] {
        assert!(calls[1].contains(&js(name)));
    }
    drop(calls);
    let snapshot = session.snapshot_for_file(&uri("/project/app.js")).unwrap();
    for name in ["node", "commander", "ember__component"] {
        assert!(snapshot
            .project_for_file(b"/project/app.js")
            .unwrap()
            .contains_file(format!("/cache/node_modules/@types/{name}/index.d.ts").as_bytes()));
    }
    session.close();
}
