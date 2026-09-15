//! Source-program regressions for the bounded production checker. The two
//! assignment diagnostics below are the pinned Go P2 observations, including
//! scanner ranges and diagnostic payload, not a second expression evaluator.

use std::sync::Arc;
use ts_arena::{CheckerIdentity, Counters, Generation, NodeId};
use ts_checker::{CheckerOwner, Error};
use ts_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use ts_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use ts_jsstring::JsString;

fn options() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ESNEXT,
        module: ModuleKind::ESNEXT,
        strict: Tristate::TRUE,
        no_lib: Tristate::TRUE,
        ..Default::default()
    }
}

fn checker(text: &[u8], options: CompilerOptions) -> (Arc<CheckerOwner>, NodeId) {
    let (checker, program, _) = fixture(text, options);
    (checker, program.file(b"/main.ts").unwrap().source())
}

fn fixture(text: &[u8], options: CompilerOptions) -> (Arc<CheckerOwner>, Arc<Program>, Counters) {
    fixture_files(b"/main.ts", &[(b"/main.ts", text)], options)
}

fn fixture_files(
    root: &[u8],
    files: &[(&[u8], &[u8])],
    options: CompilerOptions,
) -> (Arc<CheckerOwner>, Arc<Program>, Counters) {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let mut fs = ts_vfs::MemoryBuilder::new(b"/", true);
    for &(path, text) in files {
        fs.insert_loaded(path, text);
    }
    let program = Arc::new(
        Program::load(
            ProgramOptions {
                config: ts_tsoptions::ParsedCommandLine::new(
                    options,
                    std::iter::once(root)
                        .chain(
                            files
                                .iter()
                                .map(|&(path, _)| path)
                                .filter(|&path| path != root),
                        )
                        .map(JsString::from_bytes)
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
    let checker = CheckerOwner::for_program(
        CheckerIdentity::new(generation, &counters),
        &counters,
        Arc::new(ProgramCheckerHost::new(program.clone())),
    )
    .unwrap();
    (Arc::new(checker), program, counters)
}

#[test]
fn display_rejects_foreign_and_builder_generated_enclosing_nodes() {
    let (owner, source) = checker(b"let local = 1;", options());
    let (_foreign_owner, foreign_source) = checker(b"let foreign = 1;", options());
    let mut op = owner.operation().unwrap();
    let typ = op.builtin_type("stringType").unwrap();
    let symbol = op
        .new_symbol(ts_ast::symbol_flags::VARIABLE, b"local", 0)
        .unwrap();
    let symbol = op.symbol_ref(symbol).unwrap();
    let generated = op
        .node_builder()
        .type_to_type_node(typ, Some(source), 0, 0)
        .unwrap()
        .unwrap();
    for (case, enclosing) in [
        ("foreign program", foreign_source),
        ("dropped builder", generated),
    ] {
        assert_eq!(
            op.type_to_string_at(typ, Some(enclosing), 0),
            Err(Error::Arena(ts_arena::Error::WrongOwner)),
            "type string: {case}"
        );
        assert_eq!(
            op.symbol_to_string_at(symbol, Some(enclosing), 0, 0),
            Err(Error::Arena(ts_arena::Error::WrongOwner)),
            "symbol string: {case}"
        );
        let mut builder = op.node_builder();
        assert_eq!(
            builder.type_to_type_node(typ, Some(enclosing), 0, 0),
            Err(Error::Arena(ts_arena::Error::WrongOwner)),
            "type node: {case}"
        );
        assert!(builder
            .type_to_type_node(typ, Some(source), 0, 0)
            .unwrap()
            .is_some());
    }
    assert_eq!(
        op.type_to_string_at(typ, Some(source), 0)
            .unwrap()
            .as_bytes(),
        b"string"
    );
    assert_eq!(
        op.symbol_to_string_at(symbol, Some(source), 0, 0)
            .unwrap()
            .as_bytes(),
        b"local"
    );
}

#[test]
fn source_assignment_diagnostics_match_pinned_native_ranges_and_payload_on_repeat() {
    let requests: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/s08/p2/requests.json")).unwrap();
    let request = requests["programs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|request| request["id"] == "variables-and-failing-assignment")
        .unwrap();
    let text = request["files"]["/variables.ts"].as_str().unwrap();
    let (checker, source) = checker(text.as_bytes(), options());
    let expected_ranges = [(96, 99), (119, 124)];
    let mut previous = None;
    for _ in 0..3 {
        let actual = checker
            .operation()
            .unwrap()
            .semantic_diagnostics(source)
            .unwrap();
        assert_eq!(actual.len(), 2);
        for (diagnostic, (pos, end)) in actual.iter().zip(expected_ranges) {
            assert_eq!(diagnostic.file, Some(source));
            assert_eq!((diagnostic.loc.pos(), diagnostic.loc.end()), (pos, end));
            assert_eq!(diagnostic.code, 2322);
            assert_eq!(diagnostic.category, 1);
            assert_eq!(
                diagnostic.message_key.as_bytes(),
                b"Type_0_is_not_assignable_to_type_1_2322"
            );
            assert_eq!(
                diagnostic
                    .message_args
                    .iter()
                    .map(JsString::as_bytes)
                    .collect::<Vec<_>>(),
                [b"string".as_slice(), b"number"]
            );
            assert!(diagnostic.message_text.is_empty());
            assert!(diagnostic.source.is_empty());
            assert!(diagnostic.message_chain.is_empty());
            assert!(diagnostic.related_information.is_empty());
            assert!(
                !diagnostic.reports_unnecessary
                    && !diagnostic.reports_deprecated
                    && !diagnostic.skipped_on_no_emit
            );
        }
        if let Some(previous) = previous {
            assert_eq!(actual, previous);
        }
        previous = Some(actual);
    }
}

#[test]
fn source_check_repeats_its_result_across_operations_for_diagnostics_and_unported_input() {
    // `typeof before` resolves now, so the program that once stopped at an
    // unported boundary reports its assignment diagnostic instead. JSX input is
    // still unported, and a failure must repeat across operations just as a
    // diagnostic does.
    let (checker, source) = checker(
        b"let before: number = \"wrong\"; type Later = typeof before;",
        options(),
    );
    for _ in 0..3 {
        let diagnostics = checker
            .operation()
            .unwrap()
            .semantic_diagnostics(source)
            .unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, 2322);
        assert_eq!(
            (diagnostics[0].loc.pos(), diagnostics[0].loc.end()),
            (4, 10)
        );
    }
    let (owner, program, _) = fixture_files(
        b"/main.tsx",
        &[(b"/main.tsx", b"export const a = <div/>;")],
        options(),
    );
    let source = program.file(b"/main.tsx").unwrap().source();
    for _ in 0..3 {
        assert_eq!(
            owner.operation().unwrap().semantic_diagnostics(source),
            Err(Error::Unsupported("checkExpressionWorker"))
        );
    }
}

#[test]
fn grammar_relations_and_options_report_their_native_diagnostics() {
    // Each of these once stopped at an unported boundary. They are checked now,
    // so the pinned diagnostics themselves are the expectation; `let value!`
    // and the `@ts-ignore` directive are both no-ops at this layer.
    for (text, expected) in [
        (
            b"interface A { value: number; value: string }".as_slice(),
            vec![
                (2300, 14, 19, vec!["value"]),
                (2300, 29, 34, vec!["value"]),
                (2717, 29, 34, vec!["value", "number", "string"]),
            ],
        ),
        (
            b"let value: { field: number } = { field: 1, extra: 2 };",
            vec![(2353, 43, 48, vec!["extra", "{ field: number; }"])],
        ),
        (b"let value!: number;", vec![]),
        (
            b"// @ts-ignore\nlet value: number = \"wrong\";",
            vec![(2322, 18, 23, vec!["string", "number"])],
        ),
    ] {
        let (checker, source) = checker(text, options());
        let diagnostics = checker
            .operation()
            .unwrap()
            .semantic_diagnostics(source)
            .unwrap();
        let actual = diagnostics
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.code,
                    diagnostic.loc.pos(),
                    diagnostic.loc.end(),
                    diagnostic
                        .message_args
                        .iter()
                        .map(|argument| String::from_utf8_lossy(argument.as_bytes()).into_owned())
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        let expected = expected
            .into_iter()
            .map(|(code, pos, end, args)| {
                (
                    code,
                    pos,
                    end,
                    args.into_iter().map(str::to_string).collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "source {text:?}");
        // The later duplicate member points back at the first declaration.
        if let Some(duplicate) = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == 2717)
        {
            assert_eq!(duplicate.related_information.len(), 1);
            assert_eq!(duplicate.related_information[0].code, 6203);
            assert_eq!(
                (
                    duplicate.related_information[0].loc.pos(),
                    duplicate.related_information[0].loc.end()
                ),
                (14, 19)
            );
        }
    }
    // These options are accepted by the raw checker. Program-level noCheck
    // suppression and declaration diagnostics have separate entry points.
    for options in [
        CompilerOptions {
            no_check: Tristate::TRUE,
            ..options()
        },
        CompilerOptions {
            isolated_declarations: Tristate::TRUE,
            ..options()
        },
    ] {
        let (checker, source) = checker(b"let value: number = 1;", options);
        assert!(checker
            .operation()
            .unwrap()
            .semantic_diagnostics(source)
            .unwrap()
            .is_empty());
    }
}

#[test]
fn review_tsx_files_check_ordinary_declarations() {
    let (owner, program, _) = fixture_files(
        b"/main.tsx",
        &[(b"/main.tsx", b"export const value: number = \"wrong\";")],
        options(),
    );
    let source = program.file(b"/main.tsx").unwrap().source();
    for _ in 0..2 {
        let diagnostics = owner
            .operation()
            .unwrap()
            .semantic_diagnostics(source)
            .unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, 2322);
    }
}

#[test]
fn review_circular_parameter_initializers_preserve_native_diagnostics_on_repeat() {
    // Pinned Go observation: tools/s08/p4/review-regressions.json, circular-default.
    let (owner, source) = checker(
        b"function fn1(x: number | undefined = x > 0 ? x : 0) {}\nfunction fn2(x?: string = someCondition ? \"value1\" : x) {}\ntype Query = number;",
        options(),
    );
    let expected = [
        (2502, 13, 50),
        (2372, 37, 38),
        (18048, 37, 38),
        (2372, 45, 46),
        (1015, 68, 69),
        (2502, 68, 109),
        (2304, 81, 94),
        (2372, 108, 109),
    ];
    for _ in 0..3 {
        let diagnostics = owner
            .operation()
            .unwrap()
            .semantic_diagnostics(source)
            .unwrap();
        assert_eq!(
            diagnostics
                .iter()
                .map(|d| (d.code, d.loc.pos(), d.loc.end()))
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn review_nested_alias_resolution_keeps_each_circularity_target() {
    let files: &[(&[u8], &[u8])] = &[
        (
            b"/a.ts",
            b"import second = require(\"./b\"); var first = second; export = first;",
        ),
        (
            b"/b.ts",
            b"import third = require(\"./c\"); export = third;",
        ),
        (
            b"/c.ts",
            b"import first = require(\"./a\"); export = first;",
        ),
        (
            b"/case.ts",
            b"import first = require(\"./a\"); let value = first; type Query = typeof value;",
        ),
    ];
    let (owner, program, _) = fixture_files(b"/a.ts", files, options());
    for _ in 0..2 {
        let mut op = owner.operation().unwrap();
        for &(path, _) in files {
            op.semantic_diagnostics(program.file(path).unwrap().source())
                .unwrap();
        }
        let mut targets = Vec::new();
        for &(path, _) in files {
            for diagnostic in op
                .semantic_diagnostics(program.file(path).unwrap().source())
                .unwrap()
            {
                if diagnostic.code == 7022 {
                    targets.push((
                        path,
                        diagnostic.loc.pos(),
                        diagnostic.loc.end(),
                        diagnostic.message_args[0].as_bytes().to_vec(),
                    ));
                }
            }
        }
        // Go reports the local first and the export third, never the imported second.
        assert_eq!(
            targets,
            vec![
                (b"/a.ts".as_slice(), 36, 41, b"first".to_vec()),
                (b"/b.ts".as_slice(), 31, 46, b"third".to_vec())
            ]
        );
    }
}

#[test]
fn review_date_property_uses_the_pinned_lib_suggestion() {
    let (owner, source) = checker(
        b"interface Date {} declare const date: Date; date.toTemporalInstant();",
        options(),
    );
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, 2550);
    assert_eq!(
        diagnostics[0]
            .message_args
            .iter()
            .map(JsString::as_bytes)
            .collect::<Vec<_>>(),
        [b"toTemporalInstant".as_slice(), b"Date", b"esnext"]
    );
}

#[test]
fn source_check_reports_structural_assignment_failures() {
    // Pinned Go emits no chain for either shape: the elaborated property
    // mismatch carries a TS6500 note, and the missing property carries TS2728.
    for (text, code, range, note_code, note_range, note_args) in [
        (
            b"let value: { field: number } = { field: \"wrong\" };".as_slice(),
            2322,
            (33, 38),
            6500,
            (13, 18),
            vec!["field".to_string(), "{ field: number; }".to_string()],
        ),
        (
            b"let value: { field: number } = {};".as_slice(),
            2741,
            (4, 9),
            2728,
            (13, 18),
            vec!["field".to_string()],
        ),
    ] {
        let (owner, source) = checker(text, options());
        for _ in 0..2 {
            let diagnostics = owner
                .operation()
                .unwrap()
                .semantic_diagnostics(source)
                .unwrap();
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].code, code);
            assert_eq!(
                (diagnostics[0].loc.pos(), diagnostics[0].loc.end()),
                range,
                "diagnostic range for {text:?}"
            );
            assert!(diagnostics[0].message_chain.is_empty());
            assert_eq!(diagnostics[0].related_information.len(), 1);
            let note = &diagnostics[0].related_information[0];
            assert_eq!(note.code, note_code);
            assert_eq!((note.loc.pos(), note.loc.end()), note_range);
            assert_eq!(
                note.message_args
                    .iter()
                    .map(|argument| String::from_utf8_lossy(argument.as_bytes()).into_owned())
                    .collect::<Vec<_>>(),
                note_args
            );
        }
    }
}

#[test]
fn source_check_accepts_plain_structural_assignments_and_nonstrict_nullable_boolean() {
    for (text, options) in [
        (b"type Record = { label: string; count: number }; let value: Record = { label: \"ok\", count: 1 };".as_slice(), options()),
        (b"let flag: boolean = null;", CompilerOptions { strict_null_checks: Tristate::FALSE, ..options() }),
    ] {
        let (checker, source) = checker(text, options);
        assert!(checker.operation().unwrap().semantic_diagnostics(source).unwrap().is_empty());
    }
}

fn declarations(program: &Program) -> Vec<NodeId> {
    let file = program.file(b"/main.ts").unwrap();
    let view = file.bound().view().ast();
    view.node_slice(view.node(file.source()).unwrap().statements(view).unwrap())
        .unwrap()
        .iter()
        .flatten()
        .collect()
}

fn declaration_name(program: &Program, declaration: NodeId) -> NodeId {
    program
        .file(b"/main.ts")
        .unwrap()
        .bound()
        .view()
        .node(declaration)
        .unwrap()
        .name()
        .unwrap()
}

#[test]
fn source_symbol_references_are_bound_to_the_exact_checker_even_when_source_is_shared() {
    let (first, program, counters) = fixture(b"interface Shared { field: string }", options());
    let second = Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(Generation::new(&counters), &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    );
    let name = declaration_name(&program, declarations(&program)[0]);
    let mut first_op = first.operation().unwrap();
    let first_symbol = first_op.get_symbol_at_location(name).unwrap().unwrap();
    let ty = first_op.get_declared_type_of_symbol(first_symbol).unwrap();
    let retained = first_op.retain_type(ty).unwrap();
    let mut second_op = second.operation().unwrap();
    let second_symbol = second_op.get_symbol_at_location(name).unwrap().unwrap();
    assert_eq!(
        first_symbol.id(),
        second_symbol.id(),
        "shared bound symbol identity"
    );
    assert_ne!(
        first_symbol, second_symbol,
        "checker association is not source identity"
    );
    assert!(matches!(
        second_op.get_declared_type_of_symbol(first_symbol),
        Err(Error::Arena(ts_arena::Error::WrongOwner))
    ));
    assert!(matches!(
        second_op.properties_of_type(ty),
        Err(Error::Arena(ts_arena::Error::WrongOwner))
    ));
    assert!(second_op.get_declared_type_of_symbol(second_symbol).is_ok());
    let weak = Arc::downgrade(&program);
    drop(first_op);
    drop(second_op);
    drop(second);
    drop(first);
    drop(program);
    assert!(weak.upgrade().is_some());
    drop(retained);
    assert!(weak.upgrade().is_none());
}

#[test]
fn declared_type_queries_repeat_without_drift_and_unresolved_names_are_any() {
    let (owner, program, _) = fixture(b"interface Callable { (value: string): number } interface Generic<T> {} type Failed = typeof value; type Good = number;", options());
    let declarations = declarations(&program);
    for _ in 0..3 {
        let mut op = owner.operation().unwrap();
        let symbol = op
            .get_symbol_at_location(declaration_name(&program, declarations[0]))
            .unwrap()
            .unwrap();
        let callable = op.get_declared_type_of_symbol(symbol).unwrap();
        assert!(op.properties_of_type(callable).unwrap().is_empty());
        let generic_symbol = op
            .get_symbol_at_location(declaration_name(&program, declarations[1]))
            .unwrap()
            .unwrap();
        let generic = op.get_declared_type_of_symbol(generic_symbol).unwrap();
        assert_ne!(
            op.type_object_flags(generic).unwrap() & ts_checker::object_flags::REFERENCE,
            0
        );
        // `typeof value` on a name that resolves to nothing is `any`, not a failure.
        for &declaration in &declarations[2..3] {
            let symbol = op
                .get_symbol_at_location(declaration_name(&program, declaration))
                .unwrap()
                .unwrap();
            let failed = op.get_declared_type_of_symbol(symbol).unwrap();
            assert_eq!(op.type_to_string(failed, 0).unwrap().as_bytes(), b"any");
        }
        let symbol = op
            .get_symbol_at_location(declaration_name(&program, declarations[3]))
            .unwrap()
            .unwrap();
        let good = op.get_declared_type_of_symbol(symbol).unwrap();
        assert_eq!(op.type_to_string(good, 0).unwrap().as_bytes(), b"number");
    }
}

#[test]
fn literal_grammar_preserves_bigint_spelling_and_keeps_suggestions_out_of_errors() {
    let (owner, program, _) = fixture(b"type Hex = 0xffn; type Decimal = 255n; let large: number = -9007199254740993; let scientific: number = 9e99; let fractional: number = 9007199254740993.1; let value: bigint = 1n;", CompilerOptions { target: ScriptTarget::ES2019, ..options() });
    let source = program.file(b"/main.ts").unwrap().source();
    let decls = declarations(&program);
    let mut op = owner.operation().unwrap();
    let mut types = Vec::new();
    for &declaration in &decls[..2] {
        let symbol = op
            .get_symbol_at_location(declaration_name(&program, declaration))
            .unwrap()
            .unwrap();
        let ty = op.get_declared_type_of_symbol(symbol).unwrap();
        assert_eq!(op.type_to_string(ty, 0).unwrap().as_bytes(), b"255n");
        types.push(ty);
    }
    assert_eq!(types[0], types[1]);
    let errors = op.semantic_diagnostics(source).unwrap();
    assert_eq!(errors.iter().map(|d| d.code).collect::<Vec<_>>(), [2737]);
    let suggestions = op.recorded_suggestions(source).unwrap();
    assert_eq!(
        suggestions
            .iter()
            .map(|d| (d.code, d.category))
            .collect::<Vec<_>>(),
        [(80008, 2)]
    );
}

#[test]
fn an_outer_generic_interface_retains_outer_parameters_on_repeated_queries() {
    let (owner, program, _) = fixture(
        b"function outer<T>() { interface Nested { value: T } }",
        options(),
    );
    let view = program.file(b"/main.ts").unwrap().bound().view().ast();
    let function = declarations(&program)[0];
    let body = view.node(function).unwrap().body().unwrap();
    let nested = view
        .node_slice(view.node(body).unwrap().statements(view).unwrap())
        .unwrap()
        .get(0)
        .unwrap()
        .unwrap();
    let name = declaration_name(&program, nested);
    for _ in 0..2 {
        let mut op = owner.operation().unwrap();
        let symbol = op.get_symbol_at_location(name).unwrap().unwrap();
        let ty = op.get_declared_type_of_symbol(symbol).unwrap();
        assert_ne!(
            op.type_object_flags(ty).unwrap() & ts_checker::object_flags::REFERENCE,
            0
        );
        let property = op.properties_of_type(ty).unwrap()[0];
        let value = op.get_type_of_symbol(property).unwrap();
        assert_eq!(
            op.type_flags(value).unwrap(),
            ts_checker::type_flags::TYPE_PARAMETER
        );
        assert_eq!(op.type_to_string(value, 0).unwrap().as_bytes(), b"T");
    }
}

#[test]
fn lazy_jsdoc_type_names_do_not_resolve_as_ordinary_wrapper_interfaces() {
    use ts_ast::JsDocProvider;
    let (owner, program, _) = fixture(
        b"interface String { tag: number }\n/** @type {String} */\nlet value: string = \"ok\";",
        options(),
    );
    let file = program.file(b"/main.ts").unwrap();
    let view = file.bound().view().ast();
    let statement = declarations(&program)[1];
    let roots = ts_parser::ParserJsDocProvider::default()
        .jsdoc(view, file.source(), statement)
        .unwrap();
    let doc = view.node(roots[0]).unwrap();
    let tags = doc.data_source().as_js_doc().unwrap().tags().unwrap();
    let tag = view
        .node_slice(view.list(tags).unwrap().nodes())
        .unwrap()
        .at(0)
        .unwrap();
    let expression = view
        .node(tag)
        .unwrap()
        .data_source()
        .as_js_doc_type_tag()
        .unwrap()
        .type_expression()
        .unwrap();
    let reference = view.node(expression).unwrap().type_node().unwrap();
    assert_eq!(
        view.node(reference).unwrap().kind(),
        ts_ast::SyntaxKind::TypeReference
    );
    assert_ne!(
        reference.arena(),
        file.source().arena(),
        "exercises retained lazy owner routing"
    );
    // The tag names the primitive; the `interface String` declared above is a
    // different type, so the lazy JSDoc name must not resolve to that wrapper.
    for _ in 0..2 {
        let mut op = owner.operation().unwrap();
        let ty = op.get_type_at_location(reference).unwrap();
        assert_eq!(op.type_to_string(ty, 0).unwrap().as_bytes(), b"string");
        let wrapper = op
            .get_symbol_at_location(declaration_name(&program, declarations(&program)[0]))
            .unwrap()
            .unwrap();
        let wrapper = op.get_declared_type_of_symbol(wrapper).unwrap();
        assert_eq!(op.type_to_string(wrapper, 0).unwrap().as_bytes(), b"String");
        assert_ne!(ty, wrapper);
    }
}

#[test]
fn readonly_and_const_grammar_errors_do_not_cascade_into_unrelated_checks() {
    for (text, codes) in [
        (b"const a = 1; a = 2;".as_slice(), vec![2588]),
        (b"const let: number;", vec![1155]),
        // An ordinary let declaration still runs its name grammar check.
        (b"let let: number;", vec![2480]),
    ] {
        let (owner, source) = checker(text, options());
        for _ in 0..2 {
            let actual = owner
                .operation()
                .unwrap()
                .semantic_diagnostics(source)
                .unwrap();
            assert_eq!(actual.iter().map(|d| d.code).collect::<Vec<_>>(), codes);
        }
    }
    let (owner, source) = checker(
        b"const a = 1; a = 1n;",
        CompilerOptions {
            target: ScriptTarget::ES2019,
            ..options()
        },
    );
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
        [2588, 2737],
        "a readonly left side must not suppress right-side grammar errors"
    );
}

#[test]
fn reference_identifier_and_whole_reference_preserve_distinct_native_queries() {
    let (owner, program, _) = fixture(b"type Foo = number; type Alias = Foo;", options());
    let view = program.file(b"/main.ts").unwrap().bound().view().ast();
    let declaration = declarations(&program)[1];
    let reference = view.node(declaration).unwrap().type_node().unwrap();
    let name = view
        .node(reference)
        .unwrap()
        .data_source()
        .as_type_reference_node()
        .unwrap()
        .type_name()
        .unwrap();
    let mut op = owner.operation().unwrap();
    assert_eq!(
        op.get_type_at_location(name).unwrap(),
        op.builtin_type("errorType").unwrap()
    );
    assert_eq!(
        op.get_type_at_location(reference).unwrap(),
        op.builtin_type("numberType").unwrap()
    );
    let symbol = op.get_symbol_at_location(name).unwrap().unwrap();
    assert_eq!(
        op.get_declared_type_of_symbol(symbol).unwrap(),
        op.builtin_type("numberType").unwrap()
    );
}

#[test]
fn variable_widening_does_not_rebuild_a_successful_cached_initializer() {
    let (owner, program, _) = fixture(b"let value = { field: 1 };", options());
    let file = program.file(b"/main.ts").unwrap();
    let view = file.bound().view().ast();
    let statement = declarations(&program)[0];
    let list = view
        .node(statement)
        .unwrap()
        .data_source()
        .as_variable_statement()
        .unwrap()
        .declaration_list()
        .unwrap();
    let declarations = view
        .node(list)
        .unwrap()
        .data_source()
        .as_variable_declaration_list()
        .unwrap()
        .declarations()
        .unwrap();
    let declaration = view
        .node_slice(view.list(declarations).unwrap().nodes())
        .unwrap()
        .at(0)
        .unwrap();
    let name = view.node(declaration).unwrap().name().unwrap();
    let mut op = owner.operation().unwrap();
    let symbol = op.get_symbol_at_location(name).unwrap().unwrap();
    let widened = op.get_type_of_symbol(symbol).unwrap();
    assert_eq!(
        op.type_to_string(widened, 0).unwrap().as_bytes(),
        b"{ field: number; }"
    );
    let before = (op.type_count(), op.symbol_count());
    for _ in 0..2 {
        assert_eq!(op.get_type_of_symbol(symbol).unwrap(), widened);
        assert_eq!(
            (op.type_count(), op.symbol_count()),
            before,
            "retrying widening must reuse the checked initializer's type and property symbols"
        );
    }
    assert!(op.semantic_diagnostics(file.source()).unwrap().is_empty());
    assert_eq!((op.type_count(), op.symbol_count()), before);
}

#[test]
fn union_property_normalization_is_deferred_and_repeated_identity_is_stable() {
    let (owner, program, _) = fixture(b"type A = { value: 'a' }; type B = { value: 'b' }; type C = { value: 'c' }; type U = A | B | C;", options());
    let declarations = declarations(&program);
    let mut op = owner.operation().unwrap();
    // Resolve the three property types first. Discovering U's property should
    // create only its deferred symbol, with no normalized value union yet.
    for &declaration in &declarations[..3] {
        let symbol = op
            .get_symbol_at_location(declaration_name(&program, declaration))
            .unwrap()
            .unwrap();
        let ty = op.get_declared_type_of_symbol(symbol).unwrap();
        let property = op.properties_of_type(ty).unwrap()[0];
        op.get_type_of_symbol(property).unwrap();
    }
    let symbol = op
        .get_symbol_at_location(declaration_name(&program, declarations[3]))
        .unwrap()
        .unwrap();
    let union = op.get_declared_type_of_symbol(symbol).unwrap();
    let before = op.type_count();
    let properties = op.properties_of_type(union).unwrap();
    assert_eq!(properties.len(), 1);
    assert_eq!(
        op.type_count(),
        before,
        "discovery must defer the property type union"
    );
    assert_ne!(
        op.symbol(properties[0]).unwrap().check_flags() & ts_ast::check_flags::DEFERRED_TYPE,
        0
    );
    let ty = op.get_type_of_symbol(properties[0]).unwrap();
    assert_eq!(op.type_count(), before + 1);
    assert_eq!(
        op.type_to_string(ty, 0).unwrap().as_bytes(),
        b"\"a\" | \"b\" | \"c\""
    );
    let counts = (op.type_count(), op.symbol_count());
    for _ in 0..3 {
        assert_eq!(op.properties_of_type(union).unwrap(), properties);
        assert_eq!(op.get_type_of_symbol(properties[0]).unwrap(), ty);
        assert_eq!((op.type_count(), op.symbol_count()), counts);
    }
}

#[test]
fn union_properties_publish_a_complete_list_and_repeat_without_drift() {
    let (owner, program, _) = fixture(b"type A = { good: string; bad: typeof missingA }; type B = { good: number; bad: typeof missingB }; type U = A | B;", options());
    let name = declaration_name(&program, declarations(&program)[2]);
    let mut op = owner.operation().unwrap();
    let symbol = op.get_symbol_at_location(name).unwrap().unwrap();
    let ty = op.get_declared_type_of_symbol(symbol).unwrap();
    let mut previous_counts = None;
    for _ in 0..3 {
        // `bad` names an unresolvable value on both sides, which is `any` rather
        // than a failure, so the whole list is published.
        let properties = op.properties_of_type(ty).unwrap();
        assert_eq!(properties.len(), 2);
        let good = op.get_type_of_symbol(properties[0]).unwrap();
        assert_eq!(
            op.type_to_string(good, 0).unwrap().as_bytes(),
            b"string | number"
        );
        let bad = op.get_type_of_symbol(properties[1]).unwrap();
        assert_eq!(op.type_to_string(bad, 0).unwrap().as_bytes(), b"any");
        let counts = (op.type_count(), op.symbol_count());
        if let Some(previous) = previous_counts {
            assert_eq!(counts, previous);
        }
        previous_counts = Some(counts);
    }
}

#[test]
fn recursive_union_properties_keep_the_original_type_on_a_small_stack() {
    std::thread::Builder::new().stack_size(512 * 1024).spawn(|| {
        let (owner, program, _) = fixture(b"type A = { next: U; a: string }; type B = { next: U; b: number }; type U = A | B;", options());
        let mut op = owner.operation().unwrap();
        let symbol = op.get_symbol_at_location(declaration_name(&program, declarations(&program)[2])).unwrap().unwrap();
        let union = op.get_declared_type_of_symbol(symbol).unwrap();
        let properties = op.properties_of_type(union).unwrap();
        assert_eq!(properties.len(), 1);
        assert_eq!(op.symbol(properties[0]).unwrap().name_bytes(), b"next");
        assert_eq!(op.get_type_of_symbol(properties[0]).unwrap(), union);
        assert_eq!(op.properties_of_type(union).unwrap(), properties);
        assert!(op.semantic_diagnostics(program.file(b"/main.ts").unwrap().source()).unwrap().is_empty());
    }).unwrap().join().unwrap();
}

#[test]
fn primitive_union_diagnostics_preserve_literal_target_spelling() {
    let (owner, source) = checker(b"type Allowed = string | number; let good: Allowed = 1; let bad: Allowed = true; let wrong: 'a' | 'b' = 'c';", options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
        [2322, 2322]
    );
    assert_eq!(
        diagnostics[0]
            .message_args
            .iter()
            .map(JsString::as_bytes)
            .collect::<Vec<_>>(),
        [b"boolean".as_slice(), b"Allowed"]
    );
    assert_eq!(
        diagnostics[1]
            .message_args
            .iter()
            .map(JsString::as_bytes)
            .collect::<Vec<_>>(),
        [b"\"c\"".as_slice(), b"\"a\" | \"b\""]
    );
    let (owner, source) = checker(
        b"type A = { value?: number }; type B = { value: string }; type U = A | B;",
        options(),
    );
    assert!(owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap()
        .is_empty());
}

#[test]
fn compound_constituents_are_checked_even_after_reduction_and_on_retry() {
    // The duplicate member trio is TS2300 twice then TS2717; an unresolvable
    // type name is a single TS2304. Both survive reduction and repeat exactly.
    for (text, codes) in [
        (
            "type U = { a: string; a: number } | string;",
            vec![2300, 2300, 2717],
        ),
        (
            "type U = string | { a: string; a: number };",
            vec![2300, 2300, 2717],
        ),
        (
            "type U = unknown | ({ a: string; a: number } | string);",
            vec![2300, 2300, 2717],
        ),
        ("type U = { a: Missing } | string;", vec![2304]),
        ("type U = unknown | { a: Missing };", vec![2304]),
        (
            "type U = unknown & { a: string; a: number };",
            vec![2300, 2300, 2717],
        ),
        ("type U = never & { a: Missing };", vec![2304]),
        // Checking follows source order, before construction sorts/reduces
        // types: the same pair of constituents reports in the order written.
        (
            "type U = { a: string; a: number } | typeof missing;",
            vec![2300, 2300, 2717, 2304],
        ),
        (
            "type U = typeof missing | { a: string; a: number };",
            vec![2304, 2300, 2300, 2717],
        ),
    ] {
        let (owner, source) = checker(text.as_bytes(), options());
        for _ in 0..3 {
            let diagnostics = owner
                .operation()
                .unwrap()
                .semantic_diagnostics(source)
                .unwrap();
            assert_eq!(
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.code)
                    .collect::<Vec<_>>(),
                codes,
                "{text}"
            );
        }
    }
    let (owner, source) = checker(b"type U = { a: string } | number;", options());
    assert!(owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap()
        .is_empty());
}

#[test]
fn a_unique_symbol_widens_in_a_mutable_object_literal_location() {
    // Upstream widens a unique symbol outside its const-like declaration, so a
    // later declaration serialization never reaches an inaccessible one.
    let (owner, program, _) = fixture(
        b"declare function Symbol(): symbol; const key = Symbol(); const inner = { key }; type Probe = typeof inner;",
        options(),
    );
    let mut op = owner.operation().unwrap();
    let symbol = op
        .get_symbol_at_location(declaration_name(&program, declarations(&program)[3]))
        .unwrap()
        .unwrap();
    let ty = op.get_declared_type_of_symbol(symbol).unwrap();
    assert_eq!(
        op.type_to_string(ty, 0).unwrap().as_bytes(),
        b"{ key: symbol; }"
    );
}

#[test]
fn an_incompatible_property_reports_the_index_signature_wrapper_chain() {
    // A fresh object literal elaborates to the offending property instead, so
    // the wrapper is only observable through an already-typed source.
    let (owner, source) = checker(
        b"interface Target { [key: string]: string }\ndeclare const source: { type: number };\nconst check: Target = source;",
        options(),
    );
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, 2322);
    let wrapper = diagnostics[0]
        .message_chain
        .first()
        .expect("index signature wrapper");
    assert_eq!(wrapper.code, 2530);
    assert_eq!(
        wrapper
            .message_args
            .first()
            .map(|argument| argument.as_bytes().to_vec()),
        Some(b"type".to_vec())
    );
    assert_eq!(
        wrapper.message_chain.first().map(|inner| inner.code),
        Some(2322)
    );
}

#[test]
fn intersection_discriminant_reduction_is_lazy_and_raw_display_is_available() {
    use ts_checker::{object_flags as of, type_format_flags as ff};
    let (owner, program, _) = fixture(
        b"type A = { tag: 'a' }; type B = { tag: 'b' }; type I = A & B;",
        options(),
    );
    let mut op = owner.operation().unwrap();
    let symbol = op
        .get_symbol_at_location(declaration_name(&program, declarations(&program)[2]))
        .unwrap()
        .unwrap();
    let ty = op.get_declared_type_of_symbol(symbol).unwrap();
    assert_eq!(
        op.type_object_flags(ty).unwrap() & of::IS_NEVER_INTERSECTION_COMPUTED,
        0
    );
    assert_eq!(
        op.type_to_string(ty, ff::NO_TYPE_REDUCTION | ff::IN_TYPE_ALIAS)
            .unwrap()
            .as_bytes(),
        b"A & B"
    );
    assert_eq!(
        op.type_object_flags(ty).unwrap() & of::IS_NEVER_INTERSECTION_COMPUTED,
        0
    );
    assert_eq!(op.type_to_string(ty, 0).unwrap().as_bytes(), b"never");
    assert_ne!(
        op.type_object_flags(ty).unwrap() & of::IS_NEVER_INTERSECTION,
        0
    );
    assert!(op.properties_of_type(ty).unwrap().is_empty());
    let counts = (op.type_count(), op.symbol_count());
    for _ in 0..3 {
        assert_eq!(op.type_to_string(ty, 0).unwrap().as_bytes(), b"never");
        assert_eq!(
            op.type_to_string(ty, ff::NO_TYPE_REDUCTION | ff::IN_TYPE_ALIAS)
                .unwrap()
                .as_bytes(),
            b"A & B"
        );
        assert_eq!((op.type_count(), op.symbol_count()), counts);
    }
}

#[test]
fn intersection_reduction_computes_its_never_flag_once_and_repeats_without_drift() {
    let (owner, program, _) = fixture(b"type A = { good: string; bad: typeof missingA }; type B = { good: number; bad: typeof missingB }; type I = A & B;", options());
    let mut op = owner.operation().unwrap();
    let symbol = op
        .get_symbol_at_location(declaration_name(&program, declarations(&program)[2]))
        .unwrap()
        .unwrap();
    let ty = op.get_declared_type_of_symbol(symbol).unwrap();
    // Discovering the alias must not reduce it.
    assert_eq!(
        op.type_object_flags(ty).unwrap()
            & ts_checker::object_flags::IS_NEVER_INTERSECTION_COMPUTED,
        0
    );
    let mut counts = None;
    for _ in 0..3 {
        // `good` reduces to never, but `I` itself is not a never intersection.
        let properties = op.properties_of_type(ty).unwrap();
        assert_eq!(properties.len(), 2);
        let good = op.get_type_of_symbol(properties[0]).unwrap();
        assert_eq!(op.type_to_string(good, 0).unwrap().as_bytes(), b"never");
        let bad = op.get_type_of_symbol(properties[1]).unwrap();
        assert_eq!(op.type_to_string(bad, 0).unwrap().as_bytes(), b"any");
        assert_eq!(op.type_to_string(ty, 0).unwrap().as_bytes(), b"I");
        assert_ne!(
            op.type_object_flags(ty).unwrap()
                & ts_checker::object_flags::IS_NEVER_INTERSECTION_COMPUTED,
            0
        );
        assert_eq!(
            op.type_object_flags(ty).unwrap() & ts_checker::object_flags::IS_NEVER_INTERSECTION,
            0
        );
        let current = (op.type_count(), op.symbol_count());
        if let Some(counts) = counts {
            assert_eq!(current, counts);
        }
        counts = Some(current);
    }
}

#[test]
fn deferred_intersection_property_builds_an_intersection_and_caches_its_identity() {
    let (owner, program, _) = fixture(b"type PA = { a: string }; type PB = { b: number }; type PC = { c: boolean }; type A = { value: PA }; type B = { value: PB }; type C = { value: PC }; type I = A & B & C;", options());
    let declarations = declarations(&program);
    let mut op = owner.operation().unwrap();
    for &declaration in &declarations[3..6] {
        let symbol = op
            .get_symbol_at_location(declaration_name(&program, declaration))
            .unwrap()
            .unwrap();
        let ty = op.get_declared_type_of_symbol(symbol).unwrap();
        let property = op.properties_of_type(ty).unwrap()[0];
        op.get_type_of_symbol(property).unwrap();
    }
    let symbol = op
        .get_symbol_at_location(declaration_name(&program, declarations[6]))
        .unwrap()
        .unwrap();
    let ty = op.get_declared_type_of_symbol(symbol).unwrap();
    let before = op.type_count();
    let properties = op.properties_of_type(ty).unwrap();
    assert_eq!(properties.len(), 1);
    assert_ne!(
        op.symbol(properties[0]).unwrap().check_flags() & ts_ast::check_flags::DEFERRED_TYPE,
        0
    );
    assert_eq!(
        op.type_count(),
        before,
        "property discovery must defer normalization"
    );
    let value = op.get_type_of_symbol(properties[0]).unwrap();
    assert_eq!(op.type_count(), before + 1);
    assert_eq!(
        op.type_flags(value).unwrap(),
        ts_checker::type_flags::INTERSECTION
    );
    assert_eq!(
        op.type_to_string(value, 0).unwrap().as_bytes(),
        b"PA & PB & PC"
    );
    let counts = (op.type_count(), op.symbol_count());
    for _ in 0..3 {
        assert_eq!(op.properties_of_type(ty).unwrap(), properties);
        assert_eq!(op.get_type_of_symbol(properties[0]).unwrap(), value);
        assert_eq!((op.type_count(), op.symbol_count()), counts);
    }
}

#[test]
fn recursive_intersection_properties_preserve_order_and_identity_on_a_small_stack() {
    std::thread::Builder::new().stack_size(512 * 1024).spawn(|| {
        let (owner, program, _) = fixture(b"type A = { next: I; a: string }; type B = { next: I; b: number }; type I = A & B;", options());
        let mut op = owner.operation().unwrap();
        let symbol = op.get_symbol_at_location(declaration_name(&program, declarations(&program)[2])).unwrap().unwrap();
        let ty = op.get_declared_type_of_symbol(symbol).unwrap();
        let properties = op.properties_of_type(ty).unwrap();
        let names: Vec<_> = properties.iter().map(|&prop| op.symbol(prop).unwrap().name_bytes().to_vec()).collect();
        assert_eq!(names, [b"next".to_vec(), b"a".to_vec(), b"b".to_vec()]);
        // Native displays next as A & B, not I: intersection construction
        // flattens I & I and interns an unaliased A & B result.
        let next = op.get_type_of_symbol(properties[0]).unwrap();
        assert_ne!(next, ty);
        assert_eq!(op.type_to_string(next, 0).unwrap().as_bytes(), b"A & B");
        assert_eq!(op.get_type_of_symbol(properties[0]).unwrap(), next);
        assert_eq!(op.properties_of_type(ty).unwrap(), properties);
        assert!(op.semantic_diagnostics(program.file(b"/main.ts").unwrap().source()).unwrap().is_empty());
    }).unwrap().join().unwrap();
}

#[test]
fn delete_operands_must_be_optional_writable_property_references() {
    let (owner, source) = checker(
        b"declare const o: { a?: number; b: number; readonly c?: number };\ndelete o.a;\ndelete o.b;\ndelete o.c;\ndelete 1;\n",
        options(),
    );
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    let codes: Vec<i32> = diagnostics.iter().map(|d| d.code).collect();
    assert_eq!(
        codes,
        vec![
            ts_diagnostics::The_operand_of_a_delete_operator_must_be_optional.code,
            ts_diagnostics::The_operand_of_a_delete_operator_cannot_be_a_read_only_property.code,
            ts_diagnostics::The_operand_of_a_delete_operator_must_be_a_property_reference.code,
        ]
    );
}

#[test]
fn meta_properties_need_their_containers_and_module_targets() {
    let (owner, source) = checker(
        b"const outer = new.target;\nfunction f() { return new.target; }\n",
        options(),
    );
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code,
        ts_diagnostics::Meta_property_0_is_only_allowed_in_the_body_of_a_function_declaration_function_expression_or_constructor.code
    );
    assert_eq!(
        (diagnostics[0].loc.pos(), diagnostics[0].loc.end()),
        (14, 24),
        "the diagnostic spans `new.target` without its leading trivia"
    );
    let mut es2015 = options();
    es2015.module = ModuleKind::ES2015;
    let (owner, source) = checker(b"const m = import.meta;\n", es2015);
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    let codes: Vec<i32> = diagnostics.iter().map(|d| d.code).collect();
    assert_eq!(
        codes,
        vec![ts_diagnostics::The_import_meta_meta_property_is_only_allowed_when_the_module_option_is_es2020_es2022_esnext_system_node16_node18_node20_or_nodenext.code]
    );
}

