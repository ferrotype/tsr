use super::*;
use std::sync::Arc;
use tsr_compiler::{FileCache, Program, ProgramOptions};
use tsr_jsstring::JsString;
use tsr_lsproto as lsp;

fn program(text: &[u8], dependency: &[u8], cache: &mut FileCache) -> Program {
    program_at(b"/main.ts", text, dependency, cache)
}
fn program_at(name: &[u8], text: &[u8], dependency: &[u8], cache: &mut FileCache) -> Program {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(name, text);
    fs.insert_loaded(b"/dep.ts", dependency);
    Program::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                tsr_core::CompilerOptions {
                    no_lib: tsr_core::Tristate::TRUE,
                    allow_js: tsr_core::Tristate::from(name.ends_with(b".js")),
                    ..Default::default()
                },
                vec![
                    JsString::from_bytes(name),
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
                format: &tsr_format::FormatCodeSettings::default(),
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
                    format: &tsr_format::FormatCodeSettings::default(),
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

#[test]
fn import_edits_preserve_multiline_ranges_and_native_comment_behavior() {
    for (source, expected, count) in [
        (
            "import {\n    A,\n} from './dep';",
            "import {\n    A,\n    Value,\n} from './dep';",
            2,
        ),
        // The pin copies the last member's trailing comment when printing the
        // synthesized separator, without deleting the original comment.
        (
            "import {\n    A // keep\n} from './dep';",
            "import {\n    A, // keep // keep\n    Value\n} from './dep';",
            2,
        ),
    ] {
        let p = program(
            source.as_bytes(),
            b"export const A=1; export const Value=2",
            &mut FileCache::new(),
        );
        let file = p.source_file(b"/main.ts").unwrap();
        let (edits, _) = edits::edits(
            file.bound().view().ast(),
            file.source(),
            &fix("Value", lsp::AddAsTypeOnly::NOT_ALLOWED),
            &edits::Options {
                format: &tsr_format::FormatCodeSettings::default(),
                single_quote: true,
                semicolons: true,
                prefer_type_only: false,
                verbatim: false,
                newline: "\n",
                usage: None,
            },
        )
        .unwrap();
        assert_eq!(edits.len(), count);
        assert_eq!(apply(source, edits), expected);
    }
}

// source: tsc/internal/ls/autoimport/aliasresolver_crash_test.go:TestAliasResolverGetDiagnosticsDoesNotPanic
#[test]
fn erroneous_export_initializers_do_not_abort_extraction() {
    let p = Arc::new(program(
        b"",
        b"declare function f(arg: { a: string }): () => void; export const x = f({ a: 1 });",
        &mut FileCache::new(),
    ));
    let pool = tsr_compiler::CompilerCheckerPool::new(p.clone(), &tsr_arena::Counters::new());
    let mut checker = pool
        .checker_for_file_exclusive(p.source_file(b"/dep.ts").unwrap().source())
        .unwrap();
    let registry = Registry::build(&p, &mut checker, || false)
        .unwrap()
        .unwrap();
    assert_eq!(registry.index.find(b"x", true).len(), 1);
    let errors = checker
        .semantic_diagnostics(p.source_file(b"/dep.ts").unwrap().source())
        .unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, 2322);
}

// source: tsc/internal/ls/autoimport/util_test.go:TestGetPackageRealpathFuncs_FollowsNodeModulesSymlinks
// source: tsc/internal/ls/autoimport/util_test.go:TestGetPackageRealpathFuncs_DuplicateCacheKeys
// source: tsc/internal/ls/autoimport/util_test.go:TestGetPackageRealpathFuncs_NonSymlinkedPackageWithSymlinkedDeps
#[test]
fn package_realpaths_share_dependency_keys_and_cache_directory_lookups() {
    use std::sync::Mutex;
    use tsr_vfs::FileSystem;
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_symlink(b"/workspace/app-a", b"/store/app-a");
    fs.insert_symlink(b"/workspace/app-b", b"/store/app-b");
    fs.insert_loaded(b"/store/app-a/index.d.ts", b"".as_slice());
    fs.insert_loaded(b"/store/app-b/index.d.ts", b"".as_slice());
    fs.insert_symlink(b"/store/app-a/node_modules/shared", b"/store/shared");
    fs.insert_symlink(b"/store/app-b/node_modules/shared", b"/store/shared");
    fs.insert_loaded(b"/store/shared/index.d.ts", b"".as_slice());
    fs.insert_loaded(b"/store/shared/src/helper.d.ts", b"".as_slice());
    let fs = Arc::new(fs.finish());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let host: Arc<dyn FileSystem> = Arc::new(tsr_vfs::wrapped::WrappedFs::new(
        fs.clone(),
        tsr_vfs::wrapped::Replacements {
            realpath: Some(Arc::new({
                let calls = calls.clone();
                move |path| {
                    calls.lock().unwrap().push(path.to_vec());
                    fs.realpath(path)
                }
            })),
            ..Default::default()
        },
    ));
    for root in [
        b"/workspace/app-a".as_slice(),
        b"/workspace/app-b",
        b"/store/app-a",
    ] {
        let paths = crate::realpaths::PackagePaths::new(host.clone(), root).unwrap();
        let real = host.realpath(root).unwrap();
        calls.lock().unwrap().clear();
        let input = [root, b"/index.d.ts"].concat();
        let expected = [real.as_bytes(), b"/index.d.ts"].concat();
        assert_eq!(paths.to_realpath(&input).unwrap().as_bytes(), expected);
        assert_eq!(paths.to_symlink(&expected).as_bytes(), input);
        let dependency = [real.as_bytes(), b"/node_modules/shared"].concat();
        for name in [
            b"/index.d.ts".as_slice(),
            b"/src/helper.d.ts",
            b"/index.d.ts",
        ] {
            assert_eq!(
                paths
                    .to_realpath(&[dependency.as_slice(), name].concat())
                    .unwrap()
                    .as_bytes(),
                [b"/store/shared".as_slice(), name].concat(),
            );
        }
        assert_eq!(*calls.lock().unwrap(), vec![dependency]);
        // The inverse only maps the primary package, not a dependency alias.
        assert_eq!(
            paths.to_symlink(b"/store/shared/index.d.ts").as_bytes(),
            b"/store/shared/index.d.ts"
        );
    }
}

// source: tsc/internal/ls/autoimport/registry_test.go:TestHiddenDirectoriesInNodeModules
// source: tsc/internal/ls/autoimport/registry_test.go:TestAutoImportEntrypointDirectorySearch
#[test]
fn package_discovery_ignores_hidden_directories_and_obeys_directory_search() {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, text) in [
        ("/main.ts", ""),
        (
            "/node_modules/.store/package.json",
            r#"{"types":"index.d.ts"}"#,
        ),
        (
            "/node_modules/.store/index.d.ts",
            "export const Hidden = 1;",
        ),
        (
            "/node_modules/visible/package.json",
            r#"{"types":"index.d.ts"}"#,
        ),
        (
            "/node_modules/visible/index.d.ts",
            "export const Visible = 1;",
        ),
        (
            "/node_modules/visible/deep/extra.d.ts",
            "export const Extra = 1;",
        ),
    ] {
        fs.insert_loaded(name.as_bytes(), text.as_bytes());
    }
    let host: Arc<dyn tsr_vfs::FileSystem> = Arc::new(fs.finish());
    let p = Program::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                tsr_core::CompilerOptions {
                    no_lib: tsr_core::Tristate::TRUE,
                    types: Some(vec![]),
                    ..Default::default()
                },
                vec![JsString::from_bytes(b"/main.ts".as_slice())],
            ),
            host: host.clone(),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/".as_slice()),
            skip_module_resolution: false,
            single_threaded: tsr_core::Tristate::TRUE,
        },
        &mut FileCache::new(),
        &tsr_arena::Counters::new(),
    )
    .unwrap();
    for (search, count) in [(false, 1), (true, 2)] {
        let packages = packages::discover(
            &p,
            b"/main.ts",
            &host,
            &Preferences {
                directory_search: Some(search),
                ..Default::default()
            },
            || false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name.as_bytes(), b"visible");
        assert_eq!(packages[0].entrypoints.len(), count);
        let loaded = packages[0].load(&p, &tsr_arena::Counters::new()).unwrap();
        assert_eq!(loaded.files().len(), count);
    }
}

