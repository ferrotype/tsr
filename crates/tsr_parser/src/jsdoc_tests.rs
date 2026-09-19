use tsr_ast::{JsString, SourceFileParseOptions};
use tsr_core::ScriptKind;
use tsr_jsstring::SourceText;

// These allocation/identifier counts were observed through the pinned Go
// ParseSourceFile before Rust comparison. They include discarded speculation
// and distinguish eager JS elaboration from deferred TS JSDoc.
#[test]
fn jsdoc_eager_and_deferred_counts_match_pinned_go() {
    type Counts = (i64, i64, i64);
    type Case<'a> = (&'a str, &'a [u8], Counts, Counts);
    let cases: &[Case<'_>] = &[
        (
            "parameter",
            b"/** @param {number} x @returns {string} text */\nfunction f(x) { return \"\"; }",
            (24, 9, 7),
            (9, 3, 2),
        ),
        (
            "optional",
            b"/** @param {number} [x=42] */\nfunction f(x) {}",
            (18, 6, 5),
            (7, 2, 2),
        ),
        (
            "nested",
            b"/** @param {Object} x\n * @param {string} x.y\n */\nfunction f(x) {}",
            (29, 11, 10),
            (7, 2, 2),
        ),
        (
            "typedef",
            b"/** @typedef {Object} A\n * @property {string} x\n * @property {number} [y]\n */",
            (33, 13, 10),
            (2, 0, 0),
        ),
        (
            "namespace",
            b"/** @typedef {string} A.B.C */",
            (24, 8, 5),
            (2, 0, 0),
        ),
        (
            "callback",
            b"/** @callback F\n * @param {string} x\n * @returns {number}\n */",
            (27, 10, 8),
            (2, 0, 0),
        ),
        (
            "template",
            b"/** @template {string} T,U\n * @param {T} x */\nfunction f(x) {}",
            (31, 13, 10),
            (7, 2, 2),
        ),
        (
            "constraint-default",
            b"/** @template {string} [T=\"x\"] */\nclass C {}",
            (19, 7, 4),
            (4, 1, 1),
        ),
        (
            "link",
            b"/** hello {@link Foo.bar label} and {@linkplain Other} */\nlet x;",
            (16, 9, 4),
            (16, 9, 4),
        ),
        (
            "see",
            b"/** @see Foo.bar\n * @see http://example.test */\nlet x;",
            (16, 6, 5),
            (16, 6, 5),
        ),
        (
            "malformed",
            b"/** @param {string missing\n * @returns {number} */\nfunction f() {}",
            (18, 6, 6),
            (5, 1, 1),
        ),
        (
            "invalid-name",
            b"/** @callback F\n * @param {number} [\"a b\"] */",
            (24, 9, 5),
            (2, 0, 0),
        ),
        (
            "imports",
            b"/** @import {A as B} from \"m\" */\nlet x;",
            (22, 8, 4),
            (6, 1, 1),
        ),
        (
            "implements",
            b"/** @implements {I<string>}\n * @extends {B<number>} */\nclass C extends B {}",
            (24, 8, 6),
            (7, 2, 2),
        ),
        (
            "satisfies",
            b"/** @satisfies {string} */ const x = \"a\";",
            (15, 4, 3),
            (7, 2, 1),
        ),
        (
            "function-type",
            b"/** @type {function(number): string} */ const f = x => \"\";",
            (20, 7, 4),
            (11, 3, 2),
        ),
        (
            "multiple-docs",
            b"/** @param {number} x */\n/** @param {string} y */\nfunction f(x,y) {}",
            (24, 9, 9),
            (9, 3, 3),
        ),
    ];
    for &(name, source, js, ts) in cases {
        for (kind, expected) in [(ScriptKind::JS, js), (ScriptKind::TS, ts)] {
            let file = crate::parse_source_file(
                SourceText::from_loaded_bytes(source),
                kind,
                SourceFileParseOptions {
                    file_name: JsString::from_bytes(b"/probe".as_slice()),
                    ..Default::default()
                },
            );
            let view = file.view();
            let data = view.source_file(file.root()).unwrap();
            assert_eq!(
                (data.node_count, data.text_count, data.identifier_count),
                expected,
                "{name}, kind {}",
                kind.0
            );
        }
    }
}

#[test]
fn type_tag_sets_the_full_signature_edge_and_its_immediate_parent() {
    use std::ops::ControlFlow;
    use tsr_ast::{ChildVisitor, NodeId, NodeListId, NodeSlice};
    let file = crate::parse_source_file(
        SourceText::from_loaded_bytes(
            b"/**\n * @type {...Object}  ``` @see ignore ```\n */\nfunction f(x,y) {}".as_slice(),
        ),
        ScriptKind::JS,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/probe.js".as_slice()),
            ..Default::default()
        },
    );
    let view = file.view();
    let list = view
        .node(file.root())
        .unwrap()
        .data_source()
        .as_source_file()
        .unwrap()
        .statements()
        .unwrap();
    let fun = view
        .node_slice(view.list(list).unwrap().nodes())
        .unwrap()
        .at(0)
        .unwrap();
    let node = view.node(fun).unwrap();
    let data = node.data_source().as_function_declaration().unwrap();
    assert!(data.r#type().is_none());
    let signature = data
        .full_signature()
        .expect("@type annotates FullSignature");
    assert_eq!(view.node(signature).unwrap().parent(), Some(fun));
    assert_eq!(
        view.node(signature).unwrap().kind(),
        tsr_ast::SyntaxKind::JSDocVariadicType
    );
    struct Edges(Vec<NodeId>);
    impl ChildVisitor for Edges {
        fn visit_node(&mut self, id: NodeId) -> ControlFlow<()> {
            self.0.push(id);
            ControlFlow::Continue(())
        }
        fn visit_list(&mut self, _: NodeListId) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }
        fn visit_node_slice(&mut self, _: NodeSlice) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }
    }
    let mut edges = Edges(Vec::new());
    let _ = node.for_each_child(&mut edges);
    assert!(edges.0.contains(&signature));
}

#[test]
fn nested_jsdoc_namespaces_grow_the_stack_during_parse_and_reparse() {
    // Optimized frame sizes differ by target: 6,000 levels fit in the initial
    // stack on Linux/x86_64. Increase input depth until growth is observed,
    // keeping the production guard and the initial stack size unchanged.
    for depth in [6000, 12_000, 24_000, 48_000] {
        let result = std::thread::Builder::new()
            .stack_size(512 * 1024)
            .spawn(move || {
                let text = format!("/** @typedef {{string}} {}Z */", "A.".repeat(depth));
                let source = SourceText::from_loaded_bytes(text.into_bytes());
                let factory =
                    tsr_ast::AstBuilder::new(source.clone(), &tsr_arena::Counters::default());
                let mut parser = crate::Parser::new(
                    SourceFileParseOptions {
                        file_name: JsString::from_bytes(b"/depth.js".as_slice()),
                        ..Default::default()
                    },
                    &source,
                    ScriptKind::JS,
                    factory,
                );
                crate::recursion::take_observations();
                parser.next_token();
                let root = parser.parse_source_file_worker();
                assert!(parser.diagnostics.is_empty());
                assert!(parser.jsdoc_diagnostics.is_empty());
                parser.factory.complete(root).unwrap();
                crate::recursion::take_observations()
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(result.entries > 2 * depth, "depth {depth}: {result:?}");
        if result.growths > 0 {
            return;
        }
        assert!(depth < 48_000, "depth {depth}: {result:?}");
    }
}
