//! Compact flow records retain full identities at their semantic boundary.

use super::{FlowData, FlowFlags, FlowId, FlowList, FlowListId, FlowNode};
use crate::NodeId;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use ts_arena::{ArenaId, Counters, Error, OwnedArena};

const NONE: u32 = 0;
const AST: u32 = 1;
const SWITCH_CLAUSE: u32 = 2;
const REDUCE_LABEL: u32 = 3;
const ESCAPED: u32 = u32::MAX;

#[repr(C)]
#[derive(Debug)]
struct StoredFlowNode {
    flags: FlowFlags,
    tag: u32,
    node: u32,
    antecedent: u32,
    antecedents: u32,
}

#[repr(C)]
#[derive(Debug)]
struct StoredFlowList {
    flow: u32,
    next: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FieldKey {
    slot: u32,
    field: u32,
}
impl FieldKey {
    fn new(slot: u32, field: u32) -> Self {
        Self { slot, field }
    }
}

#[derive(Clone, Copy, Debug)]
enum FullReference {
    Ast(NodeId),
    Flow(FlowId),
    List(FlowListId),
}

/// A binding result sets its namespaces before construction. Standalone stores
/// infer each external namespace once; every other owner remains an escape.
#[derive(Debug, Default)]
struct References {
    ast: Option<ArenaId>,
    flow: Option<ArenaId>,
    list: Option<ArenaId>,
    escaped: HashMap<FieldKey, FullReference>,
}
fn initialize_arena(current: &mut Option<ArenaId>, arena: ArenaId) {
    assert!(
        current.is_none_or(|existing| existing == arena),
        "flow reference namespace is immutable"
    );
    *current = Some(arena);
}

macro_rules! reference_codec {
    ($encode:ident, $decode:ident, $arena:ident, $id:ty, $variant:ident) => {
        impl References {
            fn $encode(&mut self, key: FieldKey, value: Option<$id>) -> u32 {
                if !self.escaped.is_empty() {
                    self.escaped.remove(&key);
                }
                let Some(id) = value else {
                    return 0;
                };
                let arena = *self.$arena.get_or_insert(id.arena());
                if id.arena() == arena && id.slot() != ESCAPED {
                    id.slot()
                } else {
                    self.escaped.insert(key, FullReference::$variant(id));
                    ESCAPED
                }
            }
            fn $decode(&self, key: FieldKey, word: u32) -> Option<$id> {
                match word {
                    0 => None,
                    ESCAPED => match self.escaped.get(&key).expect("flow reference escape") {
                        FullReference::$variant(id) => Some(*id),
                        _ => unreachable!("flow reference escape retains its identity type"),
                    },
                    slot => Some(
                        <$id>::from_parts(self.$arena.expect("flow reference namespace"), slot)
                            .expect("stored flow reference has a nonzero slot"),
                    ),
                }
            }
        }
    };
}
reference_codec!(encode_ast, decode_ast, ast, NodeId, Ast);
reference_codec!(encode_flow, decode_flow, flow, FlowId, Flow);
reference_codec!(encode_list, decode_list, list, FlowListId, List);

/// Ordinary records occupy twenty bytes. Only synthetic payloads and exceptional
/// full identities allocate cold side storage; neither adds an ordinary read probe.
#[derive(Debug)]
pub struct FlowNodes {
    records: OwnedArena<StoredFlowNode>,
    references: References,
    outlined: HashMap<u32, FlowData>,
}
impl FlowNodes {
    pub fn new(counters: &Counters) -> Self {
        let records = OwnedArena::new(counters);
        let references = References {
            flow: Some(records.id()),
            ..References::default()
        };
        Self {
            records,
            references,
            outlined: HashMap::new(),
        }
    }
    pub(crate) fn initialize_reference_arenas(&mut self, ast: ArenaId, lists: ArenaId) {
        initialize_arena(&mut self.references.ast, ast);
        initialize_arena(&mut self.references.list, lists);
    }
    pub fn id(&self) -> ArenaId {
        self.records.id()
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn push(&mut self, value: FlowNode) -> FlowId {
        let slot = u32::try_from(self.len())
            .ok()
            .and_then(|len| len.checked_add(1))
            .expect("ts_arena: arena slot space exhausted before reuse");
        let record = pack_node(slot, value, &mut self.references, &mut self.outlined);
        FlowId(self.records.push(record))
    }
    pub fn get(&self, id: FlowId) -> Result<FlowNodeRead<'_>, Error> {
        let record = self.records.get(id.0)?;
        Ok(FlowNodeRead {
            record,
            owner: self,
            slot: id.slot(),
        })
    }
    #[inline]
    pub(crate) fn get_slot(&self, slot: u32) -> Result<FlowNodeRead<'_>, Error> {
        Ok(FlowNodeRead {
            record: self.records.get_slot(slot)?,
            owner: self,
            slot,
        })
    }
    /// Compatibility for unrestricted edits. Production binding uses narrow setters.
    pub fn get_mut(&mut self, id: FlowId) -> Result<FlowNodeMut<'_>, Error> {
        let value = self.get(id)?.to_owned();
        Ok(FlowNodeMut {
            owner: self,
            id,
            value,
        })
    }
    pub fn iter(&self) -> impl Iterator<Item = (FlowId, FlowNodeRead<'_>)> {
        self.records.iter().map(|(id, record)| {
            (
                FlowId(id),
                FlowNodeRead {
                    record,
                    owner: self,
                    slot: id.slot(),
                },
            )
        })
    }
    pub fn set_flags(&mut self, id: FlowId, flags: FlowFlags) -> Result<(), Error> {
        self.records.get_mut(id.0)?.flags = flags;
        Ok(())
    }
    /// A flags-only borrow cannot alter the payload tag or graph identities.
    pub fn flags_mut(&mut self, id: FlowId) -> Result<&mut FlowFlags, Error> {
        Ok(&mut self.records.get_mut(id.0)?.flags)
    }
    #[inline]
    pub(crate) fn flags_slot_mut(&mut self, slot: u32) -> Result<&mut FlowFlags, Error> {
        Ok(&mut self.records.get_slot_mut(slot)?.flags)
    }
    pub fn set_node(&mut self, id: FlowId, node: Option<FlowData>) -> Result<(), Error> {
        if id.arena() != self.records.id() {
            return Err(Error::WrongOwner);
        }
        self.set_node_slot(id.slot(), node)
    }
    #[inline]
    pub(crate) fn set_node_slot(&mut self, slot: u32, node: Option<FlowData>) -> Result<(), Error> {
        let record = self.records.get_slot_mut(slot)?;
        let (tag, word) = pack_data(slot, node, &mut self.references, &mut self.outlined);
        record.tag = tag;
        record.node = word;
        Ok(())
    }
    pub fn set_antecedent(&mut self, id: FlowId, value: Option<FlowId>) -> Result<(), Error> {
        if id.arena() != self.records.id() {
            return Err(Error::WrongOwner);
        }
        self.set_antecedent_slot(id.slot(), value)
    }
    #[inline]
    pub(crate) fn set_antecedent_slot(
        &mut self,
        slot: u32,
        value: Option<FlowId>,
    ) -> Result<(), Error> {
        let record = self.records.get_slot_mut(slot)?;
        record.antecedent = self.references.encode_flow(FieldKey::new(slot, 1), value);
        Ok(())
    }
    pub fn set_antecedents(&mut self, id: FlowId, value: Option<FlowListId>) -> Result<(), Error> {
        if id.arena() != self.records.id() {
            return Err(Error::WrongOwner);
        }
        self.set_antecedents_slot(id.slot(), value)
    }
    #[inline]
    pub(crate) fn set_antecedents_slot(
        &mut self,
        slot: u32,
        value: Option<FlowListId>,
    ) -> Result<(), Error> {
        let record = self.records.get_slot_mut(slot)?;
        record.antecedents = self.references.encode_list(FieldKey::new(slot, 2), value);
        Ok(())
    }
}

