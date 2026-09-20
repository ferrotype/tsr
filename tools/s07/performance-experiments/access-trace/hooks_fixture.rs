//! Test fixture for exact original/transformed binder helper bodies.
//! These stubs expose lookup/read/drop/write order; they do not model AST soundness.
use std::{cell::RefCell, ops::Deref, panic::{catch_unwind, AssertUnwindSafe}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Id(u64);
impl Id {
    fn bits(self) -> u64 { self.0 }
}
type NodeId = Id;
type NodeListId = Id;
type AuxId = Id;
type FlowId = Id;
const NODE: Id = Id(0x1_0000_0001);
const OTHER: Id = Id(0x1_0000_0002);
const MISSING: Id = Id(0x2_0000_0001);
const FLOW: Id = Id(0x3_0000_0001);
const BACKING: Id = Id(0x4_0000_0001);
const LIST: Id = Id(0x4_0000_0002);
const EMPTY: Id = Id(0x4_0000_0003);
const FOREIGN_EMPTY: Id = Id(0x5_0000_0001);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Observation {
    Operation(&'static str, u64, u64),
    Event(u16, u16, [u64; 4]),
}
thread_local! {
    static LOG: RefCell<Vec<Observation>> = const { RefCell::new(Vec::new()) };
}
fn observe(name: &'static str, a: u64, b: u64) {
    LOG.with(|log| log.borrow_mut().push(Observation::Operation(name, a, b)));
}
mod tsr_ast {
    pub type NodeSliceRead<'a> = super::SliceRead<'a>;
    pub mod access_trace {
        pub fn event(op: u16, site: u16, a: u64, b: u64, c: u64, d: u64) {
            super::super::LOG.with(|log| log.borrow_mut().push(
                super::super::Observation::Event(op, site, [a, b, c, d])));
        }
    }
}
mod nf {
    pub const THIS_NODE_OR_ANY_SUB_NODES_HAS_ERROR: u32 = 0x80;
}

struct Node {
    id: Id,
    flags: u32,
    flow_capable: bool,
}
impl Node {
    fn flags(&self) -> u32 {
        observe("flags", self.id.bits(), u64::from(self.flags));
        self.flags
    }
}
struct ReadDrop(Id);
impl Drop for ReadDrop {
    fn drop(&mut self) { observe("drop_node_read", self.0.bits(), 0); }
}
// Like StorageRead, the handle itself has no custom Drop that uses its borrow;
// its optional owned retention can still have an observable drop order.
struct NodeRead<'a> {
    value: &'a Node,
    _retention: ReadDrop,
}
impl Deref for NodeRead<'_> {
    type Target = Node;
    fn deref(&self) -> &Node { self.value }
}
fn has_flow_node_data(node: &Node) -> bool {
    observe("flow_capable", node.id.bits(), 0);
    node.flow_capable
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NodeSlice {
    backing: Option<AuxId>,
    start: u32,
    len: u32,
}
impl NodeSlice {
    fn empty() -> Self { Self { backing: None, start: 0, len: 0 } }
    fn backing_id(self) -> Option<AuxId> { self.backing }
    fn start(self) -> u32 { self.start }
    fn len(self) -> usize { self.len as usize }
}
struct ListRead(NodeSlice);
impl ListRead {
    fn nodes(&self) -> NodeSlice {
        observe("list_nodes", 0, 0);
        self.0
    }
}
impl Drop for ListRead {
    fn drop(&mut self) { observe("drop_list_read", 0, 0); }
}
struct SliceRead<'a>(&'a [Option<NodeId>]);
impl Deref for SliceRead<'_> {
    type Target = [Option<NodeId>];
    fn deref(&self) -> &Self::Target { self.0 }
}
impl Drop for SliceRead<'_> {
    fn drop(&mut self) { observe("drop_slice_read", 0, 0); }
}
struct Parsed<'a>(&'a Builder);
impl<'a> Parsed<'a> {
    fn list(&self, list: NodeListId) -> Result<ListRead, ()> {
        observe("list", list.bits(), 0);
        let nodes = match list {
            LIST => NodeSlice { backing: Some(BACKING), start: 1, len: 2 },
            EMPTY => NodeSlice { backing: Some(BACKING), start: 4, len: 0 },
            FOREIGN_EMPTY => NodeSlice { backing: Some(FOREIGN_EMPTY), start: 0, len: 0 },
            _ => return Err(()),
        };
        Ok(ListRead(nodes))
    }
    fn node_slice(&self, nodes: NodeSlice) -> Result<SliceRead<'a>, ()> {
        observe("node_slice", nodes.backing.map_or(0, |id| id.bits()),
                (u64::from(nodes.start) << 32) | u64::from(nodes.len));
        match nodes.backing {
            None if nodes.start == 0 && nodes.len == 0 => Ok(SliceRead(&[])),
            Some(BACKING) => {
                let end = nodes.start.checked_add(nodes.len).ok_or(())?;
                self.0.edges.get(nodes.start as usize..end as usize).map(SliceRead).ok_or(())
            }
            _ => Err(()),
        }
    }
}

