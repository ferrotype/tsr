//! Explicit traversal frames preserve VisitEachChild order and defer JSDoc until
//! ordinary descendants have completed, without consuming the native call stack.
use ts_arena::Error;
use ts_ast::{AstView, JsDocProvider, NodeId, NodeListId};
#[derive(Clone, Copy)]
pub(crate) enum Edge {
    Node(NodeId),
    List(NodeListId),
}
enum Pending {
    Edge(Edge, u32),
    Children(NodeId, u32),
    JsDoc(NodeId, u32),
}
pub(crate) struct Walk<'a> {
    view: AstView<'a>,
    source: Option<NodeId>,
    pending: Vec<Pending>,
    scratch: Vec<Edge>,
    index: u32,
}
impl<'a> Walk<'a> {
    pub(crate) fn new(view: AstView<'a>, root: NodeId, source: Option<NodeId>) -> Self {
        Self {
            view,
            source,
            pending: vec![Pending::Edge(Edge::Node(root), 0)],
            scratch: Vec::new(),
            index: 0,
        }
    }
    pub(crate) fn next(
        &mut self,
        provider: &mut impl JsDocProvider,
    ) -> Result<Option<(u32, u32, Edge)>, Error> {
        while let Some(pending) = self.pending.pop() {
            match pending {
                Pending::Children(id, index) => {
                    let node = self.view.node(id)?;
                    if self.source.is_some() {
                        self.pending.push(Pending::JsDoc(id, index));
                    }
                    crate::runtime_generated::children(self.view, &node, &mut self.scratch);
                    self.pending.extend(
                        self.scratch
                            .drain(..)
                            .rev()
                            .map(|edge| Pending::Edge(edge, index)),
                    );
                }
                Pending::JsDoc(parent, index) => {
                    let roots = provider.jsdoc(
                        self.view,
                        self.source.expect("JSDoc has source context"),
                        parent,
                    )?;
                    self.pending.extend(
                        roots
                            .iter()
                            .rev()
                            .copied()
                            .map(|id| Pending::Edge(Edge::Node(id), index)),
                    );
                }
                Pending::Edge(edge, parent) => {
                    self.index = self.index.wrapping_add(1);
                    let index = self.index;
                    match edge {
                        Edge::Node(id) => {
                            self.pending.push(Pending::Children(id, index));
                        }
                        Edge::List(id) => {
                            let list = self.view.list(id)?;
                            let children = self.view.node_slice(list.nodes())?;
                            self.pending.extend(
                                children
                                    .iter()
                                    .rev()
                                    .flatten()
                                    .map(|id| Pending::Edge(Edge::Node(id), index)),
                            );
                        }
                    }
                    return Ok(Some((index, parent, edge)));
                }
            }
        }
        Ok(None)
    }
}
