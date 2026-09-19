use super::*;
use crate::{AstBuilder, Factory, FactoryMethods, NodeData, SyntaxKind, TokenData};
use ts_arena::Counters;
use ts_core::TextRange;

#[test]
fn physical_header_is_24_bytes_and_typed_pages_keep_addresses_during_growth() {
    assert_eq!(std::mem::size_of::<StoredNode>(), 24);
    let mut rows = RowPages::<u32>::default();
    rows.push(41);
    let first = std::ptr::from_ref(rows.get(0).unwrap());
    for value in 1..1027 {
        rows.push(value);
    }
    assert_eq!(std::ptr::from_ref(rows.get(0).unwrap()), first);
    assert_eq!(rows.get(0), Some(&41));
    assert_eq!(rows.get(1026), Some(&1026));
    assert!(rows.get(1027).is_none());
    assert!(rows.get_mut(1027).is_none());
    assert!(rows.get(u32::MAX).is_none());
    for ordinal in [15, 16, 17, 31, 32, 33] {
        assert_eq!(rows.get(ordinal), Some(&ordinal));
        *rows.get_mut(ordinal).unwrap() += 100;
        assert_eq!(rows.get(ordinal), Some(&(ordinal + 100)));
    }

    #[derive(Default)]
    struct RetainedRow(Option<std::rc::Rc<std::cell::Cell<usize>>>);
    impl Drop for RetainedRow {
        fn drop(&mut self) {
            if let Some(drops) = &self.0 {
                drops.set(drops.get() + 1);
            }
        }
    }
    let drops = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut retained = RowPages::default();
    for _ in 0..35 {
        retained.push(RetainedRow(Some(drops.clone())));
    }
    assert_eq!(drops.get(), 0);
    drop(retained);
    assert_eq!(drops.get(), 35);
    assert_eq!(std::rc::Rc::strong_count(&drops), 1);
}

#[test]
fn compact_reads_and_unrestricted_edits_keep_identity_text_and_independent_shape() {
    let source = SourceText::from_loaded_bytes(b"alpha beta".as_slice());
    let mut ast = AstBuilder::new(source.clone(), &Counters::new());
    let literal = ast.new_string_literal(source.slice(6..10).unwrap(), 0);
    ast.finish_node(literal, TextRange::new(0, 5), 0);
    let retained_literal = ast.node(literal).as_string_literal().unwrap().text_owned();
    assert_eq!(retained_literal.as_bytes(), b"beta");
    assert_eq!(
        retained_literal.as_bytes().as_ptr(),
        source.as_bytes()[6..].as_ptr()
    );
    let changed_literal = JsString::from_bytes(b"alpha".as_slice());
    let changed_pointer = changed_literal.as_bytes().as_ptr();
    {
        let mut node = ast.node_mut(literal).unwrap();
        *node.data_mut() = crate::StringLiteralData {
            text: changed_literal,
            token_flags: 0,
        }
        .into();
    }
    let retained_change = ast.node(literal).as_string_literal().unwrap().text_owned();
    assert_eq!(retained_change.as_bytes().as_ptr(), changed_pointer);
    let name = ast.new_identifier(JsString::from_bytes(b"alpha".as_slice()));
    ast.finish_node(name, TextRange::new(0, 5), 0);
    let runtime = crate::runtime_node_id(&ast.node(name));
    ast.set_node_range(name, TextRange::new(6, 10));
    assert_eq!(ast.node(name).as_identifier().unwrap().text(), b"alpha");
    {
        let mut node = ast.node_mut(name).unwrap();
        assert_eq!(crate::runtime_node_id(&*node), runtime);
        node.data_mut().clone_from(&NodeData::Token(TokenData {}));
    }
    let node = ast.node(name);
    assert_eq!(node.kind(), SyntaxKind::Identifier);
    assert!(node.as_token().is_some());
    assert!(node.as_identifier().is_none());
    assert_eq!(crate::runtime_node_id(&node), runtime);
    drop(node);
    let second = ast.new_identifier(JsString::from_bytes(b"beta".as_slice()));
    let second_runtime = {
        let node = ast.node_mut(second).unwrap();
        crate::runtime_node_id(&*node)
    };
    assert_ne!(second_runtime, 0);
    assert_ne!(runtime, second_runtime);
    assert_eq!(crate::runtime_node_id(&ast.node(second)), second_runtime);
    let file = ast.complete(name).unwrap().publish_unbound();
    assert_eq!(
        crate::runtime_node_id(&file.view().node(second).unwrap()),
        second_runtime
    );
    let published_literal = file
        .view()
        .node(literal)
        .unwrap()
        .as_string_literal()
        .unwrap()
        .text_owned();
    assert_eq!(published_literal.as_bytes().as_ptr(), changed_pointer);
    drop(file);
    drop(source);
    assert_eq!(retained_literal.as_bytes(), b"beta");
    assert_eq!(retained_change.as_bytes(), b"alpha");
    assert_eq!(published_literal.as_bytes(), b"alpha");
}

