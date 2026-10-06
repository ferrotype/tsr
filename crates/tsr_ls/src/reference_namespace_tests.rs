use crate::LanguageService;
use std::sync::Arc;
use tsr_core::CancellationToken;
use tsr_jsstring::{JsString, PositionEncoding};
use tsr_lsproto as lsp;

// source: findAllRefsForUMDModuleAlias1_test.go
#[test]
fn umd_namespace_references_do_not_include_module_path_strings() {
    let declaration = "export function doThing(): string; export as namespace myLib;";
    let consumer = "/// <reference path=\"0.d.ts\" />\nmyLib.doThing();";
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/0.d.ts", declaration.as_bytes());
    fs.insert_loaded(b"/1.ts", consumer.as_bytes());
    let counters = tsr_arena::Counters::new();
    let program = Arc::new(
        tsr_compiler::Program::load(
            tsr_compiler::ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    tsr_core::CompilerOptions {
                        no_lib: tsr_core::Tristate::TRUE,
                        ..Default::default()
                    },
                    vec![
                        JsString::from_bytes(b"/0.d.ts".as_slice()),
                        JsString::from_bytes(b"/1.ts".as_slice()),
                    ],
                ),
                host: Arc::new(fs.finish()),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(b"/".as_slice()),
                skip_module_resolution: false,
                single_threaded: tsr_core::Tristate::TRUE,
            },
            &mut tsr_compiler::FileCache::new(),
            &counters,
        )
        .unwrap(),
    );
    let source = program.source_file(b"/0.d.ts").unwrap().source();
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
    let mut checker = pool.checker_for_file_exclusive(source).unwrap();
    let mut service =
        LanguageService::new(&program, PositionEncoding::Utf16, CancellationToken::new());
    let locations = service
        .references(
            &mut checker,
            &lsp::ReferenceParams {
                text_document: lsp::TextDocumentIdentifier {
                    uri: lsp::DocumentUri("file:///0.d.ts".into()),
                },
                position: lsp::Position {
                    line: 0,
                    character: declaration.find("myLib").unwrap() as u32,
                },
                context: Some(Box::new(lsp::ReferenceContext {
                    include_declaration: true,
                })),
                ..Default::default()
            },
        )
        .unwrap()
        .locations
        .unwrap();
    assert_eq!(locations.len(), 2);
    assert!(locations
        .iter()
        .any(|location| location.uri.0 == "file:///0.d.ts"));
    assert!(locations
        .iter()
        .any(|location| location.uri.0 == "file:///1.ts" && location.range.start.line == 1));
    assert!(!locations
        .iter()
        .any(|location| location.uri.0 == "file:///1.ts" && location.range.start.line == 0));
}
