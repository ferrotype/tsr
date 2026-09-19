use crate::auxiliary::AuxRead;
use crate::{JsString, NodeId};
use std::{
    ops::{Deref, Range},
    sync::Arc,
};
use ts_arena::{AuxId, Error};
use ts_core::TextRange;

/// An immutable list identity. The referenced header remains mutable only while
/// its file or lazy graph is under exclusive construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeListId(pub(crate) AuxId);
impl NodeListId {
    pub fn bits(self) -> u64 {
        self.0.bits()
    }
}

macro_rules! slice_handle {
    ($name:ident) => {
        /// A non-owning slice of one owner-qualified backing allocation. Empty
        /// slices compare equal under `same`, including nil and allocated-empty.
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
        pub struct $name {
            pub(crate) backing: Option<AuxId>,
            pub(crate) start: u32,
            pub(crate) len: u32,
        }
        impl $name {
            /// Non-owning backing identity for observing shared slice headers.
            pub fn backing_id(self) -> Option<AuxId> {
                self.backing
            }
            pub fn start(self) -> u32 {
                self.start
            }
            pub const fn empty() -> Self {
                Self {
                    backing: None,
                    start: 0,
                    len: 0,
                }
            }
            pub fn len(self) -> usize {
                self.len as usize
            }
            pub fn is_empty(self) -> bool {
                self.len == 0
            }
            pub fn is_nil(self) -> bool {
                self.backing.is_none() && self.start == 0
            }
            pub fn same(self, other: Self) -> bool {
                self.len == other.len
                    && (self.len == 0 || self.backing == other.backing && self.start == other.start)
            }
            pub fn slice(self, range: Range<usize>) -> Result<Self, Error> {
                if range.start > range.end || range.end > self.len() {
                    return Err(Error::InvalidSlot);
                }
                let start = self
                    .start
                    .checked_add(u32::try_from(range.start).map_err(|_| Error::InvalidSlot)?)
                    .ok_or(Error::InvalidSlot)?;
                Ok(Self {
                    backing: self.backing,
                    start,
                    len: u32::try_from(range.len()).map_err(|_| Error::InvalidSlot)?,
                })
            }
        }
    };
}
slice_handle!(NodeSlice);
slice_handle!(TextSlice);

impl NodeSlice {
    // Go's process-wide missingListNodes has an empty, nonnil sentinel backing.
    // A reserved empty descriptor carries that identity without fabricating an
    // owner or allocating a per-file substitute. It still compares Same to any
    // other empty slice, as the pinned core.Same does.
    pub(crate) const fn missing() -> Self {
        Self {
            backing: None,
            start: 1,
            len: 0,
        }
    }
    pub(crate) fn is_missing(self) -> bool {
        self.backing.is_none() && self.start == 1 && self.len == 0
    }
}

/// NodeList/ModifierList header identity is separate from edge backing identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeList {
    loc: TextRange,
    nodes: NodeSlice,
    modifier_flags: u32,
}
impl NodeList {
    pub fn new(loc: TextRange, nodes: NodeSlice) -> Self {
        Self {
            loc,
            nodes,
            modifier_flags: 0,
        }
    }
    pub fn loc(&self) -> TextRange {
        self.loc
    }
    pub fn set_loc(&mut self, loc: TextRange) {
        self.loc = loc;
    }
    pub fn nodes(&self) -> NodeSlice {
        self.nodes
    }
    pub fn modifier_flags(&self) -> u32 {
        self.modifier_flags
    }
    pub fn set_modifier_flags(&mut self, flags: u32) {
        self.modifier_flags = flags;
    }
    pub fn is_missing(&self) -> bool {
        self.nodes.is_missing()
    }
    pub(crate) fn set_missing(&mut self, missing: bool) {
        if missing {
            self.nodes = NodeSlice::missing();
        } else if self.nodes.is_missing() {
            self.nodes = NodeSlice::empty();
        }
    }
    pub(crate) fn set_nodes(&mut self, nodes: NodeSlice) {
        self.nodes = nodes;
    }
}

/// The file frame is owned with records, so an escaped retained node preserves it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileInfo {
    pub root: Option<NodeId>,
    pub node_count: i64,
    pub text_count: i64,
    pub(crate) source_files: Option<AuxId>,
}

/// Typed auxiliary records share the node owner's lifetime and publication lock.
/// This enum is storage plumbing; checked AST APIs distinguish its record kinds.
#[derive(Debug)]
pub enum AstStorageData {
    /// Cold lazy nodes retain full construction payloads under the publication lock.
    FallbackNode(std::sync::Arc<crate::Node>),
    List(NodeList),
    Nodes(Box<[Option<NodeId>]>),
    CompactNodes(crate::compact::lists::CompactNodes),
    Text(Box<[JsString]>),
    File(FileInfo),
    SourceMetadata(crate::SourceMetadataData),
    SourceFiles(std::collections::BTreeMap<NodeId, crate::SourceFileState>),
}

/// A semantic list header retains its borrowed owner or lazy publication guard.
///
/// ```compile_fail
/// use ts_ast::{AstFile, NodeListId, NodeListRead};
/// fn escape(file: &AstFile, list: NodeListId) -> NodeListRead<'static> {
///     file.view().list(list).unwrap()
/// }
/// ```
pub struct NodeListRead<'a>(pub(crate) AuxRead<'a>);
impl NodeListRead<'_> {
    pub fn to_owned(&self) -> NodeList {
        self.0.list().expect("validated list record")
    }
    pub fn loc(&self) -> TextRange {
        self.to_owned().loc()
    }
    pub fn nodes(&self) -> NodeSlice {
        self.to_owned().nodes()
    }
    pub fn modifier_flags(&self) -> u32 {
        self.to_owned().modifier_flags()
    }
    pub fn is_missing(&self) -> bool {
        self.to_owned().is_missing()
    }
}
impl std::fmt::Debug for NodeListRead<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.to_owned().fmt(f)
    }
}

