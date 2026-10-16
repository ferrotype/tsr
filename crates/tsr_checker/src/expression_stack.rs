//! WebAssembly cannot grow its engine call stack. Binary and parenthesized
//! expression continuations carry the same entry/exit state as checkExpressionEx
//! on the heap; operator semantics remain in the ordinary checker worker. The
//! contextual execution experiment shares this machine but keeps its frames in
//! checker-owned storage, including across reentrant semantic callbacks.
use crate::{trace::TraceSpan, CheckerState, Error, TypeId};
use tsr_arena::NodeId;
use tsr_ast::SyntaxKind as K;

#[derive(Clone, Copy)]
enum Step {
    Enter,
    Parenthesized,
    Left {
        left: NodeId,
        right: NodeId,
        token: NodeId,
    },
    Right {
        left: NodeId,
        right: NodeId,
        token: NodeId,
        ty: TypeId,
    },
}

struct Frame {
    node: NodeId,
    previous_node: Option<NodeId>,
    previous_mode: u32,
    step: Step,
    // Drop after state restoration, as the local guard in checkExpressionEx.
    _trace: Option<TraceSpan>,
}

#[derive(Default)]
pub(crate) struct Frames(Vec<Frame>);

#[cfg(feature = "operation-experiments")]
impl Frames {
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn capacity(&self) -> usize {
        self.0.capacity()
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.0.capacity() * std::mem::size_of::<Frame>()
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        // An unwind must end trace scopes innermost first too. Vec's ordinary
        // element destruction order is not the recursive call stack's order.
        while self.0.pop().is_some() {}
    }
}

// Two monomorphizations of the same machine: ordinary wasm/probe calls own a
// local stack; the experiment takes a slice of the checker's reusable stack.
// No mutable frame reference survives a semantic callback.
trait FrameStack {
    fn push(&mut self, state: &mut CheckerState, frame: Frame);
    fn pop(&mut self, state: &mut CheckerState) -> Option<Frame>;
}

impl FrameStack for Frames {
    fn push(&mut self, _: &mut CheckerState, frame: Frame) {
        self.0.push(frame);
    }

    fn pop(&mut self, _: &mut CheckerState) -> Option<Frame> {
        self.0.pop()
    }
}

#[cfg(feature = "operation-experiments")]
struct SharedFrames {
    base: usize,
}

#[cfg(feature = "operation-experiments")]
impl FrameStack for SharedFrames {
    fn push(&mut self, state: &mut CheckerState, frame: Frame) {
        state.expression_execution_frames.0.push(frame);
    }

    fn pop(&mut self, state: &mut CheckerState) -> Option<Frame> {
        if state.expression_execution_frames.0.len() == self.base {
            None
        } else {
            state.expression_execution_frames.0.pop()
        }
    }
}

// Owns only a mutable borrow, not a raw pointer: nested calls borrow it again
// and get their own base. On panic this closes this call's trace scopes before
// its caller unwinds. The outer operation still retires the checker generation.
#[cfg(feature = "operation-experiments")]
struct ExecutionScope<'a> {
    state: &'a mut CheckerState,
    base: usize,
}

#[cfg(feature = "operation-experiments")]
impl Drop for ExecutionScope<'_> {
    fn drop(&mut self) {
        while self.state.expression_execution_frames.0.len() > self.base {
            let frame = self
                .state
                .expression_execution_frames
                .0
                .pop()
                .expect("active expression scope");
            self.state.leave_expression_frame(frame);
        }
    }
}

impl CheckerState {
    fn enter_expression_frame(&mut self, node: NodeId, mode: u32) -> Result<Frame, Error> {
        let trace = self.trace_node_span("checkExpression", node)?;
        let previous_mode = std::mem::replace(&mut self.expression_mode, mode);
        let previous_node = self.current_node.replace(node);
        // Deliberately not restored on exit: this is the pin's reset, not a
        // dynamically scoped counter.
        self.instantiation.count = 0;
        #[cfg(feature = "recursion-probe")]
        if let Some(probe) = &mut self.relations.recursion_probe {
            probe.expression_depth += 1;
            probe.maximum_expression_depth =
                probe.maximum_expression_depth.max(probe.expression_depth);
        }
        Ok(Frame {
            node,
            previous_node,
            previous_mode,
            step: Step::Enter,
            _trace: trace,
        })
    }

