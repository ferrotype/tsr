//! Physical syntax edges: local core slots share fixed 256-word pages.
//!
//! Backing descriptors retain their auxiliary identities. Exceptional edges
//! retain full non-owning IDs; the enclosing owner retains their dependencies.

use crate::NodeId;
use std::ops::Range;
use tsr_arena::{ArenaId, Error};

const PAGE_WORDS: usize = 256;
const ESCAPE: u32 = u32::MAX;

/// A physical backing in one owner's edge pages. The public syntax slice adds
/// its own offset and keeps its stable auxiliary identity separately.
#[derive(Clone, Copy, Debug)]
pub struct CompactNodes {
    pub(crate) start: usize,
    pub(crate) len: u32,
}

#[derive(Default)]
pub(crate) struct EdgePages {
    #[allow(clippy::vec_box)] // Directory growth must not copy existing edge pages.
    pages: Vec<Box<[u32; PAGE_WORDS]>>,
    len: usize,
    escapes:
        hashbrown::HashMap<usize, NodeId, std::hash::BuildHasherDefault<std::hash::DefaultHasher>>,
}

impl EdgePages {
    pub(crate) const fn empty() -> Self {
        Self {
            pages: Vec::new(),
            len: 0,
            escapes: hashbrown::HashMap::with_hasher(std::hash::BuildHasherDefault::new()),
        }
    }

    /// Scope eligibility checks this before treating stored words as local slots.
    pub(crate) fn has_escapes(&self) -> bool {
        !self.escapes.is_empty()
    }

    /// Known page/directory bytes plus the escape table's allocation; the
    /// second value counts entries a census cannot size and is always zero
    /// now that the escape table exposes its allocation size.
    pub(crate) fn storage_bytes(&self) -> (usize, usize) {
        (
            self.pages.capacity() * size_of::<Box<[u32; PAGE_WORDS]>>()
                + self.pages.len() * size_of::<[u32; PAGE_WORDS]>()
                + self.escapes.allocation_size(),
            0,
        )
    }

    /// Read a stored word without namespace decoding. Zero preserves a nil edge.
    /// Local callers must establish `!has_escapes()` before importing nonzero
    /// words as core slots; this method does not resolve an escape sentinel.
    #[inline]
    pub(crate) fn local_word(&self, index: usize) -> Option<u32> {
        if index >= self.len {
            return None;
        }
        Some(self.pages[index / PAGE_WORDS][index % PAGE_WORDS])
    }

    pub(crate) fn append(
        &mut self,
        owner: ArenaId,
        nodes: &[Option<NodeId>],
    ) -> Result<CompactNodes, Error> {
        self.append_iter(owner, nodes.iter().copied())
    }

    pub(crate) fn append_iter(
        &mut self,
        owner: ArenaId,
        nodes: impl ExactSizeIterator<Item = Option<NodeId>>,
    ) -> Result<CompactNodes, Error> {
        let len = u32::try_from(nodes.len()).map_err(|_| Error::InvalidSlot)?;
        self.len
            .checked_add(nodes.len())
            .ok_or(Error::InvalidSlot)?;
        let start = self.len;
        for node in nodes {
            let index = self.len;
            if index.is_multiple_of(PAGE_WORDS) {
                self.pages.push(Box::new([0; PAGE_WORDS]));
            }
            let word = self.encode(owner, index, node);
            self.pages[index / PAGE_WORDS][index % PAGE_WORDS] = word;
            self.len += 1;
        }
        Ok(CompactNodes { start, len })
    }

    fn encode(&mut self, owner: ArenaId, index: usize, node: Option<NodeId>) -> u32 {
        match node {
            None => 0,
            Some(node) if node.arena() == owner && node.slot() != ESCAPE => node.slot(),
            Some(node) => {
                self.escapes.insert(index, node);
                ESCAPE
            }
        }
    }

    #[allow(clippy::option_option)] // A present nil edge differs from an out-of-bounds index.
    pub(crate) fn get(&self, owner: ArenaId, index: usize) -> Option<Option<NodeId>> {
        let word = self.local_word(index)?;
        Some(match word {
            0 => None,
            ESCAPE => Some(
                *self
                    .escapes
                    .get(&index)
                    .expect("escaped edge has a full identity"),
            ),
            slot => {
                Some(NodeId::from_parts(owner, slot).expect("stored local edge slot is nonzero"))
            }
        })
    }

    pub(crate) fn valid_range(&self, range: Range<usize>) -> bool {
        range.start <= range.end && range.end <= self.len
    }

