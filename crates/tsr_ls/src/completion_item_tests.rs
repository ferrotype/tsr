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

fn item<'a>(list: &'a lsp::CompletionList, label: &str) -> &'a lsp::CompletionItem {
    list.items
        .iter()
        .flatten()
        .find(|item| item.label == label)
        .unwrap()
}

// source: tsc/internal/fourslash/tests/completionListStaticMembers_test.go:TestCompletionListStaticMembers
#[test]
fn static_property_sort_matches_pin() {
    let list = complete(
        b"/a.ts",
        "class Foo { static a() {} static b = 0; } Foo.|",
        &CompletionOptions::default(),
    );
    for label in ["a", "b"] {
        assert_eq!(
            item(&list, label).sort_text.as_deref().map(String::as_str),
            Some("10")
        );
    }
    assert_eq!(
        item(&list, "prototype")
            .sort_text
            .as_deref()
            .map(String::as_str),
        Some("11")
    );
    let list = complete(
        b"/a.ts",
        "class Foo {\n/** @deprecated */\nstatic m() {}\n}\nFoo.|",
        &CompletionOptions::default(),
    );
    assert_eq!(
        item(&list, "m").sort_text.as_deref().map(String::as_str),
        Some("z10")
    );
}

// source: tsc/internal/fourslash/tests/jsxAttributeCompletionStyleAuto_test.go:TestJsxAttributeCompletionStyleAuto
#[test]
fn optional_jsx_snippet_filter_matches_pin() {
    for style in ["auto", "braces"] {
        let list = complete(
            b"/a.tsx",
            "declare namespace JSX { interface Element {} interface IntrinsicElements { foo: { optional?: string; required: string; } } } <foo | />",
            &CompletionOptions {
                snippets: true,
                jsx_attribute_style: Some(style.into()),
                ..Default::default()
            },
        );
        let optional = item(&list, "optional?");
        assert_eq!(
            optional.filter_text.as_deref().map(String::as_str),
            Some("optional")
        );
        assert_eq!(
            optional.insert_text_format.as_deref(),
            Some(&lsp::InsertTextFormat::SNIPPET)
        );
        let required = item(&list, "required");
        assert!(required.filter_text.as_deref().unwrap().contains("$1"));
    }
}
