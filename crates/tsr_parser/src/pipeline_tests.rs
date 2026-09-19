use ts_ast::{
    ExternalModuleIndicatorOptions, JsString, NodeDataRead, ParsedFile, SourceFileParseOptions,
    SyntaxKind as K,
};
use ts_core::{ScriptKind, Tristate};
use ts_jsstring::SourceText;

fn parse(name: &[u8], text: &[u8], kind: ScriptKind) -> ParsedFile {
    crate::parse_source_file(
        SourceText::from_loaded_bytes(text),
        kind,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(name),
            ..SourceFileParseOptions::default()
        },
    )
}

// Counts/ranges below were independently observed through ParseSourceFile at
// the pinned Go revision 1f70213d4922b434345f639b441681e470c7cfc1.
#[test]
fn json_token_payload_and_recovery_keep_source_counts_and_diagnostic_order() {
    crate::on_parser_worker(|| {
        for (text, nodes, texts, identifiers, expected) in [
            (b"true".as_slice(), 4, 0, 0, vec![]),
            (b"true false", 6, 0, 0, vec![(1012, 5, 10)]),
            (
                b"{foo:'x',a:+1}",
                11,
                4,
                2,
                vec![(1327, 1, 4), (1327, 5, 8), (1327, 9, 10), (1328, 11, 13)],
            ),
        ] {
            let file = parse(b"/value.json", text, ScriptKind::JSON);
            let view = file.view();
            let root = file.root();
            let data = view.source_file(root).unwrap();
            assert_eq!(
                (data.node_count, data.text_count, data.identifier_count),
                (nodes, texts, identifiers)
            );
            let actual: Vec<_> = data
                .diagnostics
                .iter()
                .map(|diagnostic| {
                    assert_eq!(diagnostic.file, Some(root));
                    (diagnostic.code, diagnostic.loc.pos(), diagnostic.loc.end())
                })
                .collect();
            assert_eq!(actual, expected);
            assert!(data.external_module_indicator.is_none());
            assert!(data.has_lazy_jsdoc); // JSON follows the non-JS/JSX branch.
            if text == b"true" {
                let list = view
                    .node(root)
                    .unwrap()
                    .data_source()
                    .as_source_file()
                    .unwrap()
                    .statements()
                    .unwrap();
                let statement = view
                    .node_slice(view.list(list).unwrap().nodes())
                    .unwrap()
                    .at(0)
                    .unwrap();
                let value = view
                    .node(statement)
                    .unwrap()
                    .data_source()
                    .as_expression_statement()
                    .unwrap()
                    .expression()
                    .unwrap();
                assert_eq!(view.node(value).unwrap().kind(), K::TrueKeyword);
                assert!(matches!(
                    view.node(value).unwrap().data(),
                    NodeDataRead::Token(_)
                ));
            }
        }
    });
}

#[test]
fn pragmas_preserve_last_duplicate_argument_and_last_check_directive() {
    let text = b"/// <reference types='one' types='two' preserve='true' resolution-mode='import' />\n// @ts-check\n// @ts-nocheck\n";
    let file = parse(b"/pragma.ts", text, ScriptKind::TS);
    let data = file.view().source_file(file.root()).unwrap();
    assert_eq!((data.node_count, data.text_count), (2, 0));
    assert_eq!(data.pragmas.len(), 3);
    assert!(!data.check_js_directive.unwrap().enabled);
    assert_eq!(data.type_reference_directives.len(), 1);
    let reference = &data.type_reference_directives().unwrap()[0];
    assert_eq!(reference.file_name.as_bytes(), b"two");
    assert_eq!(reference.resolution_mode, 99);
    assert!(reference.preserve);
    assert_eq!(
        &text[reference.loc.pos() as usize..reference.loc.end() as usize],
        b"two"
    );
    assert!(data.diagnostics.is_empty());

    let file = parse(
        b"/pragma-invalid.ts",
        b"/// <reference nope='x' />\n/// <reference types='x' resolution-mode='bad' />\n",
        ScriptKind::TS,
    );
    let data = file.view().source_file(file.root()).unwrap();
    let actual: Vec<_> = data
        .diagnostics
        .iter()
        .map(|d| (d.code, d.loc.pos(), d.loc.end()))
        .collect();
    assert_eq!(actual, [(1084, 0, 26), (1453, 69, 72)]);
    assert_eq!(
        data.type_reference_directives().unwrap()[0].resolution_mode,
        0
    );
}

