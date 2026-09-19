use crate::*;
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier, Mutex,
    },
};
use ts_arena::{Counters, Error};
use ts_core::TextRange;
use ts_jsstring::SourceText;

fn builder(counters: &Counters) -> AstBuilder {
    AstBuilder::new(
        SourceText::from_loaded_bytes(&b"x\n\xf0\x9f\x98\x80"[..]),
        counters,
    )
}

#[test]
fn unrestricted_record_replacement_starts_with_the_replacements_runtime_identity() {
    let mut build = builder(&Counters::new());
    let id = build.new_identifier(JsString::from_bytes(b"first".as_slice()));
    let original = runtime_node_id(&build.node(id));
    let replacement = Node::new(
        SyntaxKind::Identifier,
        -1,
        -1,
        IdentifierData {
            text: JsString::from_bytes(b"replacement".as_slice()),
        }
        .into(),
    )
    .unwrap();
    assert_eq!(existing_runtime_node_id(&replacement), 0);
    *build.node_mut(id).unwrap() = replacement;
    assert_eq!(existing_runtime_node_id(&build.node(id)), 0);
    assert_ne!(runtime_node_id(&build.node(id)), original);
    assert_eq!(
        build.node(id).as_identifier().unwrap().text(),
        b"replacement"
    );
}

#[test]
fn node_slice_values_preserve_nil_bounds_and_bidirectional_iteration() {
    let counters = Counters::new();
    let mut build = builder(&counters);
    let first = build.new_identifier(JsString::from_bytes(b"first".as_slice()));
    let last = build.new_identifier(JsString::from_bytes(b"last".as_slice()));
    let backing = build
        .node_slice(vec![Some(first), None, Some(last)])
        .unwrap();
    let read = build.view().node_slice(backing).unwrap();
    assert_eq!(read.get(1), Some(None));
    assert_eq!(read.get(3), None);
    assert_eq!(read.get(usize::MAX), None);
    assert_eq!(read.first(), Some(Some(first)));
    assert_eq!(read.last(), Some(Some(last)));
    let mut values = read.iter();
    assert_eq!(values.len(), 3);
    assert_eq!(values.next_back(), Some(Some(last)));
    assert_eq!(values.len(), 2);
    assert_eq!(values.next(), Some(Some(first)));
    assert_eq!(values.next_back(), Some(None));
    assert_eq!(values.len(), 0);
    assert_eq!(values.next(), None);
    assert_eq!(values.next_back(), None);
    let nil_only = build
        .view()
        .node_slice(backing.slice(1..2).unwrap())
        .unwrap();
    assert_eq!(nil_only.at(0), None);
    assert_eq!(nil_only.iter().collect::<Vec<_>>(), vec![None]);
    let panic = catch_unwind(AssertUnwindSafe(|| read.at(3))).unwrap_err();
    assert_eq!(
        panic.downcast_ref::<String>().unwrap(),
        "index out of bounds: the len is 3 but the index is 3"
    );
    let empty = build.view().node_slice(NodeSlice::empty()).unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.first(), None);
    assert_eq!(empty.last(), None);
    assert_eq!(empty.iter().len(), 0);
}

#[test]
fn borrowed_node_slices_copy_nil_edges_and_keep_allocated_empty_identity() {
    let mut build = builder(&Counters::new());
    let first = build.new_token(SyntaxKind::Unknown.into());
    let last = build.new_token(SyntaxKind::EndOfFile.into());
    let mut input = [Some(first), None, Some(last)];
    let borrowed = build.node_slice_from_slice(&input).unwrap();
    let consumed = build.node_slice(input.to_vec()).unwrap();
    input[0] = Some(last);
    for slice in [borrowed, consumed] {
        assert_eq!(
            build
                .view()
                .node_slice(slice)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(first), None, Some(last)]
        );
    }
    assert_eq!(input[0], Some(last));
    let first_empty = build.node_slice_from_slice(&[]).unwrap();
    let second_empty = build.node_slice_from_slice(&[]).unwrap();
    for slice in [first_empty, second_empty] {
        assert!(slice.is_empty());
        assert!(!slice.is_nil());
        assert!(slice.backing_id().is_some());
        assert!(build.view().node_slice(slice).unwrap().is_empty());
    }
    assert_ne!(first_empty.backing_id(), second_empty.backing_id());
    assert!(NodeSlice::empty().is_nil());
}

#[test]
fn borrowed_node_slice_rejection_preserves_error_order_and_inserts_nothing() {
    let mut build = builder(&Counters::new());
    let local = build.new_token(SyntaxKind::Unknown.into());
    let missing = NodeId::from_parts(local.arena(), u32::MAX).unwrap();
    let mut foreign = builder(&Counters::new());
    let foreign_node = foreign.new_token(SyntaxKind::Unknown.into());
    let foreign_missing = NodeId::from_parts(foreign_node.arena(), u32::MAX).unwrap();
    let prefix = build.node_slice_from_slice(&[Some(local), None]).unwrap();
    let count_before = build.storage.view().core_auxiliary().count();
    for (nodes, error) in [
        (
            [Some(local), Some(missing), Some(foreign_node)],
            Error::InvalidSlot,
        ),
        (
            [Some(local), Some(foreign_node), Some(missing)],
            Error::WrongOwner,
        ),
        (
            [Some(local), None, Some(foreign_missing)],
            Error::WrongOwner,
        ),
    ] {
        assert_eq!(build.node_slice_from_slice(&nodes), Err(error));
        assert_eq!(build.storage.view().core_auxiliary().count(), count_before);
    }
    let next = build.node_slice_from_slice(&[Some(local)]).unwrap();
    assert_eq!(
        next.backing_id().unwrap().slot(),
        prefix.backing_id().unwrap().slot() + 1
    );
    let crate::auxiliary::AuxValue::CompactNodes(backing) = build
        .view()
        .auxiliary(next.backing_id().unwrap())
        .unwrap()
        .value()
    else {
        panic!("borrowed slice uses compact edge storage");
    };
    assert_eq!(backing.start, 2, "failed inputs must not append edge words");
    assert_eq!(build.view().node_slice(prefix).unwrap().at(0), Some(local));
    assert_eq!(build.view().node_slice(prefix).unwrap().at(1), None);
}

