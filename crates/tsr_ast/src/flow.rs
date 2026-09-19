//! Binder-owned flow graphs. Node, list and synthetic payload links are non-owning
//! identities; cycles never create an owning reference back to the bind result.

use crate::{NodeId, NodeKind, SyntaxKind};
use ts_arena::{ArenaId, AuxId, Error};
use ts_core::TextRange;

pub type FlowFlags = u32;
pub mod flow_flags {
    use super::FlowFlags;
    pub const UNREACHABLE: FlowFlags = 1 << 0;
    pub const START: FlowFlags = 1 << 1;
    pub const BRANCH_LABEL: FlowFlags = 1 << 2;
    pub const LOOP_LABEL: FlowFlags = 1 << 3;
    pub const ASSIGNMENT: FlowFlags = 1 << 4;
    pub const TRUE_CONDITION: FlowFlags = 1 << 5;
    pub const FALSE_CONDITION: FlowFlags = 1 << 6;
    pub const SWITCH_CLAUSE: FlowFlags = 1 << 7;
    pub const ARRAY_MUTATION: FlowFlags = 1 << 8;
    pub const CALL: FlowFlags = 1 << 9;
    pub const REDUCE_LABEL: FlowFlags = 1 << 10;
    pub const REFERENCED: FlowFlags = 1 << 11;
    pub const SHARED: FlowFlags = 1 << 12;
    pub const LABEL: FlowFlags = BRANCH_LABEL | LOOP_LABEL;
    pub const CONDITION: FlowFlags = TRUE_CONDITION | FALSE_CONDITION;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowNode {
    pub flags: FlowFlags,
    pub node: Option<FlowData>,
    pub antecedent: Option<FlowId>,
    pub antecedents: Option<FlowListId>,
}
pub type FlowLabel = FlowNode;
impl FlowNode {
    pub fn new(flags: FlowFlags) -> Self {
        Self {
            flags,
            ..Self::default()
        }
    }
    pub fn new_ex(flags: FlowFlags, node: Option<FlowData>, antecedent: Option<FlowId>) -> Self {
        Self {
            flags,
            node,
            antecedent,
            antecedents: None,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowList {
    pub flow: Option<FlowId>,
    pub next: Option<FlowListId>,
}

/// Go stores either an ordinary AST node or one of two synthetic payloads in
/// FlowNode.Node. Keep that discriminant independently of the open flow flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowData {
    Ast(NodeId),
    SwitchClause(FlowSwitchClauseData),
    ReduceLabel(FlowReduceLabelData),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlowSwitchClauseData {
    pub switch_statement: Option<NodeId>,
    pub clause_start: i32,
    pub clause_end: i32,
}
impl FlowSwitchClauseData {
    // port: tsc/internal/ast/flow.go:NewFlowSwitchClauseData
    pub fn new(switch_statement: Option<NodeId>, clause_start: i64, clause_end: i64) -> Self {
        Self {
            switch_statement,
            clause_start: clause_start as i32,
            clause_end: clause_end as i32,
        }
    }
    // port: tsc/internal/ast/flow.go:FlowSwitchClauseData.IsEmpty
    pub fn is_empty(&self) -> bool {
        self.clause_start == self.clause_end
    }
    pub fn kind(&self) -> NodeKind {
        SyntaxKind::Unknown.into()
    }
    pub fn range(&self) -> TextRange {
        TextRange::new(-1, -1)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlowReduceLabelData {
    pub target: Option<FlowId>,
    pub antecedents: Option<FlowListId>,
}
impl FlowReduceLabelData {
    // port: tsc/internal/ast/flow.go:NewFlowReduceLabelData
    pub fn new(target: Option<FlowId>, antecedents: Option<FlowListId>) -> Self {
        Self {
            target,
            antecedents,
        }
    }
    pub fn kind(&self) -> NodeKind {
        SyntaxKind::Unknown.into()
    }
    pub fn range(&self) -> TextRange {
        TextRange::new(-1, -1)
    }
}

macro_rules! flow_id {
    ($id:ident) => {
        /// A checked-owner input identity, never an ownership capability.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $id(AuxId);
        impl $id {
            pub fn bits(self) -> u64 {
                self.0.bits()
            }
            pub fn arena(self) -> ArenaId {
                self.0.arena()
            }
            pub fn slot(self) -> u32 {
                self.0.slot()
            }
            pub(crate) fn from_parts(arena: ArenaId, slot: u32) -> Result<Self, Error> {
                AuxId::from_parts(arena, slot).map(Self)
            }
        }
    };
}
flow_id!(FlowId);
flow_id!(FlowListId);

mod storage;
pub use storage::{FlowListMut, FlowListRead, FlowLists, FlowNodeMut, FlowNodeRead, FlowNodes};

#[cfg(test)]
mod tests {
    use super::*;
    use ts_arena::Counters;

    #[test]
    fn synthetic_payloads_keep_discriminants_nil_edges_and_go_int32_narrowing() {
        let clause = FlowSwitchClauseData::new(None, i64::from(i32::MAX) + 1, -2_147_483_648);
        assert!(clause.is_empty());
        assert_eq!(clause.clause_start, i32::MIN);
        assert_eq!(clause.kind(), SyntaxKind::Unknown);
        assert_eq!(clause.range(), TextRange::new(-1, -1));
        let reduce = FlowReduceLabelData::new(None, None);
        assert_eq!(reduce.kind(), SyntaxKind::Unknown);
        assert_eq!(reduce.range(), clause.range());
        let node = FlowNode::new_ex(flow_flags::START, Some(FlowData::ReduceLabel(reduce)), None);
        assert!(matches!(node.node, Some(FlowData::ReduceLabel(_))));
        assert!(node.antecedent.is_none() && node.antecedents.is_none());
    }

    #[test]
    fn owned_flow_cycles_share_links_without_retention_and_reject_foreign_ids() {
        let counters = Counters::new();
        let before = counters.snapshot();
        let stale = {
            let mut nodes = FlowNodes::new(&counters);
            let mut lists = FlowLists::new(&counters);
            let label = nodes.push(FlowNode::new(flow_flags::LOOP_LABEL));
            let list = lists.push(FlowList {
                flow: Some(label),
                next: None,
            });
            nodes.get_mut(label).unwrap().antecedents = Some(list);
            lists.get_mut(list).unwrap().next = Some(list);
            assert_eq!(lists.get(list).unwrap().flow(), Some(label));
            let other_nodes = FlowNodes::new(&counters);
            let other_lists = FlowLists::new(&counters);
            assert!(matches!(other_nodes.get(label), Err(Error::WrongOwner)));
            assert!(matches!(other_lists.get(list), Err(Error::WrongOwner)));
            label
        };
        assert_eq!(counters.snapshot(), before);
        let mut replacement = FlowNodes::new(&counters);
        let id = replacement.push(FlowNode::new(flow_flags::UNREACHABLE));
        assert_ne!(id.arena(), stale.arena());
        assert!(matches!(replacement.get(stale), Err(Error::WrongOwner)));
    }
}