#[test]
fn regular_expression_literals_report_grammar_errors_once() {
    let (owner, source) = checker(b"const a = /x/gg;\nconst b = /y/i;\n", options());
    for _ in 0..2 {
        let diagnostics = owner
            .operation()
            .unwrap()
            .semantic_diagnostics(source)
            .unwrap();
        let codes: Vec<i32> = diagnostics.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            vec![ts_diagnostics::Duplicate_regular_expression_flag.code],
            "one flag error for the duplicated `g`, none for `/y/i`"
        );
    }
}

#[test]
fn debugger_statements_in_ambient_blocks_report_once_per_block() {
    let (owner, source) = checker(
        b"declare namespace N { debugger; debugger; }\ndeclare namespace M { debugger; }\n",
        options(),
    );
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    let codes: Vec<i32> = diagnostics.iter().map(|d| d.code).collect();
    assert_eq!(
        codes,
        vec![
            ts_diagnostics::Statements_are_not_allowed_in_ambient_contexts.code,
            ts_diagnostics::Statements_are_not_allowed_in_ambient_contexts.code,
        ]
    );
}

fn codes_and_args(diagnostics: &[ts_ast::Diagnostic]) -> Vec<(i32, Vec<String>)> {
    diagnostics
        .iter()
        .map(|d| {
            (
                d.code,
                d.message_args
                    .iter()
                    .map(|argument| String::from_utf8_lossy(argument.as_bytes()).into_owned())
                    .collect(),
            )
        })
        .collect()
}