struct Builder {
    node: Node,
    other: Node,
    edges: [Option<NodeId>; 4],
    flow: Option<FlowId>,
    reject_flags: bool,
    reject_flow: bool,
}
impl Builder {
    fn new() -> Self {
        Self {
            node: Node { id: NODE, flags: 1, flow_capable: true },
            other: Node { id: OTHER, flags: 0, flow_capable: false },
            edges: [Some(OTHER), None, Some(NODE), None],
            flow: None, reject_flags: false, reject_flow: false,
        }
    }
    fn node(&self, id: NodeId) -> Result<NodeRead<'_>, ()> {
        observe("node", id.bits(), 0);
        match id {
            NODE => Ok(NodeRead { value: &self.node, _retention: ReadDrop(id) }),
            OTHER => Ok(NodeRead { value: &self.other, _retention: ReadDrop(id) }),
            _ => Err(()),
        }
    }
    fn set_node_flags(&mut self, id: NodeId, flags: u32) -> Result<(), ()> {
        observe("set_flags", id.bits(), u64::from(flags));
        if self.reject_flags || id != NODE { return Err(()); }
        self.node.flags = flags;
        Ok(())
    }
    fn set_node_flow(&mut self, id: NodeId, flow: Option<FlowId>) -> Result<(), ()> {
        observe("set_flow", id.bits(), flow.map_or(0, |id| id.bits()));
        if self.reject_flow || id != NODE { return Err(()); }
        self.flow = flow;
        Ok(())
    }
}

macro_rules! binder {
    ($module:ident, $($methods:tt)*) => {
        mod $module {
            use super::*;
            pub struct Binder<'a> {
                pub builder: &'a mut Builder,
                pub seen_parse_error: bool,
            }
            impl Binder<'_> {
                fn parsed_view(&self) -> Parsed<'_> {
                    observe("parsed_view", 0, 0);
                    Parsed(self.builder)
                }
                $($methods)*
            }
        }
    };
}
binder!(original,
// ORIGINAL_METHODS
);
binder!(traced,
// TRACED_METHODS
);

#[derive(Clone, Copy, Debug)]
enum Scenario {
    FlagsNoop, FlagsWrite, FlagsFail, FlowWrite, FlowClear, FlowUnsupported, FlowFail,
    MissingNode, SyntaxAbsent, SyntaxEmpty, SyntaxForeignEmpty, SyntaxSubrange,
    SyntaxNilElement, SyntaxSelectedElement, SyntaxOutOfBounds, PropagateError, NoError,
}
const SCENARIOS: [Scenario; 17] = [
    Scenario::FlagsNoop, Scenario::FlagsWrite, Scenario::FlagsFail,
    Scenario::FlowWrite, Scenario::FlowClear, Scenario::FlowUnsupported, Scenario::FlowFail,
    Scenario::MissingNode, Scenario::SyntaxAbsent, Scenario::SyntaxEmpty,
    Scenario::SyntaxForeignEmpty, Scenario::SyntaxSubrange, Scenario::SyntaxNilElement,
    Scenario::SyntaxSelectedElement, Scenario::SyntaxOutOfBounds,
    Scenario::PropagateError, Scenario::NoError,
];
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    operations: Vec<Observation>,
    events: Vec<Observation>,
    result: Vec<Option<NodeId>>,
    flags: u32,
    flow: Option<FlowId>,
    error: bool,
    panicked: bool,
}
macro_rules! run_scenario {
    ($name:ident, $module:ident) => {
        fn $name(scenario: Scenario) -> Outcome {
            LOG.with(|log| log.borrow_mut().clear());
            let mut builder = Builder::new();
            builder.reject_flags = matches!(scenario, Scenario::FlagsFail);
            builder.reject_flow = matches!(scenario, Scenario::FlowFail);
            if matches!(scenario, Scenario::FlowClear) { builder.flow = Some(FLOW); }
            let mut binder = $module::Binder { builder: &mut builder, seen_parse_error: false };
            let mut result = Vec::new();
            let panicked = catch_unwind(AssertUnwindSafe(|| match scenario {
                Scenario::FlagsNoop => binder.set_flags(NODE, 1),
                Scenario::FlagsWrite | Scenario::FlagsFail => binder.set_flags(NODE, u32::MAX),
                Scenario::FlowWrite | Scenario::FlowFail => binder.set_flow_node(NODE, Some(FLOW)),
                Scenario::FlowClear => binder.set_flow_node(NODE, None),
                Scenario::FlowUnsupported => binder.set_flow_node(OTHER, Some(FLOW)),
                Scenario::MissingNode => { let _ = binder.n(MISSING); }
                Scenario::SyntaxAbsent => {
                    assert_eq!(binder.syntax_slice(None), NodeSlice::empty());
                    result.extend_from_slice(&binder.syntax_nodes(None));
                }
                Scenario::SyntaxEmpty => {
                    let slice = binder.syntax_slice(Some(EMPTY));
                    assert_eq!(slice, NodeSlice { backing: Some(BACKING), start: 4, len: 0 });
                    result.extend_from_slice(&binder.syntax_nodes(Some(EMPTY)));
                }
                Scenario::SyntaxForeignEmpty => { binder.syntax_slice(Some(FOREIGN_EMPTY)); }
                Scenario::SyntaxSubrange => {
                    result.extend_from_slice(&binder.syntax_nodes(Some(LIST)));
                }
                Scenario::SyntaxNilElement | Scenario::SyntaxSelectedElement | Scenario::SyntaxOutOfBounds => {
                    let slice = binder.syntax_slice(Some(LIST));
                    let index = match scenario {
                        Scenario::SyntaxNilElement => 0,
                        Scenario::SyntaxSelectedElement => 1,
                        _ => 2,
                    };
                    result.push(binder.syntax_node(slice, index));
                }
                Scenario::PropagateError => binder.bind_node_error(NODE, true),
                Scenario::NoError => binder.bind_node_error(NODE, false),
            })).is_err();
            let error = binder.seen_parse_error;
            let log = LOG.with(|log| log.borrow().clone());
            let (events, operations) = log.into_iter().partition(
                |item| matches!(item, Observation::Event(..)));
            Outcome { operations, events, result, flags: builder.node.flags,
                      flow: builder.flow, error, panicked }
        }
    };
}
run_scenario!(run_original, original);
run_scenario!(run_traced, traced);

