use crate::{LanguageService, SignatureHelpOptions};
use std::sync::Arc;
use tsr_core::{CancellationToken, CompilerOptions, Tristate};
use tsr_jsstring::{JsString, PositionEncoding};
use tsr_lsproto as lsp;

// source: tsc/internal/fourslash/tests/signatureHelpJSMissingPropertyAccess_test.go:TestSignatureHelpJSMissingPropertyAccess
#[test]
fn js_fallback_signature_keeps_invocation_owner() {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/test.js", b"foo.filter();".as_slice());
    fs.insert_loaded(
        b"/api.d.ts",
        b"declare class Collection { filter(value: string): number; }".as_slice(),
    );
    let counters = tsr_arena::Counters::new();
    let program = Arc::new(
        tsr_compiler::Program::load(
            tsr_compiler::ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        no_lib: Tristate::TRUE,
                        allow_js: Tristate::TRUE,
                        check_js: Tristate::TRUE,
                        ..Default::default()
                    },
                    vec![
                        JsString::from_bytes(b"/test.js".as_slice()),
                        JsString::from_bytes(b"/api.d.ts".as_slice()),
                    ],
                ),
                host: Arc::new(fs.finish()),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(b"/".as_slice()),
                skip_module_resolution: false,
                single_threaded: Tristate::TRUE,
            },
            &mut tsr_compiler::FileCache::new(),
            &counters,
        )
        .unwrap(),
    );
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
    let mut checker = pool
        .checker_for_file_exclusive(program.source_file(b"/test.js").unwrap().source())
        .unwrap();
    let mut service =
        LanguageService::new(&program, PositionEncoding::Utf16, CancellationToken::new());
    for classified in [false, true] {
        let result = service
            .signature_help(
                &mut checker,
                &lsp::SignatureHelpParams {
                    text_document: lsp::TextDocumentIdentifier {
                        uri: lsp::DocumentUri("file:///test.js".into()),
                    },
                    position: lsp::Position {
                        line: 0,
                        character: 11,
                    },
                    ..Default::default()
                },
                &SignatureHelpOptions {
                    classified,
                    ..Default::default()
                },
            )
            .unwrap()
            .signature_help
            .unwrap();
        assert_eq!(result.signatures.len(), 1);
        assert!(result.signatures[0]
            .as_ref()
            .unwrap()
            .label
            .contains("filter(value: string): number"));
    }
}
