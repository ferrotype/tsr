use crate::{CompletionOptions, LanguageService};
use std::sync::Arc;
use tsr_core::CancellationToken;
use tsr_jsstring::PositionEncoding;
use tsr_lsproto as lsp;

fn complete(text: &str) -> lsp::CompletionList {
    let cursor = text.find('|').unwrap();
    let text = text.replacen('|', "", 1);
    let name = b"/namespace.ts".as_slice();
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(name, text.as_bytes());
    fs.insert_loaded(
        b"/top.ts",
        b"export interface Bat {} export const a: number;".as_slice(),
    );
    fs.insert_loaded(b"/ns.ts", b"export namespace Foo { export namespace Bar { export class Baz {} export interface Bat {} export const a: number; const b: string; } }".as_slice());
    fs.insert_loaded(
        b"/equals.ts",
        b"class Foo { public static bar: string; private static baz: number; } export = Foo;"
            .as_slice(),
    );
    let program = Arc::new(
        tsr_compiler::Program::load(
            tsr_compiler::ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    tsr_core::CompilerOptions {
                        no_lib: tsr_core::Tristate::TRUE,
                        ..Default::default()
                    },
                    vec![tsr_jsstring::JsString::from_bytes(name)],
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

fn labels(text: &str) -> Vec<String> {
    complete(text)
        .items
        .iter()
        .flatten()
        .map(|item| item.label.clone())
        .collect()
}

#[test]
fn literal_import_member_meaning_distinguishes_typeof() {
    assert_eq!(labels("type T = typeof import(\"./top\").|"), ["a"]);
    assert_eq!(labels("type T = import(\"./top\").|"), ["Bat"]);
}

#[test]
fn qualified_import_members_preserve_runtime_and_type_meanings() {
    for prefix in ["typeof ", ""] {
        assert_eq!(
            labels(&format!("type T = {prefix}import(\"./ns\").|")),
            ["Foo"]
        );
        assert_eq!(
            labels(&format!("type T = {prefix}import(\"./ns\").Foo.|")),
            ["Bar"]
        );
    }
    assert_eq!(
        labels("type T = typeof import(\"./ns\").Foo.Bar.|"),
        ["Baz", "a"]
    );
    assert_eq!(
        labels("type T = import(\"./ns\").Foo.Bar.|"),
        ["Baz", "Bat"]
    );
}

#[test]
fn typeof_export_equals_offers_only_accessible_class_statics() {
    assert_eq!(
        labels("type T = typeof import(\"./equals\").|"),
        ["bar", "prototype"]
    );
}
