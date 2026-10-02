//! `transformers/estransforms/nullishcoalescing.go`: `a ?? b` to a
//! conditional over a not-null test.
use super::utilities::{create_not_null_condition, is_simple_copiable_expression, subtree_facts};
use crate::transformer::{Failure, TransformOptions, Transformer};
use std::cell::RefCell;
use std::rc::Rc;
use tsr_ast::{subtree_flags, FactoryMethods, NodeId, NodeVisitor, SyntaxKind as K};
use tsr_printer::EmitContext;

struct NullishCoalescingTransformer {
    emit_context: RefCell<EmitContext>,
    failure: Failure,
}

impl NullishCoalescingTransformer {
    // port: tsc/internal/transformers/estransforms/nullishcoalescing.go:nullishCoalescingTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        if self.failure.is_set() {
            return node;
        }
        let id = node.expect("runtime error: invalid memory address or nil pointer dereference");
        let Some(facts) = self.failure.ok(subtree_facts(visitor.factory(), id)) else {
            return node;
        };
        if facts & subtree_flags::NULLISH_COALESCING == 0 {
            return node;
        }
        match visitor.factory().node(id).kind().known() {
            Some(K::BinaryExpression) => self.visit_binary_expression(visitor, id),
            _ => visitor.visit_each_child(node),
        }
    }

    // port: tsc/internal/transformers/estransforms/nullishcoalescing.go:nullishCoalescingTransformer.visitBinaryExpression
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
        if operator == K::QuestionQuestionToken {
            let mut left = nil_checked(visitor.visit_node(node_left));
            let mut right = left;
            {
                let mut emit_context = self.emit_context.borrow_mut();
                let factory = visitor.factory_mut();
                if !is_simple_copiable_expression(factory, left) {
                    right = emit_context.new_temp_variable(factory);
                    emit_context.add_variable_declaration(factory, right);
                    left = emit_context.new_assignment_expression(factory, right, left);
                }
            }
            let condition = create_not_null_condition(
                &self.emit_context.borrow(),
                visitor.factory_mut(),
                left,
                right,
                false,
            );
            let question = visitor.factory_mut().new_token(K::QuestionToken.into());
            let colon = visitor.factory_mut().new_token(K::ColonToken.into());
            let when_false = visitor.visit_node(node_right);
            return Some(visitor.factory_mut().new_conditional_expression(
                Some(condition),
                Some(question),
                Some(right),
                Some(colon),
                when_false,
            ));
        }
        visitor.visit_each_child(Some(node))
    }
}

fn nil_checked(node: Option<NodeId>) -> NodeId {
    node.expect("runtime error: invalid memory address or nil pointer dereference")
}

// port: tsc/internal/transformers/estransforms/nullishcoalescing.go:newNullishCoalescingTransformer
pub fn new_nullish_coalescing_transformer<'a>(
    opts: &TransformOptions<'a>,
) -> Option<Transformer<'a>> {
    let tx = Rc::new(NullishCoalescingTransformer {
        emit_context: RefCell::new(opts.context.clone()),
        failure: opts.failure.clone(),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| tx.visit(visitor, node),
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}
