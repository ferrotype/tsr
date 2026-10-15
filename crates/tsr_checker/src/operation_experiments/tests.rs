use super::*;
use tsr_arena::NodeId;
use tsr_ast::{FactoryMethods, JsString, SyntaxKind};

#[test]
fn observed_failure_keeps_result_type_delta_and_cleanup_evidence() {
    let (_counters, _generation, _identity, owner) = crate::tests::owner();
    let mut operation = owner.operation().unwrap();
    operation
        .begin_argument_experiment(ArgumentBackend::ScopedCore, ObservationMode::Observe)
        .unwrap();
    let state = operation.state_mut();
    let node = state
        .factory
        .new_string_literal(JsString::from_bytes(b"value".as_slice()), 0);
    let previous = state.factory.new_token(SyntaxKind::TrueKeyword.into());
    state.current_node = Some(previous);
    state.expression_mode = 16;
    let invalid = TypeId::next(0, state.types.len() + 100).unwrap();
    let before = state.types.len();
    assert_eq!(
        state.check_call_argument_ex(node, invalid, None, 4),
        Err(Error::Arena(tsr_arena::Error::InvalidSlot))
    );
    let after = state.types.len();
    let report = operation.finish_argument_experiment();
    assert_eq!(report.calls, 1);
    assert_eq!(report.outermost_calls, 1);
    assert_eq!(report.errors, 1);
    assert_eq!(report.location_errors, 0);
    assert_eq!(report.cleanup_errors, 0);
    assert_eq!(report.modes.get(&4), Some(&1));
    let observed = &report.samples[0];
    assert_eq!(observed.contextual_type, invalid.get());
    assert_eq!(observed.result_type, None);
    assert!(observed.error.is_some());
    assert_eq!(observed.types_before, before);
    assert_eq!(observed.types_after, after);
    assert_eq!(observed.expression_mode_before, 16);
    assert_eq!(observed.expression_mode_after, 16);
    assert!(observed.current_node_restored);
    assert!(observed.read_scope_restored);
}

fn inference_observation(index: usize) -> ArgumentReport {
    let (_counters, _generation, _identity, owner) = crate::tests::owner();
    let mut operation = owner.operation().unwrap();
    operation
        .begin_argument_experiment(ArgumentBackend::Routed, ObservationMode::Observe)
        .unwrap();
    let state = operation.state_mut();
    let number = state.builtins.number_type;
    let node = state.new_synthetic_expression(number, false, None).unwrap();
    for _ in 0..index {
        state.new_inference_context(&[], None, 0).unwrap();
    }
    let inference = state.new_inference_context(&[], None, 0).unwrap();
    assert_eq!(
        state.check_call_argument_ex(node, number, Some(inference), 0),
        Ok(number)
    );
    operation.finish_argument_experiment()
}

#[test]
fn observed_request_distinguishes_inference_contexts_with_identical_modes() {
    let first = inference_observation(0);
    let second = inference_observation(1);
    assert_eq!(first.calls, second.calls);
    assert_eq!(first.kinds, second.kinds);
    assert_eq!(first.modes, second.modes);
    assert_eq!(first.samples[0].inference_context, Some(0));
    assert_eq!(second.samples[0].inference_context, Some(1));
    assert_ne!(first.trace_checksum, second.trace_checksum);
    assert_eq!(first.cleanup_errors, 0);
    assert_eq!(second.cleanup_errors, 0);
}

#[test]
fn malformed_location_keeps_the_failed_request_and_result() {
    let (_counters, _generation, _identity, owner) = crate::tests::owner();
    let mut operation = owner.operation().unwrap();
    operation
        .begin_argument_experiment(ArgumentBackend::Routed, ObservationMode::Observe)
        .unwrap();
    let state = operation.state_mut();
    let invalid = NodeId::from_parts(state.factory.id().arena(), u32::MAX).unwrap();
    let number = state.builtins.number_type;
    assert_eq!(
        state.check_call_argument_ex(invalid, number, None, 0),
        Err(Error::Arena(tsr_arena::Error::InvalidSlot))
    );
    let report = operation.finish_argument_experiment();
    assert_eq!(report.calls, 1);
    assert_eq!(report.errors, 1);
    assert_eq!(report.location_errors, 1);
    assert_eq!(report.cleanup_errors, 0);
    assert_eq!(report.samples.len(), 1);
    assert_eq!(report.samples[0].node_slot, u32::MAX);
    assert!(report.samples[0].location_error.is_some());
    assert!(report.samples[0].error.is_some());
    assert!(report.kinds.is_empty());
}

#[test]
fn structural_backends_restore_selection_after_recoverable_errors() {
    for backend in [
        ArgumentBackend::TypedPayload,
        ArgumentBackend::ExecutionState,
        ArgumentBackend::Combined,
    ] {
        let (_counters, _generation, _identity, owner) = crate::tests::owner();
        let mut operation = owner.operation().unwrap();
        operation
            .begin_argument_experiment(backend, ObservationMode::Observe)
            .unwrap();
        let state = operation.state_mut();
        let missing = state.factory.new_parenthesized_expression(None);
        let number = state.builtins.number_type;
        assert_eq!(
            state.check_call_argument_ex(missing, number, None, 0),
            Err(Error::MissingLink("parenthesized expression")),
        );
        assert!(!state.argument_typed_payloads);
        assert!(!state.argument_explicit_execution);
        let valid = state.new_synthetic_expression(number, false, None).unwrap();
        assert_eq!(
            state.check_call_argument_ex(valid, number, None, 0),
            Ok(number)
        );
        let report = operation.finish_argument_experiment();
        assert_eq!(report.calls, 2);
        assert_eq!(report.errors, 1);
        assert_eq!(report.cleanup_errors, 0);
    }
}