#[test]
fn compact_syntax_backings_span_pages_and_keep_imported_and_lazy_context() {
    let counters = Counters::new();
    let before = counters.snapshot();
    {
        let mut dependency = builder(&counters);
        let child = dependency.new_identifier(JsString::from_bytes(b"dependency".as_slice()));
        let prefix = dependency.node_slice(vec![Some(child); 255]).unwrap();
        let crossing = dependency
            .node_slice(vec![None, Some(child), None])
            .unwrap();
        let empty = dependency.node_slice(Vec::new()).unwrap();
        assert!(!empty.is_nil());
        assert!(matches!(
            dependency
                .view()
                .auxiliary(crossing.backing.unwrap())
                .unwrap()
                .value(),
            crate::auxiliary::AuxValue::CompactNodes(_)
        ));
        let lazy_root = dependency.new_identifier(JsString::from_bytes(b"lazy parent".as_slice()));
        let dependency = dependency.complete(child).unwrap().publish_unbound();
        let lazy = dependency
            .view()
            .jsdoc(lazy_root, |transaction| {
                // This reads compact core pages under the lazy publication lock.
                assert_eq!(
                    transaction
                        .node_slice_read(crossing)?
                        .iter()
                        .collect::<Vec<_>>(),
                    vec![None, Some(child), None]
                );
                let nodes = transaction.node_slice(vec![Some(child), None])?;
                let list = transaction.new_list(TextRange::new(-1, -1), nodes)?;
                let root = transaction.new_array_literal_expression(Some(list), false);
                transaction.node_mut(root)?.set_parent(Some(lazy_root));
                assert!(matches!(
                    transaction.storage.aux(nodes.backing.unwrap())?,
                    ts_arena::AuxiliaryRead::Lazy(record)
                        if matches!(&*record, AstStorageData::Nodes(_))
                ));
                Ok(vec![root])
            })
            .unwrap()[0];
        let mut importer = builder(&counters);
        let own_child = importer.new_identifier(JsString::from_bytes(b"importer".as_slice()));
        assert_eq!(own_child.slot(), child.slot());
        assert_ne!(own_child.arena(), child.arena());
        importer.retain_file(dependency);
        // A foreign core edge and a retained lazy edge both use full-ID escapes.
        let mixed = importer
            .node_slice(vec![Some(own_child), Some(child), Some(lazy)])
            .unwrap();
        let file = importer.complete(own_child).unwrap().publish_unbound();
        assert_eq!(
            file.view()
                .node_slice(mixed)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(own_child), Some(child), Some(lazy)]
        );
        assert_eq!(file.view().node_slice(prefix).unwrap().len(), 255);
        let read = file.view().node_slice(crossing).unwrap();
        let mut iter = read.iter();
        assert_eq!(iter.nth(1), Some(Some(child)));
        assert_eq!(iter.next_back(), Some(None));
        assert_eq!(iter.len(), 0);
        assert_eq!(
            read.iter().rev().collect::<Vec<_>>(),
            vec![None, Some(child), None]
        );
        assert!(file.view().node_slice(empty).unwrap().is_empty());
        assert!(file
            .view()
            .node_slice(NodeSlice {
                backing: crossing.backing,
                start: 2,
                len: 2,
            })
            .is_err());
        let lazy_list = file.view().node(lazy).unwrap().element_list().unwrap();
        let lazy_nodes = file.view().list(lazy_list).unwrap().nodes();
        assert_eq!(
            file.view()
                .node_slice(lazy_nodes)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![Some(child), None]
        );
    }
    assert_eq!(counters.snapshot(), before);
}
fn array(factory: &mut impl Factory, list: NodeListId) -> NodeId {
    factory.new_node(
        SyntaxKind::ArrayLiteralExpression.into(),
        ArrayLiteralExpressionData {
            elements: Some(list),
            multi_line: false,
        }
        .into(),
    )
}

#[test]
fn storage_list_identity_is_independent_of_edges_empty_and_missing_state() {
    let mut build = builder(&Counters::new());
    let child = build.new_identifier(JsString::from_bytes(&b"x"[..]));
    let nodes = build.node_slice(vec![Some(child), None]).unwrap();
    let equal_values = build.node_slice(vec![Some(child), None]).unwrap();
    assert!(!nodes.same(equal_values));
    assert!(nodes.slice(0..1).unwrap().same(nodes.slice(0..1).unwrap()));
    assert!(!nodes.slice(0..1).unwrap().same(nodes.slice(1..2).unwrap()));
    assert!(nodes.slice(1..1).unwrap().same(NodeSlice::empty()));
    assert!(nodes.slice(1..3).is_err());
    let first = build.new_list(TextRange::new(4, 9), nodes).unwrap();
    build
        .list_mut(first)
        .unwrap()
        .set_modifier_flags(0x8000_0001);
    let cloned = build.clone_list(first).unwrap();
    assert_ne!(first, cloned);
    assert_eq!(
        build.view().list(first).unwrap().loc(),
        TextRange::new(4, 9)
    );
    assert!(build
        .view()
        .list(first)
        .unwrap()
        .nodes()
        .same(build.view().list(cloned).unwrap().nodes()));
    assert_eq!(
        build.view().list(cloned).unwrap().modifier_flags(),
        0x8000_0001
    );
    let empty_a = build
        .new_list(TextRange::new(-1, -1), NodeSlice::empty())
        .unwrap();
    let empty_b = build
        .new_list(TextRange::new(-1, -1), NodeSlice::empty())
        .unwrap();
    assert_ne!(empty_a, empty_b);
    build.mark_list_missing(empty_b).unwrap();
    assert!(!build.view().list(empty_a).unwrap().is_missing());
    assert!(build.view().list(empty_b).unwrap().is_missing());
    assert_eq!(
        &build
            .view()
            .node_slice(nodes)
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        &[Some(child), None]
    );
}