const UNUSED_FIXTURE: &[u8] = b"export {};
let unused = 1;
let used = 2;
export const value = used;
let p = 1, q = 2;
function helper<T, U>(a: number, _b: string): void { let x = 1; }
helper(1, \"\");
class C {
    private secret = 1;
    private read = 2;
    method() { return this.read; }
    constructor(private param: number) {}
}
new C(1);
";

#[test]
fn no_unused_locals_reports_locals_and_private_members_as_errors() {
    let (owner, source) = checker(
        UNUSED_FIXTURE,
        CompilerOptions {
            no_unused_locals: Tristate::TRUE,
            ..options()
        },
    );
    let mut op = owner.operation().unwrap();
    let diagnostics = op.semantic_diagnostics(source).unwrap();
    let never_read = ts_diagnostics::X_0_is_declared_but_its_value_is_never_read.code;
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![
            (never_read, vec!["unused".to_string()]),
            (ts_diagnostics::All_variables_are_unused.code, vec![]),
            (never_read, vec!["x".to_string()]),
            (never_read, vec!["secret".to_string()]),
            (
                ts_diagnostics::Property_0_is_declared_but_its_value_is_never_read.code,
                vec!["param".to_string()]
            ),
        ]
    );
    // Parameters and type parameters are suggestions while noUnusedParameters is off.
    let suggestions = op.recorded_suggestions(source).unwrap();
    assert_eq!(
        codes_and_args(&suggestions),
        vec![
            (ts_diagnostics::All_type_parameters_are_unused.code, vec![]),
            (never_read, vec!["a".to_string()]),
        ]
    );
    assert!(suggestions
        .iter()
        .all(|d| d.category == ts_diagnostics::Category::Suggestion as i32));
}

#[test]
fn no_unused_parameters_reports_parameters_and_type_parameter_lists() {
    let (owner, source) = checker(
        UNUSED_FIXTURE,
        CompilerOptions {
            no_unused_parameters: Tristate::TRUE,
            ..options()
        },
    );
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![
            (ts_diagnostics::All_type_parameters_are_unused.code, vec![]),
            (
                ts_diagnostics::X_0_is_declared_but_its_value_is_never_read.code,
                vec!["a".to_string()]
            ),
        ]
    );
    let text = std::str::from_utf8(UNUSED_FIXTURE).unwrap();
    let start = text.find("<T, U>").unwrap() as i64;
    assert_eq!(
        (diagnostics[0].loc.pos(), diagnostics[0].loc.end()),
        (start, start + "<T, U>".len() as i64),
        "the list range spans both angle brackets"
    );
}

#[test]
fn unused_reports_are_suggestions_without_the_options() {
    let (owner, source) = checker(b"export {};\nlet unused = 1;\n", options());
    let mut op = owner.operation().unwrap();
    assert!(op.semantic_diagnostics(source).unwrap().is_empty());
    let suggestions = op.recorded_suggestions(source).unwrap();
    assert_eq!(
        codes_and_args(&suggestions),
        vec![(
            ts_diagnostics::X_0_is_declared_but_its_value_is_never_read.code,
            vec!["unused".to_string()]
        )]
    );
    assert_eq!(
        suggestions[0].category,
        ts_diagnostics::Category::Suggestion as i32
    );
}

#[test]
fn renamed_binding_elements_in_function_types_are_errors_regardless_of_options() {
    let typed = b"type F = ({ a: renamed }: { a: string }) => void;\n";
    let (owner, source) = checker(typed, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    let code = ts_diagnostics::X_0_is_an_unused_renaming_of_1_Did_you_intend_to_use_it_as_a_type_annotation.code;
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(code, vec!["renamed".to_string(), "a".to_string()])]
    );
    assert!(diagnostics[0].related_information.is_empty());

    let untyped = b"type F = ({ a: renamed }) => void;\n";
    let (owner, source) = checker(
        untyped,
        CompilerOptions {
            no_implicit_any: Tristate::FALSE,
            ..options()
        },
    );
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(code, vec!["renamed".to_string(), "a".to_string()])]
    );
    let related = &diagnostics[0].related_information;
    assert_eq!(related.len(), 1);
    assert_eq!(
        related[0].code,
        ts_diagnostics::We_can_only_write_a_type_for_0_by_adding_a_type_for_the_entire_parameter_here
            .code
    );
    let end = untyped.iter().position(|&b| b == b')').unwrap() as i64;
    assert_eq!((related[0].loc.pos(), related[0].loc.end()), (end, end));
}

