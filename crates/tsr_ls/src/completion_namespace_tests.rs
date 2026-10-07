use crate::{CompletionOptions, LanguageService};
use std::sync::Arc;
use tsr_core::CancellationToken;
use tsr_jsstring::PositionEncoding;
use tsr_lsproto as lsp;

fn complete(text: &str) -> lsp::CompletionList {
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
                ..Default::default()
            },
        )
        .unwrap()
        .list
        .unwrap()
}

fn labels(list: &lsp::CompletionList) -> Vec<&str> {
    list.items
        .iter()
        .flatten()
        .map(|item| item.label.as_str())
        .collect()
}

#[test]
fn incomplete_namespace_declaration_has_no_self_completion() {
    // TestCompletionAtDottedNamespace and CompletionListBuilderLocations_Modules.
    for text in ["namespace wwer.|w", "module A\nmodule A.|"] {
        let list = complete(text);
        assert!(labels(&list).is_empty());
        assert!(!list.is_incomplete);
        assert_eq!(
            list.item_defaults
                .as_ref()
                .unwrap()
                .commit_characters
                .as_deref(),
            Some(&Vec::new())
        );
    }
}

#[test]
fn dotted_namespace_name_keeps_only_members_declared_elsewhere() {
    // TestCompletionsNamespaceName: M from an earlier declaration is offered;
    // the current incomplete namespace member is not offered on its own.
    let list = complete("namespace N {}\nnamespace N.M {}\nnamespace N.|");
    assert_eq!(labels(&list), ["M"]);
    let list = complete("namespace N1.M {}\nnamespace N2.M {}\nnamespace N2.M|");
    assert_eq!(labels(&list), ["M"]);
    let list = complete(
        "namespace N { export const value = 1; export interface Shape {} export namespace M {} }\nnamespace N.|",
    );
    assert_eq!(labels(&list), ["M"]);
}

#[test]
fn nested_namespace_comments_do_not_take_property_access_path() {
    // TestDocCommentTemplateNamespacesAndModules02's nested declaration names.
    for text in [
        "namespace n1.\n    |n2.\n    n3 {}",
        "namespace n1.\n    n2.\n    |n3 {}",
    ] {
        let list = complete(text);
        assert!(labels(&list).is_empty());
    }
}

#[test]
fn namespace_merged_with_object_preserves_type_and_value_members() {
    let prefix = "namespace N { export type T = number; } const N = { m() {} }; ";
    assert_eq!(
        labels(&complete(&format!("{prefix}let value: N.|;"))),
        ["T"]
    );
    assert_eq!(labels(&complete(&format!("{prefix}N.|;"))), ["m"]);
}

#[test]
fn namespace_merged_with_class_offers_inherited_static_members() {
    let prefix =
        "class C { static m() {} } class D extends C {} namespace D { export type T = number; } ";
    assert_eq!(
        labels(&complete(&format!("{prefix}let value: D.|;"))),
        ["T"]
    );
    let list = complete(&format!("{prefix}D.|;"));
    assert_eq!(labels(&list), ["prototype", "m"]);
    let method = list
        .items
        .iter()
        .flatten()
        .find(|item| item.label == "m")
        .unwrap();
    assert_eq!(method.sort_text.as_deref().map(String::as_str), Some("10"));
}

#[test]
fn ordinary_namespace_exports_remain_available_without_duplicates() {
    let list = complete("class N { static m() {} } namespace N { export const own = 1; } N.|;");
    assert_eq!(labels(&list), ["prototype", "own", "m"]);
}
