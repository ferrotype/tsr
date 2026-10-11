//! External public-entry-point witnesses. These cover the named session and
//! transpile contracts below. PHASE7-R4.md maps the public E3 applicability.
use std::sync::Arc;
use tsr_arena::{Counters, Counts, Error as ArenaError};
use tsr_core::{CancellationToken, CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_embed::{
    transpile_declaration, transpile_module, DiagnosticKind, EmitOptions, FileCache,
    ProgramOptions, QueryError, Session, SessionError, SessionRequest, TranspileOptions,
    TranspileRequest,
};
use tsr_jsstring::JsString;

const GLOBALS: &[u8] = b"interface Array<T> { length: number; [n: number]: T; }
interface Boolean {}
interface CallableFunction {}
interface Function {}
interface IArguments {}
interface NewableFunction {}
interface Number {}
interface Object {}
interface RegExp {}
interface String {}
";
const INPUT: &[u8] = b"export const value: string = 1;\n";

fn load(input: &[u8], cache: &mut FileCache, counters: &Counters) -> Session {
    load_with_options(
        input,
        cache,
        counters,
        CompilerOptions {
            no_lib: Tristate::TRUE,
            strict: Tristate::TRUE,
            target: ScriptTarget::ES2022,
            module: ModuleKind::ESNEXT,
            ..Default::default()
        },
    )
}

fn load_with_options(
    input: &[u8],
    cache: &mut FileCache,
    counters: &Counters,
    options: CompilerOptions,
) -> Session {
    let mut host = tsr_vfs::MemoryBuilder::new(b"/", true);
    host.insert_loaded(b"/globals.d.ts", GLOBALS);
    host.insert_loaded(b"/main.ts", input);
    Session::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                options,
                vec![
                    JsString::from_bytes(b"/globals.d.ts".as_slice()),
                    JsString::from_bytes(b"/main.ts".as_slice()),
                ],
            ),
            host: Arc::new(host.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/lib".as_slice()),
            skip_module_resolution: false,
            single_threaded: Tristate::TRUE,
        },
        cache,
        counters,
    )
    .unwrap()
}

fn session(counters: &Counters) -> Session {
    load(INPUT, &mut FileCache::new(), counters)
}

#[test]
fn repeated_load_check_query_emit_drop_releases_tracked_storage() {
    let counters = Counters::new();
    let mut cache = FileCache::new();
    for _ in 0..8 {
        let session = load(INPUT, &mut cache, &counters);
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
            let string = operation.builtin_type("stringType").unwrap();
            assert_eq!(
                operation.intrinsic_type_name(string).unwrap().as_bytes(),
                b"string"
            );
        }
        let emitted = session.emit(&EmitOptions::default()).unwrap();
        drop(session);
        assert_eq!(emitted.files.len(), 1);
        assert_eq!(emitted.files[0].name.as_bytes(), b"/main.js");
        assert_eq!(emitted.files[0].text, b"export const value = 1;\n");
        assert!(weak_program.upgrade().is_some());
        drop(emitted);
        assert!(weak_program.upgrade().is_none());
        assert!(weak_checker.upgrade().is_none());
        assert_eq!(counters.snapshot(), Counts::default());
    }
}