#[test]
fn excess_properties_report_the_offending_property_with_spelling_suggestions() {
    let plain = b"let value: { field: number } = { field: 1, extra: 2 };";
    let (owner, source) = checker(plain, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::Object_literal_may_only_specify_known_properties_and_0_does_not_exist_in_type_1.code,
            vec!["extra".to_string(), "{ field: number; }".to_string()]
        )]
    );
    let start = plain.windows(5).position(|w| w == b"extra").unwrap() as i64;
    assert_eq!(
        (diagnostics[0].loc.pos(), diagnostics[0].loc.end()),
        (start, start + 5),
        "the property name is the error node"
    );
    assert!(diagnostics[0].message_chain.is_empty());

    let misspelled = b"let value: { field: number } = { feild: 1 };";
    let (owner, source) = checker(misspelled, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::Object_literal_may_only_specify_known_properties_but_0_does_not_exist_in_type_1_Did_you_mean_to_write_2.code,
            vec![
                "feild".to_string(),
                "{ field: number; }".to_string(),
                "field".to_string()
            ]
        )]
    );

    let discriminated = b"let value: { kind: \"a\"; x: number } | { kind: \"b\"; y: number } = { kind: \"a\", y: 1 };";
    let (owner, source) = checker(discriminated, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::Object_literal_may_only_specify_known_properties_and_0_does_not_exist_in_type_1.code,
            vec!["y".to_string(), "{ kind: \"a\"; x: number; }".to_string()]
        )],
        "the matching discriminant narrows the reported target"
    );
}

#[test]
fn commonjs_files_cannot_import_ecmascript_modules_synchronously_under_node16() {
    let node16 = CompilerOptions {
        module: ModuleKind::NODE16,
        module_resolution: ts_core::ModuleResolutionKind::NODE16,
        ..options()
    };
    let main = b"import { x } from \"./esm.mjs\";\nx;\n";
    let (owner, program, _) = fixture_files(
        b"/main.ts",
        &[(b"/main.ts", main), (b"/esm.mts", b"export const x = 1;\n")],
        node16.clone(),
    );
    let source = program.file(b"/main.ts").unwrap().source();
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::The_current_file_is_a_CommonJS_module_whose_imports_will_produce_require_calls_however_the_referenced_file_is_an_ECMAScript_module_and_cannot_be_imported_with_require_Consider_writing_a_dynamic_import_0_call_instead.code,
            vec!["./esm.mjs".to_string()]
        )]
    );
    let start = main.iter().position(|&b| b == b'"').unwrap() as i64;
    assert_eq!(
        (diagnostics[0].loc.pos(), diagnostics[0].loc.end()),
        (start, start + "\"./esm.mjs\"".len() as i64),
        "the specifier is the error node"
    );
    assert_eq!(diagnostics[0].message_chain.len(), 1);
    let details = &diagnostics[0].message_chain[0];
    assert_eq!(
        details.code,
        ts_diagnostics::To_convert_this_file_to_an_ECMAScript_module_change_its_file_extension_to_0_or_create_a_local_package_json_file_with_type_Colon_module.code
    );
    assert_eq!(
        codes_and_args(std::slice::from_ref(details))[0].1,
        vec![".mts".to_string()]
    );

    let type_only = b"import type { x } from \"./esm.mjs\";\nlet value: typeof x = 1;\nvalue;\n";
    let (owner, program, _) = fixture_files(
        b"/main.ts",
        &[
            (b"/main.ts", type_only),
            (b"/esm.mts", b"export const x = 1;\n"),
        ],
        node16,
    );
    let source = program.file(b"/main.ts").unwrap().source();
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::Type_only_import_of_an_ECMAScript_module_from_a_CommonJS_module_must_have_a_resolution_mode_attribute.code,
            vec!["./esm.mjs".to_string()]
        )]
    );
}

#[test]
fn classes_extending_an_any_base_check_without_resolving_members_on_any() {
    let text = b"declare var Err: any;\nclass A extends Err {\n    payload: string;\n    constructor() {\n        super(1, 2);\n        super.unknown;\n        super[\"unknown\"];\n    }\n    process() { return this.payload + \"!\"; }\n}\nvar o = { m() { super.unknown; } };\n";
    let (owner, source) = checker(text, options());
    let diagnostics = owner.operation().unwrap().semantic_diagnostics(source);
    assert!(diagnostics.is_ok(), "{diagnostics:?}");
}

#[test]
fn using_declarations_report_grammar_and_disposable_initializer_errors() {
    const GLOBALS: &[u8] = b"interface Disposable { dispose(): void }
interface AsyncDisposable { asyncDispose(): void }
";
    let script = b"interface Disposable { dispose(): void }
interface AsyncDisposable { asyncDispose(): void }
using bad = 1;
using good = { dispose() {} };
switch (1) { case 1: using inClause = null; }
";
    let (owner, source) = checker(script, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    // The relation error carries the generalized source and the nullable-stripped
    // target as arguments even though the head message has no placeholders.
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![
            (
                ts_diagnostics::The_initializer_of_a_using_declaration_must_be_either_an_object_with_a_Symbol_dispose_method_or_be_null_or_undefined.code,
                vec!["number".to_string(), "Disposable".to_string()]
            ),
            (
                ts_diagnostics::X_using_declarations_are_not_allowed_in_case_or_default_clauses_unless_contained_within_a_block.code,
                vec![]
            ),
        ]
    );
    let start = script.windows(7).position(|w| w == b"bad = 1").unwrap() as i64 + 6;
    assert_eq!(
        (diagnostics[0].loc.pos(), diagnostics[0].loc.end()),
        (start, start + 1),
        "the initializer is the error node"
    );

    // `await using` at the top level of a module checks against AsyncDisposable | Disposable.
    let module = b"export {};\nawait using x = { asyncDispose() {} };\nawait using y = 2;\n";
    let (owner, program, _) = fixture_files(
        b"/main.ts",
        &[(b"/main.ts", module), (b"/globals.ts", GLOBALS)],
        options(),
    );
    let source = program.file(b"/main.ts").unwrap().source();
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    let codes: Vec<i32> = diagnostics.iter().map(|d| d.code).collect();
    assert_eq!(
        codes,
        vec![
            ts_diagnostics::The_initializer_of_an_await_using_declaration_must_be_either_an_object_with_a_Symbol_asyncDispose_or_Symbol_dispose_method_or_be_null_or_undefined.code
        ]
    );

    let top_level = b"await using x = null;\n";
    let (owner, source) = checker(top_level, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::X_await_using_statements_are_only_allowed_at_the_top_level_of_a_file_when_that_file_is_a_module_but_this_file_has_no_imports_or_exports_Consider_adding_an_empty_export_to_make_this_file_a_module.code,
            vec![]
        )]
    );
}

#[test]
fn import_helpers_report_a_missing_tslib_once_per_file() {
    let text = b"export const { a, ...rest } = { a: 1, b: 2 };\n";
    for (target, expected) in [
        (ScriptTarget::ES2018, 0),
        (ScriptTarget::ES2017, 1),
        (ScriptTarget::ES2015, 1),
    ] {
        let (owner, source) = checker(
            text,
            CompilerOptions {
                target,
                import_helpers: Tristate::TRUE,
                ..options()
            },
        );
        let diagnostics = owner
            .operation()
            .unwrap()
            .semantic_diagnostics(source)
            .unwrap();
        let codes: Vec<i32> = diagnostics.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            vec![
                ts_diagnostics::This_syntax_requires_an_imported_helper_but_module_0_cannot_be_found.code;
                expected
            ],
            "target {target:?}"
        );
        if expected == 1 {
            let start = text.windows(7).position(|w| w == b"...rest").unwrap() as i64 + 3;
            assert_eq!(
                (diagnostics[0].loc.pos(), diagnostics[0].loc.end()),
                (start, start + 4),
                "the rest binding element is the error node, spanning its name"
            );
        }
    }
}

#[test]
fn import_attribute_values_are_contextually_typed_and_inline_attributes_are_const_contexts() {
    let globals: &[u8] = b"interface ImportAttributes { [name: string]: string }
interface Array<T> { length: number }
interface RegExp {}
interface Number { toString(): string }
";
    let module = b"import * as thing1 from \"./mod.mjs\" with { field: 0 };
import * as thing2 from \"./mod.mjs\" with { field: `a` };
import * as thing3 from \"./mod.mjs\" with { field: /a/g };
import * as thing4 from \"./mod.mjs\" with { field: [\"a\"] };
import * as thing5 from \"./mod.mjs\" with { field: { a: 0 } };
import * as thing6 from \"./mod.mjs\" with { type: \"json\", field: 0..toString() };
";
    let nodenext = CompilerOptions {
        target: ScriptTarget::ES2022,
        module: ModuleKind::NODE_NEXT,
        module_resolution: ts_core::ModuleResolutionKind::NODE_NEXT,
        ..options()
    };
    let (owner, program, _) = fixture_files(
        b"/mod.mts",
        &[(b"/mod.mts", module), (b"/globals.d.ts", globals)],
        nodenext.clone(),
    );
    let source = program.file(b"/mod.mts").unwrap().source();
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    let codes: Vec<i32> = diagnostics.iter().map(|d| d.code).collect();
    let not_assignable = ts_diagnostics::Type_0_is_not_assignable_to_type_1.code;
    let not_string =
        ts_diagnostics::Import_attribute_values_must_be_string_literal_expressions.code;
    // Pinned Go: importAttributes6(module=nodenext).errors.txt, ten errors in source order.
    assert_eq!(
        codes,
        vec![
            not_assignable,
            not_string,
            not_string,
            not_assignable,
            not_string,
            not_assignable,
            not_string,
            not_assignable,
            not_string,
            not_string
        ]
    );
    assert_eq!(
        codes_and_args(&diagnostics[..1])[0].1,
        vec!["{ field: 0; }".to_string(), "ImportAttributes".to_string()],
        "the attribute value keeps its literal type under the ImportAttributes contextual type"
    );

    let inline = b"export const loaded = import(\"./mod.mjs\", { with: { type: \"json\" } });\n";
    let (owner, program, _) = fixture_files(
        b"/main.mts",
        &[
            (b"/main.mts", inline),
            (b"/mod.mjs", b"export const x = 1;\n"),
            (b"/globals.ts", globals),
        ],
        nodenext,
    );
    let source = program.file(b"/main.mts").unwrap().source();
    let diagnostics = owner.operation().unwrap().semantic_diagnostics(source);
    assert!(diagnostics.is_ok(), "{diagnostics:?}");
}

#[test]
fn inferred_qualified_type_names_emit_as_entity_names_in_declarations() {
    // Inferred types are written by the node builder; upstream builds
    // `NS.I` as a qualified name and `typeof C` over an entity name.
    let text = b"export namespace NS { export interface I { x: number } }
declare const make: () => NS.I;
export const v = make();
export class C {}
export const cls = C;
export const mixin = (Base: new (...args: any[]) => any) => class extends Base { get(node: NS.I) {} };
";
    let (owner, program, _) = fixture(text, options());
    let file = program.file(b"/main.ts").unwrap();
    let mut op = owner.operation().unwrap();
    assert!(op.semantic_diagnostics(file.source()).unwrap().is_empty());
    let declarations = program.declaration_diagnostics_with_checker(&mut op, file);
    assert!(declarations.is_ok(), "{declarations:?}");
    assert!(declarations.unwrap().is_empty());
}

#[test]
fn abstract_properties_destructured_from_this_in_constructors_are_reported() {
    // Pinned Go: abstractPropertyInConstructor.errors.txt, class C1.
    let text = b"abstract class C1 {
    abstract x: string;
    abstract y: string;
    constructor() {
        let self = this;
        let { x, y: y1 } = this;
        ({ x, y: y1, \"y\": y1 } = this);
    }
}
";
    let (owner, source) = checker(text, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    let code =
        ts_diagnostics::Abstract_property_0_in_class_1_cannot_be_accessed_in_the_constructor.code;
    assert_eq!(
        codes_and_args(&diagnostics),
        ["x", "y", "x", "y", "y"]
            .iter()
            .map(|name| (code, vec![(*name).to_string(), "C1".to_string()]))
            .collect::<Vec<_>>()
    );

    // Destructuring a private member checks accessibility at the binding element.
    let private = b"class A { private p = 1; q = 2; }\nconst { p, q } = new A();\np; q;\n";
    let (owner, source) = checker(private, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::Property_0_is_private_and_only_accessible_within_class_1.code,
            vec!["p".to_string(), "A".to_string()]
        )]
    );
    let start = private.windows(8).position(|w| w == b"{ p, q }").unwrap() as i64 + 2;
    assert_eq!(
        (diagnostics[0].loc.pos(), diagnostics[0].loc.end()),
        (start, start + 1)
    );
}

#[test]
fn missing_properties_from_later_libs_suggest_the_lib() {
    let text =
        b"interface String { length: number }\ndeclare const s: string;\ns.padStart(2);\ns.nope;\n";
    let (owner, source) = checker(text, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![
            (
                ts_diagnostics::Property_0_does_not_exist_on_type_1_Do_you_need_to_change_your_target_library_Try_changing_the_lib_compiler_option_to_2_or_later.code,
                vec!["padStart".to_string(), "string".to_string(), "es2017".to_string()]
            ),
            (
                ts_diagnostics::Property_0_does_not_exist_on_type_1.code,
                vec!["nope".to_string(), "string".to_string()]
            ),
        ]
    );
}

#[test]
fn declaration_emit_names_types_from_other_modules_through_ranked_specifiers() {
    let (owner, program, _) = fixture_files(
        b"/main.ts",
        &[
            (
                b"/main.ts",
                b"import { make } from \"./lib\";\nexport const v = make();\n",
            ),
            (
                b"/lib.ts",
                b"export interface I { x: number }\nexport declare function make(): I;\n",
            ),
        ],
        options(),
    );
    let file = program.file(b"/main.ts").unwrap();
    let mut op = owner.operation().unwrap();
    assert!(op.semantic_diagnostics(file.source()).unwrap().is_empty());
    let declarations = program.declaration_diagnostics_with_checker(&mut op, file);
    assert!(declarations.is_ok(), "{declarations:?}");
    assert!(declarations.unwrap().is_empty());
}

#[test]
fn untyped_packages_report_the_types_install_chain() {
    let (owner, program, _) = fixture_files(
        b"/main.ts",
        &[
            (b"/main.ts", b"import * as foo from \"foo\";\nfoo;\n"),
            (b"/node_modules/foo/index.js", b"module.exports = {};\n"),
            (
                b"/node_modules/foo/package.json",
                b"{ \"name\": \"foo\", \"version\": \"1.0.0\", \"main\": \"index.js\" }\n",
            ),
        ],
        CompilerOptions {
            module: ModuleKind::COMMON_JS,
            module_resolution: ts_core::ModuleResolutionKind::NODE10,
            ..options()
        },
    );
    let source = program.file(b"/main.ts").unwrap().source();
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::Could_not_find_a_declaration_file_for_module_0_1_implicitly_has_an_any_type.code,
            vec!["foo".to_string(), "/node_modules/foo/index.js".to_string()]
        )]
    );
    assert_eq!(diagnostics[0].message_chain.len(), 1);
    let chain = &diagnostics[0].message_chain[0];
    assert_eq!(
        chain.code,
        ts_diagnostics::Try_npm_i_save_dev_types_Slash_1_if_it_exists_or_add_a_new_declaration_d_ts_file_containing_declare_module_0.code
    );
    assert_eq!(
        codes_and_args(std::slice::from_ref(chain))[0].1,
        vec!["foo".to_string(), "foo".to_string()]
    );
}

#[test]
fn global_augmentations_merging_into_aliases_resolve_the_alias() {
    // Pinned Go: checkMergedGlobalUMDSymbol.errors.txt, two TS2451 in global.d.ts.
    let (owner, program, _) = fixture_files(
        b"/test.ts",
        &[
            (b"/test.ts", b"const m = THREE;\nm;\n"),
            (b"/three.d.ts", b"export namespace THREE {\n  export class Vector2 {}\n}\n"),
            (
                b"/global.d.ts",
                b"import * as _three from './three';\n\nexport as namespace THREE;\n\ndeclare global {\n  export const THREE: typeof _three;\n}\n",
            ),
        ],
        CompilerOptions {
            target: ScriptTarget::ES2015,
            ..options()
        },
    );
    let mut op = owner.operation().unwrap();
    let test = program.file(b"/test.ts").unwrap().source();
    let test_diagnostics = op.semantic_diagnostics(test);
    assert!(test_diagnostics.is_ok(), "{test_diagnostics:?}");
    assert!(test_diagnostics.unwrap().is_empty());
    let global = program.file(b"/global.d.ts").unwrap().source();
    let diagnostics = op.semantic_diagnostics(global).unwrap();
    let redeclare = ts_diagnostics::Cannot_redeclare_block_scoped_variable_0.code;
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![
            (redeclare, vec!["THREE".to_string()]),
            (redeclare, vec!["THREE".to_string()]),
        ]
    );
}

#[test]
fn unique_symbol_index_errors_name_the_symbol_fully_qualified() {
    let text = b"declare const s: unique symbol;\ndeclare const o: { a: number };\no[s];\n";
    let (owner, source) = checker(text, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::Element_implicitly_has_an_any_type_because_expression_of_type_0_can_t_be_used_to_index_type_1.code,
            vec!["unique symbol".to_string(), "{ a: number; }".to_string()]
        )]
    );
    let chain = &diagnostics[0].message_chain;
    assert_eq!(chain.len(), 1);
    assert_eq!(
        codes_and_args(std::slice::from_ref(&chain[0])),
        vec![(
            ts_diagnostics::Property_0_does_not_exist_on_type_1.code,
            vec!["[s]".to_string(), "{ a: number; }".to_string()]
        )]
    );
}