pub struct NodeSliceRead<'a> {
    pub(crate) record: Option<AuxRead<'a>>,
    pub(crate) compact: Option<(&'a crate::compact::lists::EdgePages, ts_arena::ArenaId)>,
    pub(crate) start: usize,
    pub(crate) len: usize,
}
impl NodeSliceRead<'_> {
    fn values(&self) -> &[Option<NodeId>] {
        match &self.record {
            None => &[],
            Some(record) => match record.full() {
                Some(AstStorageData::Nodes(nodes)) => &nodes[self.start..self.start + self.len],
                _ => unreachable!("validated node slice record"),
            },
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Read a node identity by value; `Some(None)` is a present nil element.
    pub fn get(&self, index: usize) -> Option<Option<NodeId>> {
        if index >= self.len {
            return None;
        }
        match self.compact {
            Some((edges, owner)) => edges.get(owner, self.start + index),
            None => self.values().get(index).copied(),
        }
    }

    /// Read an existing element, preserving the bounds panic of slice indexing.
    #[track_caller]
    pub fn at(&self, index: usize) -> Option<NodeId> {
        self.get(index).unwrap_or_else(|| {
            panic!(
                "index out of bounds: the len is {} but the index is {index}",
                self.len
            )
        })
    }

    pub fn first(&self) -> Option<Option<NodeId>> {
        self.get(0)
    }

    pub fn last(&self) -> Option<Option<NodeId>> {
        self.len.checked_sub(1).and_then(|index| self.get(index))
    }

    /// Borrow the read guard while yielding copied, non-retaining identities.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = Option<NodeId>> + ExactSizeIterator + '_ {
        match self.compact {
            Some((edges, owner)) => NodeSliceIter::Compact {
                edges,
                owner,
                range: self.start..self.start + self.len,
            },
            None => NodeSliceIter::Full(self.values().iter().copied()),
        }
    }
}

enum NodeSliceIter<'a> {
    Full(std::iter::Copied<std::slice::Iter<'a, Option<NodeId>>>),
    Compact {
        edges: &'a crate::compact::lists::EdgePages,
        owner: ts_arena::ArenaId,
        range: std::ops::Range<usize>,
    },
}

impl Iterator for NodeSliceIter<'_> {
    type Item = Option<NodeId>;
    fn next(&mut self) -> Option<Self::Item> {
        self.nth(0)
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        match self {
            Self::Full(values) => values.nth(n),
            Self::Compact {
                edges,
                owner,
                range,
            } => range.nth(n).map(|index| {
                edges
                    .get(*owner, index)
                    .expect("validated compact edge index")
            }),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len(), Some(self.len()))
    }
    fn count(self) -> usize {
        self.len()
    }
    fn last(mut self) -> Option<Self::Item> {
        self.next_back()
    }
}

impl DoubleEndedIterator for NodeSliceIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.nth_back(0)
    }
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        match self {
            Self::Full(values) => values.nth_back(n),
            Self::Compact {
                edges,
                owner,
                range,
            } => range.nth_back(n).map(|index| {
                edges
                    .get(*owner, index)
                    .expect("validated compact edge index")
            }),
        }
    }
}

impl ExactSizeIterator for NodeSliceIter<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Full(values) => values.len(),
            Self::Compact { range, .. } => range.len(),
        }
    }
}
impl std::iter::FusedIterator for NodeSliceIter<'_> {}

/// Text references cannot escape the read that holds their lazy backing guard.
///
/// ```compile_fail
/// use ts_ast::{AstFile, JsString, TextSlice};
/// fn escape<'a>(file: &'a AstFile, text: TextSlice) -> &'a [JsString] {
///     &file.view().text_slice(text).unwrap()
/// }
/// ```
pub struct TextSliceRead<'a> {
    pub(crate) record: Option<AuxRead<'a>>,
    pub(crate) start: usize,
    pub(crate) len: usize,
}
impl Deref for TextSliceRead<'_> {
    type Target = [JsString];
    fn deref(&self) -> &Self::Target {
        match &self.record {
            None => &[],
            Some(record) => match record.full() {
                Some(AstStorageData::Text(text)) => &text[self.start..self.start + self.len],
                _ => unreachable!("validated text slice record"),
            },
        }
    }
}

/// Cached root IDs do not retain a file. Empty results need no allocation or
/// reference-count operation; cache presence is tracked separately by the owner.
#[derive(Clone, Debug, Default)]
pub struct JSDocRoots(Option<Arc<[NodeId]>>);
impl Deref for JSDocRoots {
    type Target = [NodeId];
    fn deref(&self) -> &[NodeId] {
        self.0.as_deref().unwrap_or(&[])
    }
}

impl JSDocRoots {
    /// An empty result without allocating or retaining shared storage.
    pub const fn empty() -> Self {
        Self(None)
    }
    pub(crate) fn from_shared(roots: Arc<[NodeId]>) -> Self {
        if roots.is_empty() {
            Self::empty()
        } else {
            Self(Some(roots))
        }
    }
}
