use crate::{CompletionOptions, LanguageService};
use std::sync::Arc;
use tsr_core::{CancellationToken, CompilerOptions, Tristate};
use tsr_jsstring::{JsString, PositionEncoding};
use tsr_lsproto as lsp;

fn complete(name: &str, input: &str, files: &[(&str, &str)]) -> lsp::CompletionList {
    complete_with_symlinks(name, input, files, &[])
}
fn complete_with_symlinks(
    name: &str,
    input: &str,
    files: &[(&str, &str)],
    symlinks: &[(&str, &str)],
) -> lsp::CompletionList {
    let cursor = input.find('|').unwrap();
    let text = input.replacen('|', "", 1);
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(name.as_bytes(), text.as_bytes());
    for (path, target) in symlinks {
        fs.insert_symlink(path.as_bytes(), target.as_bytes());
    }
    let mut roots = vec![JsString::from_bytes(name.as_bytes())];
    for (path, text) in files {
        fs.insert_loaded(path.as_bytes(), text.as_bytes());
        if !path.as_bytes().ends_with(b".json") {
            roots.push(JsString::from_bytes(path.as_bytes()));
        }
    }
    let program = Arc::new(
        tsr_compiler::Program::load(
            tsr_compiler::ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        no_lib: Tristate::TRUE,
                        allow_js: Tristate::TRUE,
                        ..Default::default()
                    },
                    roots,
                ),
                host: Arc::new(fs.finish()),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(b"/".as_slice()),
                skip_module_resolution: false,
                single_threaded: Tristate::TRUE,
            },
            &mut tsr_compiler::FileCache::new(),
            &tsr_arena::Counters::new(),
        )
        .unwrap(),
    );
    let counters = tsr_arena::Counters::new();
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
    let mut checker = pool
        .checker_for_file_exclusive(program.source_file(name.as_bytes()).unwrap().source())
        .unwrap();
    let mut service =
        LanguageService::new(&program, PositionEncoding::Utf16, CancellationToken::new());
    *service
        .completion(
            &mut checker,
            &lsp::CompletionParams {
                text_document: lsp::TextDocumentIdentifier {
                    uri: lsp::DocumentUri(format!("file://{name}")),
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
        .list
        .unwrap()
}

#[test]
fn global_names_allow_multiple_auto_import_candidates_but_local_names_shadow_them() {
    let files = [
        ("/global.d.ts", "declare var foo: number;"),
        ("/a.ts", "export const foo = 0;"),
        ("/b.ts", "export const foo = 1;"),
    ];
    let list = complete("/c.ts", "fo|", &files);
    let items: Vec<_> = list
        .items
        .iter()
        .flatten()
        .filter(|i| i.label == "foo")
        .collect();
    assert_eq!(items.len(), 3);
    let list = complete("/c.ts", "const foo = 2; fo|", &files);
    assert_eq!(
        list.items
            .iter()
            .flatten()
            .filter(|i| i.label == "foo")
            .count(),
        1
    );
}

#[test]
fn jsx_default_import_candidates_use_component_casing() {
    let list = complete(
        "/index.tsx",
        "export function Index() { return <Component|\n}",
        &[("/component.tsx", "export default function (props: any) {}")],
    );
    assert!(list.items.iter().flatten().any(|i| i.label == "Component"));
    assert!(!list.items.iter().flatten().any(|i| i.label == "component"));
}

#[test]
fn class_namespace_merge_keeps_function_members_in_expression_locations() {
    let list = complete(
        "/index.ts",
        "class Foo { static method1() {} }\nnamespace Foo.Namespace { export interface SomeType {} }\nFoo.|;",
        &[(
            "/lib.d.ts",
            "interface Function { apply(): void; call(): void; bind(): void; prototype: any; length: number; arguments: any; caller: any; } interface NewableFunction extends Function {} interface CallableFunction extends Function {}",
        )],
    );
    let labels: Vec<_> = list
        .items
        .iter()
        .flatten()
        .map(|i| i.label.as_str())
        .collect();
    for name in [
        "method1",
        "prototype",
        "apply",
        "call",
        "bind",
        "length",
        "arguments",
        "caller",
    ] {
        assert!(labels.contains(&name), "{name}: {labels:?}");
    }
}

#[test]
fn js_name_table_does_not_duplicate_auto_import_candidates() {
    let list = complete(
        "/file2.js",
        "import * as foo from './file1';\n|\nexport default foo.b;",
        &[(
            "/file1.js",
            "const a = 1; export { a as b }; export default a;",
        )],
    );
    let labels: Vec<_> = list
        .items
        .iter()
        .flatten()
        .map(|i| i.label.as_str())
        .collect();
    assert!(labels.contains(&"foo"));
    assert_eq!(
        list.items
            .iter()
            .flatten()
            .filter(|i| i.label == "a")
            .count(),
        1
    );
    assert_eq!(
        list.items
            .iter()
            .flatten()
            .filter(|i| i.label == "b")
            .count(),
        1
    );
}

#[test]
fn local_package_symlink_keeps_its_source_module_specifier_before_import() {
    let list = complete_with_symlinks(
        "/project/src/index.ts",
        "const a = new MyClass|();",
        &[
            (
                "/project/packages/mylib/package.json",
                r#"{ "name": "mylib", "version": "1.0.0", "main": "index.js" }"#,
            ),
            (
                "/project/packages/mylib/index.ts",
                "export * from './mySubDir';",
            ),
            (
                "/project/packages/mylib/mySubDir/index.ts",
                "export * from './myClass';",
            ),
            (
                "/project/packages/mylib/mySubDir/myClass.ts",
                "export class MyClass {}",
            ),
        ],
        &[("/project/node_modules/mylib", "/project/packages/mylib")],
    );
    let items: Vec<_> = list
        .items
        .iter()
        .flatten()
        .filter(|i| i.label == "MyClass")
        .collect();
    let modules: Vec<_> = items
        .iter()
        .map(|i| {
            i.data
                .as_ref()
                .unwrap()
                .auto_import
                .as_ref()
                .unwrap()
                .module_specifier
                .as_str()
        })
        .collect();
    assert!(modules.contains(&"../packages/mylib"), "{modules:?}");
}
