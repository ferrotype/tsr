//! Checked semantic reads borrow their physical owner and its compact payloads.
use crate::compact::{CompactContext, FieldKey, StoredNode};
use crate::{AstStorageData, Node, NodeDataRead, NodeDataSource, NodeId};
use ts_arena::{AuxId, FileId, StorageOwner, StorageRead, StorageTransaction, StorageView};
use ts_jsstring::SourceText;

/// A node read borrows its physical owner. Lazy payload references cannot outlive
/// the read's publication guard. Escaping requires `RetainedNode`.
///
/// ```compile_fail
/// use ts_ast::{AstFile, NodeId};
/// fn escaped_text<'owner>(file: &'owner AstFile, lazy: NodeId) -> &'owner [u8] {
///     let read = file.view().node(lazy).unwrap();
///     read.as_identifier().unwrap().text()
/// }
/// ```
///
/// ```compile_fail
/// use ts_ast::{AstFile, NodeId};
/// fn escaped_payload<'owner>(file: &'owner AstFile, lazy: NodeId) -> &'owner [u8] {
///     let read = file.view().node(lazy).unwrap();
///     read.data_source().as_identifier().unwrap().text()
/// }
/// ```
pub struct NodeRead<'a> {
    record: ReadRecord<'a>,
    id: NodeId,
}
enum ReadRecord<'a> {
    Core {
        header: &'a StoredNode,
        owner: &'a StorageOwner<StoredNode>,
    },
    TransactionCore {
        header: &'a StoredNode,
        owner: &'a StorageTransaction<'a, StoredNode>,
    },
    Owned {
        node: &'a Node,
        source: &'a SourceText,
        owner_id: FileId,
    },
    Lazy {
        record: StorageRead<'a, AstStorageData>,
        owner: &'a StorageOwner<StoredNode>,
    },
}
impl std::fmt::Debug for NodeRead<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("NodeRead")
            .field("id", &self.id)
            .field("owner_id", &self.owner_id())
            .field("kind", &self.kind())
            .field("shape", &self.data().name())
            .finish_non_exhaustive()
    }
}
impl<'a> NodeRead<'a> {
    #[inline]
    pub(crate) fn resolved(
        id: NodeId,
        record: &StorageRead<'a, StoredNode>,
        owner: StorageView<'a, StoredNode>,
    ) -> Self {
        if id.arena() == owner.id().arena() {
            Self::core(
                id,
                record
                    .as_borrowed()
                    .expect("core headers borrow their owner"),
                owner.physical_owner(),
            )
        } else {
            Self::resolved_lazy(id, record, owner)
        }
    }
    #[cold]
    #[inline(never)]
    fn resolved_lazy(
        id: NodeId,
        record: &StorageRead<'a, StoredNode>,
        owner: StorageView<'a, StoredNode>,
    ) -> Self {
        let aux = AuxId::from_parts(owner.lazy_auxiliary_arena(), record.ordinal)
            .expect("lazy payload slot");
        Self {
            record: ReadRecord::Lazy {
                record: crate::auxiliary::AuxRead::resolved(
                    owner.aux(aux).expect("published lazy payload"),
                    owner,
                )
                .into_full()
                .expect("lazy payload is full"),
                owner: owner.physical_owner(),
            },
            id,
        }
    }
    #[inline]
    pub(crate) fn core(
        id: NodeId,
        header: &'a StoredNode,
        owner: &'a StorageOwner<StoredNode>,
    ) -> Self {
        Self {
            record: ReadRecord::Core { header, owner },
            id,
        }
    }
    pub(crate) fn transaction_core(
        id: NodeId,
        header: &'a StoredNode,
        owner: &'a StorageTransaction<'a, StoredNode>,
    ) -> Self {
        Self {
            record: ReadRecord::TransactionCore { header, owner },
            id,
        }
    }
    pub(crate) fn owned(
        id: NodeId,
        node: &'a Node,
        owner_id: FileId,
        source: &'a SourceText,
    ) -> Self {
        Self {
            record: ReadRecord::Owned {
                node,
                source,
                owner_id,
            },
            id,
        }
    }
    pub fn id(&self) -> NodeId {
        self.id
    }
    pub fn owner_id(&self) -> FileId {
        match self.record {
            ReadRecord::Core { owner, .. } | ReadRecord::Lazy { owner, .. } => owner.id(),
            ReadRecord::TransactionCore { owner, .. } => owner.owner_id(),
            ReadRecord::Owned { owner_id, .. } => owner_id,
        }
    }
    pub fn source(&self) -> &'a SourceText {
        match self.record {
            ReadRecord::Core { owner, .. } | ReadRecord::Lazy { owner, .. } => owner.source_text(),
            ReadRecord::TransactionCore { owner, .. } => owner.source(),
            ReadRecord::Owned { source, .. } => source,
        }
    }
    /// A directly borrowed read can retain its owner's lifetime. A directory
    /// resolved lazy read must stay bounded by its publication guard instead.
    pub fn as_borrowed(&self) -> Option<Self> {
        let record = match self.record {
            ReadRecord::Core { header, owner } => ReadRecord::Core { header, owner },
            ReadRecord::TransactionCore { header, owner } => {
                ReadRecord::TransactionCore { header, owner }
            }
            ReadRecord::Owned {
                node,
                source,
                owner_id,
            } => ReadRecord::Owned {
                node,
                source,
                owner_id,
            },
            ReadRecord::Lazy { .. } => return None,
        };
        Some(Self {
            record,
            id: self.id,
        })
    }
    fn owned_record(&self) -> Option<&Node> {
        match &self.record {
            ReadRecord::Owned { node, .. } => Some(node),
            ReadRecord::Lazy { record, .. } => match &**record {
                AstStorageData::FallbackNode(node) => Some(node),
                _ => unreachable!("lazy header names its owned payload"),
            },
            ReadRecord::Core { .. } | ReadRecord::TransactionCore { .. } => None,
        }
    }
    #[inline]
    fn core_header(&self) -> Option<&'a StoredNode> {
        match self.record {
            ReadRecord::Core { header, .. } | ReadRecord::TransactionCore { header, .. } => {
                Some(header)
            }
            _ => None,
        }
    }
    /// Assemble decode context only when a compact field is actually requested.
    #[inline]
    pub(crate) fn compact_context(&self) -> CompactContext<'_> {
        match self.record {
            ReadRecord::Core { owner, .. } => CompactContext {
                nodes: owner.id().arena(),
                auxiliary: owner.auxiliary_arena(),
                source: owner.source_text(),
                store: owner.store(),
            },
            ReadRecord::TransactionCore { owner, .. } => CompactContext {
                nodes: owner.owner_id().arena(),
                auxiliary: owner.core_auxiliary_arena(),
                source: owner.source(),
                store: owner.store(),
            },
            _ => unreachable!("compact decode requires a core header"),
        }
    }
    /// Explicit construction copy used by unrestricted exclusive edits and cold
    /// published overlays; ordinary payload reads never reconstruct a node.
    pub(crate) fn to_owned_preserving_identity(&self) -> Node {
        let facts = match self.core_header() {
            Some(_) => self
                .compact_context()
                .store
                .parked_facts(self.id.slot())
                .unwrap_or_else(|| self.cached_subtree_facts()),
            _ => self.cached_subtree_facts(),
        };
        Node {
            kind: self.kind(),
            parent: self.parent(),
            flags: self.flags(),
            pos: self.pos(),
            end: self.end(),
            data: self.data().to_owned(),
            subtree_facts: std::sync::atomic::AtomicU32::new(facts),
            runtime_id: std::sync::atomic::AtomicU64::new(crate::existing_runtime_node_id(self)),
        }
    }
    pub(crate) fn copy_for_binding(&self) -> Node {
        let mut node = self.to_owned_preserving_identity();
        node.runtime_id = std::sync::atomic::AtomicU64::new(crate::runtime_node_id(self));
        node
    }
    pub fn kind(&self) -> crate::NodeKind {
        match self.core_header() {
            Some(header) => header.kind,
            _ => self.owned_record().unwrap().kind(),
        }
    }
    pub fn parent(&self) -> Option<NodeId> {
        match self.core_header() {
            Some(header) => self
                .compact_context()
                .decode_node(FieldKey::parent(self.id.slot()), header.parent),
            _ => self.owned_record().unwrap().parent(),
        }
    }
    pub fn flags(&self) -> u32 {
        match self.core_header() {
            Some(header) => header.flags,
            _ => self.owned_record().unwrap().flags(),
        }
    }
    pub fn pos(&self) -> i32 {
        match self.core_header() {
            Some(header) => header.pos,
            _ => self.owned_record().unwrap().pos(),
        }
    }
    pub fn end(&self) -> i32 {
        match self.core_header() {
            Some(header) => header.end,
            _ => self.owned_record().unwrap().end(),
        }
    }
    pub fn range(&self) -> ts_core::TextRange {
        ts_core::TextRange::new(i64::from(self.pos()), i64::from(self.end()))
    }
    pub fn data(&self) -> NodeDataRead<'_> {
        match self.core_header() {
            Some(header) => {
                let context = self.compact_context();
                context.store.payloads.read(
                    header.actual_shape(),
                    header.ordinal,
                    context,
                    header.end,
                )
            }
            _ => NodeDataRead::from_owned(self.owned_record().unwrap().data()),
        }
    }
    pub fn data_source(&self) -> NodeDataSource<'_> {
        match self.core_header() {
            Some(header) => NodeDataSource::from_stored(header, self),
            _ => NodeDataSource::from_owned(self.owned_record().unwrap().data()),
        }
    }
    pub(crate) fn inline_binding(&self) -> Option<crate::NodeBinding> {
        match self.core_header() {
            Some(header) => {
                let context = self.compact_context();
                context.store.node_binding(header, self.id.slot(), context)
            }
            _ => None,
        }
    }
    pub(crate) fn inline_symbol(&self) -> Option<crate::SymbolId> {
        match self.core_header() {
            Some(header) => {
                let context = self.compact_context();
                context.store.node_symbol(header, self.id.slot(), context)
            }
            _ => None,
        }
    }
    pub(crate) fn inline_locals(&self) -> Option<crate::SymbolTableId> {
        match self.core_header() {
            Some(header) => {
                let context = self.compact_context();
                context.store.node_locals(header, self.id.slot(), context)
            }
            _ => None,
        }
    }
    pub(crate) fn inline_flow(&self) -> Option<crate::FlowId> {
        match self.core_header() {
            Some(header) => {
                let context = self.compact_context();
                context.store.node_flow(header, self.id.slot(), context)
            }
            _ => None,
        }
    }
    pub fn for_each_child(
        &self,
        visitor: &mut impl crate::ChildVisitor,
    ) -> std::ops::ControlFlow<()> {
        self.for_each_child_generated(visitor)
    }
    pub(crate) fn cached_subtree_facts(&self) -> u32 {
        match self.core_header() {
            Some(header) => self
                .compact_context()
                .store
                .payloads
                .cached_subtree_facts(header.actual_shape(), header.ordinal),
            _ => self.owned_record().unwrap().cached_subtree_facts(),
        }
    }
    pub(crate) fn store_subtree_facts(&self, facts: u32) {
        match self.core_header() {
            Some(header) => self.compact_context().store.payloads.store_subtree_facts(
                header.actual_shape(),
                header.ordinal,
                facts,
            ),
            _ => self.owned_record().unwrap().store_subtree_facts(facts),
        }
    }
}
impl crate::NodeAccess for NodeRead<'_> {
    fn kind(&self) -> crate::NodeKind {
        self.kind()
    }
    fn parent(&self) -> Option<NodeId> {
        self.parent()
    }
    fn flags(&self) -> u32 {
        self.flags()
    }
    fn pos(&self) -> i32 {
        self.pos()
    }
    fn end(&self) -> i32 {
        self.end()
    }
    fn range(&self) -> ts_core::TextRange {
        self.range()
    }
    fn data(&self) -> NodeDataRead<'_> {
        self.data()
    }
    fn data_source(&self) -> NodeDataSource<'_> {
        self.data_source()
    }
    fn cached_subtree_facts(&self) -> u32 {
        self.cached_subtree_facts()
    }
    fn store_subtree_facts(&self, facts: u32) {
        self.store_subtree_facts(facts);
    }
    fn existing_runtime_id(&self) -> u64 {
        match self.core_header() {
            Some(_) => self
                .compact_context()
                .store
                .existing_runtime_id(self.id.slot()),
            _ => crate::runtime_id::owned_existing_runtime_node_id(self.owned_record().unwrap()),
        }
    }
    fn runtime_id(&self) -> u64 {
        match self.core_header() {
            Some(_) => self.compact_context().store.runtime_id(self.id.slot()),
            _ => crate::runtime_id::owned_runtime_node_id(self.owned_record().unwrap()),
        }
    }
}
#[cfg(test)]
mod tests;
