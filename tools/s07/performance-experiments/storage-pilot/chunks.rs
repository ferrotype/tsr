//! Diagnostic whole-backing allocation into safe, typed contiguous chunks.
//!
//! Each completed backing occupies one range of one `Box<[u32]>`. Only the last
//! chunk's tail is reused; a new chunk holds `max(CHUNK, backing_length)` words.
//! Empty backings retain distinct descriptors without forcing a chunk allocation.
//! One nested scratch Vec stages edges and stays retained after publication.
//!
//! This standalone pilot validates a supplied node domain but does not retain AST
//! nodes, implement foreign imports, or provide the production `Option<NodeId>`
//! slice API. Its borrowed raw words use zero for nil and nonzero local slots.
//! Allocation replay does not reconstruct parser nesting or initial Vec capacity.
//! Chunk zero-initialization, unused tails, 12-byte backing descriptors, directory
//! capacity, separate 24-byte list headers, scratch and frames must all be charged.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    WrongOwner,
    InvalidNode,
    InvalidSlice,
    WrongFrame,
    Incomplete,
    Exhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeRef {
    pub owner: u64,
    pub slot: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocalSlice {
    backing: u32,
    start: u32,
    len: u32,
}

/// Non-owning slice identity. Copied/subsliced headers share mutable construction
/// backing; allocated-empty, nil and missing remain distinct representations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slice {
    owner: u64,
    local: LocalSlice,
}

impl Slice {
    pub fn is_nil(self) -> bool {
        self.local.backing == 0 && self.local.start == 0
    }
    pub fn is_missing(self) -> bool {
        self.local.backing == 0 && self.local.start == 1
    }
    pub fn len(self) -> usize {
        self.local.len as usize
    }
    pub fn is_empty(self) -> bool {
        self.local.len == 0
    }
    pub fn same(self, other: Self) -> bool {
        self.local.len == other.local.len
            && (self.is_empty()
                || (self.owner == other.owner
                    && self.local.backing == other.local.backing
                    && self.local.start == other.local.start))
    }
    pub fn slice(self, range: Range<usize>) -> Result<Self, Error> {
        if range.start > range.end || range.end > self.len() {
            return Err(Error::InvalidSlice);
        }
        let start = self
            .local
            .start
            .checked_add(u32::try_from(range.start).map_err(|_| Error::Exhausted)?)
            .ok_or(Error::Exhausted)?;
        Ok(Self {
            local: LocalSlice {
                start,
                len: u32::try_from(range.len()).map_err(|_| Error::Exhausted)?,
                ..self.local
            },
            ..self
        })
    }
}

/// No marker values: all u32 starts/lengths are available. Empty descriptors do
/// not reference their chunk field. The chunk index is zero-based.
#[derive(Clone, Copy, Debug)]
struct Backing {
    chunk: u32,
    start: u32,
    len: u32,
}

struct Chunk {
    words: Box<[u32]>,
    used: usize,
}

#[derive(Clone, Copy, Debug)]
struct Header {
    pos: i32,
    end: i32,
    nodes: LocalSlice,
    modifiers: u32,
}

pub const BACKING_BYTES: usize = std::mem::size_of::<Backing>();
pub const CHUNK_DIRECTORY_ENTRY_BYTES: usize = std::mem::size_of::<Chunk>();
pub const HEADER_BYTES: usize = std::mem::size_of::<Header>();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListId {
    owner: u64,
    index: u32,
}

/// Non-Copy token; a successful finish/abort invalidates its serial. Errors leave
/// the token usable, including an attempted outer finish while a child is open.
#[derive(Debug)]
pub struct Frame {
    owner: u64,
    serial: u64,
    depth: usize,
    start: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub edge_words: usize,
    pub edge_capacity: usize,
    pub edge_chunks: usize,
    pub edge_directory_capacity: usize,
    pub backings: usize,
    pub backing_capacity: usize,
    pub list_headers: usize,
    pub list_header_capacity: usize,
    pub scratch_capacity: usize,
    pub scratch_peak_len: usize,
    pub frame_capacity: usize,
}

