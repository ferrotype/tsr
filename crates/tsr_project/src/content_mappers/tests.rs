use super::*;
use crate::parse_cache::{ContentMappedParseCache, ParseCache};
use crate::ref_count_cache::RefCountCacheOptions;
use tsr_arena::Counters;
use tsr_compiler::{FileCache, SourceFileCache};
use tsr_core::{CompilerOptions, Tristate};
fn js(value: &str) -> JsString {
    JsString::from_bytes(value.as_bytes())
}
fn command() -> Arc<ParsedCommandLine> {
    let mut command = ParsedCommandLine::new(
        CompilerOptions {
            no_lib: Tristate::TRUE,
            ..Default::default()
        },
        vec![js("/app.box")],
    );
    command.content_mappers = Some(vec![ContentMapper {
        package: js("mapper"),
        extensions: vec![js(".box")],
        manifest: tsr_tsoptions::config_mappers::MapperManifest {
            name: js("mapper"),
            exec: Some(vec![js("compiler-test-mapper")]),
            compiler_options: Some(vec![js("target")]),
            ..Default::default()
        },
        ..Default::default()
    }]);
    Arc::new(command)
}
struct Cache {
    ordinary: ParseCache,
    mapped: Arc<ContentMappedParseCache>,
}
impl SourceFileCache for Cache {
    fn retain(
        &self,
        file: &Arc<tsr_compiler::ProgramFile>,
    ) -> Result<Box<dyn Send + Sync>, tsr_compiler::Error> {
        if file
            .bound()
            .view()
            .source_file()?
            .content_mapper()
            .is_empty()
        {
            self.ordinary.retain(file)
        } else {
            self.mapped.retain(file)
        }
    }
    fn acquire(
        &self,
        source: tsr_jsstring::SourceText,
        kind: tsr_core::ScriptKind,
        options: tsr_ast::SourceFileParseOptions,
        counters: &Counters,
        tracing: Option<&Arc<dyn tsr_checker::TraceSink>>,
    ) -> Result<tsr_compiler::CachedProgramFile, tsr_compiler::Error> {
        self.ordinary
            .acquire(source, kind, options, counters, tracing)
    }
    fn acquire_mapped(
        &self,
        request: &tsr_compiler::MappedSourceFileRequest<'_>,
    ) -> tsr_compiler::MappedFileResult<tsr_compiler::CachedMappedProgramFiles> {
        self.mapped.acquire_mapped(request, "en")
    }
}
// source: tsc/internal/project/contentmapper_test.go:TestContentMapperInProject
#[test]
fn real_host_cache_reuses_transform_across_full_program_loads_and_edits() {
    let counters = Counters::new();
    let cache = Arc::new(Cache {
        ordinary: ParseCache::new(RefCountCacheOptions::default()),
        mapped: Arc::new(ContentMappedParseCache::new(RefCountCacheOptions::default())),
    });
    let host = MapperHost::new(
        true,
        Some(tsr_contentmappertest::new_spawner()),
        &tsr_ipc::Context::background(),
        tsr_locale::Locale::default(),
        None,
    )
    .unwrap();
    let command = command();
    let project = host.project(&command).unwrap();
    assert!(Arc::ptr_eq(&project, &host.project(&command).unwrap()));
    let fs = |text: &str| {
        let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
        fs.insert_loaded(b"/app.box", text.as_bytes());
        Arc::new(fs.finish())
    };
    let load = |filesystem| {
        tsr_compiler::Program::load_live_with_content_mapper_project(
            tsr_compiler::ProgramOptions {
                config: (*command).clone(),
                host: filesystem,
                current_directory: js("/"),
                default_library_path: js("/"),
                skip_module_resolution: false,
                single_threaded: Tristate::TRUE,
            },
            Some(project.clone()),
            &mut FileCache::for_project(cache.clone()),
            &counters,
        )
        .unwrap()
    };
    let first = load(fs("export const version = #{target};\n"));
    let second = load(fs("export const version = #{target};\n"));
    let source =
        |program: &tsr_compiler::Program| program.source_file(b"/app.box").unwrap().source();
    assert_eq!(source(&first), source(&second));
    let file = second.source_file(b"/app.box").unwrap();
    assert!(!file
        .bound()
        .view()
        .source_file()
        .unwrap()
        .text()
        .as_bytes()
        .windows(9)
        .any(|bytes| bytes == b"#{target}"));
    assert_eq!(
        host.timings()
            .mappers
            .values()
            .map(|m| m.transform.count)
            .sum::<u64>(),
        1
    );
    let changed = second
        .reuse_program(
            b"/app.box",
            fs("export const version = 2;\n"),
            &mut FileCache::for_project(cache.clone()),
            &counters,
        )
        .unwrap();
    assert!(changed.program.is_some());
    drop((first, second, changed));
    assert!(cache.mapped.is_empty());
    drop(project);
    assert_eq!(
        host.timings()
            .mappers
            .values()
            .map(|m| m.close_project.count)
            .sum::<u64>(),
        1
    );
}

