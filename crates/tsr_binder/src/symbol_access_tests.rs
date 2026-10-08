use crate::{backend::Backend, symbol_access::BindingSymbol, table_access::BindingTable, Binder};
use std::panic::{catch_unwind, AssertUnwindSafe};
use tsr_arena::{Counters, SymbolArena};
use tsr_ast::{internal_symbol_names as names, JsString, SourceFileParseOptions};
use tsr_core::ScriptKind;
use tsr_jsstring::SourceText;

fn parsed() -> tsr_ast::ParsedFile {
    tsr_parser::parse_source_file(
        SourceText::default(),
        ScriptKind::TS,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/scoped-symbols.ts".as_slice()),
            ..Default::default()
        },
    )
}

#[test]
fn declaration_merges_keep_local_symbols_and_tables_and_canonical_parent_identity() {
    parsed()
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|local| {
                    let mut b = Binder::from_backend(Backend::Local(local));
                    let source = b.binding_node(b.file);
                    let parent =
                        b.new_binding_symbol(0, JsString::from_bytes(b"parent".as_slice()));
                    let table = b.new_binding_table();
                    assert!(matches!(parent, BindingSymbol::Local(_)));
                    assert!(matches!(table, BindingTable::Local(_)));
                    assert!(b.binding_table_get(table, names::EXPORT_EQUALS).is_none());
                    assert!(b
                        .binding_table_insert(
                            table,
                            JsString::from_bytes(names::EXPORT_EQUALS),
                            None
                        )
                        .is_none());
                    assert!(matches!(
                        b.binding_table_get(table, names::EXPORT_EQUALS),
                        Some(None)
                    ));
                    let first = b.declare_binding_symbol(table, Some(parent), source, 0, 0);
                    let checked_parent = BindingSymbol::Checked(b.symbol_id(parent));
                    let checked_table = BindingTable::Checked(b.table_id(table));
                    assert!(b.same_symbol(Some(parent), Some(checked_parent)));
                    assert_eq!(b.table_id(table), b.table_id(checked_table));
                    let second =
                        b.declare_binding_symbol(checked_table, Some(checked_parent), source, 0, 0);
                    assert!(matches!(second, BindingSymbol::Local(_)));
                    assert!(b.same_symbol(Some(first), Some(second)));
                    assert_eq!(b.symbol_count, 2);
                    assert_eq!(
                        b.builder
                            .declarations()
                            .get(b.s_binding(first).declarations())?
                            .len(),
                        1
                    );
                    assert!(b.same_symbol(b.node_binding_symbol(source), Some(first)));
                    assert!(b.same_symbol(b.binding_symbol_parent(first), Some(parent)));
                    Ok(())
                })
                .expect("eligible local source")
        })
        .unwrap();
}