#[test]
fn compact_reference_words_preserve_full_slot_and_foreign_namespaces() {
    let owner = AstBuilder::new(SourceText::default(), &Counters::new());
    let foreign = AstBuilder::new(SourceText::default(), &Counters::new());
    let nodes = owner.id().arena();
    let auxiliary = owner.view().0.auxiliary_arena();
    let mut store = CoreStore::default();
    let source = SourceText::default();
    let ids = [
        None,
        Some(NodeId::from_parts(nodes, 7).unwrap()),
        Some(NodeId::from_parts(nodes, u32::MAX).unwrap()),
        Some(NodeId::from_parts(foreign.id().arena(), 7).unwrap()),
    ];
    let mut words = Vec::new();
    {
        let (_, mut context) = store.packing_parts(nodes, auxiliary, &source, -1);
        for (row, id) in ids.iter().enumerate() {
            words.push(context.encode_node(FieldKey::new(3, row as u32, 0), *id));
        }
    }
    assert_eq!(words, [0, 7, u32::MAX, u32::MAX]);
    let context = CompactContext {
        nodes,
        auxiliary,
        source: &source,
        store: &store,
    };
    for (row, word) in words.into_iter().enumerate() {
        assert_eq!(
            context.decode_node(FieldKey::new(3, row as u32, 0), word),
            ids[row]
        );
    }
}

#[test]
fn escape_overwrites_clear_only_the_changed_field_for_every_reference_namespace() {
    let counters = Counters::new();
    let owner = AstBuilder::new(SourceText::default(), &counters);
    let foreign = AstBuilder::new(SourceText::default(), &counters);
    let nodes = owner.id().arena();
    let auxiliary = owner.view().0.auxiliary_arena();
    let symbols = ts_arena::SymbolArena::<crate::Symbol>::new(&counters);
    let tables = crate::SymbolTables::new(&counters);
    let flows = crate::FlowNodes::new(&counters);
    let mut store = CoreStore::default();
    store.initialize_binding(binding::BindingArenas {
        symbols: symbols.id(),
        tables: tables.id(),
        flows: flows.id(),
    });
    let source = SourceText::default();
    let retained_key = FieldKey::parent(99);
    let retained_id = NodeId::from_parts(foreign.id().arena(), 99).unwrap();
    {
        let (_, mut context) = store.packing_parts(nodes, auxiliary, &source, -1);
        assert_eq!(
            context.encode_node(retained_key, Some(retained_id)),
            u32::MAX
        );
    }

    macro_rules! check_overwrite {
        ($encode:ident, $decode:ident, $id:ty, $arena:expr) => {{
            let key = FieldKey::new(3, 7, 0);
            let local = <$id>::from_parts($arena, 7).unwrap();
            for escaped in [
                <$id>::from_parts($arena, u32::MAX).unwrap(),
                <$id>::from_parts(foreign.id().arena(), 7).unwrap(),
            ] {
                for replacement in [Some(local), None] {
                    let word = {
                        let (_, mut context) = store.packing_parts(nodes, auxiliary, &source, -1);
                        assert_eq!(context.$encode(key, Some(escaped)), u32::MAX);
                        context.$encode(key, replacement)
                    };
                    let context = CompactContext {
                        nodes,
                        auxiliary,
                        source: &source,
                        store: &store,
                    };
                    assert_eq!(context.$decode(key, word), replacement);
                    assert_eq!(
                        context.decode_node(retained_key, u32::MAX),
                        Some(retained_id)
                    );
                    assert!(!store.links.contains_key(&key));
                    assert_eq!(store.links.len(), 1);
                }
            }
        }};
    }
    check_overwrite!(encode_node, decode_node, NodeId, nodes);
    check_overwrite!(encode_aux, decode_aux, AuxId, auxiliary);
    check_overwrite!(encode_symbol, decode_symbol, crate::SymbolId, symbols.id());
    check_overwrite!(
        encode_symbol_table,
        decode_symbol_table,
        crate::SymbolTableId,
        tables.id()
    );
    check_overwrite!(encode_flow, decode_flow, crate::FlowId, flows.id());

    {
        let (_, mut context) = store.packing_parts(nodes, auxiliary, &source, -1);
        assert_eq!(context.encode_node(retained_key, None), 0);
    }
    assert!(store.links.is_empty());
    assert_ne!(store.links.capacity(), 0);
    // Reuse an allocated but empty escape directory with ordinary values.
    let (_, mut context) = store.packing_parts(nodes, auxiliary, &source, -1);
    assert_eq!(
        context.encode_node(retained_key, Some(NodeId::from_parts(nodes, 7).unwrap())),
        7
    );
    assert_eq!(context.encode_node(retained_key, None), 0);
}