#[test]
fn deprecated_contextual_properties_are_suggested_with_their_tag() {
    let text = b"interface Opts {\n    /** @deprecated use fresh */\n    old?: number;\n    fresh?: number;\n}\nexport const o: Opts = { old: 1 };\n";
    let (owner, source) = checker(text, options());
    let mut op = owner.operation().unwrap();
    assert!(op.semantic_diagnostics(source).unwrap().is_empty());
    let suggestions = op.recorded_suggestions(source).unwrap();
    assert_eq!(
        codes_and_args(&suggestions),
        vec![(
            ts_diagnostics::X_0_is_deprecated.code,
            vec!["old".to_string()]
        )]
    );
    assert_eq!(suggestions[0].related_information.len(), 1);
    assert_eq!(
        suggestions[0].related_information[0].code,
        ts_diagnostics::The_declaration_was_marked_as_deprecated_here.code
    );
}

#[test]
fn typeof_this_in_a_method_signature_checks_without_a_boundary() {
    // Pinned Go: typeofThisInMethodSignature has no errors.
    let text = b"export class A {\n\tx = 1\n\ta(x: typeof this.x): void {}\n}\n\nconst a = new A().a(1);\n";
    let (owner, source) = checker(
        text,
        CompilerOptions {
            target: ScriptTarget::ES2015,
            ..options()
        },
    );
    let diagnostics = owner.operation().unwrap().semantic_diagnostics(source);
    assert!(diagnostics.is_ok(), "{diagnostics:?}");
    assert_eq!(codes_and_args(&diagnostics.unwrap()), vec![]);
}

#[test]
fn rewritten_relative_imports_that_resolve_to_directories_are_reported() {
    // Pinned Go: rewriteRelativeImportExtensions/cjsErrors(module=node18).errors.txt.
    let (owner, program, _) = fixture_files(
        b"/index.ts",
        &[
            (
                b"/index.ts",
                b"import foo = require(\"./foo.ts\"); // Error\nimport type _foo = require(\"./foo.ts\"); // Ok\nfoo;\n",
            ),
            (b"/foo.ts/index.ts", b"export = {};\n"),
        ],
        CompilerOptions {
            target: ScriptTarget::ES2022,
            module: ModuleKind::NODE18,
            module_resolution: ts_core::ModuleResolutionKind::NODE16,
            rewrite_relative_import_extensions: Tristate::TRUE,
            verbatim_module_syntax: Tristate::TRUE,
            ..options()
        },
    );
    let source = program.file(b"/index.ts").unwrap().source();
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::This_relative_import_path_is_unsafe_to_rewrite_because_it_looks_like_a_file_name_but_actually_resolves_to_0.code,
            vec!["./foo.ts/index.ts".to_string()]
        )]
    );
}

#[test]
fn never_intersections_explain_the_conflicting_property() {
    let text = b"type A = { kind: \"a\" } & { kind: \"b\" };\ndeclare const a: A;\na.kind;\n";
    let (owner, source) = checker(text, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    // The head message displays the reduced type; the chain keeps the alias
    // through NoTypeReduction.
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::Property_0_does_not_exist_on_type_1.code,
            vec!["kind".to_string(), "never".to_string()]
        )]
    );
    let chain = &diagnostics[0].message_chain;
    assert_eq!(chain.len(), 1);
    assert_eq!(
        codes_and_args(std::slice::from_ref(&chain[0])),
        vec![(
            ts_diagnostics::The_intersection_0_was_reduced_to_never_because_property_1_has_conflicting_types_in_some_constituents.code,
            vec!["A".to_string(), "kind".to_string()]
        )]
    );
}

#[test]
fn nominal_classes_are_not_subtype_reduced_unless_derived() {
    let text = b"class A { x = 1 }\nclass B extends A {}\nclass C { x = 1 }\ndeclare const a: A;\ndeclare const b: B;\ndeclare const c: C;\nexport const arr = [a, b, c];\n";
    let (owner, program, _) = fixture_files(
        b"/main.ts",
        &[
            (b"/main.ts", text),
            (b"/globals.d.ts", b"interface Array<T> { length: number }\n"),
        ],
        options(),
    );
    let export = *declarations(&program).last().unwrap();
    let view = program.file(b"/main.ts").unwrap().bound().view().ast();
    let list = view
        .node(export)
        .unwrap()
        .data_source()
        .as_variable_statement()
        .unwrap()
        .declaration_list()
        .unwrap();
    let declarations_list = view
        .node(list)
        .unwrap()
        .data_source()
        .as_variable_declaration_list()
        .unwrap()
        .declarations()
        .unwrap();
    let declaration = view
        .node_slice(view.list(declarations_list).unwrap().nodes())
        .unwrap()
        .get(0)
        .unwrap()
        .unwrap();
    let name = declaration_name(&program, declaration);
    let mut op = owner.operation().unwrap();
    let symbol = op.get_symbol_at_location(name).unwrap().unwrap();
    let ty = op.get_type_of_symbol(symbol).unwrap();
    // B derives from A and is removed; C is structurally identical to A but nominal.
    assert_eq!(op.type_to_string(ty, 0).unwrap().as_bytes(), b"(A | C)[]");
}

#[test]
fn too_many_arguments_through_a_spread_report_the_extra_argument_span() {
    // Pinned Go: functionParameterArityMismatch.errors.txt, the last two calls.
    let text = b"interface Array<T> { length: number }\ndeclare function f2();\ndeclare function f2(a: number, b: number, c: number, d: number, e: number, f: number);\nf2(1, 2, 3, 4, 5, 6, 7);\nf2(1, 2, 3, 4, 5, ...[6, 7]);\n";
    let (owner, source) = checker(
        text,
        CompilerOptions {
            target: ScriptTarget::ES2015,
            strict: Tristate::FALSE,
            ..options()
        },
    );
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    let expected = ts_diagnostics::Expected_0_arguments_but_got_1.code;
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![
            (expected, vec!["0-6".to_string(), "7".to_string()]),
            (expected, vec!["0-6".to_string(), "7".to_string()]),
        ]
    );
    // The second span starts at the spread element that carries the extra argument.
    let spread = text.windows(8).position(|w| w == b"...[6, 7").unwrap() as i64;
    assert_eq!(diagnostics[1].loc.pos(), spread);
}

#[test]
fn readonly_type_operators_are_limited_to_array_and_tuple_literals() {
    let (owner, source) = checker(b"type T = readonly string;\n", options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::X_readonly_type_modifier_is_only_permitted_on_array_and_tuple_literal_types.code,
            vec!["symbol".to_string()]
        )]
    );
}

#[test]
fn conflicting_private_members_reduce_intersections_to_never_with_an_explanation() {
    let text =
        b"class A { private p = 1 }\nclass B { private p = 1 }\ndeclare const x: A & B;\nx.p;\n";
    let (owner, source) = checker(text, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::Property_0_does_not_exist_on_type_1.code,
            vec!["p".to_string(), "never".to_string()]
        )]
    );
    assert_eq!(
        codes_and_args(std::slice::from_ref(&*diagnostics[0].message_chain[0])),
        vec![(
            ts_diagnostics::The_intersection_0_was_reduced_to_never_because_property_1_exists_in_multiple_constituents_and_is_private_in_some.code,
            vec!["A & B".to_string(), "p".to_string()]
        )]
    );
}

#[test]
fn circular_import_aliases_report_the_circularity() {
    let (owner, source) = checker(b"import a = a;\na;\n", options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == ts_diagnostics::Circular_definition_of_import_alias_0.code),
        "{:?}",
        codes_and_args(&diagnostics)
    );
}

#[test]
fn misspelled_mapped_types_suggest_the_in_keyword() {
    let text = b"type Keys = \"a\" | \"b\";\ntype M = { [Keys]: number };\n";
    let (owner, source) = checker(text, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::X_0_only_refers_to_a_type_but_is_being_used_as_a_value_here_Did_you_mean_to_use_1_in_0.code,
            vec!["Keys".to_string(), "K".to_string()]
        )]
    );
}

#[test]
fn misspelled_builtin_names_suggest_the_primitive_alias() {
    // A case difference costs 0.1 in the spelling distance, so `strng` prefers the
    // primitive alias while `Strng` would pick the `String` interface.
    let (owner, program, _) = fixture_files(
        b"/main.ts",
        &[
            (b"/main.ts", b"let value: strng = \"\";\nvalue;\n"),
            (b"/globals.d.ts", b"interface String {}\n"),
        ],
        options(),
    );
    let source = program.file(b"/main.ts").unwrap().source();
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::Cannot_find_name_0_Did_you_mean_1.code,
            vec!["strng".to_string(), "string".to_string()]
        )]
    );
}

#[test]
fn exported_namespaces_in_commonjs_files_are_rejected_under_verbatim_module_syntax() {
    let (owner, source) = checker(
        b"export namespace N { export const x = 1; }\n",
        CompilerOptions {
            module: ModuleKind::COMMON_JS,
            verbatim_module_syntax: Tristate::TRUE,
            ..options()
        },
    );
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::A_top_level_export_modifier_cannot_be_used_on_value_declarations_in_a_CommonJS_module_when_verbatimModuleSyntax_is_enabled.code,
            vec![]
        )]
    );
    assert_eq!(diagnostics[0].loc.pos(), 0);
}

#[test]
fn constructor_visibility_mismatches_report_the_visibilities() {
    let text = b"class A { private constructor() {} }\nclass B { protected constructor() {} }\nlet x: typeof B = A;\nx;\n";
    let (owner, source) = checker(text, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code,
        ts_diagnostics::Type_0_is_not_assignable_to_type_1.code
    );
    assert_eq!(
        codes_and_args(std::slice::from_ref(&*diagnostics[0].message_chain[0])),
        vec![(
            ts_diagnostics::Cannot_assign_a_0_constructor_type_to_a_1_constructor_type.code,
            vec!["private".to_string(), "protected".to_string()]
        )]
    );
}

#[test]
fn imports_conflicting_with_global_values_need_type_only_imports_under_isolated_modules() {
    let (owner, program, _) = fixture_files(
        b"/main.ts",
        &[
            (b"/main.ts", b"import { Foo } from \"./a\";\nFoo;\n"),
            (b"/a.ts", b"export interface Foo { x: number }\n"),
            (b"/globals.d.ts", b"declare var Foo: number;\n"),
        ],
        CompilerOptions {
            isolated_modules: Tristate::TRUE,
            ..options()
        },
    );
    let source = program.file(b"/main.ts").unwrap().source();
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert!(
        diagnostics.iter().any(|d| d.code
            == ts_diagnostics::Import_0_conflicts_with_global_value_used_in_this_file_so_must_be_declared_with_a_type_only_import_when_isolatedModules_is_enabled.code),
        "{:?}",
        codes_and_args(&diagnostics)
    );
}

#[test]
fn uncalled_function_checks_resolve_this_property_symbols() {
    let text = b"class C {\n    f = () => 1;\n    m() { return this.f ? 1 : 2; }\n}\n";
    let (owner, source) = checker(text, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
        vec![ts_diagnostics::This_condition_will_always_return_true_since_this_function_is_always_defined_Did_you_mean_to_call_it_instead.code]
    );
}

#[test]
fn split_value_and_type_exports_combine_into_one_symbol() {
    // Pinned Go: mergedDeclarations7.errors.txt. `Passport` resolves to the
    // interface from the namespace merged with the `export =` value.
    let (owner, program, _) = fixture_files(
        b"/test.ts",
        &[
            (
                b"/passport.d.ts",
                b"declare module 'passport' {
    namespace passport {
        interface Passport {
            use(): this;
        }
        interface PassportStatic extends Passport {
            Passport: {new(): Passport};
        }
    }
    const passport: passport.PassportStatic;
    export = passport;
}
",
            ),
            (
                b"/test.ts",
                b"import * as passport from \"passport\";
import { Passport } from \"passport\";
let p: Passport = passport.use();
",
            ),
        ],
        CompilerOptions {
            module: ModuleKind::COMMON_JS,
            ..options()
        },
    );
    let source = program.file(b"/test.ts").unwrap().source();
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::Type_0_is_not_assignable_to_type_1.code,
            vec!["PassportStatic".to_string(), "Passport".to_string()]
        )]
    );
}

#[test]
fn commonjs_class_expression_containers_resolve_for_declaration_emit() {
    // Pinned Go: jsDeclarationsExportAssignedClassExpressionAnonymousWithSub.
    // getContainersOfSymbol reaches the class expressions through their
    // `module.exports` assignments instead of failing the declaration phase.
    let (owner, program, _) = fixture_files(
        b"/index.js",
        &[(
            b"/index.js",
            b"module.exports = class {
    /** @param {number} p */
    constructor(p) {
        this.t = 12 + p;
    }
}
module.exports.Sub = class {
    constructor() {
        this.instance = new module.exports(10);
    }
}
",
        )],
        CompilerOptions {
            allow_js: Tristate::TRUE,
            check_js: Tristate::TRUE,
            declaration: Tristate::TRUE,
            module: ModuleKind::COMMON_JS,
            ..options()
        },
    );
    let file = program.file(b"/index.js").unwrap();
    let mut op = owner.operation().unwrap();
    let semantic = op.semantic_diagnostics(file.source()).unwrap();
    assert_eq!(
        semantic.iter().map(|d| d.code).collect::<Vec<_>>(),
        vec![
            ts_diagnostics::An_export_assignment_cannot_be_used_in_a_module_with_other_exported_elements.code,
            ts_diagnostics::Property_0_does_not_exist_on_type_1.code,
        ]
    );
    let declarations = program.declaration_diagnostics_with_checker(&mut op, file);
    assert!(declarations.is_ok(), "{declarations:?}");
}

#[test]
fn implements_errors_keep_their_head_message_over_missing_properties() {
    // Pinned Go: jsdocImplements_class.errors.txt (B3) and relater.go's
    // isConversionOrInterfaceImplementationMessage.
    let text = b"class A { method(): number { throw 1 } }
class B3 implements A {}
interface I { method(): number }
class B4 implements I {}
";
    let (owner, source) = checker(text, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
        vec![
            ts_diagnostics::Class_0_incorrectly_implements_class_1_Did_you_mean_to_extend_1_and_inherit_its_members_as_a_subclass.code,
            ts_diagnostics::Class_0_incorrectly_implements_interface_1.code,
        ]
    );
    assert_eq!(
        diagnostics[0].message_chain[0].code,
        ts_diagnostics::Property_0_is_missing_in_type_1_but_required_in_type_2.code
    );
}

#[test]
fn reentrant_effects_signature_resolution_terminates_like_upstream() {
    // Pinned Go: controlFlowFunctionLikeCircular1.errors.txt, file 8. The
    // assertion call's effects signature re-enters itself through the type
    // predicate's `typeof arg`; upstream recomputes and the explicit-type
    // resolving set ends the recursion.
    let text = b"function test(arg: string | number, whatever: any) {
  if (typeof arg === \"string\") {
    b();
    type First = typeof arg;
    type Test = (arg: unknown) => arg is First;
    const b: Test = whatever;
    return b;
  }
  return undefined;
}
";
    let (owner, source) = checker(
        text,
        CompilerOptions {
            strict: Tristate::TRUE,
            ..options()
        },
    );
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
        vec![
            ts_diagnostics::Block_scoped_variable_0_used_before_its_declaration.code,
            ts_diagnostics::Variable_0_is_used_before_being_assigned.code,
            ts_diagnostics::Expected_0_arguments_but_got_1.code,
            ts_diagnostics::Type_alias_0_circularly_references_itself.code,
        ]
    );
}

#[test]
fn iife_rest_parameters_past_the_argument_list_check_without_panicking() {
    // `getSpreadArgumentType`'s `for i := index; i < argCount; i++` simply does
    // not run when a rest parameter sits past the argument list, so the window is
    // empty. Both shapes are lines 9 and 10 of the pinned fixture
    // emitDefaultParametersFunctionExpression.ts, which Go checks without error.
    for text in [
        b"var y = (function (num = 10, boo = false, ...rest) { })();".as_slice(),
        b"var z = (function (num: number, boo = false, ...rest) { })(10);",
    ] {
        let (checker, source) = checker(
            text,
            CompilerOptions {
                strict: Tristate::FALSE,
                ..options()
            },
        );
        let first = checker.operation().unwrap().semantic_diagnostics(source);
        assert!(first.is_ok(), "source {text:?}: {first:?}");
        assert!(
            first.as_ref().unwrap().is_empty(),
            "source {text:?}: {first:?}"
        );
        for _ in 0..2 {
            assert_eq!(
                checker.operation().unwrap().semantic_diagnostics(source),
                first,
                "source {text:?}"
            );
        }
    }
}

