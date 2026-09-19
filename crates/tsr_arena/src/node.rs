use crate::NodeId;

/// The storage operations that inspect syntax metadata use this single record
/// interface. Concrete AST records own their header; storage adds no wrapper.
pub trait NodeRecord {
    /// Core auxiliary records can use owner-relative storage independently of
    /// the full values staged under the lazy publication lock.
    type CoreAux;
    type Aux;
    /// Exclusive per-owner payload storage, published with its record headers.
    type Store: Default;
    fn storage_kind(&self) -> u32;
    fn storage_reparsed(&self) -> bool;
}

/// Full records whose parent can be assigned without owner-specific payload
/// context. Compact records initialize that link through a transaction instead.
pub trait NodeParentRecord: NodeRecord {
    fn set_storage_parent(&mut self, parent: Option<NodeId>);
}

/// Storage metadata around an AST-defined payload. Graph edges are non-owning ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node<T> {
    pub kind: u32,
    pub parent: Option<NodeId>,
    pub reparsed: bool,
    pub data: T,
}

impl<T> Node<T> {
    pub fn new(kind: u32, data: T) -> Self {
        Self {
            kind,
            parent: None,
            reparsed: false,
            data,
        }
    }
}

impl<T> NodeRecord for Node<T> {
    type CoreAux = ();
    type Aux = ();
    type Store = ();
    fn storage_kind(&self) -> u32 {
        self.kind
    }
    fn storage_reparsed(&self) -> bool {
        self.reparsed
    }
}

impl<T> NodeParentRecord for Node<T> {
    fn set_storage_parent(&mut self, parent: Option<NodeId>) {
        self.parent = parent;
    }
}
