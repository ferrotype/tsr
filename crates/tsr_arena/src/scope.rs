use crate::NodeRecord;
use crate::{
    arena::Arena, ArenaId, Error, NodeId, RecordRef, RetainedRecord, StorageBuilder, StorageHandle,
    StorageSymbolRef, StorageView, SymbolId,
};
use std::{collections::BTreeMap, marker::PhantomData};
use ts_jsstring::SourceText;

/// An owning membership table. Raw imports check owner and published slot in release.
pub struct StorageScope<N: NodeRecord, S = ()> {
    nodes: BTreeMap<ArenaId, StorageHandle<N, S>>,
    symbols: BTreeMap<ArenaId, StorageHandle<N, S>>,
}

impl<N: NodeRecord, S> Default for StorageScope<N, S> {
    fn default() -> Self {
        Self {
            nodes: BTreeMap::new(),
            symbols: BTreeMap::new(),
        }
    }
}

impl<N: NodeRecord, S> StorageScope<N, S> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Retaining a mapped member admits every sibling to the resolution set.
    /// This table does not define a program's observable source-file list.
    pub fn insert(&mut self, file: StorageHandle<N, S>) {
        for member in file.into_members() {
            self.nodes.insert(member.core.id, member.clone());
            self.nodes.insert(member.lazy_arena(), member.clone());
            self.symbols.insert(member.symbol_arena(), member);
        }
    }

    pub fn import(&self, id: NodeId) -> Result<RecordRef<'_, N, S>, Error> {
        self.nodes
            .get(&id.arena())
            .ok_or(Error::WrongOwner)?
            .node(id)
    }

    pub fn import_retained(&self, id: NodeId) -> Result<RetainedRecord<N, S>, Error> {
        self.import(id).map(RecordRef::retain)
    }

    pub fn import_symbol(&self, id: SymbolId) -> Result<StorageSymbolRef<'_, N, S>, Error> {
        self.symbols
            .get(&id.arena())
            .ok_or(Error::WrongOwner)?
            .symbol(id)
    }

    /// Open one immutable core arena with a fresh invariant brand. Checks mint
    /// local handles that cannot escape or be used with a different arena.
    ///
    /// ```compile_fail
    /// use ts_arena::{Counters, StorageBuilder, Node, StorageScope};
    /// let counters = Counters::new();
    /// let mut builder = StorageBuilder::<Node<u32>>::new(std::sync::Arc::from(&b"x"[..]), &counters);
    /// let id = builder.push(Node::new(1, 7));
    /// let mut scope = StorageScope::new();
    /// scope.insert(builder.finish());
    /// let escaped = scope.with_core_arena(id.arena(), |local| local.check(id).unwrap());
    /// ```
    ///
    /// ```compile_fail
    /// use ts_arena::{Counters, StorageBuilder, Node, StorageScope};
    /// let counters = Counters::new();
    /// let mut first = StorageBuilder::<Node<u32>>::new(std::sync::Arc::from(&b"x"[..]), &counters);
    /// let mut second = StorageBuilder::<Node<u32>>::new(std::sync::Arc::from(&b"y"[..]), &counters);
    /// let first_id = first.push(Node::new(1, 7));
    /// let second_id = second.push(Node::new(1, 8));
    /// let mut scope = StorageScope::new();
    /// scope.insert(first.finish()); scope.insert(second.finish());
    /// scope.with_core_arena(first_id.arena(), |first| {
    ///     let local = first.check(first_id).unwrap();
    ///     scope.with_core_arena(second_id.arena(), |second| second.get(local));
    /// });
    /// ```
    pub fn with_core_arena<R>(
        &self,
        arena: ArenaId,
        operation: impl for<'brand> FnOnce(StorageLocalArena<'brand, '_, N>) -> R,
    ) -> Result<R, Error> {
        let file = self.nodes.get(&arena).ok_or(Error::WrongOwner)?;
        if file.core.id != arena {
            return Err(Error::WrongOwner);
        }
        Ok(operation(StorageLocalArena {
            arena: &file.core,
            brand: PhantomData,
        }))
    }
}

type Brand<'brand> = PhantomData<fn(&'brand ()) -> &'brand ()>;

/// One immutable core arena, with an invariant brand private to this callback.
pub struct StorageLocalArena<'brand, 'owner, N: NodeRecord> {
    arena: &'owner Arena<N>,
    brand: Brand<'brand>,
}

/// Proof of an owner-and-bounds check within one fresh arena scope.
#[derive(Clone, Copy, Debug)]
pub struct LocalNode<'brand> {
    slot: u32,
    brand: Brand<'brand>,
}

impl LocalNode<'_> {
    /// The checked owner-local slot. A slot alone carries no ownership proof.
    #[inline]
    pub fn slot(self) -> u32 {
        self.slot
    }
}

/// Exclusive core access with an invariant brand private to one callback.
///
/// Raw imports validate owner before slot; local access retains safe bounds
/// checks. This scope cannot replace the builder or grow/shrink its core arena.
/// Reads, writes and general views borrow the scope, so immutable borrows cannot
/// overlap mutation. This primitive proves identity only, not syntax validity;
/// AST layers must restrict writes that would invalidate their graph proof.
pub struct CoreScopeMut<'brand, 'owner, N: NodeRecord, S = ()> {
    builder: &'owner mut StorageBuilder<N, S>,
    brand: Brand<'brand>,
}

