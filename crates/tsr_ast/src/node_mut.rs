//! Explicit unrestricted edits stage one construction value; narrow binder and
//! parser header writes bypass this guard and preserve their existing proofs.
use crate::compact::{CoreStore, FieldKey, StoredNode};
use crate::{Node, NodeData, NodeId};
use std::ops::{Deref, DerefMut};
use ts_arena::ArenaId;
use ts_jsstring::SourceText;

pub struct NodeMut<'a> {
    inner: Mutation<'a>,
}
enum Mutation<'a> {
    Core {
        value: Node,
        header: &'a mut StoredNode,
        store: &'a mut CoreStore,
        source: &'a SourceText,
        nodes: ArenaId,
        auxiliary: ArenaId,
        slot: u32,
    },
    Lazy {
        value: &'a mut Node,
        header: &'a mut StoredNode,
    },
    Owned(&'a mut Node),
}
impl<'a> NodeMut<'a> {
    pub(crate) fn core(
        value: Node,
        header: &'a mut StoredNode,
        store: &'a mut CoreStore,
        source: &'a SourceText,
        id: NodeId,
        auxiliary: ArenaId,
    ) -> Self {
        Self {
            inner: Mutation::Core {
                value,
                header,
                store,
                source,
                nodes: id.arena(),
                auxiliary,
                slot: id.slot(),
            },
        }
    }
    pub(crate) fn lazy(value: &'a mut Node, header: &'a mut StoredNode) -> Self {
        Self {
            inner: Mutation::Lazy { value, header },
        }
    }
    pub(crate) fn owned(value: &'a mut Node) -> Self {
        Self {
            inner: Mutation::Owned(value),
        }
    }
}
impl Deref for NodeMut<'_> {
    type Target = Node;
    fn deref(&self) -> &Node {
        match &self.inner {
            Mutation::Core { value, .. } => value,
            Mutation::Lazy { value, .. } | Mutation::Owned(value) => value,
        }
    }
}
impl DerefMut for NodeMut<'_> {
    fn deref_mut(&mut self) -> &mut Node {
        match &mut self.inner {
            Mutation::Core { value, .. } => value,
            Mutation::Lazy { value, .. } | Mutation::Owned(value) => value,
        }
    }
}
impl Drop for NodeMut<'_> {
    fn drop(&mut self) {
        match &mut self.inner {
            Mutation::Core {
                value,
                header,
                store,
                source,
                nodes,
                auxiliary,
                slot,
            } => {
                let old_shape = header.actual_shape();
                let data = std::mem::replace(&mut value.data, NodeData::Token(crate::TokenData {}));
                let shape = crate::AstPayloadStore::shape_of(&data);
                let composite = data.uses_subtree_cache();
                let binding = if shape == old_shape {
                    None
                } else {
                    store.node_binding(
                        header,
                        *slot,
                        crate::compact::CompactContext {
                            nodes: *nodes,
                            auxiliary: *auxiliary,
                            source,
                            store,
                        },
                    )
                };
                let (payloads, mut context) =
                    store.packing_parts(*nodes, *auxiliary, source, value.end);
                if shape == old_shape {
                    payloads.replace(shape, header.ordinal, data, &mut context);
                } else {
                    payloads.release_text(old_shape, header.ordinal, &mut context);
                    let (new_shape, ordinal) = payloads.insert(data, &mut context);
                    header.shape = new_shape | (header.shape & 0x8000);
                    header.ordinal = ordinal;
                }
                header.parent = context.encode_node(FieldKey::parent(*slot), value.parent);
                header.kind = value.kind;
                header.flags = value.flags;
                header.pos = value.pos;
                header.end = value.end;
                payloads.store_subtree_facts(shape, header.ordinal, value.cached_subtree_facts());
                if shape != old_shape {
                    let id = NodeId::from_parts(*nodes, *slot).expect("mutable core node identity");
                    store.restore_binding(header, id, *auxiliary, source, binding);
                }
                store.preserve_runtime_id(*slot, crate::existing_runtime_node_id(value));
                store.park_facts(*slot, value.cached_subtree_facts(), composite);
            }
            Mutation::Lazy { value, header } => {
                header.kind = value.kind;
                header.flags = value.flags;
                header.pos = value.pos;
                header.end = value.end;
            }
            Mutation::Owned(_) => {}
        }
    }
}
