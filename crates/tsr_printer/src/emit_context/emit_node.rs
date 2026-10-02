//! The rest of upstream's per-node `emitNode` record (source-map ranges, token
//! ranges, helpers, the external helpers name, the erased type node and the
//! snippet element), the context's other side tables, emit-helper requests,
//! and the context pool.

use super::{EmitContext, SideTables, SynthesizedComment};
use crate::emit_flags;
use crate::emit_helpers::EmitHelper;
use hashbrown::HashMap;
use std::sync::Mutex;
use tsr_ast::{AstView, Factory, NodeId, NodeKind, SyntaxKind};
use tsr_core::TextRange;

/// Go's `SnippetKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnippetKind {
    TabStop = 0,
}

/// Go's `SnippetElement`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnippetElement {
    pub kind: SnippetKind,
    pub order: i64,
}

/// The fields of upstream's `emitNode` that the parent module does not keep in
/// its own tables. `source_map_range` is `Some` exactly when upstream's
/// `hasSourceMapRange` flag is set.
#[derive(Clone, Debug, Default)]
pub(super) struct EmitNode {
    source_map_range: Option<TextRange>,
    token_source_map_ranges: HashMap<NodeKind, TextRange, tsr_arena::hash::FastState>,
    helpers: Vec<&'static EmitHelper>,
    external_helpers_module_name: Option<NodeId>,
    type_node: Option<NodeId>,
    snippet_element: Option<SnippetElement>,
}

impl SideTables {
    /// Upstream's `emitNodes.TryGet(node) != nil`: an operation that went
    /// through `emitNodes.Get` left an entry in one of these tables.
    pub(super) fn has_emit_node(&self, node: NodeId) -> bool {
        self.emit_flags.contains_key(&node)
            || self.comment_ranges.contains_key(&node)
            || self.leading_comments.contains_key(&node)
            || self.trailing_comments.contains_key(&node)
            || self.emit_nodes.contains_key(&node)
    }

    /// Synthetic comments and the type node are not copied, as upstream; a
    /// snippet element is copied only when the source has one.
    // port: tsc/internal/printer/emitcontext.go:emitNode.copyFrom
    pub(super) fn copy_from(&mut self, node: NodeId, source: NodeId) {
        let flags = self.emit_flags.get(&source).copied().unwrap_or(0);
        self.emit_flags.insert(node, flags);
        match self.comment_ranges.get(&source).copied() {
            Some(range) => self.comment_ranges.insert(node, range),
            None => self.comment_ranges.remove(&node),
        };
        let source = self.emit_nodes.get(&source).cloned().unwrap_or_default();
        let target = self.emit_nodes.entry(node).or_default();
        target.source_map_range = source.source_map_range;
        target.token_source_map_ranges = source.token_source_map_ranges;
        target.helpers = source.helpers;
        target.external_helpers_module_name = source.external_helpers_module_name;
        if source.snippet_element.is_some() {
            target.snippet_element = source.snippet_element;
        }
    }

    fn emit_node(&mut self, node: NodeId) -> &mut EmitNode {
        self.emit_nodes.entry(node).or_default()
    }

    /// Whether every node a retained key's metadata names is retained too.
    pub(super) fn node_references_retained(
        &self,
        retained: &mut impl FnMut(NodeId) -> bool,
    ) -> bool {
        let mut dependency_retained =
            |key: NodeId, value: Option<NodeId>| !retained(key) || value.is_none_or(&mut *retained);
        self.emit_nodes.iter().all(|(&key, node)| {
            dependency_retained(key, node.external_helpers_module_name)
                && dependency_retained(key, node.type_node)
        }) && [&self.text_source, &self.assigned_name, &self.class_this]
            .into_iter()
            .all(|table| {
                table
                    .iter()
                    .all(|(&key, &value)| dependency_retained(key, Some(value)))
            })
    }
    pub(super) fn retain_node_tables(&mut self, retained: &mut impl FnMut(NodeId) -> bool) {
        self.emit_nodes.retain(|&key, _| retained(key));
        self.text_source.retain(|&key, _| retained(key));
        self.assigned_name.retain(|&key, _| retained(key));
        self.class_this.retain(|&key, _| retained(key));
    }
    pub(super) fn node_table_bytes(&self) -> usize {
        self.emit_nodes.allocation_size()
            + self
                .emit_nodes
                .values()
                .map(|node| {
                    node.token_source_map_ranges.allocation_size()
                        + node.helpers.capacity() * size_of::<&EmitHelper>()
                })
                .sum::<usize>()
            + self.text_source.allocation_size()
            + self.assigned_name.allocation_size()
            + self.class_this.allocation_size()
    }
    pub(super) fn node_table_entries(&self) -> usize {
        self.emit_nodes.len()
            + self.text_source.len()
            + self.assigned_name.len()
            + self.class_this.len()
    }
}

