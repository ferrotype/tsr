use crate::scenarios;
use crate::{Counters, Error, FileBuilder, Node, Scope, TokenKey};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
};
#[test]
fn e3_id_exhaustion() {
    scenarios::id_exhaustion();
}
#[test]
fn e3_wrong_owner_rejected() {
    scenarios::wrong_owner_rejected();
}
#[test]
fn e3_stale_and_recycled_ids_rejected() {
    scenarios::stale_and_recycled_ids_rejected();
}
#[test]
fn e3_concurrent_lazy_storage() {
    scenarios::concurrent_lazy_storage();
}
#[test]
fn e3_mapper_bundle_disposal() {
    scenarios::mapper_bundle_disposal();
}
#[test]
fn e3_owners_return_to_baseline() {
    scenarios::owners_return_to_baseline();
}
#[test]
fn e3_allocations_return_to_baseline() {
    scenarios::allocations_return_to_baseline();
}

#[test]
fn branded_core_access_borrows_without_retaining_another_owner() {
    let counters = Counters::new();
    let mut builder = FileBuilder::<u32>::new(Arc::from(&b"x"[..]), &counters);
    let id = builder.push(Node::new(1, 42));
    let file = builder.finish();
    let mut scope = Scope::new();
    scope.insert(file.clone());
    let retained_counts = counters.snapshot();
    let crate::bundle::Root::File(root) = &file.root else {
        unreachable!()
    };
    let retained_refs = Arc::strong_count(root);
    let direct = file.node(id).unwrap();
    scope
        .with_core_arena(id.arena(), |local| {
            let checked = local.check(id).unwrap();
            assert_eq!(local.id(checked), id);
            assert!(std::ptr::eq(
                local.get(checked),
                std::ptr::from_ref(&*direct)
            ));
            assert_eq!(local.get(checked).data, 42);
            assert_eq!(counters.snapshot(), retained_counts);
            assert_eq!(Arc::strong_count(root), retained_refs);
        })
        .unwrap();
    assert!(scope.with_core_arena(file.lazy_arena(), |_| ()).is_err());
}

#[test]
fn lazy_graph_cycles_and_empty_results_are_cached() {
    let counters = Counters::new();
    let mut builder = FileBuilder::<Vec<crate::NodeId>>::new(Arc::from(&b"x"[..]), &counters);
    let parent = builder.push(Node::new(1, Vec::new()));
    let empty_parent = builder.push(Node::new(1, Vec::new()));
    let file = builder.finish();
    let docs = file
        .jsdoc(parent, |transaction| {
            let root = transaction.push(Node::new(2, Vec::new()));
            let child = transaction.push(Node::new(3, vec![root]));
            transaction.node_mut(root)?.data.push(child);
            transaction.node_mut(child)?.parent = Some(root);
            Ok(vec![root, child])
        })
        .unwrap();
    let cached = file
        .jsdoc(parent, |_| panic!("cached graph must skip initialization"))
        .unwrap();
    assert_eq!(cached.ids().as_ptr(), docs.ids().as_ptr());
    assert_eq!(docs.node(0).unwrap().data, [docs.ids()[1]]);
    assert_eq!(docs.node(1).unwrap().data, [docs.ids()[0]]);
    assert_eq!(docs.node(1).unwrap().parent, Some(docs.ids()[0]));
    assert!(file
        .jsdoc(empty_parent, |_| Ok(Vec::new()))
        .unwrap()
        .ids()
        .is_empty());
    assert!(file
        .jsdoc(empty_parent, |_| panic!("empty result is cached"))
        .unwrap()
        .ids()
        .is_empty());
}