#[test]
fn storage_text_slice_identity_preserves_raw_bytes_and_rejects_foreign_backing() {
    let counters = Counters::new();
    let mut build = builder(&counters);
    let text = build
        .text_slice(vec![JsString::from_bytes(&b"\xed\xa0\x80\xff"[..])])
        .unwrap();
    let other = build
        .text_slice(vec![JsString::from_bytes(&b"\xed\xa0\x80\xff"[..])])
        .unwrap();
    assert!(text.same(text.slice(0..1).unwrap()));
    assert!(!text.same(other));
    assert_eq!(
        build.view().text_slice(text).unwrap()[0].as_bytes(),
        b"\xed\xa0\x80\xff"
    );
    let empty = build.text_slice(Vec::new()).unwrap();
    assert!(!empty.is_nil());
    assert!(empty.same(TextSlice::empty()));
    let mut foreign = builder(&counters);
    let foreign_text = foreign.text_slice(Vec::new()).unwrap();
    assert!(matches!(
        build.view().text_slice(foreign_text),
        Err(Error::WrongOwner)
    ));
    let original = build.new_js_doc_link(None, text);
    assert_eq!(build.update_js_doc_link(original, None, text), original);
    let cloned = build.clone_js_doc_link(original);
    assert_ne!(cloned, original);
    assert!(build
        .view()
        .node(cloned)
        .unwrap()
        .data_source()
        .as_js_doc_link()
        .unwrap()
        .text()
        .same(text));
}

#[test]
fn storage_exclusive_file_moves_to_worker_and_publication_keeps_lazy_identity() {
    let counters = Counters::new();
    let mut build = builder(&counters);
    let root = build.new_token(SyntaxKind::EndOfFile.into());
    let mut parsed = build.complete(root).unwrap();
    let original_count = parsed.view().file_info().node_count;
    let docs = parsed
        .view()
        .jsdoc(root, |transaction| {
            let text = transaction.text_slice(vec![JsString::from_bytes(&b"comment"[..])])?;
            let doc = transaction.new_js_doc_text(text);
            transaction.node_mut(doc)?.set_parent(Some(root));
            Ok(vec![doc])
        })
        .unwrap();
    let doc = docs[0];
    assert_eq!(parsed.view().file_info().node_count, original_count);
    parsed
        .builder_mut()
        .node_mut(root)
        .unwrap()
        .set_flags(0x4000);
    let (parsed, position) = std::thread::spawn(move || {
        let position = parsed.view().position_map().utf8_to_utf16(6);
        (parsed, position)
    })
    .join()
    .unwrap();
    assert_eq!(position, 4);
    let file = parsed.publish_unbound();
    assert_eq!(file.root(), Some(root));
    assert_eq!(file.view().node(root).unwrap().flags(), 0x4000);
    assert_eq!(file.view().node(doc).unwrap().parent(), Some(root));
    let cached = file
        .view()
        .jsdoc(root, |_| panic!("publication replaced lazy cache"))
        .unwrap();
    assert_eq!(&*cached, &[doc]);
}

#[test]
fn storage_eager_jsdoc_is_core_only_and_survives_publication_without_initializer() {
    let mut build = builder(&Counters::new());
    let parent = build.new_token(SyntaxKind::EndOfFile.into());
    let doc = build.new_token(SyntaxKind::Unknown.into());
    build.seed_jsdoc(parent, vec![doc]).unwrap();
    assert_eq!(build.seed_jsdoc(parent, vec![]), Err(Error::InvalidGraph));
    assert_eq!(&*build.view().eager_jsdoc(parent).unwrap().unwrap(), &[doc]);
    let file = build.complete(parent).unwrap().publish_unbound();
    assert_eq!(
        &*file
            .view()
            .jsdoc(parent, |_| panic!("eager cache missed"))
            .unwrap(),
        &[doc]
    );
}

#[test]
fn storage_imported_cache_operations_use_the_parents_owner() {
    let counters = Counters::new();
    let mut dependency = builder(&counters);
    let eager_parent = dependency.new_token(SyntaxKind::EndOfFile.into());
    let eager_doc = dependency.new_token(SyntaxKind::Unknown.into());
    dependency
        .seed_jsdoc(eager_parent, vec![eager_doc])
        .unwrap();
    let lazy_parent = dependency.new_token(SyntaxKind::Identifier.into());
    let dependency = dependency.complete(eager_parent).unwrap().publish_unbound();
    let mut importing = builder(&counters);
    importing.retain_file(dependency.clone());
    let root = importing.new_token(SyntaxKind::EndOfFile.into());
    let importing = importing.complete(root).unwrap().publish_unbound();
    assert_eq!(
        &*importing.view().eager_jsdoc(eager_parent).unwrap().unwrap(),
        &[eager_doc]
    );
    assert_eq!(
        &*importing
            .view()
            .jsdoc(eager_parent, |_| panic!("import lost eager cache"))
            .unwrap(),
        &[eager_doc]
    );
    let roots = importing
        .view()
        .jsdoc(lazy_parent, |transaction| {
            let doc = transaction.new_js_doc_text(TextSlice::empty());
            transaction.node_mut(doc)?.set_parent(Some(lazy_parent));
            Ok(vec![doc])
        })
        .unwrap();
    assert_eq!(
        &*dependency
            .view()
            .jsdoc(lazy_parent, |_| panic!(
                "import filled another owner's cache"
            ))
            .unwrap(),
        &*roots
    );
    let escaped = importing.retain_node(roots[0]).unwrap();
    drop(dependency);
    drop(importing);
    assert_eq!(escaped.read().parent(), Some(lazy_parent));
}

#[test]
fn storage_logical_source_jsdoc_caches_are_independent_after_clone() {
    let mut build = builder(&Counters::new());
    let parent = build.new_token(SyntaxKind::EndOfFile.into());
    build
        .node_mut(parent)
        .unwrap()
        .set_flags(node_flags::HAS_JS_DOC);
    let source = build.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(&b"/a.ts"[..]),
            path: JsString::from_bytes(&b"/a.ts"[..]),
            ..SourceFileParseOptions::default()
        },
        SourceText::from_loaded_bytes(&b""[..]),
        None,
        Some(parent),
    );
    let cloned = build.clone_source_file(source);
    let doc = build.new_js_doc_text(TextSlice::empty());
    build.seed_source_jsdoc(source, parent, vec![doc]).unwrap();
    assert!(build
        .view()
        .source_eager_jsdoc(cloned, parent)
        .unwrap()
        .is_none());
    let mut provider = EagerJsDocProvider::default();
    assert_eq!(
        &*provider.jsdoc(build.view(), source, parent).unwrap(),
        &[doc]
    );
    assert!(provider
        .jsdoc(build.view(), cloned, parent)
        .unwrap()
        .is_empty());
    let cloned_docs = build
        .view()
        .source_jsdoc(cloned, parent, |transaction| {
            Ok(vec![transaction.new_js_doc_text(TextSlice::empty())])
        })
        .unwrap();
    assert_ne!(cloned_docs[0], doc);
    let file = build.complete(source).unwrap().publish_unbound();
    assert_eq!(
        &*file
            .view()
            .source_jsdoc(source, parent, |_| panic!("source cache lost"))
            .unwrap(),
        &[doc]
    );
    assert_eq!(
        &*file
            .view()
            .source_jsdoc(cloned, parent, |_| panic!("clone cache lost"))
            .unwrap(),
        &*cloned_docs
    );
}

