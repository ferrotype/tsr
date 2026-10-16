use super::*;
use tsr_ast::{AstBuilder, BindBuilder, FlowList, FlowNode, SourceFileParseOptions};
use tsr_jsstring::SourceText;

fn graph<T>(
    counters: &tsr_arena::Counters,
    build: impl FnOnce(&mut BindBuilder<'_>) -> T,
) -> (FlowGraph, T) {
    let text = SourceText::default();
    let mut builder = AstBuilder::new(text.clone(), counters);
    let source = builder.new_source_file(
        SourceFileParseOptions {
            file_name: tsr_ast::JsString::from_bytes(b"/reachability.ts".as_slice()),
            ..SourceFileParseOptions::default()
        },
        text,
        None,
        None,
    );
    let mut result = None;
    let completed = builder
        .complete(source)
        .unwrap()
        .bind_and_publish(|binding| {
            result = Some(build(binding));
            Ok(())
        })
        .unwrap();
    (
        FlowGraph {
            owner: source,
            shared: completed.shared(),
        },
        result.unwrap(),
    )
}

fn list(binding: &mut BindBuilder<'_>, values: &[Option<FlowId>]) -> Option<FlowListId> {
    let mut next = None;
    for &flow in values.iter().rev() {
        next = Some(binding.flow_lists_mut().push(FlowList { flow, next }));
    }
    next
}

fn compare(state: &mut CheckerState, graph: &FlowGraph, roots: &[(FlowId, Result<bool, Error>)]) {
    state.flow.reachable.clear();
    state.flow.last_reachable = None;
    for (flow, expected) in roots {
        assert_eq!(&run(state, graph, *flow), expected);
    }
    let checked_cache = state.flow.reachable.clone();
    let checked_last = state.flow.last_reachable;
    state.flow.reachable.clear();
    state.flow.last_reachable = None;
    for (flow, expected) in roots {
        assert_eq!(&run_local(state, graph, *flow), expected);
    }
    assert_eq!(state.flow.reachable, checked_cache);
    assert_eq!(state.flow.last_reachable, checked_last);
}

#[test]
fn completed_graphs_keep_branch_loop_and_reduction_semantics() {
    let (counters, _generation, _identity, owner) = crate::tests::owner();
    let (graph, (branch, loop_flow, empty_loop, reduce, start)) = graph(&counters, |binding| {
        let start = binding.flows_mut().push(FlowNode::new(ff::START));
        let unreachable = binding.flows_mut().push(FlowNode::new(ff::UNREACHABLE));
        let branches = list(binding, &[Some(unreachable), Some(start)]);
        let branch = binding.flows_mut().push(FlowNode {
            flags: ff::BRANCH_LABEL,
            antecedents: branches,
            ..FlowNode::default()
        });
        let loop_flow = binding.flows_mut().push(FlowNode {
            flags: ff::LOOP_LABEL,
            antecedents: branches,
            ..FlowNode::default()
        });
        let empty_loop = binding.flows_mut().push(FlowNode::new(ff::LOOP_LABEL));
        let reduced = list(binding, &[Some(unreachable)]);
        let reduce = binding.flows_mut().push(FlowNode::new_ex(
            ff::REDUCE_LABEL,
            Some(FlowData::ReduceLabel(FlowReduceLabelData::new(
                Some(branch),
                reduced,
            ))),
            Some(branch),
        ));
        (branch, loop_flow, empty_loop, reduce, start)
    });
    let mut operation = owner.operation().unwrap();
    compare(
        operation.state_mut(),
        &graph,
        &[
            (reduce, Ok(false)),
            (branch, Ok(true)),
            (loop_flow, Ok(false)),
            (empty_loop, Ok(false)),
            (start, Ok(true)),
            (reduce, Ok(false)),
        ],
    );
}

#[test]
fn both_readers_validate_all_antecedents_before_an_early_branch_or_loop_result() {
    let (counters, _generation, _identity, owner) = crate::tests::owner();
    let (graph, (branch, loop_flow)) = graph(&counters, |binding| {
        let start = binding.flows_mut().push(FlowNode::new(ff::START));
        let branches = list(binding, &[Some(start), None]);
        let branch = binding.flows_mut().push(FlowNode {
            flags: ff::BRANCH_LABEL,
            antecedents: branches,
            ..FlowNode::default()
        });
        let loop_flow = binding.flows_mut().push(FlowNode {
            flags: ff::LOOP_LABEL,
            antecedents: branches,
            ..FlowNode::default()
        });
        (branch, loop_flow)
    });
    let mut operation = owner.operation().unwrap();
    compare(
        operation.state_mut(),
        &graph,
        &[
            (branch, Err(Error::MissingLink("flow list element"))),
            (loop_flow, Err(Error::MissingLink("flow list element"))),
        ],
    );
    assert!(operation.state().flow.reachable.is_empty());
}

#[test]
fn missing_edges_and_payload_errors_keep_the_same_failure_and_no_completed_result() {
    let (counters, _generation, _identity, owner) = crate::tests::owner();
    let (graph, roots) = graph(&counters, |binding| {
        let mut roots = Vec::new();
        let assignment = binding.flows_mut().push(FlowNode::new(ff::ASSIGNMENT));
        roots.push((assignment, Err(Error::MissingLink("reachable antecedent"))));
        for flags in [ff::CALL, ff::SWITCH_CLAUSE, ff::REDUCE_LABEL] {
            let flow = binding.flows_mut().push(FlowNode::new(flags));
            roots.push((flow, Err(Error::Arena(tsr_arena::Error::InvalidGraph))));
        }
        let reduce = binding.flows_mut().push(FlowNode::new_ex(
            ff::REDUCE_LABEL,
            Some(FlowData::ReduceLabel(FlowReduceLabelData::new(None, None))),
            None,
        ));
        roots.push((reduce, Err(Error::MissingLink("reduce antecedent"))));
        let switch = binding.flows_mut().push(FlowNode::new_ex(
            ff::SWITCH_CLAUSE,
            Some(FlowData::SwitchClause(tsr_ast::FlowSwitchClauseData::new(
                None, 0, 0,
            ))),
            None,
        ));
        roots.push((
            switch,
            Err(Error::MissingLink("reachable switch statement")),
        ));
        roots
    });
    let mut operation = owner.operation().unwrap();
    compare(operation.state_mut(), &graph, &roots);
    assert!(operation.state().flow.reachable.is_empty());
    let mut reductions = Vec::new();
    assert_eq!(
        traverse(operation.state_mut(), &graph, roots[4].0, &mut reductions),
        Err(Error::MissingLink("reduce antecedent"))
    );
    assert!(reductions.is_empty());
}

#[test]
fn external_roots_check_owner_after_the_existing_completed_cache_shortcut() {
    let (counters, _generation, _identity, owner) = crate::tests::owner();
    let (graph, _) = graph(&counters, |binding| {
        binding.flows_mut().push(FlowNode::new(ff::START))
    });
    let mut foreign = tsr_ast::FlowNodes::new(&counters);
    let flow = foreign.push(FlowNode::new(ff::START));
    let mut operation = owner.operation().unwrap();
    compare(
        operation.state_mut(),
        &graph,
        &[(flow, Err(Error::Arena(tsr_arena::Error::WrongOwner)))],
    );
    operation.state_mut().flow.reachable.insert(flow, true);
    assert_eq!(run(operation.state_mut(), &graph, flow), Ok(true));
    operation.state_mut().flow.last_reachable = None;
    assert_eq!(run_local(operation.state_mut(), &graph, flow), Ok(true));
}

#[cfg(feature = "operation-experiments")]
#[test]
fn experimental_readers_share_cached_root_entry_and_counter_semantics() {
    let (counters, _generation, _identity, owner) = crate::tests::owner();
    let (graph, _) = graph(&counters, |binding| {
        binding.flows_mut().push(FlowNode::new(ff::START))
    });
    let mut foreign = tsr_ast::FlowNodes::new(&counters);
    let flow = foreign.push(FlowNode::new(ff::START));
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    state.flow.reachable.insert(flow, true);
    for local in [false, true] {
        state.flow.local_reachability = local;
        state.flow.last_reachable = None;
        assert_eq!(run(state, &graph, flow), Ok(true));
        assert_eq!(state.flow.last_reachable, Some((flow, true)));
        // Exercise both the map hit and subsequent last-root hit.
        assert_eq!(run(state, &graph, flow), Ok(true));
    }
    let counts = &state.flow.reachability_counters;
    assert_eq!(counts.routed_requests, 2);
    assert_eq!(counts.local_requests, 2);
    assert_eq!(counts.node_reads, 0);
    assert_eq!(counts.list_reads, 0);
    assert_eq!(counts.semantic_calls, 0);
}
