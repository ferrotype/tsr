//! Expression forms retained in generated parameter and computed-property names.
use super::{greatest_end, Session, Span, WriteKind};
use crate::{emit_flags as ef, list_format as lf, Error};
use ts_ast::{operator_precedence as op, NodeId, NodeKind, SyntaxKind as K};

impl Session<'_, '_> {
    pub(super) fn binary_parts(&self, node: NodeId) -> Result<(NodeId, NodeId, NodeId), Error> {
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_binary_expression()
            .ok_or(Error::MissingNode("binary payload"))?;
        Ok((
            data.left().ok_or(Error::MissingNode("binary left"))?,
            data.operator_token()
                .ok_or(Error::MissingNode("binary operator"))?,
            data.right().ok_or(Error::MissingNode("binary right"))?,
        ))
    }

    pub(super) fn binary_precedence(&self, operator: NodeId) -> Result<i32, Error> {
        let kind = self.node(operator)?.kind();
        Ok(if kind == K::CommaToken {
            op::COMMA
        } else if ts_ast::is_assignment_operator(kind) {
            op::ASSIGNMENT
        } else {
            ts_ast::get_binary_operator_precedence(kind)
        })
    }

    // port: tsc/internal/printer/printer.go:Printer.emitArgument
    pub(super) fn emit_argument(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_expression(node, op::SPREAD)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitCallee
    pub(super) fn emit_callee(&mut self, callee: NodeId, parent: NodeId) -> Result<(), Error> {
        if self.emit_flags(parent) & ef::INDIRECT_CALL != 0 {
            self.write_punctuation(b"(");
            self.writer.write_literal(b"0");
            self.write_punctuation(b",");
            self.write_space();
            self.emit_expression(callee, op::COMMA)?;
            self.write_punctuation(b")");
            return Ok(());
        }
        let skipped = ts_ast::skip_partially_emitted_expressions(self.view, callee)?;
        let read = self.node(skipped)?;
        let needs_parens = read.kind() == K::NewExpression
            && read
                .data_source()
                .as_new_expression()
                .is_some_and(|n| n.arguments().is_none());
        let precedence = if needs_parens {
            op::PARENTHESES
        } else if ts_ast::utilities::is_optional_chain(&self.node(parent)?) {
            op::OPTIONAL_CHAIN
        } else {
            op::MEMBER
        };
        self.emit_expression(callee, precedence)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitCallExpression
    pub(super) fn emit_call_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let call = read
            .data_source()
            .as_call_expression()
            .ok_or(Error::MissingNode("call payload"))?;
        let (callee, question_dot, type_arguments, arguments) = (
            call.expression().ok_or(Error::MissingNode("callee"))?,
            call.question_dot_token(),
            call.type_arguments(),
            call.arguments(),
        );
        self.emit_callee(callee, node)?;
        self.emit_token_node(question_dot)?;
        self.emit_type_arguments(node, type_arguments)?;
        self.emit_list(
            Self::emit_argument,
            node,
            arguments,
            lf::CALL_EXPRESSION_ARGUMENTS,
        )?;
        self.exit_node(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.writeLinesAndIndent
    pub(super) fn write_lines_and_indent(&mut self, count: i64, space: bool) {
        if count > 0 {
            self.increase_indent();
            self.write_line_repeat(count);
        } else if space {
            self.write_space();
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitParenthesizedExpression
    pub(super) fn emit_parenthesized_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let read = self.node(node)?;
        let expression = read
            .data_source()
            .as_parenthesized_expression()
            .and_then(|n| n.expression())
            .ok_or(Error::MissingNode("parenthesized expression"))?;
        self.emit_token(
            K::OpenParenToken,
            i64::from(read.pos()),
            WriteKind::Punctuation,
            node,
        );
        let leading = if self.printer.options.preserve_source_newlines {
            self.get_leading_line_terminator_count(Some(node), Some(expression), lf::NONE)?
        } else {
            0
        };
        self.write_lines_and_indent(leading, false);
        self.emit_expression(expression, op::COMMA)?;
        if self.printer.options.preserve_source_newlines {
            let trailing =
                self.get_closing_line_terminator_count(Some(node), Some(expression), lf::NONE)?;
            self.write_line_repeat(trailing);
        }
        self.decrease_indent_if(leading > 0);
        self.emit_token(
            K::CloseParenToken,
            greatest_end(-1, &[Some(i64::from(self.node(expression)?.end()))]),
            WriteKind::Punctuation,
            node,
        );
        self.exit_node(node);
        Ok(())
    }

    fn binary_operator(&self, node: NodeId) -> Result<Option<NodeKind>, Error> {
        let node = ts_ast::skip_partially_emitted_expressions(self.view, node)?;
        if self.node(node)?.kind() != K::BinaryExpression {
            return Ok(None);
        }
        let (_, operator, _) = self.binary_parts(node)?;
        Ok(Some(self.node(operator)?.kind()))
    }

    // port: tsc/internal/printer/printer.go:Printer.getLiteralKindOfBinaryPlusOperand
    fn literal_kind_of_binary_plus_operand(&self, node: NodeId) -> Result<Option<NodeKind>, Error> {
        let mut pending = vec![node];
        let mut literal = None;
        while let Some(node) = pending.pop() {
            let node = ts_ast::skip_partially_emitted_expressions(self.view, node)?;
            let kind = self.node(node)?.kind();
            if ts_ast::is_literal_kind(kind) {
                if literal.is_some_and(|previous| previous != kind) {
                    return Ok(None);
                }
                literal = Some(kind);
            } else if self.binary_operator(node)? == Some(K::PlusToken.into()) {
                let (left, _, right) = self.binary_parts(node)?;
                pending.push(right);
                pending.push(left);
            } else {
                return Ok(None);
            }
        }
        Ok(literal)
    }

    // port: tsc/internal/printer/printer.go:Printer.getBinaryExpressionPrecedence
    fn binary_operand_precedences(
        &self,
        left: NodeId,
        operator: NodeId,
        right: NodeId,
    ) -> Result<(i32, i32), Error> {
        let kind = self.node(operator)?.kind();
        let precedence = self.binary_precedence(operator)?;
        let (mut left_prec, mut right_prec) = (precedence, precedence);
        match precedence {
            op::COMMA | op::BITWISE_OR | op::BITWISE_XOR | op::BITWISE_AND => {}
            op::ASSIGNMENT => {
                left_prec = op::CONDITIONAL;
                right_prec = op::YIELD;
            }
            op::LOGICAL_OR => right_prec = op::LOGICAL_AND,
            op::LOGICAL_AND => right_prec = op::BITWISE_OR,
            op::EQUALITY => right_prec = op::RELATIONAL,
            op::RELATIONAL => right_prec = op::SHIFT,
            op::SHIFT => right_prec = op::ADDITIVE,
            op::ADDITIVE => {
                let homogeneous = if kind == K::PlusToken
                    && self.binary_operator(right)? == Some(K::PlusToken.into())
                {
                    let literal = self.literal_kind_of_binary_plus_operand(left)?;
                    literal.is_some()
                        && literal == self.literal_kind_of_binary_plus_operand(right)?
                } else {
                    false
                };
                if !homogeneous {
                    right_prec = op::MULTIPLICATIVE;
                }
            }
            op::MULTIPLICATIVE => {
                if kind != K::AsteriskToken
                    || self.binary_operator(right)? != Some(K::AsteriskToken.into())
                {
                    right_prec = op::EXPONENTIATION;
                }
            }
            op::EXPONENTIATION => left_prec = op::UPDATE,
            _ => {
                return Err(Error::UnexpectedKind {
                    context: "binary operator",
                    kind,
                })
            }
        }
        Ok((left_prec, right_prec))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitBinaryExpression
    pub(super) fn emit_binary_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.enter_node(node);
        let (left, operator, right) = self.binary_parts(node)?;
        let (mut left_prec, mut right_prec) =
            self.binary_operand_precedences(left, operator, right)?;
        let outer = self.node(operator)?.kind();
        for (operand, precedence) in [(left, &mut left_prec), (right, &mut right_prec)] {
            let skipped = ts_ast::skip_partially_emitted_expressions(self.view, operand)?;
            if ts_ast::utilities::node_is_synthesized(&self.node(skipped)?) {
                if let Some(inner) = self.binary_operator(skipped)? {
                    // port: tsc/internal/printer/utilities.go:mixingBinaryOperatorsRequiresParentheses
                    let logical = |kind: NodeKind| {
                        matches!(
                            kind.known(),
                            Some(K::AmpersandAmpersandToken | K::BarBarToken)
                        )
                    };
                    if (outer == K::QuestionQuestionToken && logical(inner))
                        || (inner == K::QuestionQuestionToken && logical(outer))
                    {
                        *precedence = op::HIGHEST;
                    }
                }
            }
        }
        self.emit_expression(left, left_prec)?;
        let before = self.get_lines_between_nodes(
            node,
            Span::of(&self.node(left)?),
            Span::of(&self.node(operator)?),
        )?;
        let after = self.get_lines_between_nodes(
            node,
            Span::of(&self.node(operator)?),
            Span::of(&self.node(right)?),
        )?;
        self.write_lines_and_indent(before, outer != K::CommaToken);
        self.emit_token_node(Some(operator))?;
        self.write_lines_and_indent(after, true);
        self.emit_expression(right, right_prec)?;
        self.decrease_indent_if(after > 0);
        self.decrease_indent_if(before > 0);
        self.exit_node(node);
        Ok(())
    }
}