/// `IsFileLevelUniqueName`'s optional global-name test.
pub type HasGlobalName<'a> = &'a dyn Fn(&[u8]) -> bool;

static EMIT_CONTEXT_POOL: Mutex<Vec<EmitContext>> = Mutex::new(Vec::new());

/// A context taken from the pool. Dropping it is upstream's release function:
/// the context is reset and returned. Clones taken from it share its tables
/// and observe the reset, as a retained `*EmitContext` does upstream.
pub struct PooledEmitContext(Option<EmitContext>);

impl std::ops::Deref for PooledEmitContext {
    type Target = EmitContext;
    fn deref(&self) -> &EmitContext {
        self.0
            .as_ref()
            .expect("pooled emit context is present until drop")
    }
}
impl std::ops::DerefMut for PooledEmitContext {
    fn deref_mut(&mut self) -> &mut EmitContext {
        self.0
            .as_mut()
            .expect("pooled emit context is present until drop")
    }
}
impl Drop for PooledEmitContext {
    fn drop(&mut self) {
        if let Some(mut context) = self.0.take() {
            context.reset();
            EMIT_CONTEXT_POOL
                .lock()
                .expect("emit context pool poisoned")
                .push(context);
        }
    }
}

// port: tsc/internal/printer/emitcontext.go:GetEmitContext
pub fn get_emit_context() -> PooledEmitContext {
    let pooled = EMIT_CONTEXT_POOL
        .lock()
        .expect("emit context pool poisoned")
        .pop();
    PooledEmitContext(Some(pooled.unwrap_or_default()))
}

