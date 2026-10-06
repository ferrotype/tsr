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
