//! `transformers/estransforms/exponentiation.go`: `**` and `**=` to `Math.pow`.
use super::utilities::subtree_facts;
use crate::transformer::{Failure, TransformOptions, Transformer};
use std::cell::RefCell;
use std::rc::Rc;
use tsr_ast::{node_flags, subtree_flags, FactoryMethods, NodeId, NodeVisitor, SyntaxKind as K};
use tsr_printer::EmitContext;

struct ExponentiationTransformer {
    emit_context: RefCell<EmitContext>,
    failure: Failure,
}

impl ExponentiationTransformer {
    // port: tsc/internal/transformers/estransforms/exponentiation.go:exponentiationTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        if self.failure.is_set() {
            return node;
        }
        let id = node.expect("runtime error: invalid memory address or nil pointer dereference");
        let Some(facts) = self.failure.ok(subtree_facts(visitor.factory(), id)) else {
            return node;
        };
        if facts & subtree_flags::EXPONENTIATION_OPERATOR == 0 {
            return node;
        }
        match visitor.factory().node(id).kind().known() {
            Some(K::BinaryExpression) => Some(self.visit_binary_expression(visitor, id)),
            _ => visitor.visit_each_child(node),
        }
    }

    // port: tsc/internal/transformers/estransforms/exponentiation.go:exponentiationTransformer.visitBinaryExpression
    fn visit_binary_expression(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let operator = binary_operator_kind(visitor, node);
        match operator {
            Some(K::AsteriskAsteriskEqualsToken) => {
                return self.visit_exponentiation_assignment_expression(visitor, node);
            }
            Some(K::AsteriskAsteriskToken) => {
                return self.visit_exponentiation_expression(visitor, node);
            }
            _ => {}
        }
        visitor
            .visit_each_child(Some(node))
            .expect("visited binary expression")
    }

    // port: tsc/internal/transformers/estransforms/exponentiation.go:exponentiationTransformer.visitExponentiationAssignmentExpression
    fn visit_exponentiation_assignment_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (node_left, node_right) = binary_operands(visitor, node);
        let left = visitor
            .visit_node(node_left)
            .expect("runtime error: invalid memory address or nil pointer dereference");
        let right = visitor.visit_node(node_right);
        let mut emit_context = self.emit_context.borrow_mut();
        let factory = visitor.factory_mut();
        let target;
        let value;
        let left_read = factory.node(left);
        if tsr_ast::is_element_access_expression(&left_read) {
            // Transforms `a[x] **= b` into `(_a = a)[_x = x] = Math.pow(_a[_x], b)`
            drop(left_read);
            let expression_temp = emit_context.new_temp_variable(factory);
            emit_context.add_variable_declaration(factory, expression_temp);
            let argument_expression_temp = emit_context.new_temp_variable(factory);
            emit_context.add_variable_declaration(factory, argument_expression_temp);

            let left_expression = nil_checked(factory.node(left).expression());
            let obj_expr =
                emit_context.new_assignment_expression(factory, expression_temp, left_expression);
            let loc = factory.node(left_expression).range();
            factory.set_node_range(obj_expr, loc);
            let argument_expression = nil_checked(
                factory
                    .node(left)
                    .as_element_access_expression()
                    .expect("ElementAccessExpression payload")
                    .argument_expression(),
            );
            let access_expr = emit_context.new_assignment_expression(
                factory,
                argument_expression_temp,
                argument_expression,
            );
            let loc = factory.node(argument_expression).range();
            factory.set_node_range(access_expr, loc);

            target = factory.new_element_access_expression(
                Some(obj_expr),
                None,
                Some(access_expr),
                node_flags::NONE,
            );

            value = factory.new_element_access_expression(
                Some(expression_temp),
                None,
                Some(argument_expression_temp),
                node_flags::NONE,
            );
            let loc = factory.node(left).range();
            factory.set_node_range(value, loc);
        } else if tsr_ast::is_property_access_expression(&left_read) {
            // Transforms `a.x **= b` into `(_a = a).x = Math.pow(_a.x, b)`
            drop(left_read);
            let expression_temp = emit_context.new_temp_variable(factory);
            emit_context.add_variable_declaration(factory, expression_temp);
            let left_expression = nil_checked(factory.node(left).expression());
            let assignment =
                emit_context.new_assignment_expression(factory, expression_temp, left_expression);
            let loc = factory.node(left_expression).range();
            factory.set_node_range(assignment, loc);
            let name = factory.node(left).name();
            target = factory.new_property_access_expression(
                Some(assignment),
                None,
                name,
                node_flags::NONE,
            );
            let loc = factory.node(left).range();
            factory.set_node_range(target, loc);

            let name = factory.node(left).name();
            value = factory.new_property_access_expression(
                Some(expression_temp),
                None,
                name,
                node_flags::NONE,
            );
            let loc = factory.node(left).range();
            factory.set_node_range(value, loc);
        } else {
            // Transforms `a **= b` into `a = Math.pow(a, b)`
            drop(left_read);
            target = left;
            value = left;
        }

        let mut arguments = vec![value];
        arguments.extend(right);
        let rhs = emit_context.new_global_method_call(factory, b"Math", b"pow", arguments);
        let loc = factory.node(node).range();
        factory.set_node_range(rhs, loc);
        let result = emit_context.new_assignment_expression(factory, target, rhs);
        factory.set_node_range(result, loc);
        result
    }

    // port: tsc/internal/transformers/estransforms/exponentiation.go:exponentiationTransformer.visitExponentiationExpression
    fn visit_exponentiation_expression(
        &self,
        visitor: &mut NodeVisitor<'_>,
        node: NodeId,
    ) -> NodeId {
        let (node_left, node_right) = binary_operands(visitor, node);
        let left = visitor.visit_node(node_left);
        let right = visitor.visit_node(node_right);
        let factory = visitor.factory_mut();
        let arguments = left.into_iter().chain(right).collect();
        let result = self
            .emit_context
            .borrow()
            .new_global_method_call(factory, b"Math", b"pow", arguments);
        let loc = factory.node(node).range();
        factory.set_node_range(result, loc);
        result
    }
}

fn nil_checked(node: Option<NodeId>) -> NodeId {
    node.expect("runtime error: invalid memory address or nil pointer dereference")
}

/// `node.OperatorToken.Kind`.
fn binary_operator_kind(visitor: &NodeVisitor<'_>, node: NodeId) -> Option<K> {
    let factory = visitor.factory();
    let operator = factory
        .node(node)
        .as_binary_expression()
        .expect("BinaryExpression payload")
        .operator_token();
    factory.node(nil_checked(operator)).kind().known()
}

/// `node.Left`, `node.Right`.
fn binary_operands(visitor: &NodeVisitor<'_>, node: NodeId) -> (Option<NodeId>, Option<NodeId>) {
    let read = visitor.factory().node(node);
    let data = read
        .as_binary_expression()
        .expect("BinaryExpression payload");
    (data.left(), data.right())
}

// port: tsc/internal/transformers/estransforms/exponentiation.go:newExponentiationTransformer
pub fn new_exponentiation_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let tx = Rc::new(ExponentiationTransformer {
        emit_context: RefCell::new(opts.context.clone()),
        failure: opts.failure.clone(),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| tx.visit(visitor, node),
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}
