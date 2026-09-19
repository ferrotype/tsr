//! Exclusive binding over a validated local core. The fresh scope proves the
//! namespace once; typed row reads retain ordinary safe page bounds checks.
use super::{BindBuilder, BindResult, BindStorage};
use crate::compact::{CompactSlice, CoreStore, FieldKey, StoredNode};
use crate::{AstView, FlowId, NodeId, NodeKind};
use std::{marker::PhantomData, num::NonZeroU32, ops::ControlFlow};
use ts_arena::{CoreScopeMut, Error};
use ts_jsstring::SourceText;

type Brand<'scope> = PhantomData<fn(&'scope ()) -> &'scope ()>;

#[path = "local_bind_compatibility.rs"]
mod compatibility;
#[path = "local_bind_state.rs"]
mod state;
#[path = "local_bind_state_nodes.rs"]
mod state_nodes;
pub use state::{BindFlowList, BindSymbol, BindTable};
#[path = "local_bind_helpers.rs"]
mod helpers;
#[path = "local_bind_semantics.rs"]
mod semantics;

macro_rules! local_identity {
    ($name:ident) => {
        /// Non-owning identity valid only within the callback that minted it.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name<'scope> {
            word: NonZeroU32,
            brand: Brand<'scope>,
        }
        impl $name<'_> {
            fn from_word(word: u32) -> Option<Self> {
                NonZeroU32::new(word).map(|word| Self {
                    word,
                    brand: PhantomData,
                })
            }
        }
    };
}
local_identity!(BindNode);
local_identity!(BindList);
local_identity!(BindFlow);

/// A syntax slice retains its backing identity and subrange within the scope.
#[derive(Clone, Copy, Debug)]
pub struct BindSlice<'scope> {
    value: CompactSlice,
    brand: Brand<'scope>,
}

/// Text and syntax backing namespaces cannot be interchanged.
#[derive(Clone, Copy, Debug)]
pub struct BindTextSlice<'scope> {
    value: CompactSlice,
    brand: Brand<'scope>,
}

/// A resolved immutable syntax range. Narrow binder writes cannot move or edit
/// its backing, so this descriptor may cross recursive binding mutations.
#[derive(Clone, Copy, Debug)]
pub struct BindEdges<'scope> {
    start: usize,
    len: usize,
    brand: Brand<'scope>,
}
impl BindEdges<'_> {
    pub fn len(self) -> usize {
        self.len
    }
    pub fn is_empty(self) -> bool {
        self.len == 0
    }
}

impl BindSlice<'_> {
    pub fn is_nil(self) -> bool {
        self.value.backing == 0 && self.value.start == 0
    }
    pub fn len(self) -> usize {
        self.value.len as usize
    }
    pub fn is_empty(self) -> bool {
        self.value.len == 0
    }
}
impl BindTextSlice<'_> {
    pub fn is_nil(self) -> bool {
        self.value.backing == 0 && self.value.start == 0
    }
    pub fn len(self) -> usize {
        self.value.len as usize
    }
    pub fn is_empty(self) -> bool {
        self.value.len == 0
    }
}

#[derive(Clone, Copy)]
pub(crate) struct BindContext<'scope, 'read> {
    pub(crate) store: &'read CoreStore,
    pub(crate) source: &'read SourceText,
    brand: Brand<'scope>,
}
impl<'scope, 'read> BindContext<'scope, 'read> {
    #[inline]
    pub(crate) fn node(self, word: u32) -> Option<BindNode<'scope>> {
        NonZeroU32::new(word).map(|word| BindNode {
            word,
            brand: self.brand,
        })
    }
    #[inline]
    pub(crate) fn list(self, word: u32) -> Option<BindList<'scope>> {
        BindList::from_word(word).map(|list| BindList {
            brand: self.brand,
            ..list
        })
    }
    #[inline]
    pub(crate) fn node_slice(self, value: CompactSlice) -> BindSlice<'scope> {
        BindSlice {
            value,
            brand: self.brand,
        }
    }
    #[inline]
    pub(crate) fn text_slice(self, value: CompactSlice) -> BindTextSlice<'scope> {
        BindTextSlice {
            value,
            brand: self.brand,
        }
    }
    #[inline]
    pub(crate) fn text(self, key: FieldKey, word: u32, end: i32) -> &'read [u8] {
        self.store.local_text(key, word, end, self.source)
    }

    #[inline]
    pub(crate) fn text_owned(self, key: FieldKey, word: u32, end: i32) -> crate::JsString {
        self.store.local_text_owned(key, word, end, self.source)
    }
}

