//! Core auxiliary identities select compact rows; lazy records keep full values.
use crate::compact::{lists::CompactNodes, CompactSlice, CoreStore, RowPages, StoredNode};
use crate::{AstStorageData, NodeList, NodeSlice};
use hashbrown::HashMap;
use tsr_arena::{ArenaId, AuxId, AuxiliaryRead, Error, StorageRead, StorageView};
use tsr_core::TextRange;

const COLD: u32 = 0;
const LIST: u32 = 1;
const FOREIGN_LIST: u32 = 2;
const BACKING: u32 = 3;

/// A physical core slot retains the public mixed auxiliary identity sequence.
#[derive(Debug)]
#[repr(C)]
pub struct StoredAux {
    kind: u32,
    row: u32,
}

#[derive(Default)]
struct ListRow {
    loc: TextRange,
    backing: u32,
    start: u32,
    len: u32,
    modifier_flags: u32,
}
#[derive(Default)]
struct BackingRow {
    start: u32,
    len: u32,
}

#[derive(Default)]
pub(crate) struct AuxStore {
    lists: RowPages<ListRow>,
    backings: RowPages<BackingRow>,
    cold: Vec<AstStorageData>,
    foreign: HashMap<u32, AuxId, tsr_arena::hash::FastState>,
}

pub(crate) enum AuxValue<'a> {
    List(NodeList),
    CompactNodes(CompactNodes),
    Full(&'a AstStorageData),
}

impl AuxStore {
    /// Admit only list representations whose links stay in the core owner.
    /// Cold metadata and wide compact backing offsets do not require ID routing.
    /// Inspect the exceptional collection, not every ordinary list/backing row.
    pub(crate) fn local_lists_only(&self) -> bool {
        self.foreign.is_empty()
            && self
                .cold
                .iter()
                .all(|value| !matches!(value, AstStorageData::List(_) | AstStorageData::Nodes(_)))
    }

    /// Copy the local backing word and range without reconstructing an AuxId.
    /// A returned descriptor still requires backing-kind and range validation.
    #[inline]
    pub(crate) fn local_list(&self, record: &StoredAux) -> Option<CompactSlice> {
        if record.kind != LIST {
            return None;
        }
        let row = self.lists.get(record.row)?;
        Some(CompactSlice {
            backing: row.backing,
            start: row.start,
            len: row.len,
        })
    }

    /// Read the stored flags, including explicit values unrelated to edge kinds.
    #[inline]
    pub(crate) fn local_modifier_flags(&self, record: &StoredAux) -> Option<u32> {
        if record.kind != LIST {
            return None;
        }
        Some(self.lists.get(record.row)?.modifier_flags)
    }

