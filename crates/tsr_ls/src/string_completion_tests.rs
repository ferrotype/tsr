use crate::{CompletionOptions, LanguageService};
use std::sync::Arc;
use tsr_core::CancellationToken;
use tsr_jsstring::PositionEncoding;
use tsr_lsproto as lsp;

fn complete(name: &[u8], text: &str, options: &CompletionOptions) -> lsp::CompletionList {
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
    *service
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
            options,
        )
        .unwrap()
        .list
        .unwrap()
}

fn check(text: &str, expected: &[&str]) {
    let list = complete(
        b"/a.ts",
        text,
        &CompletionOptions {
            commit_characters: true,
            default_commit_characters: true,
            ..Default::default()
        },
    );
    assert!(!list.items.is_empty());
    let actual = list.item_defaults.unwrap().commit_characters.unwrap();
    assert_eq!(
        *actual,
        expected.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()
    );
}

// source: tsc/internal/fourslash/tests/completionForStringLiteral2_test.go:TestCompletionForStringLiteral2
#[test]
fn string_property_index_signature_controls_commit_defaults() {
    check(
        "declare const p: { [s: string]: any, a: number }; p[\"|\"];",
        &[],
    );
    check(
        "declare const p: { [s: number]: any, a: number }; p[\"|\"];",
        &[],
    );
    let empty = complete(
        b"/a.ts",
        "declare const p: { [s: string]: any }; p[\"|\"];",
        &CompletionOptions {
            commit_characters: true,
            default_commit_characters: true,
            ..Default::default()
        },
    );
    assert!(empty.items.is_empty());
    assert!(empty
        .item_defaults
        .unwrap()
        .commit_characters
        .unwrap()
        .is_empty());
    check(
        "declare const p: { a: number }; p[\"|\"];",
        &[".", ",", ";"],
    );
    check(
        "declare const p: { [s: string]: any, a: number }; \"|\" in p;",
        &[".", ",", ";"],
    );
    check(
        "declare let p: { [s: string]: any, a: number }; p = { \"|\": 1 };",
        &[],
    );
}

// source: tsc/internal/fourslash/tests/completionForStringLiteralFromSignature_test.go:TestCompletionForStringLiteralFromSignature
#[test]
fn broad_signature_string_controls_commit_defaults() {
    check(
        "declare function f(a: \"x\"): void; declare function f(a: string): void; f(\"|\");",
        &[],
    );
    check(
        "declare function f(a: \"x\"): void; f(\"|\");",
        &[".", ",", ";"],
    );
    check("let x: \"a\" | \"b\"; x = \"|\";", &[".", ",", ";"]);
}
