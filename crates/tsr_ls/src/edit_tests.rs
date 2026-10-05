//! Edit regressions with expected results checked against the pinned server.
use super::*;
use std::sync::Arc;
use tsr_core::{CompilerOptions, Tristate};
use tsr_jsstring::{JsString, PositionEncoding};

fn program(files: &[(&str, &str)], mut options: CompilerOptions) -> Arc<Program> {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, text) in files {
        fs.insert_loaded(name.as_bytes(), text.as_bytes());
    }
    options.no_lib = Tristate::TRUE;
    Arc::new(
        Program::load(
            tsr_compiler::ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    options,
                    files
                        .iter()
                        .filter(|(name, _)| {
                            std::path::Path::new(name)
                                .extension()
                                .is_some_and(|extension| extension.eq_ignore_ascii_case("ts"))
                        })
                        .map(|(name, _)| JsString::from_bytes(name.as_bytes()))
                        .collect(),
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
    )
}

fn apply(text: &str, edits: impl IntoIterator<Item = lsp::TextEdit>) -> String {
    let mut converters = Converters::new(PositionEncoding::Utf16);
    let script = Script::plain(b"/main.ts", text.as_bytes());
    let changes: Vec<_> = edits
        .into_iter()
        .map(|edit| tsr_core::TextChange {
            range: converters.from_lsp_range_to_original(&script, &edit.range),
            new_text: edit.new_text.into_bytes(),
        })
        .collect();
    String::from_utf8(tsr_core::apply_bulk_edits(text.as_bytes(), &changes).unwrap()).unwrap()
}

#[test]
fn quoted_property_rename_keeps_the_checker_usable() {
    let text = "const obj = { \"foo-bar\": 1 }; obj[\"foo-bar\"];";
    let p = program(&[("/main.ts", text)], CompilerOptions::default());
    let source = p.source_file(b"/main.ts").unwrap().source();
    let pool = tsr_compiler::CompilerCheckerPool::new(p.clone(), &tsr_arena::Counters::new());
    let mut checker = pool.checker_for_file_exclusive(source).unwrap();
    let mut service = LanguageService::new(&p, PositionEncoding::Utf16, CancellationToken::new());
    let uri = lsp::DocumentUri("file:///main.ts".into());
    let position = lsp::Position {
        line: 0,
        character: text.find("foo-bar").unwrap() as u32 + 1,
    };
    for import_paths in [false, true] {
        let options = RenameOptions {
            import_paths,
            ..Default::default()
        };
        let info = service
            .rename_info(
                &mut checker,
                &uri,
                &position,
                "changed",
                options,
                &tsr_locale::DEFAULT,
            )
            .unwrap();
        assert!(info.can_rename);
        let result = service
            .rename(
                &mut checker,
                &lsp::RenameParams {
                    text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
                    position: position.clone(),
                    new_name: "changed".into(),
                    ..Default::default()
                },
                options,
                &tsr_locale::DEFAULT,
            )
            .unwrap();
        let mut changes = result.workspace_edit.unwrap().changes.unwrap();
        let edits = changes
            .remove(&uri)
            .unwrap()
            .into_iter()
            .flatten()
            .map(|edit| *edit);
        assert_eq!(
            apply(text, edits),
            "const obj = { \"changed\": 1 }; obj[\"changed\"];"
        );
    }
}

#[test]
fn export_equals_rename_leaves_differently_named_imports_and_reexports_alone() {
    let files = [
        ("/dep.ts", "class Foo {} export = Foo;"),
        (
            "/barrel.ts",
            "import Alias = require(\"./dep\"); export {Alias as Public};",
        ),
        (
            "/main.ts",
            "import {Public} from \"./barrel\"; new Public();",
        ),
        ("/namespace.ts", "import * as Alias from \"./dep\"; Alias;"),
        ("/same.ts", "import Foo = require(\"./dep\"); new Foo();"),
    ];
    let p = program(&files, CompilerOptions::default());
    let source = p.source_file(b"/dep.ts").unwrap().source();
    let pool = tsr_compiler::CompilerCheckerPool::new(p.clone(), &tsr_arena::Counters::new());
    let mut checker = pool.checker_for_file_exclusive(source).unwrap();
    let mut service = LanguageService::new(&p, PositionEncoding::Utf16, CancellationToken::new());
    let result = service
        .rename(
            &mut checker,
            &lsp::RenameParams {
                text_document: lsp::TextDocumentIdentifier {
                    uri: lsp::DocumentUri("file:///dep.ts".into()),
                },
                position: lsp::Position {
                    line: 0,
                    character: 7,
                },
                new_name: "Changed".into(),
                ..Default::default()
            },
            RenameOptions {
                aliases: false,
                ..Default::default()
            },
            &tsr_locale::DEFAULT,
        )
        .unwrap();
    let changes = result.workspace_edit.unwrap().changes.unwrap();
    assert_eq!(changes.len(), 2, "{changes:?}");
    for (file, original, expected) in [
        ("dep.ts", files[0].1, "class Changed {} export = Changed;"),
        (
            "same.ts",
            files[4].1,
            "import Changed = require(\"./dep\"); new Changed();",
        ),
    ] {
        let edits = changes[&lsp::DocumentUri(format!("file:///{file}"))]
            .iter()
            .flatten()
            .map(|edit| edit.as_ref().clone());
        assert_eq!(apply(original, edits), expected);
    }
}

