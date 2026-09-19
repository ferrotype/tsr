//! Both production entry points must expose the same completed graph. These
//! comparisons supplement the independent Go corpus; they are not Go evidence.
use std::panic::{catch_unwind, AssertUnwindSafe};
use ts_arena::{Counters, Error};
use ts_ast::{
    node_flags, AstBuilder, AstView, BindError, ContentMapperSourceFileInfo, FactoryMethods,
    JsString, NodeId, ParsedFile, SourceFileParseOptions, SyntaxKind,
};
use ts_core::ScriptKind;
use ts_jsstring::SourceText;

#[path = "../examples/support/graph.rs"]
mod graph;

fn parse(bytes: &[u8], kind: ScriptKind, counters: &Counters) -> ParsedFile {
    ts_parser::parse_source_file_with_counters(
        SourceText::from_loaded_bytes(bytes),
        kind,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/exclusive.ts".as_slice()),
            path: JsString::from_bytes(b"/exclusive.ts".as_slice()),
            ..Default::default()
        },
        counters,
    )
}

fn first_statement(view: AstView<'_>, source: NodeId) -> NodeId {
    let list = view.node(source).unwrap().statement_list().unwrap();
    let nodes = view.list(list).unwrap().nodes();
    view.node_slice(nodes).unwrap().at(0).unwrap()
}

fn assert_graph_eq(
    expected: &[(&str, serde_json::Value)],
    actual: &[(&str, serde_json::Value)],
    name: &str,
) {
    assert_eq!(expected.len(), actual.len(), "{name}: graph record count");
    for (index, (expected, actual)) in expected.iter().zip(actual).enumerate() {
        assert_eq!(expected.0, actual.0, "{name}: graph record {index} kind");
        let (mut expected, mut actual) = (expected.1.clone(), actual.1.clone());
        normalize_runtime_names(&mut expected);
        normalize_runtime_names(&mut actual);
        assert_eq!(expected, actual, "{name}: graph record {index}");
    }
}

fn normalize_runtime_names(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                normalize_runtime_names(item);
            }
        }
        serde_json::Value::Object(fields) => {
            // The observer supplies this identity only after verifying the raw
            // name against its owning node/symbol's current runtime ID. Separate
            // parses receive distinct process IDs; compare canonical graph refs
            // and the exact prefix/suffix. Unrecognized raw names stay exact.
            if fields.len() == 2
                && fields.contains_key("raw_hex")
                && fields.get("identity").is_some_and(|id| !id.is_null())
            {
                fields.remove("raw_hex");
            }
            for field in fields.values_mut() {
                normalize_runtime_names(field);
            }
        }
        _ => {}
    }
}