pub struct Builder<const CHUNK: usize> {
    owner: u64,
    node_owner: u64,
    max_node: u32,
    chunks: Vec<Chunk>,
    backings: Vec<Backing>,
    headers: Vec<Header>,
    scratch: Vec<u32>,
    frames: Vec<u64>,
    next_serial: u64,
    scratch_peak_len: usize,
}

/// No mutable access or mutation capability survives consuming publication.
pub struct Published<const CHUNK: usize> {
    store: Builder<CHUNK>,
}

fn checked_backing_id(count: usize) -> Result<u32, Error> {
    u32::try_from(count)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or(Error::Exhausted)
}

fn checked_chunk_index(count: usize) -> Result<u32, Error> {
    u32::try_from(count).map_err(|_| Error::Exhausted)
}

/// Uses no sentinel: an empty endpoint at u32::MAX is representable. Widen only
/// after checking addition in the format's full local domain.
fn checked_local_range(start: u32, len: u32, used: usize) -> Result<Range<usize>, Error> {
    let end = start.checked_add(len).ok_or(Error::InvalidSlice)?;
    if end as usize > used {
        return Err(Error::InvalidSlice);
    }
    Ok(start as usize..end as usize)
}

impl<const CHUNK: usize> Builder<CHUNK> {
    pub fn new(node_owner: u64, max_node: u32) -> Self {
        assert_ne!(node_owner, 0, "pilot node domain must be nonzero");
        assert!(
            CHUNK != 0 && u32::try_from(CHUNK).is_ok(),
            "pilot chunk policy must fit a nonzero u32 word count"
        );
        let owner = NEXT_OWNER
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("pilot construction identities exhausted before reuse");
        Self {
            owner,
            node_owner,
            max_node,
            chunks: Vec::new(),
            backings: Vec::new(),
            headers: Vec::new(),
            scratch: Vec::new(),
            frames: Vec::new(),
            next_serial: 0,
            scratch_peak_len: 0,
        }
    }

    fn wrap(&self, local: LocalSlice) -> Slice {
        Slice {
            owner: if local.backing == 0 { 0 } else { self.owner },
            local,
        }
    }

    pub fn nil(&self) -> Slice {
        self.wrap(LocalSlice {
            backing: 0,
            start: 0,
            len: 0,
        })
    }

    pub fn missing(&self) -> Slice {
        self.wrap(LocalSlice {
            backing: 0,
            start: 1,
            len: 0,
        })
    }

    fn check_frame(&self, frame: &Frame) -> Result<(), Error> {
        if frame.owner != self.owner {
            return Err(Error::WrongOwner);
        }
        if self.frames.len() != frame.depth || self.frames.last() != Some(&frame.serial) {
            return Err(Error::WrongFrame);
        }
        Ok(())
    }

    pub fn begin(&mut self) -> Result<Frame, Error> {
        self.next_serial = self.next_serial.checked_add(1).ok_or(Error::Exhausted)?;
        self.frames.push(self.next_serial);
        Ok(Frame {
            owner: self.owner,
            serial: self.next_serial,
            depth: self.frames.len(),
            start: self.scratch.len(),
        })
    }

    fn check_node(&self, node: Option<NodeRef>) -> Result<u32, Error> {
        match node {
            None => Ok(0),
            Some(node) if node.owner != self.node_owner => Err(Error::WrongOwner),
            Some(node) if node.slot == 0 || node.slot > self.max_node => Err(Error::InvalidNode),
            Some(node) => Ok(node.slot),
        }
    }

    pub fn push(&mut self, frame: &Frame, node: Option<NodeRef>) -> Result<(), Error> {
        self.check_frame(frame)?;
        let slot = self.check_node(node)?;
        if self.scratch.len() - frame.start >= u32::MAX as usize {
            return Err(Error::Exhausted);
        }
        self.scratch.push(slot);
        self.scratch_peak_len = self.scratch_peak_len.max(self.scratch.len());
        Ok(())
    }

