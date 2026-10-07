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
            &CompletionOptions {
                commit_characters: true,
                default_commit_characters: true,
                ..Default::default()
            },
        )
        .unwrap()
}

#[test]
fn stray_dots_and_incomplete_spread_have_no_completion_list() {
    // Native MemberListAfterSingleDot/DoubleDot and GetJavaScriptCompletions22.
    for (name, text) in [
        (b"/a.ts".as_slice(), ".|"),
        (b"/a.ts".as_slice(), "..|"),
        (b"/a.js".as_slice(), "const abc = {}; ({..|});"),
    ] {
        assert!(complete(name, text).list.is_none(), "{text}");
    }
}

#[test]
fn call_recovery_dots_do_not_become_number_member_completions() {
    // Native CompletionsWritingSpreadArgument evolves . -> .. -> ... .
    let prefix = "declare const Math: { min(...values: number[]): number };\n";
    for dots in [".", ".."] {
        let text = format!("{prefix}const [] = [Math.min({dots}|)]");
        assert!(complete(b"/a.ts", &text).list.is_none(), "{dots}");
    }
    let text = format!("{prefix}const [] = [Math.min(...|)]");
    let result = complete(b"/a.ts", &text);
    let list = result.list.unwrap();
    assert!(list.items.iter().flatten().any(|item| item.label == "Math"));
}

#[test]
fn complete_call_and_real_member_access_still_offer_properties() {
    for text in [
        "declare function make(): { value: number }; make().|",
        "const object = { value: 1 }; object.|",
    ] {
        let list = complete(b"/a.ts", text).list.unwrap();
        assert_eq!(
            list.items
                .iter()
                .flatten()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["value"]
        );
    }
}

#[test]
fn literal_text_and_regex_flags_block_completion_but_expression_trivia_does_not() {
    // Native isInStringOrRegularExpressionOrTemplateLiteral includes the end of
    // a regexp because the user may still be entering its flags.
    for text in [
        "const known = 1; const expression = /ab|c/;",
        "const known = 1; const expression = /abc/|;",
        "const known = 1; const expression = /abc/g|;",
        "const known = 1; const template = `a|b`;",
        "const known = 1; const template = `a${known}b|c`;",
        "const known = 1; const template = `unfinished|",
    ] {
        assert!(complete(b"/a.ts", text).list.is_none(), "{text}");
    }
    for text in [
        "const known = 1; const expression = /abc/; |",
        "const known = 1; const template = `a${|}`;",
    ] {
        let list = complete(b"/a.ts", text).list.unwrap();
        assert!(
            list.items
                .iter()
                .flatten()
                .any(|item| item.label == "known"),
            "{text}"
        );
    }
}

#[test]
fn interface_member_slots_keep_member_keywords_and_no_commit_characters() {
    for text in [
        "interface I { /** JSDoc */ |foo(): void; }",
        "interface I { m(): void; fo| }",
        "type T = { fo| };",
        "interface I { f; |",
    ] {
        let list = complete(b"/a.ts", text).list.unwrap();
        assert_eq!(
            list.items
                .iter()
                .flatten()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["readonly"],
            "{text}"
        );
        assert_eq!(
            list.item_defaults.unwrap().commit_characters.as_deref(),
            Some(&Vec::new()),
            "{text}"
        );
    }
}

#[test]
fn declaration_name_slots_block_and_constructor_modifiers_remain_available() {
    for text in [
        "class C<T, |",
        "const C = class D<T, |",
        "var [x, ...z|",
        "const x = 1 as const |",
    ] {
        assert!(complete(b"/a.ts", text).list.is_none(), "{text}");
    }
    for text in [
        "class C { constructor(public |",
        "class C { constructor(public a|",
        "class C { constructor(a|",
    ] {
        let list = complete(b"/a.ts", text).list.unwrap();
        assert!(
            list.items
                .iter()
                .flatten()
                .any(|item| item.label == "readonly"),
            "{text}"
        );
    }
    let list = complete(b"/a.ts", "const x = 1 as const\n|").list.unwrap();
    assert!(list.items.iter().flatten().any(|item| item.label == "x"));
    let list = complete(b"/a.ts", "class C { constructor(private a, |")
        .list
        .unwrap();
    assert_eq!(list.items.len(), 5);
}

#[test]
fn index_signature_type_query_keeps_its_parameter_in_scope() {
    let list = complete(b"/a.ts", "class C { [foo: typeof |\n}")
        .list
        .unwrap();
    assert!(list.items.iter().flatten().any(|item| item.label == "foo"));
    let list = complete(b"/a.ts", "class C { [foo: |\n}").list.unwrap();
    assert!(!list.items.iter().flatten().any(|item| item.label == "foo"));
}

#[test]
fn incomplete_array_binding_rest_names_block_completions() {
    for name in [b"/a.ts".as_slice(), b"/d.ts".as_slice()] {
        for text in ["var [x, ...z|", "var [x, ...z|\n"] {
            assert!(complete(name, text).list.is_none(), "{text}");
        }
    }
}

#[test]
fn jsdoc_type_literal_member_completion_uses_its_container() {
    let text = "class MssqlClient {\n  /**\n   * @returns {Promise<{upStatement|, downStatement}>}\n   */\n  async relationCreate(args) {}\n}\nexport default MssqlClient;";
    let list = complete(b"/index.ts", text).list.unwrap();
    assert_eq!(
        list.items
            .iter()
            .flatten()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        ["readonly"]
    );
}

#[test]
fn object_binding_without_source_type_has_no_completion_list() {
    for text in [
        "var {x|",
        "var {x, y|",
        "function f({a|",
        "function f({a, b|",
    ] {
        assert!(complete(b"/a.ts", text).list.is_none(), "{text}");
    }
    let list = complete(
        b"/a.ts",
        "declare const source: { x: number, y: number }; const {x, |} = source;",
    )
    .list
    .unwrap();
    assert!(list.items.iter().flatten().any(|item| item.label == "y"));
}

#[test]
fn jsdoc_type_literal_declaration_name_has_no_completion_list() {
    assert!(complete(
        b"/index.ts",
        "/**\n * @type { {|ageX: number} }\n */\nvar y;"
    )
    .list
    .is_none());
}
