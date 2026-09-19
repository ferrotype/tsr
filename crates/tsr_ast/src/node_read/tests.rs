use crate::{AstBuilder, AstFile, FactoryMethods, JsString, NodeId};
use ts_arena::{Counters, Error};
use ts_jsstring::SourceText;

fn fragment(counters: &Counters, bytes: &[u8]) -> (AstFile, NodeId, NodeId) {
    let mut builder = AstBuilder::new(SourceText::from_loaded_bytes(bytes), counters);
    let identifier = builder.new_identifier(JsString::from_bytes(bytes));
    let root = builder.new_expression_statement(Some(identifier));
    builder.node_mut(identifier).unwrap().set_parent(Some(root));
    let parsed = builder.complete(root).unwrap();
    let record = parsed.core_node_read(identifier).unwrap();
    assert_eq!(record.id(), identifier);
    assert_eq!(record.owner_id().arena(), identifier.arena());
    assert_eq!(record.source().as_bytes(), bytes);
    assert!(record.as_borrowed().is_some());
    drop(record);
    (parsed.publish_unbound(), root, identifier)
}

#[test]
fn imported_records_keep_their_physical_owner_source_and_edges() {
    let counters = Counters::new();
    let (first, first_root, first_name) = fragment(&counters, b"alpha");
    let (second, second_root, second_name) = fragment(&counters, b"beta");
    assert_eq!(first_root.slot(), second_root.slot());
    assert_eq!(first_name.slot(), second_name.slot());
    assert_ne!(first_root.arena(), second_root.arena());

    let mut importer = AstBuilder::new(
        SourceText::from_loaded_bytes(b"caller".as_slice()),
        &counters,
    );
    importer.retain_file(first);
    importer.retain_file(second);
    let root = importer.new_qualified_name(Some(first_root), Some(second_root));
    let caller = importer.complete(root).unwrap().publish_unbound();
    for (root, name, bytes) in [
        (first_root, first_name, b"alpha".as_slice()),
        (second_root, second_name, b"beta".as_slice()),
    ] {
        let record = caller.view().node(root).unwrap();
        assert_eq!(record.id(), root);
        assert_eq!(record.owner_id().arena(), root.arena());
        assert_eq!(record.source().as_bytes(), bytes);
        assert_eq!(record.expression(), Some(name));
        assert!(record.as_borrowed().is_some());
        let identifier = caller.view().node(name).unwrap();
        assert_eq!(identifier.id(), name);
        assert_eq!(identifier.owner_id(), record.owner_id());
        assert_eq!(identifier.source().as_bytes(), bytes);
        assert_eq!(identifier.as_identifier().unwrap().text(), bytes);
    }
    let record = caller.view().node(root).unwrap();
    assert_eq!(record.source().as_bytes(), b"caller");
    assert_eq!(record.owner_id().arena(), root.arena());
}

#[test]
fn core_reads_preserve_owner_before_slot_validation() {
    let counters = Counters::new();
    let foreign = AstBuilder::new(
        SourceText::from_loaded_bytes(b"foreign".as_slice()),
        &counters,
    );
    let mut builder = AstBuilder::new(
        SourceText::from_loaded_bytes(b"local".as_slice()),
        &counters,
    );
    let root = builder.new_identifier(JsString::from_bytes(b"local".as_slice()));
    let parsed = builder.complete(root).unwrap();
    let wrong_owner = NodeId::from_parts(foreign.view().0.id().arena(), u32::MAX).unwrap();
    let wrong_slot = NodeId::from_parts(root.arena(), u32::MAX).unwrap();
    assert!(matches!(
        parsed.core_node_read(wrong_owner),
        Err(Error::WrongOwner)
    ));
    assert!(matches!(
        parsed.core_node_read(wrong_slot),
        Err(Error::InvalidSlot)
    ));
    let file = parsed.publish_unbound();
    assert!(matches!(
        file.view().node(wrong_owner),
        Err(Error::WrongOwner)
    ));
    assert!(matches!(
        file.view().node(wrong_slot),
        Err(Error::InvalidSlot)
    ));
}

