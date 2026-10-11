//! The expression continuation used on wasm, where stacker cannot grow the
//! engine stack. Binary operand sequencing shares the ordinary printer helpers.
use super::{expressions::BinaryEmission, statements::Operand, Session};
use crate::Error;
use tsr_ast::{NodeId, SyntaxKind as K};

enum Frame {
    Enter(NodeId, i32),
    Finish {
        operand: Operand,
        parens: bool,
        spine: bool,
    },
    Left(NodeId, BinaryEmission),
    Right(NodeId, BinaryEmission, i64, i64),
}

impl Session<'_, '_> {
    pub(super) fn emit_expression_iterative(
        &mut self,
        node: NodeId,
        precedence: i32,
    ) -> Result<(), Error> {
        let depth = self.expression_depth;
        let mut frames = vec![Frame::Enter(node, precedence)];
        let result = (|| {
            while let Some(frame) = frames.pop() {
                match frame {
                    Frame::Enter(node, precedence) => {
                        let operand = self.take_spine_occurrence(node);
                        self.expression_depth += 1;
                        let parens = self.expression_precedence(operand)? < precedence;
                        if parens {
                            self.write_punctuation(b"(");
                        }
                        if let Some(created) = self.no_asi_parens(operand) {
                            self.emit_no_asi_parens(created)?;
                            frames.push(Frame::Finish {
                                operand,
                                parens,
                                spine: false,
                            });
                        } else {
                            self.open_spine(operand);
                            frames.push(Frame::Finish {
                                operand,
                                parens,
                                spine: true,
                            });
                            if self.known_kind(node)? == K::BinaryExpression {
                                let emission = self.begin_binary_expression(node)?;
                                let left = Frame::Enter(emission.left, emission.left_precedence);
                                frames.push(Frame::Left(node, emission));
                                frames.push(left);
                            } else {
                                self.emit_expression_kind(node)?;
                            }
                        }
                    }
                    Frame::Left(node, emission) => {
                        let (before, after) = self.between_binary_operands(node, &emission)?;
                        let right = Frame::Enter(emission.right, emission.right_precedence);
                        frames.push(Frame::Right(node, emission, before, after));
                        frames.push(right);
                    }
                    Frame::Right(node, emission, before, after) => {
                        self.end_binary_expression(node, &emission, before, after)?;
                    }
                    Frame::Finish {
                        operand,
                        parens,
                        spine,
                    } => {
                        if spine {
                            self.close_spine(operand);
                        }
                        if parens {
                            self.write_punctuation(b")");
                        }
                        self.expression_depth -= 1;
                    }
                }
            }
            Ok(())
        })();
        // Recursive emit_expression_worker decrements every entered frame on
        // an error too. Comment/spine state is otherwise left as at failure.
        self.expression_depth = depth;
        result
    }
}