impl EmitContext {
    // port: tsc/internal/printer/emitcontext.go:EmitContext.Reset
    pub fn reset(&mut self) {
        *self.tables() = SideTables::default();
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.UnsetOriginal
    pub fn unset_original(&mut self, node: NodeId) {
        self.tables().original.remove(&node);
    }

    /// `None` is upstream's nil.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.ParseNode
    pub fn parse_node(&self, factory: &dyn Factory, node: NodeId) -> Option<NodeId> {
        let node = self.most_original(node);
        if tsr_ast::utilities_positions::is_parse_tree_node(&factory.node(node)) {
            return Some(node);
        }
        None
    }

    /// `view` reads the most original source file and its identifiers.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.IsFileLevelUniqueName
    pub fn is_file_level_unique_name(
        &self,
        view: AstView<'_>,
        source_file: NodeId,
        name: &[u8],
        has_global_name: Option<HasGlobalName<'_>>,
    ) -> Result<bool, crate::Error> {
        if has_global_name.is_some_and(|has_global_name| has_global_name(name)) {
            return Ok(false);
        }
        let source_file = self.most_original(source_file);
        if view.node(source_file)?.kind() != SyntaxKind::SourceFile {
            return Err(crate::Error::InterfaceConversion {
                found: view.node(source_file)?.kind(),
                expected: "SourceFile",
            });
        }
        Ok(!tsr_ast::source_file_tables::has_identifier(
            view,
            source_file,
            name,
        )?)
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.SnippetElement
    pub fn snippet_element(&self, node: NodeId) -> Option<SnippetElement> {
        self.tables()
            .emit_nodes
            .get(&node)
            .and_then(|emit_node| emit_node.snippet_element)
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.SetSnippetElement
    pub fn set_snippet_element(&mut self, node: NodeId, snippet_element: SnippetElement) {
        self.tables().emit_node(node).snippet_element = Some(snippet_element);
    }

    /// Gets the range to use for a node when emitting source maps.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.SourceMapRange
    pub fn source_map_range(&self, factory: &dyn Factory, node: NodeId) -> TextRange {
        let range = self
            .tables()
            .emit_nodes
            .get(&node)
            .and_then(|emit_node| emit_node.source_map_range);
        range.unwrap_or_else(|| factory.node(node).range())
    }
    /// The source-map range set on `node`, if one was (upstream's
    /// `hasSourceMapRange`): what a copy of the node's emit metadata carries.
    pub(crate) fn source_map_range_if_set(&self, node: NodeId) -> Option<TextRange> {
        self.tables()
            .emit_nodes
            .get(&node)
            .and_then(|emit_node| emit_node.source_map_range)
    }
    /// Sets the range to use for a node when emitting source maps.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.SetSourceMapRange
    pub fn set_source_map_range(&mut self, node: NodeId, loc: TextRange) {
        self.tables().emit_node(node).source_map_range = Some(loc);
    }
    /// Sets the range to use for a node when emitting source maps.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.AssignSourceMapRange
    pub fn assign_source_map_range(&mut self, factory: &dyn Factory, to: NodeId, from: NodeId) {
        let range = self.source_map_range(factory, from);
        self.set_source_map_range(to, range);
    }
    /// Sets the range to use for a node when emitting comments and source maps.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.AssignCommentAndSourceMapRanges
    pub fn assign_comment_and_source_map_ranges(
        &mut self,
        factory: &dyn Factory,
        to: NodeId,
        from: NodeId,
    ) {
        self.tables().emit_node(to);
        let comment_range = self.comment_range_of(factory, from);
        let source_map_range = self.source_map_range(factory, from);
        let mut tables = self.tables();
        tables.comment_ranges.insert(to, comment_range);
        tables.emit_node(to).source_map_range = Some(source_map_range);
    }

    /// Gets the range for a token of a node when emitting source maps. `None`
    /// is upstream's `(core.TextRange{}, false)`.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.TokenSourceMapRange
    pub fn token_source_map_range(&self, node: NodeId, kind: NodeKind) -> Option<TextRange> {
        self.tables()
            .emit_nodes
            .get(&node)
            .and_then(|emit_node| emit_node.token_source_map_ranges.get(&kind).copied())
    }
    /// Sets the range for a token of a node when emitting source maps.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.SetTokenSourceMapRange
    pub fn set_token_source_map_range(&mut self, node: NodeId, kind: NodeKind, loc: TextRange) {
        self.tables()
            .emit_node(node)
            .token_source_map_ranges
            .insert(kind, loc);
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.AssignedName
    pub fn assigned_name(&self, node: NodeId) -> Option<NodeId> {
        self.tables().assigned_name.get(&node).copied()
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.TextSource
    pub fn text_source(&self, node: NodeId) -> Option<NodeId> {
        self.tables().text_source.get(&node).copied()
    }
    pub(super) fn set_text_source(&mut self, node: NodeId, text_source: NodeId) {
        self.tables().text_source.insert(node, text_source);
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.SetAssignedName
    pub fn set_assigned_name(&mut self, node: NodeId, name: NodeId) {
        self.tables().assigned_name.insert(node, name);
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.ClassThis
    pub fn class_this(&self, node: NodeId) -> Option<NodeId> {
        self.tables().class_this.get(&node).copied()
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.SetClassThis
    pub fn set_class_this(&mut self, node: NodeId, class_this: NodeId) {
        self.tables().class_this.insert(node, class_this);
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.RequestEmitHelper
    pub fn request_emit_helper(&mut self, helper: &'static EmitHelper) {
        assert!(!helper.scoped, "Cannot request a scoped emit helper");
        for dependency in helper.dependencies {
            self.request_emit_helper(dependency);
        }
        let mut tables = self.tables();
        if !tables.emit_helpers.contains(&helper) {
            tables.emit_helpers.push(helper);
        }
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.ReadEmitHelpers
    pub fn read_emit_helpers(&mut self) -> Vec<&'static EmitHelper> {
        std::mem::take(&mut self.tables().emit_helpers)
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.AddEmitHelper
    pub fn add_emit_helper(&mut self, node: NodeId, helpers: &[&'static EmitHelper]) {
        let mut tables = self.tables();
        let emit_node = tables.emit_node(node);
        for &helper in helpers {
            append_if_unique(&mut emit_node.helpers, helper);
        }
    }
    /// The predicate runs with no table lock held, once per source helper in
    /// order, so it may read this context: it sees the target's helpers as
    /// they are after the moves before it.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.MoveEmitHelpers
    pub fn move_emit_helpers(
        &mut self,
        source: NodeId,
        target: NodeId,
        mut predicate: impl FnMut(&'static EmitHelper) -> bool,
    ) {
        let Some(source_emit_helpers) = self
            .tables()
            .emit_nodes
            .get(&source)
            .map(|emit_node| emit_node.helpers.clone())
        else {
            return;
        };
        if source_emit_helpers.is_empty() {
            return;
        }
        self.tables().emit_node(target);
        let mut helpers_removed = 0;
        let mut kept = Vec::with_capacity(source_emit_helpers.len());
        for &helper in &source_emit_helpers {
            if predicate(helper) {
                helpers_removed += 1;
                append_if_unique(&mut self.tables().emit_node(target).helpers, helper);
            } else {
                kept.push(helper);
            }
        }
        if helpers_removed > 0 {
            self.tables().emit_node(source).helpers = kept;
        }
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.GetEmitHelpers
    pub fn get_emit_helpers(&self, node: NodeId) -> Vec<&'static EmitHelper> {
        self.tables()
            .emit_nodes
            .get(&node)
            .map(|emit_node| emit_node.helpers.clone())
            .unwrap_or_default()
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.GetExternalHelpersModuleName
    pub fn get_external_helpers_module_name(
        &self,
        factory: &dyn Factory,
        node: NodeId,
    ) -> Option<NodeId> {
        let parse_node = self.parse_node(factory, node)?;
        self.tables()
            .emit_nodes
            .get(&parse_node)
            .and_then(|emit_node| emit_node.external_helpers_module_name)
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.SetExternalHelpersModuleName
    pub fn set_external_helpers_module_name(
        &mut self,
        factory: &dyn Factory,
        node: NodeId,
        name: Option<NodeId>,
    ) {
        let parse_node = self.parse_node(factory, node).expect(
            "Node must be a parse tree node or have an Original pointer to a parse tree node.",
        );
        self.tables()
            .emit_node(parse_node)
            .external_helpers_module_name = name;
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.HasRecordedExternalHelpers
    pub fn has_recorded_external_helpers(&self, factory: &dyn Factory, node: NodeId) -> bool {
        if let Some(parse_node) = self.parse_node(factory, node) {
            let tables = self.tables();
            return tables.has_emit_node(parse_node)
                && (tables
                    .emit_nodes
                    .get(&parse_node)
                    .is_some_and(|emit_node| emit_node.external_helpers_module_name.is_some())
                    || tables.emit_flags.get(&parse_node).copied().unwrap_or(0)
                        & emit_flags::EXTERNAL_HELPERS
                        != 0);
        }
        false
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.IsCallToHelper
    pub fn is_call_to_helper(
        &self,
        factory: &dyn Factory,
        first_segment: NodeId,
        helper_name: &[u8],
    ) -> bool {
        let read = factory.node(first_segment);
        if read.kind() != SyntaxKind::CallExpression {
            return false;
        }
        let expression = read
            .expression()
            .expect("runtime error: invalid memory address or nil pointer dereference");
        let expression_read = factory.node(expression);
        let Some(identifier) = expression_read.as_identifier() else {
            return false;
        };
        self.emit_flags(expression) & emit_flags::HELPER_NAME != 0
            && identifier.text() == helper_name
    }

    // port: tsc/internal/printer/emitcontext.go:EmitContext.SetSyntheticLeadingComments
    pub fn set_synthetic_leading_comments(
        &mut self,
        node: NodeId,
        comments: Vec<SynthesizedComment>,
    ) -> NodeId {
        self.tables().leading_comments.insert(node, comments);
        node
    }
    // port: tsc/internal/printer/emitcontext.go:EmitContext.SetSyntheticTrailingComments
    pub fn set_synthetic_trailing_comments(
        &mut self,
        node: NodeId,
        comments: Vec<SynthesizedComment>,
    ) -> NodeId {
        self.tables().trailing_comments.insert(node, comments);
        node
    }

    /// Stores the original type node on a name node when the type is erased,
    /// so the emitter can use the type's position for comment preservation.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.SetTypeNode
    pub fn set_type_node(&mut self, node: NodeId, type_node: Option<NodeId>) {
        self.tables().emit_node(node).type_node = type_node;
    }
    /// Gets the type node stored on a name node by the type eraser.
    // port: tsc/internal/printer/emitcontext.go:EmitContext.GetTypeNode
    pub fn get_type_node(&self, node: NodeId) -> Option<NodeId> {
        self.tables()
            .emit_nodes
            .get(&node)
            .and_then(|emit_node| emit_node.type_node)
    }
}

/// `core.AppendIfUnique` over helper identities.
fn append_if_unique(helpers: &mut Vec<&'static EmitHelper>, helper: &'static EmitHelper) {
    if !helpers.contains(&helper) {
        helpers.push(helper);
    }
}
