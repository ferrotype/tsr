use super::*;
use crate::{fix, Registry};
use std::sync::Arc;
use tsr_core::{CompilerOptions, ModuleKind};
use tsr_jsstring::JsString;

fn barrel_program() -> Arc<Program> {
    barrel_program_with(CompilerOptions::default(), "export {}; A")
}
fn barrel_program_with(options: CompilerOptions, main: &str) -> Arc<Program> {
    let files = [
        ("/foo/a.ts", "export const A = 0;"),
        ("/foo/b.ts", "export {}; A"),
        ("/foo/index.ts", "export * from './a'; export * from './b';"),
        ("/index.ts", "export * from './foo'; export * from './src';"),
        ("/src/a.ts", main),
        ("/src/index.ts", "export * from './a';"),
    ];
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, text) in files {
        fs.insert_loaded(name.as_bytes(), text.as_bytes());
    }
    Arc::new(
        Program::load(
            tsr_compiler::ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        no_lib: Tristate::TRUE,
                        module: ModuleKind::COMMON_JS,
                        ..options
                    },
                    files
                        .iter()
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

#[test]
fn barrel_order_matches_native_sibling_parent_and_ending_variants() {
    let program = barrel_program();
    let counters = tsr_arena::Counters::new();
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
    for (file, ending, expected) in [
        (b"/foo/b.ts".as_slice(), None, vec!["./a", ".", ".."]),
        (
            b"/src/a.ts".as_slice(),
            None,
            vec!["../foo", "../foo/a", ".."],
        ),
        (
            b"/foo/b.ts".as_slice(),
            Some("index"),
            vec!["./a", "./index", "../index"],
        ),
        (
            b"/src/a.ts".as_slice(),
            Some("index"),
            vec!["../foo/a", "../foo/index", "../index"],
        ),
        (
            b"/src/a.ts".as_slice(),
            Some("js"),
            vec!["../foo/a.js", "../foo/index.js", "../index.js"],
        ),
    ] {
        let source = program.source_file(file).unwrap().source();
        let mut checker = pool.checker_for_file_exclusive(source).unwrap();
        let registry = Registry::build(&program, &mut checker, || false)
            .unwrap()
            .unwrap();
        let preferences = Preferences {
            ending: ending.map(str::to_owned),
            ..Default::default()
        };
        let mut fixes = Vec::new();
        for export in registry.index.find(b"A", true) {
            fixes.extend(
                fix::fixes_with_info(
                    &program,
                    &mut checker,
                    source,
                    export,
                    fix::Usage::default(),
                    &preferences,
                )
                .unwrap(),
            );
        }
        assert!(fixes
            .iter()
            .all(|fix| fix.module_specifier_kind == Kind::Relative));
        assert!(fixes
            .iter()
            .any(|fix| fix.is_re_export && fix.module_file_name.as_bytes() == b"/index.ts"));
        let ranking = Ranking::new(&program, source, &preferences).unwrap();
        fixes.sort_by(|a, b| ranking.compare(a, b));
        assert_eq!(
            fixes
                .iter()
                .map(|fix| fix.module_specifier.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{file:?}, {ending:?}"
        );
    }
}

fn candidate(specifier: &str, kind: Kind, filename: &str, reexport: bool) -> Fix {
    Fix {
        protocol: tsr_lsproto::AutoImportFix {
            kind: tsr_lsproto::AutoImportFixKind::ADD_NEW,
            module_specifier: specifier.into(),
            ..Default::default()
        },
        module_specifier_kind: kind,
        module_file_name: JsString::from_bytes(filename.as_bytes()),
        is_re_export: reexport,
    }
}

#[test]
fn provenance_controls_relative_and_cycle_ranking_not_specifier_spelling() {
    let ranking = Ranking {
        importing_file: b"/foo/main.ts",
        prefer_non_relative: true,
        uri_style: Tristate::UNKNOWN,
    };
    let mapped = candidate("mapped/deep/path", Kind::Paths, "/foo/index.ts", true);
    let relative = candidate(".", Kind::Relative, "/foo/direct.ts", false);
    assert_eq!(ranking.rank(&mapped, &relative), Ordering::Less);
    let ranking = Ranking {
        prefer_non_relative: false,
        ..ranking
    };
    let barrel = candidate(".", Kind::Relative, "/foo/index.ts", true);
    let direct = candidate("./deep/direct", Kind::Relative, "/foo/direct.ts", false);
    assert_eq!(ranking.rank(&direct, &barrel), Ordering::Less);
    let sibling_barrel = candidate(".", Kind::Relative, "/foobar/index.ts", true);
    assert_eq!(ranking.rank(&sibling_barrel, &direct), Ordering::Less);
    let ordinary_alias = candidate(".", Kind::Relative, "/foo/index.ts", false);
    assert_eq!(ranking.rank(&ordinary_alias, &direct), Ordering::Less);
    let paths = candidate(".", Kind::Paths, "/foo/index.ts", true);
    assert_eq!(ranking.rank(&paths, &direct), Ordering::Less);
}

#[test]
fn ambient_node_style_and_stable_sort_ties_match_pin() {
    let plain = candidate("fs", Kind::Ambient, "", false);
    let uri = candidate("node:fs", Kind::Ambient, "", false);
    for (uri_style, expected) in [
        (Tristate::TRUE, Ordering::Less),
        (Tristate::FALSE, Ordering::Greater),
        (Tristate::UNKNOWN, Ordering::Equal),
    ] {
        let ranking = Ranking {
            importing_file: b"/main.ts",
            prefer_non_relative: false,
            uri_style,
        };
        assert_eq!(ranking.rank(&uri, &plain), expected);
    }
    let ranking = Ranking {
        importing_file: b"/main.ts",
        prefer_non_relative: false,
        uri_style: Tristate::UNKNOWN,
    };
    let a = candidate("./foo", Kind::Relative, "/foo.ts", false);
    let b = candidate("../foo", Kind::Relative, "/foo.ts", false);
    assert_eq!(ranking.rank(&a, &b), Ordering::Equal);
    assert_eq!(ranking.compare(&a, &b), Ordering::Less);
}

#[test]
fn generated_specifier_provenance_retains_paths_and_existing_import_kind() {
    let mut paths = tsr_core::PathMappings::default();
    paths.insert(
        JsString::from_bytes(b"alias".as_slice()),
        Some(vec![JsString::from_bytes(b"foo/a".as_slice())]),
    );
    let options = CompilerOptions {
        base_url: JsString::from_bytes(b"/".as_slice()),
        paths: Some(paths),
        ..Default::default()
    };
    let program = barrel_program_with(options, "export {}; A");
    let source = program.source_file(b"/src/a.ts").unwrap().source();
    let counters = tsr_arena::Counters::new();
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
    let checker = pool.checker_for_file_exclusive(source).unwrap();
    let result = checker
        .module_specifier_for_auto_import(source, b"/foo/a.ts", Some("non-relative"), None, &|_| {
            false
        })
        .unwrap()
        .unwrap();
    assert_eq!(result.specifier.as_bytes(), b"alias");
    assert_eq!(result.kind, Kind::Paths);
    drop(checker);
    let program = barrel_program_with(
        CompilerOptions::default(),
        "import { A } from '../foo/a'; A",
    );
    let source = program.source_file(b"/src/a.ts").unwrap().source();
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
    let checker = pool.checker_for_file_exclusive(source).unwrap();
    let result = checker
        .module_specifier_for_auto_import(source, b"/foo/a.ts", None, None, &|_| false)
        .unwrap()
        .unwrap();
    assert_eq!(result.specifier.as_bytes(), b"../foo/a");
    assert_eq!(result.kind, Kind::None);
}

#[test]
fn node_style_uses_first_nonexclusive_core_import_in_request_file() {
    for (main, expected) in [
        (
            "import 'node:test'; import 'fs'; import 'node:path';",
            Tristate::FALSE,
        ),
        ("import 'node:fs'; import 'path';", Tristate::TRUE),
        ("import 'node:test'; export {};", Tristate::UNKNOWN),
    ] {
        let program = barrel_program_with(CompilerOptions::default(), main);
        let source = program.source_file(b"/src/a.ts").unwrap().source();
        let ranking = Ranking::new(&program, source, &Preferences::default()).unwrap();
        assert_eq!(ranking.uri_style, expected);
    }
}