// source: tsc/internal/project/contentmapper_test.go:TestUnusedDynamicContentMapperIsNotOpened
#[test]
fn trust_gate_and_unused_project_do_not_spawn_a_mapper() {
    assert!(MapperHost::new(
        false,
        Some(tsr_contentmappertest::new_spawner()),
        &tsr_ipc::Context::background(),
        tsr_locale::Locale::default(),
        None
    )
    .is_none());
    let host = MapperHost::new(
        true,
        Some(tsr_contentmappertest::new_spawner()),
        &tsr_ipc::Context::background(),
        tsr_locale::Locale::default(),
        None,
    )
    .unwrap();
    let project = host.project(&command()).unwrap();
    assert!(host.timings().mappers.values().all(|m| m.spawn.count == 0));
    drop(project);
    assert!(host.timings().mappers.values().all(|m| m.spawn.count == 0));
}

use crate::session::{Session, SessionOptions};
use tsr_lsproto::{DocumentUri, FileChangeType, FileEvent, LanguageKind};
use tsr_vfs::{
    iovfs,
    vfstest::{self, InputFile, TestFs},
};
fn uri(name: &str) -> DocumentUri {
    DocumentUri::from_file_name(name.as_bytes())
}
fn session(
    command: &str,
    trusted: bool,
    files: &[(&str, &str)],
) -> (
    Arc<TestFs>,
    Arc<Session>,
    Arc<tsr_contentmappertest::ProjectLifecycle>,
) {
    session_with_counters(command, trusted, files, &Counters::new())
}
fn session_with_counters(
    command: &str,
    trusted: bool,
    files: &[(&str, &str)],
    counters: &Counters,
) -> (
    Arc<TestFs>,
    Arc<Session>,
    Arc<tsr_contentmappertest::ProjectLifecycle>,
) {
    let manifest = format!(
        r#"{{"name":"mapper","version":"1.0.0","typescript":{{"contentMapper":{{"exec":["{command}"],"compilerOptions":["target"],"dynamicConfig":{}}}}}}}"#,
        command == "dynamic-verbatim-mapper"
    );
    let mut files: std::collections::BTreeMap<_, _> = files
        .iter()
        .map(|(name, text)| {
            (
                name.as_bytes().to_vec(),
                InputFile::Text(text.as_bytes().to_vec()),
            )
        })
        .collect();
    files.insert(
        b"/p/node_modules/mapper/package.json".to_vec(),
        InputFile::Text(manifest.into_bytes()),
    );
    let fs = Arc::new(vfstest::from_map(&files, false));
    let lifecycle = Arc::new(tsr_contentmappertest::ProjectLifecycle::default());
    let session = Session::new(
        SessionOptions {
            current_directory: js("/p"),
            run_external_code: trusted,
            mapper_spawner: Some(tsr_contentmappertest::new_spawner_with_project_lifecycle(
                lifecycle.clone(),
            )),
            ..Default::default()
        },
        Arc::new(iovfs::from(fs.clone(), false)),
        counters,
    );
    (fs, session, lifecycle)
}
const CONFIG: &str = r#"{"compilerOptions":{"noLib":true,"target":"es2020","module":"esnext","moduleResolution":"bundler"},"contentMappers":[{"package":"mapper","extensions":[".box"]}]}"#;
fn open(session: &Session, name: &str, text: &str) -> crate::Snapshot {
    session
        .did_open_file(
            uri(name),
            1,
            js(text),
            LanguageKind(
                if tsr_tspath::file_extension_is(name.as_bytes(), b".box") {
                    "box"
                } else {
                    "typescript"
                }
                .into(),
            ),
        )
        .unwrap()
}
fn source(snapshot: &crate::Snapshot, name: &str) -> Arc<tsr_compiler::ProgramFile> {
    snapshot
        .project_for_file(b"/p/main.ts")
        .or_else(|| snapshot.project_for_file(name.as_bytes()))
        .unwrap()
        .program()
        .unwrap()
        .files()
        .iter()
        .find(|file| file.bound().view().source_file().unwrap().file_name() == name.as_bytes())
        .unwrap()
        .clone()
}
fn watch(session: &Session, name: &str, kind: u32) {
    session
        .did_change_watched_files([FileEvent {
            uri: uri(name),
            r#type: FileChangeType(kind),
        }])
        .unwrap();
}
fn changed(session: &Session, name: &str, text: &str) {
    session
        .did_change_file(
            uri(name),
            2,
            vec![
                tsr_lsproto::TextDocumentContentChangePartialOrWholeDocument {
                    whole_document: Some(Box::new(
                        tsr_lsproto::TextDocumentContentChangeWholeDocument { text: text.into() },
                    )),
                    ..Default::default()
                },
            ],
        )
        .unwrap();
}

