use crate::{
    AstBuilder, AstTransaction, AstView, Node, NodeData, NodeId, NodeKind, NodeMut, NodeRead,
    SourceFileRead, SourceFileState,
};
use ts_arena::Error;
use ts_core::TextRange;

/// Hooks receive identities and an exclusive factory context, with no outstanding
/// node borrow. An immutable callback can reenter construction or mutate the
/// original core node. A present hook is retained explicitly for each invocation;
/// ordinary construction with no hook performs no hook reference-count operation.
pub trait FactoryHooks: Send + Sync {
    fn on_create(&self, _factory: &mut dyn Factory, _node: NodeId) {}
    fn on_update(&self, _factory: &mut dyn Factory, _node: NodeId, _original: NodeId) {}
    fn on_clone(&self, _factory: &mut dyn Factory, _node: NodeId, _original: NodeId) {}
}

/// Storage operations consumed by the authoritative generated constructors.
/// IDs are checked at this Rust ownership boundary even in release builds.
pub trait Factory {
    fn node(&self, id: NodeId) -> NodeRead<'_>;
    fn node_count(&self) -> i64;
    fn text_count(&self) -> i64;
    fn node_mut(&mut self, id: NodeId) -> NodeMut<'_>;
    /// Rich SourceFile metadata belongs to a complete owner. A lazy JSDoc
    /// transaction has no authority to create or mutate another source frame.
    fn read_source_file(&self, _id: NodeId) -> Result<SourceFileRead<'_>, Error> {
        Err(Error::InvalidGraph)
    }
    fn mut_source_file(&mut self, _id: NodeId) -> Result<&mut SourceFileState, Error> {
        Err(Error::InvalidGraph)
    }
    fn new_node(&mut self, kind: NodeKind, data: NodeData) -> NodeId;
    crate::factory_generated::factory_construction_methods!(defaults);
    // port: tsc/internal/ast/ast.go:NodeFactory.NewModifier
    fn new_modifier(&mut self, kind: NodeKind) -> NodeId {
        self.new_token_data(kind, crate::TokenData {})
    }
    fn increment_text_count(&mut self);
    fn set_node_flags(&mut self, id: NodeId, flags: u32);
    /// Header-only writes do not expose a mutable syntax payload. Parent edges
    /// keep the factory's existing validation timing; this is not an early
    /// owner check or a proof that a new parent belongs to retained storage.
    fn set_node_parent(&mut self, id: NodeId, parent: Option<NodeId>) {
        self.node_mut(id).set_parent(parent);
    }
    /// Preserve Go's signed int32 narrowing at the node's range boundary.
    fn set_node_range(&mut self, id: NodeId, range: TextRange) {
        self.node_mut(id).set_range(range);
    }
    fn add_node_flags(&mut self, id: NodeId, flags: u32) {
        let mut node = self.node_mut(id);
        let combined_flags = node.flags() | flags;
        node.set_flags(combined_flags);
    }
    /// Parser completion sets the range before adding context/error flags.
    /// Keep that ordinary operation to one mutable header lookup.
    fn finish_node(&mut self, id: NodeId, range: TextRange, flags: u32) {
        let mut node = self.node_mut(id);
        node.set_range(range);
        let combined_flags = node.flags() | flags;
        node.set_flags(combined_flags);
    }
    fn finish_update(&mut self, updated: NodeId, original: NodeId) -> NodeId;
    fn finish_clone(&mut self, updated: NodeId, original: NodeId) -> NodeId;
}
impl AstBuilder {
    /// Factory copies observe completed binding writes on retained source
    /// owners. Ordinary builder views and mutable construction remain parsed.
    /// This borrows the publication cell without initializing it or retaining
    /// another owner; only imported nodes need logical-source resolution.
    pub(crate) fn factory_view(&self, id: NodeId) -> Result<AstView<'_>, Error> {
        let parsed = self.view();
        if id.arena() == self.id().arena() {
            return Ok(parsed);
        }
        if parsed.for_node_owner(id)?.0.id() == self.id() {
            return Ok(parsed);
        }
        let source = match parsed.owning_source(id) {
            Ok(source) => source,
            // Parentless synthetic nodes have no logical binding selection.
            Err(Error::InvalidGraph) => return Ok(parsed),
            Err(error) => return Err(error),
        };
        let binding = parsed.source_file(source)?.state_ref().binding.result();
        Ok(AstView(parsed.0, binding))
    }

    pub(crate) fn new_node_before_hook(&mut self, kind: NodeKind, data: NodeData) -> NodeId {
        self.view()
            .validate_data(&data)
            .expect("factory edges belong to retained storage");
        let frame = self.frame_mut();
        frame.node_count = frame.node_count.wrapping_add(1);
        self.push_node(Node::from_factory_parts(kind, data))
    }

    pub(crate) fn run_create_hook(&mut self, node: NodeId) {
        if let Some(hooks) = self.hooks.clone() {
            hooks.on_create(self, node);
        }
    }
}
impl Factory for AstBuilder {
    crate::factory_generated::factory_construction_methods!(builder);

