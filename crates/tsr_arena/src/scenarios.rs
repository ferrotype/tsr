//! S04 assertions shared by tests, release evidence, Miri and AddressSanitizer.
use crate::{
    BundleOwner, CheckerIdentity, Counters, Counts, Error, FileBuilder, Generation, Node, NodeId,
    Scope, ScratchOwner, SymbolId, TokenKey,
};
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc, Barrier,
};

#[derive(Clone, Copy, Debug)]
pub struct Measurement {
    pub live_owner_delta: usize,
    pub live_allocation_delta: usize,
    pub peak_owners: usize,
    pub peak_allocations: usize,
}
struct Observation {
    counters: Counters,
    before: Counts,
    peak: Counts,
}
impl Observation {
    fn new() -> Self {
        let counters = Counters::new();
        Self {
            before: counters.snapshot(),
            counters,
            peak: Counts::default(),
        }
    }
    fn observe(&mut self) {
        let now = self.counters.snapshot();
        self.peak.owners = self.peak.owners.max(now.owners);
        self.peak.allocations = self.peak.allocations.max(now.allocations);
    }
    fn finish(self) -> Measurement {
        let after = self.counters.snapshot();
        let result = Measurement {
            live_owner_delta: after.owners.abs_diff(self.before.owners),
            live_allocation_delta: after.allocations.abs_diff(self.before.allocations),
            peak_owners: self.peak.owners,
            peak_allocations: self.peak.allocations,
        };
        assert_eq!(
            result.live_owner_delta, 0,
            "owner roots remain after final drop"
        );
        assert_eq!(
            result.live_allocation_delta, 0,
            "storage remains after final drop"
        );
        assert!(result.peak_owners > 0 && result.peak_allocations > 0);
        result
    }
}
fn file(counters: &Counters, value: u64) -> (FileBuilder<u64, u64>, NodeId, SymbolId) {
    let mut builder = FileBuilder::new(Arc::from(&b"a b c d"[..]), counters);
    let node = builder.push(Node::new(1, value));
    let symbol = builder.push_symbol(value + 10);
    (builder, node, symbol)
}

pub fn id_exhaustion() -> Measurement {
    let mut observation = Observation::new();
    assert_eq!(std::mem::size_of::<NodeId>(), 8);
    assert_eq!(std::mem::size_of::<Option<NodeId>>(), 8);
    assert_eq!(std::mem::size_of::<Option<SymbolId>>(), 8);
    for start in [0, u64::from(u32::MAX) + 1, u64::MAX] {
        let counter = AtomicU64::new(start);
        assert!(crate::ids::checked_arena(&counter).is_err());
        assert_eq!(counter.load(Ordering::Relaxed), start);
    }
    let counter = AtomicU64::new((1 << 31) - 1);
    for expected in [(1_u32 << 31) - 1, 1_u32 << 31, (1_u32 << 31) + 1] {
        let arena = crate::ids::checked_arena(&counter).unwrap();
        let node = NodeId::new(arena, u32::MAX);
        let symbol = SymbolId::new(arena, u32::MAX);
        assert_eq!(node.arena().get(), expected);
        assert_eq!(node.slot(), u32::MAX);
        assert_eq!(symbol.arena(), node.arena());
        assert_eq!(SymbolId::from_bits(symbol.bits()), Ok(symbol));
    }
    let counter = AtomicU64::new(u64::from(u32::MAX));
    assert_eq!(crate::ids::checked_arena(&counter).unwrap().get(), u32::MAX);
    assert!(crate::ids::checked_arena(&counter).is_err());
    assert_eq!(counter.load(Ordering::Relaxed), u64::from(u32::MAX) + 1);
    assert_eq!(crate::ids::next_slot(u32::MAX as usize - 1), Ok(u32::MAX));
    assert_eq!(
        crate::ids::next_slot(u32::MAX as usize),
        Err(Error::InvalidSlot)
    );
    for raw in [0, 1, 1_u64 << 32] {
        assert_eq!(NodeId::from_bits(raw), Err(Error::InvalidId));
        assert_eq!(SymbolId::from_bits(raw), Err(Error::InvalidId));
    }
    let mut ids = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    (0..16)
                        .map(|_| crate::ids::next_arena())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            ids.extend(handle.join().unwrap());
        }
    });
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 64);
    let mut scratch = ScratchOwner::new(&observation.counters);
    let id = scratch.push(Node::new(1, 17));
    assert_eq!(scratch.node(id).unwrap().data, 17);
    observation.observe();
    drop(scratch);
    observation.finish()
}