#[test]
fn storage_retained_node_keeps_list_text_frame_and_source_alive() {
    let counters = Counters::new();
    let before = counters.snapshot();
    let mut build = builder(&counters);
    let text = build
        .text_slice(vec![JsString::from_bytes(&b"saved"[..])])
        .unwrap();
    let child = build.new_js_doc_text(text);
    let nodes = build.node_slice(vec![Some(child), None]).unwrap();
    let list = build.new_list(TextRange::new(0, 6), nodes).unwrap();
    let root = array(&mut build, list);
    build.node_mut(child).unwrap().set_parent(Some(root));
    let file = build.complete(root).unwrap().publish_unbound();
    let escaped = file.retain_node(child).unwrap();
    drop(file);
    let owner = escaped.file();
    assert_eq!(owner.view().file_info().root, Some(root));
    assert_eq!(owner.view().source().as_bytes(), b"x\n\xf0\x9f\x98\x80");
    assert_eq!(
        &owner
            .view()
            .node_slice(owner.view().list(list).unwrap().nodes())
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        &[Some(child), None]
    );
    assert_eq!(
        owner.view().text_slice(text).unwrap()[0].as_bytes(),
        b"saved"
    );
    drop(owner);
    assert!(counters.snapshot().owners > before.owners);
    drop(escaped);
    assert_eq!(counters.snapshot(), before);
}

#[test]
fn storage_failed_lazy_graph_burns_node_list_and_text_identities() {
    let counters = Counters::new();
    let mut build = builder(&counters);
    let root = build.new_token(SyntaxKind::EndOfFile.into());
    let file = build.complete(root).unwrap().publish_unbound();
    let mut failed = None;
    let bytes: Arc<[u8]> = Arc::from(&b"dropped"[..]);
    let weak = Arc::downgrade(&bytes);
    let result = file.view().jsdoc(root, |transaction| {
        let text = transaction.text_slice(vec![JsString::from_bytes(bytes)])?;
        let child = transaction.new_js_doc_text(text);
        let nodes = transaction.node_slice(vec![Some(child)])?;
        let list = transaction.new_list(TextRange::new(0, 1), nodes)?;
        let doc = array(transaction, list);
        failed = Some((doc, list, text));
        Err(Error::InvalidGraph)
    });
    assert!(matches!(result, Err(Error::InvalidGraph)));
    assert!(weak.upgrade().is_none());
    let (node, list, text) = failed.unwrap();
    assert!(matches!(file.view().node(node), Err(Error::InvalidSlot)));
    assert!(matches!(file.view().list(list), Err(Error::InvalidSlot)));
    assert!(matches!(
        file.view().text_slice(text),
        Err(Error::InvalidSlot)
    ));
    let retry = file
        .view()
        .jsdoc(root, |transaction| {
            Ok(vec![transaction.new_token(SyntaxKind::Unknown.into())])
        })
        .unwrap();
    assert_ne!(retry[0], node);
    assert!(matches!(file.view().list(list), Err(Error::InvalidSlot)));
}

#[test]
fn storage_transaction_resolves_core_previous_and_staged_data_without_reentry() {
    let mut build = builder(&Counters::new());
    let parent_a = build.new_token(SyntaxKind::EndOfFile.into());
    let parent_b = build.new_token(SyntaxKind::Unknown.into());
    let core_nodes = build.node_slice(vec![Some(parent_b)]).unwrap();
    let core_list = build.new_list(TextRange::new(0, 1), core_nodes).unwrap();
    let file = build.complete(parent_a).unwrap().publish_unbound();
    let old = file
        .view()
        .jsdoc(parent_a, |transaction| {
            Ok(vec![transaction.new_token(SyntaxKind::Unknown.into())])
        })
        .unwrap()[0];
    let docs = file
        .view()
        .jsdoc(parent_b, |transaction| {
            assert_eq!(transaction.node(old)?.kind(), SyntaxKind::Unknown);
            assert_eq!(transaction.node(parent_a)?.kind(), SyntaxKind::EndOfFile);
            assert!(matches!(
                transaction.node_mut(parent_a),
                Err(Error::WrongOwner)
            ));
            let missing_core = NodeId::from_parts(parent_a.arena(), u32::MAX).unwrap();
            assert!(matches!(
                transaction.node_mut(missing_core),
                Err(Error::WrongOwner)
            ));
            assert!(matches!(transaction.node_mut(old), Err(Error::InvalidSlot)));
            assert_eq!(transaction.list(core_list)?.loc(), TextRange::new(0, 1));
            let nodes = transaction.node_slice(vec![Some(old), Some(parent_a), None])?;
            let list = transaction.new_list(TextRange::new(1, 4), nodes)?;
            assert_eq!(
                &transaction
                    .node_slice_read(transaction.list(list)?.nodes())?
                    .iter()
                    .collect::<Vec<_>>(),
                &[Some(old), Some(parent_a), None]
            );
            Ok(vec![array(transaction, list)])
        })
        .unwrap();
    assert_eq!(
        file.view().node(docs[0]).unwrap().kind(),
        SyntaxKind::ArrayLiteralExpression
    );
}

#[test]
fn storage_reentry_panic_rolls_back_auxiliary_storage_and_allows_retry() {
    let mut build = builder(&Counters::new());
    let root = build.new_token(SyntaxKind::EndOfFile.into());
    let file = build.complete(root).unwrap().publish_unbound();
    let mut failed_list = None;
    let panic = catch_unwind(AssertUnwindSafe(|| {
        file.view().jsdoc(root, |transaction| {
            let list = transaction.new_list(TextRange::new(0, 0), NodeSlice::empty())?;
            failed_list = Some(list);
            file.view().eager_jsdoc(root)?;
            Ok(vec![array(transaction, list)])
        })
    }));
    let payload = panic.unwrap_err();
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap();
    assert!(message.contains("lazy initializer reentered its file's lazy storage"));
    assert!(matches!(
        file.view().list(failed_list.unwrap()),
        Err(Error::InvalidSlot)
    ));
    assert!(file
        .view()
        .jsdoc(root, |_| Ok(Vec::new()))
        .unwrap()
        .is_empty());
}

