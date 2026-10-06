use super::*;
use std::sync::Arc;
use tsr_core::CancellationToken;

fn completion(source: &str, extra: Option<(&[u8], &[u8])>, snippets: bool) -> lsp::CompletionList {
    let offset = source.find("/*cursor*/").unwrap();
    let text = source.replacen("/*cursor*/", "", 1);
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/index.ts", text.as_bytes());
    if let Some((name, text)) = extra {
        fs.insert_loaded(name, text);
    }
    let program = Arc::new(
        tsr_compiler::Program::load(
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
        .unwrap(),
    );
    let file = program.source_file(b"/index.ts").unwrap().source();
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &tsr_arena::Counters::new());
    let mut checker = pool.checker_for_file_exclusive(file).unwrap();
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
            &CompletionOptions {
                class_member_snippets: true,
                snippets,
                ..Default::default()
            },
        )
        .unwrap()
        .list
        .unwrap()
}
fn item(list: &lsp::CompletionList, name: &str) -> lsp::CompletionItem {
    list.items
        .iter()
        .flatten()
        .find(|item| item.label == name)
        .unwrap_or_else(|| panic!("missing {name}"))
        .as_ref()
        .clone()
}

#[test]
fn parameter_property_keeps_name_fallback_with_and_without_snippets() {
    for snippets in [false, true] {
        let value = item(
            &completion(
                "class B { constructor(public value: string) {} } class C extends B { /*cursor*/ }",
                None,
                snippets,
            ),
            "value",
        );
        assert_eq!(
            value.insert_text.as_deref().map(String::as_str),
            Some("value")
        );
        assert_eq!(
            value.filter_text.as_deref().map(String::as_str),
            Some("value")
        );
        assert_eq!(value.insert_text_format.is_some(), snippets);
        assert!(value.additional_text_edits.is_none());
    }
}

#[test]
fn decorated_modifier_is_retained_without_rejecting_member() {
    let foo = item(&completion("declare function decorator(...args: any[]): any;\nclass Base { protected foo(a: string): string; protected foo(a: number): number; protected foo(a: any): any { return a; } }\nclass Derived extends Base { @decorator protected /*cursor*/ }", None, false), "foo");
    let text = foo.insert_text.as_deref().unwrap();
    assert!(text.contains("@decorator"), "{text}");
    assert!(text.contains("protected foo(a: string): string;"), "{text}");
    assert!(foo
        .additional_text_edits
        .as_ref()
        .is_some_and(|edits| edits.iter().flatten().any(|edit| edit.new_text.is_empty())));
    assert_eq!(foo.data.as_ref().unwrap().source, "ClassMemberSnippet/");
}

#[test]
fn inherited_signature_type_import_marks_class_member_action_source() {
    let method = item(&completion("import { Base } from './base'; class Derived extends Base { /*cursor*/ }", Some((b"/base.ts", b"export interface Result { value: number }; export class Base { method(): Result { throw 0; } }")), false), "method");
    assert!(method
        .additional_text_edits
        .as_ref()
        .is_some_and(|edits| !edits.is_empty()));
    assert_eq!(method.data.as_ref().unwrap().source, "ClassMemberSnippet/");
}

#[test]
fn optional_member_filter_uses_name_without_optional_label_suffix() {
    let list = completion(
        "interface I { a?: number; b?(x: number): void; } class C implements I { /*cursor*/ }",
        None,
        false,
    );
    for name in ["a", "b"] {
        let member = item(&list, &format!("{name}?"));
        assert_eq!(
            member.filter_text.as_deref().map(String::as_str),
            Some(name)
        );
    }
}
