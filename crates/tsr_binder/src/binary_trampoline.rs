//! Explicit continuations for ordinary binary-expression evaluation. Logical and
//! destructuring forms retain their own branch semantics under the bind guard.
use crate::{backend::Backend, need, target::BindingNode, Binder};
use ts_ast::{flow_flags as F, node_flags as N, SyntaxKind as K};

struct Exit<'scope> {
    node: BindingNode<'scope>,
    parse_error: bool,
    saved_parse_error: bool,
    saved_assignment_pattern: bool,
}

enum Step<'scope> {
    Visit(Option<BindingNode<'scope>>),
    AfterLeft(BindingNode<'scope>),
    AfterRight(BindingNode<'scope>),
    Exit(Exit<'scope>),
}

// Only the links needed across a recursive bind are copied. The local variant
// borrows its typed payload directly and keeps each link in the same scope.
struct BinaryOperands<'scope> {
    left: Option<BindingNode<'scope>>,
    r#type: Option<BindingNode<'scope>>,
    operator_token: Option<BindingNode<'scope>>,
    right: Option<BindingNode<'scope>>,
}

impl<'scope> Binder<'_, 'scope, '_> {
    pub(crate) fn bind_binary_expression_target(&mut self, node: BindingNode<'scope>) {
        let expression = self.binary_operands(node);
        let left = expression.left;
        // Ordinary leaves need no continuation allocation. Long binary chains
        // share one stack for every directly nested ordinary binary operand.
        if [left, expression.right]
            .into_iter()
            .flatten()
            .all(|child| self.node_kind(child) != K::BinaryExpression)
        {
            self.bind_binary_child(left);
            self.bind_binary_middle(node);
            self.bind_binary_child(expression.right);
            self.finish_binary_flow(node);
            return;
        }
        let mut steps = vec![Step::AfterLeft(node), Step::Visit(left)];
        #[cfg(test)]
        crate::recursion::binary_frame(steps.len());
        while let Some(step) = steps.pop() {
            match step {
                Step::Visit(Some(child)) if self.can_continue_binary(child) => {
                    self.bind_target_head(child);
                    let exit = Exit {
                        node: child,
                        parse_error: self.node_flags(child) & N::THIS_NODE_HAS_ERROR != 0,
                        saved_parse_error: self.seen_parse_error,
                        saved_assignment_pattern: self.in_assignment_pattern,
                    };
                    self.seen_parse_error = false;
                    self.in_assignment_pattern = false;
                    steps.push(Step::Exit(exit));
                    steps.push(Step::AfterLeft(child));
                    steps.push(Step::Visit(self.binary_operands(child).left));
                    #[cfg(test)]
                    crate::recursion::binary_frame(steps.len());
                }
                Step::Visit(node) => {
                    self.bind_binary_child(node);
                }
                Step::AfterLeft(node) => {
                    let expression = self.binary_operands(node);
                    self.bind_binary_middle(node);
                    steps.push(Step::AfterRight(node));
                    steps.push(Step::Visit(expression.right));
                }
                Step::AfterRight(node) => self.finish_binary_flow(node),
                Step::Exit(exit) => {
                    self.in_assignment_pattern = exit.saved_assignment_pattern;
                    let error = exit.parse_error || self.seen_parse_error;
                    self.seen_parse_error = exit.saved_parse_error;
                    self.bind_target_error(exit.node, error);
                }
            }
        }
    }
    fn can_continue_binary(&self, node: BindingNode<'scope>) -> bool {
        if self.node_kind(node) != K::BinaryExpression
            || self.same_flow(self.current_flow, self.unreachable_flow)
        {
            return false;
        }
        if ts_ast::is_destructuring_assignment(self.view(), self.node_id(node))
            .expect("retained destructuring expression")
        {
            return false;
        }
        let operator = self.node_kind(need(self.binary_operands(node).operator_token));
        !ts_ast::utilities::is_logical_or_coalescing_binary_operator(operator)
            && !ts_ast::is_logical_or_coalescing_assignment_operator(operator)
    }
    fn bind_binary_middle(&mut self, node: BindingNode<'scope>) {
        let expression = self.binary_operands(node);
        self.bind_binary_child(expression.r#type);
        if self.node_kind(need(expression.operator_token)) == K::CommaToken {
            self.maybe_bind_expression_flow_if_call(need(expression.left));
        }
        self.bind_binary_child(expression.operator_token);
    }
    fn finish_binary_flow(&mut self, node: BindingNode<'scope>) {
        let expression = self.binary_operands(node);
        let operator = self.node_kind(need(expression.operator_token));
        if operator == K::CommaToken {
            self.maybe_bind_expression_flow_if_call(need(expression.right));
        }
        if ts_ast::is_assignment_operator(operator)
            && !ts_ast::is_assignment_target(self.view(), self.node_id(node))
                .expect("retained assignment")
        {
            self.bind_assignment_target_flow(need(expression.left));
            if operator == K::EqualsToken
                && self.node_kind(need(expression.left)) == K::ElementAccessExpression
            {
                let target = self.binary_element_expression(need(expression.left));
                if self.is_narrowable_operand(target) {
                    self.current_flow = Some(self.create_flow_mutation(
                        F::ARRAY_MUTATION,
                        need(self.current_flow),
                        node,
                    ));
                }
            }
        }
    }

    fn bind_binary_child(&mut self, node: Option<BindingNode<'scope>>) {
        if let Some(node) = node {
            self.bind_target(node);
        }
    }

    fn binary_operands(&self, node: BindingNode<'scope>) -> BinaryOperands<'scope> {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope");
                };
                let expression = local
                    .node(node)
                    .as_binary_expression()
                    .expect("binder syntax payload");
                BinaryOperands {
                    left: expression.left().map(BindingNode::Local),
                    r#type: expression.r#type().map(BindingNode::Local),
                    operator_token: expression.operator_token().map(BindingNode::Local),
                    right: expression.right().map(BindingNode::Local),
                }
            }
            BindingNode::Checked(node) => {
                let read = self.n(node);
                let expression = read
                    .data_source()
                    .as_binary_expression()
                    .expect("binder syntax payload");
                BinaryOperands {
                    left: expression.left().map(BindingNode::Checked),
                    r#type: expression.r#type().map(BindingNode::Checked),
                    operator_token: expression.operator_token().map(BindingNode::Checked),
                    right: expression.right().map(BindingNode::Checked),
                }
            }
        }
    }

    // Called only after the left operand's kind selected ElementAccessExpression.
    // A constructed mismatch retains the public expression-accessor panic.
    fn binary_element_expression(&self, node: BindingNode<'scope>) -> BindingNode<'scope> {
        if let BindingNode::Local(node) = node {
            let Backend::Local(local) = &self.builder else {
                unreachable!("local binder scope");
            };
            if let Some(element) = local.node(node).as_element_access_expression() {
                return BindingNode::Local(need(element.expression()));
            }
        }
        BindingNode::Checked(need(self.n(self.node_id(node)).expression()))
    }
}

#[cfg(test)]
#[path = "binary_trampoline_tests.rs"]
mod tests;
