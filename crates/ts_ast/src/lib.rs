//! Schema-generated syntax shapes with exclusive construction and owning storage.
//!
//! Child identities do not retain their owner. The generated visitors enumerate
//! node and list edges; an owning tree supplies list resolution and recursion.

mod accessors_generated;
mod bind_result;
pub use bind_result::local_bind;
#[cfg(any(test, doctest))]
mod local_bind_tests;
mod local_read_generated;
pub use local_read_generated::*;
pub mod check_flags;
mod flow;
mod symbol_access;
pub mod symbol_flags;
mod symbol_store;
mod symbol_tables;
pub use symbol_tables::name_hash;
mod symbols;
pub use flow::*;
pub use symbol_access::{SymbolAccess, SymbolRef};
pub use symbol_store::{SymbolMut, SymbolRead, SymbolsMut, SymbolsRead};
pub use symbols::*;
mod binder_helpers;
mod declaration_helpers;
mod syntax_helpers;
pub use binder_helpers::*;
mod clone;
mod compact;
mod compact_generated;
pub(crate) use compact_generated::AstPayloadStore;
pub(crate) mod auxiliary;
#[cfg(test)]
mod auxiliary_tests;
mod data_generated;
mod diagnostic;
mod diagnostic_order;
mod factory;
mod factory_generated;
mod jsdoc;
mod kinds_generated;
pub use auxiliary::StoredAux;
mod lists;
pub mod modifier_flags;
mod node_access;
mod node_semantics;
pub use node_access::NodeAccess;
mod node_accessors;
pub mod node_flags;
mod node_index;
mod node_kind;
mod node_map;
mod node_mut;
pub use node_mut::NodeMut;
mod node_read;
mod node_read_generated;
mod node_text;
mod precedence;
mod runtime_generated;
mod runtime_id;
mod source_cache;
mod source_file;
mod source_metadata;
mod storage;
mod subtree_facts;
mod subtree_generated;
mod tokens;
mod transform_generated;
mod visitor;
mod visitors_generated;

pub use bind_result::{
    BindBuilder, BindError, BindResult, BoundFile, BoundView, CompletedFile, CompletedNode,
    CompletedSymbol, NodeBinding, PatternAmbientModule, RetainedBoundNode, RetainedSymbol,
    SharedBoundFile,
};
pub use clone::{
    clone_node, deep_clone_node, deep_clone_reparse, deep_clone_reparse_modifiers,
    set_parent_in_children,
};
pub use data_generated::*;
pub use diagnostic::Diagnostic;
pub use diagnostic_order::{
    compare_diagnostics, equal_diagnostics, equal_diagnostics_no_related_info,
};
pub use factory::{BorrowedFactory, Factory, FactoryHooks};
pub use factory_generated::FactoryMethods;
pub use jsdoc::{EagerJsDocProvider, JsDocProvider};
pub use kinds_generated::SyntaxKind;
pub use lists::{
    AstStorageData, FileInfo, JSDocRoots, NodeList, NodeListId, NodeListRead, NodeSlice,
    NodeSliceRead, TextSlice, TextSliceRead,
};
pub use node_index::NodeIndexCache;
pub use node_kind::NodeKind;
pub use node_read::NodeRead;
pub use node_read_generated::*;
pub use node_text::NodeText;
pub use precedence::{get_binary_operator_precedence, operator_precedence};
pub use runtime_generated::*;
pub use runtime_id::{existing_runtime_node_id, runtime_node_id};
pub use source_file::*;
pub use source_metadata::SourceFileMetaData;
pub use storage::{
    AstBuilder, AstBundle, AstFile, AstTransaction, AstView, ParsedFile, RetainedNode,
};
pub use subtree_facts::{is_left_hand_side_expression_kind, subtree_flags, SubtreeFacts};
pub use subtree_generated::SubtreeContext;
pub use tokens::{token_flags, CommentDirective, CommentDirectiveKind, TokenFlags};
pub use transform_generated::{VisitContext, VisitorMethods};
pub use ts_arena::{ArenaId, NodeId};
pub use ts_jsstring::JsString;
pub use visitor::{
    modifier_to_flag, ListVisit, NodeVisit, NodeVisitor, NodeVisitorHooks, RuntimeFactory,
};

use std::ops::ControlFlow;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// A descriptor into one owner's child-id table, not an ownership proof.
///
/// Construction checks arithmetic only. The future list owner must additionally
/// validate its identity and published bounds before resolving the range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeListRange {
    owner: ArenaId,
    start: u32,
    len: u32,
}

impl NodeListRange {
    pub fn new(owner: ArenaId, start: u32, len: u32) -> Option<Self> {
        start.checked_add(len)?;
        Some(Self { owner, start, len })
    }
    pub fn owner(self) -> ArenaId {
        self.owner
    }
    pub fn start(self) -> u32 {
        self.start
    }
    pub fn len(self) -> u32 {
        self.len
    }
    pub fn is_empty(self) -> bool {
        self.len == 0
    }
}

