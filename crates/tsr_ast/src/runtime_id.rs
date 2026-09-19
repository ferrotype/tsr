//! Go's lazily assigned comparison identity is distinct from the owner-qualified
//! NodeId used for checked storage access. It carries no retention capability.
use crate::{Node, NodeAccess};

/// Observe a previously assigned source identity without assigning one.
pub(crate) fn owned_existing_runtime_node_id(node: &Node) -> u64 {
    node.runtime_id.load(std::sync::atomic::Ordering::SeqCst)
}
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_NODE_ID: AtomicU64 = AtomicU64::new(0);

/// port: tsc/internal/ast/utilities.go:GetNodeId
pub(crate) fn owned_runtime_node_id(node: &Node) -> u64 {
    let mut id = node.runtime_id.load(Ordering::SeqCst);
    if id == 0 {
        // This numeric source identity follows Go's atomic uint64 arithmetic.
        // Arena/generation capabilities use their separate checked allocator.
        id = allocate_runtime_node_id();
        if node
            .runtime_id
            .compare_exchange(0, id, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            id = node.runtime_id.load(Ordering::SeqCst);
        }
    }
    id
}

/// Observe a previously assigned logical identity without assigning one.
pub fn existing_runtime_node_id(node: &(impl NodeAccess + ?Sized)) -> u64 {
    node.existing_runtime_id()
}

/// port: tsc/internal/ast/utilities.go:GetNodeId
pub fn runtime_node_id(node: &(impl NodeAccess + ?Sized)) -> u64 {
    node.runtime_id()
}

pub(crate) fn allocate_runtime_node_id() -> u64 {
    NEXT_NODE_ID.fetch_add(1, Ordering::SeqCst).wrapping_add(1)
}