#[test]
fn export_name_sorting_without_named_imports_uses_ordinal_fallback() {
    let text = "export { b, B, a } from \"./dep\";\n";
    let p = program(
        &[("/main.ts", text), ("/dep.ts", "export const a=1,b=2,B=3;")],
        CompilerOptions::default(),
    );
    let source = p.source_file(b"/main.ts").unwrap().source();
    let pool = tsr_compiler::CompilerCheckerPool::new(p.clone(), &tsr_arena::Counters::new());
    let mut checker = pool.checker_for_file_exclusive(source).unwrap();
    let mut service = LanguageService::new(&p, PositionEncoding::Utf16, CancellationToken::new());
    let mut edits = service
        .organize_imports(
            &mut checker,
            source,
            OrganizeMode::Sort,
            &tsr_format::FormatCodeSettings::default(),
            &OrganizeOptions {
                unicode: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        apply(
            text,
            edits
                .remove(&lsp::DocumentUri("file:///main.ts".into()))
                .unwrap()
        ),
        "export { B, a, b } from \"./dep\";\n"
    );
}

fn moved_specifier(
    p: &Arc<Program>,
    source_name: &[u8],
    new_name: &[u8],
    target: &[u8],
    preferences: &tsr_autoimport::Preferences,
) -> JsString {
    let file = p.source_file(source_name).unwrap();
    let source = file.source();
    let view = file.bound().view().ast();
    let imports = view.source_file(source).unwrap().imports().unwrap();
    let old = imports.iter().flatten().next().copied().unwrap_or_else(|| {
        // The loader deliberately omits empty specifiers from its import list.
        // The public generator still accepts that literal and uses user preferences.
        view.node_slice(view.node(source).unwrap().statements(view).unwrap())
            .unwrap()
            .iter()
            .flatten()
            .find_map(|statement| view.node(statement).unwrap().module_specifier())
            .unwrap()
    });
    let pool = tsr_compiler::CompilerCheckerPool::new(p.clone(), &tsr_arena::Counters::new());
    let checker = pool.checker_for_file_exclusive(source).unwrap();
    checker
        .update_module_specifier(
            source,
            new_name,
            old,
            target,
            tsr_checker::ModuleSpecifierPreferences {
                relative: preferences.module_specifier.as_deref(),
                ending: preferences.ending.as_deref(),
                excluded: &|text| preferences.excludes(text),
            },
        )
        .unwrap()
}

#[test]
fn file_moves_preserve_exclusions_rooted_paths_and_empty_specifier_preferences() {
    for (old, relative, excludes, expected) in [
        ("@app/dep", None, vec!["^@app/".into()], "./src/renamed"),
        ("/src/dep", None, vec![], "./src/renamed"),
        ("", Some("relative".into()), vec![], "./src/renamed"),
        ("", Some("non-relative".into()), vec![], "@app/renamed"),
    ] {
        let text = format!("import {{a}} from \"{old}\"; a;");
        let mut paths = tsr_core::PathMappings::default();
        paths.insert(
            JsString::from_bytes(b"@app/*".as_slice()),
            Some(vec![JsString::from_bytes(b"src/*".as_slice())]),
        );
        let p = program(
            &[("/main.ts", &text), ("/src/dep.ts", "export const a=1;")],
            CompilerOptions {
                paths: Some(paths),
                base_url: JsString::from_bytes(b"/".as_slice()),
                ..Default::default()
            },
        );
        let result = moved_specifier(
            &p,
            b"/main.ts",
            b"/main.ts",
            b"/src/renamed.ts",
            &tsr_autoimport::Preferences {
                module_specifier: relative,
                exclude_specifiers: excludes,
                ..Default::default()
            },
        );
        assert_eq!(result.as_bytes(), expected.as_bytes(), "{old}");
    }
}

#[test]
fn file_move_extension_rules_use_the_original_source_name() {
    for (old, new, expected) in [
        ("/entry.d.ts", "/entry.ts", "./dep.ts"),
        ("/entry.ts", "/entry.d.ts", "./dep.js"),
    ] {
        let p = program(
            &[
                (old, "export type T = import(\"./dep.ts\").T;"),
                ("/dep.ts", "export interface T {x:number}"),
                ("/package.json", "{\"type\":\"module\"}"),
            ],
            CompilerOptions {
                module: tsr_core::ModuleKind::NODE_NEXT,
                module_resolution: tsr_core::ModuleResolutionKind::NODE_NEXT,
                ..Default::default()
            },
        );
        assert_eq!(
            moved_specifier(
                &p,
                old.as_bytes(),
                new.as_bytes(),
                b"/dep.ts",
                &tsr_autoimport::Preferences::default()
            )
            .as_bytes(),
            expected.as_bytes()
        );
    }
}

#[test]
fn package_specifier_endings_do_not_inherit_an_old_local_path() {
    for (old, target, expected) in [
        (
            "./node_modules/b/src/lib/index",
            "/node_modules/b/src/other/index.ts",
            "b/src/other",
        ),
        (
            "./node_modules/b/src/y.js",
            "/node_modules/b/src/z.ts",
            "b/src/z",
        ),
    ] {
        let text = format!("import {{a}} from \"{old}\"; a;");
        let p = program(
            &[
                ("/main.ts", &text),
                (target, "export const a=1;"),
                ("/node_modules/b/package.json", "{\"name\":\"b\"}"),
            ],
            CompilerOptions::default(),
        );
        assert_eq!(
            moved_specifier(
                &p,
                b"/main.ts",
                b"/main.ts",
                target.as_bytes(),
                &tsr_autoimport::Preferences {
                    ending: Some("minimal".into()),
                    ..Default::default()
                }
            )
            .as_bytes(),
            expected.as_bytes()
        );
    }
}

#[test]
fn type_only_promotion_handles_zero_tab_size_and_unicode_indentation() {
    for (indent, tab_size, expected_indent) in
        [("\t", 0, ""), (" \t", 4, "      "), ("\u{2003}", 4, " ")]
    {
        let text = format!("import {{\n    type D,\n{indent}a\n}} from \"./dep\";\nnew D(); a;\n");
        let p = program(
            &[
                ("/main.ts", &text),
                ("/dep.ts", "export const a=1; export class D {}"),
            ],
            CompilerOptions::default(),
        );
        let source = p.source_file(b"/main.ts").unwrap().source();
        let pool = tsr_compiler::CompilerCheckerPool::new(p.clone(), &tsr_arena::Counters::new());
        let mut checker = pool.checker_for_file_exclusive(source).unwrap();
        let mut service =
            LanguageService::new(&p, PositionEncoding::Utf16, CancellationToken::new());
        let uri = lsp::DocumentUri("file:///main.ts".into());
        let mut options = CompletionOptions::default();
        options.format.editor.tab_size = tab_size;
        let result = service
            .code_actions(
                &mut checker,
                &lsp::CodeActionParams {
                    text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
                    context: Some(Box::new(lsp::CodeActionContext {
                        diagnostics: vec![Some(Box::new(lsp::Diagnostic {
                            range: lsp::Range {
                                start: lsp::Position {
                                    line: 4,
                                    character: 4,
                                },
                                end: lsp::Position {
                                    line: 4,
                                    character: 5,
                                },
                            },
                            code: Some(Box::new(lsp::IntegerOrString {
                                integer: Some(Box::new(1361)),
                                ..Default::default()
                            })),
                            source: Some(Box::new("ts".into())),
                            ..Default::default()
                        }))],
                        only: Some(Box::new(vec![lsp::CodeActionKind("quickfix".into())])),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                &OrganizeOptions {
                    type_order: "first".into(),
                    ..Default::default()
                },
                &options,
                &tsr_locale::DEFAULT,
            )
            .unwrap();
        let actions = result.command_or_code_action_array.unwrap();
        assert_eq!(actions.len(), 1);
        let edits = &actions[0]
            .code_action
            .as_ref()
            .unwrap()
            .edit
            .as_ref()
            .unwrap()
            .changes
            .as_ref()
            .unwrap()[&uri];
        assert_eq!(
            apply(
                &text,
                edits.iter().flatten().map(|edit| edit.as_ref().clone())
            ),
            format!("import {{\n    a,\n{expected_indent}D\n}} from \"./dep\";\nnew D(); a;\n")
        );
    }
}