    pub fn finish(&mut self, frame: &Frame) -> Result<Slice, Error> {
        self.check_frame(frame)?;
        let backing_id = checked_backing_id(self.backings.len())?;
        let len = u32::try_from(self.scratch.len() - frame.start).map_err(|_| Error::Exhausted)?;
        let backing = if len == 0 {
            Backing {
                chunk: 0,
                start: 0,
                len: 0,
            }
        } else {
            let needs_chunk = self
                .chunks
                .last()
                .is_none_or(|chunk| chunk.words.len() - chunk.used < len as usize);
            let chunk_index = if needs_chunk {
                let index = checked_chunk_index(self.chunks.len())?;
                let capacity = CHUNK.max(len as usize);
                self.chunks.push(Chunk {
                    words: vec![0; capacity].into_boxed_slice(),
                    used: 0,
                });
                index
            } else {
                checked_chunk_index(self.chunks.len() - 1)?
            };
            let chunk = &mut self.chunks[chunk_index as usize];
            let start = u32::try_from(chunk.used).expect("chunk capacity fits the full u32 domain");
            let end = chunk.used + len as usize;
            chunk.words[chunk.used..end].copy_from_slice(&self.scratch[frame.start..]);
            chunk.used = end;
            Backing {
                chunk: chunk_index,
                start,
                len,
            }
        };
        self.backings.push(backing);
        self.scratch.truncate(frame.start);
        self.frames.pop();
        Ok(self.wrap(LocalSlice {
            backing: backing_id,
            start: 0,
            len,
        }))
    }

    pub fn abort(&mut self, frame: &Frame) -> Result<(), Error> {
        self.check_frame(frame)?;
        self.scratch.truncate(frame.start);
        self.frames.pop();
        Ok(())
    }

    /// Checks owner, backing, relative slice and physical chunk bounds once.
    /// Nil/missing and allocated-empty backings have no physical chunk range.
    fn range(&self, slice: Slice) -> Result<Option<(usize, Range<usize>)>, Error> {
        if slice.local.backing == 0 {
            if slice.owner != 0 || slice.local.len != 0 || slice.local.start > 1 {
                return Err(Error::InvalidSlice);
            }
            return Ok(None);
        }
        if slice.owner != self.owner {
            return Err(Error::WrongOwner);
        }
        let backing = self
            .backings
            .get(slice.local.backing as usize - 1)
            .ok_or(Error::InvalidSlice)?;
        checked_local_range(slice.local.start, slice.local.len, backing.len as usize)?;
        if backing.len == 0 {
            return Ok(None);
        }
        let chunk_index = backing.chunk as usize;
        let chunk = self.chunks.get(chunk_index).ok_or(Error::InvalidSlice)?;
        checked_local_range(backing.start, backing.len, chunk.used)?;
        let start = backing
            .start
            .checked_add(slice.local.start)
            .ok_or(Error::InvalidSlice)?;
        let range = checked_local_range(start, slice.local.len, chunk.used)?;
        if range.end > chunk.words.len() {
            return Err(Error::InvalidSlice);
        }
        Ok(Some((chunk_index, range)))
    }

    /// Borrow one checked contiguous backing/subslice. Words are local slots,
    /// with zero representing a nil edge; the borrow cannot outlive this store.
    pub fn words(&self, slice: Slice) -> Result<&[u32], Error> {
        match self.range(slice)? {
            None => Ok(&[]),
            Some((chunk, range)) => self.chunks[chunk]
                .words
                .get(range)
                .ok_or(Error::InvalidSlice),
        }
    }

    pub fn get(&self, slice: Slice, index: usize) -> Result<Option<NodeRef>, Error> {
        let slot = *self.words(slice)?.get(index).ok_or(Error::InvalidSlice)?;
        Ok((slot != 0).then_some(NodeRef {
            owner: self.node_owner,
            slot,
        }))
    }

    /// Copied/subsliced headers observe mutation through their shared backing.
    pub fn set(&mut self, slice: Slice, index: usize, value: Option<NodeRef>) -> Result<(), Error> {
        let (chunk_index, range) = self.range(slice)?.ok_or(Error::InvalidSlice)?;
        if index >= range.len() {
            return Err(Error::InvalidSlice);
        }
        let slot = self.check_node(value)?;
        self.chunks[chunk_index].words[range.start + index] = slot;
        Ok(())
    }

