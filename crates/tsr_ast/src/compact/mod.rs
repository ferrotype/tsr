//! Physical syntax storage. Public identities and semantic field values stay full-width.

pub(crate) mod binding;
pub(crate) mod lists;
mod pages;
mod text;

pub(crate) use pages::RowPages;

use crate::{AstStorageData, JsString, NodeId, NodeKind, NodeListId, NodeSlice, TextSlice};
use hashbrown::HashMap;
use tsr_arena::{ArenaId, AuxId};
use tsr_jsstring::SourceText;

/// Independent kind/shape, byte positions and owner-local links in 24 bytes.
#[derive(Debug)]
#[repr(C)]
pub struct StoredNode {
    pub(crate) kind: NodeKind,
    pub(crate) shape: u16,
    pub(crate) flags: u32,
    pub(crate) pos: i32,
    pub(crate) end: i32,
    pub(crate) parent: u32,
    pub(crate) ordinal: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FieldKey {
    shape: u16,
    field: u16,
    row: u32,
}
impl FieldKey {
    pub(crate) const fn new(shape: u16, row: u32, field: u16) -> Self {
        Self { shape, field, row }
    }
    pub(crate) const fn parent(slot: u32) -> Self {
        Self::new(u16::MAX, slot, 0)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CompactSlice {
    pub(crate) backing: u32,
    pub(crate) start: u32,
    pub(crate) len: u32,
}

#[derive(Clone, Copy)]
enum FullReference {
    Node(NodeId),
    Auxiliary(AuxId),
    Symbol(crate::SymbolId),
    SymbolTable(crate::SymbolTableId),
    Flow(crate::FlowId),
}

#[derive(Default)]
pub struct CoreStore {
    pub(crate) payloads: crate::AstPayloadStore,
    pub(crate) auxiliary: crate::auxiliary::AuxStore,
    links: HashMap<FieldKey, FullReference, tsr_arena::hash::FastState>,
    text: text::TextPool,
    pub(crate) edges: lists::EdgePages,
    #[allow(clippy::box_collection)]
    // The cold shape-change directory costs one word in other owners.
    parked_facts: Option<Box<HashMap<u32, u32, tsr_arena::hash::FastState>>>,
    runtime_ids:
        std::sync::OnceLock<std::sync::Mutex<HashMap<u32, u64, tsr_arena::hash::FastState>>>,
    binding_arenas: Option<binding::BindingArenas>,
    binding_overrides: HashMap<u32, crate::NodeBinding, tsr_arena::hash::FastState>,
}

#[derive(Clone, Copy)]
pub(crate) struct CompactContext<'a> {
    pub(crate) nodes: ArenaId,
    pub(crate) auxiliary: ArenaId,
    pub(crate) source: &'a SourceText,
    pub(crate) store: &'a CoreStore,
}

pub(crate) struct PackingContext<'a> {
    pub(crate) nodes: ArenaId,
    pub(crate) auxiliary: ArenaId,
    pub(crate) source: &'a SourceText,
    pub(crate) end: i32,
    links: &'a mut HashMap<FieldKey, FullReference, tsr_arena::hash::FastState>,
    text: &'a mut text::TextPool,
    binding_arenas: Option<binding::BindingArenas>,
}

impl CoreStore {
    pub(crate) fn local_binding_eligible(&self) -> bool {
        self.links.is_empty()
            && self.binding_overrides.is_empty()
            && !self.edges.has_escapes()
            && self.auxiliary.local_lists_only()
    }

    #[inline]
    pub(crate) fn local_text<'a>(
        &'a self,
        key: FieldKey,
        word: u32,
        end: i32,
        source: &'a SourceText,
    ) -> &'a [u8] {
        self.text.bytes(key, word, end, source)
    }

    #[inline]
    pub(crate) fn local_text_owned(
        &self,
        key: FieldKey,
        word: u32,
        end: i32,
        source: &SourceText,
    ) -> JsString {
        self.text.owned(key, word, end, source)
    }

    pub(crate) fn has_link_escapes(&self) -> bool {
        !self.links.is_empty()
    }

    pub(crate) fn packing_parts<'a>(
        &'a mut self,
        nodes: ArenaId,
        auxiliary: ArenaId,
        source: &'a SourceText,
        end: i32,
    ) -> (&'a mut crate::AstPayloadStore, PackingContext<'a>) {
        (
            &mut self.payloads,
            PackingContext {
                nodes,
                auxiliary,
                source,
                end,
                links: &mut self.links,
                text: &mut self.text,
                binding_arenas: self.binding_arenas,
            },
        )
    }
}