pub fn wrong_owner_rejected() -> Measurement {
    let mut observation = Observation::new();
    {
        let (a, a_node, a_symbol) = file(&observation.counters, 1);
        let (b, b_node, b_symbol) = file(&observation.counters, 2);
        assert_eq!(a_node.slot(), b_node.slot());
        assert_eq!(a_symbol.slot(), b_symbol.slot());
        let mut a_scope = Scope::new();
        a_scope.insert(a.finish());
        let mut b_scope = Scope::new();
        b_scope.insert(b.finish());
        assert_eq!(a_scope.import(a_node).unwrap().data, 1);
        assert!(matches!(a_scope.import(b_node), Err(Error::WrongOwner)));
        assert!(matches!(
            b_scope.import_symbol(a_symbol),
            Err(Error::WrongOwner)
        ));
        assert!(matches!(
            a_scope.import(NodeId::from_bits(a_node.bits() + 1).unwrap()),
            Err(Error::InvalidSlot)
        ));
        assert!(matches!(
            a_scope.import_symbol(SymbolId::from_bits(a_node.bits()).unwrap()),
            Err(Error::WrongOwner)
        ));
        let generation = Generation::new(&observation.counters);
        let first = CheckerIdentity::new(generation.clone(), &observation.counters);
        let second = CheckerIdentity::new(generation, &observation.counters);
        let lease = second.lease().unwrap();
        assert_eq!(lease.check_slot(second.id(), 1, 1), Ok(0));
        assert_eq!(lease.check_slot(first.id(), 1, 1), Err(Error::WrongOwner));
        assert_eq!(lease.check_slot(second.id(), 0, 1), Err(Error::InvalidSlot));
        assert_eq!(lease.check_slot(second.id(), 2, 1), Err(Error::InvalidSlot));
        observation.observe();
    }
    observation.finish()
}

pub fn stale_and_recycled_ids_rejected() -> Measurement {
    let mut observation = Observation::new();
    {
        let (builder, old_id, _) = file(&observation.counters, 1);
        let owner = builder.finish();
        let retained = owner.retain_node(old_id).unwrap();
        drop(owner);
        let (builder, new_id, _) = file(&observation.counters, 2);
        assert_eq!(new_id.slot(), old_id.slot());
        assert_ne!(new_id.arena(), old_id.arena());
        let mut scope = Scope::new();
        scope.insert(builder.finish());
        assert!(matches!(scope.import(old_id), Err(Error::WrongOwner)));
        assert_eq!(retained.data, 1);
        drop(retained);
        assert!(matches!(scope.import(old_id), Err(Error::WrongOwner)));
        let (builder, parent, _) = file(&observation.counters, 3);
        let owner = builder.finish();
        let mut failed_ids = Vec::new();
        assert!(matches!(
            owner.jsdoc(parent, |transaction| {
                failed_ids.push(transaction.push(Node::new(2, 10)));
                Err(Error::InvalidGraph)
            }),
            Err(Error::InvalidGraph)
        ));
        assert!(matches!(
            owner.jsdoc(parent, |transaction| {
                failed_ids.push(transaction.push(Node::new(2, 11)));
                Ok(vec![parent])
            }),
            Err(Error::InvalidGraph)
        ));
        let docs = owner
            .jsdoc(parent, |transaction| {
                Ok(vec![transaction.push(Node::new(2, 12))])
            })
            .unwrap();
        for failed in failed_ids {
            assert_ne!(failed, docs.ids()[0]);
            assert!(matches!(owner.node(failed), Err(Error::InvalidSlot)));
        }
        let generation = Generation::new(&observation.counters);
        let checker = CheckerIdentity::new(generation.clone(), &observation.counters);
        let lease = checker.lease().unwrap();
        assert_eq!(lease.check_slot(checker.id(), 1, 1), Ok(0));
        generation.retire();
        assert_eq!(lease.check_slot(checker.id(), 1, 1), Err(Error::Retired));
        assert!(matches!(checker.lease(), Err(Error::Retired)));
        drop(lease);
        let replacement = CheckerIdentity::new(
            Generation::new(&observation.counters),
            &observation.counters,
        );
        assert_ne!(replacement.generation().id(), generation.id());
        assert_eq!(
            replacement.lease().unwrap().check_slot(checker.id(), 1, 1),
            Err(Error::WrongOwner)
        );
        let mut scratch = ScratchOwner::new(&observation.counters);
        let stale = scratch.push(Node::new(1, 7));
        drop(scratch);
        let mut scratch = ScratchOwner::new(&observation.counters);
        let fresh = scratch.push(Node::new(1, 8));
        assert_eq!(stale.slot(), fresh.slot());
        assert!(matches!(scratch.node(stale), Err(Error::WrongOwner)));
        observation.observe();
    }
    observation.finish()
}

