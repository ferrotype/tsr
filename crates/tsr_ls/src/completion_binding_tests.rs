use crate::{CompletionOptions, LanguageService};
use std::sync::Arc;
use tsr_core::CancellationToken;
use tsr_jsstring::PositionEncoding;
use tsr_lsproto as lsp;

fn complete(name: &[u8], text: &str) -> lsp::CompletionItemsOrListOrNull {
    let cursor = text.find("/*cursor*/").unwrap();
    let mut text = text.to_owned();
    text.replace_range(cursor..cursor + "/*cursor*/".len(), "");
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
            &CompletionOptions {
                commit_characters: true,
                default_commit_characters: true,
                ..Default::default()
            },
        )
        .unwrap()
}

fn labels(text: &str) -> Vec<String> {
    let list = complete(b"/a.ts", text).list.unwrap();
    assert!(!list.is_incomplete);
    assert_eq!(
        list.item_defaults
            .as_ref()
            .unwrap()
            .commit_characters
            .as_ref()
            .unwrap()
            .as_slice(),
        [".", ",", ";"]
    );
    list.items
        .iter()
        .flatten()
        .map(|item| item.label.clone())
        .collect()
}

#[test]
fn binding_union_offers_only_shared_properties() {
    assert_eq!(labels("interface I { x: number; y: string; z: boolean } interface J { x: string; y: string } let { /*cursor*/ }: I | J = { x: 10 };"), ["x", "y"]);
}

#[test]
fn binding_private_and_protected_properties_respect_enclosing_class() {
    let prefix =
        "class Foo { private hidden = 1; protected inherited = 2; public value = 3; method() { ";
    assert_eq!(
        labels(&format!("{prefix}const {{ /*cursor*/ }} = this; }} }}")),
        ["hidden", "inherited", "value", "method"]
    );
    assert_eq!(labels("class Foo { private hidden = 1; protected inherited = 2; public value = 3; method() {} } const { /*cursor*/ } = new Foo();"), ["value", "method"]);
    assert!(complete(
        b"/a.ts",
        "const { b/*cursor*/ } = new class { private ab; protected bc; }"
    )
    .list
    .is_none());
}

#[test]
fn constructor_binding_has_only_real_static_members() {
    let prefix = "class Foo { private static hidden = 1; protected static inherited = 2; public static value = 3; method() { ";
    assert_eq!(
        labels(&format!("{prefix}const {{ /*cursor*/ }} = Foo; }} }}")),
        ["hidden", "inherited", "value", "prototype"]
    );
    assert_eq!(labels("class Foo { private static hidden = 1; protected static inherited = 2; public static value = 3; } const { /*cursor*/ } = Foo;"), ["value", "prototype"]);
}