impl CompactContext<'_> {
    pub(crate) fn decode_node(self, key: FieldKey, word: u32) -> Option<NodeId> {
        match word {
            0 => None,
            u32::MAX => match self
                .store
                .links
                .get(&key)
                .expect("published compact reference escape")
            {
                FullReference::Node(id) => Some(*id),
                _ => unreachable!("node field has a node escape"),
            },
            slot => Some(NodeId::from_parts(self.nodes, slot).expect("stored nonzero node slot")),
        }
    }
    pub(crate) fn decode_list(self, key: FieldKey, word: u32) -> Option<NodeListId> {
        self.decode_aux(key, word).map(NodeListId)
    }
    fn decode_aux(self, key: FieldKey, word: u32) -> Option<AuxId> {
        match word {
            0 => None,
            u32::MAX => match self
                .store
                .links
                .get(&key)
                .expect("published compact reference escape")
            {
                FullReference::Auxiliary(id) => Some(*id),
                _ => unreachable!("auxiliary field has an auxiliary escape"),
            },
            slot => Some(
                AuxId::from_parts(self.auxiliary, slot).expect("stored nonzero auxiliary slot"),
            ),
        }
    }
    pub(crate) fn decode_node_slice(self, key: FieldKey, value: CompactSlice) -> NodeSlice {
        NodeSlice {
            backing: self.decode_aux(key, value.backing),
            start: value.start,
            len: value.len,
        }
    }
    pub(crate) fn decode_text_slice(self, key: FieldKey, value: CompactSlice) -> TextSlice {
        TextSlice {
            backing: self.decode_aux(key, value.backing),
            start: value.start,
            len: value.len,
        }
    }
}

impl<'a> CompactContext<'a> {
    pub(crate) fn text(self, key: FieldKey, word: u32, end: i32) -> &'a [u8] {
        self.store.text.bytes(key, word, end, self.source)
    }
    pub(crate) fn text_owned(self, key: FieldKey, word: u32, end: i32) -> JsString {
        self.store.text.owned(key, word, end, self.source)
    }
}

impl PackingContext<'_> {
    fn remove_link(&mut self, key: FieldKey) {
        // HashMap::remove hashes even an empty map. Ordinary core references
        // need no escape cleanup until an exceptional reference was stored.
        if !self.links.is_empty() {
            self.links.remove(&key);
        }
    }
    pub(crate) fn encode_node(&mut self, key: FieldKey, value: Option<NodeId>) -> u32 {
        self.remove_link(key);
        match value {
            None => 0,
            Some(id) if id.arena() == self.nodes && id.slot() != u32::MAX => id.slot(),
            Some(id) => {
                self.links.insert(key, FullReference::Node(id));
                u32::MAX
            }
        }
    }
    pub(crate) fn encode_list(&mut self, key: FieldKey, value: Option<NodeListId>) -> u32 {
        self.encode_aux(key, value.map(|id| id.0))
    }
    fn encode_aux(&mut self, key: FieldKey, value: Option<AuxId>) -> u32 {
        self.remove_link(key);
        match value {
            None => 0,
            Some(id) if id.arena() == self.auxiliary && id.slot() != u32::MAX => id.slot(),
            Some(id) => {
                self.links.insert(key, FullReference::Auxiliary(id));
                u32::MAX
            }
        }
    }
    pub(crate) fn encode_node_slice(&mut self, key: FieldKey, value: NodeSlice) -> CompactSlice {
        CompactSlice {
            backing: self.encode_aux(key, value.backing),
            start: value.start,
            len: value.len,
        }
    }
    pub(crate) fn encode_text_slice(&mut self, key: FieldKey, value: TextSlice) -> CompactSlice {
        CompactSlice {
            backing: self.encode_aux(key, value.backing),
            start: value.start,
            len: value.len,
        }
    }
    pub(crate) fn encode_text(&mut self, key: FieldKey, value: JsString) -> u32 {
        self.text.insert(key, value, self.end, self.source)
    }
    pub(crate) fn release_text(&mut self, key: FieldKey, word: u32) {
        self.text.release(key, word);
    }
    pub(crate) fn change_text_end(
        &mut self,
        key: FieldKey,
        word: u32,
        old_end: i32,
        new_end: i32,
    ) -> u32 {
        self.text
            .change_end(key, word, old_end, new_end, self.source)
    }
}