#[test]
fn stored_validation_preserves_all_reference_kinds_order_and_first_error() {
    use std::cell::RefCell;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Visit {
        Node(NodeId),
        List(NodeListId),
        Raw(NodeSlice),
        Text(TextSlice),
    }

    let counters = Counters::new();
    let owner = AstBuilder::new(SourceText::default(), &counters);
    let foreign = AstBuilder::new(SourceText::default(), &counters);
    let nodes = owner.id().arena();
    let auxiliary = owner.view().0.auxiliary_arena();
    let left = NodeId::from_parts(nodes, 1).unwrap();
    let typ = NodeId::from_parts(foreign.id().arena(), u32::MAX).unwrap();
    let operator = NodeId::from_parts(nodes, 3).unwrap();
    let right = NodeId::from_parts(nodes, 4).unwrap();
    let modifiers = NodeListId(AuxId::from_parts(auxiliary, 1).unwrap());
    let raw = NodeSlice {
        backing: Some(AuxId::from_parts(auxiliary, u32::MAX).unwrap()),
        start: 2,
        len: 3,
    };
    let text = TextSlice {
        backing: Some(AuxId::from_parts(foreign.view().0.auxiliary_arena(), 5).unwrap()),
        start: 1,
        len: 2,
    };
    // BinaryExpression.type is deliberately outside child traversal. Completion
    // still validates it in schema field order, before operator_token and right.
    let cases = [
        (
            NodeData::BinaryExpression(Box::new(crate::BinaryExpressionData {
                modifiers: Some(modifiers),
                left: Some(left),
                r#type: Some(typ),
                operator_token: Some(operator),
                right: Some(right),
            })),
            vec![
                Visit::List(modifiers),
                Visit::Node(left),
                Visit::Node(typ),
                Visit::Node(operator),
                Visit::Node(right),
            ],
        ),
        (
            NodeData::BinaryExpression(Box::new(crate::BinaryExpressionData {
                modifiers: None,
                left: None,
                r#type: Some(typ),
                operator_token: None,
                right: Some(right),
            })),
            vec![Visit::Node(typ), Visit::Node(right)],
        ),
        (
            NodeData::SyntaxList(crate::SyntaxListData { children: raw }),
            vec![Visit::Raw(raw)],
        ),
        (
            NodeData::JSDocLink(crate::JSDocLinkData {
                text,
                name: Some(left),
            }),
            vec![Visit::Text(text), Visit::Node(left)],
        ),
        (
            NodeData::SyntaxList(crate::SyntaxListData {
                children: NodeSlice::default(),
            }),
            vec![Visit::Raw(NodeSlice::default())],
        ),
        (
            NodeData::JSDocLink(crate::JSDocLinkData {
                text: TextSlice::default(),
                name: None,
            }),
            vec![Visit::Text(TextSlice::default())],
        ),
    ];
    let source = SourceText::default();
    let mut store = CoreStore::default();
    for (data, expected) in cases {
        let (shape, ordinal) = {
            let (payloads, mut context) = store.packing_parts(nodes, auxiliary, &source, -1);
            payloads.insert(data, &mut context)
        };
        let header = StoredNode {
            // Neither unrelated kind nor the binding-presence bit selects rows.
            kind: SyntaxKind::Unknown.into(),
            shape: shape | 0x8000,
            flags: 0,
            pos: -1,
            end: -1,
            parent: 0,
            ordinal,
        };
        let context = CompactContext {
            nodes,
            auxiliary,
            source: &source,
            store: &store,
        };
        for stop in 1..=expected.len() + 1 {
            for direct in [false, true] {
                let visits = RefCell::new(Vec::new());
                let observe = |visit| {
                    let mut visits = visits.borrow_mut();
                    visits.push(visit);
                    if visits.len() == stop {
                        Err(stop)
                    } else {
                        Ok(())
                    }
                };
                let result = if direct {
                    store.payloads.validate_references(
                        &header,
                        context,
                        |id| observe(Visit::Node(id)),
                        |id| observe(Visit::List(id)),
                        |slice| observe(Visit::Raw(slice)),
                        |slice| observe(Visit::Text(slice)),
                    )
                } else {
                    store
                        .payloads
                        .read(shape, ordinal, context, -1)
                        .validate_references(
                            |id| observe(Visit::Node(id)),
                            |id| observe(Visit::List(id)),
                            |slice| observe(Visit::Raw(slice)),
                            |slice| observe(Visit::Text(slice)),
                        )
                };
                assert_eq!(
                    visits.into_inner(),
                    expected[..stop.min(expected.len())],
                    "shape {shape}, direct {direct}, stop {stop}"
                );
                assert_eq!(
                    result,
                    if stop <= expected.len() {
                        Err(stop)
                    } else {
                        Ok(())
                    }
                );
            }
        }
    }
}