#[test]
fn transaction_and_retained_lazy_reads_keep_identity_through_owner_disposal() {
    let counters = Counters::new();
    let before = counters.snapshot();
    let retired;
    {
        let (dependency, parent, other_parent) = fragment(&counters, b"dependency");
        let dependency_id = dependency.view().node(parent).unwrap().owner_id();
        let mut importer = AstBuilder::new(
            SourceText::from_loaded_bytes(b"importer".as_slice()),
            &counters,
        );
        importer.retain_file(dependency);
        let root = importer.new_expression_statement(Some(parent));
        let caller = importer.complete(root).unwrap().publish_unbound();

        let mut failed = None;
        let result = caller.view().jsdoc(parent, |transaction| {
            let staged = transaction.new_identifier(JsString::from_bytes(b"failed".as_slice()));
            failed = Some(staged);
            let record = transaction.node(staged)?;
            assert_eq!(record.id(), staged);
            assert_eq!(record.owner_id(), dependency_id);
            assert_eq!(record.source().as_bytes(), b"dependency");
            Err(Error::InvalidGraph)
        });
        assert!(matches!(result, Err(Error::InvalidGraph)));
        let failed = failed.unwrap();
        assert!(matches!(
            caller.view().node(failed),
            Err(Error::InvalidSlot)
        ));

        let roots = caller
            .view()
            .jsdoc(parent, |transaction| {
                let core = transaction.node(parent)?;
                assert_eq!(core.id(), parent);
                assert_eq!(core.owner_id(), dependency_id);
                assert_eq!(core.source().as_bytes(), b"dependency");
                drop(core);
                let staged = transaction.new_identifier(JsString::from_bytes(b"lazy".as_slice()));
                transaction.node_mut(staged)?.set_parent(Some(parent));
                let record = transaction.node(staged)?;
                assert_eq!(record.id(), staged);
                assert_eq!(record.owner_id(), dependency_id);
                assert_ne!(record.id().arena(), dependency_id.arena());
                assert_eq!(record.source().as_bytes(), b"dependency");
                assert!(record.as_borrowed().is_some());
                Ok(vec![staged])
            })
            .unwrap();
        let lazy = roots[0];
        assert_ne!(failed, lazy);
        let record = caller.view().node(lazy).unwrap();
        assert_eq!(record.id(), lazy);
        assert_eq!(record.owner_id(), dependency_id);
        assert_eq!(record.source().as_bytes(), b"dependency");
        assert!(record.as_borrowed().is_none());
        drop(record);
        drop(roots);

        let retained = caller.retain_node(lazy).unwrap();
        caller
            .view()
            .jsdoc(other_parent, |_| {
                // This is the same physical owner's lazy publication lock. Reading
                // an already retained record must use its stable page, not reenter
                // the directory through the original raw ID.
                let record = retained.read();
                assert_eq!(record.id(), lazy);
                assert_eq!(record.source().as_bytes(), b"dependency");
                assert!(record.as_borrowed().is_some());
                Ok(Vec::new())
            })
            .unwrap();
        drop(caller);
        let record = retained.read();
        assert_eq!(record.id(), lazy);
        assert_eq!(record.owner_id(), dependency_id);
        assert_eq!(record.source().as_bytes(), b"dependency");
        assert_eq!(record.parent(), Some(parent));
        assert_eq!(record.as_identifier().unwrap().text(), b"lazy");
        // The retained handle already owns its lazy page. Its read borrows that
        // stable record instead of acquiring another page guard.
        assert!(record.as_borrowed().is_some());
        assert_eq!(
            retained
                .file()
                .view()
                .node(root)
                .unwrap()
                .source()
                .as_bytes(),
            b"importer"
        );
        retired = lazy;
    }
    // A read context borrows its retention root and does not add an owner cycle.
    assert_eq!(counters.snapshot(), before);
    let (replacement, _, _) = fragment(&counters, b"replacement");
    assert!(matches!(
        replacement.view().node(retired),
        Err(Error::WrongOwner)
    ));
}