#[test]
fn storage_lazy_first_use_publishes_one_graph_with_real_list_payloads() {
    let mut build = builder(&Counters::new());
    let root = build.new_token(SyntaxKind::EndOfFile.into());
    let file = build.complete(root).unwrap().publish_unbound();
    let barrier = Arc::new(Barrier::new(6));
    let calls = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..6)
        .map(|_| {
            let file = file.clone();
            let barrier = barrier.clone();
            let calls = calls.clone();
            std::thread::spawn(move || {
                barrier.wait();
                file.view()
                    .jsdoc(root, |transaction| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        let child = transaction.new_token(SyntaxKind::Unknown.into());
                        let nodes = transaction.node_slice(vec![Some(child), None])?;
                        let list = transaction.new_list(TextRange::new(0, 2), nodes)?;
                        Ok(vec![array(transaction, list)])
                    })
                    .unwrap()[0]
            })
        })
        .collect();
    let ids: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(ids.iter().all(|&id| id == ids[0]));
    let list = file
        .view()
        .node(ids[0])
        .unwrap()
        .data_source()
        .as_array_literal_expression()
        .unwrap()
        .elements()
        .unwrap();
    assert_eq!(
        file.view()
            .node_slice(file.view().list(list).unwrap().nodes())
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn storage_hooks_can_reenter_and_mutate_original_without_borrow_aliasing() {
    #[derive(Default)]
    struct Hooks {
        calls: Mutex<Vec<&'static str>>,
        recursive: AtomicUsize,
    }
    impl FactoryHooks for Hooks {
        fn on_create(&self, factory: &mut dyn Factory, node: NodeId) {
            self.calls.lock().unwrap().push("create");
            factory.node_mut(node).set_flags(11);
            if self.recursive.fetch_add(1, Ordering::SeqCst) == 0 {
                factory.new_node(SyntaxKind::Unknown.into(), TokenData {}.into());
            }
        }
        fn on_update(&self, factory: &mut dyn Factory, node: NodeId, original: NodeId) {
            self.calls.lock().unwrap().push("update");
            assert_eq!(factory.node(node).flags(), factory.node(original).flags());
            factory.node_mut(original).set_flags(19);
        }
        fn on_clone(&self, factory: &mut dyn Factory, _: NodeId, original: NodeId) {
            self.calls.lock().unwrap().push("clone");
            assert_eq!(factory.node(original).flags(), 19);
        }
    }
    let hooks = Arc::new(Hooks::default());
    let mut build = AstBuilder::with_hooks(
        SourceText::from_loaded_bytes(&b""[..]),
        &Counters::new(),
        hooks.clone(),
    );
    let first = build.new_token(SyntaxKind::Unknown.into());
    assert_eq!(build.node_count(), 2);
    let cloned = build.clone_token(first);
    assert_ne!(first, cloned);
    assert_eq!(build.view().node(first).unwrap().flags(), 19);
    assert_eq!(
        &*hooks.calls.lock().unwrap(),
        &["create", "create", "create", "update", "clone"]
    );
    let before = hooks.calls.lock().unwrap().len();
    assert_eq!(build.finish_clone(first, first), first);
    assert_eq!(hooks.calls.lock().unwrap().len(), before);
}

#[test]
fn storage_hook_unwind_discards_the_exclusive_owner() {
    struct PanicHook;
    impl FactoryHooks for PanicHook {
        fn on_create(&self, _: &mut dyn Factory, _: NodeId) {
            panic!("hook failure");
        }
    }
    let counters = Counters::new();
    let before = counters.snapshot();
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let mut build = AstBuilder::with_hooks(
            SourceText::from_loaded_bytes(&b"source"[..]),
            &counters,
            Arc::new(PanicHook),
        );
        build.new_token(SyntaxKind::Unknown.into());
    }));
    assert!(panic.is_err());
    assert_eq!(counters.snapshot(), before);
}

#[test]
fn storage_retained_mapped_node_keeps_both_files_auxiliary_data_and_metadata() {
    let counters = Counters::new();
    let before = counters.snapshot();
    let mut canonical = builder(&counters);
    let canonical_root = canonical.new_token(SyntaxKind::EndOfFile.into());
    let canonical_id = canonical.id();
    let canonical_text = canonical
        .text_slice(vec![JsString::from_bytes(&b"canonical"[..])])
        .unwrap();
    let mut supplemental = builder(&counters);
    let supplemental_root = supplemental.new_token(SyntaxKind::EndOfFile.into());
    let supplemental_text = supplemental
        .text_slice(vec![JsString::from_bytes(&b"supplemental"[..])])
        .unwrap();
    let group = canonical
        .complete(canonical_root)
        .unwrap()
        .publish_bundle_unbound(vec![supplemental.complete(supplemental_root).unwrap()]);
    let supplemental = group.file(1).unwrap();
    let retained = supplemental.retain_node(supplemental_root).unwrap();
    drop(supplemental);
    drop(group);
    let source = retained.file();
    let sibling = source.file(canonical_id).unwrap();
    assert_eq!(sibling.root(), Some(canonical_root));
    assert_eq!(
        sibling.view().text_slice(canonical_text).unwrap()[0].as_bytes(),
        b"canonical"
    );
    assert_eq!(
        source.view().text_slice(supplemental_text).unwrap()[0].as_bytes(),
        b"supplemental"
    );
    drop(sibling);
    drop(source);
    drop(retained);
    assert_eq!(counters.snapshot(), before);
}

#[test]
fn storage_complete_and_publication_reject_references_installed_by_mutation() {
    let counters = Counters::new();
    let mut foreign = builder(&counters);
    let foreign_node = foreign.new_token(SyntaxKind::Unknown.into());
    let mut initial = builder(&counters);
    let root = initial.new_token(SyntaxKind::Unknown.into());
    initial
        .node_mut(root)
        .unwrap()
        .set_parent(Some(foreign_node));
    assert!(matches!(initial.complete(root), Err(Error::WrongOwner)));

    let mut initial = builder(&counters);
    let root = initial.new_token(SyntaxKind::Unknown.into());
    let mut parsed = initial.complete(root).unwrap();
    parsed
        .builder_mut()
        .node_mut(root)
        .unwrap()
        .set_parent(Some(foreign_node));
    assert!(matches!(
        parsed.try_publish_unbound(),
        Err(Error::WrongOwner)
    ));
}