#[test]
fn module_references_preserve_static_order_ambient_filter_and_node_prefix_precedence() {
    crate::on_parser_worker(|| {
        for (name, text, kind, expected, uri, ambient, augmentations, count) in [
            (b"/imports.ts".as_slice(), b"import a from 'fs'; import b from 'node:fs'; import c from 'node:test';".as_slice(), ScriptKind::TS, vec![b"fs".as_slice(), b"node:fs", b"node:test"], Tristate::TRUE, vec![], vec![], 14),
            (b"/dynamic.js", b"const x = require('fs'); import('x'); import.defer('y');", ScriptKind::JS, vec![b"fs".as_slice(), b"x", b"y"], Tristate::UNKNOWN, vec![], vec![], 18),
            (b"/ambient.d.ts", b"declare module 'a' { import x from './x'; import y from 'y'; declare module 'z' {} }", ScriptKind::TS, vec![b"y".as_slice()], Tristate::UNKNOWN, vec![b"a".as_slice()], vec![b"z".as_slice()], 18),
        ] {
            let file = parse(name, text, kind); let view = file.view(); let data = view.source_file(file.root()).unwrap();
            let imports: Vec<_> = data.imports().unwrap().iter().flatten().map(|&id| view.node(id).unwrap().data_source().as_string_literal().unwrap().text_owned()).collect();
            assert_eq!(imports.iter().map(JsString::as_bytes).collect::<Vec<_>>(), expected);
            assert_eq!(data.uses_uri_style_node_core_modules, uri);
            assert_eq!(data.node_count, count);
            assert_eq!(data.ambient_module_names().unwrap().iter().map(JsString::as_bytes).collect::<Vec<_>>(), ambient);
            let names: Vec<_> = data.module_augmentations().unwrap().iter().flatten().map(|&id| view.node(id).unwrap().data_source().as_string_literal().unwrap().text_owned()).collect();
            assert_eq!(names.iter().map(JsString::as_bytes).collect::<Vec<_>>(), augmentations);
        }
    });
}

#[test]
fn jsx_detection_reports_the_tag_and_declaration_files_ignore_force() {
    let opts = SourceFileParseOptions {
        file_name: JsString::from_bytes(b"/tag.tsx".as_slice()),
        external_module_indicator_options: ExternalModuleIndicatorOptions {
            jsx: true,
            force: false,
        },
        ..SourceFileParseOptions::default()
    };
    let file = crate::parse_source_file(
        SourceText::from_loaded_bytes(b"const x = <div/>;".as_slice()),
        ScriptKind::TSX,
        opts,
    );
    let view = file.view();
    let metadata = view.source_file(file.root()).unwrap();
    let indicator = view
        .node(metadata.external_module_indicator.unwrap())
        .unwrap();
    assert_eq!(indicator.kind(), K::JsxSelfClosingElement);
    assert_eq!((indicator.range().pos(), indicator.range().end()), (9, 16));
    assert_eq!(metadata.node_count, 9);
    let file = crate::parse_source_file(
        SourceText::default(),
        ScriptKind::TS,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/empty.d.ts".as_slice()),
            external_module_indicator_options: ExternalModuleIndicatorOptions {
                jsx: true,
                force: true,
            },
            ..SourceFileParseOptions::default()
        },
    );
    assert!(file
        .view()
        .source_file(file.root())
        .unwrap()
        .external_module_indicator
        .is_none());
}

#[test]
fn isolated_name_fragment_retains_source_and_has_no_synthetic_source_parent() {
    let file =
        crate::parse_isolated_entity_name(SourceText::from_loaded_bytes(b"Alpha.Beta".as_slice()))
            .unwrap();
    let root = file.view().node(file.root()).unwrap();
    assert_eq!(root.kind(), K::QualifiedName);
    assert_eq!(root.parent(), None);
    assert!(
        crate::parse_isolated_entity_name(SourceText::from_loaded_bytes(b"Alpha.".as_slice()))
            .is_none()
    );
}

