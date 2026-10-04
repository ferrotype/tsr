use super::*;
use std::sync::Arc;
use tsr_compiler::{FileCache, Program, ProgramOptions};
use tsr_jsstring::JsString;
use tsr_lsproto as lsp;

fn program(text: &[u8], dependency: &[u8], cache: &mut FileCache) -> Program {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/main.ts", text);
    fs.insert_loaded(b"/dep.ts", dependency);
    Program::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                tsr_core::CompilerOptions {
                    no_lib: tsr_core::Tristate::TRUE,
                    ..Default::default()
                },
                vec![
                    JsString::from_bytes(b"/main.ts".as_slice()),
                    JsString::from_bytes(b"/dep.ts".as_slice()),
                ],
            ),
            host: Arc::new(fs.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/".as_slice()),
            skip_module_resolution: false,
            single_threaded: tsr_core::Tristate::TRUE,
        },
        cache,
        &tsr_arena::Counters::new(),
    )
    .unwrap()
}
fn fix(name: &str, allowed: lsp::AddAsTypeOnly) -> lsp::AutoImportFix {
    lsp::AutoImportFix {
        kind: lsp::AutoImportFixKind::ADD_TO_EXISTING,
        import_kind: lsp::ImportKind::NAMED,
        name: name.into(),
        module_specifier: "./dep".into(),
        import_index: 0,
        add_as_type_only: allowed,
        ..Default::default()
    }
}
fn apply(text: &str, edits: Vec<edits::Edit>) -> String {
    let mut result = text.to_owned();
    for pair in edits.windows(2) {
        assert!(pair[0].end <= pair[1].start, "overlapping edits: {edits:?}");
    }
    for edit in edits.into_iter().rev() {
        result.replace_range(edit.start as usize..edit.end as usize, &edit.text);
    }
    result
}
#[test]
fn batched_bindings_share_clause_promotion_and_keep_existing_aliases() {
    // The pin promotes the clause once, preserving the existing names as
    // type-only and sorting the added names against that promoted view.
    let text = "import type { A as Renamed, Z } from './dep';\n";
    let p = program(
        text.as_bytes(),
        b"export interface A {} export interface Z {}",
        &mut FileCache::new(),
    );
    let f = p.source_file(b"/main.ts").unwrap();
    let mut adder = ImportAdder::default();
    adder.add(fix("Value", lsp::AddAsTypeOnly::NOT_ALLOWED), false);
    adder.add(fix("Shape", lsp::AddAsTypeOnly::REQUIRED), false);
    adder.add(fix("Shape", lsp::AddAsTypeOnly::ALLOWED), false);
    let edits = adder
        .edits(
            f.bound().view().ast(),
            f.source(),
            &edits::Options {
                single_quote: true,
                semicolons: true,
                prefer_type_only: false,
                verbatim: false,
                newline: "\n",
                usage: None,
            },
        )
        .unwrap();
    assert_eq!(
        apply(text, edits),
        "import { Value, type A as Renamed, type Shape, type Z } from './dep';\n"
    );
}
#[test]
fn empty_bindings_and_default_imports_are_coalesced_once() {
    for (text, expected) in [
        ("import {} from './dep';", "import { A, B } from './dep';"),
        (
            "import Default from './dep';",
            "import Default, { A, B } from './dep';",
        ),
    ] {
        let p = program(
            text.as_bytes(),
            b"export const A=1; export const B=2;",
            &mut FileCache::new(),
        );
        let f = p.source_file(b"/main.ts").unwrap();
        let mut adder = ImportAdder::default();
        for name in ["B", "A"] {
            adder.add(fix(name, lsp::AddAsTypeOnly::NOT_ALLOWED), false);
        }
        let edits = adder
            .edits(
                f.bound().view().ast(),
                f.source(),
                &edits::Options {
                    single_quote: true,
                    semicolons: true,
                    prefer_type_only: false,
                    verbatim: false,
                    newline: "\n",
                    usage: None,
                },
            )
            .unwrap();
        assert_eq!(apply(text, edits), expected);
    }
}
#[test]
fn export_cache_rejects_foreign_programs_dependency_edits_and_extraction_preferences() {
    let mut files = FileCache::new();
    let first = Arc::new(program(
        b"import { A } from './dep';",
        b"export const A=1;",
        &mut files,
    ));
    let pool = tsr_compiler::CompilerCheckerPool::new(first.clone(), &tsr_arena::Counters::new());
    let source = first.source_file(b"/main.ts").unwrap().source();
    let mut operation = pool.checker_for_file_exclusive(source).unwrap();
    let registry = Registry::build(&first, &mut operation, || false)
        .unwrap()
        .unwrap();
    let cache = Cache::new(&first);
    let index = cache.publish(registry);
    let prefs = Preferences::default();
    assert!(Arc::ptr_eq(
        &index,
        &cache.get(&first, b"/main.ts", &prefs).unwrap().unwrap()
    ));
    let updated = program(
        b"import { A } from './dep'; A;",
        b"export const A=1;",
        &mut files,
    );
    assert!(matches!(
        cache.get(&updated, b"/main.ts", &prefs),
        Err(tsr_checker::Error::Arena(tsr_arena::Error::WrongOwner))
    ));
    let copy = Cache::for_update(
        &updated,
        &cache,
        &JsString::from_bytes(b"/main.ts".as_slice()),
    );
    assert!(Arc::ptr_eq(
        &index,
        &copy.get(&updated, b"/main.ts", &prefs).unwrap().unwrap()
    ));
    let changed = program(
        b"import { A } from './dep'; A;",
        b"export const B=2;",
        &mut files,
    );
    let copy = Cache::for_update(
        &changed,
        &copy,
        &JsString::from_bytes(b"/dep.ts".as_slice()),
    );
    assert!(copy.get(&changed, b"/main.ts", &prefs).unwrap().is_none());
    assert!(cache
        .get(
            &first,
            b"/main.ts",
            &Preferences {
                exclude_files: vec![JsString::from_bytes(b"**/dep.ts".as_slice())],
                ..Default::default()
            }
        )
        .unwrap()
        .is_none());
    assert!(cache
        .get(
            &first,
            b"/main.ts",
            &Preferences {
                directory_search: Some(true),
                ..Default::default()
            }
        )
        .unwrap()
        .is_none());
    // Specifier selection is request-local and must not rebuild the index.
    assert!(cache
        .get(
            &first,
            b"/main.ts",
            &Preferences {
                ending: Some("js".into()),
                ..Default::default()
            }
        )
        .unwrap()
        .is_some());
    assert!(Arc::ptr_eq(
        &index,
        &cache.get(&first, b"/main.ts", &prefs).unwrap().unwrap()
    ));
}