#[test]
fn consuming_and_published_entries_have_identical_complete_graphs() {
    let cases: &[(&str, ScriptKind, &[u8])] = &[
        (
            "control flow and functions-first traversal",
            ScriptKind::TS,
            b"f(1); function f(x: number) { for (let i = 0; i < x; i++) { if (i) continue; } try { switch (x) { case 0: case 1: return x; default: throw x; } } finally { x++; } }",
        ),
        (
            "CommonJS and eager JSDoc",
            ScriptKind::JS,
            b"/** @typedef {{x: number}} T */\nexports.x = this; /** @param {T} x */ function f(x) { this.x = x; return x?.x; } f.extra = 1; module.exports.f = f;",
        ),
        (
            "private identifiers and module symbols",
            ScriptKind::TS,
            b"namespace N { export class C { #x = 1; method(x: number) { return this.#x + x; } } } declare module 'ambient' { export const x: number; }",
        ),
        (
            "destructuring and optional calls",
            ScriptKind::JS,
            b"function f(value) { let [a, ...rest] = value; ({x: a, ...rest} = value); return value?.method?.(a) ?? rest; }",
        ),
        (
            "recovered malformed syntax",
            ScriptKind::TS,
            b"function f( { if (true) return; const x = ; } let a = [,];",
        ),
        (
            "cooked identifiers and malformed bytes",
            ScriptKind::TS,
            b"let \\u0061 = 1; const x = '\xff'; a++;",
        ),
        (
            "borrowed and cooked contextual keyword checks",
            ScriptKind::TS,
            b"'use strict'; let \\u0069mplements = 1; implements; const value = 1; value;",
        ),
        (
            "JSX and empty child lists",
            ScriptKind::TSX,
            b"const element = <div><span />{items.map(x => <b>{x}</b>)}</div>; function empty() {}",
        ),
    ];
    for &(name, kind, bytes) in cases {
        let counters = Counters::new();
        let baseline = counters.snapshot();
        let parsed = parse(bytes, kind, &counters);
        let original = graph::collect(parsed.view(), parsed.root(), None);
        let shared = parsed.publish_unbound();
        let source = shared.root().unwrap();
        let bound = crate::bind_source_file(&shared, source).unwrap();
        let view = bound.view();
        let expected = graph::collect(view.ast(), source, Some(view.result()));
        let repeated = crate::bind_source_file(&shared, source).unwrap();
        assert!(std::ptr::eq(
            bound.view().result(),
            repeated.view().result()
        ));
        assert_eq!(
            expected,
            graph::collect(
                repeated.view().ast(),
                source,
                Some(repeated.view().result())
            ),
            "{name}: repeated published binding"
        );

        let parsed = parse(bytes, kind, &counters);
        assert_eq!(
            original,
            graph::collect(parsed.view(), parsed.root(), None),
            "{name}: input parse"
        );
        let completed = crate::bind_parsed_file(parsed).unwrap();
        assert!(
            completed.bound_in_place(),
            "{name}: ordinary parse must use exclusive binding"
        );
        let view = completed.view();
        assert_graph_eq(
            &expected,
            &graph::collect(view.ast(), completed.source(), Some(view.result())),
            name,
        );
        let retained = completed.clone();
        assert!(std::ptr::eq(
            completed.view().result(),
            retained.view().result()
        ));
        drop((completed, retained, repeated, bound, shared));
        assert_eq!(
            counters.snapshot(),
            baseline,
            "{name}: final tracked storage disposal"
        );
    }
}

#[test]
fn completed_nodes_and_symbols_retain_storage_and_preassigned_identity() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let parsed = parse(
        b"exports.value = 1; function f() { return this; }",
        ScriptKind::JS,
        &counters,
    );
    let source = parsed.root();
    let statement = first_statement(parsed.view(), source);
    let identity = ts_ast::runtime_node_id(&parsed.view().node(statement).unwrap());
    let completed = crate::bind_parsed_file(parsed).unwrap();
    assert!(completed.bound_in_place());
    let node = completed.retain_node(statement).unwrap();
    let symbol_id = completed.view().result().symbols().iter().next().unwrap().0;
    let symbol = completed.retain_symbol(symbol_id).unwrap();
    let name = symbol.symbol().name_to_owned();
    drop(completed);
    assert_eq!(node.node().kind(), SyntaxKind::ExpressionStatement);
    assert_eq!(ts_ast::runtime_node_id(&node.node()), identity);
    assert!(node
        .file()
        .view()
        .source_file()
        .unwrap()
        .common_js_module_indicator()
        .is_some());
    assert!(counters.snapshot().allocations > baseline.allocations);
    drop(node);
    assert_eq!(symbol.symbol().name_bytes(), name.as_bytes());
    assert!(symbol.file().view().node(source).is_ok());
    drop(symbol);
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn consuming_initializer_error_and_panic_drop_partial_binding() {
    for panics in [false, true] {
        let counters = Counters::new();
        let baseline = counters.snapshot();
        let parsed = parse(
            b"let x = 1; function f() { return x; }",
            ScriptKind::TS,
            &counters,
        );
        let source = parsed.root();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            ts_parser::on_parser_worker(|| {
                parsed.bind_and_publish(|builder| {
                    crate::initialize_binding(builder);
                    builder.node_mut(source)?.set_flags(node_flags::UNREACHABLE);
                    assert!(builder.result().symbol_count() > 0);
                    if panics {
                        std::panic::panic_any("exclusive initializer failed after mutation");
                    }
                    Err(Error::InvalidGraph)
                })
            })
        }));
        if panics {
            let payload = outcome.unwrap_err();
            assert_eq!(
                payload.downcast_ref::<&str>(),
                Some(&"exclusive initializer failed after mutation")
            );
        } else {
            assert!(matches!(
                outcome.unwrap(),
                Err(BindError::Storage(Error::InvalidGraph))
            ));
        }
        assert_eq!(
            counters.snapshot(),
            baseline,
            "failed private construction must drop its storage"
        );
    }
}