#[test]
fn bound_overlays_keep_physical_source_separate_from_logical_source_metadata() {
    let counters = Counters::new();
    let mut builder = AstBuilder::new(
        SourceText::from_loaded_bytes(b"physical".as_slice()),
        &counters,
    );
    let child = builder.new_identifier(JsString::from_bytes(b"child".as_slice()));
    let source = builder.new_source_file(
        crate::SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/physical.ts".as_slice()),
            ..crate::SourceFileParseOptions::default()
        },
        SourceText::from_loaded_bytes(b"logical source text".as_slice()),
        None,
        None,
    );
    builder.node_mut(child).unwrap().set_parent(Some(source));
    let owner_id = builder.id();
    let file = builder.complete(source).unwrap().publish_unbound();
    let roots = file
        .view()
        .source_jsdoc(source, child, |transaction| {
            let lazy = transaction.new_identifier(JsString::from_bytes(b"lazy".as_slice()));
            transaction.node_mut(lazy)?.set_parent(Some(child));
            assert_eq!(transaction.node(lazy)?.source().as_bytes(), b"physical");
            Ok(vec![lazy])
        })
        .unwrap();
    let lazy = roots[0];
    file.bind_with(source, |binding| {
        binding
            .node_mut(child)?
            .set_flags(crate::node_flags::AMBIENT);
        binding
            .node_mut(lazy)?
            .set_flags(crate::node_flags::UNREACHABLE);
        Ok(())
    })
    .unwrap();
    let bound = file.bound_view(source).unwrap().unwrap();
    for (id, flags) in [
        (child, crate::node_flags::AMBIENT),
        (lazy, crate::node_flags::UNREACHABLE),
    ] {
        let record = bound.node(id).unwrap();
        assert_eq!(record.id(), id);
        assert_eq!(record.owner_id(), owner_id);
        assert_eq!(record.source().as_bytes(), b"physical");
        assert_eq!(record.flags(), flags);
        assert!(record.as_borrowed().is_some());
        assert_eq!(file.view().node(id).unwrap().flags(), 0);
    }
    assert_eq!(
        file.view().source_file(source).unwrap().text().as_bytes(),
        b"logical source text"
    );
}

#[test]
fn payload_views_preserve_shape_dispatch_lists_and_owned_text() {
    use crate::{NodeKind, SyntaxKind};
    let counters = Counters::new();
    let mut builder = AstBuilder::new(
        SourceText::from_loaded_bytes(b"source spelling differs".as_slice()),
        &counters,
    );
    // Identifier text can be decoded, synthetic or raw WTF-8; it need not be
    // recoverable from this node's source range.
    let bytes = b"decoded\xed\xa0\x80\xff";
    let name = builder.new_identifier(JsString::from_bytes(bytes.as_slice()));
    let token = builder.new_token(SyntaxKind::Identifier.into());
    let property = crate::Factory::new_node(
        &mut builder,
        NodeKind::from_raw(-1),
        crate::PropertyAccessExpressionData {
            expression: Some(name),
            question_dot_token: None,
            name: Some(name),
        }
        .into(),
    );
    let elements = builder.node_slice(vec![Some(name), None]).unwrap();
    let parameters = builder
        .new_list(ts_core::TextRange::new(0, 2), elements)
        .unwrap();
    let function = builder.new_function_declaration(
        None,
        None,
        Some(name),
        None,
        Some(parameters),
        None,
        None,
        None,
    );
    // Actual shape controls access even when factory callers use an open kind.
    let file = builder.complete(function).unwrap().publish_unbound();
    let view = file.view();
    let property_record = view.node(property).unwrap();
    let access = property_record.as_property_access_expression().unwrap();
    assert_eq!(access.expression(), Some(name));
    assert_eq!(access.name(), Some(name));
    assert_eq!(access.question_dot_token(), None);
    assert_eq!(access.node().id(), property);
    assert!(property_record.as_identifier().is_none());
    drop(property_record);
    let token_read = view.node(token).unwrap();
    assert!(token_read.as_token().is_some());
    assert!(token_read.as_identifier().is_none());
    let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = view.node_text(token);
    }))
    .expect_err("kind/shape mismatch must fail before returning text");
    let message = error
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| error.downcast_ref::<&str>().copied())
        .unwrap();
    assert_eq!(
        message,
        "interface conversion: ast.nodeData is *ast.Token, not *ast.Identifier"
    );
    let record = view.node(function).unwrap();
    let function_read = record.as_function_declaration().unwrap();
    assert_eq!(function_read.name(), Some(name));
    assert_eq!(function_read.parameters(), Some(parameters));
    assert_eq!(function_read.body(), None);
    let list = view.list(function_read.parameters().unwrap()).unwrap();
    assert_eq!(
        &view
            .node_slice(list.nodes())
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        &[Some(name), None]
    );
    let text = view.node_text(name).unwrap();
    assert_eq!(text.as_bytes(), bytes);
    let owned = text.into_js_string();
    // Ownership conversion keeps the current backing; compact source words
    // must likewise return an owning string only when explicitly requested.
    let name_read = view.node(name).unwrap();
    assert_eq!(
        owned.as_bytes().as_ptr(),
        name_read.as_identifier().unwrap().text().as_ptr()
    );
    drop(name_read);
    drop(record);
    drop(list);
    drop(token_read);
    drop(file);
    assert_eq!(owned.as_bytes(), bytes);
}