#[test]
fn storage_group_validation_admits_only_the_consumed_sibling_owners() {
    let counters = Counters::new();
    let mut first = builder(&counters);
    let local_list = first
        .new_list(TextRange::new(0, 0), NodeSlice::empty())
        .unwrap();
    let first_root = array(&mut first, local_list);
    let mut second = builder(&counters);
    let child = second.new_token(SyntaxKind::Unknown.into());
    let edges = second.node_slice(vec![Some(child)]).unwrap();
    let foreign_list = second.new_list(TextRange::new(0, 1), edges).unwrap();
    let second_root = array(&mut second, foreign_list);
    let mut first = first.complete(first_root).unwrap();
    let second = second.complete(second_root).unwrap();
    match first.builder_mut().node_mut(first_root).unwrap().data_mut() {
        NodeData::ArrayLiteralExpression(data) => data.elements = Some(foreign_list),
        _ => unreachable!(),
    }
    assert!(matches!(
        first.view().list(foreign_list),
        Err(Error::WrongOwner)
    ));
    let bundle = first.try_publish_bundle_unbound(vec![second]).unwrap();
    let file = bundle.file(0).unwrap();
    assert_eq!(
        file.view()
            .for_node_owner(second_root)
            .unwrap()
            .file_info()
            .root,
        Some(second_root)
    );
    assert_eq!(
        &file
            .view()
            .node_slice(file.view().list(foreign_list).unwrap().nodes())
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        &[Some(child)]
    );
    let retained = file.retain_node(first_root).unwrap();
    drop(file);
    drop(bundle);
    let file = retained.file();
    assert_eq!(
        file.view().for_node_owner(child).unwrap().file_info().root,
        Some(second_root)
    );
}

#[test]
fn storage_lazy_validation_rejects_foreign_edges_added_after_node_creation() {
    let counters = Counters::new();
    let mut foreign = builder(&counters);
    let foreign_node = foreign.new_token(SyntaxKind::Unknown.into());
    let mut initial = builder(&counters);
    let root = initial.new_token(SyntaxKind::Unknown.into());
    let file = initial.complete(root).unwrap().publish_unbound();
    let mut rejected = None;
    let result = file.view().jsdoc(root, |transaction| {
        let doc = transaction.new_token(SyntaxKind::Unknown.into());
        rejected = Some(doc);
        transaction.node_mut(doc)?.set_parent(Some(foreign_node));
        Ok(vec![doc])
    });
    assert!(matches!(result, Err(Error::WrongOwner)));
    assert!(matches!(
        file.view().node(rejected.unwrap()),
        Err(Error::InvalidSlot)
    ));
    let result = file.view().jsdoc(root, |transaction| {
        let doc = transaction.new_parenthesized_expression(None);
        rejected = Some(doc);
        {
            let mut node = transaction.node_mut(doc)?;
            let NodeData::ParenthesizedExpression(data) = node.data_mut() else {
                unreachable!("constructed parenthesized expression");
            };
            data.expression = Some(foreign_node);
        }
        Ok(vec![doc])
    });
    assert!(matches!(result, Err(Error::WrongOwner)));
    assert!(matches!(
        file.view().node(rejected.unwrap()),
        Err(Error::InvalidSlot)
    ));
    assert!(file.view().eager_jsdoc(root).unwrap().is_none());
}

#[test]
fn storage_explicit_import_retains_transitive_bundles_and_rejects_bare_ids() {
    let counters = Counters::new();
    let before = counters.snapshot();
    let mut first = builder(&counters);
    let first_root = first.new_identifier(JsString::from_bytes(&b"first"[..]));
    let mut second = builder(&counters);
    let second_root = second.new_identifier(JsString::from_bytes(&b"second"[..]));
    let group = first
        .complete(first_root)
        .unwrap()
        .publish_bundle_unbound(vec![second.complete(second_root).unwrap()]);
    let lazy = group
        .file(0)
        .unwrap()
        .view()
        .jsdoc(first_root, |transaction| {
            Ok(vec![
                transaction.new_identifier(JsString::from_bytes(&b"lazy import"[..]))
            ])
        })
        .unwrap()[0];
    let mut importer = builder(&counters);
    assert!(matches!(
        importer.view().node(first_root),
        Err(Error::WrongOwner)
    ));
    importer.retain_file(group.file(1).unwrap());
    importer.retain_file(group.file(0).unwrap());
    let imported_view = importer.view().for_node_owner(second_root).unwrap();
    assert_eq!(
        imported_view
            .for_node_owner(first_root)
            .unwrap()
            .file_info()
            .root,
        Some(first_root)
    );
    let edges = importer
        .node_slice(vec![Some(first_root), Some(second_root)])
        .unwrap();
    let list = importer.new_list(TextRange::new(0, 2), edges).unwrap();
    let root = array(&mut importer, list);
    assert_eq!(
        importer
            .view()
            .for_node_owner(first_root)
            .unwrap()
            .node(root)
            .unwrap()
            .kind(),
        SyntaxKind::ArrayLiteralExpression
    );
    let importer = importer.complete(root).unwrap().publish_unbound();
    let mut outer = builder(&counters);
    outer.retain_file(importer.clone());
    let outer_root = array(&mut outer, list);
    let outer = outer.complete(outer_root).unwrap().publish_unbound();
    let retained = outer.retain_node(outer_root).unwrap();
    let imported_retained = outer.retain_node(first_root).unwrap();
    let imported_lazy = outer.retain_node(lazy).unwrap();
    assert_eq!(imported_retained.id(), first_root);
    assert_eq!(
        imported_retained
            .read()
            .data_source()
            .as_identifier()
            .unwrap()
            .text(),
        b"first"
    );
    drop(group);
    drop(importer);
    drop(outer);
    let outer = retained.file();
    assert_eq!(
        &outer
            .view()
            .node_slice(edges)
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        &[Some(first_root), Some(second_root)]
    );
    assert_eq!(
        outer
            .view()
            .for_node_owner(second_root)
            .unwrap()
            .file_info()
            .root,
        Some(second_root)
    );
    assert!(matches!(
        builder(&counters).view().node(first_root),
        Err(Error::WrongOwner)
    ));
    drop(outer);
    drop(retained);
    assert_eq!(
        imported_retained
            .read()
            .data_source()
            .as_identifier()
            .unwrap()
            .text(),
        b"first"
    );
    drop(imported_retained);
    assert_eq!(
        imported_lazy
            .read()
            .data_source()
            .as_identifier()
            .unwrap()
            .text(),
        b"lazy import"
    );
    drop(imported_lazy);
    assert_eq!(counters.snapshot(), before);
}