#[test]
fn fragment_binding_returns_invalid_graph_and_drops_storage() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let mut builder = AstBuilder::new(SourceText::default(), &counters);
    let root = builder.new_identifier(JsString::from_bytes(b"fragment".as_slice()));
    let parsed = builder.complete(root).unwrap();
    assert!(matches!(
        crate::bind_parsed_file(parsed),
        Err(BindError::Storage(Error::InvalidGraph))
    ));
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn imported_owner_selects_compatibility_before_binding() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let foreign = parse(b"let foreign = 1;", ScriptKind::TS, &counters).publish_unbound();
    let foreign_root = foreign.root().unwrap();
    let mut parsed = parse(b"let local = 1;", ScriptKind::TS, &counters);
    parsed.builder_mut().retain_file(foreign.clone());
    let completed = crate::bind_parsed_file(parsed).unwrap();
    assert!(!completed.bound_in_place());
    drop(foreign);
    assert!(completed.view().ast().node(foreign_root).is_ok());
    assert!(matches!(
        completed.retain_node(foreign_root),
        Err(Error::InvalidGraph)
    ));
    drop(completed);
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn same_arena_sources_select_compatibility_and_reject_sibling_writes() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let mut parsed = parse(b"let local = 1;", ScriptKind::TS, &counters);
    let second = parsed.builder_mut().new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/sibling.ts".as_slice()),
            ..Default::default()
        },
        SourceText::default(),
        None,
        None,
    );
    let completed = ts_parser::on_parser_worker(|| {
        parsed.bind_and_publish(|builder| {
            assert!(matches!(builder.node_mut(second), Err(Error::WrongOwner)));
            assert!(matches!(
                builder.binding_mut(second),
                Err(Error::WrongOwner)
            ));
            crate::initialize_binding(builder);
            Ok(())
        })
    })
    .unwrap();
    assert!(!completed.bound_in_place());
    assert_eq!(completed.view().node(second).unwrap().flags(), 0);
    assert!(completed.view().node_binding(second).unwrap().is_none());
    drop(completed);
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn mapper_metadata_selects_compatibility() {
    let counters = Counters::new();
    let mut parsed = parse(b"let x = 1;", ScriptKind::TS, &counters);
    let source = parsed.root();
    parsed
        .builder_mut()
        .source_file_mut(source)
        .unwrap()
        .set_content_mapper_info(ContentMapperSourceFileInfo {
            content_mapper: JsString::from_bytes(b"test-mapper".as_slice()),
            ..Default::default()
        });
    let completed = crate::bind_parsed_file(parsed).unwrap();
    assert!(!completed.bound_in_place());
    assert_eq!(
        completed.view().source_file().unwrap().content_mapper(),
        b"test-mapper"
    );
}