pub fn concurrent_lazy_storage() -> Measurement {
    let mut observation = Observation::new();
    {
        let (mut builder, parent, _) = file(&observation.counters, 11);
        let other = builder.push(Node::new(1, 12));
        let reparsed = builder.push(Node {
            kind: 1,
            parent: None,
            reparsed: true,
            data: 13,
        });
        let owner = builder.finish();
        let initial = owner
            .jsdoc(parent, |transaction| {
                Ok(vec![transaction.push(Node::new(2, 99))])
            })
            .unwrap();
        let retained = initial.node(0).unwrap().retain();
        let address = std::ptr::from_ref(&*retained);
        let doc_calls = AtomicUsize::new(0);
        let token_calls = AtomicUsize::new(0);
        let barrier = Barrier::new(5);
        let growth_started = Barrier::new(2);
        let growth_finished = Barrier::new(2);
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                let retained_address = std::ptr::from_ref(&*retained);
                barrier.wait();
                growth_started.wait();
                // Bound the runnable reader so blocked workers remain detectable by Miri.
                for _ in 0..32 {
                    assert_eq!(retained.data, 99);
                    assert_eq!(std::ptr::from_ref(&*retained), retained_address);
                    std::thread::yield_now();
                }
                growth_finished.wait();
                assert_eq!(retained.data, 99);
                assert_eq!(std::ptr::from_ref(&*retained), retained_address);
            });
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        let docs = owner
                            .jsdoc(other, |transaction| {
                                doc_calls.fetch_add(1, Ordering::SeqCst);
                                growth_started.wait();
                                let nodes = (0..300)
                                    .map(|index| {
                                        transaction.push(Node {
                                            kind: 2,
                                            parent: Some(other),
                                            reparsed: false,
                                            data: index,
                                        })
                                    })
                                    .collect();
                                growth_finished.wait();
                                Ok(nodes)
                            })
                            .unwrap();
                        for index in 0..300 {
                            let node = docs.node(index).unwrap();
                            assert_eq!(node.kind, 2);
                            assert_eq!(node.parent, Some(other));
                            assert_eq!(node.data, index as u64);
                        }
                        let key = TokenKey {
                            parent,
                            start: 0,
                            end: 1,
                        };
                        let token = owner
                            .token(key, 3, || {
                                token_calls.fetch_add(1, Ordering::SeqCst);
                                77
                            })
                            .unwrap();
                        assert_eq!(token.parent, Some(parent));
                        assert_eq!(token.data, 77);
                        (docs.ids().to_vec(), token.id())
                    })
                })
                .collect();
            let results: Vec<_> = handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect();
            assert!(results.windows(2).all(|pair| pair[0] == pair[1]));
            reader.join().unwrap();
        });
        assert_eq!(doc_calls.load(Ordering::SeqCst), 1);
        assert_eq!(token_calls.load(Ordering::SeqCst), 1);
        assert_eq!(retained.data, 99);
        assert_eq!(std::ptr::from_ref(&*retained), address);
        let key = TokenKey {
            parent,
            start: 0,
            end: 1,
        };
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.token(key, 99, || 1)))
                .is_err()
        );
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.token(
                TokenKey {
                    parent: reparsed,
                    start: 0,
                    end: 1
                },
                3,
                || 1
            )))
            .is_err()
        );
        assert_eq!(
            owner.token(key, 3, || panic!("cache miss")).unwrap().data,
            77
        );
        let mut failed = None;
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = owner.jsdoc(reparsed, |transaction| {
                failed = Some(transaction.push(Node::new(2, 123)));
                panic!("injected lazy initializer panic");
            });
        }))
        .is_err());
        let failed = failed.unwrap();
        assert!(matches!(owner.node(failed), Err(Error::InvalidSlot)));
        assert_eq!(owner.node(initial.ids()[0]).unwrap().data, 99);
        assert_eq!(retained.data, 99);
        assert_eq!(
            owner.token(key, 3, || panic!("cache miss")).unwrap().data,
            77
        );
        let retried = owner
            .jsdoc(reparsed, |transaction| {
                Ok(vec![transaction.push(Node::new(2, 124))])
            })
            .unwrap();
        assert_ne!(retried.ids()[0], failed);
        assert_eq!(retried.node(0).unwrap().data, 124);
        assert!(matches!(owner.node(failed), Err(Error::InvalidSlot)));
        observation.observe();
    }
    observation.finish()
}