#[test]
fn retained_type_survives_session_drop_and_rejects_a_foreign_session() {
    let counters = Counters::new();
    let first = session(&counters);
    let weak_program = Arc::downgrade(first.program());
    let retained = {
        let operation = first.operation().unwrap();
        operation
            .retain_type(operation.builtin_type("stringType").unwrap())
            .unwrap()
    };
    let other = Session::from_program(first.program().clone(), &counters);
    assert_eq!(
        other.operation().unwrap().import_type(&retained),
        Err(QueryError::Arena(ArenaError::WrongOwner))
    );
    drop(other);
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
fn reentry_fails_and_retirement_invalidates_retained_queries_and_emit() {
    let counters = Counters::new();
    for emit_first in [false, true] {
        let session = session(&counters);
        let retained = {
            let operation = session.operation().unwrap();
            assert!(matches!(session.operation(), Err(QueryError::Reentry)));
            operation
                .retain_type(operation.builtin_type("numberType").unwrap())
                .unwrap()
        };
        if emit_first {
            session.emit(&EmitOptions::default()).unwrap();
        }
        session.retire();
        assert!(matches!(
            session.operation(),
            Err(QueryError::Arena(ArenaError::Retired))
        ));
        assert!(matches!(
            retained.owner().operation(),
            Err(QueryError::Arena(ArenaError::Retired))
        ));
        assert!(matches!(
            session.emit(&EmitOptions::default()),
            Err(SessionError::Checker(QueryError::Arena(
                ArenaError::Retired
            )))
        ));
        drop(session);
        drop(retained);
        assert_eq!(counters.snapshot(), Counts::default());
    }
}

#[test]
fn retirement_before_initialization_refuses_a_query() {
    let counters = Counters::new();
    let session = session(&counters);
    session.retire();
    assert!(matches!(
        session.operation(),
        Err(QueryError::Arena(ArenaError::Retired))
    ));
    drop(session);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn native_query_unwind_preserves_payload_and_invalidates_the_session() {
    let counters = Counters::new();
    let session = session(&counters);
    session.emit(&EmitOptions::default()).unwrap();
    let retained = {
        let operation = session.operation().unwrap();
        operation
            .retain_type(operation.builtin_type("numberType").unwrap())
            .unwrap()
    };
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _operation = session.operation().unwrap();
        std::panic::panic_any(42_u32);
    }));
    assert_eq!(*panic.unwrap_err().downcast::<u32>().unwrap(), 42);
    assert!(matches!(
        session.operation(),
        Err(QueryError::Arena(ArenaError::Retired))
    ));
    assert!(matches!(
        retained.owner().operation(),
        Err(QueryError::Arena(ArenaError::Retired))
    ));
    assert!(matches!(
        session.emit(&EmitOptions::default()),
        Err(SessionError::Checker(QueryError::Arena(
            ArenaError::Retired
        )))
    ));
    drop(session);
    drop(retained);
    assert_eq!(counters.snapshot(), Counts::default());
    // Invalidating one session does not poison a newly loaded session.
    let fresh = self::session(&counters);
    fresh.operation().unwrap();
    fresh.emit(&EmitOptions::default()).unwrap();
    drop(fresh);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn emit_refuses_a_target_file_from_another_program() {
    let counters = Counters::new();
    let session = session(&counters);
    let other = self::session(&counters);
    let foreign = other.program().file(b"/main.ts").unwrap();
    assert!(matches!(
        session.emit(&EmitOptions {
            target_source_files: Some(&[foreign]),
            ..Default::default()
        }),
        Err(SessionError::Ast(ArenaError::WrongOwner))
    ));
    drop(session);
    drop(other);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn concurrent_first_queries_share_a_checker_identity() {
    let counters = Counters::new();
    let session = Arc::new(session(&counters));
    let barrier = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    session.operation().unwrap().owner().identity().id()
                })
            })
            .collect();
        let ids: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(ids.iter().all(|id| *id == ids[0]));
    });
    drop(session);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn malformed_source_keeps_original_bytes_and_resolvable_diagnostics() {
    let counters = Counters::new();
    let input = b"export const value: = ;\n// \xff\n";
    let session = load(input, &mut FileCache::new(), &counters);
    let diagnostics = session.program().syntactic_diagnostics(None).unwrap();
    assert!(!diagnostics.is_empty());
    for diagnostic in &diagnostics {
        let file = session
            .program()
            .file_of_node(diagnostic.file.unwrap())
            .unwrap();
        let source = file.bound().view().source_file().unwrap();
        assert_eq!(source.text().as_bytes(), input);
    }
    drop(session);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn emitter_records_and_write_metadata_retain_their_source_owners() {
    let counters = Counters::new();
    let session = load_with_options(
        INPUT,
        &mut FileCache::new(),
        &counters,
        CompilerOptions {
            no_lib: Tristate::TRUE,
            source_map: Tristate::TRUE,
            ..Default::default()
        },
    );
    let source = session.program().file(b"/main.ts").unwrap().source();
    let weak = Arc::downgrade(session.program());
    let output = session.emit(&EmitOptions::default()).unwrap();
    drop(session);
    assert!(weak.upgrade().is_some());
    assert_eq!(
        output
            .writes
            .iter()
            .map(|write| output.written_file(write).unwrap().name.as_bytes())
            .collect::<Vec<_>>(),
        [b"/main.js.map".as_slice(), b"/main.js".as_slice()]
    );
    for write in &output.writes {
        assert!(write.unlisted_file().is_none());
        assert!(std::ptr::eq(
            output.written_file(write).unwrap(),
            &output.files[write.emitted_file_index.unwrap()],
        ));
        assert_eq!(write.data.source_file, Some(source));
        assert!(!write.data.skipped_dts_write);
        assert!(write.data.build_info.is_none());
        assert!(write.data.diagnostics.is_empty());
        let file = output
            .program()
            .unwrap()
            .file_of_node(write.data.source_file.unwrap())
            .unwrap();
        assert_eq!(
            file.bound().view().source_file().unwrap().text().as_bytes(),
            INPUT
        );
        assert_eq!(
            output
                .files
                .iter()
                .find(|file| file.name == output.written_file(write).unwrap().name)
                .unwrap()
                .text,
            output.written_file(write).unwrap().text
        );
    }
    let js = &output.writes[1];
    let comment = output
        .written_file(js)
        .unwrap()
        .text
        .windows(b"//# sourceMappingURL=".len())
        .position(|part| part == b"//# sourceMappingURL=")
        .unwrap();
    assert_eq!(js.data.source_map_url_pos, comment as isize);
    let map = &output.source_maps[0];
    assert_eq!(map.generated_file.as_bytes(), b"/main.js");
    assert_eq!(
        map.input_source_file_names
            .iter()
            .map(JsString::as_bytes)
            .collect::<Vec<_>>(),
        [b"/main.ts".as_slice()]
    );
    assert!(!map.source_map.mappings.is_empty());
    assert_eq!(map.source_map.file.as_bytes(), b"main.js");
    assert!(output
        .program()
        .unwrap()
        .source_file(map.input_source_file_names[0].as_bytes())
        .is_some());
    drop(output);
    assert!(weak.upgrade().is_none());
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn callback_order_and_inline_maps_are_observed_independently_of_output_files() {
    let counters = Counters::new();
    let mut host = tsr_vfs::MemoryBuilder::new(b"/", true);
    host.insert_loaded(b"/a.ts", b"export const a = 1;".as_slice());
    host.insert_loaded(b"/b.ts", b"export const b = 2;".as_slice());
    let session = Session::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                CompilerOptions {
                    no_lib: Tristate::TRUE,
                    no_check: Tristate::TRUE,
                    inline_source_map: Tristate::TRUE,
                    ..Default::default()
                },
                vec![
                    JsString::from_bytes(b"/a.ts".as_slice()),
                    JsString::from_bytes(b"/b.ts".as_slice()),
                ],
            ),
            host: Arc::new(host.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/lib".as_slice()),
            skip_module_resolution: false,
            single_threaded: Tristate::TRUE,
        },
        &mut FileCache::new(),
        &counters,
    )
    .unwrap();
    let output = session.emit(&EmitOptions::default()).unwrap();
    assert_eq!(
        output
            .files
            .iter()
            .map(|file| file.name.as_bytes())
            .collect::<Vec<_>>(),
        [b"/a.js".as_slice(), b"/b.js".as_slice()]
    );
    // The native single-threaded work group runs the last queued file first;
    // its combined EmitResult still lists files and map records in input order.
    assert_eq!(
        output
            .writes
            .iter()
            .map(|write| output.written_file(write).unwrap().name.as_bytes())
            .collect::<Vec<_>>(),
        [b"/b.js".as_slice(), b"/a.js".as_slice()]
    );
    assert_eq!(
        output
            .source_maps
            .iter()
            .map(|map| map.generated_file.as_bytes())
            .collect::<Vec<_>>(),
        [b"/a.js".as_slice(), b"/b.js".as_slice()]
    );
    assert!(output
        .files
        .iter()
        .all(|file| !file.name.as_bytes().ends_with(b".map")));
    assert_eq!(output.source_maps.len(), 2);
    assert_eq!(
        output
            .writes
            .iter()
            .map(|write| write.emitted_file_index)
            .collect::<Vec<_>>(),
        [Some(1), Some(0)],
    );
    drop(output);
    drop(session);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn compiler_diagnostics_and_queries_share_the_emit_pool() {
    let counters = Counters::new();
    let session = session(&counters);
    let file = session.program().file(b"/main.ts").unwrap();
    let request = SessionRequest::default();
    let diagnostics = session
        .diagnostics(&request, DiagnosticKind::Semantic, Some(file))
        .unwrap();
    assert_eq!(
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
        [2322]
    );
    let retained = session
        .with_compiler_operations(|operations| {
            assert_eq!(operations.len(), 1);
            assert!(!operations.is_empty());
            assert_eq!(operations.iter().count(), 1);
            let operation = operations.for_file(file)?;
            operation.retain_type(operation.builtin_type("stringType").unwrap())
        })
        .unwrap();
    let independent = session.operation().unwrap().owner().clone();
    assert!(!Arc::ptr_eq(retained.owner(), &independent));
    session.emit(&EmitOptions::default()).unwrap();
    session
        .with_compiler_operations(|operations| {
            let operation = operations.for_file(file)?;
            assert!(Arc::ptr_eq(operation.owner(), retained.owner()));
            let typ = operation.import_type(&retained)?;
            assert_eq!(operation.intrinsic_type_name(typ)?.as_bytes(), b"string");
            Ok(())
        })
        .unwrap();
    for kind in [
        DiagnosticKind::Config,
        DiagnosticKind::Program,
        DiagnosticKind::Syntactic,
        DiagnosticKind::Bind,
        DiagnosticKind::Global,
        DiagnosticKind::Declaration,
        DiagnosticKind::Suggestion,
    ] {
        assert!(
            session
                .diagnostics(&request, kind, None)
                .unwrap()
                .is_empty(),
            "{kind:?}"
        );
    }
    let other = self::session(&counters);
    let foreign = other.program().file(b"/main.ts").unwrap();
    assert!(matches!(
        session.diagnostics(&request, DiagnosticKind::Semantic, Some(foreign)),
        Err(SessionError::Ast(ArenaError::WrongOwner))
    ));
    session
        .with_compiler_operations(|operations| {
            assert!(matches!(
                operations.for_file(foreign),
                Err(QueryError::Arena(ArenaError::WrongOwner))
            ));
            assert!(matches!(session.operation(), Err(QueryError::Reentry)));
            assert!(matches!(
                session.emit(&EmitOptions::default()),
                Err(SessionError::Checker(QueryError::Reentry))
            ));
            assert!(matches!(
                session.diagnostics(&request, DiagnosticKind::Global, None),
                Err(SessionError::Checker(QueryError::Reentry))
            ));
            assert!(matches!(
                session.with_compiler_operations(|_| Ok(())),
                Err(SessionError::Checker(QueryError::Reentry))
            ));
            Ok(())
        })
        .unwrap();
    drop(other);
    drop(session);
    let operation = retained.owner().operation().unwrap();
    let typ = operation.import_type(&retained).unwrap();
    assert_eq!(
        operation.intrinsic_type_name(typ).unwrap().as_bytes(),
        b"string"
    );
    drop(operation);
    drop(retained);
    drop(independent);

    let suggestions = load(
        b"export function f() { const unused = 1; return 2; }",
        &mut FileCache::new(),
        &counters,
    );
    assert!(suggestions
        .diagnostics(&request, DiagnosticKind::Semantic, None)
        .unwrap()
        .is_empty());
    assert!(suggestions
        .diagnostics(&request, DiagnosticKind::Suggestion, None)
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic.code == 6133));
    drop(suggestions);
    let declarations = load_with_options(
        b"export const Hidden = class { private secret = 1; };",
        &mut FileCache::new(),
        &counters,
        CompilerOptions {
            no_lib: Tristate::TRUE,
            declaration: Tristate::TRUE,
            ..Default::default()
        },
    );
    assert!(declarations
        .diagnostics(&request, DiagnosticKind::Declaration, None)
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic.code == 4094));
    drop(declarations);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn compiler_scope_panic_retires_both_checker_roots() {
    let counters = Counters::new();
    let session = session(&counters);
    let file = session.program().file(b"/main.ts").unwrap();
    let query_type = {
        let operation = session.operation().unwrap();
        operation
            .retain_type(operation.builtin_type("numberType").unwrap())
            .unwrap()
    };
    let compiler_type = session
        .with_compiler_operations(|operations| {
            let operation = operations.for_file(file)?;
            operation.retain_type(operation.builtin_type("numberType").unwrap())
        })
        .unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), SessionError> = session.with_compiler_operations(|_| {
            std::panic::panic_any(73_u32);
        });
    }));
    assert_eq!(*panic.unwrap_err().downcast::<u32>().unwrap(), 73);
    assert!(matches!(
        session.operation(),
        Err(QueryError::Arena(ArenaError::Retired))
    ));
    assert!(matches!(
        compiler_type.owner().operation(),
        Err(QueryError::Arena(ArenaError::Retired))
    ));
    assert!(matches!(
        query_type.owner().operation(),
        Err(QueryError::Arena(ArenaError::Retired))
    ));
    assert!(matches!(
        session.diagnostics(&SessionRequest::default(), DiagnosticKind::Global, None),
        Err(SessionError::Checker(QueryError::Arena(
            ArenaError::Retired
        )))
    ));
    assert!(matches!(
        session.emit(&EmitOptions::default()),
        Err(SessionError::Checker(QueryError::Arena(
            ArenaError::Retired
        )))
    ));
    drop(session);
    drop(query_type);
    drop(compiler_type);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn compiler_request_cancellation_preserves_native_emit_and_checker_contracts() {
    let counters = Counters::new();
    let session = session(&counters);
    let token = CancellationToken::new();
    token.cancel();
    let request = SessionRequest {
        cancellation: Some(token),
        ..Default::default()
    };
    assert!(session
        .emit_with_request(&request, &EmitOptions::default())
        .unwrap()
        .is_none());
    assert!(!session
        .emit(&EmitOptions::default())
        .unwrap()
        .files
        .is_empty());
    let fresh = self::session(&counters);
    let file = fresh.program().file(b"/main.ts").unwrap();
    assert!(fresh
        .diagnostics(&request, DiagnosticKind::Semantic, Some(file))
        .unwrap()
        .is_empty());
    assert!(matches!(
        fresh.diagnostics(
            &SessionRequest::default(),
            DiagnosticKind::Semantic,
            Some(file)
        ),
        Err(SessionError::Checker(QueryError::PreviouslyCanceled))
    ));
    // Native cancellation poisons the selected compiler checker, without
    // retiring this session's distinct interactive query checker.
    fresh.operation().unwrap();
    fresh.retire();
    assert!(matches!(
        fresh.with_compiler_operations(|_| Ok(())),
        Err(SessionError::Checker(QueryError::Arena(
            ArenaError::Retired
        )))
    ));
    drop(session);
    drop(fresh);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn transpile_results_retain_sources_and_follow_pinned_cancellation() {
    let token = CancellationToken::new();
    token.cancel();
    let canceled = TranspileRequest {
        cancellation: Some(token),
        ..Default::default()
    };
    assert!(
        transpile_module(&canceled, INPUT, &TranspileOptions::default())
            .unwrap()
            .is_none()
    );
    let declaration = transpile_declaration(&canceled, INPUT, &TranspileOptions::default())
        .unwrap()
        .unwrap();
    assert_eq!(
        declaration.output_text.as_bytes(),
        b"export declare const value: string;\n"
    );
    let weak = Arc::downgrade(&declaration.program);
    drop(declaration);
    assert!(weak.upgrade().is_none());

    let module = {
        let input = b"export const a string = 1;\n".to_vec();
        transpile_module(
            &TranspileRequest::default(),
            &input,
            &TranspileOptions {
                report_diagnostics: true,
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap()
    };
    assert_eq!(
        module.output_text.as_bytes(),
        b"export const a, string = 1;\n"
    );
    assert_eq!(
        module
            .diagnostics
            .iter()
            .map(|d| d.code)
            .collect::<Vec<_>>(),
        [1005]
    );
    let file = module
        .program
        .file_of_node(module.diagnostics[0].file.unwrap())
        .unwrap();
    assert_eq!(
        file.bound().view().source_file().unwrap().text().as_bytes(),
        b"export const a string = 1;\n"
    );
}

fn bound_local(file: &tsr_ast::CompletedFile, name: &[u8]) -> tsr_arena::SymbolId {
    let view = file.view();
    let locals = view
        .node_binding(file.source())
        .unwrap()
        .unwrap()
        .locals
        .unwrap();
    view.result()
        .tables()
        .get(locals)
        .unwrap()
        .get(name)
        .unwrap()
        .unwrap()
}

#[test]
fn public_ownership_shared_file_survives_concurrent_snapshot_edit() {
    let counters = Counters::new();
    let mut cache = FileCache::new();
    let old = load(b"const before = 1;", &mut cache, &counters);
    let source = old.program().file(b"/main.ts").unwrap().source();
    let file = old
        .program()
        .files()
        .iter()
        .find(|file| file.source() == source)
        .unwrap()
        .clone();
    let symbol = bound_local(file.bound(), b"before");
    let response = file.bound().retain_symbol(symbol).unwrap();
    let unchanged = load(b"const before = 1;", &mut cache, &counters);
    let shared = unchanged
        .program()
        .files()
        .iter()
        .find(|file| file.source() == source)
        .unwrap();
    assert!(Arc::ptr_eq(&file, shared));
    assert_eq!(bound_local(shared.bound(), b"before"), symbol);
    drop(old);
    drop(unchanged);
    cache.prune();

    let ready = std::sync::Barrier::new(2);
    let edited = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            ready.wait();
            assert_eq!(bound_local(response.file(), b"before"), symbol);
            assert_eq!(
                response
                    .file()
                    .view()
                    .source_file()
                    .unwrap()
                    .text()
                    .as_bytes(),
                b"const before = 1;"
            );
            edited.wait();
            assert_eq!(response.symbol().name_bytes(), b"before");
            assert!(response
                .file()
                .view()
                .node(response.symbol().value_declaration().unwrap())
                .is_ok());
        });
        ready.wait();
        let new = load(b"const after = 2;", &mut cache, &counters);
        let new_file = new.program().file(b"/main.ts").unwrap();
        let new_symbol = bound_local(new_file.bound(), b"after");
        assert_ne!(source, new_file.source());
        assert_ne!(symbol, new_symbol);
        assert!(matches!(
            new_file.bound().view().node(source),
            Err(ArenaError::WrongOwner)
        ));
        assert!(matches!(
            file.bound().view().node(new_file.source()),
            Err(ArenaError::WrongOwner)
        ));
        assert!(matches!(
            new_file.bound().view().symbol(symbol),
            Err(ArenaError::WrongOwner)
        ));
        assert!(matches!(
            file.bound().view().symbol(new_symbol),
            Err(ArenaError::WrongOwner)
        ));
        drop(new);
        cache.prune();
        edited.wait();
        reader.join().unwrap();
    });
    drop(file);
    cache.prune();
    assert_eq!(bound_local(response.file(), b"before"), symbol);
    drop(response);
    cache.prune();
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn public_ownership_retained_results_and_generated_syntax_keep_exact_owners() {
    let counters = Counters::new();
    let session = load(
        b"interface Shared { field: string }",
        &mut FileCache::new(),
        &counters,
    );
    let weak_program = Arc::downgrade(session.program());
    let file = session.program().file(b"/main.ts").unwrap();
    let view = file.bound().view().ast();
    let declaration = view
        .node_slice(view.node(file.source()).unwrap().statements(view).unwrap())
        .unwrap()
        .iter()
        .flatten()
        .next()
        .unwrap();
    let name = view.node(declaration).unwrap().name().unwrap();
    let (ty, symbol, signature, list, node, mut syntax, generated) = {
        let mut operation = session.operation().unwrap();
        let symbol = operation.get_symbol_at_location(name).unwrap().unwrap();
        let ty = operation.get_declared_type_of_symbol(symbol).unwrap();
        let string = operation.builtin_type("stringType").unwrap();
        let signature = operation.call_signature(&[], ty).unwrap();
        let node = operation
            .synthetic_expression_at(declaration, ty, false)
            .unwrap();
        let retained = (
            operation.retain_type(ty).unwrap(),
            operation.retain_symbol_ref(symbol).unwrap(),
            operation.retain_signature(signature).unwrap(),
            operation.retain_type_list(&[ty, string]).unwrap(),
            operation.retain_node(node).unwrap(),
        );
        let mut builder = operation.node_builder();
        builder.retain_source_node(file.source()).unwrap();
        let generated = builder
            .type_to_type_node(ty, Some(file.source()), 0, 0)
            .unwrap()
            .unwrap();
        let syntax = builder.into_syntax();
        (
            retained.0, retained.1, retained.2, retained.3, retained.4, syntax, generated,
        )
    };
    // A different checker over the identical Program must reject every family.
    let foreign = Session::from_program(session.program().clone(), &counters);
    {
        let operation = foreign.operation().unwrap();
        let wrong = Some(QueryError::Arena(ArenaError::WrongOwner));
        assert_eq!(operation.import_symbol_ref(&symbol).err(), wrong);
        assert_eq!(operation.import_signature(&signature).err(), wrong);
        assert_eq!(operation.import_type_list(&list).err(), wrong);
        assert_eq!(operation.import_node(&node).err(), wrong);
    }
    drop(foreign);
    drop(session);
    assert!(weak_program.upgrade().is_some());
    {
        let mut operation = node.owner().operation().unwrap();
        let typ = operation.import_type(&ty).unwrap();
        let imported_symbol = operation.import_symbol_ref(&symbol).unwrap();
        assert_eq!(
            operation
                .get_declared_type_of_symbol(imported_symbol)
                .unwrap(),
            typ
        );
        assert_eq!(operation.import_type_list(&list).unwrap()[0], typ);
        let imported_node = operation.import_node(&node).unwrap();
        assert_eq!(
            operation.synthetic_expression_type(imported_node).unwrap(),
            typ
        );
        assert_eq!(
            operation.node_parent(imported_node).unwrap().unwrap().0,
            declaration
        );
        let imported_signature = operation.import_signature(&signature).unwrap();
        assert_eq!(
            operation.signature_return_type(imported_signature).unwrap(),
            Some(typ)
        );
        let signature_node = operation
            .signature_declaration(imported_signature)
            .unwrap()
            .unwrap();
        assert_eq!(
            operation.node_kind(signature_node).unwrap().known(),
            Some(tsr_ast::SyntaxKind::FunctionType)
        );
    }
    // Transferred syntax owns its AST independently of checker permits.
    assert!(syntax.ast.view().node(generated).is_ok());
    let cloned = syntax.clone_node(Some(generated)).unwrap();
    assert!(syntax.ast.view().node(cloned).is_ok());
    assert_eq!(
        syntax.ast.view().node(declaration).unwrap().kind().known(),
        Some(tsr_ast::SyntaxKind::InterfaceDeclaration)
    );
    drop(ty);
    drop(symbol);
    drop(list);
    drop(node);
    assert!(
        weak_program.upgrade().is_some(),
        "the signature is still a checker root"
    );
    drop(signature);
    assert!(weak_program.upgrade().is_none());
    assert!(
        syntax.ast.view().node(cloned).is_ok(),
        "the transferred AST still owns its source graph"
    );
    drop(syntax);
    assert_eq!(counters.snapshot(), Counts::default());
}
