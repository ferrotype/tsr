use crate::{CompletionOptions, LanguageService};
use std::sync::Arc;
use tsr_core::CancellationToken;
use tsr_jsstring::PositionEncoding;
use tsr_lsproto as lsp;

fn complete(name: &[u8], text: &str) -> lsp::CompletionItemsOrListOrNull {
    let cursor = text.find('|').unwrap();
    let text = text.replacen('|', "", 1);
    let program = Arc::new(crate::tests::program(name, text.as_bytes()));
    let counters = tsr_arena::Counters::new();
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
    let mut checker = pool
        .checker_for_file_exclusive(program.source_file(name).unwrap().source())
        .unwrap();
    let mut service =
        LanguageService::new(&program, PositionEncoding::Utf16, CancellationToken::new());
    service
        .completion(
            &mut checker,
            &lsp::CompletionParams {
                text_document: lsp::TextDocumentIdentifier {
                    uri: lsp::DocumentUri(format!("file://{}", String::from_utf8_lossy(name))),
                },
                position: lsp::Position {
                    line: text[..cursor].bytes().filter(|b| *b == b'\n').count() as u32,
                    character: text[..cursor]
                        .rsplit('\n')
                        .next()
                        .unwrap()
                        .encode_utf16()
                        .count() as u32,
                },
                ..Default::default()
            },
            &CompletionOptions::default(),
        )
        .unwrap()
}

#[test]
fn local_export_remains_visible_after_reference_at_eof() {
    for text in [
        "export const foo = 1;\nfoo|\n",
        "const __VERSION = \"1.0.0\";\nexport const foo = 1;\nfoo|\n",
    ] {
        let list = complete(b"/app.ts", text).list.unwrap();
        assert!(
            list.items.iter().flatten().any(|item| item.label == "foo"),
            "{text}"
        );
    }
}

#[test]
fn exported_type_and_namespace_meanings_remain_visible() {
    for text in [
        "export interface Foo { value: number }; let value: Fo|",
        "export class Foo {}; let value: Fo|",
        "export namespace Foo { export interface Bar {} }; let value: Fo|",
    ] {
        let list = complete(b"/app.ts", text).list.unwrap();
        assert!(
            list.items.iter().flatten().any(|item| item.label == "Foo"),
            "{text}"
        );
    }
}

#[test]
fn expression_space_does_not_offer_type_only_declarations() {
    let list = complete(
        b"/app.ts",
        "export interface Foo {}; export const foo = 1; fo|",
    )
    .list
    .unwrap();
    assert!(list.items.iter().flatten().any(|item| item.label == "foo"));
    assert!(!list.items.iter().flatten().any(|item| item.label == "Foo"));
}
