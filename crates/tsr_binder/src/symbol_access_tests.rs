use crate::{backend::Backend, symbol_access::BindingSymbol, table_access::BindingTable, Binder};
use std::panic::{catch_unwind, AssertUnwindSafe};
use ts_arena::{Counters, SymbolArena};
use ts_ast::{internal_symbol_names as names, JsString, SourceFileParseOptions};
use ts_core::ScriptKind;
use ts_jsstring::SourceText;

fn parsed() -> ts_ast::ParsedFile {
    ts_parser::parse_source_file(
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
                    let local = b.new_binding_symbol(0, ts_ast::JsString::default());
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
    use ts_ast::{AstBuilder, FactoryMethods, SyntaxKind};

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
