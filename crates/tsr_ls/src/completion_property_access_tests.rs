use crate::{CompletionOptions, LanguageService};
use std::sync::Arc;
use tsr_core::CancellationToken;
use tsr_jsstring::PositionEncoding;
use tsr_lsproto as lsp;

fn complete(text: &str, quote: crate::QuotePreference) -> lsp::CompletionList {
    let cursor = text.find('|').unwrap();
    let text = text.replacen('|', "", 1);
    let name = b"/namespace.ts";
    let program = Arc::new(crate::tests::program(name, text.as_bytes()));
    let counters = tsr_arena::Counters::new();
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
    let mut checker = pool
        .checker_for_file_exclusive(program.source_file(name).unwrap().source())
        .unwrap();
    let mut service =
        LanguageService::new(&program, PositionEncoding::Utf16, CancellationToken::new());
    *service
        .completion(
            &mut checker,
            &lsp::CompletionParams {
                text_document: lsp::TextDocumentIdentifier {
                    uri: lsp::DocumentUri("file:///namespace.ts".into()),
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
            &CompletionOptions {
                commit_characters: true,
                default_commit_characters: true,
                default_edit_range: true,
                quote,
                ..Default::default()
            },
        )
        .unwrap()
        .list
        .unwrap()
}

#[test]
fn public_hash_properties_convert_dot_access_with_selected_quote() {
    for (quote, expected) in [
        (crate::QuotePreference::Single, "['#']"),
        (crate::QuotePreference::Double, "[\"#\"]"),
        (crate::QuotePreference::Auto, "['#']"),
    ] {
        let text = "import './other'; const object = { '#': 1 }; object.|";
        let list = complete(text, quote);
        let item = list
            .items
            .iter()
            .flatten()
            .find(|item| item.label == "#")
            .unwrap();
        assert_eq!(
            item.insert_text.as_deref().map(String::as_str),
            Some(expected)
        );
        let edit = item
            .text_edit
            .as_deref()
            .unwrap()
            .text_edit
            .as_deref()
            .unwrap();
        assert_eq!(edit.new_text, expected);
        assert_eq!(
            edit.range.start,
            lsp::Position {
                line: 0,
                character: (text.find('|').unwrap() - 1) as u32
            }
        );
        assert_eq!(
            edit.range.end,
            lsp::Position {
                line: 0,
                character: text.find('|').unwrap() as u32
            }
        );
    }
    let list = complete(
        "const object = { '#name': 1 }; object.|",
        crate::QuotePreference::Single,
    );
    let item = list
        .items
        .iter()
        .flatten()
        .find(|item| item.label == "#name")
        .unwrap();
    assert_eq!(
        item.insert_text.as_deref().map(String::as_str),
        Some("['#name']")
    );
}

#[test]
fn actual_private_class_member_remains_dot_access() {
    let list = complete(
        "class C { #field = 1; method() { this.| } }",
        crate::QuotePreference::Single,
    );
    let item = list
        .items
        .iter()
        .flatten()
        .find(|item| item.label == "#field")
        .unwrap();
    assert!(item.insert_text.is_none());
    assert!(item.text_edit.is_none());
}