/// Direct header and generated typed payload reads; no owned/stored facade.
#[derive(Clone, Copy)]
pub struct BindRead<'scope, 'read> {
    pub(crate) header: &'read StoredNode,
    pub(crate) context: BindContext<'scope, 'read>,
}
impl<'scope> BindRead<'scope, '_> {
    #[inline]
    pub fn kind(&self) -> NodeKind {
        self.header.kind
    }
    #[inline]
    pub fn flags(&self) -> u32 {
        self.header.flags
    }
    #[inline]
    pub fn pos(&self) -> i32 {
        self.header.pos
    }
    #[inline]
    pub fn end(&self) -> i32 {
        self.header.end
    }
    #[inline]
    pub fn parent(&self) -> Option<BindNode<'scope>> {
        self.context.node(self.header.parent)
    }
}

/// Generated enumeration preserves schema child order, including nil handling.
pub trait LocalChildVisitor<'scope> {
    fn visit_node(&mut self, node: BindNode<'scope>) -> ControlFlow<()>;
    fn visit_list(&mut self, list: BindList<'scope>) -> ControlFlow<()>;
    fn visit_node_slice(&mut self, slice: BindSlice<'scope>) -> ControlFlow<()>;
}

/// Mutable binding capability. Syntax edges and storage growth are inaccessible.
pub struct LocalBind<'scope, 'owner> {
    core: CoreScopeMut<'scope, 'owner, StoredNode>,
    result: &'owner mut BindResult,
}

impl BindBuilder<'_> {
    /// Enter a fresh local scope only for validated exclusive syntax without
    /// escaped references or compatibility binding records. Exceptional owners
    /// return None before invoking the callback and retain the checked backend.
    pub fn with_local_scope<R>(
        &mut self,
        operation: impl for<'scope> FnOnce(LocalBind<'scope, '_>) -> R,
    ) -> Option<R> {
        let BindStorage::Exclusive(parsed) = &mut self.storage else {
            return None;
        };
        if !self.result.direct_nodes
            || self.result.multiple_sources
            || !self.result.nodes.is_empty()
            || !self.result.bindings.is_empty()
            || !self.result.flow_bindings.is_empty()
            || !parsed.local_binding_eligible()
        {
            return None;
        }
        let result = &mut self.result;
        parsed.with_local_core(|core| {
            // Public checked builders can replace whole result arenas between
            // callbacks. Inline words still name their original namespaces.
            let arenas = crate::compact::binding::BindingArenas {
                symbols: result.symbols.id(),
                tables: result.tables.id(),
                flows: result.flows.id(),
            };
            if !core.store().binding_arenas_match(arenas) {
                return None;
            }
            result.used_local_scope = true;
            Some(operation(LocalBind { core, result }))
        })
    }
}

impl<'scope> LocalBind<'scope, '_> {
    #[inline]
    fn context(&self) -> BindContext<'scope, '_> {
        BindContext {
            store: self.core.store(),
            source: self.core.source(),
            brand: PhantomData,
        }
    }
    /// A raw identity crosses the owner boundary once. Owner errors precede slots.
    pub fn import_node(&self, node: NodeId) -> Result<BindNode<'scope>, Error> {
        let local = self.core.check(node)?;
        Ok(BindNode::from_word(local.slot()).expect("checked nonzero node slot"))
    }
    /// Import a core list at a checked boundary. Lazy or foreign backing stays
    /// on the general path even when initialization created it after entry.
    pub fn import_list(&self, list: crate::NodeListId) -> Result<BindList<'scope>, Error> {
        if list.0.arena() != self.core.auxiliary_arena() {
            return Err(Error::WrongOwner);
        }
        let _ = self.parsed_view().list(list)?;
        Ok(BindList::from_word(list.0.slot()).expect("checked nonzero list slot"))
    }
    pub fn import_slice(&self, slice: crate::NodeSlice) -> Result<BindSlice<'scope>, Error> {
        if slice
            .backing
            .is_some_and(|backing| backing.arena() != self.core.auxiliary_arena())
        {
            return Err(Error::WrongOwner);
        }
        let _ = self.parsed_view().node_slice(slice)?;
        Ok(self.context().node_slice(CompactSlice {
            backing: slice.backing.map_or(0, ts_arena::AuxId::slot),
            start: slice.start,
            len: slice.len,
        }))
    }
    pub fn source(&self) -> BindNode<'scope> {
        self.import_node(self.result.source)
            .expect("validated local source")
    }
    #[inline]
    pub fn node(&self, node: BindNode<'scope>) -> BindRead<'scope, '_> {
        let (_, header) = self
            .core
            .resolve_slot(node.word.get())
            .expect("validated local edge");
        BindRead {
            header,
            context: self.context(),
        }
    }
    /// Explicit boundary for shared helpers not yet ported to local identities.
    pub fn node_id(&self, node: BindNode<'scope>) -> NodeId {
        NodeId::from_parts(self.result.source.arena(), node.word.get())
            .expect("nonzero local identity")
    }

    /// Same contextual-name roles as the public helper, with local parent links.
    pub fn is_identifier_name(&self, node: BindNode<'scope>) -> bool {
        use crate::binder_helpers::{identifier_name_role, IdentifierNameRole as Role};
        let parent = self.node(
            self.node(node)
                .parent()
                .expect("nil node in source AST utility"),
        );
        match identifier_name_role(parent.kind()) {
            Role::Name => parent.name() == Some(node),
            Role::Right => parent.as_qualified_name().map_or_else(
                || {
                    crate::is_identifier_name(self.view(), self.node_id(node))
                        .expect("binder graph belongs to its retained source")
                },
                |parent| parent.right() == Some(node),
            ),
            Role::PropertyName => {
                let field = match parent.kind().known() {
                    Some(crate::SyntaxKind::ImportSpecifier) => parent
                        .as_import_specifier()
                        .map(|data| data.property_name()),
                    Some(crate::SyntaxKind::BindingElement) => {
                        parent.as_binding_element().map(|data| data.property_name())
                    }
                    _ => unreachable!("shared identifier property-name role"),
                };
                field.map_or_else(
                    || {
                        crate::is_identifier_name(self.view(), self.node_id(node))
                            .expect("binder graph belongs to its retained source")
                    },
                    |field| field == Some(node),
                )
            }
            Role::Always => true,
            Role::Never => false,
        }
    }
    pub fn view(&self) -> AstView<'_> {
        AstView(self.core.view(), Some(self.result))
    }
    pub fn set_flags(&mut self, node: BindNode<'scope>, flags: u32) {
        let local = self
            .core
            .check_slot(node.word.get())
            .expect("validated local node");
        self.core.get_mut(local).set_flags(flags);
    }
    pub fn import_flow(&self, flow: FlowId) -> Result<BindFlow<'scope>, Error> {
        self.result.flows.get(flow)?;
        Ok(BindFlow::from_word(flow.slot()).expect("checked nonzero flow slot"))
    }
    pub fn flow_id(&self, flow: BindFlow<'scope>) -> FlowId {
        FlowId::from_parts(self.result.flows.id(), flow.word.get()).expect("nonzero local flow")
    }
    /// Flow links do not change syntax edges or materialize empty binding records.
    /// Unsupported concrete shapes return false without any mutation.
    pub fn set_flow(&mut self, node: BindNode<'scope>, flow: Option<BindFlow<'scope>>) -> bool {
        // Checked compatibility operations may have materialized a record after
        // entry. Such a record supersedes all inline fields, including clears.
        if !self.result.bindings.is_empty() || !self.result.flow_bindings.is_empty() {
            let id = self.node_id(node);
            let flow = flow.map(|flow| self.flow_id(flow));
            if let Some(binding) = self.result.bindings.get_mut(&id) {
                binding.flow_node = flow;
                return true;
            }
            if let std::collections::hash_map::Entry::Occupied(mut entry) =
                self.result.flow_bindings.entry(id)
            {
                if let Some(flow) = flow {
                    entry.insert(flow);
                } else {
                    entry.remove();
                }
                return true;
            }
        }
        let local = self
            .core
            .check_slot(node.word.get())
            .expect("validated local node");
        if self.core.store().has_link_escapes()
            || flow.is_some_and(|flow| flow.word.get() == u32::MAX)
        {
            let id = self.node_id(node);
            let raw_flow = flow.map(|flow| self.flow_id(flow));
            let auxiliary = self.core.auxiliary_arena();
            let (header, store, source) = self.core.node_store_and_source_mut(local);
            return store.write_binding(
                header,
                id,
                auxiliary,
                source,
                crate::compact::binding::BindingWrite::Flow(raw_flow),
            );
        }
        let (header, store, _) = self.core.node_store_and_source_mut(local);
        store.payloads.set_local_flow_node(
            header.actual_shape(),
            header.ordinal,
            flow.map_or(0, |flow| flow.word.get()),
        )
    }
    /// Modifier flags are stored on the list header; syntax edges are not scanned.
    #[inline]
    pub(crate) fn list_modifier_flags(&self, list: BindList<'scope>) -> u32 {
        let record = self
            .core
            .auxiliary_slot(list.word.get())
            .expect("validated local list");
        self.core
            .store()
            .auxiliary
            .local_modifier_flags(record)
            .expect("local list encoding")
    }
    pub fn list(&self, list: BindList<'scope>) -> BindSlice<'scope> {
        let record = self
            .core
            .auxiliary_slot(list.word.get())
            .expect("validated local list");
        let value = self
            .core
            .store()
            .auxiliary
            .local_list(record)
            .expect("local list encoding");
        self.context().node_slice(value)
    }
    /// Resolve a backing and subrange once, before iterating its local edge words.
    pub fn slice(&self, slice: BindSlice<'scope>) -> BindSliceRead<'scope, '_> {
        let value = slice.value;
        let range = if value.backing == 0 {
            assert!(
                value.len == 0 && value.start <= 1,
                "validated nil or missing local slice"
            );
            0..0
        } else {
            let record = self
                .core
                .auxiliary_slot(value.backing)
                .expect("validated local backing");
            let backing = self
                .core
                .store()
                .auxiliary
                .local_backing(record)
                .expect("local backing encoding");
            let end = value
                .start
                .checked_add(value.len)
                .expect("validated local slice range");
            assert!(end <= backing.len, "validated local slice bounds");
            let start = backing
                .start
                .checked_add(value.start as usize)
                .expect("validated local edge start");
            let end = start
                .checked_add(value.len as usize)
                .expect("validated local edge end");
            assert!(
                self.core.store().edges.valid_range(start..end),
                "validated local edge range"
            );
            start..end
        };
        BindSliceRead {
            edges: &self.core.store().edges,
            range,
            brand: PhantomData,
        }
    }
    pub fn edges(&self, slice: BindSlice<'scope>) -> BindEdges<'scope> {
        let read = self.slice(slice);
        BindEdges {
            start: read.range.start,
            len: read.range.len(),
            brand: PhantomData,
        }
    }
    /// None denotes a stored nil edge; out-of-range indexing remains a panic.
    pub fn edge(&self, edges: BindEdges<'scope>, index: usize) -> Option<BindNode<'scope>> {
        assert!(index < edges.len, "local edge index in resolved range");
        BindNode::from_word(
            self.core
                .store()
                .edges
                .local_word(edges.start + index)
                .expect("resolved local backing"),
        )
    }
    /// Cold metadata consumers can explicitly reconstruct their checked slice.
    pub fn text_slice(&self, slice: BindTextSlice<'scope>) -> crate::TextSlice {
        crate::TextSlice {
            backing: NonZeroU32::new(slice.value.backing).map(|word| {
                ts_arena::AuxId::from_parts(self.core.auxiliary_arena(), word.get())
                    .expect("local text backing")
            }),
            start: slice.value.start,
            len: slice.value.len,
        }
    }
}

pub struct BindSliceRead<'scope, 'read> {
    edges: &'read crate::compact::lists::EdgePages,
    range: std::ops::Range<usize>,
    brand: Brand<'scope>,
}
impl<'scope> BindSliceRead<'scope, '_> {
    pub fn len(&self) -> usize {
        self.range.len()
    }
    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }
    pub fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = Option<BindNode<'scope>>> + ExactSizeIterator + '_ {
        self.range.clone().map(|index| {
            BindNode::from_word(
                self.edges
                    .local_word(index)
                    .expect("validated local edge index"),
            )
        })
    }
}