    pub(crate) fn iter(
        &self,
        owner: ArenaId,
        range: Range<usize>,
    ) -> impl DoubleEndedIterator<Item = Option<NodeId>> + ExactSizeIterator + '_ {
        assert!(
            self.valid_range(range.clone()),
            "validated compact edge range"
        );
        range.map(move |index| {
            self.get(owner, index)
                .expect("validated compact edge index")
        })
    }

    /// Change one existing cell without replacing any copied backing header.
    /// Encoding records identity only; the enclosing graph keeps its own
    /// validation boundary for newly written references.
    pub(crate) fn set(
        &mut self,
        owner: ArenaId,
        index: usize,
        node: Option<NodeId>,
    ) -> Result<(), Error> {
        if index >= self.len {
            return Err(Error::InvalidSlot);
        }
        self.escapes.remove(&index);
        let word = self.encode(owner, index, node);
        self.pages[index / PAGE_WORDS][index % PAGE_WORDS] = word;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> ArenaId {
        tsr_arena::StorageBuilder::<tsr_arena::Node<()>>::from_source_text(
            tsr_jsstring::SourceText::default(),
            &tsr_arena::Counters::new(),
        )
        .id()
        .arena()
    }

    #[test]
    fn pages_preserve_cross_page_nil_full_slot_and_foreign_edges() {
        let owner = owner();
        let foreign = NodeId::from_parts(self::owner(), 1).unwrap();
        let local = NodeId::from_parts(owner, 7).unwrap();
        let full = NodeId::from_parts(owner, u32::MAX).unwrap();
        let mut pages = EdgePages::default();
        let empty = pages.append(owner, &[]).unwrap();
        assert_eq!(empty.len, 0);
        assert!(pages.pages.is_empty());
        let prefix = pages.append(owner, &vec![Some(local); 255]).unwrap();
        let backing = pages
            .append(owner, &[None, Some(foreign), Some(full), Some(local)])
            .unwrap();
        assert_eq!(prefix.start, 0);
        assert_eq!(backing.start, 255);
        assert_eq!(pages.pages.len(), 2);
        assert_eq!(pages.escapes.len(), 2);
        assert_eq!(
            pages.iter(owner, 254..259).collect::<Vec<_>>(),
            vec![Some(local), None, Some(foreign), Some(full), Some(local)]
        );
        assert_eq!(
            pages.iter(owner, 255..259).rev().collect::<Vec<_>>(),
            vec![Some(local), Some(full), Some(foreign), None]
        );
        assert_eq!(pages.get(owner, 259), None);
        assert_eq!(pages.get(owner, usize::MAX), None);
        assert!(!pages.valid_range(259..260));
        assert!(pages.iter(owner, 259..259).next().is_none());
    }

    #[test]
    fn cell_changes_preserve_backing_aliases_and_remove_stale_escapes() {
        let owner = owner();
        let local = NodeId::from_parts(owner, 3).unwrap();
        let foreign = NodeId::from_parts(self::owner(), 3).unwrap();
        let full = NodeId::from_parts(owner, u32::MAX).unwrap();
        let mut pages = EdgePages::default();
        let backing = pages.append(owner, &[Some(foreign)]).unwrap();
        let copied = backing;
        for value in [Some(full), Some(local), None, Some(foreign)] {
            pages.set(owner, backing.start, value).unwrap();
            assert_eq!(pages.get(owner, copied.start), Some(value));
            assert_eq!(
                pages.escapes.len(),
                usize::from(matches!(value, Some(id) if id == full || id == foreign))
            );
            assert_eq!(
                pages.has_escapes(),
                value.is_some_and(|id| id == full || id == foreign)
            );
        }
        assert_eq!(pages.set(owner, 1, None), Err(Error::InvalidSlot));
        assert_eq!(pages.get(owner, 0), Some(Some(foreign)));
    }

    #[test]
    fn local_words_preserve_cross_page_nils_and_reject_out_of_range_indices() {
        let owner = owner();
        let local = NodeId::from_parts(owner, 7).unwrap();
        let mut pages = EdgePages::default();
        assert!(!pages.has_escapes());
        assert_eq!(pages.local_word(0), None);
        pages.append(owner, &vec![Some(local); 255]).unwrap();
        let backing = pages.append(owner, &[None, Some(local), None]).unwrap();
        assert!(!pages.has_escapes());
        assert_eq!((backing.start, backing.len), (255, 3));
        assert!(pages.valid_range(backing.start..backing.start + backing.len as usize));
        assert_eq!(pages.local_word(254), Some(7));
        assert_eq!(pages.local_word(255), Some(0));
        assert_eq!(pages.local_word(256), Some(7));
        assert_eq!(pages.local_word(257), Some(0));
        assert_eq!(pages.local_word(258), None);
        assert_eq!(pages.local_word(usize::MAX), None);
        assert!(pages.valid_range(258..258));
        assert!(!pages.valid_range(257..259));
        assert!(!pages.valid_range(Range {
            start: 258,
            end: 257
        }));
        assert!(!pages.valid_range(usize::MAX..usize::MAX));
    }
}
