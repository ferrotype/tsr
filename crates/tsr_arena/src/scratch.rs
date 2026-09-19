use crate::{arena::Arena, counters::Track, ArenaId, Counters, Error, Node, NodeId};

/// Request-local storage. References cannot outlive the exclusive scratch owner.
pub struct ScratchOwner<N> {
    nodes: Arena<Node<N>>,
    _owner: Track,
}

impl<N> ScratchOwner<N> {
    pub fn new(counters: &Counters) -> Self {
        Self {
            nodes: Arena::new(counters),
            _owner: counters.owner(),
        }
    }

    pub fn arena_id(&self) -> ArenaId {
        self.nodes.id
    }

    pub fn push(&mut self, node: Node<N>) -> NodeId {
        NodeId::new(self.nodes.id, self.nodes.push(node))
    }

    pub fn node(&self, id: NodeId) -> Result<&Node<N>, Error> {
        self.nodes.get(id.arena(), id.slot())
    }
}