#[test]
fn declaration_lookup_reuse_preserves_merge_replacement_and_conflict_states() {
    use tsr_ast::{symbol_flags as sf, NodeId};
    use tsr_core::TextRange;

    fn declarations<'scope>(
        b: &Binder<'_, 'scope, '_>,
        s: BindingSymbol<'scope>,
    ) -> Vec<Option<NodeId>> {
        b.builder
            .declarations()
            .get(b.s_binding(s).declarations())
            .unwrap()
            .to_vec()
    }

    fn check(b: &mut Binder<'_, '_, '_>, checked_table: bool) {
        let list = b.n(b.file).statement_list().unwrap();
        let nodes = b
            .view()
            .node_slice(b.view().list(list).unwrap().nodes())
            .unwrap();
        let raw = [
            nodes.at(0).unwrap(),
            nodes.at(1).unwrap(),
            nodes.at(2).unwrap(),
        ];
        let [first_node, next_node, missing_node] = raw.map(|node| b.binding_node(node));
        let parent = b.new_binding_symbol(0, JsString::from_bytes(b"parent".as_slice()));
        let (f, p, m) = (sf::FUNCTION, sf::PROPERTY, sf::METHOD);
        for (case, initial, initial_replaceable, incoming, excludes, replaceable) in [
            ("merge", f, false, f, 0, false),
            ("present nil", f, false, f, 0, false),
            ("replace", p, true, m, p, false),
            ("early return", m, false, p, m, true),
            ("duplicate", p, false, p, p, false),
        ] {
            let table = b.new_binding_table();
            let table = if checked_table {
                BindingTable::Checked(b.table_id(table))
            } else {
                table
            };
            assert!(b.binding_table_get(table, b"entry").is_none());
            if case == "present nil" {
                b.binding_table_insert(table, JsString::from_bytes(b"entry".as_slice()), None);
                assert!(matches!(b.binding_table_get(table, b"entry"), Some(None)));
            }
            let before = b.symbol_count;
            let previous_next = b.node_binding_symbol(next_node);
            let mut declare = |node, flags, excludes, replaceable| {
                b.declare_binding_symbol_ex(
                    table,
                    Some(parent),
                    node,
                    flags,
                    excludes,
                    replaceable,
                    false,
                )
            };
            let first = declare(first_node, initial, 0, initial_replaceable);
            let next = declare(next_node, incoming, excludes, replaceable);
            if case == "early return" {
                assert!(b.same_symbol(b.node_binding_symbol(next_node), previous_next));
            }
            let merged = matches!(case, "merge" | "present nil");
            let same = merged || case == "early return";
            assert_eq!(b.same_symbol(Some(first), Some(next)), same, "{case}");
            assert_eq!(b.symbol_count - before, if same { 1 } else { 2 }, "{case}");
            let stored = b.binding_table_get(table, b"entry").flatten();
            let expected = if case == "replace" { next } else { first };
            assert!(b.same_symbol(stored, Some(expected)), "{case}");
            assert_eq!(b.table_binding(table).len(), 1, "{case}");
            let flags = if matches!(case, "replace" | "early return") {
                m
            } else {
                incoming
            };
            assert_eq!(b.s_binding(next).flags(), flags, "{case}");
            assert!(
                b.same_symbol(b.binding_symbol_parent(next), Some(parent)),
                "{case}"
            );
            let expected = if merged {
                vec![Some(raw[0]), Some(raw[1])]
            } else {
                vec![Some(raw[0])]
            };
            assert_eq!(declarations(b, first), expected, "{case}");
            if !same {
                assert_eq!(declarations(b, next), [Some(raw[1])], "{case}");
            }
            // An early return must preserve the incoming node's previous symbol.
            assert_eq!(
                b.same_symbol(b.node_binding_symbol(next_node), Some(next)),
                case != "early return",
                "{case}"
            );
            let diagnostics = b.builder.diagnostics_mut();
            assert_eq!(
                diagnostics.len(),
                if case == "duplicate" { 2 } else { 0 },
                "{case}"
            );
            for (diagnostic, range) in diagnostics
                .iter()
                .zip([TextRange::new(9, 14), TextRange::new(29, 34)])
            {
                assert_eq!(diagnostic.code, 2300);
                assert_eq!(diagnostic.loc, range);
                assert_eq!(
                    diagnostic.message_args,
                    [JsString::from_bytes(b"entry".as_slice())]
                );
            }
        }
        let table = b.new_binding_table();
        let table = if checked_table {
            BindingTable::Checked(b.table_id(table))
        } else {
            table
        };
        let seed = b.new_binding_symbol(0, JsString::from_bytes(names::MISSING));
        b.binding_table_insert(table, JsString::from_bytes(names::MISSING), Some(seed));
        let before = b.symbol_count;
        let mut declare = || {
            b.declare_binding_symbol_ex(
                table,
                Some(parent),
                missing_node,
                sf::FUNCTION,
                sf::ALL,
                true,
                false,
            )
        };
        let (first, next) = (declare(), declare());
        assert!(!b.same_symbol(Some(first), Some(next)));
        assert_eq!(b.symbol_count - before, 2);
        assert!(b.same_symbol(
            b.binding_table_get(table, names::MISSING).flatten(),
            Some(seed)
        ));
        assert_eq!(b.table_binding(table).len(), 1);
        assert_eq!(b.s_binding(next).flags(), sf::FUNCTION);
        assert_eq!(declarations(b, next), [Some(raw[2])]);
    }

    for local in [false, true] {
        tsr_parser::parse_source_file(
            SourceText::from_loaded_bytes(b"function entry() {} function entry() {} ;".as_slice()),
            ScriptKind::TS,
            SourceFileParseOptions {
                file_name: JsString::from_bytes(b"/declaration-states.ts".as_slice()),
                ..Default::default()
            },
        )
        .bind_and_publish(|builder| {
            if local {
                builder
                    .with_local_scope(|local| {
                        let mut binder = Binder::from_backend(Backend::Local(local));
                        check(&mut binder, false);
                        binder.builder.diagnostics_mut().clear();
                        check(&mut binder, true);
                    })
                    .expect("eligible local source");
            } else {
                check(&mut Binder::new(builder), true);
            }
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn tombstones_and_foreign_symbol_links_keep_their_deferred_failure_boundary() {
    let counters = Counters::new();
    let mut foreign = SymbolArena::new(&counters);
    let foreign = foreign.push(());
    parsed()
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|local| {
                    let mut b = Binder::from_backend(Backend::Local(local));
                    let table = b.new_binding_table();
                    let raw_table = b.table_id(table);
                    b.table_mut(raw_table)
                        .insert(JsString::from_bytes(b"foreign".as_slice()), Some(foreign));
                    let value = b.binding_table_get(table, b"foreign").unwrap().unwrap();
                    assert!(matches!(value, BindingSymbol::Checked(id) if id == foreign));
                    assert!(catch_unwind(AssertUnwindSafe(|| b.s_binding(value))).is_err());
                    let previous = b
                        .binding_table_insert(
                            table,
                            JsString::from_bytes(b"foreign".as_slice()),
                            None,
                        )
                        .unwrap()
                        .unwrap();
                    assert!(b.same_symbol(Some(previous), Some(value)));
                    assert!(matches!(b.binding_table_get(table, b"foreign"), Some(None)));
                    assert!(b.binding_table_get(table, b"absent").is_none());

                    let source = b.binding_node(b.file);
                    assert!(b.node_binding_symbol(source).is_none());
                    assert!(catch_unwind(AssertUnwindSafe(
                        || b.set_binding_node_symbol(source, Some(value))
                    ))
                    .is_err());
                    assert!(b.node_binding_symbol(source).is_none());
                    assert!(b
                        .builder
                        .result()
                        .node_binding(b.parsed_view(), b.file)
                        .is_none());
                    let local = b.new_binding_symbol(0, tsr_ast::JsString::default());
                    let raw = b.symbol_id(local);
                    b.set_symbol_parent(raw, Some(foreign));
                    let parent = b.binding_symbol_parent(local).unwrap();
                    assert!(matches!(parent, BindingSymbol::Checked(id) if id == foreign));
                    b.set_binding_symbol_parent(local, None);
                    assert!(b.binding_symbol_parent(local).is_none());
                    Ok(())
                })
                .expect("eligible local source")
        })
        .unwrap();
}

