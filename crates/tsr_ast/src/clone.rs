use crate::{
    node_flags, ChildVisitor, FactoryMethods, NodeDataRead, NodeId, NodeListId, NodeSlice,
    NodeVisitor, NodeVisitorHooks, RuntimeFactory,
};
use std::ops::ControlFlow;
use ts_core::TextRange;

// port: tsc/internal/ast/ast.go:Node.Clone
pub fn clone_node(factory: &mut dyn RuntimeFactory, original: NodeId) -> NodeId {
    if let Some(cloned) = factory.clone_node_generated(original) {
        return cloned;
    }
    let source = matches!(factory.node(original).data(), NodeDataRead::SourceFile(_));
    if source {
        return factory.clone_source(original);
    }
    panic!("SyntheticExpression clone requires the deferred checker-owned type runtime")
}

// port: tsc/internal/ast/deepclone.go:getDeepCloneVisitor
fn with_deep_clone<T>(
    factory: &mut dyn RuntimeFactory,
    synthetic: bool,
    operation: impl FnOnce(&mut NodeVisitor<'_>) -> T,
) -> T {
    let visit = |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
        let visited = visitor.visit_each_child(node);
        let cloned = if visited == node {
            clone_node(visitor.factory_mut(), node.expect("nil deep clone input"))
        } else {
            visited.expect("nil deep clone result")
        };
        if synthetic {
            visitor
                .factory_mut()
                .set_node_range(cloned, TextRange::new(-1, -1));
        }
        Some(cloned)
    };
    let nodes = |visitor: &mut NodeVisitor<'_>, list: Option<NodeListId>| {
        clone_visited_list(visitor, list, synthetic, false)
    };
    let modifiers = |visitor: &mut NodeVisitor<'_>, list: Option<NodeListId>| {
        clone_visited_list(visitor, list, synthetic, true)
    };
    let hooks = NodeVisitorHooks {
        visit_nodes: Some(&nodes),
        visit_modifiers: Some(&modifiers),
        ..NodeVisitorHooks::default()
    };
    operation(&mut NodeVisitor::new(Some(&visit), Some(factory), hooks))
}
fn clone_visited_list(
    visitor: &mut NodeVisitor<'_>,
    list: Option<NodeListId>,
    synthetic: bool,
    modifiers: bool,
) -> Option<NodeListId> {
    let original = list?;
    let visited = if modifiers {
        visitor.visit_modifiers(list)
    } else {
        visitor.visit_nodes(list)
    };
    let new = if visited == list {
        if modifiers {
            visitor.factory_mut().clone_modifier_list_header(original)
        } else {
            visitor.factory_mut().clone_list_header(original)
        }
    } else {
        visited.expect("nil cloned list")
    };
    if synthetic {
        visitor
            .factory_mut()
            .mutable_list(new)
            .set_loc(TextRange::new(-1, -1));
        if visitor.factory().list_has_trailing_comma(original) {
            let nodes = visitor.factory().read_list(new).nodes();
            let last = visitor
                .factory()
                .read_nodes(nodes)
                .at(nodes.len() - 1)
                .expect("nil trailing-comma clone");
            visitor
                .factory_mut()
                .set_node_range(last, TextRange::new(-2, -2));
        }
    }
    Some(new)
}
// port: tsc/internal/ast/deepclone.go:NodeFactory.DeepCloneNode
pub fn deep_clone_node(factory: &mut dyn RuntimeFactory, node: Option<NodeId>) -> Option<NodeId> {
    with_deep_clone(factory, true, |visitor| visitor.visit_node(node))
}
// port: tsc/internal/ast/deepclone.go:NodeFactory.DeepCloneReparse
pub fn deep_clone_reparse(
    factory: &mut dyn RuntimeFactory,
    node: Option<NodeId>,
) -> Option<NodeId> {
    let node = node?;
    let cloned = with_deep_clone(factory, false, |visitor| visitor.visit_node(Some(node)))
        .expect("deep clone root");
    set_parent_in_children(factory, cloned);
    let flags = factory.node(cloned).flags() | node_flags::REPARSED;
    factory.set_node_flags(cloned, flags);
    Some(cloned)
}
// port: tsc/internal/ast/deepclone.go:NodeFactory.DeepCloneReparseModifiers
pub fn deep_clone_reparse_modifiers(
    factory: &mut dyn RuntimeFactory,
    list: Option<NodeListId>,
) -> Option<NodeListId> {
    with_deep_clone(factory, false, |visitor| visitor.visit_modifiers(list))
}

/// Set every descendant's immediate parent in depth-first child order. Repeated
/// shared edges are visited repeatedly, as upstream; this does not accept cycles.
// port: tsc/internal/ast/utilities.go:SetParentInChildren
pub fn set_parent_in_children(factory: &mut dyn RuntimeFactory, root: NodeId) {
    let mut pending = vec![(Some(root), None)];
    let mut children = Vec::new();
    while let Some((node, parent)) = pending.pop() {
        let node = node.expect("nil child in SetParentInChildren");
        if let Some(parent) = parent {
            factory.set_node_parent(node, Some(parent));
        }
        children.clear();
        let value = factory.node(node);
        let _ = value.for_each_child(&mut ParentEdges {
            factory,
            nodes: &mut children,
        });
        drop(value);
        pending.extend(children.iter().rev().map(|&child| (child, Some(node))));
    }
}
struct ParentEdges<'a> {
    factory: &'a dyn RuntimeFactory,
    nodes: &'a mut Vec<Option<NodeId>>,
}
impl ChildVisitor for ParentEdges<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.nodes.push(Some(node));
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        self.visit_node_slice(self.factory.read_list(list).nodes())
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        self.nodes.extend(self.factory.read_nodes(nodes).iter());
        ControlFlow::Continue(())
    }
}