pub fn mapper_bundle_disposal() -> Measurement {
    let mut observation = Observation::new();
    for first in 0..4 {
        for second in 0..4 {
            for third in 0..4 {
                if first == second || first == third || second == third {
                    continue;
                }
                let last = (0..4)
                    .find(|index| ![first, second, third].contains(index))
                    .unwrap();
                let (canonical, _, _) = file(&observation.counters, 0);
                let canonical_id = canonical.id();
                let mut node_ids = Vec::new();
                let supplemental = (1..4)
                    .map(|value| {
                        let (builder, node, _) = file(&observation.counters, value);
                        node_ids.push(node);
                        builder
                    })
                    .collect();
                let bundle = BundleOwner::new(canonical, supplemental);
                let weak = Arc::downgrade(&bundle);
                let mut handles: Vec<_> = (0..4).map(|index| bundle.file(index)).collect();
                let ids = handles[0].as_ref().unwrap().supplemental().to_vec();
                assert_eq!(ids.len(), 3);
                let retained = handles[3]
                    .as_ref()
                    .unwrap()
                    .retain_node(node_ids[2])
                    .unwrap();
                for handle in handles.iter().skip(1) {
                    assert_eq!(handle.as_ref().unwrap().canonical(), Some(canonical_id));
                }
                observation.observe();
                drop(bundle);
                for index in [first, second, third, last] {
                    drop(handles[index].take());
                }
                assert!(weak.upgrade().is_some());
                let canonical = retained.owner().file(canonical_id).unwrap();
                assert_eq!(canonical.supplemental(), ids);
                let mut scope = Scope::new();
                scope.insert(canonical);
                for (index, node) in node_ids.into_iter().enumerate() {
                    assert_eq!(scope.import(node).unwrap().data, index as u64 + 1);
                }
                drop(scope);
                drop(retained);
                assert!(weak.upgrade().is_none());
                assert_eq!(observation.counters.snapshot(), observation.before);
            }
        }
    }
    observation.finish()
}

pub fn owners_return_to_baseline() -> Measurement {
    let mut observation = Observation::new();
    {
        let (builder, id, symbol_id) = file(&observation.counters, 5);
        let file = builder.finish();
        let list = file
            .jsdoc(id, |transaction| {
                Ok(vec![transaction.push(Node::new(2, 7))])
            })
            .unwrap();
        let mut scope = Scope::new();
        scope.insert(file);
        let node = scope.import_retained(id).unwrap();
        let symbol = scope.import_symbol(symbol_id).unwrap().retain();
        observation.observe();
        drop(scope);
        assert_eq!(observation.counters.snapshot().owners, 1);
        assert_eq!(node.data, 5);
        assert_eq!(*symbol, 15);
        drop(node);
        drop(symbol);
        assert_eq!(observation.counters.snapshot().owners, 1);
        assert_eq!(list.node(0).unwrap().data, 7);
        drop(list);
        assert_eq!(observation.counters.snapshot(), observation.before);
    }
    observation.finish()
}

struct DropProbe(Arc<AtomicUsize>);
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
pub fn allocations_return_to_baseline() -> Measurement {
    let mut observation = Observation::new();
    let destroyed = Arc::new(AtomicUsize::new(0));
    {
        let mut builder = FileBuilder::new(Arc::from(&b"x"[..]), &observation.counters);
        let parent = builder.push(Node::new(1, DropProbe(destroyed.clone())));
        builder.push_symbol(DropProbe(destroyed.clone()));
        let file = builder.finish();
        assert!(matches!(
            file.jsdoc(parent, |transaction| {
                transaction.push(Node::new(2, DropProbe(destroyed.clone())));
                Ok(vec![parent])
            }),
            Err(Error::InvalidGraph)
        ));
        assert_eq!(destroyed.load(Ordering::SeqCst), 1);
        let docs = file
            .jsdoc(parent, |transaction| {
                Ok((0..260)
                    .map(|_| transaction.push(Node::new(2, DropProbe(destroyed.clone()))))
                    .collect())
            })
            .unwrap();
        let token = file
            .token(
                TokenKey {
                    parent,
                    start: 0,
                    end: 1,
                },
                3,
                || DropProbe(destroyed.clone()),
            )
            .unwrap()
            .retain();
        let mut scratch = ScratchOwner::new(&observation.counters);
        scratch.push(Node::new(4, DropProbe(destroyed.clone())));
        observation.observe();
        drop(file);
        drop(token);
        assert_eq!(destroyed.load(Ordering::SeqCst), 1);
        drop(docs);
        drop(scratch);
    }
    assert_eq!(destroyed.load(Ordering::SeqCst), 265);
    observation.finish()
}

pub type Scenario = (&'static str, fn() -> Measurement);

pub const ALL: [Scenario; 7] = [
    ("id_exhaustion", id_exhaustion),
    ("wrong_owner_rejected", wrong_owner_rejected),
    (
        "stale_and_recycled_ids_rejected",
        stale_and_recycled_ids_rejected,
    ),
    ("concurrent_lazy_storage", concurrent_lazy_storage),
    ("mapper_bundle_disposal", mapper_bundle_disposal),
    ("owners_return_to_baseline", owners_return_to_baseline),
    (
        "allocations_return_to_baseline",
        allocations_return_to_baseline,
    ),
];

pub fn run_all() -> [(&'static str, Measurement); 7] {
    ALL.map(|(id, scenario)| (id, scenario()))
}