fn pack_data(
    slot: u32,
    data: Option<FlowData>,
    references: &mut References,
    outlined: &mut HashMap<u32, FlowData>,
) -> (u32, u32) {
    if !outlined.is_empty() {
        outlined.remove(&slot);
    }
    match data {
        None => (NONE, references.encode_ast(FieldKey::new(slot, 0), None)),
        Some(FlowData::Ast(node)) => (
            AST,
            references.encode_ast(FieldKey::new(slot, 0), Some(node)),
        ),
        Some(data @ (FlowData::SwitchClause(_) | FlowData::ReduceLabel(_))) => {
            references.encode_ast(FieldKey::new(slot, 0), None);
            let tag = match data {
                FlowData::SwitchClause(_) => SWITCH_CLAUSE,
                _ => REDUCE_LABEL,
            };
            outlined.insert(slot, data);
            (tag, 0)
        }
    }
}

fn pack_node(
    slot: u32,
    value: FlowNode,
    references: &mut References,
    outlined: &mut HashMap<u32, FlowData>,
) -> StoredFlowNode {
    let (tag, node) = pack_data(slot, value.node, references, outlined);
    StoredFlowNode {
        flags: value.flags,
        tag,
        node,
        antecedent: references.encode_flow(FieldKey::new(slot, 1), value.antecedent),
        antecedents: references.encode_list(FieldKey::new(slot, 2), value.antecedents),
    }
}

