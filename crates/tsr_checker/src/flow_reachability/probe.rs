//! Captured production requests and a replay of the actual reachability worker.
//! Semantic caches are deliberately left warm; only completed reachability
//! results are cleared at the start of each replay batch.

use crate::{CheckerState, Error, Operation};
use tsr_arena::NodeId;
use tsr_ast::FlowId;

/// Native graph identities. Requests may only be replayed against the program
/// whose checking captured them; they do not retain its file owners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlowReachabilityRequest {
    pub owner: NodeId,
    pub flow: FlowId,
}

#[derive(Debug, Default)]
pub struct FlowReachabilityCapture {
    pub requests: Vec<FlowReachabilityRequest>,
    pub total_seen: u64,
    /// At least one stride-selected request exceeded the configured capacity.
    pub truncated: bool,
}

#[derive(Debug, Default)]
pub struct FlowReachabilityReplay {
    /// Every request keeps its outcome, including its exact checker error.
    pub results: Vec<Result<bool, String>>,
    pub local_requests: u64,
    pub routed_requests: u64,
    pub node_reads: u64,
    pub list_reads: u64,
    pub semantic_calls: u64,
}

#[derive(Default)]
pub(crate) struct ReachabilityCounters {
    pub(crate) local_requests: u64,
    pub(crate) routed_requests: u64,
    pub(crate) node_reads: u64,
    pub(crate) list_reads: u64,
    pub(crate) semantic_calls: u64,
}

pub(crate) struct CaptureState {
    limit: usize,
    stride: u64,
    capture: FlowReachabilityCapture,
}

impl CheckerState {
    pub(crate) fn capture_reachability(&mut self, owner: NodeId, flow: FlowId) {
        let owner = self
            .flow
            .synthetic
            .get(&owner)
            .map_or(owner, |&(source, _)| source);
        let Some(state) = &mut self.flow.capture else {
            return;
        };
        let index = state.capture.total_seen;
        state.capture.total_seen += 1;
        if index % state.stride != 0 {
            return;
        }
        if state.capture.requests.len() == state.limit {
            state.capture.truncated = true;
        } else {
            state
                .capture
                .requests
                .push(FlowReachabilityRequest { owner, flow });
        }
    }
}

impl Operation<'_> {
    /// Start deterministic bounded capture of root queries, including requests
    /// that immediately hit the completed reachability cache.
    pub fn begin_flow_capture(&mut self, max_requests: usize, stride: usize) -> Result<(), Error> {
        if self.state().flow.capture.is_some() {
            return Err(Error::Reentry);
        }
        if max_requests == 0 || stride == 0 {
            return Err(Error::Unsupported(
                "flow capture requires nonzero limit and stride",
            ));
        }
        self.state_mut().flow.capture = Some(CaptureState {
            limit: max_requests,
            stride: stride as u64,
            capture: FlowReachabilityCapture::default(),
        });
        Ok(())
    }

    pub fn finish_flow_capture(&mut self) -> FlowReachabilityCapture {
        self.state_mut()
            .flow
            .capture
            .take()
            .map_or_else(FlowReachabilityCapture::default, |state| state.capture)
    }

    /// Replay actual graph traversals with real checker semantic callbacks.
    /// The caller must prepare independently warmed checkers for the readers;
    /// running two readers on one checker is not an unbiased comparison.
    /// Root import/scope setup and cache initialization are part of this call.
    pub fn replay_flow_reachability(
        &mut self,
        requests: &[FlowReachabilityRequest],
        local: bool,
    ) -> FlowReachabilityReplay {
        let state = self.state_mut();
        let capture = state.flow.capture.take();
        let previous_local = std::mem::replace(&mut state.flow.local_reachability, local);
        state.flow.reachable.clear();
        state.flow.last_reachable = None;
        state.flow.reachability_counters = ReachabilityCounters::default();
        let results = requests
            .iter()
            .map(|request| {
                let result = state
                    .flow_graph(request.owner)
                    .and_then(|graph| super::run(state, &graph, request.flow));
                result.map_err(|error| format!("{error:?}"))
            })
            .collect();
        state.flow.capture = capture;
        state.flow.local_reachability = previous_local;
        let counters = &state.flow.reachability_counters;
        FlowReachabilityReplay {
            results,
            local_requests: counters.local_requests,
            routed_requests: counters.routed_requests,
            node_reads: counters.node_reads,
            list_reads: counters.list_reads,
            semantic_calls: counters.semantic_calls,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tsr_ast::{FlowNode, FlowNodes};

    #[test]
    fn capture_is_bounded_ordered_and_counts_hot_requests() {
        let (counters, _generation, _identity, owner) = crate::tests::owner();
        let mut flows = FlowNodes::new(&counters);
        let flow = flows.push(FlowNode::default());
        let node = NodeId::from_parts(flows.id(), 1).unwrap();
        let mut operation = owner.operation().unwrap();
        assert!(operation.begin_flow_capture(0, 1).is_err());
        assert!(operation.begin_flow_capture(1, 0).is_err());
        operation.begin_flow_capture(2, 2).unwrap();
        assert_eq!(operation.begin_flow_capture(99, 1), Err(Error::Reentry));
        operation.state_mut().flow.last_reachable = Some((flow, true));
        for _ in 0..7 {
            assert!(operation.state_mut().reachable_flow(node, flow).unwrap());
        }
        let capture = operation.finish_flow_capture();
        assert_eq!(capture.total_seen, 7);
        assert_eq!(
            capture.requests,
            vec![FlowReachabilityRequest { owner: node, flow }; 2]
        );
        assert!(capture.truncated);
        assert_eq!(operation.finish_flow_capture().total_seen, 0);
    }
}