// source: tsc/internal/project/contentmapper_test.go:TestContentMapperInProject
// source: tsc/internal/project/contentmapper_test.go:TestContentMapperLocaleChange
#[test]
fn session_configured_mapper_trust_edit_watch_cache_and_locale() {
    let files = [
        ("/p/tsconfig.json", CONFIG),
        ("/p/app.box", "export const version = #{target};"),
        (
            "/p/main.ts",
            "import { version } from './app.box'; version;",
        ),
    ];
    let (_, untrusted, _) = session("compiler-test-mapper", false, &files);
    let snapshot = open(&untrusted, "/p/main.ts", files[2].1);
    assert!(snapshot
        .project_for_file(b"/p/main.ts")
        .unwrap()
        .program()
        .unwrap()
        .source_file(b"/p/app.box")
        .is_none());
    untrusted.close();
    let (fs, session, _) = session("compiler-test-mapper", true, &files);
    let first = open(&session, "/p/main.ts", files[2].1);
    let old = source(&first, "/p/app.box");
    assert!(old
        .bound()
        .view()
        .source_file()
        .unwrap()
        .text()
        .as_bytes()
        .ends_with(b"export const version = 7;"));
    fs.write_file(
        b"p/tsconfig.json",
        CONFIG
            .replace("\"noLib\":true", "\"noLib\":true,\"strict\":true")
            .as_bytes(),
        0,
    )
    .unwrap();
    watch(&session, "/p/tsconfig.json", 2);
    let rebuilt = session.flush(Some(&uri("/p/main.ts"))).unwrap();
    assert_eq!(source(&rebuilt, "/p/app.box").source(), old.source());
    fs.write_file(
        b"p/app.box",
        b"export const version = #{target}; export const watched = 1;",
        0,
    )
    .unwrap();
    watch(&session, "/p/app.box", 2);
    let watched = session.flush(Some(&uri("/p/main.ts"))).unwrap();
    assert_ne!(source(&watched, "/p/app.box").source(), old.source());
    open(&session, "/p/app.box", "export const version = #{target};");
    changed(
        &session,
        "/p/app.box",
        "export const version = #{target}; export const extra = 1;",
    );
    let edited = session.flush(Some(&uri("/p/main.ts"))).unwrap();
    assert!(source(&edited, "/p/app.box")
        .bound()
        .view()
        .source_file()
        .unwrap()
        .text()
        .as_bytes()
        .ends_with(b"export const version = 7; export const extra = 1;"));
    session.set_locale(tsr_locale::Locale::parse("fr").0);
    let localized = session.flush(Some(&uri("/p/main.ts"))).unwrap();
    assert_ne!(
        source(&localized, "/p/app.box").source(),
        source(&edited, "/p/app.box").source()
    );
    session.close();
}