    fn read_source_file(&self, id: NodeId) -> Result<SourceFileRead<'_>, Error> {
        self.factory_view(id)?.source_file(id)
    }
    fn mut_source_file(&mut self, id: NodeId) -> Result<&mut SourceFileState, Error> {
        self.source_file_mut(id)
    }
    fn node_count(&self) -> i64 {
        AstBuilder::node_count(self)
    }
    fn text_count(&self) -> i64 {
        AstBuilder::text_count(self)
    }
    fn node(&self, id: NodeId) -> NodeRead<'_> {
        self.factory_view(id)
            .expect("factory node belongs to retained storage")
            .node(id)
            .expect("factory node belongs to retained storage")
    }
    fn node_mut(&mut self, id: NodeId) -> NodeMut<'_> {
        AstBuilder::node_mut(self, id).expect("factory owns mutable core node")
    }
    fn new_node(&mut self, kind: NodeKind, data: NodeData) -> NodeId {
        let node = self.new_node_before_hook(kind, data);
        self.run_create_hook(node);
        node
    }
    fn increment_text_count(&mut self) {
        let frame = self.frame_mut();
        frame.text_count = frame.text_count.wrapping_add(1);
    }
    fn set_node_flags(&mut self, id: NodeId, flags: u32) {
        self.storage
            .node_mut(id)
            .expect("factory owns mutable node")
            .set_flags(flags);
    }
    fn set_node_parent(&mut self, id: NodeId, parent: Option<NodeId>) {
        self.write_parent(id, parent)
            .expect("factory owns mutable core node");
    }
    fn set_node_range(&mut self, id: NodeId, range: TextRange) {
        self.finish_header(id, range, 0, false)
            .expect("factory owns mutable core node");
    }
    fn add_node_flags(&mut self, id: NodeId, flags: u32) {
        let node = self
            .storage
            .node_mut(id)
            .expect("factory owns mutable core node");
        node.set_flags(node.flags | flags);
    }
    fn finish_node(&mut self, id: NodeId, range: TextRange, flags: u32) {
        self.finish_header(id, range, flags, false)
            .expect("factory owns mutable core node");
    }
    fn finish_update(&mut self, updated: NodeId, original: NodeId) -> NodeId {
        if updated != original {
            let original_read = self.node(original);
            let (flags, range) = (original_read.flags(), original_read.range());
            self.finish_header(updated, range, flags, true)
                .expect("factory owns updated core node");
            if let Some(hooks) = self.hooks.clone() {
                hooks.on_update(self, updated, original);
            }
        }
        updated
    }
    fn finish_clone(&mut self, updated: NodeId, original: NodeId) -> NodeId {
        let updated = self.finish_update(updated, original);
        if updated != original {
            if let Some(hooks) = self.hooks.clone() {
                hooks.on_clone(self, updated, original);
            }
        }
        updated
    }
}
impl Factory for AstTransaction<'_, '_> {
    fn node_count(&self) -> i64 {
        self.node_count
    }
    fn text_count(&self) -> i64 {
        self.text_count
    }
    fn node(&self, id: NodeId) -> NodeRead<'_> {
        AstTransaction::node(self, id).expect("transaction node belongs to this file")
    }
    fn node_mut(&mut self, id: NodeId) -> NodeMut<'_> {
        AstTransaction::node_mut(self, id).expect("transaction owns mutable staged node")
    }
    fn new_node(&mut self, kind: NodeKind, data: NodeData) -> NodeId {
        self.validate_data(&data)
            .expect("transaction edges belong to this file");
        self.node_count = self.node_count.wrapping_add(1);
        self.push_node(Node::from_factory_parts(kind, data))
    }
    fn increment_text_count(&mut self) {
        self.text_count = self.text_count.wrapping_add(1);
    }
    fn set_node_flags(&mut self, id: NodeId, flags: u32) {
        AstTransaction::node_mut(self, id)
            .expect("transaction owns mutable node")
            .set_flags(flags);
    }
    fn finish_update(&mut self, updated: NodeId, original: NodeId) -> NodeId {
        if updated != original {
            let original = Factory::node(self, original);
            let (flags, range) = (original.flags(), original.range());
            let mut updated = Factory::node_mut(self, updated);
            updated.set_flags(flags);
            updated.set_range(range);
        }
        updated
    }
    fn finish_clone(&mut self, updated: NodeId, original: NodeId) -> NodeId {
        self.finish_update(updated, original)
    }
}

