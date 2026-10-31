//! One reachability operation over interchangeable immutable graph readers.
//! Semantic effects and completed-result caches stay in the actual checker.

use crate::{flow::FlowGraph, CheckerState, Error};
use tsr_ast::{flow_flags as ff, FlowData, FlowId, FlowListId, FlowReduceLabelData};

struct Node<F, L> {
    flags: u32,
    data: Option<FlowData>,
    antecedent: Option<F>,
    antecedents: Option<L>,
}
struct List<F, L> {
    flow: Option<F>,
    next: Option<L>,
}

trait Reader {
    type Flow: Copy;
    type List: Copy;
    fn flow_id(&self, flow: Self::Flow) -> FlowId;
    fn node(
        &self,
        state: &CheckerState,
        flow: Self::Flow,
    ) -> Result<Node<Self::Flow, Self::List>, Error>;
    fn list(
        &self,
        state: &CheckerState,
        list: Self::List,
    ) -> Result<List<Self::Flow, Self::List>, Error>;
    fn import_list(&self, list: FlowListId) -> Result<Self::List, Error>;
}

impl Reader for FlowGraph {
    type Flow = FlowId;
    type List = FlowListId;
    fn flow_id(&self, flow: FlowId) -> FlowId {
        flow
    }
    fn node(&self, state: &CheckerState, flow: FlowId) -> Result<Node<FlowId, FlowListId>, Error> {
        let read = self.binding(state)?.flows().get(flow)?;
        Ok(Node {
            flags: read.flags(),
            data: read.node(),
            antecedent: read.antecedent(),
            antecedents: read.antecedents(),
        })
    }
    fn list(
        &self,
        state: &CheckerState,
        list: FlowListId,
    ) -> Result<List<FlowId, FlowListId>, Error> {
        let read = self.binding(state)?.flow_lists().get(list)?;
        Ok(List {
            flow: read.flow(),
            next: read.next(),
        })
    }
    fn import_list(&self, list: FlowListId) -> Result<FlowListId, Error> {
        Ok(list)
    }
}

// The candidate is compiled for tests and experiments, leaving default
// production routing on the checked reader until full-pipeline evidence exists.
#[cfg(any(test, feature = "operation-experiments"))]
impl<'scope> Reader for tsr_ast::FlowScope<'scope, '_> {
    type Flow = tsr_ast::LocalFlow<'scope>;
    type List = tsr_ast::LocalFlowList<'scope>;
    fn flow_id(&self, flow: Self::Flow) -> FlowId {
        self.flow_id(flow)
    }
    fn node(
        &self,
        _: &CheckerState,
        flow: Self::Flow,
    ) -> Result<Node<Self::Flow, Self::List>, Error> {
        let read = self.node(flow);
        Ok(Node {
            flags: read.flags(),
            data: read.node(),
            antecedent: read.antecedent(),
            antecedents: read.antecedents(),
        })
    }
    fn list(
        &self,
        _: &CheckerState,
        list: Self::List,
    ) -> Result<List<Self::Flow, Self::List>, Error> {
        let read = self.list(list);
        Ok(List {
            flow: read.flow(),
            next: read.next(),
        })
    }
    fn import_list(&self, list: FlowListId) -> Result<Self::List, Error> {
        Ok(self.import_list(list)?)
    }
}

fn required<T>(value: Option<T>, context: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(context))
}

/// Check every list item before any branch can return early. Publication allows
/// absent flow values in list records; consuming them as antecedents does not.
fn antecedents<R: Reader>(
    state: &mut CheckerState,
    reader: &R,
    mut list: Option<R::List>,
) -> Result<Option<R::List>, Error> {
    let first = list;
    while let Some(id) = list {
        let item = read_list(state, reader, id)?;
        required(item.flow, "flow list element")?;
        list = item.next;
    }
    Ok(first)
}

fn read_list<R: Reader>(
    state: &mut CheckerState,
    reader: &R,
    list: R::List,
) -> Result<List<R::Flow, R::List>, Error> {
    #[cfg(feature = "operation-experiments")]
    {
        state.flow.reachability_counters.list_reads += 1;
    }
    reader.list(state, list)
}

fn next<R: Reader>(
    state: &mut CheckerState,
    reader: &R,
    list: &mut Option<R::List>,
) -> Result<Option<R::Flow>, Error> {
    let Some(id) = *list else {
        return Ok(None);
    };
    let item = read_list(state, reader, id)?;
    let flow = required(item.flow, "flow list element")?;
    *list = item.next;
    Ok(Some(flow))
}

pub(crate) fn run(
    state: &mut CheckerState,
    graph: &FlowGraph,
    flow: FlowId,
) -> Result<bool, Error> {
    #[cfg(feature = "operation-experiments")]
    {
        if state.flow.local_reachability && graph.shared.is_some() {
            state.flow.reachability_counters.local_requests += 1;
        } else {
            state.flow.reachability_counters.routed_requests += 1;
        }
        // Both experimental readers use the same entry path for completed
        // hits. A cache-heavy replay must not compare bypassing stack growth
        // on one backend with entering the traversal on the other.
        if let Some(result) = completed_root(state, flow) {
            return Ok(result);
        }
        if state.flow.local_reachability {
            return run_local_after_miss(state, graph, flow);
        }
    }
    let result = traverse(state, graph, flow, &mut Vec::new())?;
    state.flow.last_reachable = Some((flow, result));
    Ok(result)
}

