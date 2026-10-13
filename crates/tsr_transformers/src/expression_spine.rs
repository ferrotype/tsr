//! Heap continuations for expression kinds handled by a transform's default
//! visit-each-child branch. Only opt in when binary and transparent nodes have
//! no transformer-specific pre/post effects and callbacks cannot return lists.
//! Its EmitVisitorHooks must not override generic node or modifier visits;
//! such role hooks cannot be bypassed by directly scheduling the spine.
//! Other children retain NodeVisitor's normal callbacks and environment hooks.
use crate::transformer::Failure;
use tsr_ast::{FactoryMethods, NodeId, NodeVisitor, SyntaxKind as K};

enum ExpressionFrame {
    Transparent(NodeId, K),
    BinaryLeft(NodeId, tsr_ast::BinaryExpressionData),
    BinaryRight(NodeId, tsr_ast::BinaryExpressionData),
}

pub(crate) fn visit_expression_spine(
    failure: &Failure,
    visitor: &mut NodeVisitor<'_>,
    root: NodeId,
) -> Option<NodeId> {
    let mut frames = Vec::new();
    let mut next = Some(root);
    let mut value;
    loop {
        if let Some(id) = next.take() {
            if failure.is_set() {
                // The recursive visitor still enters the pending child's
                // callback. Its failure guard returns without descending.
                value = visitor.visit_node(Some(id));
            } else {
                let read = visitor.factory().node(id);
                match read.kind().known() {
                    Some(K::BinaryExpression) => {
                        let binary = read
                            .as_binary_expression()
                            .expect("operation requires BinaryExpression payload");
                        let mut data = tsr_ast::BinaryExpressionData {
                            modifiers: binary.modifiers(),
                            left: binary.left(),
                            r#type: binary.r#type(),
                            operator_token: binary.operator_token(),
                            right: binary.right(),
                        };
                        drop(read);
                        data.modifiers = visitor.visit_modifiers(data.modifiers);
                        next = data.left;
                        frames.push(ExpressionFrame::BinaryLeft(id, data));
                        if next.is_some() {
                            continue;
                        }
                        value = None;
                    }
                    Some(kind @ (K::ParenthesizedExpression | K::PartiallyEmittedExpression)) => {
                        next = read.expression();
                        drop(read);
                        frames.push(ExpressionFrame::Transparent(id, kind));
                        if next.is_some() {
                            continue;
                        }
                        value = None;
                    }
                    _ => {
                        drop(read);
                        value = visitor.visit_node(Some(id));
                    }
                }
            }
        } else {
            value = None;
        }
        loop {
            match frames.pop() {
                None => return value,
                Some(ExpressionFrame::Transparent(id, kind)) => {
                    value = Some(if kind == K::ParenthesizedExpression {
                        visitor
                            .factory_mut()
                            .update_parenthesized_expression(id, value)
                    } else {
                        visitor
                            .factory_mut()
                            .update_partially_emitted_expression(id, value)
                    });
                }
                Some(ExpressionFrame::BinaryLeft(id, mut data)) => {
                    data.left = value;
                    data.r#type = visitor.visit_node(data.r#type);
                    data.operator_token = visitor.visit_node(data.operator_token);
                    next = data.right;
                    frames.push(ExpressionFrame::BinaryRight(id, data));
                    if next.is_some() {
                        break;
                    }
                    value = None;
                }
                Some(ExpressionFrame::BinaryRight(id, mut data)) => {
                    data.right = value;
                    value = Some(visitor.factory_mut().update_binary_expression(
                        id,
                        data.modifiers,
                        data.left,
                        data.r#type,
                        data.operator_token,
                        data.right,
                    ));
                }
            }
        }
    }
}
