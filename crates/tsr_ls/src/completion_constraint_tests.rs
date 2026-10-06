use crate::{CompletionOptions, LanguageService};
use std::sync::Arc;
use tsr_core::CancellationToken;
use tsr_jsstring::PositionEncoding;
use tsr_lsproto as lsp;

fn complete(name: &[u8], text: &str) -> lsp::CompletionItemsOrListOrNull {
    let cursor = text.rfind('|').unwrap();
    let mut text = text.to_owned();
    text.remove(cursor);
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
                snippets: true,
                object_method_snippets: true,
                ..Default::default()
            },
        )
        .unwrap()
}

fn labels(text: &str) -> Vec<String> {
    let list = complete(b"/a.ts", text).list.unwrap();
    assert!(!list.is_incomplete);
    assert!(list
        .item_defaults
        .as_ref()
        .unwrap()
        .commit_characters
        .as_ref()
        .unwrap()
        .is_empty());
    list.items
        .iter()
        .flatten()
        .map(|item| item.label.clone())
        .collect()
}

#[test]
fn generic_constraint_members_replace_interface_keywords() {
    let prefix = "interface Foo { one: string; two: number; 333: symbol; '4four': boolean; } interface Bar<T extends Foo> {} ";
    assert_eq!(
        labels(&format!("{prefix}var value: Bar<{{|")),
        ["one", "two", "\"333\"", "\"4four\""]
    );
    assert_eq!(
        labels(&format!("{prefix}var value: Bar<{{ on|")),
        ["one", "two", "\"333\"", "\"4four\""]
    );
}

#[test]
fn existing_members_include_intersection_siblings_but_not_union_siblings() {
    let prefix = "interface Foo { one: string; two: number; } interface Bar<T extends Foo> {} ";
    for suffix in [
        "{ one: string, |",
        "{ one: string } & {|",
        "{ one: string } & { tw|",
    ] {
        assert_eq!(
            labels(&format!("{prefix}var value: Bar<{suffix}")),
            ["two"],
            "{suffix}"
        );
    }
    assert_eq!(
        labels(&format!("{prefix}var value: Bar<{{ one: string }} | {{|")),
        ["one", "two"]
    );
}

#[test]
fn nested_property_and_call_type_argument_use_constraint() {
    let prefix = "interface Foo { one: string; nested: { two: number }; } declare function use<T extends Foo>(): void; ";
    assert_eq!(
        labels(&format!("{prefix}use<{{ nested: {{| }} }}>();")),
        ["two"]
    );
    assert_eq!(
        labels(&format!("{prefix}use<{{| }}>();")),
        ["one", "nested"]
    );
}

#[test]
fn unconstrained_type_literal_keeps_interface_keyword_completion() {
    for text in [
        "type Value = {|",
        "interface Value {|",
        "interface Bar<T> {} var value: Bar<{|",
    ] {
        let list = complete(b"/a.ts", text).list.unwrap();
        assert!(
            list.items
                .iter()
                .flatten()
                .any(|item| item.label == "readonly"),
            "{text}"
        );
        assert!(
            !list.items.iter().flatten().any(|item| item.label == "one"),
            "{text}"
        );
    }
}

#[test]
fn deeply_nested_type_argument_unwinds_contextual_properties() {
    // 96 property/type-literal pairs exercise 192 ancestor transformations.
    let depth = 96;
    let expected = format!(
        "{}{{ leaf: string }}{}",
        "{ next: ".repeat(depth),
        " }".repeat(depth)
    );
    let actual = format!("{}{{|}}{}", "{ next: ".repeat(depth), " }".repeat(depth));
    let text = format!("interface Bar<T extends {expected}> {{}} var value: Bar<{actual}>;");
    assert_eq!(labels(&text), ["leaf"]);
}

#[test]
fn callable_constraint_does_not_generate_object_method_body() {
    let list = complete(b"/a.ts", "interface Bar<T extends { method(): void; property?: string; 'invalid key': number }> {} let value: Bar<{|}>").list.unwrap();
    assert_eq!(
        list.items
            .iter()
            .flatten()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        ["method", "property?", "\"invalid key\""]
    );
    assert!(list
        .item_defaults
        .as_ref()
        .unwrap()
        .commit_characters
        .as_ref()
        .unwrap()
        .is_empty());
    for item in list.items.iter().flatten() {
        assert_eq!(item.sort_text.as_deref().map(String::as_str), Some("11"));
        assert!(item.insert_text_format.is_none(), "{item:?}");
        assert!(item
            .text_edit
            .as_deref()
            .and_then(|edit| edit.text_edit.as_deref())
            .is_none_or(|edit| !edit.new_text.contains('{')));
        assert!(item
            .text_edit
            .as_deref()
            .and_then(|edit| edit.insert_replace_edit.as_deref())
            .is_none_or(|edit| !edit.new_text.contains('{')));
    }
}
