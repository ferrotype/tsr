use crate::*;
use std::panic::{catch_unwind, AssertUnwindSafe};
use ts_arena::{AuxId, Counters, Error};
use ts_core::TextRange;
use ts_jsstring::SourceText;

fn builder() -> AstBuilder {
    AstBuilder::new(SourceText::default(), &Counters::new())
}

#[test]
fn promoted_list_keeps_identity_and_unwound_edits() {
    let mut build = builder();
    let first = build.new_token(SyntaxKind::Unknown.into());
    let second = build.new_token(SyntaxKind::EndOfFile.into());
    let original_nodes = build.node_slice(vec![Some(first), None]).unwrap();
    let replacement_nodes = build.node_slice(vec![Some(second)]).unwrap();
    let list = build
        .new_list(TextRange::new(0, 2), original_nodes)
        .unwrap();
    let copied_header = build.clone_list(list).unwrap();
    let root = build.new_array_literal_expression(Some(list), false);
    build.set_list_modifier_flags(list, 7).unwrap();

    let failure = catch_unwind(AssertUnwindSafe(|| {
        let header = build.list_mut(list).unwrap();
        assert_eq!(header.nodes(), original_nodes);
        header.set_loc(TextRange::new(1, 4));
        panic!("after promoted list edit");
    }))
    .unwrap_err();
    assert_eq!(
        failure.downcast_ref::<&str>(),
        Some(&"after promoted list edit")
    );
    assert_eq!(build.view().list(list).unwrap().loc(), TextRange::new(1, 4));
    assert_eq!(build.view().list(list).unwrap().nodes(), original_nodes);
    assert_eq!(build.view().list(list).unwrap().modifier_flags(), 7);

    build.list_mut(list).unwrap().set_modifier_flags(11);
    build.set_list_nodes(list, replacement_nodes).unwrap();
    build.set_list_location(list, TextRange::new(2, 5)).unwrap();
    build.set_list_modifier_flags(list, 13).unwrap();
    let file = build.complete(root).unwrap().publish_unbound();
    assert_eq!(file.view().node(root).unwrap().element_list(), Some(list));
    let read = file.view().list(list).unwrap();
    assert_eq!(read.loc(), TextRange::new(2, 5));
    assert_eq!(read.nodes(), replacement_nodes);
    assert_eq!(read.modifier_flags(), 13);
    assert_eq!(
        file.view().node_slice(read.nodes()).unwrap().at(0),
        Some(second)
    );
    let copied = file.view().list(copied_header).unwrap();
    assert_eq!(copied.loc(), TextRange::new(0, 2));
    assert_eq!(copied.nodes(), original_nodes);
    assert_eq!(copied.modifier_flags(), 0);
}

#[test]
fn supplied_list_backing_is_checked_before_invalid_target() {
    let mut build = builder();
    let list = build
        .new_list(TextRange::new(0, 0), NodeSlice::empty())
        .unwrap();
    let missing = AuxId::from_parts(list.0.arena(), u32::MAX).unwrap();
    let absent_backing = NodeSlice {
        backing: Some(missing),
        start: 0,
        len: 0,
    };
    let mut foreign = builder();
    let foreign_target = foreign
        .new_list(TextRange::new(0, 0), NodeSlice::empty())
        .unwrap();
    assert_eq!(
        build.set_list_nodes(foreign_target, absent_backing),
        Err(Error::InvalidSlot)
    );
    assert_eq!(
        build.set_list_nodes(foreign_target, NodeSlice::empty()),
        Err(Error::WrongOwner)
    );
    let wrong_kind_backing = NodeSlice {
        backing: Some(list.0),
        start: 0,
        len: 0,
    };
    assert_eq!(
        build.set_list_nodes(NodeListId(missing), wrong_kind_backing),
        Err(Error::InvalidGraph)
    );
    assert_eq!(
        build.set_list_nodes(NodeListId(missing), NodeSlice::empty()),
        Err(Error::InvalidSlot)
    );
    assert!(build.view().list(list).unwrap().nodes().is_nil());
}

#[test]
fn cold_metadata_mutator_rejects_compact_list_without_promoting_it() {
    let mut build = builder();
    let list = build
        .new_list(TextRange::new(1, 3), NodeSlice::empty())
        .unwrap();
    build.set_list_modifier_flags(list, 17).unwrap();
    let original = build.view().list(list).unwrap().to_owned();
    // Metadata mutators share this cold-record helper. A mismatched identity
    // must fail before it changes the compact list's representation or value.
    assert!(matches!(
        build.auxiliary_mut(list.0),
        Err(Error::InvalidGraph)
    ));
    assert!(matches!(
        build.view().auxiliary(list.0).unwrap().value(),
        crate::auxiliary::AuxValue::List(_)
    ));
    assert_eq!(build.view().list(list).unwrap().to_owned(), original);
}

#[test]
fn lazy_list_retains_compact_core_backing_through_owner_retirement() {
    let mut build = builder();
    let parent = build.new_token(SyntaxKind::EndOfFile.into());
    let child = build.new_token(SyntaxKind::Unknown.into());
    let core_nodes = build.node_slice(vec![Some(child), None]).unwrap();
    let core_list = build.new_list(TextRange::new(0, 2), core_nodes).unwrap();
    let file = build.complete(parent).unwrap().publish_unbound();
    let mut published_list = None;
    let lazy = file
        .view()
        .jsdoc(parent, |transaction| {
            let core = transaction.list(core_list)?;
            assert_eq!(core.loc(), TextRange::new(0, 2));
            let nodes = core.nodes();
            assert_eq!(
                transaction
                    .node_slice_read(nodes)?
                    .iter()
                    .collect::<Vec<_>>(),
                vec![Some(child), None]
            );
            let list = transaction.new_list(TextRange::new(3, 5), nodes)?;
            transaction.list_mut(list)?.set_modifier_flags(19);
            let root = transaction.new_array_literal_expression(Some(list), false);
            transaction.node_mut(root)?.set_parent(Some(parent));
            published_list = Some(list);
            Ok(vec![root])
        })
        .unwrap()[0];
    let list = published_list.unwrap();
    assert!(matches!(
        file.view().auxiliary(list.0).unwrap().full(),
        Some(AstStorageData::List(_))
    ));
    let retained = file.retain_node(lazy).unwrap();
    drop(file);
    let owner = retained.file();
    let core = owner.view().list(core_list).unwrap();
    let lazy = owner.view().list(list).unwrap();
    assert_eq!(core.nodes(), core_nodes);
    assert_eq!(lazy.nodes(), core_nodes);
    assert_eq!(lazy.loc(), TextRange::new(3, 5));
    assert_eq!(lazy.modifier_flags(), 19);
    assert_eq!(
        owner
            .view()
            .node_slice(lazy.nodes())
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        vec![Some(child), None]
    );
}