fn source_cache_builder() -> (AstBuilder, NodeId) {
    let mut builder = AstBuilder::new(SourceText::default(), &Counters::new());
    let source = builder.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/source.ts".as_slice()),
            ..SourceFileParseOptions::default()
        },
        SourceText::default(),
        None,
        None,
    );
    (builder, source)
}

#[test]
fn storage_foreign_cache_candidates_are_rejected_before_installation_and_can_retry() {
    let (mut factory, source) = source_cache_builder();
    let (mut foreign, _) = source_cache_builder();
    let own = factory.new_token(SyntaxKind::Unknown.into());
    let foreign = foreign.new_token(SyntaxKind::Unknown.into());
    {
        let state = factory.view().source_file(source).unwrap();
        assert!(matches!(
            state.try_node_index_cache(|| Ok(NodeIndexCache::new(vec![None, Some(foreign)]))),
            Err(Error::WrongOwner)
        ));
        let cached = state
            .try_node_index_cache(|| Ok(NodeIndexCache::new(vec![None, Some(own)])))
            .unwrap()
            .unwrap();
        assert_eq!(cached.nodes(), &[None, Some(own)]);
    }
    let published = factory.complete(source).unwrap().publish_unbound();
    let state = published.view().source_file(source).unwrap();
    assert_eq!(
        state
            .node_index_cache(|| panic!("publication moves the existing cache"))
            .unwrap()
            .nodes(),
        &[None, Some(own)]
    );
}

#[test]
fn storage_published_source_cache_accepts_only_its_actual_retention_root() {
    let (factory, source) = source_cache_builder();
    let published = factory.complete(source).unwrap().publish_unbound();
    let (mut other, _) = source_cache_builder();
    let foreign = other.new_token(SyntaxKind::Unknown.into());
    let state = published.view().source_file(source).unwrap();
    assert!(matches!(
        state.try_node_index_cache(|| Ok(NodeIndexCache::new(vec![None, Some(foreign)]))),
        Err(Error::WrongOwner)
    ));
    // A caller retaining this source cannot lend its own nodes to a persistent
    // cache that survives independently under the imported source's owner.
    other.retain_file(published.clone());
    assert!(matches!(
        other
            .view()
            .source_file(source)
            .unwrap()
            .try_node_index_cache(|| Ok(NodeIndexCache::new(vec![None, Some(foreign)]))),
        Err(Error::WrongOwner)
    ));
    let other = other.complete(foreign).unwrap().publish_unbound();
    assert!(matches!(
        other
            .view()
            .source_file(source)
            .unwrap()
            .try_node_index_cache(|| Ok(NodeIndexCache::new(vec![None, Some(foreign)]))),
        Err(Error::WrongOwner)
    ));
    assert_eq!(
        state
            .node_index_cache(|| NodeIndexCache::new(vec![None, Some(source)]))
            .unwrap()
            .nodes(),
        &[None, Some(source)]
    );

    // Narrowing to the imported source must retain its original mapped siblings.
    let (canonical, canonical_id) = source_cache_builder();
    let (supplemental, supplemental_id) = source_cache_builder();
    let bundle = canonical
        .complete(canonical_id)
        .unwrap()
        .publish_bundle_unbound(vec![supplemental.complete(supplemental_id).unwrap()]);
    let mapped = bundle.file(0).unwrap();
    let (mut importing, importing_id) = source_cache_builder();
    importing.retain_file(mapped.clone());
    let importing = importing.complete(importing_id).unwrap().publish_unbound();
    importing
        .view()
        .source_file(canonical_id)
        .unwrap()
        .node_index_cache(|| {
            NodeIndexCache::new(vec![None, Some(canonical_id), Some(supplemental_id)])
        });
    drop(importing);
    drop(bundle);
    assert_eq!(
        mapped
            .view()
            .source_file(canonical_id)
            .unwrap()
            .node_index_cache(|| panic!("imported bundle cache already initialized"))
            .unwrap()
            .nodes(),
        &[None, Some(canonical_id), Some(supplemental_id)]
    );
}

#[test]
fn storage_publication_revalidates_cached_references_after_exclusive_state_replacement() {
    let (mut first, first_source) = source_cache_builder();
    let (mut second, second_source) = source_cache_builder();
    first
        .view()
        .source_file(first_source)
        .unwrap()
        .node_index_cache(|| NodeIndexCache::new(vec![None, Some(first_source)]));
    std::mem::swap(
        first.source_file_mut(first_source).unwrap(),
        second.source_file_mut(second_source).unwrap(),
    );
    assert!(matches!(
        second.complete(second_source),
        Err(Error::WrongOwner)
    ));
}

fn source_metadata_file(factory: &mut AstBuilder) -> NodeId {
    factory.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/source.ts".as_slice()),
            ..SourceFileParseOptions::default()
        },
        SourceText::default(),
        None,
        None,
    )
}