#[test]
fn declaration_and_infer_payload_failures_keep_their_named_messages() {
    use tsr_ast::{AstBuilder, FactoryMethods, SyntaxKind};

    let text = SourceText::default();
    let mut build = AstBuilder::new(text.clone(), &Counters::new());
    let conditional = build.new_token(SyntaxKind::ConditionalType.into());
    let child = build.new_identifier(JsString::from_bytes(b"T".as_slice()));
    let export = build.new_token(SyntaxKind::ExportAssignment.into());
    let source = build.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/payload-failures.ts".as_slice()),
            ..Default::default()
        },
        text,
        None,
        None,
    );
    build.node_mut(child).unwrap().set_parent(Some(conditional));
    build
        .node_mut(conditional)
        .unwrap()
        .set_parent(Some(source));
    build.node_mut(export).unwrap().set_parent(Some(source));
    build
        .complete(source)
        .unwrap()
        .bind_and_publish(|builder| {
            let check = |binder: &Binder<'_, '_, '_>| {
                for (node, infer, expected) in [
                    (child, true, "conditional type payload"),
                    (export, false, "export assignment payload"),
                ] {
                    let failure = catch_unwind(AssertUnwindSafe(|| {
                        let node = binder.binding_node(node);
                        if infer {
                            binder.get_infer_type_container(node);
                        } else {
                            binder.declaration_name(node);
                        }
                    }))
                    .unwrap_err();
                    let message = failure
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| failure.downcast_ref::<&str>().copied())
                        .expect("text payload contract panic");
                    assert_eq!(message, expected);
                }
            };
            check(&Binder::new(builder));
            builder
                .with_local_scope(|local| check(&Binder::from_backend(Backend::Local(local))))
                .expect("constructed payload failures admit local access");
            Ok(())
        })
        .unwrap();
}