#[test]
fn missing_dom_intersections_use_the_native_lib_diagnostic() {
    // The six accesses and expected diagnostics are from the pinned
    // compiler/missingDomElements.ts fixture, including its negative controls.
    let text =
        include_bytes!("../../../upstream/tsc/testdata/tests/cases/compiler/missingDomElements.ts");
    let (checker, source) = checker(text, options());
    for _ in 0..2 {
        let diagnostics = checker
            .operation()
            .unwrap()
            .semantic_diagnostics(source)
            .unwrap();
        assert_eq!(
            diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
            [2812, 2812, 2812, 2812, 2339, 2339]
        );
        assert_eq!(
            diagnostics[3]
                .message_args
                .iter()
                .map(JsString::as_bytes)
                .collect::<Vec<_>>(),
            [b"textContent".as_slice(), b"EventTarget & HTMLInputElement"]
        );
    }
}

#[test]
fn jsdoc_rest_parameter_displays_reuse_the_variadic_operand() {
    // Pinned Go nodecopy.go reuses JSDocVariadicType.Type. Sources and displays
    // are the `f` rows of jsdocRestParameter, jsdocRestParameter_es6 and
    // jsdocParseStarEquals .types, requested with the type baseline walker flags.
    use ts_checker::type_format_flags as ff;
    use ts_printer::{EmitTextWriter, Printer, PrinterOptions, TextWriter};
    let cases: [(&[u8], &[u8]); 3] = [
        (
            b"/** @param {...number} a */\nfunction f(a) {\n    a;\n}\n",
            b"(a: number[]) => void",
        ),
        (
            b"/** @param {...number} a */\nfunction f(...a) {\n    a;\n}\n",
            b"(...a: number[]) => void",
        ),
        (
            b"/** @param {...*=} args\n    @return {*=} */\nfunction f(...args) {\n    return null\n}\n",
            b"(...args?: any[] | undefined) => any | undefined",
        ),
    ];
    let flags = ((ff::NO_TRUNCATION
        | ff::ALLOW_UNIQUE_ES_SYMBOL_TYPE
        | ff::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS)
        & ff::NODE_BUILDER_FLAGS_MASK)
        | ts_nodebuilder::flags::IGNORE_ERRORS;
    for (text, expected) in cases {
        let (owner, program, _) = fixture_files(
            b"/a.js",
            &[(b"/a.js", text)],
            CompilerOptions {
                target: ScriptTarget::ES2015,
                allow_js: Tristate::TRUE,
                check_js: Tristate::TRUE,
                ..options()
            },
        );
        let file = program.file(b"/a.js").unwrap();
        let view = file.bound().view().ast();
        let function = view
            .node_slice(view.node(file.source()).unwrap().statements(view).unwrap())
            .unwrap()
            .iter()
            .flatten()
            .next()
            .unwrap();
        let name = view.node(function).unwrap().name().unwrap();
        let mut op = owner.operation().unwrap();
        let typ = op.get_type_at_location(name).unwrap();
        let mut builder = op.node_builder();
        let generated = builder
            .type_to_type_node(
                typ,
                Some(function),
                flags,
                ts_nodebuilder::internal_flags::ALLOW_UNRESOLVED_NAMES,
            )
            .unwrap()
            .unwrap();
        let mut writer = TextWriter::new(b"", 0);
        Printer::new(
            PrinterOptions {
                remove_comments: true,
                ..Default::default()
            },
            builder.emit_context(),
        )
        .write(builder.view(), generated, Some(file.source()), &mut writer)
        .unwrap();
        assert_eq!(
            String::from_utf8_lossy(writer.text()),
            String::from_utf8_lossy(expected)
        );
    }
}

#[test]
fn declaration_transform_reads_the_jsdoc_variadic_operand() {
    // Pinned Go: parseJSDocType wraps a leading `...` in JSDocVariadicType, the
    // typedef reparse keeps that node as the alias type, and
    // transformJSDocVariadicType emits an array of JSDocVariadicType.Type.
    use ts_ast::{AstBuilder, SyntaxKind as K};
    use ts_printer::{EmitContext, EmitTextWriter, Printer, PrinterOptions, TextWriter};
    use ts_transformers::declarations::{transform_declarations, DeclarationOptions};
    let (owner, program, _) = fixture_files(
        b"/a.js",
        &[(
            b"/a.js",
            b"/** @typedef {...number} Nums */\nvar value = 1;\n",
        )],
        CompilerOptions {
            allow_js: Tristate::TRUE,
            check_js: Tristate::TRUE,
            declaration: Tristate::TRUE,
            ..options()
        },
    );
    let file = program.file(b"/a.js").unwrap();
    let mut op = owner.operation().unwrap();
    let counters = Counters::new();
    let mut emit = EmitContext::new();
    let mut output = AstBuilder::with_hooks(
        ts_jsstring::SourceText::from_loaded_bytes(&b""[..]),
        &counters,
        emit.factory_hooks(),
    );
    let transformed = transform_declarations(
        &mut op,
        &mut output,
        &mut emit,
        file.source(),
        DeclarationOptions::default(),
    )
    .unwrap();
    let view = output.view();
    let statements: Vec<_> = view
        .node_slice(
            view.node(transformed.root)
                .unwrap()
                .statements(view)
                .unwrap(),
        )
        .unwrap()
        .iter()
        .flatten()
        .collect();
    let alias = statements
        .iter()
        .copied()
        .find(|&statement| {
            matches!(
                view.node(statement).unwrap().kind().known(),
                Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
            )
        })
        .expect("transformed typedef alias");
    let array = view.node(alias).unwrap().type_node().unwrap();
    assert_eq!(view.node(array).unwrap().kind(), K::ArrayType);
    let element = view
        .node(array)
        .unwrap()
        .data_source()
        .as_array_type_node()
        .unwrap()
        .element_type();
    assert_eq!(
        element.map(|element| view.node(element).unwrap().kind().known()),
        Some(Some(K::NumberKeyword))
    );
    let mut writer = TextWriter::new(b"", 0);
    Printer::new(
        PrinterOptions {
            remove_comments: true,
            ..Default::default()
        },
        &emit,
    )
    .write(view, array, Some(file.source()), &mut writer)
    .unwrap();
    assert_eq!(String::from_utf8_lossy(writer.text()), "number[]");
    output.complete(transformed.root).unwrap();
}

#[test]
fn es_module_marker_grammar_error_is_skipped_only_by_program_no_emit_filtering() {
    // Pinned Go: checkGrammarForEsModuleMarkerInBindingName reports through
    // grammarErrorOnNodeSkippedOnNoEmit without reading noEmit, and
    // Program.getSemanticDiagnosticsWithChecker drops it under noEmit. The source
    // is compiler/es5-commonjs8.ts, whose noEmit es2015 baseline has no errors.
    let text = b"export default \"test\";\nexport var __esModule = 1;\n";
    for no_emit in [Tristate::UNKNOWN, Tristate::TRUE] {
        let (owner, program, _) = fixture(
            text,
            CompilerOptions {
                target: ScriptTarget::ES2015,
                module: ModuleKind::COMMON_JS,
                no_emit,
                ..options()
            },
        );
        let file = program.file(b"/main.ts").unwrap();
        let mut op = owner.operation().unwrap();
        let checked = op.semantic_diagnostics(file.source()).unwrap();
        assert_eq!(
            checked
                .iter()
                .map(|d| (d.code, d.loc.pos(), d.loc.end(), d.skipped_on_no_emit))
                .collect::<Vec<_>>(),
            [(1216, 34, 44, true)],
            "checker diagnostics with noEmit {no_emit:?}"
        );
        let selected = program
            .semantic_diagnostics_with_checker(&mut op, file)
            .unwrap();
        let expected: &[i32] = if no_emit.is_true() { &[] } else { &[1216] };
        assert_eq!(
            selected.iter().map(|d| d.code).collect::<Vec<_>>(),
            expected,
            "program diagnostics with noEmit {no_emit:?}"
        );
    }
}

#[test]
fn qualified_enum_member_declaration_phase_completes_and_displays_like_native() {
    // Pinned compiler/declarationEmitQualifiedName.ts. b.ts reaches E only through
    // an import type, so appendReferenceToType extends that import's qualifier.
    // Native reports no declaration diagnostics; displays are the `.types` rows.
    use ts_printer::{EmitTextWriter, Printer, PrinterOptions, TextWriter};
    let files: [(&[u8], &[u8]); 3] = [
        (b"/e.ts", b"export enum E {\n    A = 'a',\n    B = 'b',\n}\n"),
        (
            b"/a.ts",
            b"import { E } from './e.js'\nexport const A = {\n    item: {\n        a: E.A,\n    },\n} as const\n",
        ),
        (
            b"/b.ts",
            b"import { A } from './a.js'\nexport const B = { ...A } as const\n",
        ),
    ];
    let (owner, program, _) = fixture_files(
        b"/e.ts",
        &files,
        CompilerOptions {
            declaration: Tristate::TRUE,
            ..options()
        },
    );
    let mut op = owner.operation().unwrap();
    for (path, _) in files {
        let file = program.file(path).unwrap();
        let diagnostics = program.declaration_diagnostics_with_checker(&mut op, file);
        assert!(
            diagnostics.as_ref().is_ok_and(Vec::is_empty),
            "{diagnostics:?}"
        );
    }
    let flags = 79_659_013;
    for (path, statement, expected) in [
        (
            b"/a.ts".as_slice(),
            1,
            "{ readonly item: { readonly a: E.A; }; }",
        ),
        (
            b"/b.ts",
            1,
            "{ readonly item: { readonly a: import(\"./e.js\").E.A; }; }",
        ),
    ] {
        let file = program.file(path).unwrap();
        let view = file.bound().view().ast();
        let statement = view
            .node_slice(view.node(file.source()).unwrap().statements(view).unwrap())
            .unwrap()
            .at(statement)
            .unwrap();
        let list = view
            .node(statement)
            .unwrap()
            .data_source()
            .as_variable_statement()
            .unwrap()
            .declaration_list()
            .unwrap();
        let declarations = view
            .node(list)
            .unwrap()
            .data_source()
            .as_variable_declaration_list()
            .unwrap()
            .declarations()
            .unwrap();
        let declaration = view
            .node_slice(view.list(declarations).unwrap().nodes())
            .unwrap()
            .at(0)
            .unwrap();
        let name = view.node(declaration).unwrap().name().unwrap();
        let typ = op.get_type_at_location(name).unwrap();
        let mut builder = op.node_builder();
        let generated = builder
            .type_to_type_node(
                typ,
                Some(declaration),
                flags,
                ts_nodebuilder::internal_flags::ALLOW_UNRESOLVED_NAMES,
            )
            .unwrap()
            .unwrap();
        let mut writer = TextWriter::new(b"", 0);
        Printer::new(
            PrinterOptions {
                remove_comments: true,
                ..Default::default()
            },
            builder.emit_context(),
        )
        .write(builder.view(), generated, Some(file.source()), &mut writer)
        .unwrap();
        assert_eq!(String::from_utf8_lossy(writer.text()), expected);
    }
}

#[test]
fn missing_identifier_value_references_report_no_cannot_find_name_like_native() {
    // Focused native witnesses (data/s08/p6/missing-identifier). Parser recovery
    // matches: syntactic diagnostics are equal. Pinned Go getResolvedSymbol skips
    // resolution for a missing node, so no TS2304 '(Missing)' is reported.
    let request: serde_json::Value = serde_json::from_str(include_str!(
        "../../../data/s08/p6/missing-identifier/requests.json"
    ))
    .unwrap();
    let native: serde_json::Value = serde_json::from_str(include_str!(
        "../../../data/s08/p6/missing-identifier/observations.json"
    ))
    .unwrap();
    let observe = |values: &[ts_ast::Diagnostic]| -> Vec<serde_json::Value> {
        values
            .iter()
            .map(|d| {
                let args: Vec<_> = d
                    .message_args
                    .iter()
                    .map(|arg| String::from_utf8(arg.as_bytes().to_vec()).unwrap())
                    .collect();
                serde_json::json!([d.code, d.loc.pos(), d.loc.end(), d.category, args])
            })
            .collect()
    };
    let expected = |values: &serde_json::Value| -> Vec<serde_json::Value> {
        values
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                let args = d["args"].as_array().cloned().unwrap_or_default();
                serde_json::json!([d["code"], d["pos"], d["end"], d["category"], args])
            })
            .collect()
    };
    let programs = request["programs"].as_array().unwrap();
    let observed = native["programs"].as_array().unwrap();
    assert_eq!(programs.len(), observed.len());
    for (spec, native) in programs.iter().zip(observed) {
        assert_eq!(spec["id"], native["id"]);
        let text = spec["files"]["/main.ts"].as_str().unwrap();
        let (owner, program, _) = fixture(text.as_bytes(), options());
        let file = program.file(b"/main.ts").unwrap();
        assert_eq!(
            observe(&program.syntactic_diagnostics(Some(file)).unwrap()),
            expected(&native["diagnostics"]["syntactic"]),
            "{} syntactic",
            spec["id"]
        );
        assert_eq!(
            observe(
                &owner
                    .operation()
                    .unwrap()
                    .semantic_diagnostics(file.source())
                    .unwrap()
            ),
            expected(&native["diagnostics"]["semantic"]),
            "{} semantic",
            spec["id"]
        );
    }
}

#[test]
fn write_only_references_leave_locals_unused_like_native() {
    // Pinned compiler/noUnusedLocals_writeOnly.ts: getResolvedSymbol passes
    // !IsWriteOnlyAccess as isUse, so only `x` and `z` stay unreferenced, as in
    // its errors.txt (1,12) and (16,9). `f2` needs the library and is omitted.
    let fixture = include_str!(
        "../../../upstream/tsc/testdata/tests/cases/compiler/noUnusedLocals_writeOnly.ts"
    );
    let text =
        &fixture[fixture.find("function f(").unwrap()..fixture.find("function f2(").unwrap()];
    let (checker, source) = checker(
        text.as_bytes(),
        CompilerOptions {
            target: ScriptTarget::ES2015,
            no_unused_locals: Tristate::TRUE,
            no_unused_parameters: Tristate::TRUE,
            ..options()
        },
    );
    let diagnostics = checker
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        diagnostics
            .iter()
            .filter(|d| matches!(d.code, 6133 | 6138 | 6196 | 6198 | 6199))
            .map(|d| {
                let args: Vec<_> = d
                    .message_args
                    .iter()
                    .map(|a| a.as_bytes().to_vec())
                    .collect();
                (d.code, d.loc.pos(), d.loc.end(), args)
            })
            .collect::<Vec<_>>(),
        [
            (6133, 11, 12, vec![b"x".to_vec()]),
            (6133, 444, 445, vec![b"z".to_vec()])
        ]
    );
}

#[test]
fn property_write_access_classification_matches_native_program_diagnostics() {
    // Focused native witnesses (data/s08/p6/write-access): pinned Go passes
    // IsWriteAccess and IsWriteOnlyAccess, not assignment-target kinds, at the
    // property reference, accessibility and write-type call sites.
    let request: serde_json::Value = serde_json::from_str(include_str!(
        "../../../data/s08/p6/write-access/requests.json"
    ))
    .unwrap();
    let native: serde_json::Value = serde_json::from_str(include_str!(
        "../../../data/s08/p6/write-access/observations.json"
    ))
    .unwrap();
    let observe = |values: &[ts_ast::Diagnostic]| -> Vec<serde_json::Value> {
        values
            .iter()
            .map(|d| {
                let args: Vec<_> = d
                    .message_args
                    .iter()
                    .map(|arg| String::from_utf8(arg.as_bytes().to_vec()).unwrap())
                    .collect();
                serde_json::json!([d.code, d.loc.pos(), d.loc.end(), d.category, args])
            })
            .collect()
    };
    let expected = |values: &serde_json::Value| -> Vec<serde_json::Value> {
        values
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                let args = d["args"].as_array().cloned().unwrap_or_default();
                serde_json::json!([d["code"], d["pos"], d["end"], d["category"], args])
            })
            .collect()
    };
    let programs = request["programs"].as_array().unwrap();
    let observed = native["programs"].as_array().unwrap();
    assert_eq!(programs.len(), observed.len());
    for (spec, native) in programs.iter().zip(observed) {
        assert_eq!(spec["id"], native["id"]);
        let text = spec["files"]["/main.ts"].as_str().unwrap();
        let no_unused_locals = if spec["no_unused_locals"] == true {
            Tristate::TRUE
        } else {
            Tristate::UNKNOWN
        };
        let (owner, program, _) = fixture(
            text.as_bytes(),
            CompilerOptions {
                no_unused_locals,
                ..options()
            },
        );
        let file = program.file(b"/main.ts").unwrap();
        assert_eq!(
            observe(&program.syntactic_diagnostics(Some(file)).unwrap()),
            expected(&native["syntactic"]),
            "{} syntactic",
            spec["id"]
        );
        let mut op = owner.operation().unwrap();
        assert_eq!(
            observe(
                &program
                    .semantic_diagnostics_with_checker(&mut op, file)
                    .unwrap()
            ),
            expected(&native["semantic"]),
            "{} semantic",
            spec["id"]
        );
    }
}