    pub fn new_list(&mut self, pos: i32, end: i32, nodes: Slice) -> Result<ListId, Error> {
        self.range(nodes)?;
        let index = u32::try_from(self.headers.len()).map_err(|_| Error::Exhausted)?;
        self.headers.push(Header {
            pos,
            end,
            nodes: nodes.local,
            modifiers: 0,
        });
        Ok(ListId {
            owner: self.owner,
            index,
        })
    }

    fn header(&self, id: ListId) -> Result<&Header, Error> {
        if id.owner != self.owner {
            return Err(Error::WrongOwner);
        }
        self.headers
            .get(id.index as usize)
            .ok_or(Error::InvalidSlice)
    }

    pub fn clone_list(&mut self, id: ListId) -> Result<ListId, Error> {
        let header = *self.header(id)?;
        let index = u32::try_from(self.headers.len()).map_err(|_| Error::Exhausted)?;
        self.headers.push(header);
        Ok(ListId {
            owner: self.owner,
            index,
        })
    }

    pub fn set_modifiers(&mut self, id: ListId, flags: u32) -> Result<(), Error> {
        self.header(id)?;
        self.headers[id.index as usize].modifiers = flags;
        Ok(())
    }

    pub fn list(&self, id: ListId) -> Result<(i32, i32, Slice, u32), Error> {
        let header = self.header(id)?;
        Ok((
            header.pos,
            header.end,
            self.wrap(header.nodes),
            header.modifiers,
        ))
    }

    pub fn stats(&self) -> Stats {
        Stats {
            edge_words: self.chunks.iter().map(|chunk| chunk.used).sum(),
            edge_capacity: self.chunks.iter().map(|chunk| chunk.words.len()).sum(),
            edge_chunks: self.chunks.len(),
            edge_directory_capacity: self.chunks.capacity(),
            backings: self.backings.len(),
            backing_capacity: self.backings.capacity(),
            list_headers: self.headers.len(),
            list_header_capacity: self.headers.capacity(),
            scratch_capacity: self.scratch.capacity(),
            scratch_peak_len: self.scratch_peak_len,
            frame_capacity: self.frames.capacity(),
        }
    }

    pub fn publish(self) -> Result<Published<CHUNK>, Error> {
        if !self.frames.is_empty() || !self.scratch.is_empty() {
            return Err(Error::Incomplete);
        }
        // Scratch and frame capacity remain owned and charged at this endpoint.
        Ok(Published { store: self })
    }
}