#[test]
fn original_evaluations_values_drops_mutations_and_failures_are_preserved() {
    for scenario in SCENARIOS {
        let expected = run_original(scenario);
        let mut observed = run_traced(scenario);
        observed.events.clear();
        assert_eq!(observed, expected, "scenario {scenario:?}");
    }
}

#[test]
fn success_records_preserve_nil_subranges_empty_identity_and_write_outcomes() {
    use Observation::Event as E;
    assert_eq!(run_traced(Scenario::FlagsNoop).events, [
        E(100, 100, [NODE.bits(), 0, 0, 0]), E(102, 105, [NODE.bits(), 1, 0, 0])]);
    assert_eq!(run_traced(Scenario::FlagsWrite).events, [
        E(100, 100, [NODE.bits(), 0, 0, 0]), E(102, 105, [NODE.bits(), 1, 0, 0]),
        E(103, 106, [NODE.bits(), u64::from(u32::MAX), 0, 0])]);
    assert_eq!(run_traced(Scenario::FlagsFail).events.len(), 2);
    assert_eq!(run_traced(Scenario::FlowWrite).events, [
        E(100, 100, [NODE.bits(), 0, 0, 0]), E(107, 110, [NODE.bits(), FLOW.bits(), 0, 0])]);
    assert_eq!(run_traced(Scenario::FlowClear).events[1], E(107, 110, [NODE.bits(), 0, 0, 0]));
    assert_eq!(run_traced(Scenario::FlowFail).events.len(), 1);
    assert_eq!(run_traced(Scenario::FlowUnsupported).events.len(), 1);
    assert!(run_traced(Scenario::MissingNode).events.is_empty());
    assert!(run_traced(Scenario::SyntaxForeignEmpty).events.is_empty());
    assert_eq!(run_traced(Scenario::SyntaxAbsent).events, [E(104, 107, [0; 4]), E(106, 109, [0; 4])]);
    assert_eq!(run_traced(Scenario::SyntaxEmpty).events, [
        E(104, 107, [EMPTY.bits(), BACKING.bits(), 4, 0]),
        E(106, 109, [EMPTY.bits(), BACKING.bits(), 4, 0])]);
    assert_eq!(run_traced(Scenario::SyntaxNilElement).events[1],
        E(105, 108, [BACKING.bits(), (1 << 32) | 2, 0, 0]));
    assert_eq!(run_traced(Scenario::SyntaxSelectedElement).events[1],
        E(105, 108, [BACKING.bits(), (1 << 32) | 2, 1, NODE.bits()]));
    assert_eq!(run_traced(Scenario::SyntaxOutOfBounds).events.len(), 1);
    assert!(run_traced(Scenario::NoError).events.is_empty());
    assert_eq!(run_traced(Scenario::PropagateError).events, [
        E(100, 100, [NODE.bits(), 0, 0, 0]), E(102, 104, [NODE.bits(), 1, 0, 0]),
        E(100, 100, [NODE.bits(), 0, 0, 0]), E(102, 105, [NODE.bits(), 1, 0, 0]),
        E(103, 106, [NODE.bits(), 0x81, 0, 0])]);
}
