use super::transform::Transformer;
use ts_ast::{
    ChildRole, Factory, NodeData, NodeId, NodeKind, NodeListId, NodeMut, NodeRead, NodeSlice,
    RuntimeFactory, SourceFileRead, SourceFileState, VisitContext,
};
use ts_printer::emit_resolver::DeclarationEmitResolver;

/// The generated native visitor is infallible. This adapter records the first
/// resolver error and prevents further visits; `children` returns that error
/// before any rewritten root becomes visible to the caller.
impl<R: DeclarationEmitResolver> VisitContext for Transformer<'_, R> {
    fn visit_node(&mut self, node: Option<NodeId>, role: ChildRole) -> Option<NodeId> {
        if self.visitor_error.is_some() || matches!(role, ChildRole::Token) {
            return node;
        }
        match self.visit(node) {
            Ok(node) => node,
            Err(error) => {
                self.visitor_error = Some(error);
                node
            }
        }
    }
    fn visit_list(&mut self, list: Option<NodeListId>, _role: ChildRole) -> Option<NodeListId> {
        if self.visitor_error.is_some() {
            return list;
        }
        match self.visit_list_result(list) {
            Ok(list) => list,
            Err(error) => {
                self.visitor_error = Some(error);
                list
            }
        }
    }
    fn map_raw_nodes(&mut self, nodes: NodeSlice) -> NodeSlice {
        let original: Vec<_> = self.output.read_nodes(nodes).iter().collect();
        let mapped: Vec<_> = original
            .iter()
            .map(|node| VisitContext::visit_node(self, *node, ChildRole::Node))
            .collect();
        if original == mapped {
            nodes
        } else {
            self.output.alloc_nodes(mapped)
        }
    }
    fn visit_each_child_source_file(&mut self, node: NodeId) -> NodeId {
        if self.visitor_error.is_some() {
            return node;
        }
        match self.source_file(node) {
            Ok(node) => node,
            Err(error) => {
                self.visitor_error = Some(error);
                node
            }
        }
    }
}
impl<R: DeclarationEmitResolver> Factory for Transformer<'_, R> {
    fn node(&self, node: NodeId) -> NodeRead<'_> {
        Factory::node(&*self.output, node)
    }
    fn node_count(&self) -> i64 {
        Factory::node_count(&*self.output)
    }
    fn text_count(&self) -> i64 {
        Factory::text_count(&*self.output)
    }
    fn node_mut(&mut self, node: NodeId) -> NodeMut<'_> {
        Factory::node_mut(&mut *self.output, node)
    }
    fn read_source_file(&self, node: NodeId) -> Result<SourceFileRead<'_>, ts_arena::Error> {
        self.output.read_source_file(node)
    }
    fn mut_source_file(&mut self, node: NodeId) -> Result<&mut SourceFileState, ts_arena::Error> {
        self.output.mut_source_file(node)
    }
    fn new_node(&mut self, kind: NodeKind, data: NodeData) -> NodeId {
        self.output.new_node(kind, data)
    }
    fn increment_text_count(&mut self) {
        self.output.increment_text_count();
    }
    fn set_node_flags(&mut self, node: NodeId, flags: u32) {
        self.output.set_node_flags(node, flags);
    }
    fn finish_update(&mut self, node: NodeId, original: NodeId) -> NodeId {
        self.output.finish_update(node, original)
    }
    fn finish_clone(&mut self, node: NodeId, original: NodeId) -> NodeId {
        self.output.finish_clone(node, original)
    }
}