#[test]
fn destructuring_assignment_accessibility_errors_use_the_property_name_like_native() {
    // Pinned compiler/destructuringAssignment_private.ts and its errors.txt:
    // checkPropertyAccessibilityEx reports object-literal destructuring errors on
    // the property name, including computed names, not on the whole property.
    let fixture = include_str!(
        "../../../upstream/tsc/testdata/tests/cases/compiler/destructuringAssignment_private.ts"
    );
    let text = &fixture[fixture.find("class C {").unwrap()..];
    let (checker, source) = checker(
        text.as_bytes(),
        CompilerOptions {
            target: ScriptTarget::ES2015,
            strict: Tristate::UNKNOWN,
            ..options()
        },
    );
    let diagnostics = checker
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    let observed: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.code == 2341)
        .map(|d| {
            let args: Vec<_> = d
                .message_args
                .iter()
                .map(|a| a.as_bytes().to_vec())
                .collect();
            (d.loc.pos(), d.loc.end(), args)
        })
        .collect();
    let args = |name: &[u8]| vec![name.to_vec(), b"C".to_vec()];
    assert_eq!(
        observed,
        [
            (83, 84, args(b"x")),
            (114, 115, args(b"o")),
            (170, 177, args(b"x")),
            (230, 237, args(b"o")),
        ]
    );
}

#[test]
fn private_member_self_type_access_matches_native_program_diagnostics() {
    // Focused native witness (data/s08/p6/element-self-access): element access
    // passes the apparent object type's symbol to isSelfTypeAccess, property
    // access passes the receiver's resolved symbol.
    let request: serde_json::Value = serde_json::from_str(include_str!(
        "../../../data/s08/p6/element-self-access/requests.json"
    ))
    .unwrap();
    let native: serde_json::Value = serde_json::from_str(include_str!(
        "../../../data/s08/p6/element-self-access/observations.json"
    ))
    .unwrap();
    let programs = request["programs"].as_array().unwrap();
    let observations = native["programs"].as_array().unwrap();
    assert_eq!(programs.len(), observations.len());
    let mut mismatches = Vec::new();
    for (spec, native) in programs.iter().zip(observations) {
        assert_eq!(spec["id"], native["id"]);
        let (owner, program, _) = fixture(
            spec["files"]["/main.ts"].as_str().unwrap().as_bytes(),
            CompilerOptions {
                no_unused_locals: Tristate::TRUE,
                ..options()
            },
        );
        let file = program.file(b"/main.ts").unwrap();
        let mut op = owner.operation().unwrap();
        let observed: Vec<_> = program
            .semantic_diagnostics_with_checker(&mut op, file)
            .unwrap()
            .iter()
            .map(|d| {
                let args: Vec<_> = d
                    .message_args
                    .iter()
                    .map(|arg| String::from_utf8(arg.as_bytes().to_vec()).unwrap())
                    .collect();
                serde_json::json!([d.code, d.loc.pos(), d.loc.end(), d.category, args])
            })
            .collect();
        let expected: Vec<_> = native["semantic"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                let args = d["args"].as_array().cloned().unwrap_or_default();
                serde_json::json!([d["code"], d["pos"], d["end"], d["category"], args])
            })
            .collect();
        if observed != expected {
            mismatches.push(serde_json::json!({
                "id": spec["id"], "rust": observed, "native": expected
            }));
        }
        assert!(native["syntactic"].as_array().unwrap().is_empty());
    }
    assert!(
        mismatches.is_empty(),
        "{:#}",
        serde_json::Value::Array(mismatches)
    );
}

#[test]
fn static_index_constraints_exclude_only_synthetic_prototypes() {
    assert_native_semantic_fixture(
        include_str!("../../../data/s08/p6/static-index-prototype/requests.json"),
        include_str!("../../../data/s08/p6/static-index-prototype/observations.json"),
    );
}

#[test]
fn interface_inheritance_matches_native_diagnostic_chains() {
    assert_native_semantic_fixture(
        include_str!("../../../data/s08/p6/interface-inheritance/requests.json"),
        include_str!("../../../data/s08/p6/interface-inheritance/observations.json"),
    );
}

#[test]
fn catch_destructuring_matches_native_diagnostics() {
    assert_native_semantic_fixture(
        include_str!("../../../data/s08/p6/catch-destructuring/requests.json"),
        include_str!("../../../data/s08/p6/catch-destructuring/observations.json"),
    );
}

#[test]
fn alias_circularity_matches_native_diagnostics() {
    assert_native_semantic_fixture(
        include_str!("../../../data/s08/p6/alias-circularity/requests.json"),
        include_str!("../../../data/s08/p6/alias-circularity/observations.json"),
    );
}

#[test]
fn js_open_object_access_matches_native_diagnostics() {
    assert_native_semantic_fixture(
        include_str!("../../../data/s08/p6/js-object-expando/requests.json"),
        include_str!("../../../data/s08/p6/js-object-expando/observations.json"),
    );
}

#[test]
fn unresolved_jsdoc_property_access_drops_the_receiver_alias() {
    // The typeFromPropertyAssignment* native baselines preserve an unresolved
    // alias on the receiver, but display its property access as canonical any.
    let text = b"/** @type {Missing} */ let value;\nvalue; value.field; value?.field;\n/** @type {any} */ let plain; plain.field;\nlet known = { field: 1 }; known.field;\nclass C { #hidden = 1; read() { return value.#hidden; } } new C().read;";
    let (owner, program, _) = fixture_files(
        b"/a.js",
        &[(b"/a.js", text)],
        CompilerOptions {
            allow_js: Tristate::TRUE,
            check_js: Tristate::TRUE,
            ..options()
        },
    );
    let file = program.file(b"/a.js").unwrap();
    let view = file.bound().view().ast();
    let expressions: Vec<_> = view
        .node_slice(view.node(file.source()).unwrap().statements(view).unwrap())
        .unwrap()
        .iter()
        .flatten()
        .filter_map(|node| {
            let read = view.node(node).unwrap();
            (read.kind() == ts_ast::SyntaxKind::ExpressionStatement)
                .then(|| read.expression().unwrap())
        })
        .collect();
    for _ in 0..2 {
        let mut op = owner.operation().unwrap();
        op.semantic_diagnostics(file.source()).unwrap();
        let actual: Vec<_> = expressions
            .iter()
            .map(|&node| {
                let ty = op.get_type_at_location(node).unwrap();
                op.type_to_string(ty, 0).unwrap()
            })
            .collect();
        assert_eq!(
            actual.iter().map(JsString::as_bytes).collect::<Vec<_>>(),
            [
                b"Missing".as_slice(),
                b"any",
                b"any",
                b"any",
                b"number",
                b"() => any"
            ]
        );
    }
}

#[test]
fn js_property_followups_match_native_diagnostics() {
    assert_native_semantic_fixture(
        include_str!("../../../data/s08/p6/js-property-followups/requests.json"),
        include_str!("../../../data/s08/p6/js-property-followups/observations.json"),
    );
}

#[test]
fn jsdoc_aliases_and_optional_methods_keep_native_display() {
    // Native witnesses: topLevelBlockExpando, contextualTypedSpecialAssignment
    // and typeFromContextualThisType. Check repeated reads across operations.
    for (strict, expected_method) in [
        (Tristate::TRUE, b"(() => number) | undefined".as_slice()),
        (Tristate::FALSE, b"() => number".as_slice()),
    ] {
        let text = b"/** @typedef {{n: number}} Named */\n/** @type {Named} */ let value; value;\n/** @type {{m?(): number}} */ let receiver; receiver.m;";
        let (owner, program, _) = fixture_files(
            b"/a.js",
            &[(b"/a.js", text)],
            CompilerOptions {
                allow_js: Tristate::TRUE,
                check_js: Tristate::TRUE,
                strict,
                ..options()
            },
        );
        let file = program.file(b"/a.js").unwrap();
        let view = file.bound().view().ast();
        let expressions: Vec<_> = view
            .node_slice(view.node(file.source()).unwrap().statements(view).unwrap())
            .unwrap()
            .iter()
            .flatten()
            .filter_map(|node| {
                let read = view.node(node).unwrap();
                (read.kind() == ts_ast::SyntaxKind::ExpressionStatement)
                    .then(|| read.expression().unwrap())
            })
            .collect();
        for _ in 0..2 {
            let mut op = owner.operation().unwrap();
            op.semantic_diagnostics(file.source()).unwrap();
            for (&node, expected) in expressions
                .iter()
                .zip([b"Named".as_slice(), expected_method])
            {
                let ty = op.get_type_at_location(node).unwrap();
                assert_eq!(op.type_to_string(ty, 0).unwrap().as_bytes(), expected);
            }
            assert_eq!(expressions.len(), 2);
        }
    }
}

#[test]
fn jsdoc_return_typedef_expands_when_its_alias_is_inaccessible() {
    // Pinned typedefOnStatements.types prints the local alpha declaration as
    // `{ alpha: string; }`, although the trailing return declares alias Alpha.
    let (owner, program, _) = fixture_files(
        b"/main.js",
        &[(b"/main.js", b"function proof() {\n/** @type {Alpha} */ var alpha = { alpha: \"aleph\" };\n/** @typedef {{ alpha: string }} Alpha */ return;\n}")],
        CompilerOptions {
            allow_js: Tristate::TRUE,
            check_js: Tristate::TRUE,
            ..options()
        },
    );
    let file = program.file(b"/main.js").unwrap();
    let view = file.bound().view().ast();
    let function = view
        .node_slice(view.node(file.source()).unwrap().statements(view).unwrap())
        .unwrap()
        .at(0)
        .unwrap();
    let body = view.node(function).unwrap().body().unwrap();
    let statement = view
        .node_slice(view.node(body).unwrap().statements(view).unwrap())
        .unwrap()
        .at(0)
        .unwrap();
    let list = view
        .node(statement)
        .unwrap()
        .data_source()
        .as_variable_statement()
        .unwrap()
        .declaration_list()
        .unwrap();
    let declarations = view
        .node(list)
        .unwrap()
        .data_source()
        .as_variable_declaration_list()
        .unwrap()
        .declarations()
        .unwrap();
    let declaration = view
        .node_slice(view.list(declarations).unwrap().nodes())
        .unwrap()
        .at(0)
        .unwrap();
    let name = view.node(declaration).unwrap().name().unwrap();
    for _ in 0..2 {
        let mut op = owner.operation().unwrap();
        op.semantic_diagnostics(file.source()).unwrap();
        let typ = op.get_type_at_location(name).unwrap();
        assert_eq!(
            op.type_to_string_at(typ, Some(declaration), 0)
                .unwrap()
                .as_bytes(),
            b"{ alpha: string; }"
        );
        assert_eq!(
            op.type_to_string_at(
                typ,
                Some(declaration),
                ts_checker::type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE
            )
            .unwrap()
            .as_bytes(),
            b"Alpha"
        );
    }
}

#[test]
fn display_keeps_nontrailing_variadics_in_one_rest_parameter() {
    use ts_checker::type_format_flags as ff;
    let text = b"type Variadic = <A extends any[], B extends any[]>(...args: [...A, ...B]) => void;\ntype Fixed = (...args: [a: number, b: string]) => void;";
    let (owner, program, _) = fixture(text, options());
    let declarations = declarations(&program);
    let mut op = owner.operation().unwrap();
    for (declaration, expected) in declarations.into_iter().zip([
        b"<A extends any[], B extends any[]>(...args: [...A, ...B]) => void".as_slice(),
        b"(a: number, b: string) => void".as_slice(),
    ]) {
        let name = declaration_name(&program, declaration);
        let symbol = op.get_symbol_at_location(name).unwrap().unwrap();
        let ty = op.get_declared_type_of_symbol(symbol).unwrap();
        for _ in 0..2 {
            assert_eq!(
                op.type_to_string(ty, ff::IN_TYPE_ALIAS).unwrap().as_bytes(),
                expected
            );
        }
    }
}

fn assert_native_semantic_fixture(requests: &str, native: &str) {
    fn option(value: &serde_json::Value, default: Tristate) -> Tristate {
        match value.as_bool() {
            Some(true) => Tristate::TRUE,
            Some(false) => Tristate::FALSE,
            None => {
                assert!(value.is_null(), "fixture option must be a boolean");
                default
            }
        }
    }
    fn payload(program: &Program, d: &ts_ast::Diagnostic) -> serde_json::Value {
        let file = d.file.map(|id| {
            let file = program
                .files()
                .iter()
                .find(|file| file.source() == id)
                .unwrap();
            String::from_utf8(
                file.bound()
                    .view()
                    .source_file()
                    .unwrap()
                    .file_name()
                    .to_vec(),
            )
            .unwrap()
        });
        let args = if d.message_args.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::json!(d
                .message_args
                .iter()
                .map(|a| String::from_utf8(a.as_bytes().to_vec()).unwrap())
                .collect::<Vec<_>>())
        };
        serde_json::json!({
            "file": file, "pos": d.loc.pos(), "end": d.loc.end(),
            "code": d.code, "category": d.category, "args": args,
            "chain": d.message_chain.iter().map(|d| payload(program, d)).collect::<Vec<_>>(),
            "related": d.related_information.iter().map(|d| payload(program, d)).collect::<Vec<_>>(),
            "unnecessary": d.reports_unnecessary, "skipped_on_no_emit": d.skipped_on_no_emit,
        })
    }

    let requests: serde_json::Value = serde_json::from_str(requests).unwrap();
    let native: serde_json::Value = serde_json::from_str(native).unwrap();
    let requests = requests["programs"].as_array().unwrap();
    let observations = native["programs"].as_array().unwrap();
    assert_eq!(requests.len(), observations.len());
    let mut mismatches = Vec::new();
    for (spec, expected) in requests.iter().zip(observations) {
        assert_eq!(spec["id"], expected["id"]);
        let files: Vec<_> = spec["files"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(path, text)| (path.as_bytes(), text.as_str().unwrap().as_bytes()))
            .collect();
        let root = spec["roots"][0].as_str().unwrap().as_bytes();
        let (owner, program, _) = fixture_files(
            root,
            &files,
            CompilerOptions {
                strict: option(&spec["strict"], Tristate::TRUE),
                allow_js: option(&spec["allow_js"], Tristate::UNKNOWN),
                check_js: option(&spec["check_js"], Tristate::UNKNOWN),
                no_implicit_any: option(&spec["no_implicit_any"], Tristate::UNKNOWN),
                no_unused_locals: if spec["no_unused_locals"] == true {
                    Tristate::TRUE
                } else {
                    Tristate::UNKNOWN
                },
                ..options()
            },
        );
        let syntactic = program.syntactic_diagnostics(None).unwrap();
        assert_eq!(
            serde_json::json!(syntactic
                .iter()
                .map(|d| payload(&program, d))
                .collect::<Vec<_>>()),
            expected["syntactic"],
            "{} syntactic",
            spec["id"]
        );
        let mut op = owner.operation().unwrap();
        // The repeat exercises the once-per-merged-symbol check and cached file
        // diagnostics without discarding full chains, locations or related info.
        for attempt in 0..2 {
            let diagnostics = program
                .files()
                .iter()
                .try_fold(Vec::new(), |mut diagnostics, file| {
                    diagnostics.extend(program.semantic_diagnostics_with_checker(&mut op, file)?);
                    Ok::<_, ts_compiler::Error>(diagnostics)
                })
                .and_then(|diagnostics| program.sort_and_deduplicate_diagnostics(&diagnostics));
            match diagnostics {
                Ok(diagnostics) => {
                    let actual = serde_json::json!(diagnostics
                        .iter()
                        .map(|d| payload(&program, d))
                        .collect::<Vec<_>>());
                    if actual != expected["semantic"] {
                        mismatches.push(serde_json::json!({"id": spec["id"], "attempt": attempt, "rust": actual, "native": expected["semantic"]}));
                    }
                }
                Err(error) => mismatches
                    .push(serde_json::json!({"id": spec["id"], "error": format!("{error:?}")})),
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "{:#}",
        serde_json::Value::Array(mismatches)
    );
}

fn semantic_codes(text: &[u8], options: CompilerOptions) -> Vec<(i32, Vec<String>)> {
    let (owner, source) = checker(text, options);
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    codes_and_args(&diagnostics)
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn ambient_empty_and_expression_statements_are_reported_once_per_block() {
    // Pinned Go: semicolonsInModuleDeclarations.errors.txt and the `cjs;`
    // statements of nodeModulesDeclarationEmitWithPackageExports.
    let text = b"declare namespace N1 { export interface I { }; }
declare namespace N2 { export const a: number; a; a; }
";
    let codes = semantic_codes(text, options());
    assert_eq!(
        codes
            .iter()
            .filter(|(code, _)| *code
                == ts_diagnostics::Statements_are_not_allowed_in_ambient_contexts.code)
            .count(),
        2,
        "{codes:?}"
    );
}

#[test]
fn functions_with_missing_bodies_report_implicit_any_return_types() {
    // Pinned Go: reservedWords3.errors.txt reports TS7010 for each function
    // whose body is missing after the reserved-word parameter.
    let codes = semantic_codes(b"function f1(enum) {}\nfunction f2(class) {}\n", options());
    let code =
        ts_diagnostics::X_0_which_lacks_return_type_annotation_implicitly_has_an_1_return_type.code;
    assert!(
        codes.contains(&(code, strings(&["f1", "any"]))),
        "{codes:?}"
    );
    assert!(
        codes.contains(&(code, strings(&["f2", "any"]))),
        "{codes:?}"
    );
}

#[test]
fn computed_class_members_are_checked_against_index_signatures() {
    // Pinned Go: computedPropertyNames12_ES5.errors.txt, `[+s]: typeof s`.
    let text = b"declare const s: string;
class C {
    [k: string]: number;
    [+s]: string;
}
";
    let codes = semantic_codes(text, options());
    let code = ts_diagnostics::Property_0_of_type_1_is_not_assignable_to_2_index_type_3.code;
    assert_eq!(
        codes
            .iter()
            .filter(|(c, _)| *c == code)
            .cloned()
            .collect::<Vec<_>>(),
        vec![(code, strings(&["[+s]", "string", "string", "number"]))],
        "{codes:?}"
    );
}

#[test]
fn parameter_grammar_errors_are_suppressed_by_parse_errors() {
    // Pinned Go: fatarrowfunctionsOptionalArgs.errors.txt has no TS1015 because
    // the file has parse errors; fatarrowfunctionsOptionalArgsErrors4 has them.
    let clean = semantic_codes(b"((arg?: number = 0) => 47);\n", options());
    assert_eq!(
        clean.iter().map(|(code, _)| *code).collect::<Vec<_>>(),
        vec![ts_diagnostics::Parameter_cannot_have_question_mark_and_initializer.code]
    );
    let broken = semantic_codes(b"((arg?: number = 0) => 47);\nlet x = ;\n", options());
    assert!(
        !broken.iter().any(|(code, _)| *code
            == ts_diagnostics::Parameter_cannot_have_question_mark_and_initializer.code),
        "{broken:?}"
    );
}

#[test]
fn type_declarations_outside_blocks_are_grammar_errors() {
    // Pinned Go: typeAliasDeclarationEmit3.errors.txt and
    // typeInterfaceDeclarationsInBlockStatements1.
    let text = b"function f1(): void {
    if (true)
        type foo = [];
    while (false)
        interface bar { }
}
";
    let codes = semantic_codes(text, options());
    let code = ts_diagnostics::X_0_declarations_can_only_be_declared_inside_a_block.code;
    assert_eq!(
        codes
            .iter()
            .filter(|(c, _)| *c == code)
            .map(|(_, args)| args.clone())
            .collect::<Vec<_>>(),
        vec![strings(&["type"]), strings(&["interface"])],
        "{codes:?}"
    );
}

#[test]
fn private_method_signatures_outside_classes_and_abstract_bodies_are_reported() {
    // Pinned Go: privateNameAndPropertySignature.errors.txt and
    // classAbstractMethodWithImplementation.errors.txt.
    let text = b"interface B {
    #foo: string;
    #bar(): string;
}
abstract class A {
    abstract foo() {}
}
";
    let codes = semantic_codes(text, options());
    assert_eq!(
        codes
            .iter()
            .filter(|(c, _)| *c
                == ts_diagnostics::Private_identifiers_are_not_allowed_outside_class_bodies.code)
            .count(),
        2,
        "{codes:?}"
    );
    assert!(
        codes.contains(&(
            ts_diagnostics::Method_0_cannot_have_an_implementation_because_it_is_marked_abstract
                .code,
            strings(&["foo"])
        )),
        "{codes:?}"
    );
}

