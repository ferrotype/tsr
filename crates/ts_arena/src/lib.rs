//! Compiler storage: checked identities, immutable files and transactional lazy nodes.
//! Payloads come from the AST and binder; this crate supplies storage and retention.
#![forbid(unsafe_code)]

mod arena;
mod bundle;
mod census;
mod counters;
mod error;
mod file;
mod ids;
mod initialization;
mod lazy;
mod lease;
mod node;
mod node_slots;
mod owned;
mod refs;
mod scope;
mod scratch;

pub use bundle::{StorageBundle, StorageHandle};
pub use census::StorageCensus;
pub use counters::{Counters, Counts};
pub use error::Error;
pub mod growth;
pub mod hash;
pub use file::{CoreDataRead, CoreNodesMut, StorageBuilder, StorageOwner, StorageView};
pub use ids::{ArenaId, AuxId, FileId, NodeId, SymbolId};
pub use initialization::{InitializationDomain, InitializationGuard};
pub use lazy::{StorageTransaction, TokenKey};
pub use lease::{CheckerIdentity, CheckerLease, Generation};
pub use node::{Node, NodeParentRecord, NodeRecord};
pub use node_slots::NodeSlots;
pub use owned::{OwnedArena, SymbolArena};
pub use refs::{
    AuxiliaryRead, CachedNodes, RecordRef, RetainedRecord, RetainedStorageSymbol, StorageRead,
    StorageSymbolRef,
};
pub use scope::{CoreScopeMut, LocalNode, StorageLocalArena, StorageScope};
pub use scratch::ScratchOwner;

#[cfg(any(test, feature = "harness"))]
pub mod scenarios;

#[cfg(test)]
mod tests;

// Compatibility adapters for the S04 payload API. Runtime owners store their
// concrete record directly through StorageBuilder/StorageHandle.
pub type FileBuilder<N, S = ()> = StorageBuilder<Node<N>, S>;
pub type FileOwner<N, S = ()> = StorageOwner<Node<N>, S>;
pub type FileHandle<N, S = ()> = StorageHandle<Node<N>, S>;
pub type BundleOwner<N, S = ()> = StorageBundle<Node<N>, S>;
pub type NodeRef<'a, N, S = ()> = RecordRef<'a, Node<N>, S>;
pub type RetainedNode<N, S = ()> = RetainedRecord<Node<N>, S>;
pub type SymbolRef<'a, N, S> = StorageSymbolRef<'a, Node<N>, S>;
pub type RetainedSymbol<N, S> = RetainedStorageSymbol<Node<N>, S>;
pub type NodeListRef<N, S = ()> = CachedNodes<Node<N>, S>;
pub type LazyTransaction<'a, N> = StorageTransaction<'a, Node<N>>;
pub type Scope<N, S = ()> = StorageScope<Node<N>, S>;
pub type LocalArena<'brand, 'owner, N> = StorageLocalArena<'brand, 'owner, Node<N>>;