#[derive(Clone, Copy)]
pub struct FlowNodeRead<'a> {
    record: &'a StoredFlowNode,
    owner: &'a FlowNodes,
    slot: u32,
}
impl FlowNodeRead<'_> {
    pub fn flags(self) -> FlowFlags {
        self.record.flags
    }
    pub fn node(self) -> Option<FlowData> {
        match self.record.tag {
            NONE => None,
            AST => Some(FlowData::Ast(
                self.owner
                    .references
                    .decode_ast(FieldKey::new(self.slot, 0), self.record.node)
                    .expect("AST flow payload has a node"),
            )),
            SWITCH_CLAUSE | REDUCE_LABEL => {
                let data = *self
                    .owner
                    .outlined
                    .get(&self.slot)
                    .expect("synthetic flow payload");
                assert!(
                    matches!(
                        (self.record.tag, data),
                        (SWITCH_CLAUSE, FlowData::SwitchClause(_))
                            | (REDUCE_LABEL, FlowData::ReduceLabel(_))
                    ),
                    "synthetic flow tag matches its payload"
                );
                Some(data)
            }
            _ => unreachable!("stored flow payload tag"),
        }
    }
    pub fn antecedent(self) -> Option<FlowId> {
        self.owner
            .references
            .decode_flow(FieldKey::new(self.slot, 1), self.record.antecedent)
    }
    pub fn antecedents(self) -> Option<FlowListId> {
        self.owner
            .references
            .decode_list(FieldKey::new(self.slot, 2), self.record.antecedents)
    }
    pub fn to_owned(self) -> FlowNode {
        FlowNode {
            flags: self.flags(),
            node: self.node(),
            antecedent: self.antecedent(),
            antecedents: self.antecedents(),
        }
    }
}
impl std::fmt::Debug for FlowNodeRead<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (*self).to_owned().fmt(formatter)
    }
}

pub struct FlowNodeMut<'a> {
    owner: &'a mut FlowNodes,
    id: FlowId,
    value: FlowNode,
}
impl Deref for FlowNodeMut<'_> {
    type Target = FlowNode;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}
impl DerefMut for FlowNodeMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}
impl Drop for FlowNodeMut<'_> {
    fn drop(&mut self) {
        let record = pack_node(
            self.id.slot(),
            self.value,
            &mut self.owner.references,
            &mut self.owner.outlined,
        );
        *self
            .owner
            .records
            .get_mut(self.id.0)
            .expect("mutable flow identity remains live") = record;
    }
}