/// Source positions are signed byte offsets, including upstream's synthetic -1.
#[derive(Debug)]
pub struct Node {
    kind: NodeKind,
    parent: Option<NodeId>,
    flags: u32,
    pos: i32,
    end: i32,
    data: NodeData,
    subtree_facts: AtomicU32,
    runtime_id: AtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidNodeKind {
    pub kind: SyntaxKind,
    pub data_name: &'static str,
}

impl Node {
    /// Validate the schema's admissible kinds for this payload, not a complete
    /// parsed-tree or encoding invariant. Upstream's generic Token shape admits
    /// identifier/literal kinds too; children, concrete encoding payloads and
    /// owner/list validity require validation by the consuming runtime.
    pub fn new(
        kind: SyntaxKind,
        pos: i32,
        end: i32,
        data: NodeData,
    ) -> Result<Self, InvalidNodeKind> {
        if !data.supports_kind(kind) {
            return Err(InvalidNodeKind {
                kind,
                data_name: data.name(),
            });
        }
        Ok(Self {
            kind: kind.into(),
            parent: None,
            flags: 0,
            pos,
            end,
            data,
            subtree_facts: AtomicU32::new(0),
            runtime_id: AtomicU64::new(0),
        })
    }
    pub fn kind(&self) -> NodeKind {
        self.kind
    }
    /// Factory construction preserves Go's open kind/data pairing. Kind-specific
    /// accessors or codecs may reject a mismatched payload later.
    pub fn from_factory_parts(kind: NodeKind, data: NodeData) -> Self {
        Self {
            kind,
            parent: None,
            flags: 0,
            pos: -1,
            end: -1,
            data,
            subtree_facts: AtomicU32::new(0),
            runtime_id: AtomicU64::new(0),
        }
    }
    /// The cache exists before publication and survives same-node mutation.
    /// Only the subtree implementation interprets the source Computed bit.
    pub(crate) fn cached_subtree_facts(&self) -> u32 {
        self.subtree_facts.load(Ordering::SeqCst)
    }
    pub(crate) fn store_subtree_facts(&self, facts: u32) {
        self.subtree_facts.store(facts, Ordering::SeqCst);
    }
    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }
    pub fn set_parent(&mut self, parent: Option<NodeId>) {
        self.parent = parent;
    }
    pub fn set_range(&mut self, range: ts_core::TextRange) {
        self.pos = range.pos() as i32;
        self.end = range.end() as i32;
    }
    pub fn range(&self) -> ts_core::TextRange {
        ts_core::TextRange::new(i64::from(self.pos), i64::from(self.end))
    }
    pub fn data_mut(&mut self) -> &mut NodeData {
        &mut self.data
    }
    pub fn flags(&self) -> u32 {
        self.flags
    }
    pub fn set_flags(&mut self, flags: u32) {
        self.flags = flags;
    }
    pub fn pos(&self) -> i32 {
        self.pos
    }
    pub fn end(&self) -> i32 {
        self.end
    }
    pub fn data(&self) -> &NodeData {
        &self.data
    }
    pub fn for_each_child(&self, visitor: &mut impl ChildVisitor) -> ControlFlow<()> {
        self.for_each_child_generated(visitor)
    }
}

/// List callbacks preserve list boundaries and empty-vs-absent distinctions.
pub trait ChildVisitor {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()>;
    fn visit_list(&mut self, nodes: NodeListId) -> ControlFlow<()>;
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()>;
}

/// Maps immediate child edges into a new payload without mutating the input.
/// Allocation, recursion and retaining replacement owners are caller concerns.
pub trait ChildMapper {
    fn map_node(&mut self, node: NodeId, role: ChildRole) -> NodeId;
    fn map_list(&mut self, nodes: NodeListId, role: ChildRole) -> NodeListId;
    fn map_node_slice(&mut self, nodes: NodeSlice, role: ChildRole) -> NodeSlice;
}

/// Pinned factory-visitor dispatch. These roles select different transformation
/// hooks upstream even when the fields share a Rust storage representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildRole {
    Node,
    Nodes,
    RawNodes,
    Token,
    Modifiers,
    EmbeddedStatement,
    IterationBody,
    Parameters,
    FunctionBody,
    TopLevelStatements,
}

/// Schema storage deliberately deferred to the owning runtime phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeferredField {
    pub node: &'static str,
    pub field: &'static str,
    pub upstream_type: &'static str,
    pub owner: &'static str,
}

#[cfg(test)]
mod bind_tests;
#[cfg(test)]
mod storage_tests;
#[cfg(test)]
mod tests;

impl ts_arena::NodeRecord for Node {
    type Aux = AstStorageData;
    type CoreAux = AstStorageData;
    type Store = compact::CoreStore;
    fn storage_kind(&self) -> u32 {
        u32::from(self.kind.raw() as u16)
    }
    fn storage_reparsed(&self) -> bool {
        // Pinned NodeFlagsReparsed; no second reparsed bit is stored.
        self.flags & node_flags::REPARSED != 0
    }
}

impl ts_arena::NodeParentRecord for Node {
    fn set_storage_parent(&mut self, parent: Option<NodeId>) {
        self.parent = parent;
    }
}

impl SourceFileData {
    pub fn for_each_child(&self, visitor: &mut impl ChildVisitor) -> ControlFlow<()> {
        if let Some(statements) = self.statements {
            visitor.visit_list(statements)?;
        }
        if let Some(token) = self.end_of_file_token {
            visitor.visit_node(token)?;
        }
        ControlFlow::Continue(())
    }
}

// Structural Rust copying is not the Go factory Clone operation: it preserves
// stored fields and starts a fresh cache. Generated factory clones implement
// the source hook/count/parent behavior separately.
impl Clone for Node {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            parent: self.parent,
            flags: self.flags,
            pos: self.pos,
            end: self.end,
            data: self.data.clone(),
            subtree_facts: AtomicU32::new(0),
            runtime_id: AtomicU64::new(0),
        }
    }
}
impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.parent == other.parent
            && self.flags == other.flags
            && self.pos == other.pos
            && self.end == other.end
            && self.data == other.data
    }
}
impl Eq for Node {}

mod metadata;
pub use metadata::*;

pub mod utilities;
pub mod utilities_tail;

pub mod utilities_middle;