#[test]
fn javascript_only_diagnostics_preserve_emission_order_and_related_locations() {
    crate::on_parser_worker(|| {
        let file = parse(
            b"/typed.js",
            b"class C<T> { public x?: number; m<U>(p?: number): void {} }",
            ScriptKind::JS,
        );
        let data = file.view().source_file(file.root()).unwrap();
        let diagnostics: Vec<_> = data
            .js_diagnostics
            .iter()
            .map(|d| (d.code, d.loc.pos(), d.loc.end()))
            .collect();
        assert_eq!(
            diagnostics,
            [
                (8009, 21, 22),
                (8010, 24, 30),
                (8009, 13, 19),
                (8009, 38, 39),
                (8010, 41, 47),
                (8010, 50, 54),
                (8004, 34, 35),
                (8004, 8, 9)
            ]
        );
        assert!(data.diagnostics.is_empty());
        assert_eq!(data.js_diagnostics[0].message_args[0].as_bytes(), b"?");
        assert_eq!(data.js_diagnostics[2].message_args[0].as_bytes(), b"public");

        let file = parse(
            b"/signature.js",
            b"function f(public x?: number): void;",
            ScriptKind::JS,
        );
        let data = file.view().source_file(file.root()).unwrap();
        assert_eq!(
            data.js_diagnostics
                .iter()
                .map(|d| (d.code, d.loc.pos(), d.loc.end()))
                .collect::<Vec<_>>(),
            [
                (8009, 19, 20),
                (8010, 22, 28),
                (8012, 11, 17),
                (8017, 0, 36)
            ]
        );

        let file = parse(
            b"/decorators.js",
            b"@a export @b class C {}",
            ScriptKind::JS,
        );
        let data = file.view().source_file(file.root()).unwrap();
        assert_eq!(data.js_diagnostics.len(), 1);
        let diagnostic = &data.js_diagnostics[0];
        assert_eq!(
            (diagnostic.code, diagnostic.loc.pos(), diagnostic.loc.end()),
            (8038, 10, 12)
        );
        assert_eq!(diagnostic.file, Some(file.root()));
        assert_eq!(diagnostic.related_information.len(), 1);
        let related = &diagnostic.related_information[0];
        assert_eq!(
            (related.code, related.loc.pos(), related.loc.end()),
            (1486, 0, 2)
        );
        assert_eq!(related.file, Some(file.root()));
    });
}

#[test]
fn top_level_await_reparse_keeps_discarded_allocation_counts_and_final_statement_parents() {
    crate::on_parser_worker(|| {
        for (text, nodes, texts, identifiers, expected_flags, expected_diagnostics) in [
            (
                b"export {}; await + 1; let x = 2;".as_slice(),
                19,
                5,
                2,
                [0, 8192, 0],
                vec![],
            ),
            (
                b"await + 1; await - 2; export {};".as_slice(),
                23,
                6,
                2,
                [8192, 8192, 0],
                vec![],
            ),
            (
                b"await x; let y = ; export {};".as_slice(),
                12,
                3,
                3,
                [0, 0, 0],
                vec![(1109, 17, 18)],
            ),
        ] {
            let file = parse(b"/await.ts", text, ScriptKind::TS);
            let view = file.view();
            let root = file.root();
            let data = view.source_file(root).unwrap();
            assert_eq!(
                (data.node_count, data.text_count, data.identifier_count),
                (nodes, texts, identifiers)
            );
            assert_eq!(
                data.diagnostics
                    .iter()
                    .map(|d| (d.code, d.loc.pos(), d.loc.end()))
                    .collect::<Vec<_>>(),
                expected_diagnostics
            );
            let list = view
                .node(root)
                .unwrap()
                .data_source()
                .as_source_file()
                .unwrap()
                .statements()
                .unwrap();
            let statements = view.node_slice(view.list(list).unwrap().nodes()).unwrap();
            assert_eq!(statements.len(), 3);
            for (node, flags) in statements.iter().zip(expected_flags) {
                let node = view.node(node.unwrap()).unwrap();
                assert_eq!(node.flags(), flags);
                assert_eq!(node.parent(), Some(root));
            }
        }
    });
}

#[test]
fn a_failed_parse_drops_its_owner_and_the_same_batch_worker_accepts_the_next_request() {
    crate::on_parser_worker(|| {
        let worker = std::thread::current().id();
        let counters = ts_arena::Counters::default();
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::parse_source_file_with_counters(
                SourceText::from_loaded_bytes(b"const x = 1;".as_slice()),
                ScriptKind::TS,
                SourceFileParseOptions {
                    file_name: JsString::from_bytes(b"relative.ts".as_slice()),
                    ..SourceFileParseOptions::default()
                },
                &counters,
            )
        }));
        assert!(failed.is_err());
        assert_eq!(counters.snapshot(), ts_arena::Counts::default());
        let next = crate::parse_source_file_with_counters(
            SourceText::from_loaded_bytes(b"const x = 1;".as_slice()),
            ScriptKind::TS,
            SourceFileParseOptions {
                file_name: JsString::from_bytes(b"/valid.ts".as_slice()),
                ..SourceFileParseOptions::default()
            },
            &counters,
        );
        assert_eq!(std::thread::current().id(), worker);
        assert!(next
            .view()
            .source_file(next.root())
            .unwrap()
            .diagnostics
            .is_empty());
        assert_eq!(counters.snapshot().owners, 1);
        drop(next);
        assert_eq!(counters.snapshot(), ts_arena::Counts::default());
    });
}