    /// Keep full-width cold offsets intact; edge bounds are checked by the caller.
    #[inline]
    pub(crate) fn local_backing(&self, record: &StoredAux) -> Option<CompactNodes> {
        match record.kind {
            BACKING => {
                let row = self.backings.get(record.row)?;
                Some(CompactNodes {
                    start: row.start as usize,
                    len: row.len,
                })
            }
            COLD => match self.cold.get(record.row as usize)? {
                AstStorageData::CompactNodes(backing) => Some(*backing),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn push(&mut self, value: AstStorageData, owner: ArenaId) -> StoredAux {
        match value {
            AstStorageData::List(list) => {
                let row = self.lists.len();
                let (kind, backing) = self.encode_backing(row, list.nodes().backing, owner);
                let nodes = list.nodes();
                let stored = ListRow {
                    loc: list.loc(),
                    backing,
                    start: nodes.start,
                    len: nodes.len,
                    modifier_flags: list.modifier_flags(),
                };
                assert_eq!(self.lists.push(stored), row, "compact list row identity");
                StoredAux { kind, row }
            }
            AstStorageData::CompactNodes(backing) if u32::try_from(backing.start).is_ok() => {
                let row = self.backings.push(BackingRow {
                    start: u32::try_from(backing.start).expect("checked compact backing offset"),
                    len: backing.len,
                });
                StoredAux { kind: BACKING, row }
            }
            cold => self.push_cold(cold),
        }
    }
    fn push_cold(&mut self, value: AstStorageData) -> StoredAux {
        let row = u32::try_from(self.cold.len()).expect("cold auxiliary ordinal space exhausted");
        self.cold.push(value);
        StoredAux { kind: COLD, row }
    }
    fn encode_backing(&mut self, row: u32, backing: Option<AuxId>, owner: ArenaId) -> (u32, u32) {
        match backing {
            None => (LIST, 0),
            Some(id) if id.arena() == owner => (LIST, id.slot()),
            Some(id) => {
                self.foreign.insert(row, id);
                (FOREIGN_LIST, 0)
            }
        }
    }
    fn list(&self, record: &StoredAux, owner: ArenaId) -> NodeList {
        let row = self.lists.get(record.row).expect("compact list ordinal");
        let backing = if record.kind == FOREIGN_LIST {
            Some(
                *self
                    .foreign
                    .get(&record.row)
                    .expect("compact foreign list backing"),
            )
        } else if row.backing == 0 {
            None
        } else {
            Some(AuxId::from_parts(owner, row.backing).expect("nonzero local backing"))
        };
        let mut list = NodeList::new(
            row.loc,
            NodeSlice {
                backing,
                start: row.start,
                len: row.len,
            },
        );
        list.set_modifier_flags(row.modifier_flags);
        list
    }
    pub(crate) fn value<'a>(&'a self, record: &StoredAux, owner: ArenaId) -> AuxValue<'a> {
        match record.kind {
            LIST | FOREIGN_LIST => AuxValue::List(self.list(record, owner)),
            BACKING => {
                let row = self
                    .backings
                    .get(record.row)
                    .expect("compact backing ordinal");
                AuxValue::CompactNodes(CompactNodes {
                    start: row.start as usize,
                    len: row.len,
                })
            }
            COLD => AuxValue::Full(&self.cold[record.row as usize]),
            _ => unreachable!("stored auxiliary kind"),
        }
    }
    pub(crate) fn full(&self, record: &StoredAux) -> Option<&AstStorageData> {
        (record.kind == COLD).then(|| &self.cold[record.row as usize])
    }
    pub(crate) fn full_mut(&mut self, record: &StoredAux) -> Result<&mut AstStorageData, Error> {
        if record.kind != COLD {
            return Err(Error::InvalidGraph);
        }
        Ok(&mut self.cold[record.row as usize])
    }
    pub(crate) fn list_mut(
        &mut self,
        record: &mut StoredAux,
        owner: ArenaId,
    ) -> Result<&mut NodeList, Error> {
        if matches!(record.kind, LIST | FOREIGN_LIST) {
            let list = self.list(record, owner);
            let promoted = self.push_cold(AstStorageData::List(list));
            if record.kind == FOREIGN_LIST {
                self.foreign.remove(&record.row);
            }
            *record = promoted;
        }
        match self.full_mut(record)? {
            AstStorageData::List(list) => Ok(list),
            _ => Err(Error::InvalidGraph),
        }
    }
    pub(crate) fn set_nodes(
        &mut self,
        record: &mut StoredAux,
        nodes: NodeSlice,
        owner: ArenaId,
    ) -> Result<(), Error> {
        if matches!(record.kind, LIST | FOREIGN_LIST) {
            if record.kind == FOREIGN_LIST {
                self.foreign.remove(&record.row);
            }
            let (kind, backing) = self.encode_backing(record.row, nodes.backing, owner);
            let row = self
                .lists
                .get_mut(record.row)
                .expect("compact list ordinal");
            (row.backing, row.start, row.len) = (backing, nodes.start, nodes.len);
            record.kind = kind;
            Ok(())
        } else {
            self.list_mut(record, owner)?.set_nodes(nodes);
            Ok(())
        }
    }
    pub(crate) fn set_location(
        &mut self,
        record: &mut StoredAux,
        loc: TextRange,
    ) -> Result<(), Error> {
        if matches!(record.kind, LIST | FOREIGN_LIST) {
            self.lists
                .get_mut(record.row)
                .expect("compact list ordinal")
                .loc = loc;
        } else {
            match self.full_mut(record)? {
                AstStorageData::List(list) => list.set_loc(loc),
                _ => return Err(Error::InvalidGraph),
            }
        }
        Ok(())
    }
    pub(crate) fn set_flags(&mut self, record: &mut StoredAux, flags: u32) -> Result<(), Error> {
        if matches!(record.kind, LIST | FOREIGN_LIST) {
            self.lists
                .get_mut(record.row)
                .expect("compact list ordinal")
                .modifier_flags = flags;
        } else {
            match self.full_mut(record)? {
                AstStorageData::List(list) => list.set_modifier_flags(flags),
                _ => return Err(Error::InvalidGraph),
            }
        }
        Ok(())
    }
}

pub(crate) enum AuxRead<'a> {
    Core {
        record: &'a StoredAux,
        store: &'a CoreStore,
        owner: ArenaId,
    },
    Full(StorageRead<'a, AstStorageData>),
}
impl<'a> AuxRead<'a> {
    pub(crate) fn resolved(
        record: AuxiliaryRead<'a, StoredNode>,
        owner: StorageView<'a, StoredNode>,
    ) -> Self {
        Self::with_store(record, owner.store(), owner.auxiliary_arena())
    }
    pub(crate) fn with_store(
        record: AuxiliaryRead<'a, StoredNode>,
        store: &'a CoreStore,
        owner: ArenaId,
    ) -> Self {
        match record {
            AuxiliaryRead::Core(record) => Self::Core {
                record,
                store,
                owner,
            },
            AuxiliaryRead::Lazy(value) => Self::Full(value),
        }
    }
    pub(crate) fn value(&self) -> AuxValue<'_> {
        match self {
            Self::Core {
                record,
                store,
                owner,
            } => store.auxiliary.value(record, *owner),
            Self::Full(record) => AuxValue::Full(record),
        }
    }
    pub(crate) fn full(&self) -> Option<&AstStorageData> {
        match self {
            Self::Core { record, store, .. } => store.auxiliary.full(record),
            Self::Full(record) => Some(record),
        }
    }
    pub(crate) fn full_borrowed(&self) -> Option<&'a AstStorageData> {
        match self {
            Self::Core { record, store, .. } => store.auxiliary.full(record),
            Self::Full(record) => record.as_borrowed(),
        }
    }
    pub(crate) fn into_full(self) -> Result<StorageRead<'a, AstStorageData>, Error> {
        match self {
            Self::Core { record, store, .. } => store
                .auxiliary
                .full(record)
                .map(StorageRead::borrowed)
                .ok_or(Error::InvalidGraph),
            Self::Full(record) => Ok(record),
        }
    }
    pub(crate) fn list(&self) -> Result<NodeList, Error> {
        match self.value() {
            AuxValue::List(list) => Ok(list),
            AuxValue::Full(AstStorageData::List(list)) => Ok(list.clone()),
            _ => Err(Error::InvalidGraph),
        }
    }
}