#[test]
fn same_named_types_display_fully_qualified_in_missing_property_errors() {
    // Pinned Go: qualify.ts(58,5) reports TS2741 with 'I' and 'T.I'.
    let text = b"namespace T {
    export interface I { p: number; }
}
interface I { k: number; }
declare var y: I;
var x: T.I = y;
";
    assert_eq!(
        semantic_codes(text, options()),
        vec![(
            ts_diagnostics::Property_0_is_missing_in_type_1_but_required_in_type_2.code,
            strings(&["p", "I", "T.I"])
        )]
    );
}

#[test]
fn abstract_constructor_assignability_explains_the_mismatch() {
    // Pinned Go: classAbstractConstructorAssignability.errors.txt chains TS2517.
    let text = b"abstract class A {}
class B extends A {}
var BB: typeof B = A;
";
    let (owner, source) = checker(text, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(diagnostics.len(), 1, "{:?}", codes_and_args(&diagnostics));
    assert_eq!(
        diagnostics[0].code,
        ts_diagnostics::Type_0_is_not_assignable_to_type_1.code
    );
    assert_eq!(
        diagnostics[0].message_chain[0].code,
        ts_diagnostics::Cannot_assign_an_abstract_constructor_type_to_a_non_abstract_constructor_type.code
    );
}

#[test]
fn discriminants_narrow_unions_that_include_undefined() {
    // Pinned Go: discriminantsAndNullOrUndefined.ts has no errors; the
    // discriminant property is found through the partial union property.
    let text = b"interface A { kind: 'A'; }
interface B { kind: 'B'; }
declare var c: A | B | undefined;
declare function useA(_: A): void;
declare function useB(_: B): void;
if (c !== undefined) {
    switch (c.kind) {
        case 'A': useA(c); break;
        case 'B': useB(c); break;
    }
}
";
    assert_eq!(semantic_codes(text, options()), vec![]);
}

#[test]
fn exact_optional_property_mismatches_use_their_own_messages() {
    // Pinned Go: exactOptionalPropertyTypesArgumentError.errors.txt (TS2379).
    let text = b"declare function f(o: { y?: string }): void;
f({ y: undefined });
";
    let codes = semantic_codes(
        text,
        CompilerOptions {
            exact_optional_property_types: Tristate::TRUE,
            ..options()
        },
    );
    assert_eq!(
        codes.iter().map(|(code, _)| *code).collect::<Vec<_>>(),
        vec![ts_diagnostics::Argument_of_type_0_is_not_assignable_to_parameter_of_type_1_with_exactOptionalPropertyTypes_Colon_true_Consider_adding_undefined_to_the_types_of_the_target_s_properties.code]
    );
}

#[test]
fn comparison_errors_name_same_named_types_by_module() {
    // Pinned Go: errorWithSameNameType.errors.txt.
    let (owner, program, _) = fixture_files(
        b"/main.ts",
        &[
            (b"/main.ts", b"import * as A from \"./a\";\nimport * as B from \"./b\";\ndeclare let a: A.F;\ndeclare let b: B.F;\nif (a === b) {}\n"),
            (b"/a.ts", b"export interface F { a: number }\n"),
            (b"/b.ts", b"export interface F { b: number }\n"),
        ],
        options(),
    );
    let source = program.file(b"/main.ts").unwrap().source();
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::This_comparison_appears_to_be_unintentional_because_the_types_0_and_1_have_no_overlap.code,
            strings(&["import(\"/a\").F", "import(\"/b\").F"])
        )]
    );
}

#[test]
fn named_tuple_and_index_signature_grammar_is_reported() {
    // Pinned Go: namedTupleMembersErrors.errors.txt and
    // indexSignatureWithTrailingComma.errors.txt.
    let text = b"type T1 = [...a?: string[]];
type T2 = [a: string?];
type T3 = [a: ...string[]];
type A = { [key: string,]: string; };
";
    let codes = semantic_codes(text, options());
    let expected = [
        ts_diagnostics::A_tuple_member_cannot_be_both_optional_and_rest.code,
        ts_diagnostics::A_labeled_tuple_element_is_declared_as_optional_with_a_question_mark_after_the_name_and_before_the_colon_rather_than_after_the_type.code,
        ts_diagnostics::A_labeled_tuple_element_is_declared_as_rest_with_a_before_the_name_rather_than_before_the_type.code,
        ts_diagnostics::An_index_signature_cannot_have_a_trailing_comma.code,
    ];
    for code in expected {
        assert!(
            codes.iter().any(|(c, _)| *c == code),
            "missing {code}: {codes:?}"
        );
    }
}

#[test]
fn circular_return_types_name_the_assigned_variable() {
    // Pinned Go: noTypeToStringRecursion.errors.txt reports TS7023 on `f`.
    let codes = semantic_codes(b"const f = () => 42 satisfies typeof f;\n", options());
    assert!(
        codes.contains(&(
            ts_diagnostics::X_0_implicitly_has_return_type_any_because_it_does_not_have_a_return_type_annotation_and_is_referenced_directly_or_indirectly_in_one_of_its_return_expressions.code,
            strings(&["f"])
        )),
        "{codes:?}"
    );
}

#[test]
fn backslash_relative_ambient_module_names_and_deferred_rest_tuples() {
    // Pinned Go: ambientExternalModuleWithRelativeModuleName.errors.txt and
    // arrayDestructuringInSwitch1.ts (no circularity error).
    let codes = semantic_codes(
        b"declare module \".\\\\relativeModule\" { var x: string; }\n",
        options(),
    );
    assert_eq!(
        codes.iter().map(|(code, _)| *code).collect::<Vec<_>>(),
        vec![ts_diagnostics::Ambient_module_declaration_cannot_specify_relative_module_name.code]
    );
    let text = b"export type Expression = BooleanLogicExpression | 'true' | 'false';
export type BooleanLogicExpression = ['and', ...Expression[]] | ['not', Expression];
";
    assert_eq!(semantic_codes(text, options()), vec![]);
}

#[test]
fn exported_import_aliases_resolve_their_first_identifier_as_a_value() {
    // Pinned Go: importDeclWithExportModifier.errors.txt (TS2708 + TS2694) and
    // declarationEmitUnknownImport.errors.txt (TS2304).
    let codes = semantic_codes(
        b"namespace x {\n    interface c {\n    }\n}\nexport import a = x.c;\n",
        options(),
    );
    assert!(
        codes.contains(&(
            ts_diagnostics::Cannot_use_namespace_0_as_a_value.code,
            strings(&["x"])
        )),
        "{codes:?}"
    );
    let codes = semantic_codes(
        b"import Foo = SomeNonExistingName\nexport {Foo}\n",
        options(),
    );
    assert!(
        codes.contains(&(
            ts_diagnostics::Cannot_find_name_0.code,
            strings(&["SomeNonExistingName"])
        )),
        "{codes:?}"
    );
}

#[test]
fn export_equals_members_imported_from_js_get_the_require_hint() {
    // Pinned Go: importNonExportedMember8 reports TS2597 in the JS importer.
    let (owner, program, _) = fixture_files(
        b"/b.js",
        &[
            (b"/a.ts", b"class Foo {}\nexport = Foo;\n"),
            (b"/b.js", b"import { Foo } from './a';\n"),
        ],
        CompilerOptions {
            allow_js: Tristate::TRUE,
            check_js: Tristate::TRUE,
            module: ModuleKind::COMMON_JS,
            ..options()
        },
    );
    let source = program.file(b"/b.js").unwrap().source();
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::X_0_can_only_be_imported_by_using_a_require_call_or_by_using_a_default_import.code,
            strings(&["Foo"])
        )]
    );
}

#[test]
fn jsdoc_extends_tags_that_disagree_with_the_extends_clause_are_reported() {
    // Pinned Go: jsdocExtendsClauseMismatch.errors.txt.
    let (owner, program, _) = fixture_files(
        b"/main.js",
        &[
            (b"/react.d.ts", b"declare namespace React {\n    class Component { component: string }\n    class PureComponent { pure: string }\n}\n"),
            (b"/main.js", b"/**\n * @extends {React.Component}\n */\nclass C extends React.PureComponent {\n}\n"),
        ],
        CompilerOptions {
            allow_js: Tristate::TRUE,
            check_js: Tristate::TRUE,
            ..options()
        },
    );
    let source = program.file(b"/main.js").unwrap().source();
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(
        codes_and_args(&diagnostics),
        vec![(
            ts_diagnostics::JSDoc_0_1_does_not_match_the_extends_2_clause.code,
            strings(&["extends", "Component", "PureComponent"])
        )]
    );
}

#[test]
fn static_private_names_are_not_inherited_by_derived_constructors() {
    // Pinned Go: privateNameStaticAccessorssDerivedClasses.errors.txt reports
    // TS2339 on `x.#prop` for `typeof Derived`.
    let text = b"class Base {
    static #prop: number = 1;
    static method(x: typeof Derived) {
        x.#prop;
    }
}
class Derived extends Base {}
";
    let codes = semantic_codes(text, options());
    assert_eq!(
        codes,
        vec![(
            ts_diagnostics::Property_0_does_not_exist_on_type_1.code,
            strings(&["#prop", "typeof Derived"])
        )]
    );
}

#[test]
fn reported_relation_failures_are_not_elaborated_twice() {
    // Pinned Go: fuzzy.errors.txt chains one TS2741 under the `oneI: this`
    // error. The `this` type parameter relates its constraint twice; the
    // second pass reuses the reported failure instead of repeating the chain.
    let text = b"namespace M {
    export interface I { works: () => R; alsoWorks: () => R; }
    export interface R { anything: number; oneI: I; }
    export class C implements I {
        works(): R {
            return { anything: 1, oneI: this };
        }
    }
}
";
    let (owner, source) = checker(text, options());
    let diagnostics = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    let this_error = diagnostics
        .iter()
        .find(|d| d.code == ts_diagnostics::Type_0_is_not_assignable_to_type_1.code)
        .expect("assignment error for `this`");
    assert_eq!(this_error.message_chain.len(), 1);
    let missing = &this_error.message_chain[0];
    assert_eq!(
        missing.code,
        ts_diagnostics::Property_0_is_missing_in_type_1_but_required_in_type_2.code
    );
    assert!(
        missing.message_chain.is_empty(),
        "{:?}",
        codes_and_args(&diagnostics)
    );
    assert_eq!(missing.related_information.len(), 1);
}

/// The displayed type of the top-level `const`/`let` named `name` in `/main.ts`.
fn variable_type_display(text: &[u8], options: CompilerOptions, name: &[u8]) -> String {
    let (owner, program, _) = fixture(text, options);
    let file = program.file(b"/main.ts").unwrap();
    let source = file.source();
    let mut op = owner.operation().unwrap();
    op.semantic_diagnostics(source).unwrap();
    let view = file.bound().view().ast();
    let mut found = None;
    for statement in declarations(&program) {
        let Some(list) = view
            .node(statement)
            .unwrap()
            .data_source()
            .as_variable_statement()
            .and_then(|data| data.declaration_list())
        else {
            continue;
        };
        let Some(items) = view
            .node(list)
            .unwrap()
            .data_source()
            .as_variable_declaration_list()
            .and_then(|data| data.declarations())
        else {
            continue;
        };
        let items = view.list(items).unwrap().nodes();
        for declaration in view.node_slice(items).unwrap().iter().flatten() {
            let declaration_name = view.node(declaration).unwrap().name().unwrap();
            if view.node_text(declaration_name).unwrap().as_bytes() == name {
                found = Some(declaration_name);
            }
        }
    }
    let name = found.expect("declared variable");
    let ty = op.get_type_at_location(name).unwrap();
    String::from_utf8(op.type_to_string(ty, 0).unwrap().as_bytes().to_vec()).unwrap()
}

#[test]
fn new_on_unions_with_abstract_construct_signatures_is_any() {
    // Pinned Go: abstractClassUnionInstantiation.types prints `new cls2() : any`.
    let text = b"abstract class AbstractA { a: string; }
abstract class AbstractB { b: string; }
declare const cls2: typeof AbstractA | typeof AbstractB;
const v = new cls2();
";
    assert_eq!(variable_type_display(text, options(), b"v"), "any");
}

#[test]
fn in_operator_narrows_unions_to_intersections_with_record() {
    // Pinned Go: controlFlowInOperator.types prints `(A | B) & Record<"d", unknown>`.
    let text = b"type Record<K extends keyof any, T> = { [P in K]: T; };
const a = 'a';
type A = { [a]: number; };
type B = { b: string; };
declare const c: A | B;
function f() {
    if ('d' in c) {
        return c;
    }
    throw 0;
}
const w = f();
";
    assert_eq!(
        variable_type_display(text, options(), b"w"),
        "(A | B) & Record<\"d\", unknown>"
    );
}

#[test]
fn negative_numeric_string_property_names_display_as_string_literals() {
    // Pinned Go: duplicateObjectLiteralProperty_computedName1.types prints `{ "-1": number; }`.
    let text = b"const t7 = { \"-1\": 1 };\n";
    assert_eq!(
        variable_type_display(text, options(), b"t7"),
        "{ \"-1\": number; }"
    );
}