// source: tsc/internal/project/contentmapper_test.go:TestDynamicContentMapperInProject
// source: tsc/internal/project/contentmapper_test.go:TestDynamicContentMapperRefreshesForMixedWatchBatches
// source: tsc/internal/project/contentmapper_test.go:TestUnusedDynamicContentMapperIsNotOpened
#[test]
fn session_dynamic_watch_refresh_happens_even_after_other_events_make_project_dirty() {
    for excessive in [false, true] {
        let (_, session, lifecycle) = session(
            "dynamic-verbatim-mapper",
            true,
            &[
                ("/p/tsconfig.json", CONFIG),
                ("/p/main.ts", "export const value = 1;"),
                ("/p/app.box", "export const value = 1;"),
            ],
        );
        open(&session, "/p/main.ts", "export const value = 1;");
        assert_eq!(lifecycle.opens.load(std::sync::atomic::Ordering::SeqCst), 1);
        let mut events = vec![
            FileEvent {
                uri: uri("/p/main.ts"),
                r#type: FileChangeType(2),
            },
            FileEvent {
                uri: uri("/p/app.box"),
                r#type: FileChangeType(2),
            },
            FileEvent {
                uri: uri("/p/mapper.config.json"),
                r#type: FileChangeType(3),
            },
        ];
        if excessive {
            events.extend((0..1001).map(|i| FileEvent {
                uri: uri(&format!("/p/noise-{i}.ts")),
                r#type: FileChangeType(2),
            }));
        }
        session.did_change_watched_files(events).unwrap();
        session.flush(Some(&uri("/p/main.ts"))).unwrap();
        assert_eq!(lifecycle.opens.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            lifecycle.closes.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        session.close();
    }
    let (_, session, lifecycle) = session(
        "dynamic-verbatim-mapper",
        true,
        &[
            ("/p/tsconfig.json", CONFIG),
            ("/p/main.ts", "export const value = 1;"),
        ],
    );
    open(&session, "/p/main.ts", "export const value = 1;");
    assert_eq!(lifecycle.opens.load(std::sync::atomic::Ordering::SeqCst), 0);
    session.close();
}

// source: tsc/internal/project/contentmapper_test.go:TestContentMapperSupplementalFileClonedOnEdit
// source: tsc/internal/project/contentmapper_test.go:TestContentMapperModuleExtensionClonedOnUnrelatedEdit
#[test]
fn session_bundle_reuse_updates_supplemental_and_preserves_module_parse_key() {
    let counters = Counters::new();
    let (fs, session, _) = session_with_counters(
        "supplemental-mapper",
        true,
        &[
            ("/p/tsconfig.json", CONFIG),
            ("/p/main.ts", "const value: number = supplementalValue;"),
            ("/p/app.box", "declare const supplementalValue: number;"),
        ],
        &counters,
    );
    let first = open(
        &session,
        "/p/main.ts",
        "const value: number = supplementalValue;",
    );
    let old = source(&first, "/p/app.box.0.ts");
    fs.write_file(b"p/app.box", b"declare const supplementalValue: string;", 0)
        .unwrap();
    watch(&session, "/p/app.box", 2);
    let updated = session.flush(Some(&uri("/p/main.ts"))).unwrap();
    let new = source(&updated, "/p/app.box.0.ts");
    assert_ne!(old.source(), new.source());
    assert_eq!(
        new.bound().view().source_file().unwrap().text().as_bytes(),
        b"declare const supplementalValue: string;"
    );
    assert_eq!(
        new.bound().view().source_file().unwrap().hash,
        source(&updated, "/p/app.box")
            .bound()
            .view()
            .source_file()
            .unwrap()
            .hash
    );
    drop((first, updated, old));
    session.close();
    drop(session);
    assert_eq!(
        counters.snapshot().owners,
        2,
        "escaped supplemental retains both mapped owners"
    );
    assert!(new.bound().view().source_file().is_ok());
    drop(new);
    assert_eq!(counters.snapshot(), tsr_arena::Counts::default());
    let (_, session, _) = self::session(
        "module-verbatim-mapper",
        true,
        &[
            ("/p/tsconfig.json", CONFIG),
            ("/p/main.ts", "const value = 1;"),
            ("/p/app.box", "const local = 1;"),
        ],
    );
    let first = open(&session, "/p/main.ts", "const value = 1;");
    let mapped = source(&first, "/p/app.box");
    changed(&session, "/p/main.ts", "const value = 2;");
    let updated = session.flush(Some(&uri("/p/main.ts"))).unwrap();
    assert_eq!(source(&updated, "/p/app.box").source(), mapped.source());
    session.close();
}

// source: tsc/internal/project/contentmapper_test.go:TestContentMapperInferredProjectUsesExtensionContributions
// source: tsc/internal/project/contentmapper_test.go:TestContentMapperRemovalWithOpenFile
// source: tsc/internal/project/contentmapper_test.go:TestContentMapperOpenFileExcludedByConfigChange
#[test]
fn session_contribution_change_adopts_unknown_files_and_removes_them() {
    let (_, session, _) = session(
        "compiler-test-mapper",
        true,
        &[("/loose.box", "export const version = #{target};")],
    );
    session
        .set_content_mapper_contributions(
            Contributions {
                mappers: command().content_mappers.clone().unwrap(),
                extensions: vec![".box".into()],
            },
            vec![uri("/loose.box")],
        )
        .unwrap();
    let first = open(&session, "/loose.box", "export const version = #{target};");
    let mapped = source(&first, "/loose.box");
    assert!(!mapped
        .bound()
        .view()
        .source_file()
        .unwrap()
        .is_content_mapper_failure_stub());
    let removed = session
        .set_content_mapper_contributions(Contributions::default(), vec![uri("/loose.box")])
        .unwrap();
    assert!(removed.project_for_file(b"/loose.box").is_none());
    session.close();
}

// source: tsc/internal/project/contentmapper_test.go:TestContentMapperPackageManifestChangeReloadsConfig
// source: tsc/internal/project/contentmapper_test.go:TestContentMapperCreatedFileAdoptedByConfiguredProject
#[test]
fn mapper_manifest_change_and_created_file_reload_the_configured_project() {
    let (fs, session, _) = session(
        "compiler-test-mapper",
        true,
        &[
            ("/p/tsconfig.json", CONFIG),
            ("/p/main.ts", "export const main = 1;"),
        ],
    );
    open(&session, "/p/main.ts", "export const main = 1;");
    fs.write_file(b"p/new.box", b"export const version = #{target};", 0)
        .unwrap();
    watch(&session, "/p/new.box", 1);
    let created = open(&session, "/p/new.box", "export const version = #{target};");
    assert_eq!(
        created
            .project_for_file(b"/p/new.box")
            .unwrap()
            .data()
            .unwrap()
            .kind,
        crate::project::ProjectKind::Configured
    );
    let old = source(&created, "/p/new.box");
    fs.write_file(b"p/node_modules/mapper/package.json", br#"{"name":"mapper","version":"2.0.0","typescript":{"contentMapper":{"exec":["compiler-test-mapper"],"compilerOptions":["target"]}}}"#, 0).unwrap();
    watch(&session, "/p/node_modules/mapper/package.json", 2);
    let updated = session.flush(Some(&uri("/p/main.ts"))).unwrap();
    let new = source(&updated, "/p/new.box");
    assert_ne!(old.source(), new.source());
    let configured = updated.project_for_file(b"/p/new.box").unwrap();
    assert_eq!(
        configured
            .data()
            .unwrap()
            .command_line
            .content_mappers
            .as_ref()
            .unwrap()[0]
            .manifest
            .version,
        js("2.0.0")
    );
    assert!(new
        .bound()
        .view()
        .source_file()
        .unwrap()
        .text()
        .as_bytes()
        .ends_with(b"export const version = 7;"));
    session.close();
}

// source: tsc/internal/project/contentmapper_test.go:TestContentMapperProcessSharedAcrossProjects
#[test]
fn project_leases_share_a_process_and_close_only_their_project_handle() {
    let host = MapperHost::new(
        true,
        Some(tsr_contentmappertest::new_spawner()),
        &tsr_ipc::Context::background(),
        tsr_locale::Locale::default(),
        None,
    )
    .unwrap();
    let first_command = command();
    let second_command = Arc::new((*first_command).clone());
    let first = host.project(&first_command).unwrap();
    let second = host.project(&second_command).unwrap();
    let request = tsr_contentmapper::Request {
        file_name: js("/app.box"),
        content: b"export const value = 1;".to_vec(),
    };
    first.transform(0, &request).unwrap();
    second.transform(0, &request).unwrap();
    assert_eq!(
        host.timings()
            .mappers
            .values()
            .map(|mapper| mapper.spawn.count)
            .sum::<u64>(),
        1
    );
    drop(first);
    assert_eq!(
        host.timings()
            .mappers
            .values()
            .map(|mapper| mapper.close_project.count)
            .sum::<u64>(),
        1
    );
    second.transform(0, &request).unwrap();
    drop(second);
    assert_eq!(
        host.timings()
            .mappers
            .values()
            .map(|mapper| mapper.close_project.count)
            .sum::<u64>(),
        2
    );
}
