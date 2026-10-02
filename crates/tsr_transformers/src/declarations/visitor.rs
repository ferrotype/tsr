//! `tx.Visitor()`: the declaration transformer's node visitor, with the pin's
//! `NodeVisitor` rules (`ast/visitor.go`) over `DeclarationTransformer.visit`.
//! The generated child visitor is infallible, so this adapter records the
//! first resolver error and stops visiting; `visit_each_child` returns that
//! error before any rewritten node becomes visible to the caller.
use super::transform::Transformer;
use tsr_ast::{
    ChildRole, Factory, NodeData, NodeId, NodeKind, NodeListId, NodeMut, NodeRead, NodeSlice,
    RuntimeFactory, SourceFileRead, SourceFileState, SyntaxKind as K, VisitContext,
};
use tsr_printer::emit_resolver::DeclarationEmitResolver;

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    /// `Visitor().Visit(node)`: the visitor callback with the errors of a
    /// generated child visit surfaced.
    fn visit_recording(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        if self.visitor_error.is_some() {
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
    /// `Visitor().VisitNode(node)`: a syntax list result must hold exactly
    /// one node, which replaces the input.
    pub fn visit_single(&mut self, node: Option<NodeId>) -> Result<Option<NodeId>, R::Error> {
        let Some(node) = node else { return Ok(None) };
        let mut visited = self.visit(Some(node))?;
        if let Some(id) = visited {
            if self.kind(id) == K::SyntaxList {
                let children = self.node_or_syntax_list_children(id);
                assert!(
                    children.len() == 1,
                    "Expected only a single node to be written to output"
                );
                visited = Some(children[0]);
                assert!(
                    self.kind(children[0]) != K::SyntaxList,
                    "The result of visiting and lifting a Node may not be SyntaxList"
                );
            }
        }
        Ok(visited)
    }
    /// `liftToBlock`: a syntax list result of an embedded statement becomes
    /// its only child or a block.
    fn lift_to_block(&mut self, node: Option<NodeId>) -> NodeId {
        let nodes = match node {
            Some(id) if self.kind(id) == K::SyntaxList => self.node_or_syntax_list_children(id),
            Some(id) => return id,
            None => Vec::new(),
        };
        let id = if nodes.len() == 1 {
            nodes[0]
        } else {
            let list = self.new_node_list(nodes);
            tsr_ast::FactoryMethods::new_block(&mut *self.output, Some(list), true)
        };
        assert!(
            self.kind(id) != K::SyntaxList,
            "The result of visiting and lifting a Node may not be SyntaxList"
        );
        id
    }
    fn visit_list_role(&mut self, list: Option<NodeListId>, modifiers: bool) -> Option<NodeListId> {
        if self.visitor_error.is_some() {
            return list;
        }
        let result = if modifiers {
            self.visit_modifiers(list)
        } else {
            self.visit_nodes(list)
        };
        match result {
            Ok(list) => list,
            Err(error) => {
                self.visitor_error = Some(error);
                list
            }
        }
    }
    /// `Visitor().VisitModifiers(list)`.
    pub fn visit_modifiers(
        &mut self,
        list: Option<NodeListId>,
    ) -> Result<Option<NodeListId>, R::Error> {
        let Some(original) = list else {
            return Ok(None);
        };
        let nodes = self.list_nodes(list);
        let (visited, changed) = self.visit_slice(&nodes)?;
        if !changed {
            return Ok(list);
        }
        let result = self.new_modifier_list(visited);
        let loc = self.output.read_list(original).loc();
        self.output.set_list_location(result, loc)?;
        Ok(Some(result))
    }
}

impl<R: DeclarationEmitResolver> VisitContext for Transformer<'_, R> {
    fn visit_node(&mut self, node: Option<NodeId>, role: ChildRole) -> Option<NodeId> {
        if self.visitor_error.is_some() || node.is_none() {
            return node;
        }
        match role {
            ChildRole::EmbeddedStatement | ChildRole::IterationBody => {
                let visited = self.visit_recording(node);
                if self.visitor_error.is_some() {
                    return node;
                }
                visited.map(|visited| self.lift_to_block(Some(visited)))
            }
            _ => match self.visit_single(node) {
                Ok(visited) => visited,
                Err(error) => {
                    self.visitor_error = Some(error);
                    node
                }
            },
        }
    }
    fn visit_list(&mut self, list: Option<NodeListId>, role: ChildRole) -> Option<NodeListId> {
        self.visit_list_role(list, role == ChildRole::Modifiers)
    }
    fn map_raw_nodes(&mut self, nodes: NodeSlice) -> NodeSlice {
        let original: Vec<_> = self.output.read_nodes(nodes).iter().collect();
        let mut updated: Option<Vec<Option<NodeId>>> = None;
        for (index, &node) in original.iter().enumerate() {
            let visited = VisitContext::visit_node(self, node, ChildRole::Node);
            if let Some(updated) = &mut updated {
                updated.push(visited);
            } else if visited != node {
                let mut prefix = original[..index].to_vec();
                prefix.push(visited);
                updated = Some(prefix);
            }
        }
        updated.map_or(nodes, |nodes| self.output.alloc_nodes(nodes))
    }
    /// `SourceFile.VisitEachChild` with this visitor.
    fn visit_each_child_source_file(&mut self, node: NodeId) -> NodeId {
        let (statements, end_of_file_token) = {
            let read = self.node(node);
            let data = read.as_source_file().expect("SourceFile payload");
            (data.statements(), data.end_of_file_token())
        };
        let statements = self.visit_list_role(statements, false);
        let end_of_file_token = VisitContext::visit_node(self, end_of_file_token, ChildRole::Token);
        self.output
            .update_source(node, statements, end_of_file_token)
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
    fn read_source_file(&self, node: NodeId) -> Result<SourceFileRead<'_>, tsr_arena::Error> {
        self.output.read_source_file(node)
    }
    fn mut_source_file(&mut self, node: NodeId) -> Result<&mut SourceFileState, tsr_arena::Error> {
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