    fn leave_expression_frame(&mut self, frame: Frame) {
        #[cfg(feature = "recursion-probe")]
        if let Some(probe) = &mut self.relations.recursion_probe {
            probe.expression_depth -= 1;
        }
        self.current_node = frame.previous_node;
        self.expression_mode = frame.previous_mode;
        drop(frame);
    }

    #[cfg(any(target_arch = "wasm32", feature = "recursion-probe"))]
    pub(crate) fn check_expression_iterative(
        &mut self,
        node: NodeId,
        mode: u32,
    ) -> Result<TypeId, Error> {
        self.check_expression_machine(node, mode, Frames::default())
    }

    #[cfg(feature = "operation-experiments")]
    pub(crate) fn check_expression_explicit(
        &mut self,
        node: NodeId,
        mode: u32,
    ) -> Result<TypeId, Error> {
        let base = self.expression_execution_frames.0.len();
        let scope = ExecutionScope { state: self, base };
        // The machine eliminates recursive binary/wrapper frames, but semantic
        // callbacks can enter another machine. Keep the native stack bound at
        // that boundary; this is not an experiment in disabling stack safety.
        #[cfg(not(target_arch = "wasm32"))]
        {
            stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                #[cfg(feature = "recursion-probe")]
                if let Some(probe) = &mut scope.state.relations.recursion_probe {
                    probe.maximum_remaining_stack = probe
                        .maximum_remaining_stack
                        .max(stacker::remaining_stack().expect("expression contract stack bounds"));
                }
                scope
                    .state
                    .check_expression_machine(node, mode, SharedFrames { base })
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            scope
                .state
                .check_expression_machine(node, mode, SharedFrames { base })
        }
    }

