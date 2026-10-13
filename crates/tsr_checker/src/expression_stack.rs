//! WebAssembly cannot grow its engine call stack. Binary and parenthesized
//! expression continuations carry the same entry/exit state as checkExpressionEx
//! on the heap; operator semantics remain in the ordinary checker worker.
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
struct Frames(Vec<Frame>);

impl Drop for Frames {
    fn drop(&mut self) {
        // An unwind must end trace scopes innermost first too. Vec's ordinary
        // element destruction order is not the recursive call stack's order.
        while self.0.pop().is_some() {}
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

    pub(crate) fn check_expression_iterative(
        &mut self,
        node: NodeId,
        mode: u32,
    ) -> Result<TypeId, Error> {
        let mut frames = Frames::default();
        frames.0.push(self.enter_expression_frame(node, mode)?);
        let result = (|| {
            let mut value = None;
            while let Some(frame) = frames.0.last() {
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
                                    frames.0.last_mut().expect("active frame").step =
                                        Step::Left { left, right, token };
                                    frames.0.push(self.enter_expression_frame(left, mode)?);
                                    continue;
                                }
                            }
                            Some(K::ParenthesizedExpression) => {
                                let inner = read
                                    .expression()
                                    .ok_or(Error::MissingLink("parenthesized expression"))?;
                                frames.0.last_mut().expect("active frame").step =
                                    Step::Parenthesized;
                                frames.0.push(self.enter_expression_frame(inner, mode)?);
                                continue;
                            }
                            _ => self.check_expression_worker(node)?,
                        }
                    }
                    Step::Left { left, right, token } => {
                        let ty = value.take().expect("completed left expression");
                        frames.0.last_mut().expect("active frame").step = Step::Right {
                            left,
                            right,
                            token,
                            ty,
                        };
                        frames.0.push(self.enter_expression_frame(right, mode)?);
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
                self.leave_expression_frame(frames.0.pop().expect("completed frame"));
                value = Some(ty);
            }
            Ok(value.expect("root expression completed"))
        })();
        // Recoverable errors restore the same state as every recursive frame.
        // Panics retire the operation; Frames still unwinds its trace scopes.
        while let Some(frame) = frames.0.pop() {
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
        state.relations.recursion_probe = Some(crate::relater::RecursionProbe::default());
        assert_eq!(
            state.check_expression_iterative(root, 0),
            Err(Error::MissingLink("parenthesized expression"))
        );
        assert_eq!(state.current_node, Some(right));
        assert_eq!(state.expression_mode, 7);
        let probe = state.relations.recursion_probe.as_ref().unwrap();
        assert_eq!(probe.expression_depth, 0);
        assert_eq!(probe.maximum_expression_depth, 3);
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
}
