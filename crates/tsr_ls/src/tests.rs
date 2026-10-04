use super::*;
use std::sync::Arc;

fn program(name: &[u8], text: &[u8]) -> Program {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(name, text);
    let options = tsr_core::CompilerOptions {
        no_lib: tsr_core::Tristate::TRUE,
        ..Default::default()
    };
    Program::load(
        tsr_compiler::ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                options,
                vec![tsr_jsstring::JsString::from_bytes(name)],
            ),
            host: Arc::new(fs.finish()),
            current_directory: tsr_jsstring::JsString::from_bytes(b"/".as_slice()),
            default_library_path: tsr_jsstring::JsString::from_bytes(b"/".as_slice()),
            skip_module_resolution: false,
            single_threaded: tsr_core::Tristate::TRUE,
        },
        &mut tsr_compiler::FileCache::new(),
        &tsr_arena::Counters::new(),
    )
    .unwrap()
}

// source: tsc/internal/ls/selectionranges_test.go:TestSelectionRangeDepthIsLimited
#[test]
fn selection_range_depth_is_limited() {
    let depth = 12_000;
    let text = format!("const x = {}1{};", "(".repeat(depth), ")".repeat(depth));
    let program = program(b"/index.ts", text.as_bytes());
    let mut service = LanguageService::new(
        &program,
        tsr_jsstring::PositionEncoding::Utf16,
        CancellationToken::new(),
    );
    let result = service
        .selection_ranges(&lsp::SelectionRangeParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: lsp::DocumentUri("file:///index.ts".into()),
            },
            positions: vec![lsp::Position {
                line: 0,
                character: ("const x = ".len() + depth) as u32,
            }],
            ..Default::default()
        })
        .unwrap();
    let mut range = result.selection_ranges.unwrap().pop().unwrap();
    assert_eq!(
        range.as_ref().unwrap().range.start.character,
        (10 + depth) as u32
    );
    assert_eq!(
        range.as_ref().unwrap().range.end.character,
        (11 + depth) as u32
    );
    let mut count = 0;
    while let Some(mut current) = range {
        count += 1;
        range = current.parent.take();
        if range.is_none() {
            assert_eq!(current.range.start.character, 0);
            assert_eq!(current.range.end.character, text.len() as u32);
        }
    }
    assert_eq!(count, 1000);
}

#[test]
fn syntax_request_observes_cancellation() {
    let program = program(b"/index.ts", b"const x = 1;");
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let mut service = LanguageService::new(
        &program,
        tsr_jsstring::PositionEncoding::Utf16,
        cancellation,
    );
    assert!(matches!(
        service.folding_ranges(
            &lsp::DocumentUri("file:///index.ts".into()),
            FoldingOptions::default()
        ),
        Err(Error::Canceled)
    ));
}

#[test]
fn navigation_tokens_keep_their_program_owner() {
    let program = program(b"/index.ts", b"const x = 1;");
    let file = program.source_file(b"/index.ts").unwrap();
    let mut syntax = syntax::Syntax::new(file.bound().view().ast(), file.source()).unwrap();
    let token = syntax.nav().get_touching_property_name(0).unwrap();
    assert_eq!(
        syntax.view.node(token).unwrap().kind(),
        tsr_ast::SyntaxKind::ConstKeyword
    );
    assert_ne!(token.arena(), file.source().arena());
    assert_eq!(program.file_of_node(token).unwrap().source(), file.source());
    // Repeated reads exercise the cached namespace, without retaining another
    // AST owner or accepting a token from a different program generation.
    assert_eq!(program.file_of_node(token).unwrap().source(), file.source());
    let foreign = self::program(b"/index.ts", b"const x = 1;");
    assert!(foreign.file_of_node(token).is_none());
}

#[test]
fn modifiers_do_not_form_a_selection_list() {
    let program = program(b"/index.ts", b"class C { static readonly value = 1; }");
    let mut service = LanguageService::new(
        &program,
        tsr_jsstring::PositionEncoding::Utf16,
        CancellationToken::new(),
    );
    let result = service
        .selection_ranges(&lsp::SelectionRangeParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: lsp::DocumentUri("file:///index.ts".into()),
            },
            positions: vec![lsp::Position {
                line: 0,
                character: 12,
            }],
            ..Default::default()
        })
        .unwrap();
    let range = result.selection_ranges.unwrap().pop().unwrap().unwrap();
    assert_eq!(
        (range.range.start.character, range.range.end.character),
        (10, 16)
    );
    let property = range.parent.unwrap();
    assert_eq!(
        (property.range.start.character, property.range.end.character),
        (10, 36)
    );
}
