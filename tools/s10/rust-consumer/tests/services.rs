//! Public configured-session witnesses; no internal compiler entry points.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tsr_arena::{Counters, Error as ArenaError};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_embed::content_mapper::{self, Host, Project};
use tsr_embed::{
    build_info_reader, BuildInfo, BuildInfoReader, DiagnosticKind, EmitOptions, FileCache, Program,
    ProgramOptions, QueryError, Session, SessionError, SessionRequest, SessionServices,
};
use tsr_jsstring::JsString;

const GLOBALS: &[u8] = b"interface Array<T> { length: number; [n: number]: T; }\ninterface Boolean {}\ninterface CallableFunction {}\ninterface Function {}\ninterface IArguments {}\ninterface NewableFunction {}\ninterface Number {}\ninterface Object {}\ninterface RegExp {}\ninterface String {}\n";

fn options(incremental: bool, build_info: Option<&[u8]>) -> ProgramOptions {
    let mut host = tsr_vfs::MemoryBuilder::new(b"/", true);
    host.insert_loaded(b"/globals.d.ts", GLOBALS);
    host.insert_loaded(b"/main.ts", b"export const value: string = 1;\n".as_slice());
    if let Some(text) = build_info {
        host.insert_loaded(b"/state.tsbuildinfo", text);
    }
    ProgramOptions {
        config: tsr_tsoptions::ParsedCommandLine::new(
            CompilerOptions {
                no_lib: Tristate::TRUE,
                strict: Tristate::TRUE,
                no_emit: Tristate::TRUE,
                incremental: if incremental {
                    Tristate::TRUE
                } else {
                    Tristate::FALSE
                },
                ts_build_info_file: js(b"/state.tsbuildinfo"),
                target: ScriptTarget::ES2022,
                module: ModuleKind::ESNEXT,
                ..Default::default()
            },
            vec![js(b"/globals.d.ts"), js(b"/main.ts")],
        ),
        host: Arc::new(host.finish()),
        current_directory: js(b"/"),
        default_library_path: js(b"/lib"),
        skip_module_resolution: false,
        single_threaded: Tristate::TRUE,
    }
}

fn js(text: &[u8]) -> JsString {
    JsString::from_bytes(text)
}

#[test]
fn configured_load_emits_and_reuses_build_info_from_the_supplied_host() {
    let counters = Counters::new();
    let first = Session::load(options(true, None), &mut FileCache::new(), &counters).unwrap();
    let diagnostics = first
        .diagnostics(&SessionRequest::default(), DiagnosticKind::Semantic, None)
        .unwrap();
    assert_eq!(
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
        [2322]
    );
    let output = first.emit(&EmitOptions::default()).unwrap();
    assert_eq!(output.files.len(), 1);
    assert_eq!(output.files[0].name.as_bytes(), b"/state.tsbuildinfo");
    assert_eq!(output.writes[0].emitted_file_index, Some(0));
    let info = output.writes[0]
        .data
        .build_info
        .as_ref()
        .unwrap()
        .downcast_ref::<BuildInfo>()
        .unwrap();
    assert!(info.is_valid_version());
    assert!(info
        .semantic_diagnostics_per_file
        .as_ref()
        .is_some_and(|files| !files.is_empty()));
    drop(first);
    let second = Session::load(
        options(true, Some(&output.files[0].text)),
        &mut FileCache::new(),
        &counters,
    )
    .unwrap();
    let diagnostics = second
        .diagnostics(&SessionRequest::default(), DiagnosticKind::Semantic, None)
        .unwrap();
    assert_eq!(
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
        [2322]
    );
    assert!(second
        .emit(&EmitOptions::default())
        .unwrap()
        .writes
        .is_empty());
    second.retire();
    assert!(matches!(
        second.emit(&EmitOptions::default()),
        Err(SessionError::Checker(QueryError::Arena(
            ArenaError::Retired
        )))
    ));
    assert!(output.program().unwrap().file(b"/main.ts").is_some());
}

struct VersionReader {
    inner: Box<dyn BuildInfoReader>,
    calls: AtomicUsize,
}
impl BuildInfoReader for VersionReader {
    fn read_build_info(&self, config: &tsr_tsoptions::ParsedCommandLine) -> Option<BuildInfo> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut info = self.inner.read_build_info(config)?;
        info.version = js(b"foreign compiler version");
        Some(info)
    }
}