#[test]
fn export_equals_keeps_the_function_alias_in_the_index() {
    let p = Arc::new(program(
        b"",
        b"declare function PkgFactory(): void; export = PkgFactory;",
        &mut FileCache::new(),
    ));
    let pool = tsr_compiler::CompilerCheckerPool::new(p.clone(), &tsr_arena::Counters::new());
    let mut checker = pool
        .checker_for_file_exclusive(p.source_file(b"/dep.ts").unwrap().source())
        .unwrap();
    let registry = Registry::build(&p, &mut checker, || false)
        .unwrap()
        .unwrap();
    let exports = registry.index.find(b"PkgFactory", true);
    assert_eq!(exports.len(), 1, "{:?}", registry.index.entries());
    assert_eq!(exports[0].id.name.as_bytes(), b"export=");
    assert_ne!(exports[0].flags & tsr_ast::symbol_flags::FUNCTION, 0);
}

#[test]
fn require_destructuring_uses_its_variable_declaration_and_applies_the_fix() {
    let text = "const { alpha } = require('./dep'); method();";
    let p = Arc::new(program_at(
        b"/main.js",
        text.as_bytes(),
        b"export const alpha = 1; export function method() {}",
        &mut FileCache::new(),
    ));
    let file = p.source_file(b"/main.js").unwrap();
    let pool = tsr_compiler::CompilerCheckerPool::new(p.clone(), &tsr_arena::Counters::new());
    let mut checker = pool.checker_for_file_exclusive(file.source()).unwrap();
    let registry = Registry::build(&p, &mut checker, || false)
        .unwrap()
        .unwrap();
    let export = registry.index.find(b"method", true)[0];
    let fixes = crate::fix::fixes(
        &p,
        &mut checker,
        file.source(),
        export,
        false,
        Some(lsp::Position {
            line: 0,
            character: 35,
        }),
        &Preferences::default(),
    )
    .unwrap();
    let fix = fixes
        .iter()
        .find(|fix| fix.kind == lsp::AutoImportFixKind::ADD_TO_EXISTING)
        .unwrap();
    let (edits, _) = edits::edits(
        file.bound().view().ast(),
        file.source(),
        fix,
        &edits::Options {
            format: &tsr_format::FormatCodeSettings::default(),
            single_quote: true,
            semicolons: true,
            prefer_type_only: false,
            verbatim: false,
            newline: "\n",
            usage: Some(35),
        },
    )
    .unwrap();
    assert_eq!(
        apply(text, edits),
        "const { alpha, method } = require('./dep'); method();"
    );
}