#[test]
fn storage_source_file_clones_share_existing_metadata_elements_but_not_replaced_headers() {
    let mut factory = AstBuilder::new(SourceText::default(), &Counters::new());
    let original = source_metadata_file(&mut factory);
    let first = factory.new_token(SyntaxKind::Unknown.into());
    let second = factory.new_token(SyntaxKind::Identifier.into());
    let nodes = factory.source_nodes(vec![Some(first), None]).unwrap();
    let references = factory
        .source_references(vec![FileReference {
            file_name: JsString::from_bytes(b"first.ts".as_slice()),
            ..FileReference::default()
        }])
        .unwrap();
    let pragmas = factory.source_pragmas(vec![Pragma::default()]).unwrap();
    {
        let state = factory.source_file_mut(original).unwrap();
        state.imports = nodes;
        state.referenced_files = references;
        state.pragmas = pragmas;
    }
    let cloned = factory.clone_source_file(original);
    factory.source_nodes_mut(nodes).unwrap()[0] = Some(second);
    factory.source_references_mut(references).unwrap()[0].file_name =
        JsString::from_bytes(b"changed.ts".as_slice());
    factory.source_pragmas_mut(pragmas).unwrap()[0].args.insert(
        JsString::from_bytes(b"path".as_slice()),
        PragmaArgument::default(),
    );
    {
        let state = factory.view().source_file(cloned).unwrap();
        assert_eq!(&*state.imports().unwrap(), &[Some(second), None]);
        assert_eq!(
            state.referenced_files().unwrap()[0].file_name.as_bytes(),
            b"changed.ts"
        );
        assert_eq!(state.pragmas().unwrap()[0].args.len(), 1);
    }
    let replacement = factory.source_nodes(vec![Some(first)]).unwrap();
    factory.source_file_mut(original).unwrap().imports = replacement;
    let published = factory.complete(original).unwrap().publish_unbound();
    assert_eq!(
        &*published
            .view()
            .source_file(original)
            .unwrap()
            .imports()
            .unwrap(),
        &[Some(first)]
    );
    assert_eq!(
        &*published
            .view()
            .source_file(cloned)
            .unwrap()
            .imports()
            .unwrap(),
        &[Some(second), None]
    );
}

#[test]
fn storage_retained_imported_metadata_is_readable_and_cannot_be_mutated_by_another_builder() {
    let mut original = AstBuilder::new(SourceText::default(), &Counters::new());
    let root = source_metadata_file(&mut original);
    let backing = original.source_nodes(vec![Some(root)]).unwrap();
    original.source_file_mut(root).unwrap().imports = backing;
    let published = original.complete(root).unwrap().publish_unbound();
    let mut importing = AstBuilder::new(SourceText::default(), &Counters::new());
    importing.retain_file(published.clone());
    let cloned = importing.clone_source_file(root);
    assert!(matches!(
        importing.source_nodes_mut(backing),
        Err(Error::WrongOwner)
    ));
    let retained = importing.complete(cloned).unwrap().publish_unbound();
    drop(published);
    assert_eq!(
        &*retained
            .view()
            .source_file(cloned)
            .unwrap()
            .imports()
            .unwrap(),
        &[Some(root)]
    );
}

#[test]
fn storage_source_file_hooks_observe_initialized_metadata_and_keep_transaction_authority() {
    struct SourceHooks {
        original: Mutex<Option<NodeId>>,
        seen: Mutex<Vec<(ts_core::ScriptKind, i64)>>,
    }
    impl FactoryHooks for SourceHooks {
        fn on_create(&self, factory: &mut dyn Factory, node: NodeId) {
            if factory.node(node).kind() != SyntaxKind::SourceFile {
                return;
            }
            let state = factory.read_source_file(node).unwrap();
            assert_eq!(state.file_name(), b"/source.ts");
            assert!(state.text().is_empty());
            assert_eq!(state.script_kind, ts_core::ScriptKind::UNKNOWN);
            assert_eq!(state.node_count, 0);
            drop(state);
            let original = *self.original.lock().unwrap();
            if let Some(original) = original {
                let state = factory.mut_source_file(original).unwrap();
                state.script_kind = ts_core::ScriptKind::TSX;
                state.node_count = 999;
            }
            let state = factory.mut_source_file(node).unwrap();
            state.script_kind = ts_core::ScriptKind::JS;
            state.node_count = 23;
            if original.is_none() {
                *self.original.lock().unwrap() = Some(node);
            }
        }
        fn on_update(&self, factory: &mut dyn Factory, node: NodeId, _: NodeId) {
            let state = factory.read_source_file(node).unwrap();
            self.seen
                .lock()
                .unwrap()
                .push((state.script_kind, state.node_count));
        }
    }
    let hooks = Arc::new(SourceHooks {
        original: Mutex::new(None),
        seen: Mutex::new(Vec::new()),
    });
    let mut factory =
        AstBuilder::with_hooks(SourceText::default(), &Counters::new(), hooks.clone());
    let original = source_metadata_file(&mut factory);
    let cloned = factory.clone_source_file(original);
    assert_eq!(
        &*hooks.seen.lock().unwrap(),
        &[(ts_core::ScriptKind::TSX, 23)]
    );
    {
        let mut borrowed = BorrowedFactory(&mut factory);
        assert_eq!(
            borrowed.read_source_file(cloned).unwrap().script_kind,
            ts_core::ScriptKind::TSX
        );
        borrowed.mut_source_file(cloned).unwrap().node_count = 24;
    }
    let file = factory.complete(original).unwrap().publish_unbound();
    assert_eq!(file.view().source_file(cloned).unwrap().node_count, 24);
    file.view()
        .source_jsdoc(original, original, |transaction| {
            assert!(matches!(
                transaction.read_source_file(original),
                Err(Error::InvalidGraph)
            ));
            assert!(matches!(
                transaction.mut_source_file(original),
                Err(Error::InvalidGraph)
            ));
            Ok(Vec::new())
        })
        .unwrap();
}

#[test]
fn a_storage_without_text_adopts_one_and_a_storage_with_text_keeps_its_own() {
    let counters = Counters::new();
    // A decoded tree's storage has no text. Once it adopts the text of the file
    // constructed in it, tokens of that file are inside the storage's source.
    let mut empty = AstBuilder::new(SourceText::from_loaded_bytes(&b""[..]), &counters);
    let parent = empty.new_identifier(JsString::from_bytes(b"a".as_slice()));
    empty
        .adopt_source(SourceText::from_loaded_bytes(&b"a + b"[..]))
        .unwrap();
    let file = empty.complete(parent).unwrap().publish_unbound();
    let token = file
        .view()
        .get_or_create_token(SyntaxKind::PlusToken, 1, 3, parent, 0)
        .unwrap();
    assert_eq!((token.pos(), token.end()), (1, 3));
    // Nodes may refer into a text that is there, so it cannot be replaced.
    let mut full = builder(&counters);
    assert_eq!(
        full.adopt_source(SourceText::from_loaded_bytes(&b"other"[..])),
        Err(Error::InvalidGraph)
    );
}