#[test]
fn configured_adoption_validates_reader_data_and_plain_adoption_stays_plain() {
    let counters = Counters::new();
    let first = Session::load(options(true, None), &mut FileCache::new(), &counters).unwrap();
    let output = first.emit(&EmitOptions::default()).unwrap();
    let program = Arc::new(
        Program::load(
            options(true, Some(&output.files[0].text)),
            &mut FileCache::new(),
            &counters,
        )
        .unwrap(),
    );
    let reader = VersionReader {
        inner: build_info_reader(program.clone()),
        calls: AtomicUsize::new(0),
    };
    let plain = Session::from_program(program.clone(), &counters);
    assert!(plain
        .emit(&EmitOptions::default())
        .unwrap()
        .writes
        .is_empty());
    let configured =
        Session::from_program_with_build_info_reader(program, Some(&reader), &counters).unwrap();
    assert_eq!(reader.calls.load(Ordering::SeqCst), 1);
    // Production rejects a different compiler version, so this is a new build.
    assert_eq!(
        configured
            .emit(&EmitOptions::default())
            .unwrap()
            .files
            .len(),
        1
    );
    let services = SessionServices {
        build_info_reader: Some(&reader),
        ..Default::default()
    };
    let nonincremental = Session::load_with_services(
        options(false, None),
        &services,
        &mut FileCache::new(),
        &counters,
    )
    .unwrap();
    assert!(nonincremental
        .emit(&EmitOptions::default())
        .unwrap()
        .writes
        .is_empty());
    assert_eq!(reader.calls.load(Ordering::SeqCst), 1);
}