#[derive(Debug)]
pub struct FlowLists {
    records: OwnedArena<StoredFlowList>,
    references: References,
}
impl FlowLists {
    pub fn new(counters: &Counters) -> Self {
        let records = OwnedArena::new(counters);
        let references = References {
            list: Some(records.id()),
            ..References::default()
        };
        Self {
            records,
            references,
        }
    }
    pub(crate) fn initialize_flow_arena(&mut self, flows: ArenaId) {
        initialize_arena(&mut self.references.flow, flows);
    }
    pub fn id(&self) -> ArenaId {
        self.records.id()
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
    pub fn push(&mut self, value: FlowList) -> FlowListId {
        let slot = u32::try_from(self.len())
            .ok()
            .and_then(|len| len.checked_add(1))
            .expect("ts_arena: arena slot space exhausted before reuse");
        let record = pack_list(slot, value, &mut self.references);
        FlowListId(self.records.push(record))
    }
    pub fn get(&self, id: FlowListId) -> Result<FlowListRead<'_>, Error> {
        let record = self.records.get(id.0)?;
        Ok(FlowListRead {
            record,
            owner: self,
            slot: id.slot(),
        })
    }
    #[inline]
    pub(crate) fn get_slot(&self, slot: u32) -> Result<FlowListRead<'_>, Error> {
        Ok(FlowListRead {
            record: self.records.get_slot(slot)?,
            owner: self,
            slot,
        })
    }
    pub fn get_mut(&mut self, id: FlowListId) -> Result<FlowListMut<'_>, Error> {
        let value = self.get(id)?.to_owned();
        Ok(FlowListMut {
            owner: self,
            id,
            value,
        })
    }
    pub fn iter(&self) -> impl Iterator<Item = (FlowListId, FlowListRead<'_>)> {
        self.records.iter().map(|(id, record)| {
            (
                FlowListId(id),
                FlowListRead {
                    record,
                    owner: self,
                    slot: id.slot(),
                },
            )
        })
    }
    pub fn set_flow(&mut self, id: FlowListId, value: Option<FlowId>) -> Result<(), Error> {
        if id.arena() != self.records.id() {
            return Err(Error::WrongOwner);
        }
        self.set_flow_slot(id.slot(), value)
    }
    #[inline]
    pub(crate) fn set_flow_slot(&mut self, slot: u32, value: Option<FlowId>) -> Result<(), Error> {
        let record = self.records.get_slot_mut(slot)?;
        record.flow = self.references.encode_flow(FieldKey::new(slot, 0), value);
        Ok(())
    }
    pub fn set_next(&mut self, id: FlowListId, value: Option<FlowListId>) -> Result<(), Error> {
        if id.arena() != self.records.id() {
            return Err(Error::WrongOwner);
        }
        self.set_next_slot(id.slot(), value)
    }
    #[inline]
    pub(crate) fn set_next_slot(
        &mut self,
        slot: u32,
        value: Option<FlowListId>,
    ) -> Result<(), Error> {
        let record = self.records.get_slot_mut(slot)?;
        record.next = self.references.encode_list(FieldKey::new(slot, 1), value);
        Ok(())
    }
}
fn pack_list(slot: u32, value: FlowList, references: &mut References) -> StoredFlowList {
    StoredFlowList {
        flow: references.encode_flow(FieldKey::new(slot, 0), value.flow),
        next: references.encode_list(FieldKey::new(slot, 1), value.next),
    }
}

