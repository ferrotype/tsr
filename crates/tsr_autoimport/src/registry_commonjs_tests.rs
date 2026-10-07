use super::*;
use std::sync::Arc;
use tsr_core::{CompilerOptions, ModuleKind, Tristate};
use tsr_lsproto as lsp;

fn program(main: &str, dependency: &str) -> Arc<Program> {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/main.js", main.as_bytes());
    fs.insert_loaded(b"/lib.js", dependency.as_bytes());
    fs.insert_loaded(b"/package.json", b"{\"type\":\"commonjs\"}".as_slice());
    Arc::new(
        Program::load(
            tsr_compiler::ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        no_lib: Tristate::TRUE,
                        allow_js: Tristate::TRUE,
                        check_js: Tristate::TRUE,
                        module: ModuleKind::NODE20,
                        ..Default::default()
                    },
                    vec![
                        JsString::from_bytes(b"/main.js".as_slice()),
                        JsString::from_bytes(b"/lib.js".as_slice()),
                    ],
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
fn object_export_indexes_only_pinned_property_forms() {
    let program = program("LIB_VERSION", "const shorthand = 1; module.exports = { LIB_VERSION: 1, shorthand, renamed: shorthand, method() {}, ['computed']: 1, 'quoted': 1, ...{} };");
    let counters = tsr_arena::Counters::new();
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
    let mut checker = pool
        .checker_for_file_exclusive(program.source_file(b"/main.js").unwrap().source())
        .unwrap();
    let registry = Registry::build(&program, &mut checker, || false)
        .unwrap()
        .unwrap();
    let entries: Vec<_> = registry
        .index
        .entries()
        .iter()
        .filter(|e| e.id.module.as_bytes() == b"/lib.js")
        .collect();
    assert_eq!(
        entries
            .iter()
            .map(|e| e.id.name.as_bytes())
            .collect::<Vec<_>>(),
        [
            b"export=".as_slice(),
            b"LIB_VERSION",
            b"shorthand",
            b"renamed"
        ]
    );
    for entry in entries {
        assert_eq!(entry.syntax, ExportSyntax::CommonJsModuleExports);
    }
}

#[test]
fn indexed_object_property_selects_named_require_in_node_commonjs() {
    for main in ["module.exports.foo = 0;\nLIB_VERSION", "LIB_VERSION"] {
        let program = program(main, "module.exports = { LIB_VERSION: 1 };");
        let source = program.source_file(b"/main.js").unwrap().source();
        let counters = tsr_arena::Counters::new();
        let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
        let mut checker = pool.checker_for_file_exclusive(source).unwrap();
        let registry = Registry::build(&program, &mut checker, || false)
            .unwrap()
            .unwrap();
        let exports = registry.search(b"/main.js", b"LIB_VERSION");
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].id.name.as_bytes(), b"LIB_VERSION");
        let fixes = crate::fix::fixes(
            &program,
            &mut checker,
            source,
            exports[0],
            crate::fix::Usage::default(),
            &crate::Preferences::default(),
        )
        .unwrap();
        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].kind, lsp::AutoImportFixKind::ADD_NEW);
        assert_eq!(fixes[0].import_kind, lsp::ImportKind::NAMED);
        assert!(fixes[0].use_require);
        assert_eq!(fixes[0].module_specifier, "./lib");
        assert_eq!(fixes[0].name, "LIB_VERSION");
    }
}

#[test]
fn empty_or_nonobject_export_does_not_add_named_properties() {
    for dependency in [
        "module.exports = {};",
        "module.exports = function make() {};",
    ] {
        let program = program("missing", dependency);
        let counters = tsr_arena::Counters::new();
        let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
        let mut checker = pool
            .checker_for_file_exclusive(program.source_file(b"/main.js").unwrap().source())
            .unwrap();
        let registry = Registry::build(&program, &mut checker, || false)
            .unwrap()
            .unwrap();
        assert!(registry
            .index
            .entries()
            .iter()
            .all(|entry| entry.id.name.as_bytes() == b"export="));
    }
}