impl<'brand, 'owner, N: NodeRecord, S> CoreScopeMut<'brand, 'owner, N, S> {
    #[inline]
    pub(crate) fn new(builder: &'owner mut StorageBuilder<N, S>) -> Self {
        Self {
            builder,
            brand: PhantomData,
        }
    }

    #[inline]
    pub fn check(&self, id: NodeId) -> Result<LocalNode<'brand>, Error> {
        self.builder.core_node(id)?;
        Ok(LocalNode {
            slot: id.slot(),
            brand: PhantomData,
        })
    }

    /// Import a core-local child word after its enclosing encoding establishes
    /// this owner as its namespace. Null and escape tags must be decoded by the
    /// caller; zero and out-of-range slots are rejected here.
    #[inline]
    pub fn check_slot(&self, slot: u32) -> Result<LocalNode<'brand>, Error> {
        self.resolve_slot(slot).map(|(local, _)| local)
    }

    /// Check an owner-local word and borrow its header with one page lookup.
    /// It has the same namespace precondition as `check_slot`.
    #[inline]
    pub fn resolve_slot(&self, slot: u32) -> Result<(LocalNode<'brand>, &N), Error> {
        let node = self.builder.core_slot(slot)?;
        Ok((
            LocalNode {
                slot,
                brand: PhantomData,
            },
            node,
        ))
    }

    #[inline]
    pub fn get(&self, local: LocalNode<'brand>) -> &N {
        self.builder
            .core_slot(local.slot)
            .expect("branded core slot was validated")
    }

    #[inline]
    pub fn get_mut(&mut self, local: LocalNode<'brand>) -> &mut N {
        self.builder
            .core_slot_mut(local.slot)
            .expect("branded core slot was validated")
    }

    /// Split one checked header from its owner's payload store and source.
    /// No owner routing, allocation or retaining-handle clone is performed.
    #[inline]
    pub fn node_store_and_source_mut(
        &mut self,
        local: LocalNode<'brand>,
    ) -> (&mut N, &mut N::Store, &SourceText) {
        self.builder
            .core_slot_store_and_source_mut(local.slot)
            .expect("branded core slot was validated")
    }

    /// Explicitly reconstruct an ordinary, non-retaining identity.
    #[inline]
    pub fn id(&self, local: LocalNode<'brand>) -> NodeId {
        NodeId::new(self.builder.id().arena(), local.slot)
    }

    #[inline]
    pub fn store(&self) -> &N::Store {
        self.builder.store()
    }

    #[inline]
    pub fn source(&self) -> &SourceText {
        self.builder.view().source()
    }

    #[inline]
    pub fn auxiliary_arena(&self) -> ArenaId {
        self.builder.view().auxiliary_arena()
    }

    /// Resolve a non-null core auxiliary word in this owner's auxiliary
    /// namespace. Encoded foreign/lazy escapes must use the general view.
    #[inline]
    pub fn auxiliary_slot(&self, slot: u32) -> Result<&N::CoreAux, Error> {
        self.builder.core_auxiliary_slot(slot)
    }

    /// Checked general lookup for lazy records and retained foreign owners.
    /// This view does not mint local handles or retain the unpublished builder.
    #[inline]
    pub fn view(&self) -> StorageView<'_, N, S> {
        self.builder.view()
    }
}

