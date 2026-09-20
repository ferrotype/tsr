//! Reuses actual tsr_ast payloads. The alternate enum is a size experiment only.
#![forbid(unsafe_code)]
#![allow(dead_code)]

use std::mem::{align_of, size_of};
use std::sync::atomic::{AtomicU32, AtomicU64};
use tsr_ast::*;

struct WithBinding<T> {
    syntax: T,
    binding: Option<FlowId>,
}

// Same fields/types/order as production Node; ordinary Rust representation.
struct NodeFrame<D> {
    kind: NodeKind,
    parent: Option<NodeId>,
    flags: u32,
    pos: i32,
    end: i32,
    data: D,
    subtree_facts: AtomicU32,
    runtime_id: AtomicU64,
}

// ENUMS

fn observe<T>(name: &str) {
    println!("{name} {} {}", size_of::<T>(), align_of::<T>());
}

fn main() {
    assert_eq!(size_of::<Option<FlowId>>(), 8);
    assert_eq!(size_of::<CopiedData>(), size_of::<NodeData>());
    assert_eq!(align_of::<CopiedData>(), align_of::<NodeData>());
    assert_eq!(size_of::<NodeFrame<CopiedData>>(), size_of::<Node>());
    assert_eq!(align_of::<NodeFrame<CopiedData>>(), align_of::<Node>());
    observe::<Node>("ActualNode");
    observe::<NodeData>("ActualNodeData");
    observe::<CopiedData>("CopiedData");
    observe::<NodeFrame<CopiedData>>("NodeWithCopiedData");
    observe::<WithBinding<IdentifierData>>("IdentifierPlusEight");
    observe::<IdentifierPlusEightData>("IdentifierPlusEightData");
    observe::<NodeFrame<IdentifierPlusEightData>>("NodeWithIdentifierPlusEight");
    observe::<Option<FlowId>>("OptionalFlowId");
    // PAYLOAD_OBSERVATIONS
}