impl StoredNode {
    pub(crate) fn actual_shape(&self) -> u16 {
        self.shape & 0x7fff
    }
    pub(crate) fn fallback(node: &crate::Node, slot: u32) -> Self {
        Self {
            kind: node.kind(),
            shape: 0x7fff,
            flags: node.flags(),
            pos: node.pos(),
            end: node.end(),
            parent: 0,
            ordinal: slot,
        }
    }
    pub(crate) fn set_flags(&mut self, flags: u32) {
        self.flags = flags;
    }
}
impl tsr_arena::NodeRecord for StoredNode {
    type Aux = AstStorageData;
    type CoreAux = crate::StoredAux;
    type Store = CoreStore;
    fn storage_kind(&self) -> u32 {
        u32::from(self.kind.raw() as u16)
    }
    fn storage_reparsed(&self) -> bool {
        self.flags & crate::node_flags::REPARSED != 0
    }
}
impl CoreStore {
    pub(crate) fn parked_facts(&self, slot: u32) -> Option<u32> {
        self.parked_facts
            .as_deref()
            .and_then(|facts| facts.get(&slot).copied())
    }
    /// Only unrestricted payload replacement can temporarily move an existing
    /// composite cache into a shape without a facts word. Ordinary reads never
    /// consult this cold compatibility store.
    pub(crate) fn park_facts(&mut self, slot: u32, facts: u32, composite: bool) {
        if facts != 0 && !composite {
            self.parked_facts
                .get_or_insert_with(Default::default)
                .insert(slot, facts);
        } else if let Some(parked) = &mut self.parked_facts {
            parked.remove(&slot);
        }
    }
    pub(crate) fn preserve_runtime_id(&mut self, slot: u32, id: u64) {
        if id != 0 {
            self.runtime_ids
                .get_or_init(Default::default)
                .lock()
                .expect("exclusive runtime identity map")
                .insert(slot, id);
        } else if let Some(ids) = self.runtime_ids.get_mut() {
            ids.get_mut()
                .expect("exclusive runtime identity map")
                .remove(&slot);
        }
    }
    pub(crate) fn existing_runtime_id(&self, slot: u32) -> u64 {
        self.runtime_ids
            .get()
            .and_then(|ids| {
                ids.lock()
                    .expect("runtime identity lock")
                    .get(&slot)
                    .copied()
            })
            .unwrap_or(0)
    }
    pub(crate) fn runtime_id(&self, slot: u32) -> u64 {
        let mut ids = self
            .runtime_ids
            .get_or_init(Default::default)
            .lock()
            .expect("runtime identity lock");
        let id = ids.entry(slot).or_default();
        if *id == 0 {
            *id = crate::runtime_id::allocate_runtime_node_id();
        }
        *id
    }
}

#[cfg(test)]
mod tests;

impl CoreStore {
    /// Known structural bytes of every store the record arena owns, and the
    /// count of entries whose allocation extent std does not expose (edge
    /// escape entries, nested source metadata).
    pub fn structural_bytes(&self) -> (usize, usize) {
        self.structural_bytes_with(&mut tsr_arena::StorageCensus::default())
    }
    pub fn structural_bytes_with(&self, census: &mut tsr_arena::StorageCensus) -> (usize, usize) {
        let (auxiliary, auxiliary_unmeasured) = self.auxiliary.structural_bytes_with(census);
        let (edges, escapes) = self.edges.storage_bytes();
        let mut known = self.payloads.structural_bytes()
            + auxiliary
            + self.links.allocation_size()
            + self.text.structural_bytes_with(census)
            + edges
            + self.binding_overrides.allocation_size();
        if let Some(parked) = &self.parked_facts {
            known += size_of::<HashMap<u32, u32, tsr_arena::hash::FastState>>()
                + parked.allocation_size();
        }
        if let Some(ids) = self.runtime_ids.get() {
            known += ids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .allocation_size();
        }
        (known, auxiliary_unmeasured + escapes)
    }
}