impl<'brand, 'owner, N: NodeRecord> StorageLocalArena<'brand, 'owner, N> {
    pub fn check(&self, id: NodeId) -> Result<LocalNode<'brand>, Error> {
        self.arena.get(id.arena(), id.slot())?;
        Ok(LocalNode {
            slot: id.slot(),
            brand: PhantomData,
        })
    }

    /// Repeated reads elide owner validation and retain safe bounds checks.
    pub fn get(&self, local: LocalNode<'brand>) -> &'owner N {
        self.arena
            .get_slot(local.slot)
            .expect("branded slot was validated")
    }

    pub fn id(&self, local: LocalNode<'brand>) -> NodeId {
        NodeId::new(self.arena.id, local.slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Counters, Node};
    use std::{
        panic::{catch_unwind, AssertUnwindSafe},
        sync::Arc,
    };

    #[test]
    fn mutable_scope_checks_owner_before_slot_and_preserves_page_boundary_mutations() {
        let counters = Counters::new();
        let bytes: Arc<[u8]> = Arc::from(b"source".as_slice());
        let mut builder = StorageBuilder::<Node<u32>>::new(bytes.clone(), &counters);
        let ids: Vec<_> = (0..1_025)
            .map(|index| builder.push(Node::new(1, index)))
            .collect();
        let foreign = StorageBuilder::<Node<()>>::new(Arc::from([]), &counters);
        let missing = NodeId::from_parts(builder.id().arena(), u32::MAX).unwrap();
        let missing_foreign = NodeId::from_parts(foreign.id().arena(), u32::MAX).unwrap();
        let pointer = std::ptr::from_ref(builder.core_node(ids[510]).unwrap());
        let baseline = counters.snapshot();
        let source_refs = Arc::strong_count(&bytes);
        builder.with_core_scope(|mut scope| {
            assert!(matches!(scope.check(missing), Err(Error::InvalidSlot)));
            assert!(matches!(
                scope.check(missing_foreign),
                Err(Error::WrongOwner)
            ));
            for invalid in [0, 1_026, u32::MAX] {
                assert!(matches!(scope.check_slot(invalid), Err(Error::InvalidSlot)));
                assert!(matches!(
                    scope.resolve_slot(invalid),
                    Err(Error::InvalidSlot)
                ));
            }
            for &index in &[
                0, 1, 2, 5, 6, 13, 14, 29, 30, 61, 62, 125, 126, 253, 254, 509, 510, 765, 766,
                1_021, 1_024,
            ] {
                let local = scope.check(ids[index]).unwrap();
                assert_eq!(local.slot(), index as u32 + 1);
                assert_eq!(scope.id(local), ids[index]);
                assert_eq!(scope.get(local).data, index as u32);
                scope.get_mut(local).data += 10_000;
                let (decoded, header) = scope.resolve_slot(local.slot()).unwrap();
                assert_eq!(scope.id(decoded), ids[index]);
                assert_eq!(header.data, index as u32 + 10_000);
            }
            let local = scope.check_slot(511).unwrap();
            assert_eq!(std::ptr::from_ref(scope.get(local)), pointer);
            assert_eq!(counters.snapshot(), baseline);
            assert_eq!(Arc::strong_count(&bytes), source_refs);
        });
        assert_eq!(builder.core_node(ids[510]).unwrap().data, 10_510);
        assert_eq!(builder.core_node(ids[511]).unwrap().data, 511);
        assert_eq!(counters.snapshot(), baseline);
        assert_eq!(Arc::strong_count(&bytes), source_refs);
    }

    #[derive(Debug)]
    struct StoredValue {
        index: usize,
    }

    impl NodeRecord for StoredValue {
        type CoreAux = u32;
        type Aux = ();
        type Store = [u32; 2];
        fn storage_kind(&self) -> u32 {
            0
        }
        fn storage_reparsed(&self) -> bool {
            false
        }
    }

    #[test]
    fn mutable_scope_splits_header_payload_and_source_and_does_not_rollback_on_unwind() {
        let counters = Counters::new();
        let mut builder =
            StorageBuilder::<StoredValue>::new(Arc::from(b"source".as_slice()), &counters);
        let id = builder.push(StoredValue { index: 0 });
        let auxiliary = builder.push_aux(42);
        let failed = catch_unwind(AssertUnwindSafe(|| {
            builder.with_core_scope(|mut scope| {
                let local = scope.check(id).unwrap();
                assert_eq!(scope.auxiliary_arena(), auxiliary.arena());
                assert_eq!(scope.auxiliary_slot(auxiliary.slot()), Ok(&42));
                for invalid in [0, 2, u32::MAX] {
                    assert_eq!(scope.auxiliary_slot(invalid), Err(Error::InvalidSlot));
                }
                let (header, store, source) = scope.node_store_and_source_mut(local);
                header.index = 1;
                store[header.index] = source.len() as u32;
                assert_eq!(scope.store(), &[0, 6]);
                assert_eq!(scope.source().as_bytes(), b"source");
                panic!("the owner must decide whether a failed binding is discarded");
            });
        }));
        assert!(failed.is_err());
        builder.with_core_scope(|mut scope| {
            let local = scope.check(id).unwrap();
            assert_eq!(scope.get(local).index, 1);
            assert_eq!(scope.store(), &[0, 6]);
            scope.get_mut(local).index = 0;
        });
        assert_eq!(builder.core_node(id).unwrap().index, 0);
    }

    #[test]
    fn mutable_scope_keeps_lazy_and_imported_records_at_the_general_boundary() {
        let counters = Counters::new();
        let mut foreign = StorageBuilder::<Node<()>>::new(Arc::from([]), &counters);
        let foreign_id = foreign.push(Node::new(2, ()));
        let mut builder = StorageBuilder::<Node<()>>::new(Arc::from([]), &counters);
        let parent = builder.push(Node::new(1, ()));
        builder.retain_file(foreign.finish());
        builder.with_core_scope(|mut scope| {
            assert!(matches!(scope.check(foreign_id), Err(Error::WrongOwner)));
            assert_eq!(scope.view().node(foreign_id).unwrap().kind, 2);
            let local = scope.check(parent).unwrap();
            let lazy = scope
                .view()
                .jsdoc(parent, |transaction| {
                    Ok(vec![transaction.push(Node::new(3, ()))])
                })
                .unwrap()[0];
            assert!(matches!(scope.check(lazy), Err(Error::WrongOwner)));
            assert_eq!(scope.view().node(lazy).unwrap().kind, 3);
            scope.get_mut(local).kind = 4;
            assert_eq!(scope.view().core_node(parent).unwrap().kind, 4);
        });
        assert!(!builder.is_core_only());
    }
}
