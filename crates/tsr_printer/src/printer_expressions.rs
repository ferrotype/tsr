//! Expression forms retained in generated parameter and computed-property names.
use super::{comments::CommentTarget, guard, statements::Operand, Session, Span, WriteKind};
use crate::{emit_flags as ef, list_format as lf, Error};
use tsr_ast::{operator_precedence as op, NodeId, NodeKind, SyntaxKind as K};

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
        } else if tsr_ast::is_assignment_operator(kind) {
            op::ASSIGNMENT
        } else {
            tsr_ast::get_binary_operator_precedence(kind)
        })
    }

    // port: tsc/internal/printer/printer.go:Printer.emitArgument
    pub(super) fn emit_argument(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_expression(node, op::SPREAD)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitCallee
    pub(super) fn emit_callee(&mut self, callee: NodeId, parent: NodeId) -> Result<(), Error> {
        if self.should_emit_indirect_call(parent) {
            self.write_punctuation(b"(");
            self.write_literal(b"0");
            self.write_punctuation(b",");
            self.write_space();
            self.emit_expression(callee, op::COMMA)?;
            self.write_punctuation(b")");
            return Ok(());
        }
        let parent_read = self.node(parent)?;
        let skipped =
            self.skip_partially_emitted_expressions_rewritten(self.left_operand(callee))?;
        if parent_read.kind() == K::CallExpression
            && skipped.map_or(Ok(false), |skipped| {
                self.is_new_expression_without_arguments(skipped.node)
            })?
        {
            // Parenthesize `new C` inside of a CallExpression so it is treated as `(new C)()` and not `new C()`
            return self.emit_expression(callee, op::PARENTHESES);
        }
        let precedence = if tsr_ast::utilities::is_optional_chain(&parent_read) {
            op::OPTIONAL_CHAIN
        } else {
            op::MEMBER
        };
        self.emit_expression(callee, precedence)
    }

    // port: tsc/internal/printer/utilities.go:isNewExpressionWithoutArguments
    fn is_new_expression_without_arguments(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.node(node)?;
        Ok(read.kind() == K::NewExpression && read.argument_list().is_none())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitCallExpression
    pub(super) fn emit_call_expression(&mut self, node: NodeId) -> Result<(), Error> {
        let state = self.enter_node(node)?;
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
        self.exit_node(node, state)?;
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
        let state = self.enter_node(node)?;
        let read = self.node(node)?;
        let expression = read
            .data_source()
            .as_parenthesized_expression()
            .and_then(|n| n.expression())
            .ok_or(Error::MissingNode("parenthesized expression"))?;
        self.emit_parenthesized_expression_parts(Some(node), Span::of(&read), expression)?;
        self.exit_node(node, state)?;
        Ok(())
    }

    /// The writes of `emitParenthesizedExpression` between entering and
    /// leaving the node; `node` is `None` for the parentheses upstream creates
    /// around an arrow function's object-literal body.
    pub(super) fn emit_parenthesized_expression_parts(
        &mut self,
        node: Option<NodeId>,
        span: Span,
        expression: NodeId,
    ) -> Result<(), Error> {
        // Upstream closes at the open paren's end only for a nil expression,
        // which this tree cannot hold.
        self.emit_token_ex(
            K::OpenParenToken,
            span.pos,
            WriteKind::Punctuation,
            node,
            super::tef::NONE,
        )?;
        let indented = self.write_line_separators_and_indent_before(expression, span)?;
        self.emit_expression(expression, op::COMMA)?;
        self.write_line_separators_after(expression, span)?;
        self.decrease_indent_if(indented);
        let close_paren_pos = i64::from(self.node(expression)?.end());
        self.emit_token_ex(
            K::CloseParenToken,
            close_paren_pos,
            WriteKind::Punctuation,
            node,
            super::tef::NONE,
        )?;
        Ok(())
    }

    /// `emitExpression` of the `ParenthesizedExpression` upstream's
    /// `emitConciseBody` creates around an object-literal body, with the body's
    /// range. It is never part of the tree: it has no identity, no emit flags,
    /// and its parse node is nil, so its tokens carry no comments.
    pub(super) fn emit_created_parenthesized_expression(
        &mut self,
        expression: NodeId,
    ) -> Result<(), Error> {
        let read = self.node(expression)?;
        let span = Span::created(i64::from(read.pos()), i64::from(read.end()));
        let target = CommentTarget {
            node: None,
            kind: K::ParenthesizedExpression.into(),
            emit_flags: ef::NONE,
            comment_range: span.range(),
            source_map_range: span.range(),
        };
        guard(|| {
            let state = self.enter_created_node(&target)?;
            self.emit_parenthesized_expression_parts(None, span, expression)?;
            self.exit_created_node(&target, state)?;
            Ok(())
        })
    }

    fn binary_operator(&self, operand: Operand) -> Result<Option<NodeKind>, Error> {
        let Some(operand) = self.skip_partially_emitted_expressions_rewritten(operand)? else {
            return Ok(None);
        };
        if self.node(operand.node)?.kind() != K::BinaryExpression {
            return Ok(None);
        }
        let (_, operator, _) = self.binary_parts(operand.node)?;
        Ok(Some(self.node(operator)?.kind()))
    }

    // port: tsc/internal/printer/printer.go:Printer.getLiteralKindOfBinaryPlusOperand
    fn literal_kind_of_binary_plus_operand(
        &self,
        operand: Operand,
    ) -> Result<Option<NodeKind>, Error> {
        let mut pending = vec![operand];
        let mut literal = None;
        while let Some(operand) = pending.pop() {
            let Some(operand) = self.skip_partially_emitted_expressions_rewritten(operand)? else {
                return Ok(None);
            };
            let kind = self.node(operand.node)?.kind();
            if tsr_ast::is_literal_kind(kind) {
                if literal.is_some_and(|previous| previous != kind) {
                    return Ok(None);
                }
                literal = Some(kind);
            } else if self.binary_operator(operand)? == Some(K::PlusToken.into()) {
                let (left, _, right) = self.binary_parts(operand.node)?;
                pending.push(Operand::plain(right));
                pending.push(self.operand_left(operand, left));
            } else {
                return Ok(None);
            }
        }
        Ok(literal)
    }

    // port: tsc/internal/printer/printer.go:Printer.getBinaryExpressionPrecedence
    fn binary_operand_precedences(
        &self,
        left: Operand,
        operator: NodeId,
        right: Operand,
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
        let (left, operator, right) = self.binary_parts(node)?;
        let (left_operand, right_operand) = (self.left_operand(left), Operand::plain(right));
        let (mut left_prec, mut right_prec) =
            self.binary_operand_precedences(left_operand, operator, right_operand)?;
        let outer = self.node(operator)?.kind();
        for (operand, precedence) in [
            (left_operand, &mut left_prec),
            (right_operand, &mut right_prec),
        ] {
            let Some(skipped) = self.skip_partially_emitted_expressions_rewritten(operand)? else {
                continue;
            };
            if tsr_ast::utilities::node_is_synthesized(&self.node(skipped.node)?) {
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
        let state = self.enter_node(node)?;
        self.emit_expression(left, left_prec)?;
        let before = self.get_lines_between_nodes(
            node,
            self.operand_span(left_operand)?,
            Span::of(&self.node(operator)?),
        )?;
        let after = self.get_lines_between_nodes(
            node,
            Span::of(&self.node(operator)?),
            Span::of(&self.node(right)?),
        )?;
        self.write_lines_and_indent(before, outer != K::CommaToken);
        self.emit_token_node_ex(Some(operator), super::tef::NO_SOURCE_MAPS)?;
        // Binary operators should have a space before the comment starts
        self.write_lines_and_indent(after, true);
        self.emit_expression(right, right_prec)?;
        self.decrease_indent_if(after > 0);
        self.decrease_indent_if(before > 0);
        self.exit_node(node, state)?;
        Ok(())
    }
}