#[derive(Clone, Copy)]
pub struct FlowListRead<'a> {
    record: &'a StoredFlowList,
    owner: &'a FlowLists,
    slot: u32,
}
impl FlowListRead<'_> {
    pub fn flow(self) -> Option<FlowId> {
        self.owner
            .references
            .decode_flow(FieldKey::new(self.slot, 0), self.record.flow)
    }
    pub fn next(self) -> Option<FlowListId> {
        self.owner
            .references
            .decode_list(FieldKey::new(self.slot, 1), self.record.next)
    }
    pub fn to_owned(self) -> FlowList {
        FlowList {
            flow: self.flow(),
            next: self.next(),
        }
    }
}
impl std::fmt::Debug for FlowListRead<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (*self).to_owned().fmt(formatter)
    }
}
pub struct FlowListMut<'a> {
    owner: &'a mut FlowLists,
    id: FlowListId,
    value: FlowList,
}
impl Deref for FlowListMut<'_> {
    type Target = FlowList;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}
impl DerefMut for FlowListMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}
impl Drop for FlowListMut<'_> {
    fn drop(&mut self) {
        let record = pack_list(self.id.slot(), self.value, &mut self.owner.references);
        *self
            .owner
            .records
            .get_mut(self.id.0)
            .expect("mutable flow-list identity remains live") = record;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AstBuilder, FactoryMethods, FlowReduceLabelData, FlowSwitchClauseData, SyntaxKind,
    };
    use ts_jsstring::SourceText;

    #[test]
    fn physical_records_and_narrow_writes_keep_independent_flags_payloads_and_links() {
        assert_eq!(std::mem::size_of::<StoredFlowNode>(), 20);
        assert_eq!(std::mem::size_of::<StoredFlowList>(), 8);
        let counters = Counters::new();
        let mut ast = AstBuilder::new(SourceText::default(), &counters);
        let node = ast.new_token(SyntaxKind::Unknown.into());
        let mut flows = FlowNodes::new(&counters);
        let mut lists = FlowLists::new(&counters);
        flows.initialize_reference_arenas(ast.id().arena(), lists.id());
        lists.initialize_flow_arena(flows.id());
        let first = flows.push(FlowNode::new(u32::MAX));
        let list = lists.push(FlowList {
            flow: Some(first),
            next: None,
        });
        let second = flows.push(FlowNode {
            flags: 0,
            node: Some(FlowData::Ast(node)),
            antecedent: Some(first),
            antecedents: Some(list),
        });
        assert!(flows.references.escaped.is_empty());
        assert!(lists.references.escaped.is_empty());
        assert!(flows.outlined.is_empty());
        flows.set_flags(second, u32::MAX).unwrap();
        flows.set_antecedent(first, Some(second)).unwrap();
        flows.set_antecedents(first, Some(list)).unwrap();
        lists.set_next(list, Some(list)).unwrap();
        assert_eq!(
            flows.get(second).unwrap().to_owned(),
            FlowNode {
                flags: u32::MAX,
                node: Some(FlowData::Ast(node)),
                antecedent: Some(first),
                antecedents: Some(list)
            }
        );
        assert_eq!(flows.get(first).unwrap().antecedent(), Some(second));
        assert_eq!(
            lists.get(list).unwrap().to_owned(),
            FlowList {
                flow: Some(first),
                next: Some(list)
            }
        );
        flows.set_antecedent(first, None).unwrap();
        flows.set_antecedents(first, None).unwrap();
        lists.set_flow(list, None).unwrap();
        lists.set_next(list, None).unwrap();
        assert_eq!(
            flows.get(first).unwrap().to_owned(),
            FlowNode::new(u32::MAX)
        );
        assert_eq!(lists.get(list).unwrap().to_owned(), FlowList::default());
    }

    #[test]
    fn synthetic_replacement_and_cold_mutation_preserve_open_flags_without_leaks() {
        let counters = Counters::new();
        let mut nodes = FlowNodes::new(&counters);
        let id = nodes.push(FlowNode::new(0x8000_0000));
        let clause = FlowData::SwitchClause(FlowSwitchClauseData::new(None, i64::MIN, i64::MAX));
        let reduce = FlowData::ReduceLabel(FlowReduceLabelData::new(Some(id), None));
        for _ in 0..128 {
            nodes.set_node(id, Some(clause)).unwrap();
            assert_eq!(nodes.get(id).unwrap().node(), Some(clause));
            assert_eq!(nodes.get(id).unwrap().flags(), 0x8000_0000);
            nodes.get_mut(id).unwrap().node = Some(reduce);
            assert_eq!(nodes.get(id).unwrap().node(), Some(reduce));
            assert_eq!(nodes.outlined.len(), 1);
            nodes.set_node(id, None).unwrap();
            assert_eq!(
                nodes.get(id).unwrap().to_owned(),
                FlowNode::new(0x8000_0000)
            );
            assert!(nodes.outlined.is_empty());
        }
        assert_eq!(nodes.len(), 1);
        assert_eq!(
            nodes.iter().next().unwrap().1.to_owned(),
            FlowNode::new(0x8000_0000)
        );
    }

    #[test]
    fn full_slot_and_foreign_references_roundtrip_and_receiver_errors_precede_mutation() {
        let counters = Counters::new();
        let ast = AstBuilder::new(SourceText::default(), &counters);
        let foreign_ast = AstBuilder::new(SourceText::default(), &counters);
        let mut flows = FlowNodes::new(&counters);
        let mut lists = FlowLists::new(&counters);
        let foreign_flows = FlowNodes::new(&counters);
        let foreign_lists = FlowLists::new(&counters);
        flows.initialize_reference_arenas(ast.id().arena(), lists.id());
        lists.initialize_flow_arena(flows.id());
        let id = flows.push(FlowNode::default());
        let list = lists.push(FlowList::default());
        for (ast_arena, flow_arena, list_arena, slot) in [
            (ast.id().arena(), flows.id(), lists.id(), u32::MAX),
            (
                foreign_ast.id().arena(),
                foreign_flows.id(),
                foreign_lists.id(),
                1,
            ),
        ] {
            let node = NodeId::from_parts(ast_arena, slot).unwrap();
            let flow = FlowId::from_parts(flow_arena, slot).unwrap();
            let tail = FlowListId::from_parts(list_arena, slot).unwrap();
            let value = FlowNode {
                flags: u32::MAX,
                node: Some(FlowData::Ast(node)),
                antecedent: Some(flow),
                antecedents: Some(tail),
            };
            *flows.get_mut(id).unwrap() = value;
            *lists.get_mut(list).unwrap() = FlowList {
                flow: Some(flow),
                next: Some(tail),
            };
            assert_eq!(flows.get(id).unwrap().to_owned(), value);
            assert_eq!(
                lists.get(list).unwrap().to_owned(),
                FlowList {
                    flow: Some(flow),
                    next: Some(tail)
                }
            );
            flows.set_node(id, None).unwrap();
            flows.set_antecedent(id, None).unwrap();
            flows.set_antecedents(id, None).unwrap();
            lists.set_flow(list, None).unwrap();
            lists.set_next(list, None).unwrap();
            assert!(flows.references.escaped.is_empty());
            assert!(lists.references.escaped.is_empty());
        }
        let foreign = FlowId::from_parts(foreign_flows.id(), u32::MAX).unwrap();
        assert!(matches!(
            flows.set_node(
                foreign,
                Some(FlowData::Ast(
                    NodeId::from_parts(foreign_ast.id().arena(), 1).unwrap()
                ))
            ),
            Err(Error::WrongOwner)
        ));
        assert!(matches!(
            flows.get(FlowId::from_parts(flows.id(), u32::MAX).unwrap()),
            Err(Error::InvalidSlot)
        ));
        assert!(matches!(
            lists.set_next(
                FlowListId::from_parts(foreign_lists.id(), u32::MAX).unwrap(),
                Some(list)
            ),
            Err(Error::WrongOwner)
        ));
        assert!(flows.outlined.is_empty() && flows.references.escaped.is_empty());
        assert_eq!(flows.get(id).unwrap().to_owned(), FlowNode::new(u32::MAX));
    }
}