impl AuxStore {
    /// Known structural bytes and the count of nested std collections whose
    /// allocation extent is not exposed.
    pub(crate) fn structural_bytes_with(
        &self,
        census: &mut tsr_arena::StorageCensus,
    ) -> (usize, usize) {
        let mut known = self.lists.structural_bytes()
            + self.backings.structural_bytes()
            + self.cold.capacity() * size_of::<AstStorageData>()
            + self.foreign.allocation_size();
        let mut unmeasured = 0;
        for data in &self.cold {
            match data {
                AstStorageData::FallbackNode(node) => {
                    known += census.allocation(
                        std::sync::Arc::as_ptr(node) as usize,
                        16 + size_of::<crate::Node>(),
                    );
                    // The full enum may retain boxed payloads and nested backing.
                    // Its header alone cannot certify a complete census.
                    unmeasured += 1;
                }
                AstStorageData::List(_)
                | AstStorageData::CompactNodes(_)
                | AstStorageData::File(_) => {}
                AstStorageData::Nodes(nodes) => {
                    known += nodes.len() * size_of::<Option<crate::NodeId>>();
                }
                AstStorageData::Text(texts) => {
                    known += texts.len() * size_of::<crate::JsString>();
                    known += texts
                        .iter()
                        .map(|text| census.text(text.backing_bytes()))
                        .sum::<usize>();
                }
                AstStorageData::SourceMetadata(_) => unmeasured += 1,
                AstStorageData::SourceFiles(files) => unmeasured += files.len(),
            }
        }
        (known, unmeasured)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tsr_arena::{Counters, StorageBuilder};

    #[test]
    fn compact_auxiliary_layouts_and_full_local_slot_are_preserved() {
        assert_eq!(std::mem::size_of::<StoredAux>(), 8);
        assert_eq!(std::mem::size_of::<ListRow>(), 24);
        assert_eq!(std::mem::size_of::<BackingRow>(), 8);
        // The foreign map's zero-sized fast hasher took 16 bytes off the store.
        assert_eq!(std::mem::size_of::<AuxStore>(), 120);
        assert_eq!(std::mem::size_of::<AstStorageData>(), 40);
        let builder =
            StorageBuilder::<tsr_arena::Node<()>>::new(Vec::new().into(), &Counters::new());
        let owner = builder.view().auxiliary_arena();
        let mut store = AuxStore::default();
        for nodes in [
            NodeSlice::empty(),
            NodeSlice::missing(),
            NodeSlice {
                backing: Some(AuxId::from_parts(owner, u32::MAX).unwrap()),
                start: 5,
                len: 7,
            },
        ] {
            let original = NodeList::new(TextRange::new(i64::MIN, i64::MAX), nodes);
            let record = store.push(AstStorageData::List(original.clone()), owner);
            assert_eq!(store.list(&record, owner), original);
            assert_eq!(record.kind, LIST);
            assert!(store.foreign.is_empty());
        }
    }

    #[test]
    fn cold_promotion_releases_foreign_escape_and_preserves_the_header_once() {
        let counters = Counters::new();
        let first = StorageBuilder::<tsr_arena::Node<()>>::new(Vec::new().into(), &counters);
        let second = StorageBuilder::<tsr_arena::Node<()>>::new(Vec::new().into(), &counters);
        let owner = first.view().auxiliary_arena();
        let foreign = AuxId::from_parts(second.view().auxiliary_arena(), u32::MAX).unwrap();
        let nodes = NodeSlice {
            backing: Some(foreign),
            start: 3,
            len: 0,
        };
        let mut store = AuxStore::default();
        let mut record = store.push(
            AstStorageData::List(NodeList::new(TextRange::new(-1, 4), nodes)),
            owner,
        );
        assert_eq!(record.kind, FOREIGN_LIST);
        assert_eq!(store.foreign.len(), 1);
        store
            .list_mut(&mut record, owner)
            .unwrap()
            .set_modifier_flags(17);
        assert_eq!(record.kind, COLD);
        assert!(store.foreign.is_empty());
        assert_eq!(store.lists.len(), 1); // Abandoned row is charged until owner drop.
        assert_eq!(store.cold.len(), 1);
        assert_eq!(store.list_mut(&mut record, owner).unwrap().nodes(), nodes);
        store
            .set_location(&mut record, TextRange::new(2, 8))
            .unwrap();
        store.set_flags(&mut record, 19).unwrap();
        assert_eq!(store.cold.len(), 1);
        let AstStorageData::List(list) = store.full(&record).unwrap() else {
            panic!("promoted list")
        };
        assert_eq!(list.nodes(), nodes);
        assert_eq!(list.loc(), TextRange::new(2, 8));
        assert_eq!(list.modifier_flags(), 19);
    }

    #[test]
    fn wide_backing_and_rejected_mutation_do_not_truncate_or_promote() {
        let builder =
            StorageBuilder::<tsr_arena::Node<()>>::new(Vec::new().into(), &Counters::new());
        let owner = builder.view().auxiliary_arena();
        let mut store = AuxStore::default();
        let mut ordinary = store.push(
            AstStorageData::CompactNodes(CompactNodes { start: 42, len: 8 }),
            owner,
        );
        assert_eq!(ordinary.kind, BACKING);
        let local = store.local_backing(&ordinary).unwrap();
        assert_eq!((local.start, local.len), (42, 8));
        assert!(store.local_list(&ordinary).is_none());
        assert!(matches!(
            store.list_mut(&mut ordinary, owner),
            Err(Error::InvalidGraph)
        ));
        assert_eq!(ordinary.kind, BACKING);
        assert!(store.cold.is_empty());
        let wide = store.push(
            AstStorageData::CompactNodes(CompactNodes {
                start: usize::MAX,
                len: 1,
            }),
            owner,
        );
        assert_eq!(wide.kind, COLD);
        let local = store.local_backing(&wide).unwrap();
        assert_eq!((local.start, local.len), (usize::MAX, 1));
        assert!(store.local_lists_only());
        assert!(
            matches!(store.value(&wide, owner), AuxValue::Full(AstStorageData::CompactNodes(value)) if value.start == usize::MAX && value.len == 1)
        );
    }

    #[test]
    fn local_list_descriptors_preserve_nil_missing_and_slice_offsets() {
        let builder =
            StorageBuilder::<tsr_arena::Node<()>>::new(Vec::new().into(), &Counters::new());
        let owner = builder.view().auxiliary_arena();
        let mut store = AuxStore::default();
        for nodes in [
            NodeSlice::empty(),
            NodeSlice::missing(),
            NodeSlice {
                backing: Some(AuxId::from_parts(owner, u32::MAX).unwrap()),
                start: 255,
                len: 3,
            },
        ] {
            let record = store.push(
                AstStorageData::List(NodeList::new(TextRange::new(1, 4), nodes)),
                owner,
            );
            let local = store.local_list(&record).unwrap();
            assert_eq!(local.backing, nodes.backing.map_or(0, AuxId::slot));
            assert_eq!((local.start, local.len), (nodes.start, nodes.len));
            assert!(store.local_backing(&record).is_none());
        }
        assert!(store.local_lists_only());
        assert!(store
            .local_list(&StoredAux {
                kind: LIST,
                row: u32::MAX
            })
            .is_none());
        assert!(store
            .local_backing(&StoredAux {
                kind: BACKING,
                row: u32::MAX
            })
            .is_none());
        assert!(store
            .local_backing(&StoredAux {
                kind: COLD,
                row: u32::MAX
            })
            .is_none());
    }

    #[test]
    fn local_list_eligibility_rejects_foreign_and_promoted_links_but_allows_metadata() {
        let counters = Counters::new();
        let first = StorageBuilder::<tsr_arena::Node<()>>::new(Vec::new().into(), &counters);
        let second = StorageBuilder::<tsr_arena::Node<()>>::new(Vec::new().into(), &counters);
        let owner = first.view().auxiliary_arena();
        let mut store = AuxStore::default();
        let text = store.push(AstStorageData::Text(Box::new([])), owner);
        let file = store.push(AstStorageData::File(crate::FileInfo::default()), owner);
        assert!(store.local_lists_only());
        assert!(store.local_list(&text).is_none());
        assert!(store.local_backing(&file).is_none());
        let mut record = store.push(
            AstStorageData::List(NodeList::new(
                TextRange::new(0, 0),
                NodeSlice {
                    backing: Some(AuxId::from_parts(second.view().auxiliary_arena(), 1).unwrap()),
                    start: 0,
                    len: 0,
                },
            )),
            owner,
        );
        assert!(!store.local_lists_only());
        assert!(store.local_list(&record).is_none());
        store
            .set_nodes(&mut record, NodeSlice::missing(), owner)
            .unwrap();
        assert!(store.local_lists_only());
        assert_eq!(store.local_list(&record).unwrap().start, 1);
        store
            .list_mut(&mut record, owner)
            .unwrap()
            .set_loc(TextRange::new(4, 8));
        assert!(!store.local_lists_only());
        assert!(store.local_list(&record).is_none());

        let mut full_nodes = AuxStore::default();
        let backing = full_nodes.push(AstStorageData::Nodes(Box::new([None])), owner);
        assert!(!full_nodes.local_lists_only());
        assert!(full_nodes.local_backing(&backing).is_none());
    }
}
