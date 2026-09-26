//! C2 import-type regressions through loaded JS/TS modules and the public checker.
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

fn program(files: &[(&str, &str)]) -> (Arc<Program>, Arc<CheckerOwner>) {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for &(name, text) in files {
        fs.insert_loaded(name.as_bytes(), text.as_bytes());
    }
    let program = Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        target: ScriptTarget::ES2015,
                        module: ModuleKind::ESNEXT,
                        no_lib: Tristate::TRUE,
                        allow_js: Tristate::TRUE,
                        check_js: Tristate::TRUE,
                        ..Default::default()
                    },
                    files
                        .iter()
                        .map(|(name, _)| JsString::from_bytes(name.as_bytes()))
                        .collect(),
                ),
                host: Arc::new(fs.finish()),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(b"/no-default-lib".as_slice()),
                skip_module_resolution: false,
            },
            &mut FileCache::new(),
            &counters,
        )
        .unwrap(),
    );
    let owner = Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(generation, &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    );
    (program, owner)
}

#[test]
fn js_value_exports_missing_from_type_space_report_the_native_diagnostic() {
    // Pinned jsdocImportTypeReferenceToStringLiteral.errors.txt: FOO is a
    // value, not a type export. Being JS alone must not trigger a refusal.
    let (program, owner) = program(&[
        ("/b.js", "export const FOO = \"foo\";\n"),
        ("/a.js", "/** @type {import('./b').FOO} */\nlet x;\n"),
    ]);
    let source = program.file(b"/a.js").unwrap().source();
    let mut op = owner.operation().unwrap();
    for _ in 0..2 {
        let diagnostics = op.semantic_diagnostics(source).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, 2694);
        assert_eq!(diagnostics[0].loc.pos(), 25);
        assert_eq!(diagnostics[0].loc.end(), 28);
        assert_eq!(diagnostics[0].message_args[1].as_bytes(), b"FOO");
    }
}

#[test]
fn js_enum_value_does_not_become_an_import_type() {
    // Pinned enumTagImported.errors.txt distinguishes TS2694 on import types
    // from TS2749 on the value import subsequently used as a type.
    let (program, owner) = program(&[
        (
            "/mod1.js",
            "/** @enum {string} */\nexport const TestEnum = { ADD: 'add', REMOVE: 'remove' };\n",
        ),
        (
            "/type.js",
            "/** @typedef {import(\"./mod1\").TestEnum} TE */\n/** @type {TE} */\nconst test = 'add';\n/** @type {import(\"./mod1\").TestEnum} */\nconst tost = 'remove';\n",
        ),
        (
            "/value.js",
            "import { TestEnum } from \"./mod1\";\n/** @type {TestEnum} */\nconst tist = TestEnum.ADD;\n",
        ),
    ]);
    let mut op = owner.operation().unwrap();
    for (name, expected) in [("/type.js", vec![2694, 2694]), ("/value.js", vec![2749])] {
        let diagnostics = op
            .semantic_diagnostics(program.file(name.as_bytes()).unwrap().source())
            .unwrap();
        assert_eq!(
            diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn commonjs_typedef_lookup_keeps_type_and_value_meanings_separate() {
    // Pinned moduleExportAssignment7.errors.txt: import('./mod').buz is
    // valid beside module.exports, while typeof import('./mod').buz and
    // import('./mod').literal each report TS2694.
    let (program, owner) = program(&[
        (
            "/mod.js",
            "/** @typedef {() => number} buz */\nmodule.exports = { literal: \"\" };\n",
        ),
        (
            "/main.ts",
            "declare const fn: import('./mod').buz;\nconst answer: number = fn();\ntype Value = typeof import('./mod').literal;\ntype MissingValue = typeof import('./mod').buz;\ntype MissingType = import('./mod').literal;\n",
        ),
    ]);
    let mut op = owner.operation().unwrap();
    let source = program.file(b"/main.ts").unwrap().source();
    for _ in 0..2 {
        let diagnostics = op.semantic_diagnostics(source).unwrap();
        assert_eq!(
            diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
            [2694, 2694]
        );
        assert_eq!(diagnostics[0].message_args[1].as_bytes(), b"buz");
        assert_eq!(diagnostics[1].message_args[1].as_bytes(), b"literal");
    }
}

#[test]
fn commonjs_typedef_type_arguments_reach_the_normal_alias_instantiation() {
    // Boundary regression for getTypeFromImportTypeNode ->
    // resolveImportSymbolType -> getTypeReferenceType: the fallback returns
    // the alias symbol, so its type arguments still determine property types.
    let (program, owner) = program(&[
        (
            "/mod.js",
            "/** @template T\n * @typedef {{ value: T }} Box\n */\nmodule.exports = {};\n",
        ),
        (
            "/main.ts",
            "const good: import('./mod').Box<string> = { value: 'ok' };\nconst bad: import('./mod').Box<string> = { value: 1 };\n",
        ),
    ]);
    let mut op = owner.operation().unwrap();
    let diagnostics = op
        .semantic_diagnostics(program.file(b"/main.ts").unwrap().source())
        .unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, 2322);
    assert_eq!(diagnostics[0].message_args[0].as_bytes(), b"number");
    assert_eq!(diagnostics[0].message_args[1].as_bytes(), b"string");
}

#[test]
fn bare_import_type_arguments_keep_native_grammar_errors_and_error_type() {
    // Pinned importWithTypeArguments.errors.txt: checking the invalid import
    // still checks T and returns errorType instead of a production refusal.
    let (program, owner) =
        program(&[("/main.ts", "import<T>\nconst a = import<string, number>\n")]);
    let mut op = owner.operation().unwrap();
    let source = program.file(b"/main.ts").unwrap().source();
    let diagnostics = op.semantic_diagnostics(source).unwrap();
    let actual: Vec<_> = diagnostics
        .iter()
        .map(|d| (d.code, d.loc.pos(), d.loc.end()))
        .collect();
    assert_eq!(actual, [(1326, 0, 9), (2304, 7, 8), (1326, 20, 42)]);
    let file = program.file(b"/main.ts").unwrap();
    let view = file.bound().view().ast();
    let first = view
        .node_slice(view.node(source).unwrap().statements(view).unwrap())
        .unwrap()
        .get(0)
        .flatten()
        .unwrap();
    let expression = view.node(first).unwrap().expression().unwrap();
    assert_eq!(
        op.get_type_at_location(expression).unwrap(),
        op.builtin_type("errorType").unwrap()
    );
}