impl<const CHUNK: usize> Published<CHUNK> {
    pub fn words(&self, slice: Slice) -> Result<&[u32], Error> {
        self.store.words(slice)
    }
    pub fn get(&self, slice: Slice, index: usize) -> Result<Option<NodeRef>, Error> {
        self.store.get(slice, index)
    }
    pub fn list(&self, id: ListId) -> Result<(i32, i32, Slice, u32), Error> {
        self.store.list(id)
    }
    pub fn stats(&self) -> Stats {
        self.store.stats()
    }
    /// Visit backing descriptors already borrowed from this owner. No public
    /// identity is reminted or looked up again. Physical range and chunk bounds
    /// remain checked; arbitrary slice IDs still enter through `words`.
    ///
    /// This exposes local words, not retained AST nodes. The returned borrows
    /// cannot outlive the owner or coexist with mutable construction.
    ///
    /// ```compile_fail
    /// use tsr_s07_storage_pilot::chunks::{Builder, NodeRef};
    /// fn escape() -> &'static [u32] {
    ///     let mut builder = Builder::<4>::new(1, 1);
    ///     let frame = builder.begin().unwrap();
    ///     builder.push(&frame, Some(NodeRef { owner: 1, slot: 1 })).unwrap();
    ///     builder.finish(&frame).unwrap();
    ///     let owner = builder.publish().unwrap();
    ///     owner.owned_backings().next().unwrap().unwrap()
    /// }
    /// ```
    pub fn owned_backings(&self) -> impl ExactSizeIterator<Item = Result<&[u32], Error>> + '_ {
        self.store.backings.iter().map(|backing| {
            if backing.len == 0 {
                return Ok(&[][..]);
            }
            let chunk = self
                .store
                .chunks
                .get(backing.chunk as usize)
                .ok_or(Error::InvalidSlice)?;
            let range = checked_local_range(backing.start, backing.len, chunk.used)?;
            chunk.words.get(range).ok_or(Error::InvalidSlice)
        })
    }
    /// Visit every physical backing in completion order, including empty and
    /// otherwise unreferenced ones, with the public getter's checks per backing.
    /// The callback receives one contiguous slice; there is no page-span visitor.
    pub fn for_each_backing<'a>(&'a self, mut visit: impl FnMut(&'a [u32])) -> Result<(), Error> {
        for (index, backing) in self.store.backings.iter().enumerate() {
            let slice = self.store.wrap(LocalSlice {
                backing: checked_backing_id(index)?,
                start: 0,
                len: backing.len,
            });
            visit(self.words(slice)?);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(slot: u32) -> Option<NodeRef> {
        Some(NodeRef { owner: 1, slot })
    }

    fn add<const CHUNK: usize>(builder: &mut Builder<CHUNK>, words: &[u32]) -> Slice {
        let frame = builder.begin().unwrap();
        for &slot in words {
            builder
                .push(&frame, if slot == 0 { None } else { node(slot) })
                .unwrap();
        }
        builder.finish(&frame).unwrap()
    }

    #[test]
    fn owned_borrows_match_checked_reads_after_nested_construction_and_alias_writes() {
        let mut builder = Builder::<4>::new(1, 9);
        let parent = builder.begin().unwrap();
        builder.push(&parent, node(1)).unwrap();
        let child = add(&mut builder, &[2, 0, 3, 4, 5]);
        let empty = add(&mut builder, &[]);
        builder.push(&parent, node(6)).unwrap();
        let outer = builder.finish(&parent).unwrap();
        builder.set(child.slice(1..4).unwrap(), 1, node(9)).unwrap();
        let owner = builder.publish().unwrap();
        let expected = [&[2, 0, 9, 4, 5][..], &[][..], &[1, 6][..]];
        let borrowed: Vec<_> = owner.owned_backings().collect::<Result<_, _>>().unwrap();
        assert_eq!(borrowed, expected);
        for (slice, words) in [child, empty, outer].into_iter().zip(borrowed) {
            assert_eq!(owner.words(slice).unwrap(), words);
        }
    }

    #[test]
    fn owned_descriptor_access_still_checks_chunk_and_initialized_range() {
        let mut builder = Builder::<4>::new(1, 9);
        add(&mut builder, &[1, 2]);
        let mut owner = builder.publish().unwrap();
        let original = owner.store.backings[0];
        for backing in [
            Backing {
                chunk: 1,
                ..original
            },
            Backing {
                start: u32::MAX,
                len: 2,
                ..original
            },
            Backing {
                start: 2,
                len: 1,
                ..original
            },
        ] {
            owner.store.backings[0] = backing;
            assert_eq!(
                owner.owned_backings().next().unwrap(),
                Err(Error::InvalidSlice)
            );
        }
        owner.store.backings[0] = original;
        assert_eq!(owner.owned_backings().next().unwrap(), Ok(&[1, 2][..]));
    }

    #[test]
    fn whole_backings_stay_contiguous_and_abandoned_tails_are_not_revisited() {
        let mut b = Builder::<4>::new(1, 9);
        let first = add(&mut b, &[1, 2, 3]);
        let second = add(&mut b, &[4, 5]);
        let third = add(&mut b, &[6]);
        assert_eq!(b.words(first), Ok(&[1, 2, 3][..]));
        assert_eq!(b.words(second), Ok(&[4, 5][..]));
        assert_eq!(b.words(third), Ok(&[6][..]));
        assert_eq!(
            b.backings
                .iter()
                .map(|backing| (backing.chunk, backing.start))
                .collect::<Vec<_>>(),
            [(0, 0), (1, 0), (1, 2)]
        );
        assert_eq!(
            (
                b.stats().edge_words,
                b.stats().edge_capacity,
                b.stats().edge_chunks
            ),
            (6, 8, 2)
        );
        assert_eq!(&*b.chunks[0].words, &[1, 2, 3, 0]);
        assert_eq!(&*b.chunks[1].words, &[4, 5, 6, 0]);
        let pointer = b.words(first).unwrap().as_ptr();
        // Directory growth must leave already borrowed backing storage stable.
        for _ in 0..32 {
            add(&mut b, &[7, 8, 9, 1]);
        }
        assert_eq!(b.words(first).unwrap().as_ptr(), pointer);
    }

    #[test]
    fn oversized_backing_gets_exact_size_and_next_backing_starts_a_new_chunk() {
        let mut b = Builder::<4>::new(1, 9);
        let first = add(&mut b, &[1, 2]);
        let large = add(&mut b, &[3, 4, 5, 6, 7]);
        let last = add(&mut b, &[8]);
        assert_eq!(b.words(first), Ok(&[1, 2][..]));
        assert_eq!(b.words(large), Ok(&[3, 4, 5, 6, 7][..]));
        assert_eq!(b.words(last), Ok(&[8][..]));
        assert_eq!(
            b.chunks
                .iter()
                .map(|chunk| chunk.words.len())
                .collect::<Vec<_>>(),
            [4, 5, 4]
        );
        assert_eq!((b.stats().edge_words, b.stats().edge_capacity), (8, 13));
    }

    #[test]
    fn empty_backings_have_identity_without_allocating_chunks() {
        let mut b = Builder::<4>::new(1, 9);
        let a = add(&mut b, &[]);
        let c = add(&mut b, &[]);
        assert_ne!(a, c);
        assert!(!a.is_nil() && !a.is_missing());
        assert!(a.same(c) && a.same(b.nil()) && a.same(b.missing()));
        assert_eq!(a.slice(0..0), Ok(a));
        assert_eq!(b.words(a), Ok(&[][..]));
        assert_eq!(b.get(a, 0), Err(Error::InvalidSlice));
        assert_eq!(b.set(a, 0, node(1)), Err(Error::InvalidSlice));
        assert_eq!(
            (
                b.stats().backings,
                b.stats().edge_chunks,
                b.stats().edge_capacity
            ),
            (2, 0, 0)
        );
        let p = b.publish().unwrap();
        let mut lengths = Vec::new();
        p.for_each_backing(|words| lengths.push(words.len()))
            .unwrap();
        assert_eq!(lengths, [0, 0]);
    }

    #[test]
    fn nested_frames_finish_without_interleaving_parent_and_child_backings() {
        let mut b = Builder::<4>::new(1, 9);
        let parent = b.begin().unwrap();
        b.push(&parent, node(1)).unwrap();
        let child = b.begin().unwrap();
        b.push(&child, node(2)).unwrap();
        b.push(&child, None).unwrap();
        assert_eq!(b.finish(&parent), Err(Error::WrongFrame));
        assert_eq!(b.abort(&parent), Err(Error::WrongFrame));
        assert_eq!(b.push(&parent, node(9)), Err(Error::WrongFrame));
        let inside = b.finish(&child).unwrap();
        assert_eq!(b.finish(&child), Err(Error::WrongFrame));
        b.push(&parent, node(3)).unwrap();
        let outside = b.finish(&parent).unwrap();
        assert_eq!(b.words(inside), Ok(&[2, 0][..]));
        assert_eq!(b.words(outside), Ok(&[1, 3][..]));
        assert_eq!(b.stats().scratch_peak_len, 3);
        let before = b.stats();
        let p = b.publish().unwrap();
        assert_eq!(p.stats(), before);
        let mut physical = Vec::new();
        p.for_each_backing(|words| physical.push(words.to_vec()))
            .unwrap();
        assert_eq!(physical, [vec![2, 0], vec![1, 3]]);
    }

    #[test]
    fn abort_restores_parent_prefix_but_does_not_remove_completed_children() {
        let mut b = Builder::<4>::new(1, 9);
        let parent = b.begin().unwrap();
        b.push(&parent, node(1)).unwrap();
        let aborted = b.begin().unwrap();
        b.push(&aborted, node(2)).unwrap();
        b.abort(&aborted).unwrap();
        assert_eq!(b.abort(&aborted), Err(Error::WrongFrame));
        let completed = b.begin().unwrap();
        b.push(&completed, node(3)).unwrap();
        let child = b.finish(&completed).unwrap();
        b.push(&parent, node(4)).unwrap();
        assert_eq!(b.scratch, [1, 4]);
        b.abort(&parent).unwrap();
        let p = b.publish().unwrap();
        assert_eq!(p.words(child), Ok(&[3][..]));
        assert_eq!((p.stats().backings, p.stats().edge_words), (1, 1));
    }

    #[test]
    fn slice_mutation_aliases_backing_and_list_clones_keep_separate_headers() {
        let mut b = Builder::<4>::new(1, 9);
        let full = add(&mut b, &[1, 0, 3]);
        let tail = full.slice(1..3).unwrap();
        let endpoint = full.slice(3..3).unwrap();
        assert!(!full.same(tail));
        assert!(full.same(full.slice(0..3).unwrap()));
        assert_eq!(b.words(endpoint), Ok(&[][..]));
        let list = b.new_list(-1, i32::MAX, full).unwrap();
        let copy = b.clone_list(list).unwrap();
        assert_ne!(list, copy);
        b.set_modifiers(list, u32::MAX).unwrap();
        b.set(tail, 0, node(9)).unwrap();
        assert_eq!(b.get(full, 1), Ok(node(9)));
        assert_eq!(b.list(copy).unwrap(), (-1, i32::MAX, full, 0));
        b.set(full, 2, None).unwrap();
        let p = b.publish().unwrap();
        assert_eq!(p.words(tail), Ok(&[9, 0][..]));
        assert_eq!(p.list(list).unwrap(), (-1, i32::MAX, full, u32::MAX));
    }

    #[test]
    fn independent_builders_share_sentinels_but_not_frames_backings_or_headers() {
        let mut a = Builder::<4>::new(1, 9);
        let mut b = Builder::<4>::new(1, 9);
        assert_eq!(b.words(a.nil()), Ok(&[][..]));
        assert_eq!(b.words(a.missing()), Ok(&[][..]));
        b.new_list(0, 0, a.nil()).unwrap();
        b.new_list(0, 0, a.missing()).unwrap();
        let empty = add(&mut a, &[]);
        assert_eq!(b.words(empty), Err(Error::WrongOwner));
        let frame_a = a.begin().unwrap();
        let frame_b = b.begin().unwrap();
        assert_eq!(b.push(&frame_a, node(1)), Err(Error::WrongOwner));
        assert_eq!(b.finish(&frame_a), Err(Error::WrongOwner));
        assert_eq!(b.abort(&frame_a), Err(Error::WrongOwner));
        a.push(&frame_a, node(2)).unwrap();
        let slice = a.finish(&frame_a).unwrap();
        assert_eq!(b.words(slice), Err(Error::WrongOwner));
        assert_eq!(b.set(slice, 0, node(1)), Err(Error::WrongOwner));
        assert_eq!(b.new_list(0, 0, slice), Err(Error::WrongOwner));
        let list = a.new_list(0, 0, slice).unwrap();
        assert_eq!(b.list(list), Err(Error::WrongOwner));
        assert_eq!(b.clone_list(list), Err(Error::WrongOwner));
        assert_eq!(b.set_modifiers(list, 1), Err(Error::WrongOwner));
        b.abort(&frame_b).unwrap();
        assert!(a.publish().is_ok() && b.publish().is_ok());
    }

    #[test]
    fn invalid_nodes_and_ranges_fail_without_mutating_shared_storage() {
        let mut b = Builder::<4>::new(1, 9);
        let frame = b.begin().unwrap();
        assert_eq!(b.push(&frame, node(0)), Err(Error::InvalidNode));
        assert_eq!(b.push(&frame, node(10)), Err(Error::InvalidNode));
        assert_eq!(
            b.push(&frame, Some(NodeRef { owner: 2, slot: 1 })),
            Err(Error::WrongOwner)
        );
        b.push(&frame, node(1)).unwrap();
        let full = b.finish(&frame).unwrap();
        assert_eq!(b.set(full, 0, node(0)), Err(Error::InvalidNode));
        assert_eq!(b.set(full, 0, node(10)), Err(Error::InvalidNode));
        assert_eq!(
            b.set(full, 0, Some(NodeRef { owner: 2, slot: 1 })),
            Err(Error::WrongOwner)
        );
        assert_eq!(b.get(full, 1), Err(Error::InvalidSlice));
        assert_eq!(full.slice(0..2), Err(Error::InvalidSlice));
        let reversed = Range { start: 1, end: 0 };
        assert_eq!(full.slice(reversed), Err(Error::InvalidSlice));
        let malformed = Slice {
            local: LocalSlice {
                start: u32::MAX,
                len: 2,
                ..full.local
            },
            ..full
        };
        assert_eq!(b.words(malformed), Err(Error::InvalidSlice));
        assert_eq!(b.words(full), Ok(&[1][..]));
        let malformed_nil = Slice {
            local: LocalSlice {
                start: 2,
                ..b.nil().local
            },
            ..b.nil()
        };
        assert_eq!(b.words(malformed_nil), Err(Error::InvalidSlice));
        let unknown = Slice {
            local: LocalSlice {
                backing: 2,
                ..full.local
            },
            ..full
        };
        assert_eq!(b.words(unknown), Err(Error::InvalidSlice));
    }

    #[test]
    fn full_u32_domain_has_no_reserved_length_start_or_node_marker() {
        assert_eq!(BACKING_BYTES, 12);
        assert_eq!(HEADER_BYTES, 24);
        assert_eq!(
            CHUNK_DIRECTORY_ENTRY_BYTES,
            std::mem::size_of::<Box<[u32]>>() + std::mem::size_of::<usize>()
        );
        let max = u32::MAX as usize;
        assert_eq!(checked_local_range(0, u32::MAX, max), Ok(0..max));
        assert_eq!(checked_local_range(u32::MAX, 0, max), Ok(max..max));
        assert_eq!(checked_local_range(u32::MAX - 1, 1, max), Ok(max - 1..max));
        assert_eq!(
            checked_local_range(u32::MAX, 1, max),
            Err(Error::InvalidSlice)
        );
        assert_eq!(checked_chunk_index(max), Ok(u32::MAX));
        assert_eq!(checked_backing_id(max - 1), Ok(u32::MAX));
        assert_eq!(checked_backing_id(max), Err(Error::Exhausted));
        if let Some(beyond) = max.checked_add(1) {
            assert_eq!(checked_chunk_index(beyond), Err(Error::Exhausted));
        }
        // No giant allocation: exercise the complete slice arithmetic domain.
        let full = Slice {
            owner: 1,
            local: LocalSlice {
                backing: 1,
                start: 0,
                len: u32::MAX,
            },
        };
        assert_eq!(full.slice(max..max).unwrap().local.start, u32::MAX);
        let mut b = Builder::<4>::new(1, u32::MAX);
        let slice = add(&mut b, &[u32::MAX]);
        assert_eq!(b.get(slice, 0), Ok(node(u32::MAX)));
    }

    #[test]
    fn serial_exhaustion_is_reported_without_reusing_or_invalidating_open_frame() {
        let mut b = Builder::<4>::new(1, 9);
        let frame = b.begin().unwrap();
        b.next_serial = u64::MAX;
        assert!(matches!(b.begin(), Err(Error::Exhausted)));
        b.push(&frame, node(1)).unwrap();
        let slice = b.finish(&frame).unwrap();
        assert_eq!(b.words(slice), Ok(&[1][..]));
    }

    #[test]
    fn publication_rejects_open_frames_even_when_their_scratch_is_empty() {
        let mut b = Builder::<4>::new(1, 9);
        let _frame = b.begin().unwrap();
        assert!(matches!(b.publish(), Err(Error::Incomplete)));
        let mut b = Builder::<4>::new(1, 9);
        let frame = b.begin().unwrap();
        b.push(&frame, node(1)).unwrap();
        assert!(matches!(b.publish(), Err(Error::Incomplete)));
    }

    #[test]
    #[should_panic(expected = "pilot chunk policy must fit a nonzero u32 word count")]
    fn zero_chunk_policy_is_rejected_before_allocating_storage() {
        Builder::<0>::new(1, 9);
    }
}