// Borrowed transaction adapters preserve exclusive construction without moving
// the transaction out of its publication callback.
pub struct BorrowedFactory<'a, T: ?Sized>(pub &'a mut T);

impl<T: Factory + ?Sized> Factory for BorrowedFactory<'_, T> {
    crate::factory_generated::factory_construction_methods!(forward, 0);

    fn read_source_file(&self, id: NodeId) -> Result<SourceFileRead<'_>, Error> {
        self.0.read_source_file(id)
    }
    fn mut_source_file(&mut self, id: NodeId) -> Result<&mut SourceFileState, Error> {
        self.0.mut_source_file(id)
    }
    fn node(&self, id: NodeId) -> NodeRead<'_> {
        self.0.node(id)
    }
    fn node_count(&self) -> i64 {
        self.0.node_count()
    }
    fn text_count(&self) -> i64 {
        self.0.text_count()
    }
    fn node_mut(&mut self, id: NodeId) -> NodeMut<'_> {
        self.0.node_mut(id)
    }
    fn new_node(&mut self, kind: NodeKind, data: NodeData) -> NodeId {
        self.0.new_node(kind, data)
    }
    fn new_modifier(&mut self, kind: NodeKind) -> NodeId {
        self.0.new_modifier(kind)
    }
    fn increment_text_count(&mut self) {
        self.0.increment_text_count();
    }
    fn set_node_flags(&mut self, id: NodeId, flags: u32) {
        self.0.set_node_flags(id, flags);
    }
    fn set_node_parent(&mut self, id: NodeId, parent: Option<NodeId>) {
        self.0.set_node_parent(id, parent);
    }
    fn set_node_range(&mut self, id: NodeId, range: TextRange) {
        self.0.set_node_range(id, range);
    }
    fn add_node_flags(&mut self, id: NodeId, flags: u32) {
        self.0.add_node_flags(id, flags);
    }
    fn finish_node(&mut self, id: NodeId, range: TextRange, flags: u32) {
        self.0.finish_node(id, range, flags);
    }
    fn finish_update(&mut self, updated: NodeId, original: NodeId) -> NodeId {
        self.0.finish_update(updated, original)
    }
    fn finish_clone(&mut self, updated: NodeId, original: NodeId) -> NodeId {
        self.0.finish_clone(updated, original)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FactoryMethods, JsString};
    use ts_arena::Counters;
    use ts_jsstring::SourceText;

    #[test]
    fn header_operations_keep_signed_ranges_flags_and_deferred_parent_validation() {
        let mut ast = AstBuilder::new(SourceText::default(), &Counters::new());
        let child = ast.new_identifier(JsString::default());
        let parent = ast.new_identifier(JsString::default());
        {
            let mut factory = BorrowedFactory(&mut ast);
            factory.set_node_flags(child, 1);
            factory.set_node_parent(child, Some(parent));
            factory.finish_node(child, TextRange::new(i64::from(i32::MAX) + 1, -2), 2);
            factory.add_node_flags(child, 4);
            let node = factory.node(child);
            assert_eq!(node.flags(), 7);
            assert_eq!(node.range(), TextRange::new(i64::from(i32::MIN), -2));
            assert_eq!(node.parent(), Some(parent));
        }
        let mut other = AstBuilder::new(SourceText::default(), &Counters::new());
        let foreign = other.new_identifier(JsString::default());
        // A header write retains construction's existing validation boundary:
        // storing an invalid parent succeeds, and completion rejects the edge.
        Factory::set_node_range(&mut ast, child, TextRange::new(-1, -1));
        Factory::set_node_parent(&mut ast, child, Some(foreign));
        assert_eq!(ast.node(child).parent(), Some(foreign));
        assert!(matches!(ast.complete(child), Err(Error::WrongOwner)));
    }
}