#[test]
fn token_failure_preserves_existing_cache_and_permits_retry() {
    let counters = Counters::new();
    let mut builder = FileBuilder::<u32>::new(Arc::from(&b"xy"[..]), &counters);
    let parent = builder.push(Node::new(1, 0));
    let mut reparsed_node = Node::new(1, 0);
    reparsed_node.reparsed = true;
    let reparsed = builder.push(reparsed_node);
    let owner = builder.finish();
    let first_key = TokenKey {
        parent,
        start: 0,
        end: 1,
    };
    let first = owner.token(first_key, 2, || 7).unwrap().retain();
    assert!(matches!(
        owner.try_token(first_key, 3, || 8),
        Err(Error::TokenKindMismatch {
            cached: 2,
            requested: 3
        })
    ));
    assert!(matches!(
        owner.try_token(
            TokenKey {
                parent: reparsed,
                ..first_key
            },
            2,
            || 9
        ),
        Err(Error::ReparsedParent)
    ));
    assert!(matches!(
        owner.try_token(
            TokenKey {
                end: 3,
                ..first_key
            },
            2,
            || 9
        ),
        Err(Error::InvalidTokenRange)
    ));
    let retry_key = TokenKey {
        start: 1,
        end: 2,
        ..first_key
    };
    assert!(catch_unwind(AssertUnwindSafe(
        || owner.token(retry_key, 2, || panic!("initializer"))
    ))
    .is_err());
    let retry = owner.token(retry_key, 2, || 11).unwrap();
    assert_eq!(retry.parent, Some(parent));
    assert_eq!(retry.data, 11);
    assert_eq!(
        owner.token(first_key, 2, || panic!("cached")).unwrap().id(),
        first.id()
    );
    assert_eq!(first.data, 7);
}

#[test]
fn file_owns_decoded_text_and_its_position_map() {
    let counters = Counters::new();
    let file = FileBuilder::<()>::new(Arc::from(&b"\xef\xbb\xbf\xf0\x9f\x98\x80x"[..]), &counters)
        .finish();
    assert_eq!(file.source(), "\u{1f600}x".as_bytes());
    assert_eq!(file.source_text().as_bytes(), file.source());
    assert_eq!(file.position_map().utf8_to_utf16(4), 2);
    assert_eq!(file.position_map().utf16_to_utf8(2), 4);
}

#[test]
fn operation_unwind_prevents_another_checker_lease() {
    let counters = Counters::new();
    let generation = crate::Generation::new(&counters);
    let checker = crate::CheckerIdentity::new(generation.clone(), &counters);
    let sibling = crate::CheckerIdentity::new(generation.clone(), &counters);
    assert!(catch_unwind(AssertUnwindSafe(|| {
        let _lease = checker.lease().unwrap();
        panic!("checker operation panicked");
    }))
    .is_err());
    assert_eq!(generation.validate(), Err(Error::Retired));
    assert!(matches!(sibling.lease(), Err(Error::Retired)));
    assert!(matches!(checker.lease(), Err(Error::Retired)));
}