#[test]
fn committed_lazy_records_select_compatibility_and_remain_retained() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let parsed = parse(b"let x = 1;", ScriptKind::TS, &counters);
    let source = parsed.root();
    let roots = parsed
        .view()
        .source_jsdoc(source, source, |transaction| {
            let node = transaction.new_identifier(JsString::from_bytes(b"lazy".as_slice()));
            transaction.node_mut(node)?.set_parent(Some(source));
            Ok(vec![node])
        })
        .unwrap();
    let lazy = roots[0];
    drop(roots);
    let completed = crate::bind_parsed_file(parsed).unwrap();
    assert!(!completed.bound_in_place());
    let retained = completed.retain_node(lazy).unwrap();
    drop(completed);
    assert_eq!(
        retained
            .node()
            .data_source()
            .as_identifier()
            .unwrap()
            .text(),
        b"lazy"
    );
    drop(retained);
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn unrestricted_parent_and_payload_edits_still_require_core_validation() {
    for before_binding in [false, true] {
        for parent_edge in [false, true] {
            let counters = Counters::new();
            let baseline = counters.snapshot();
            let foreign = parse(b"foreign;", ScriptKind::TS, &counters);
            let foreign_root = foreign.root();
            let foreign_counts = counters.snapshot();
            let mut parsed = parse(b"local;", ScriptKind::TS, &counters);
            let root = parsed.root();
            let statement = first_statement(parsed.view(), root);
            let corrupt = |node: &mut ts_ast::Node| {
                if parent_edge {
                    node.set_parent(Some(foreign_root));
                } else {
                    let ts_ast::NodeData::ExpressionStatement(data) = node.data_mut() else {
                        panic!("fixture statement must have an expression payload")
                    };
                    data.expression = Some(foreign_root);
                }
            };
            if before_binding {
                corrupt(&mut parsed.builder_mut().node_mut(statement).unwrap());
            }
            let outcome = ts_parser::on_parser_worker(|| {
                parsed.bind_and_publish(|builder| {
                    if !before_binding {
                        let mut node = builder.node_mut(statement)?;
                        corrupt(&mut node);
                    }
                    // A subsequent narrow write cannot restore a proof dirtied
                    // either before binding or through BindBuilder::node_mut.
                    builder.set_node_flags(root, node_flags::UNREACHABLE)?;
                    Ok(())
                })
            });
            assert!(
                matches!(outcome, Err(BindError::Storage(Error::WrongOwner))),
                "before_binding={before_binding}, parent_edge={parent_edge}"
            );
            assert_eq!(counters.snapshot(), foreign_counts);
            drop(foreign);
            assert_eq!(counters.snapshot(), baseline);
        }
    }
}

#[test]
fn preserved_core_proof_does_not_skip_binding_result_validation() {
    for invalid in ["indicator", "symbol", "flow", "diagnostic"] {
        let counters = Counters::new();
        let baseline = counters.snapshot();
        let foreign =
            crate::bind_parsed_file(parse(b"let foreign = 1;", ScriptKind::TS, &counters)).unwrap();
        let foreign_root = foreign.source();
        let foreign_symbol = foreign.view().result().symbols().iter().next().unwrap().0;
        let foreign_counts = counters.snapshot();
        let parsed = parse(b"let local = 1;", ScriptKind::TS, &counters);
        let root = parsed.root();
        let outcome = ts_parser::on_parser_worker(|| {
            parsed.bind_and_publish(|builder| {
                crate::initialize_binding(builder);
                builder.set_node_flags(root, node_flags::UNREACHABLE)?;
                match invalid {
                    "indicator" => builder.set_common_js_module_indicator(Some(foreign_root)),
                    "symbol" => builder.binding_mut(root)?.symbol = Some(foreign_symbol),
                    "flow" => {
                        builder.flows_mut().push(ts_ast::FlowNode::new_ex(
                            ts_ast::flow_flags::ASSIGNMENT,
                            Some(ts_ast::FlowData::Ast(foreign_root)),
                            None,
                        ));
                    }
                    "diagnostic" => builder.diagnostics_mut().push(ts_ast::Diagnostic::new(
                        Some(foreign_root),
                        ts_core::TextRange::new(0, 1),
                        ts_diagnostics::Identifier_expected,
                        vec![],
                    )),
                    _ => unreachable!(),
                }
                Ok(())
            })
        });
        assert!(
            matches!(outcome, Err(BindError::Storage(Error::WrongOwner))),
            "{invalid}"
        );
        assert_eq!(
            counters.snapshot(),
            foreign_counts,
            "{invalid}: failed result dropped"
        );
        drop(foreign);
        assert_eq!(counters.snapshot(), baseline);
    }
}