struct MappedProject {
    transforms: AtomicUsize,
    closes: AtomicUsize,
}
impl Project for MappedProject {
    fn refresh(&self) -> Result<(), content_mapper::Error> {
        Ok(())
    }
    fn identities(&self) -> Result<Vec<String>, content_mapper::Error> {
        Ok(vec!["public-mapper@1".into()])
    }
    fn identity(&self, mapper: usize) -> Result<String, content_mapper::Error> {
        assert_eq!(mapper, 0);
        Ok("public-mapper@1".into())
    }
    fn watched_files(&self) -> Result<Vec<String>, content_mapper::Error> {
        Ok(Vec::new())
    }
    fn diagnostics(&self) -> Vec<content_mapper::OptionDiagnostic> {
        Vec::new()
    }
    fn transform(
        &self,
        mapper: usize,
        request: &content_mapper::Request,
    ) -> Result<content_mapper::TransformResult, content_mapper::Error> {
        assert_eq!(mapper, 0);
        assert_eq!(request.file_name.as_bytes(), b"/main.box");
        assert_eq!(request.content, b"original mapped source");
        self.transforms.fetch_add(1, Ordering::SeqCst);
        let mappings = Some(Arc::new(tsr_ast::span_map::SpanMap::new(&[])));
        Ok(content_mapper::TransformResult {
            text: "export const canonical: string = 1;\n".into(),
            virtual_extension: ".ts".into(),
            mappings: mappings.clone(),
            supplemental: vec![content_mapper::MappedResult {
                text: "export const supplemental = 2;\n".into(),
                virtual_extension: ".ts".into(),
                mappings,
                ..Default::default()
            }],
            ..Default::default()
        })
    }
    fn close(&self) -> Result<(), content_mapper::Error> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn mapped_options() -> ProgramOptions {
    let mut options = options(false, None);
    let mut host = tsr_vfs::MemoryBuilder::new(b"/", true);
    host.insert_loaded(b"/globals.d.ts", GLOBALS);
    host.insert_loaded(b"/main.box", b"original mapped source".as_slice());
    options.host = Arc::new(host.finish());
    options.config.root_file_names = vec![js(b"/globals.d.ts"), js(b"/main.box")];
    options.config.options.no_emit = Tristate::FALSE;
    options.config.options.declaration = Tristate::TRUE;
    options.config.options.run_external_code = Tristate::TRUE;
    options.config.content_mappers = Some(vec![tsr_tsoptions::config_mappers::ContentMapper {
        extensions: vec![js(b".box")],
        manifest: tsr_tsoptions::config_mappers::MapperManifest {
            name: js(b"public-mapper"),
            version: js(b"1"),
            exec: Some(vec![js(b"public-mapper")]),
            ..Default::default()
        },
        ..Default::default()
    }]);
    options
}

#[test]
fn caller_owned_mapper_project_loads_and_emits_canonical_and_supplemental_sources() {
    let counters = Counters::new();
    let project = Arc::new(MappedProject {
        transforms: AtomicUsize::new(0),
        closes: AtomicUsize::new(0),
    });
    let services = SessionServices {
        content_mapper_project: Some(project.clone()),
        ..Default::default()
    };
    let session = Session::load_with_services(
        mapped_options(),
        &services,
        &mut FileCache::new(),
        &counters,
    )
    .unwrap();
    let weak_program = Arc::downgrade(session.program());
    assert_eq!(project.transforms.load(Ordering::SeqCst), 1);
    assert!(session.program().file(b"/main.box.0.ts").is_some());
    let diagnostics = session
        .diagnostics(&SessionRequest::default(), DiagnosticKind::Semantic, None)
        .unwrap();
    assert_eq!(
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
        [2322]
    );
    let output = session.emit(&EmitOptions::default()).unwrap();
    assert!(output.files.iter().any(|file| file
        .text
        .windows(b"canonical: string".len())
        .any(|text| text == b"canonical: string")));
    assert!(output.files.iter().any(|file| file
        .text
        .windows(b"supplemental = 2".len())
        .any(|text| text == b"supplemental = 2")));
    drop(session);
    assert!(weak_program.upgrade().is_some());
    let retained_program = output.program().unwrap();
    for (path, text) in [
        (
            b"/main.box".as_slice(),
            b"export const canonical: string = 1;\n".as_slice(),
        ),
        (
            b"/main.box.0.ts".as_slice(),
            b"export const supplemental = 2;\n".as_slice(),
        ),
    ] {
        let source = retained_program
            .file(path)
            .unwrap()
            .bound()
            .view()
            .source_file()
            .unwrap();
        assert_eq!(source.text().as_bytes(), text);
    }
    assert_eq!(project.closes.load(Ordering::SeqCst), 0);
    drop(output);
    assert!(weak_program.upgrade().is_none());
    assert_eq!(counters.snapshot(), tsr_arena::Counts::default());
    project.close().unwrap();
    assert_eq!(project.closes.load(Ordering::SeqCst), 1);
}

#[test]
fn production_mapper_host_uses_the_supplied_spawner_and_reports_its_failure() {
    let spawns = Arc::new(AtomicUsize::new(0));
    let count = spawns.clone();
    let spawner = content_mapper::SpawnerFunc(
        move |command: &[JsString],
              _dir: &[u8],
              _stderr: Box<dyn std::io::Write + Send>|
              -> Result<content_mapper::Stream, content_mapper::SpawnError> {
            assert_eq!(command[0].as_bytes(), b"public-mapper");
            count.fetch_add(1, Ordering::SeqCst);
            Err(std::io::Error::other("embedding spawner unavailable").into())
        },
    );
    let host = content_mapper::new_host(
        &content_mapper::Context::background(),
        Arc::new(spawner),
        content_mapper::Locale::default(),
    );
    let options = mapped_options();
    let project = host
        .project(content_mapper::ProjectSpec {
            mappers: Arc::from(options.config.content_mappers.clone().unwrap()),
            compiler_options: Arc::new(options.config.options.clone()),
            ..Default::default()
        })
        .unwrap();
    let services = SessionServices {
        content_mapper_project: Some(project.clone()),
        ..Default::default()
    };
    let session =
        Session::load_with_services(options, &services, &mut FileCache::new(), &Counters::new())
            .unwrap();
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
    assert!(!session
        .diagnostics(&SessionRequest::default(), DiagnosticKind::Program, None)
        .unwrap()
        .is_empty());
    drop(session);
    project.close().unwrap();
    host.close().unwrap();
}

#[test]
fn incremental_session_cancellation_foreign_files_and_retained_queries_keep_their_gates() {
    let counters = Counters::new();
    let session = Session::load(options(true, None), &mut FileCache::new(), &counters).unwrap();
    let token = tsr_core::CancellationToken::new();
    token.cancel();
    assert!(session
        .emit_with_request(
            &SessionRequest {
                cancellation: Some(token),
                ..Default::default()
            },
            &EmitOptions::default()
        )
        .unwrap()
        .is_none());
    let foreign = Session::load(options(true, None), &mut FileCache::new(), &counters).unwrap();
    assert!(matches!(
        session.diagnostics(
            &SessionRequest::default(),
            DiagnosticKind::Semantic,
            foreign.program().file(b"/main.ts")
        ),
        Err(SessionError::Ast(ArenaError::WrongOwner))
    ));
    let retained = session
        .with_compiler_operations(|operations| {
            let operation = operations.for_file(session.program().file(b"/main.ts").unwrap())?;
            operation.retain_type(operation.builtin_type("numberType").unwrap())
        })
        .unwrap();
    assert_eq!(
        session.emit(&EmitOptions::default()).unwrap().files.len(),
        1
    );
    drop(session);
    retained.owner().operation().unwrap();
    assert!(matches!(
        foreign.with_compiler_operations(|operations| {
            operations
                .for_file(foreign.program().file(b"/main.ts").unwrap())?
                .import_type(&retained)
                .map(|_| ())
        }),
        Err(SessionError::Checker(QueryError::Arena(
            ArenaError::WrongOwner
        )))
    ));
    foreign.retire();
    assert!(matches!(
        foreign.with_compiler_operations(|_| Ok(())),
        Err(SessionError::Checker(QueryError::Arena(
            ArenaError::Retired
        )))
    ));
}

#[test]
fn public_config_host_resolves_extends_and_include_before_session_load() {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/project", true);
    fs.insert_physical(
        b"/project/base.json",
        br#"{"compilerOptions":{"noLib":true,"strict":true}}"#.as_slice(),
    );
    fs.insert_physical(
        b"/project/tsconfig.json",
        br#"{"extends":"./base.json","include":["src/*.ts"]}"#.as_slice(),
    );
    fs.insert_physical(b"/project/src/main.ts", b"export {};".as_slice());
    fs.insert_physical(b"/project/other.ts", b"export {};".as_slice());
    let fs: Arc<dyn tsr_vfs::FileSystem> = Arc::new(fs.finish());
    let host = tsr_embed::CompilerConfigHost::new(fs.clone(), js(b"/project"));
    let parsed = tsr_tsoptions::get_parsed_command_line_of_config_file(
        b"/project/tsconfig.json",
        &CompilerOptions::default(),
        &Default::default(),
        &host,
    )
    .unwrap();
    let config = parsed.command_line.unwrap();
    assert_eq!(config.root_file_names, [js(b"/project/src/main.ts")]);
    assert!(config.options.strict.is_true());
    let session = Session::load(
        ProgramOptions {
            config,
            host: fs,
            current_directory: js(b"/project"),
            default_library_path: js(b"/lib"),
            skip_module_resolution: false,
            single_threaded: Tristate::TRUE,
        },
        &mut FileCache::new(),
        &Counters::new(),
    )
    .unwrap();
    assert_eq!(session.program().files().len(), 1);
}

#[test]
fn unavailable_snapshot_content_is_fatal_through_public_config_and_load() {
    let mut builder = tsr_vfs::MemoryBuilder::new(b"/project", true);
    builder.insert_unavailable_file(b"/project/tsconfig.json", 2);
    let host = tsr_embed::CompilerConfigHost::new(Arc::new(builder.finish()), js(b"/project"));
    let error = tsr_tsoptions::get_parsed_command_line_of_config_file(
        b"/project/tsconfig.json",
        &CompilerOptions::default(),
        &Default::default(),
        &host,
    )
    .err()
    .unwrap();
    assert!(error
        .to_string()
        .contains("snapshot file content is unavailable"));
    assert!(error.to_string().contains("/project/tsconfig.json"));

    let mut builder = tsr_vfs::MemoryBuilder::new(b"/project", true);
    builder.insert_unavailable_file(b"/project/main.ts", 7);
    let error = Session::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                CompilerOptions {
                    no_lib: Tristate::TRUE,
                    ..Default::default()
                },
                vec![js(b"/project/main.ts")],
            ),
            host: Arc::new(builder.finish()),
            current_directory: js(b"/project"),
            default_library_path: js(b"/lib"),
            skip_module_resolution: false,
            single_threaded: Tristate::TRUE,
        },
        &mut FileCache::new(),
        &Counters::new(),
    )
    .err()
    .unwrap();
    assert!(error
        .to_string()
        .contains("snapshot file content is unavailable"));
    let SessionError::Host(tsr_vfs::Error::Detailed(error)) = error else {
        panic!("loader must preserve the structured host error");
    };
    assert!(!error.is_not_exist());
    assert!(error.to_string().contains("/project/main.ts"));
}
