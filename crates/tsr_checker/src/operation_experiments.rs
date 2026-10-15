//! Development-only alternatives for complete checker operations.
//!
//! Observation and timing are separate executions. An observed contextual
//! request is not a replayable snapshot: inference and checker caches are live.
//! Each backend must start in a fresh checker and execute the same program
//! prefix. These measurements do not replace whole-compiler parity or timing.

use crate::{contextual_argument::ArgumentRequest, CheckerState, Error, Operation, TypeId};
use std::collections::{hash_map::DefaultHasher, BTreeMap};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArgumentBackend {
    #[default]
    Routed,
    ScopedCore,
    /// Retained owner plus shape-selected typed payload fields.
    TypedPayload,
    /// Retained owner plus checker-owned expression continuation frames.
    ExecutionState,
    /// Both structural changes in the same operation.
    Combined,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservationMode {
    /// Read source locations and hash the ordered request/result sequence.
    Observe,
    /// Time outermost operations; nested work belongs to the outer interval.
    Time,
}

#[derive(Clone, Debug)]
pub struct ArgumentObservation {
    pub ordinal: u64,
    pub depth: usize,
    pub file: Vec<u8>,
    pub pos: i32,
    pub end: i32,
    pub kind: i16,
    pub node_slot: u32,
    pub checker_owned: bool,
    pub location_error: Option<String>,
    pub contextual_type: u32,
    pub inferential: bool,
    pub inference_context: Option<usize>,
    pub mode: u32,
    pub result_type: Option<u32>,
    pub error: Option<String>,
    pub types_before: usize,
    pub types_after: usize,
    pub context_depth_before: usize,
    pub context_depth_after: usize,
    pub inference_depth_before: usize,
    pub inference_depth_after: usize,
    pub expression_mode_before: u32,
    pub expression_mode_after: u32,
    pub current_node_restored: bool,
    pub read_scope_restored: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ArgumentReport {
    pub calls: u64,
    pub outermost_calls: u64,
    pub errors: u64,
    pub location_errors: u64,
    pub cleanup_errors: u64,
    pub elapsed: Duration,
    /// Diagnostic checksum of every entry and return, not a parity oracle.
    pub trace_checksum: u64,
    pub kinds: BTreeMap<i16, u64>,
    pub modes: BTreeMap<u32, u64>,
    pub depths: BTreeMap<usize, u64>,
    pub inferential_calls: u64,
    pub samples: Vec<ArgumentObservation>,
}

#[derive(Default)]
pub(crate) struct ExperimentState {
    configuration: Option<(ArgumentBackend, ObservationMode)>,
    depth: usize,
    report: ArgumentReport,
    hash: DefaultHasher,
}

impl Operation<'_> {
    /// Experiment-owned retained storage, read after the measured operation.
    /// This is a capacity charge, not allocator traffic or whole-checker memory.
    pub fn argument_frame_storage(&self) -> (usize, usize) {
        let frames = &self.state().expression_execution_frames;
        (frames.capacity(), frames.retained_bytes())
    }

    pub fn begin_argument_experiment(
        &mut self,
        backend: ArgumentBackend,
        mode: ObservationMode,
    ) -> Result<(), Error> {
        let state = self.state_mut();
        if state.operation_experiments.configuration.is_some() {
            return Err(Error::Reentry);
        }
        state.operation_experiments = ExperimentState {
            configuration: Some((backend, mode)),
            ..ExperimentState::default()
        };
        Ok(())
    }

    pub fn finish_argument_experiment(&mut self) -> ArgumentReport {
        let experiment = std::mem::take(&mut self.state_mut().operation_experiments);
        let mut report = experiment.report;
        report.trace_checksum = experiment.hash.finish();
        report
    }
}

fn location(
    state: &CheckerState,
    request: ArgumentRequest,
) -> Result<(Vec<u8>, i32, i32, i16), Error> {
    let view = state.ast(request.node)?;
    let node = view.node(request.node)?;
    let source = tsr_ast::utilities::get_source_file_of_node(view, Some(request.node))?;
    let file = match source {
        Some(source) => view.source_file(source)?.file_name().to_vec(),
        None => Vec::new(), // A synthetic expression has no source file.
    };
    Ok((file, node.pos(), node.end(), node.kind().raw()))
}

/// Stable input identity is local to this checker and the observed source;
/// process-wide arena numbers are deliberately absent. Every request is hashed
/// even when its source metadata is malformed, then gets a separate exit tag.
fn hash_entry(observed: &ArgumentObservation, hash: &mut DefaultHasher) {
    (0u8, observed.ordinal, observed.depth).hash(hash);
    (
        &observed.file,
        observed.pos,
        observed.end,
        observed.kind,
        observed.node_slot,
        observed.checker_owned,
        &observed.location_error,
    )
        .hash(hash);
    (
        observed.contextual_type,
        observed.inference_context,
        observed.mode,
        observed.types_before,
    )
        .hash(hash);
    (
        observed.context_depth_before,
        observed.inference_depth_before,
        observed.expression_mode_before,
    )
        .hash(hash);
}

fn hash_exit(observed: &ArgumentObservation, hash: &mut DefaultHasher) {
    (
        1u8,
        observed.ordinal,
        observed.result_type,
        &observed.error,
        observed.types_after,
    )
        .hash(hash);
    (
        observed.context_depth_after,
        observed.inference_depth_after,
        observed.expression_mode_after,
        observed.current_node_restored,
        observed.read_scope_restored,
    )
        .hash(hash);
}

pub(crate) fn check_contextual_argument(
    state: &mut CheckerState,
    request: ArgumentRequest,
) -> Result<TypeId, Error> {
    let Some((backend, mode)) = state.operation_experiments.configuration else {
        return crate::contextual_argument::check(state, request);
    };
    let depth = state.operation_experiments.depth;
    let ordinal = state.operation_experiments.report.calls;
    let context_depth_before = state.calls.contexts.len();
    let inference_depth_before = state.calls.inference_contexts.len();
    let expression_mode_before = state.expression_mode;
    let current_node_before = state.current_node;
    let typed_before = state.argument_typed_payloads;
    let execution_before = state.argument_explicit_execution;
    let frames_before = state.expression_execution_frames.len();
    let scope_before = state
        .argument_read_scope
        .as_ref()
        .map(crate::program::ArgumentReadScope::arena);
    state.operation_experiments.report.calls += 1;
    state.operation_experiments.report.outermost_calls += u64::from(depth == 0);
    let mut observation = None;
    if mode == ObservationMode::Observe {
        let (file, pos, end, kind, location_error) = match location(state, request) {
            Ok((file, pos, end, kind)) => (file, pos, end, kind, None),
            Err(error) => {
                state.operation_experiments.report.location_errors += 1;
                // The explicit error makes these placeholder location fields
                // unavailable, not a successful synthetic-node observation.
                (Vec::new(), 0, 0, 0, Some(error.to_string()))
            }
        };
        let observed = ArgumentObservation {
            ordinal,
            depth,
            file,
            pos,
            end,
            kind,
            node_slot: request.node.slot(),
            checker_owned: request.node.arena() == state.factory.id().arena(),
            location_error,
            contextual_type: request.contextual.get(),
            inferential: request.inference.is_some(),
            inference_context: request.inference.and_then(|inference| inference.index(0)),
            mode: request.mode,
            result_type: None,
            error: None,
            types_before: state.types.len(),
            types_after: 0,
            context_depth_before,
            context_depth_after: 0,
            inference_depth_before,
            inference_depth_after: 0,
            expression_mode_before,
            expression_mode_after: 0,
            current_node_restored: false,
            read_scope_restored: false,
        };
        let experiment = &mut state.operation_experiments;
        if observed.location_error.is_none() {
            *experiment.report.kinds.entry(kind).or_default() += 1;
        }
        *experiment.report.modes.entry(request.mode).or_default() += 1;
        *experiment.report.depths.entry(depth).or_default() += 1;
        experiment.report.inferential_calls += u64::from(request.inference.is_some());
        hash_entry(&observed, &mut experiment.hash);
        observation = Some(observed);
    }
    state.operation_experiments.depth += 1;
    let started = (mode == ObservationMode::Time && depth == 0).then(Instant::now);
    let result = match backend {
        ArgumentBackend::Routed => crate::contextual_argument::check(state, request),
        ArgumentBackend::ScopedCore => crate::contextual_argument::check_scoped(state, request),
        ArgumentBackend::TypedPayload => check_typed_argument(state, request, false),
        ArgumentBackend::ExecutionState => {
            crate::contextual_argument::check_execution(state, request)
        }
        ArgumentBackend::Combined => check_typed_argument(state, request, true),
    };
    if let Some(started) = started {
        state.operation_experiments.report.elapsed += started.elapsed();
    }
    state.operation_experiments.depth -= 1;
    state.operation_experiments.report.errors += u64::from(result.is_err());
    let current_node_restored = state.current_node == current_node_before;
    let read_scope_restored = state
        .argument_read_scope
        .as_ref()
        .map(crate::program::ArgumentReadScope::arena)
        == scope_before;
    let cleanup_restored = state.calls.contexts.len() == context_depth_before
        && state.calls.inference_contexts.len() == inference_depth_before
        && state.expression_mode == expression_mode_before
        && current_node_restored
        && read_scope_restored
        && state.argument_typed_payloads == typed_before
        && state.argument_explicit_execution == execution_before
        && state.expression_execution_frames.len() == frames_before;
    state.operation_experiments.report.cleanup_errors += u64::from(!cleanup_restored);
    if let Some(mut observed) = observation {
        observed.types_after = state.types.len();
        observed.context_depth_after = state.calls.contexts.len();
        observed.inference_depth_after = state.calls.inference_contexts.len();
        observed.expression_mode_after = state.expression_mode;
        observed.current_node_restored = current_node_restored;
        observed.read_scope_restored = read_scope_restored;
        match &result {
            Ok(ty) => observed.result_type = Some(ty.get()),
            Err(error) => observed.error = Some(error.to_string()),
        }
        let experiment = &mut state.operation_experiments;
        hash_exit(&observed, &mut experiment.hash);
        if ordinal < 300 {
            experiment.report.samples.push(observed);
        }
    }
    result
}

fn check_typed_argument(
    state: &mut CheckerState,
    request: ArgumentRequest,
    explicit: bool,
) -> Result<TypeId, Error> {
    let previous = std::mem::replace(&mut state.argument_typed_payloads, true);
    let result = if explicit {
        crate::contextual_argument::check_execution(state, request)
    } else {
        crate::contextual_argument::check_scoped(state, request)
    };
    state.argument_typed_payloads = previous;
    result
}

#[cfg(test)]
mod tests;
