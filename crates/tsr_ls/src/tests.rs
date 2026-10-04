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

fn hover_result(text: &[u8], position: u32, classified: bool) -> lsp::Hover {
    let program = Arc::new(program(b"/index.ts", text));
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &tsr_arena::Counters::new());
    let mut checker = pool
        .checker_for_file_exclusive(program.source_file(b"/index.ts").unwrap().source())
        .unwrap();
    let mut service = LanguageService::new(
        &program,
        tsr_jsstring::PositionEncoding::Utf16,
        CancellationToken::new(),
    );
    *service
        .hover(
            &mut checker,
            &lsp::HoverParams {
                text_document: lsp::TextDocumentIdentifier {
                    uri: lsp::DocumentUri("file:///index.ts".into()),
                },
                position: lsp::Position {
                    line: 0,
                    character: position,
                },
                ..Default::default()
            },
            HoverOptions {
                markdown: true,
                classified,
                ..Default::default()
            },
        )
        .unwrap()
        .hover
        .unwrap()
}

#[test]
fn classified_hover_preserves_the_type_reference_symbol() {
    // Native hover carries the interface identity through createAccessFromSymbolChain.
    let hover = hover_result(
        b"interface Shape { value: number } const shape: Shape = {value: 1};",
        41,
        true,
    );
    assert_eq!(
        hover.contents.markup_content.as_ref().unwrap().value,
        "```typescript\nconst shape: Shape\n```\n"
    );
    let content = hover.vs_raw_content.unwrap();
    let runs = &content.elements[1]
        .classified_text_element
        .as_ref()
        .unwrap()
        .runs;
    assert!(runs
        .iter()
        .flatten()
        .any(|r| r.text == "Shape" && r.classification_type_name == "interface name"));
}

#[test]
fn hover_string_property_range_excludes_quotes() {
    let hover = hover_result(b"const obj = {\"name\": 1};", 15, false);
    let range = hover.range.unwrap();
    assert_eq!((range.start.character, range.end.character), (14, 18));
}

#[test]
fn hover_type_parameter_reports_its_function_context() {
    let hover = hover_result(
        b"function f<T extends number>(value: T): T { return value; }",
        11,
        false,
    );
    assert_eq!(hover.contents.markup_content.unwrap().value,"```typescript\n(type parameter) T extends number in f<T extends number>(value: T): T\n```\n");
}

#[test]
fn hover_code_fences_do_not_close_inside_literal_types() {
    let mut rendered = String::new();
    documentation::code(&mut rendered, "typescript", "const text: \"```\"");
    assert_eq!(rendered, "````typescript\nconst text: \"```\"\n````\n");
}

fn signature_result(text: &str, options: &SignatureHelpOptions) -> lsp::SignatureHelp {
    let offset = text.find('|').unwrap();
    let text = text.replacen('|', "", 1);
    let program = Arc::new(program(b"/index.ts", text.as_bytes()));
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &tsr_arena::Counters::new());
    let mut checker = pool
        .checker_for_file_exclusive(program.source_file(b"/index.ts").unwrap().source())
        .unwrap();
    let mut service = LanguageService::new(
        &program,
        tsr_jsstring::PositionEncoding::Utf16,
        CancellationToken::new(),
    );
    *service
        .signature_help(
            &mut checker,
            &lsp::SignatureHelpParams {
                text_document: lsp::TextDocumentIdentifier {
                    uri: lsp::DocumentUri("file:///index.ts".into()),
                },
                position: lsp::Position {
                    line: 0,
                    character: offset as u32,
                },
                context: Some(Box::new(lsp::SignatureHelpContext {
                    trigger_kind: lsp::SignatureHelpTriggerKind::INVOKED,
                    ..Default::default()
                })),
                ..Default::default()
            },
            options,
        )
        .unwrap()
        .signature_help
        .unwrap()
}
#[test]
fn signature_type_parameter_constraints_use_the_pins_printer_context() {
    // Full native responses are also compared in tools/phase5/lsp/signature_help.py.
    let help = signature_result(
        "declare function f<T extends {x:number}>(x:T):T; f<|",
        &SignatureHelpOptions::default(),
    );
    let sig = help.signatures[0].as_ref().unwrap();
    assert_eq!(sig.label, "f<T extends {\n    x: number;\n}>(x: T): T");
    assert_eq!(
        sig.parameters.as_ref().unwrap()[0]
            .as_ref()
            .unwrap()
            .label
            .string
            .as_deref()
            .unwrap()
            .as_str(),
        "T extends {\n    x: number;\n}"
    );
}
#[test]
fn signature_middle_rest_respects_null_active_parameter_capability() {
    let text="interface Array<T>{length:number;[n:number]:T} declare function f(...args:[prefix:string,...middle:number[],suffix:boolean]):void; f(\"\",1,|2,true);";
    let help = signature_result(
        text,
        &SignatureHelpOptions {
            per_signature_active_parameter: true,
            null_active_parameter: true,
            ..Default::default()
        },
    );
    assert!(help.active_parameter.is_none());
    let sig = help.signatures[0].as_ref().unwrap();
    assert_eq!(
        sig.label,
        "f(prefix: string, ...middle: number[], suffix: boolean): void"
    );
    assert!(sig.active_parameter.as_ref().unwrap().uinteger.is_none());
    let fallback = signature_result(text, &SignatureHelpOptions::default());
    assert_eq!(
        fallback
            .active_parameter
            .as_ref()
            .unwrap()
            .uinteger
            .as_deref(),
        Some(&3)
    );
}

#[test]
fn inlay_parameter_name_links_to_its_declaration() {
    let text = b"declare function f(value:number):void; f(1);";
    let program = Arc::new(program(b"/index.ts", text));
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &tsr_arena::Counters::new());
    let mut checker = pool
        .checker_for_file_exclusive(program.source_file(b"/index.ts").unwrap().source())
        .unwrap();
    let mut service = LanguageService::new(
        &program,
        tsr_jsstring::PositionEncoding::Utf16,
        CancellationToken::new(),
    );
    let params = lsp::InlayHintParams {
        text_document: lsp::TextDocumentIdentifier {
            uri: lsp::DocumentUri("file:///index.ts".into()),
        },
        range: lsp::Range {
            start: lsp::Position::default(),
            end: lsp::Position {
                line: 0,
                character: text.len() as u32,
            },
        },
        ..Default::default()
    };
    assert!(service
        .inlay_hints(&mut checker, &params, InlayHintsOptions::default())
        .unwrap()
        .inlay_hints
        .is_none());
    let hints = service
        .inlay_hints(
            &mut checker,
            &params,
            InlayHintsOptions {
                parameter_names: ParameterNameHints::All,
                ..Default::default()
            },
        )
        .unwrap()
        .inlay_hints
        .unwrap();
    assert_eq!(hints.len(), 1);
    let hint = hints[0].as_ref().unwrap();
    assert_eq!(
        hint.position,
        lsp::Position {
            line: 0,
            character: 41
        }
    );
    let parts = hint.label.inlay_hint_label_parts.as_ref().unwrap();
    assert_eq!(
        parts
            .iter()
            .flatten()
            .map(|p| p.value.as_str())
            .collect::<Vec<_>>(),
        ["value", ":"]
    );
    let location = parts[0].as_ref().unwrap().location.as_ref().unwrap();
    assert_eq!(location.uri.0, "file:///index.ts");
    assert_eq!(
        (location.range.start.character, location.range.end.character),
        (19, 24)
    );
    assert_eq!(hint.padding_right.as_deref(), Some(&true));
}
