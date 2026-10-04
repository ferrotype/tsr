use super::*;
use std::sync::Arc;

fn program(name: &[u8], text: &[u8]) -> Program {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(name, text);
    let options = tsr_core::CompilerOptions {
        no_lib: tsr_core::Tristate::TRUE,
        allow_js: tsr_core::Tristate::from(name.ends_with(b".js")),
        check_js: tsr_core::Tristate::from(name.ends_with(b".js")),
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

// source: tsc/internal/ls/findallreferences_test.go:TestImplementationsWorklistDoesNotBlowUp
#[test]
fn implementations_retain_only_distinct_worklist_entries() {
    let measure = |count: usize| {
        let mut text = String::from("interface I { m(): void; }\n");
        for i in 0..count {
            use std::fmt::Write;
            writeln!(text, "const a{i}: I = {{ m() {{}} }};").unwrap();
        }
        text.push_str("declare const i: I;\ni.m();\n");
        let program = Arc::new(program(b"/index.ts", text.as_bytes()));
        let pool =
            tsr_compiler::CompilerCheckerPool::new(program.clone(), &tsr_arena::Counters::new());
        let source = program.source_file(b"/index.ts").unwrap().source();
        let mut checker = pool.checker_for_file_exclusive(source).unwrap();
        let mut service = LanguageService::new(
            &program,
            tsr_jsstring::PositionEncoding::Utf16,
            CancellationToken::new(),
        );
        let pos = (text.rfind("i.m").unwrap() + 2) as i64;
        let node = syntax::Syntax::new(service.view(source).unwrap(), source)
            .unwrap()
            .nav()
            .get_touching_property_name(pos)
            .unwrap();
        // Inspect retained internal entries, before LSP deduplication, as the
        // native regression does. The queue only receives these unique nodes;
        // this port does not retain an additional SymbolsAndEntries group list.
        let entries = service
            .implementation_entries(&mut checker, node, pos)
            .unwrap();
        let unique: std::collections::HashSet<_> =
            entries.iter().map(|entry| entry.node.unwrap()).collect();
        assert_eq!(unique.len(), entries.len());
        assert!(entries.len() >= count);
        entries.len()
    };
    let small = measure(40);
    let large = measure(80);
    assert!(
        large <= small * 3,
        "retained entries grew from {small} to {large}"
    );
}

#[test]
fn semantic_reference_search_observes_cancellation() {
    let program = Arc::new(program(b"/index.ts", b"const value = 1; value;"));
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &tsr_arena::Counters::new());
    let source = program.source_file(b"/index.ts").unwrap().source();
    let mut checker = pool.checker_for_file_exclusive(source).unwrap();
    let cancellation = CancellationToken::new();
    let mut service = LanguageService::new(
        &program,
        tsr_jsstring::PositionEncoding::Utf16,
        cancellation.clone(),
    );
    cancellation.cancel();
    let node = syntax::Syntax::new(service.view(source).unwrap(), source)
        .unwrap()
        .nav()
        .get_touching_property_name(6)
        .unwrap();
    assert!(matches!(
        service.implementation_entries(&mut checker, node, 6),
        Err(Error::Canceled)
    ));
}

fn source_map_program(map: &[u8]) -> Program {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(
        b"/index.ts",
        b"import {foo} from './lib'; foo();".as_slice(),
    );
    fs.insert_loaded(
        b"/lib.d.ts",
        b"export declare function foo(): void;\n//# sourceMappingURL=lib.d.ts.map".as_slice(),
    );
    fs.insert_loaded(b"/lib.d.ts.map", map);
    fs.insert_loaded(
        b"/source.ts",
        b"/** source */\nexport function foo() {}\n".as_slice(),
    );
    Program::load(
        tsr_compiler::ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                tsr_core::CompilerOptions {
                    no_lib: tsr_core::Tristate::TRUE,
                    ..Default::default()
                },
                vec![tsr_jsstring::JsString::from_bytes(b"/index.ts".as_slice())],
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

#[test]
fn declaration_maps_load_navigation_targets_without_mutating_the_program() {
    let program=Arc::new(source_map_program(br#"{"version":3,"file":"lib.d.ts","sources":["source.ts"],"names":[],"mappings":"wBACgB,GAAG"}"#));
    let source = program.source_file(b"/index.ts").unwrap().source();
    assert!(program.source_file(b"/source.ts").is_none());
    let count = program.files().len();
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &tsr_arena::Counters::new());
    let mut checker = pool.checker_for_file_exclusive(source).unwrap();
    let uri = lsp::DocumentUri("file:///index.ts".into());
    let mut service = LanguageService::new(
        &program,
        tsr_jsstring::PositionEncoding::Utf16,
        CancellationToken::new(),
    );
    for source_definition in [false, true] {
        let result = if source_definition {
            service.source_definition(
                &mut checker,
                &uri,
                &lsp::Position {
                    line: 0,
                    character: 8,
                },
                true,
            )
        } else {
            service.definition(
                &mut checker,
                &uri,
                &lsp::Position {
                    line: 0,
                    character: 8,
                },
                false,
                true,
            )
        }
        .unwrap();
        let links = result.definition_links.unwrap();
        assert_eq!(links.len(), 1);
        let link = links[0].as_ref().unwrap();
        assert_eq!(link.target_uri.0, "file:///source.ts");
        assert_eq!(
            (
                link.target_selection_range.start.line,
                link.target_selection_range.start.character,
                link.target_selection_range.end.character
            ),
            (1, 16, 19)
        );
    }
    assert_eq!(program.files().len(), count);
    assert!(program.source_file(b"/source.ts").is_none());
}

#[test]
fn cyclic_declaration_maps_fall_back_without_recursing() {
    let program = source_map_program(
        br#"{"version":3,"file":"lib.d.ts","sources":["lib.d.ts"],"names":[],"mappings":"AAAA"}"#,
    );
    let mut service = LanguageService::new(
        &program,
        tsr_jsstring::PositionEncoding::Utf16,
        CancellationToken::new(),
    );
    assert!(service.source_position(b"/lib.d.ts", 0).is_none());
}

#[test]
fn commonjs_module_names_have_local_reference_entries() {
    let text =
        b"const item=1; exports.item=item; module.exports.fn=function fn(p){return p+item;};";
    let program = Arc::new(program(b"/index.js", text));
    let source = program.source_file(b"/index.js").unwrap().source();
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &tsr_arena::Counters::new());
    let mut checker = pool.checker_for_file_exclusive(source).unwrap();
    let mut service = LanguageService::new(
        &program,
        tsr_jsstring::PositionEncoding::Utf16,
        CancellationToken::new(),
    );
    let result = service
        .references(
            &mut checker,
            &lsp::ReferenceParams {
                text_document: lsp::TextDocumentIdentifier {
                    uri: lsp::DocumentUri("file:///index.js".into()),
                },
                position: lsp::Position {
                    line: 0,
                    character: 14,
                },
                context: Some(Box::new(lsp::ReferenceContext {
                    include_declaration: true,
                })),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(result.locations.unwrap().len(), 1);
}

#[test]
fn jsdoc_names_query_the_bound_reparsed_declarations() {
    let text = b"/** @typedef {{value: number}} Shape */\n/** @param {Shape} obj */\nfunction f(obj){return obj.value;}\nconst shape={value:1}; f(shape);";
    let program = Arc::new(program(b"/index.js", text));
    let source = program.source_file(b"/index.js").unwrap().source();
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &tsr_arena::Counters::new());
    let mut checker = pool.checker_for_file_exclusive(source).unwrap();
    let mut service = LanguageService::new(
        &program,
        tsr_jsstring::PositionEncoding::Utf16,
        CancellationToken::new(),
    );
    for (character, expected) in [
        (15, [(0, 15, 20), (2, 27, 32)]),
        (31, [(0, 31, 36), (1, 12, 17)]),
    ] {
        let result = service
            .references(
                &mut checker,
                &lsp::ReferenceParams {
                    text_document: lsp::TextDocumentIdentifier {
                        uri: lsp::DocumentUri("file:///index.js".into()),
                    },
                    position: lsp::Position { line: 0, character },
                    context: Some(Box::new(lsp::ReferenceContext {
                        include_declaration: true,
                    })),
                    ..Default::default()
                },
            )
            .unwrap();
        let ranges: Vec<_> = result
            .locations
            .unwrap()
            .iter()
            .map(|l| {
                (
                    l.range.start.line,
                    l.range.start.character,
                    l.range.end.character,
                )
            })
            .collect();
        assert_eq!(ranges, expected);
    }
    let mut syntax = syntax::Syntax::new(service.view(source).unwrap(), source).unwrap();
    let declaration = syntax.nav().get_touching_property_name(15).unwrap();
    let access = syntax.nav().get_touching_property_name(93).unwrap();
    assert_eq!(
        checker.get_type_at_location(declaration).unwrap(),
        checker.get_type_at_location(access).unwrap()
    );
}

#[test]
fn source_definition_fast_path_does_not_acquire_a_checker() {
    struct NoChecker;
    impl QueryChecker for NoChecker {
        fn with_checker<T>(
            &mut self,
            _: NodeId,
            _: impl FnOnce(&mut tsr_checker::Operation<'_>) -> Result<T>,
        ) -> Result<T> {
            panic!("syntactic source definition requested a checker");
        }
    }
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(
        b"/index.ts",
        b"import {foo} from './lib'; foo();".as_slice(),
    );
    fs.insert_loaded(b"/lib.ts", b"export function foo() {}".as_slice());
    let program = Program::load(
        tsr_compiler::ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                tsr_core::CompilerOptions {
                    no_lib: tsr_core::Tristate::TRUE,
                    ..Default::default()
                },
                vec![tsr_jsstring::JsString::from_bytes(b"/index.ts".as_slice())],
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
    .unwrap();
    let mut service = LanguageService::new(
        &program,
        tsr_jsstring::PositionEncoding::Utf16,
        CancellationToken::new(),
    );
    for character in [8, 20] {
        let result = service
            .source_definition(
                &mut NoChecker,
                &lsp::DocumentUri("file:///index.ts".into()),
                &lsp::Position { line: 0, character },
                false,
            )
            .unwrap();
        let locations = result.locations.unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].uri.0, "file:///lib.ts");
    }
}

fn completion_result(text: &str, options: &CompletionOptions) -> lsp::CompletionList {
    let offset = text.find("/*cursor*/").unwrap();
    let text = text.replacen("/*cursor*/", "", 1);
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
    let prefix = &text[..offset];
    let line_start = prefix.rfind('\n').map_or(0, |i| i + 1);
    *service
        .completion(
            &mut checker,
            &lsp::CompletionParams {
                text_document: lsp::TextDocumentIdentifier {
                    uri: lsp::DocumentUri("file:///index.ts".into()),
                },
                position: lsp::Position {
                    line: prefix.bytes().filter(|&c| c == b'\n').count() as u32,
                    character: prefix[line_start..].encode_utf16().count() as u32,
                },
                ..Default::default()
            },
            options,
        )
        .unwrap()
        .list
        .unwrap()
}

#[test]
fn completion_members_keep_native_sort_keys_kinds_and_utf16_replacement() {
    // The complete response is compared against the pin by completions.py.
    let list = completion_result("/*😀*/ interface Item { required: string; optional?: number; method(): void } declare const item: Item; item.op/*cursor*/tional", &CompletionOptions { default_edit_range: true, default_commit_characters: true, commit_characters: true, ..Default::default() });
    let items = list.items.iter().flatten().collect::<Vec<_>>();
    assert_eq!(items.len(), 3);
    for item in items {
        assert_eq!(item.sort_text.as_deref().unwrap(), "11");
        assert_eq!(
            item.kind.as_deref(),
            Some(if item.label == "method" {
                &lsp::CompletionItemKind::METHOD
            } else {
                &lsp::CompletionItemKind::FIELD
            })
        );
    }
    let ranges = list
        .item_defaults
        .unwrap()
        .edit_range
        .unwrap()
        .edit_range_with_insert_replace
        .unwrap();
    assert_eq!(
        ranges.replace.end.character - ranges.replace.start.character,
        8
    );
    assert_eq!(
        ranges.insert.end.character - ranges.insert.start.character,
        2
    );
}

#[test]
fn completion_contextual_properties_do_not_repeat_already_present_members() {
    let list = completion_result("interface Options { required: string; optional?: number; done: boolean } const opt: Options = { done: true, /*cursor*/ };", &CompletionOptions::default());
    assert_eq!(
        list.items
            .iter()
            .flatten()
            .map(|i| i.label.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        ["optional?", "required"].into_iter().collect()
    );
}

#[test]
fn completion_literal_arguments_and_labels_have_native_ordering_groups() {
    let list = completion_result(
        "function choose(value: 1 | 2 | 3): void {} choose(/*cursor*/)",
        &CompletionOptions::default(),
    );
    let values: Vec<_> = list
        .items
        .iter()
        .flatten()
        .filter(|i| i.kind.as_deref() == Some(&lsp::CompletionItemKind::CONSTANT))
        .map(|i| (i.label.as_str(), i.sort_text.as_deref().unwrap().as_str()))
        .collect();
    assert_eq!(values, [("1", "11"), ("2", "11"), ("3", "11")]);
    let labels = completion_result(
        "outer: while(true) { inner: while (true) { break /*cursor*/ } }",
        &CompletionOptions::default(),
    );
    assert_eq!(
        labels
            .items
            .iter()
            .flatten()
            .map(|i| i.label.as_str())
            .collect::<Vec<_>>(),
        ["inner", "outer"]
    );
}