#[test]
fn lazy_initializer_reentry_panics_before_locking_and_can_retry() {
    for token_initializer in [false, true] {
        for operation in ["node", "jsdoc", "token"] {
            let counters = Counters::new();
            let mut builder = FileBuilder::<u32>::new(Arc::from(&b"xy"[..]), &counters);
            let cached_parent = builder.push(Node::new(1, 1));
            let parent = builder.push(Node::new(1, 2));
            let file = builder.finish();
            let cached = file
                .jsdoc(cached_parent, |transaction| {
                    Ok(vec![transaction.push(Node::new(2, 3))])
                })
                .unwrap();
            let cached_key = TokenKey {
                parent,
                start: 0,
                end: 1,
            };
            let cached_token = file.token(cached_key, 3, || 4).unwrap().retain();
            let key = TokenKey {
                start: 1,
                end: 2,
                ..cached_key
            };
            let reenter = || {
                // Core lookup is safe while the lazy write lock is held.
                assert_eq!(file.node(parent).unwrap().data, 2);
                match operation {
                    "node" => {
                        file.node(cached.ids()[0]).unwrap();
                    }
                    "jsdoc" => {
                        file.jsdoc(cached_parent, |_| panic!("cached")).unwrap();
                    }
                    "token" => {
                        file.token(cached_key, 3, || panic!("cached")).unwrap();
                    }
                    _ => unreachable!(),
                }
            };
            let mut provisional = None;
            let failure = catch_unwind(AssertUnwindSafe(|| {
                if token_initializer {
                    file.token(key, 3, || {
                        reenter();
                        5
                    })
                    .unwrap();
                } else {
                    file.jsdoc(parent, |transaction| {
                        provisional = Some(transaction.push(Node::new(2, 5)));
                        reenter();
                        Ok(vec![provisional.unwrap()])
                    })
                    .unwrap();
                }
            }))
            .unwrap_err();
            let message = failure
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| failure.downcast_ref::<&str>().copied())
                .unwrap();
            assert_eq!(
                message,
                "ts_arena: lazy initializer reentered its file's lazy storage"
            );
            assert_eq!(cached.node(0).unwrap().data, 3);
            assert_eq!(file.node(cached_token.id()).unwrap().data, 4);
            if let Some(id) = provisional {
                assert!(matches!(file.node(id), Err(Error::InvalidSlot)));
            }
            let retried = file
                .jsdoc(parent, |transaction| {
                    Ok(vec![transaction.push(Node::new(2, 6))])
                })
                .unwrap();
            assert_eq!(retried.node(0).unwrap().data, 6);
            assert_ne!(Some(retried.ids()[0]), provisional);
            assert_eq!(file.token(key, 3, || 7).unwrap().data, 7);
        }
    }
}

#[test]
fn lazy_reader_on_another_thread_waits_for_initializer_publication() {
    let counters = Counters::new();
    let mut builder = FileBuilder::<u32>::new(Arc::from(&b"x"[..]), &counters);
    let parent = builder.push(Node::new(1, 1));
    let other = builder.push(Node::new(1, 2));
    let file = builder.finish();
    let existing = file
        .jsdoc(parent, |transaction| {
            Ok(vec![transaction.push(Node::new(2, 3))])
        })
        .unwrap();
    let entered = std::sync::Barrier::new(2);
    let reader_started = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            file.jsdoc(other, |transaction| {
                entered.wait();
                reader_started.wait();
                for _ in 0..16 {
                    std::thread::yield_now();
                }
                Ok(vec![transaction.push(Node::new(2, 4))])
            })
            .unwrap()
        });
        entered.wait();
        let reader = scope.spawn(|| {
            reader_started.wait();
            assert_eq!(file.node(existing.ids()[0]).unwrap().data, 3);
        });
        assert_eq!(writer.join().unwrap().node(0).unwrap().data, 4);
        reader.join().unwrap();
    });
}

#[test]
fn lazy_initializer_can_initialize_a_different_file() {
    let counters = Counters::new();
    let mut first = FileBuilder::<u32>::new(Arc::from(&b"x"[..]), &counters);
    let first_parent = first.push(Node::new(1, 1));
    let first = first.finish();
    let mut second = FileBuilder::<u32>::new(Arc::from(&b"x"[..]), &counters);
    let second_parent = second.push(Node::new(1, 2));
    let second = second.finish();
    let docs = first
        .jsdoc(first_parent, |transaction| {
            let token = second.token(
                TokenKey {
                    parent: second_parent,
                    start: 0,
                    end: 1,
                },
                3,
                || 3,
            )?;
            Ok(vec![transaction.push(Node::new(2, token.data))])
        })
        .unwrap();
    assert_eq!(docs.node(0).unwrap().data, 3);
    assert_eq!(
        first
            .token(
                TokenKey {
                    parent: first_parent,
                    start: 0,
                    end: 1
                },
                3,
                || 4
            )
            .unwrap()
            .data,
        4
    );
}
