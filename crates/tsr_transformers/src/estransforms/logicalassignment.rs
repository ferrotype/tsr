//! `transformers/estransforms/logicalassignment.go`: `||=`, `&&=` and `??=`
//! to a logical expression over a parenthesized assignment.
use super::utilities::{is_simple_copiable_expression, skip_parentheses, subtree_facts};
use crate::transformer::{Failure, TransformOptions, Transformer};
use std::cell::RefCell;
use std::rc::Rc;
use tsr_ast::{node_flags, subtree_flags, FactoryMethods, NodeId, NodeVisitor, SyntaxKind as K};
use tsr_printer::EmitContext;

struct LogicalAssignmentTransformer {
    emit_context: RefCell<EmitContext>,
    failure: Failure,
}

impl LogicalAssignmentTransformer {
    // port: tsc/internal/transformers/estransforms/logicalassignment.go:logicalAssignmentTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        if self.failure.is_set() {
            return node;
        }
        let id = node.expect("runtime error: invalid memory address or nil pointer dereference");
        let Some(facts) = self.failure.ok(subtree_facts(visitor.factory(), id)) else {
            return node;
        };
        if facts & subtree_flags::LOGICAL_ASSIGNMENTS == 0 {
            return node;
        }
        match visitor.factory().node(id).kind().known() {
            Some(K::BinaryExpression) => self.visit_binary_expression(visitor, id),
            _ => visitor.visit_each_child(node),
        }
    }

    // port: tsc/internal/transformers/estransforms/logicalassignment.go:logicalAssignmentTransformer.visitBinaryExpression
    fn visit_binary_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> Option<NodeId> {
        let (node_left, operator, node_right) = {
            let read = visitor.factory().node(node);
            let data = read
                .as_binary_expression()
                .expect("BinaryExpression payload");
            (data.left(), data.operator_token(), data.right())
        };
        let operator = visitor.factory().node(nil_checked(operator)).kind();
        let non_assignment_operator = match operator.known() {
            Some(K::BarBarEqualsToken) => K::BarBarToken,
            Some(K::AmpersandAmpersandEqualsToken) => K::AmpersandAmpersandToken,
            Some(K::QuestionQuestionEqualsToken) => K::QuestionQuestionToken,
            _ => return visitor.visit_each_child(Some(node)),
        };

        let visited = nil_checked(visitor.visit_node(node_left));
        let Some(mut left) = self
            .failure
            .ok(skip_parentheses(visitor.factory(), visited))
        else {
            return Some(node);
        };
        let mut assignment_target = left;
        let visited = nil_checked(visitor.visit_node(node_right));
        let Some(right) = self
            .failure
            .ok(skip_parentheses(visitor.factory(), visited))
        else {
            return Some(node);
        };

        let mut emit_context = self.emit_context.borrow_mut();
        let factory = visitor.factory_mut();
        if tsr_ast::utilities::is_access_expression(&factory.node(left)) {
            let left_expression = nil_checked(factory.node(left).expression());
            let property_access_target_simple_copiable =
                is_simple_copiable_expression(factory, left_expression);
            let mut property_access_target = left_expression;
            let mut property_access_target_assignment = left_expression;
            if !property_access_target_simple_copiable {
                property_access_target = emit_context.new_temp_variable(factory);
                emit_context.add_variable_declaration(factory, property_access_target);
                property_access_target_assignment = emit_context.new_assignment_expression(
                    factory,
                    property_access_target,
                    left_expression,
                );
            }

            if tsr_ast::is_property_access_expression(&factory.node(left)) {
                let name = factory.node(left).name();
                assignment_target = factory.new_property_access_expression(
                    Some(property_access_target),
                    None,
                    name,
                    node_flags::NONE,
                );
                left = factory.new_property_access_expression(
                    Some(property_access_target_assignment),
                    None,
                    name,
                    node_flags::NONE,
                );
            } else {
                let argument_expression = nil_checked(
                    factory
                        .node(left)
                        .as_element_access_expression()
                        .expect("ElementAccessExpression payload")
                        .argument_expression(),
                );
                let element_access_argument_simple_copiable =
                    is_simple_copiable_expression(factory, argument_expression);
                let mut element_access_argument = argument_expression;
                let mut argument_expr = element_access_argument;
                if !element_access_argument_simple_copiable {
                    element_access_argument = emit_context.new_temp_variable(factory);
                    emit_context.add_variable_declaration(factory, element_access_argument);
                    argument_expr = emit_context.new_assignment_expression(
                        factory,
                        element_access_argument,
                        argument_expression,
                    );
                }

                assignment_target = factory.new_element_access_expression(
                    Some(property_access_target),
                    None,
                    Some(element_access_argument),
                    node_flags::NONE,
                );
                left = factory.new_element_access_expression(
                    Some(property_access_target_assignment),
                    None,
                    Some(argument_expr),
                    node_flags::NONE,
                );
            }
        }

        let token = factory.new_token(non_assignment_operator.into());
        let assignment = emit_context.new_assignment_expression(factory, assignment_target, right);
        let parenthesized = factory.new_parenthesized_expression(Some(assignment));
        Some(factory.new_binary_expression(
            None,
            Some(left),
            None,
            Some(token),
            Some(parenthesized),
        ))
    }
}

fn nil_checked(node: Option<NodeId>) -> NodeId {
    node.expect("runtime error: invalid memory address or nil pointer dereference")
}

// port: tsc/internal/transformers/estransforms/logicalassignment.go:newLogicalAssignmentTransformer
pub fn new_logical_assignment_transformer<'a>(
    opts: &TransformOptions<'a>,
) -> Option<Transformer<'a>> {
    let tx = Rc::new(LogicalAssignmentTransformer {
        emit_context: RefCell::new(opts.context.clone()),
        failure: opts.failure.clone(),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| tx.visit(visitor, node),
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}