#[cfg(any(test, feature = "operation-experiments"))]
fn completed_root(state: &mut CheckerState, flow: FlowId) -> Option<bool> {
    // The worker checks completed caches before reading its root, including
    // a foreign raw root already present in that cache.
    if let Some((last, result)) = state.flow.last_reachable {
        if last == flow {
            return Some(result);
        }
    }
    let result = state.flow.reachable.get(&flow).copied()?;
    state.flow.last_reachable = Some((flow, result));
    Some(result)
}

#[cfg(test)]
fn run_local(state: &mut CheckerState, graph: &FlowGraph, flow: FlowId) -> Result<bool, Error> {
    match completed_root(state, flow) {
        Some(result) => Ok(result),
        None => run_local_after_miss(state, graph, flow),
    }
}

#[cfg(any(test, feature = "operation-experiments"))]
fn run_local_after_miss(
    state: &mut CheckerState,
    graph: &FlowGraph,
    flow: FlowId,
) -> Result<bool, Error> {
    let result = match &graph.shared {
        Some(shared) => shared.view().with_flow_scope(|reader| {
            let local = reader.import_flow(flow)?;
            traverse(state, &reader, local, &mut Vec::new())
        }),
        None => traverse(state, graph, flow, &mut Vec::new()),
    }?;
    state.flow.last_reachable = Some((flow, result));
    Ok(result)
}

// port: tsc/internal/checker/flow.go:Checker.isReachableFlowNodeWorker
fn traverse<R: Reader>(
    state: &mut CheckerState,
    reader: &R,
    mut flow: R::Flow,
    reduced: &mut Vec<FlowReduceLabelData>,
) -> Result<bool, Error> {
    stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
        // Preserve completed memoization and do not cache assumptions made
        // under temporary reduce-label antecedents.
        let mut passed = Vec::new();
        let result = loop {
            let full_flow = reader.flow_id(flow);
            if let Some((last, result)) = state.flow.last_reachable {
                if last == full_flow {
                    break result;
                }
            }
            if reduced.is_empty() {
                if let Some(&result) = state.flow.reachable.get(&full_flow) {
                    break result;
                }
                passed.push(full_flow);
            }
            #[cfg(feature = "operation-experiments")]
            {
                state.flow.reachability_counters.node_reads += 1;
            }
            let node = reader.node(state, flow)?;
            if node.flags & (ff::ASSIGNMENT | ff::CONDITION | ff::ARRAY_MUTATION) != 0 {
                flow = required(node.antecedent, "reachable antecedent")?;
            } else if node.flags & ff::BRANCH_LABEL != 0 {
                // port: tsc/internal/checker/flow.go:Checker.getTypeAtFlowBranchLabel
                // port: tsc/internal/checker/flow.go:getBranchLabelAntecedents
                let list = match reduced.iter().rev().find(|r| r.target == Some(full_flow)) {
                    Some(reduction) => reduction
                        .antecedents
                        .map(|id| reader.import_list(id))
                        .transpose()?,
                    None => node.antecedents,
                };
                let mut list = antecedents(state, reader, list)?;
                let mut reachable = false;
                while let Some(branch) = next(state, reader, &mut list)? {
                    if traverse(state, reader, branch, reduced)? {
                        reachable = true;
                        break;
                    }
                }
                break reachable;
            } else if node.flags & ff::LOOP_LABEL != 0 {
                let mut list = antecedents(state, reader, node.antecedents)?;
                let Some(first) = next(state, reader, &mut list)? else {
                    break false;
                };
                flow = first;
            } else if node.flags & ff::REDUCE_LABEL != 0 {
                let Some(FlowData::ReduceLabel(data)) = node.data else {
                    return Err(tsr_arena::Error::InvalidGraph.into());
                };
                state.flow.last_reachable = None;
                reduced.push(data);
                let result = required(node.antecedent, "reduce antecedent")
                    .and_then(|antecedent| traverse(state, reader, antecedent, reduced));
                reduced.pop();
                break result?;
            } else if node.flags & ff::CALL != 0 {
                // port: tsc/internal/checker/flow.go:Checker.getTypeAtFlowCall
                let Some(FlowData::Ast(call)) = node.data else {
                    return Err(tsr_arena::Error::InvalidGraph.into());
                };
                #[cfg(feature = "operation-experiments")]
                {
                    state.flow.reachability_counters.semantic_calls += 1;
                }
                if state.flow_call_is_never(call)? {
                    break false;
                }
                flow = required(node.antecedent, "reachable call antecedent")?;
            } else if node.flags & ff::SWITCH_CLAUSE != 0 {
                let Some(FlowData::SwitchClause(data)) = node.data else {
                    return Err(tsr_arena::Error::InvalidGraph.into());
                };
                if data.is_empty() {
                    let switch = required(data.switch_statement, "reachable switch statement")?;
                    #[cfg(feature = "operation-experiments")]
                    {
                        state.flow.reachability_counters.semantic_calls += 1;
                    }
                    if state.flow_switch_exhaustive(switch)? {
                        break false;
                    }
                }
                flow = required(node.antecedent, "reachable switch antecedent")?;
            } else {
                break node.flags & ff::UNREACHABLE == 0;
            }
        };
        for flow in passed {
            state.flow.reachable.insert(flow, result);
        }
        Ok(result)
    })
}

#[cfg(feature = "operation-experiments")]
mod probe;
#[cfg(feature = "operation-experiments")]
pub(crate) use probe::{CaptureState, ReachabilityCounters};
#[cfg(feature = "operation-experiments")]
pub use probe::{FlowReachabilityCapture, FlowReachabilityReplay, FlowReachabilityRequest};

#[cfg(test)]
mod tests;