    fn check_expression_machine(
        &mut self,
        node: NodeId,
        mode: u32,
        mut frames: impl FrameStack,
    ) -> Result<TypeId, Error> {
        // Only a suspended parent needs heap storage. A scalar or a semantic
        // callback keeps the same local entry/exit record as native recursion.
        // Declared after `frames`, so unwind drops its trace before the parents.
        let mut active = Some(self.enter_expression_frame(node, mode)?);
        let result = (|| {
            let mut value = None;
            while let Some(frame) = active.as_ref() {
                let node = frame.node;
                let ty = match frame.step {
                    Step::Enter => {
                        let read = self.node(node)?;
                        match read.kind().known() {
                            Some(K::BinaryExpression) => {
                                let data = read
                                    .data_source()
                                    .as_binary_expression()
                                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                                let left = data.left().ok_or(Error::MissingLink("binary left"))?;
                                let right =
                                    data.right().ok_or(Error::MissingLink("binary right"))?;
                                let token = data
                                    .operator_token()
                                    .ok_or(Error::MissingLink("binary operator"))?;
                                let destructuring = self.node(token)?.kind() == K::EqualsToken
                                    && matches!(
                                        self.node(left)?.kind().known(),
                                        Some(
                                            K::ObjectLiteralExpression | K::ArrayLiteralExpression
                                        )
                                    );
                                if destructuring {
                                    // The ordinary worker checks only the RHS before
                                    // interpreting the LHS as an assignment pattern.
                                    self.check_binary_expression(node)?
                                } else {
                                    active.as_mut().expect("active frame").step =
                                        Step::Left { left, right, token };
                                    frames.push(self, active.take().expect("suspended parent"));
                                    active = Some(self.enter_expression_frame(left, mode)?);
                                    continue;
                                }
                            }
                            Some(K::ParenthesizedExpression) => {
                                let inner = read
                                    .expression()
                                    .ok_or(Error::MissingLink("parenthesized expression"))?;
                                active.as_mut().expect("active frame").step = Step::Parenthesized;
                                frames.push(self, active.take().expect("suspended parent"));
                                active = Some(self.enter_expression_frame(inner, mode)?);
                                continue;
                            }
                            _ => self.check_expression_worker(node)?,
                        }
                    }
                    Step::Left { left, right, token } => {
                        let ty = value.take().expect("completed left expression");
                        active.as_mut().expect("active frame").step = Step::Right {
                            left,
                            right,
                            token,
                            ty,
                        };
                        frames.push(self, active.take().expect("suspended parent"));
                        active = Some(self.enter_expression_frame(right, mode)?);
                        continue;
                    }
                    Step::Right {
                        left,
                        right,
                        token,
                        ty,
                    } => {
                        let rhs = value.take().expect("completed right expression");
                        self.check_binary_operands(node, left, right, token, ty, rhs)?
                    }
                    Step::Parenthesized => {
                        value.take().expect("completed parenthesized expression")
                    }
                };
                // Each parenthesized and binary node gets its own postchecks;
                // eliminating a syntax wrapper here would change inference.
                let ty = self.instantiate_single_generic_function(node, ty, mode)?;
                if self.const_enum_object_type(ty)? {
                    self.check_const_enum_access(node, ty)?;
                }
                let frame = active.take().expect("completed frame");
                self.leave_expression_frame(frame);
                value = Some(ty);
                active = frames.pop(self);
            }
            Ok(value.expect("root expression completed"))
        })();
        // Recoverable errors restore the same state as every recursive frame.
        // Panics retire the operation; Frames still unwinds its trace scopes.
        if let Some(frame) = active.take() {
            self.leave_expression_frame(frame);
        }
        while let Some(frame) = frames.pop(self) {
            self.leave_expression_frame(frame);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryTraceSink, TraceValue, Tracer};
    use std::sync::Arc;
    use tsr_ast::FactoryMethods;

    #[test]
    #[cfg(feature = "recursion-probe")]
    fn nested_failure_restores_expression_state_and_closes_inner_traces_first() {
        let (_counters, _generation, _identity, owner) = crate::tests::owner();
        let mut operation = owner.operation().unwrap();
        let state = operation.state_mut();
        // A malformed synthetic wrapper is a private failure injection, not a
        // claim that the parser publishes missing child edges.
        let missing = state.factory.new_parenthesized_expression(None);
        let wrapped = state.factory.new_parenthesized_expression(Some(missing));
        let token = state.factory.new_token(K::PlusToken.into());
        let right = state.factory.new_token(K::TrueKeyword.into());
        let root = state.factory.new_binary_expression(
            None,
            Some(wrapped),
            None,
            Some(token),
            Some(right),
        );
        let sink = Arc::new(MemoryTraceSink::new(false));
        state.tracer = Some(Tracer::new(sink.clone(), 0));
        state.current_node = Some(right);
        state.expression_mode = 7;
        #[cfg(feature = "recursion-probe")]
        {
            state.relations.recursion_probe = Some(crate::relater::RecursionProbe::default());
        }
        assert_eq!(
            state.check_expression_iterative(root, 0),
            Err(Error::MissingLink("parenthesized expression"))
        );
        assert_eq!(state.current_node, Some(right));
        assert_eq!(state.expression_mode, 7);
        #[cfg(feature = "recursion-probe")]
        {
            let probe = state.relations.recursion_probe.as_ref().unwrap();
            assert_eq!(probe.expression_depth, 0);
            assert_eq!(probe.maximum_expression_depth, 3);
        }
        let kinds: Vec<_> = sink
            .events()
            .into_iter()
            .filter(|event| event.name == "checkExpression")
            .map(|event| event.args["kind"].clone())
            .collect();
        assert_eq!(
            kinds,
            [
                TraceValue::Int(K::ParenthesizedExpression as i64),
                TraceValue::Int(K::ParenthesizedExpression as i64),
                TraceValue::Int(K::BinaryExpression as i64),
            ]
        );
        assert_eq!(
            state.check_expression_iterative(right, 0),
            Ok(state.builtins.true_type)
        );
        assert_eq!(state.current_node, Some(right));
        assert_eq!(state.expression_mode, 7);
    }

    #[test]
    #[cfg(feature = "operation-experiments")]
    fn shared_frames_preserve_nested_callback_order_and_reuse_capacity() {
        let run = |explicit| {
            let (_counters, _generation, _identity, owner) = crate::tests::owner();
            let mut operation = owner.operation().unwrap();
            let state = operation.state_mut();
            let leaf = state.factory.new_token(K::TrueKeyword.into());
            let inner = state.factory.new_parenthesized_expression(Some(leaf));
            // typeof remains in the semantic worker: it reenters the machine
            // while the outer parentheses and typeof frames are still active.
            let callback = state.factory.new_type_of_expression(Some(inner));
            let root = state.factory.new_parenthesized_expression(Some(callback));
            let sink = Arc::new(MemoryTraceSink::new(false));
            state.tracer = Some(Tracer::new(sink.clone(), 0));
            state.current_node = Some(leaf);
            state.expression_mode = 7;
            state.argument_explicit_execution = explicit;
            let result = state.check_expression_ex(root, 1).unwrap();
            assert_eq!(state.current_node, Some(leaf));
            assert_eq!(state.expression_mode, 7);
            assert_eq!(state.expression_execution_frames.len(), 0);
            let capacity = state.expression_execution_frames.0.capacity();
            assert_eq!(state.check_expression_ex(root, 1).unwrap(), result);
            assert_eq!(state.expression_execution_frames.0.capacity(), capacity);
            let kinds: Vec<_> = sink
                .events()
                .into_iter()
                .filter(|event| event.name == "checkExpression")
                .map(|event| event.args["kind"].clone())
                .collect();
            (state.types.flags(result).unwrap(), state.types.len(), kinds)
        };
        assert_eq!(run(false), run(true));
    }

    #[test]
    #[cfg(feature = "operation-experiments")]
    fn scalar_and_semantic_callback_entries_do_not_allocate_continuations() {
        let (_counters, _generation, _identity, owner) = crate::tests::owner();
        let mut operation = owner.operation().unwrap();
        let state = operation.state_mut();
        let leaf = state.factory.new_token(K::TrueKeyword.into());
        state.argument_explicit_execution = true;
        assert_eq!(
            state.check_expression_ex(leaf, 0),
            Ok(state.builtins.true_type)
        );
        let mut root = leaf;
        for _ in 0..256 {
            root = state.factory.new_type_of_expression(Some(root));
        }
        assert_eq!(
            state.check_expression_ex(root, 0),
            Ok(state.typeof_result_type())
        );
        assert_eq!(state.expression_execution_frames.len(), 0);
        assert_eq!(state.expression_execution_frames.capacity(), 0);
        assert_eq!(state.expression_execution_frames.retained_bytes(), 0);
        assert!(state.current_node.is_none());
        assert_eq!(state.expression_mode, 0);
    }

    #[test]
    #[cfg(feature = "operation-experiments")]
    fn shared_frames_restore_nested_callback_errors_before_outer_frames() {
        let (_counters, _generation, _identity, owner) = crate::tests::owner();
        let mut operation = owner.operation().unwrap();
        let state = operation.state_mut();
        let missing = state.factory.new_parenthesized_expression(None);
        let callback = state.factory.new_type_of_expression(Some(missing));
        let root = state.factory.new_parenthesized_expression(Some(callback));
        let previous = state.factory.new_token(K::FalseKeyword.into());
        let sink = Arc::new(MemoryTraceSink::new(false));
        state.tracer = Some(Tracer::new(sink.clone(), 0));
        state.current_node = Some(previous);
        state.expression_mode = 7;
        state.argument_explicit_execution = true;
        #[cfg(feature = "recursion-probe")]
        {
            state.relations.recursion_probe = Some(crate::relater::RecursionProbe::default());
        }
        assert_eq!(
            state.check_expression_ex(root, 1),
            Err(Error::MissingLink("parenthesized expression"))
        );
        assert_eq!(state.current_node, Some(previous));
        assert_eq!(state.expression_mode, 7);
        assert_eq!(state.expression_execution_frames.len(), 0);
        #[cfg(feature = "recursion-probe")]
        {
            let probe = state.relations.recursion_probe.as_ref().unwrap();
            assert_eq!(probe.expression_depth, 0);
            assert_eq!(probe.maximum_expression_depth, 3);
        }
        let kinds: Vec<_> = sink
            .events()
            .into_iter()
            .filter(|event| event.name == "checkExpression")
            .map(|event| event.args["kind"].clone())
            .collect();
        assert_eq!(
            kinds,
            [
                TraceValue::Int(K::ParenthesizedExpression as i64),
                TraceValue::Int(K::TypeOfExpression as i64),
                TraceValue::Int(K::ParenthesizedExpression as i64),
            ]
        );
        assert_eq!(
            state.check_expression_ex(previous, 1),
            Ok(state.builtins.false_type)
        );
    }

    #[test]
    #[cfg(feature = "operation-experiments")]
    fn shared_frame_panic_closes_scopes_and_retires_generation() {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        let (_counters, generation, _identity, owner) = crate::tests::owner();
        let sink = Arc::new(MemoryTraceSink::new(false));
        assert!(catch_unwind(AssertUnwindSafe(|| {
            let mut operation = owner.operation().unwrap();
            let state = operation.state_mut();
            let outer = state.factory.new_token(K::TrueKeyword.into());
            let inner = state.factory.new_token(K::FalseKeyword.into());
            state.tracer = Some(Tracer::new(sink.clone(), 0));
            let scope = ExecutionScope { state, base: 0 };
            let outer = scope.state.enter_expression_frame(outer, 1).unwrap();
            scope.state.expression_execution_frames.0.push(outer);
            {
                let nested = ExecutionScope {
                    state: scope.state,
                    base: 1,
                };
                let inner = nested.state.enter_expression_frame(inner, 2).unwrap();
                nested.state.expression_execution_frames.0.push(inner);
                panic!("injected expression callback panic");
            }
        }))
        .is_err());
        assert_eq!(generation.validate(), Err(tsr_arena::Error::Retired));
        let kinds: Vec<_> = sink
            .events()
            .into_iter()
            .filter(|event| event.name == "checkExpression")
            .map(|event| event.args["kind"].clone())
            .collect();
        assert_eq!(
            kinds,
            [
                TraceValue::Int(K::FalseKeyword as i64),
                TraceValue::Int(K::TrueKeyword as i64)
            ]
        );
    }

    #[test]
    #[cfg(feature = "operation-experiments")]
    fn shared_frames_check_twenty_thousand_wrappers_on_a_small_stack() {
        std::thread::Builder::new()
            .stack_size(512 * 1024)
            .spawn(|| {
                let (_counters, _generation, _identity, owner) = crate::tests::owner();
                let mut operation = owner.operation().unwrap();
                let state = operation.state_mut();
                let leaf = state.factory.new_token(K::TrueKeyword.into());
                let mut root = leaf;
                for _ in 0..20_000 {
                    root = state.factory.new_parenthesized_expression(Some(root));
                }
                state.argument_explicit_execution = true;
                assert_eq!(
                    state.check_expression_ex(root, 0),
                    Ok(state.builtins.true_type)
                );
                assert_eq!(state.expression_execution_frames.len(), 0);
                assert!(state.current_node.is_none());
                assert_eq!(state.expression_mode, 0);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    #[cfg(feature = "operation-experiments")]
    fn shared_frames_survive_deep_machine_reentry_on_a_small_stack() {
        const STACK: usize = 512 * 1024;
        std::thread::Builder::new()
            .stack_size(STACK)
            .spawn(|| {
                let (_counters, _generation, _identity, owner) = crate::tests::owner();
                let mut operation = owner.operation().unwrap();
                let state = operation.state_mut();
                let leaf = state.factory.new_token(K::TrueKeyword.into());
                let mut root = leaf;
                for _ in 0..4096 {
                    root = state.factory.new_parenthesized_expression(Some(root));
                    root = state.factory.new_type_of_expression(Some(root));
                }
                state.argument_explicit_execution = true;
                #[cfg(feature = "recursion-probe")]
                {
                    state.relations.recursion_probe =
                        Some(crate::relater::RecursionProbe::default());
                }
                let result = state.check_expression_ex(root, 0).unwrap();
                assert_eq!(result, state.typeof_result_type());
                assert_eq!(state.expression_execution_frames.len(), 0);
                assert!(state.current_node.is_none());
                assert_eq!(state.expression_mode, 0);
                // Every parenthesis suspends; typeof and the leaf stay local.
                assert!(state.expression_execution_frames.capacity() >= 4096);
                assert_eq!(
                    state.expression_execution_frames.retained_bytes(),
                    state.expression_execution_frames.capacity() * std::mem::size_of::<Frame>(),
                );
                #[cfg(feature = "recursion-probe")]
                {
                    let probe = state.relations.recursion_probe.as_ref().unwrap();
                    assert_eq!(probe.expression_depth, 0);
                    assert_eq!(probe.maximum_expression_depth, 8193);
                    assert!(probe.maximum_remaining_stack > STACK);
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
