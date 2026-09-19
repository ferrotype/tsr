//! Statement control-flow and label handling from the pinned binder.
use crate::flow_access::BindingFlow;
use crate::target::{target_payload, BindingNode};
use crate::{need, ActiveLabel, Binder};
use ts_ast::{flow_flags as F, node_flags, JsString, SyntaxKind as K};

impl<'scope> Binder<'_, 'scope, '_> {
    // port: tsc/internal/binder/binder.go:Binder.setContinueTarget
    pub(crate) fn set_continue_target(
        &mut self,
        mut node: BindingNode<'scope>,
        target: BindingFlow<'scope>,
    ) -> BindingFlow<'scope> {
        let mut label = self.active_label_list;
        while let Some(index) = label {
            let parent = need(self.node_parent(node));
            if self.node_kind(parent) != K::LabeledStatement {
                break;
            }
            self.labels[index].continue_target = Some(target);
            label = self.labels[index].next;
            node = parent;
        }
        target
    }
    // port: tsc/internal/binder/binder.go:Binder.doWithConditionalBranches
    pub(crate) fn do_with_conditional_branches(
        &mut self,
        action: fn(&mut Self, Option<BindingNode<'scope>>) -> bool,
        value: Option<BindingNode<'scope>>,
        true_target: BindingFlow<'scope>,
        false_target: BindingFlow<'scope>,
    ) {
        let saved_true = self.current_true_target;
        let saved_false = self.current_false_target;
        self.current_true_target = Some(true_target);
        self.current_false_target = Some(false_target);
        action(self, value);
        self.current_true_target = saved_true;
        self.current_false_target = saved_false;
    }
    // port: tsc/internal/binder/binder.go:Binder.bindCondition
    pub(crate) fn bind_condition(
        &mut self,
        node: Option<BindingNode<'scope>>,
        true_target: BindingFlow<'scope>,
        false_target: BindingFlow<'scope>,
    ) {
        self.do_with_conditional_branches(
            Self::bind_optional_target,
            node,
            true_target,
            false_target,
        );
        if node.is_none_or(|node| {
            !(self.is_logical_assignment_expression(node)
                || self.target_is_logical_expression(node)
                || self.target_is_optional_chain(node)
                    && self.target_is_outermost_optional_chain(node))
        }) {
            let yes = self.create_flow_condition(F::TRUE_CONDITION, need(self.current_flow), node);
            self.add_antecedent(true_target, yes);
            let no = self.create_flow_condition(F::FALSE_CONDITION, need(self.current_flow), node);
            self.add_antecedent(false_target, no);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindIterativeStatement
    pub(crate) fn bind_iterative_statement(
        &mut self,
        node: Option<BindingNode<'scope>>,
        break_target: BindingFlow<'scope>,
        continue_target: BindingFlow<'scope>,
    ) {
        let saved_break = self.current_break_target;
        let saved_continue = self.current_continue_target;
        self.current_break_target = Some(break_target);
        self.current_continue_target = Some(continue_target);
        self.bind_optional_target(node);
        self.current_break_target = saved_break;
        self.current_continue_target = saved_continue;
    }
    // port: tsc/internal/binder/binder.go:Binder.bindWhileStatement
    pub(crate) fn bind_while_statement(&mut self, node: BindingNode<'scope>) {
        let (statement_expression, statement_statement) =
            target_payload!(self, node, as_while_statement; node: expression, node: statement);
        let pre = self.create_loop_label();
        let pre = self.set_continue_target(node, pre);
        let body = self.create_branch_label();
        let post = self.create_branch_label();
        self.add_antecedent(pre, need(self.current_flow));
        self.current_flow = Some(pre);
        self.bind_condition(statement_expression, body, post);
        self.current_flow = Some(self.finish_flow_label(body));
        self.bind_iterative_statement(statement_statement, post, pre);
        self.add_antecedent(pre, need(self.current_flow));
        self.current_flow = Some(self.finish_flow_label(post));
    }
    // port: tsc/internal/binder/binder.go:Binder.bindDoStatement
    pub(crate) fn bind_do_statement(&mut self, node: BindingNode<'scope>) {
        let (statement_statement, statement_expression) =
            target_payload!(self, node, as_do_statement; node: statement, node: expression);
        let pre = self.create_loop_label();
        let condition = self.create_branch_label();
        let condition = self.set_continue_target(node, condition);
        let post = self.create_branch_label();
        self.add_antecedent(pre, need(self.current_flow));
        self.current_flow = Some(pre);
        self.bind_iterative_statement(statement_statement, post, condition);
        self.add_antecedent(condition, need(self.current_flow));
        self.current_flow = Some(self.finish_flow_label(condition));
        self.bind_condition(statement_expression, pre, post);
        self.current_flow = Some(self.finish_flow_label(post));
    }
    // port: tsc/internal/binder/binder.go:Binder.bindForStatement
    pub(crate) fn bind_for_statement(&mut self, node: BindingNode<'scope>) {
        let (
            statement_initializer,
            statement_condition,
            statement_statement,
            statement_incrementor,
        ) = target_payload!(self, node, as_for_statement; node: initializer, node: condition, node: statement, node: incrementor);
        self.bind_optional_target(statement_initializer);
        // Preserve the source's early exit: an unreachable initializer must not
        // leave a loop graph with only its cyclic incrementor antecedent.
        if self.same_flow(self.current_flow, self.unreachable_flow) {
            self.bind_optional_target(statement_condition);
            self.bind_optional_target(statement_statement);
            self.bind_optional_target(statement_incrementor);
            return;
        }
        let pre = self.create_loop_label();
        let pre = self.set_continue_target(node, pre);
        let body = self.create_branch_label();
        let incrementor = self.create_branch_label();
        let post = self.create_branch_label();
        self.add_antecedent(pre, need(self.current_flow));
        self.current_flow = Some(pre);
        self.bind_condition(statement_condition, body, post);
        self.current_flow = Some(self.finish_flow_label(body));
        self.bind_iterative_statement(statement_statement, post, incrementor);
        self.add_antecedent(incrementor, need(self.current_flow));
        self.current_flow = Some(self.finish_flow_label(incrementor));
        self.bind_optional_target(statement_incrementor);
        self.add_antecedent(pre, need(self.current_flow));
        self.current_flow = Some(self.finish_flow_label(post));
    }
    // port: tsc/internal/binder/binder.go:Binder.bindForInOrForOfStatement
    pub(crate) fn bind_for_in_or_for_of_statement(&mut self, node: BindingNode<'scope>) {
        let (
            statement_expression,
            statement_initializer,
            statement_statement,
            statement_await_modifier,
        ) = target_payload!(self, node, as_for_in_or_of_statement; node: expression, node: initializer, node: statement, node: await_modifier);
        self.bind_optional_target(statement_expression);
        if self.same_flow(self.current_flow, self.unreachable_flow) {
            self.bind_optional_target(statement_initializer);
            self.bind_optional_target(statement_statement);
            return;
        }
        let pre = self.create_loop_label();
        let pre = self.set_continue_target(node, pre);
        let post = self.create_branch_label();
        self.add_antecedent(pre, need(self.current_flow));
        self.current_flow = Some(pre);
        if self.node_kind(node) == K::ForOfStatement {
            self.bind_optional_target(statement_await_modifier);
        }
        self.add_antecedent(post, need(self.current_flow));
        self.bind_optional_target(statement_initializer);
        if self.node_kind(need(statement_initializer)) != K::VariableDeclarationList {
            self.bind_assignment_target_flow(need(statement_initializer));
        }
        self.bind_iterative_statement(statement_statement, post, pre);
        self.add_antecedent(pre, need(self.current_flow));
        self.current_flow = Some(self.finish_flow_label(post));
    }
    // port: tsc/internal/binder/binder.go:Binder.bindIfStatement
    pub(crate) fn bind_if_statement(&mut self, node: BindingNode<'scope>) {
        let (statement_expression, statement_then_statement, statement_else_statement) = target_payload!(self, node, as_if_statement; node: expression, node: then_statement, node: else_statement);
        let yes = self.create_branch_label();
        let no = self.create_branch_label();
        let post = self.create_branch_label();
        self.bind_condition(statement_expression, yes, no);
        self.current_flow = Some(self.finish_flow_label(yes));
        self.bind_optional_target(statement_then_statement);
        self.add_antecedent(post, need(self.current_flow));
        self.current_flow = Some(self.finish_flow_label(no));
        self.bind_optional_target(statement_else_statement);
        self.add_antecedent(post, need(self.current_flow));
        self.current_flow = Some(self.finish_flow_label(post));
    }
    // port: tsc/internal/binder/binder.go:Binder.bindReturnStatement
    pub(crate) fn bind_return_statement(&mut self, node: BindingNode<'scope>) {
        self.bind_optional_target(self.node_expression(node));
        if let Some(target) = self.current_return_target {
            self.add_antecedent(target, need(self.current_flow));
        }
        self.current_flow = self.unreachable_flow;
        self.has_explicit_return = true;
        self.has_flow_effects = true;
    }
    // port: tsc/internal/binder/binder.go:Binder.bindThrowStatement
    pub(crate) fn bind_throw_statement(&mut self, node: BindingNode<'scope>) {
        self.bind_optional_target(self.node_expression(node));
        self.current_flow = self.unreachable_flow;
        self.has_flow_effects = true;
    }
    // port: tsc/internal/binder/binder.go:Binder.bindBreakStatement
    pub(crate) fn bind_break_statement(&mut self, node: BindingNode<'scope>) {
        self.bind_break_or_continue_statement(
            self.node_label(node),
            self.current_break_target,
            |label| label.break_target,
        );
    }
    // port: tsc/internal/binder/binder.go:Binder.bindContinueStatement
    pub(crate) fn bind_continue_statement(&mut self, node: BindingNode<'scope>) {
        self.bind_break_or_continue_statement(
            self.node_label(node),
            self.current_continue_target,
            |label| label.continue_target,
        );
    }
    // port: tsc/internal/binder/binder.go:Binder.bindBreakOrContinueStatement
    pub(crate) fn bind_break_or_continue_statement(
        &mut self,
        label: Option<BindingNode<'scope>>,
        current: Option<BindingFlow<'scope>>,
        target: fn(&ActiveLabel<'scope>) -> Option<BindingFlow<'scope>>,
    ) {
        self.bind_optional_target(label);
        if let Some(label) = label {
            if let Some(index) = self.find_active_label(&self.target_text(label)) {
                self.labels[index].referenced = true;
                self.bind_break_or_continue_flow(target(&self.labels[index]));
            }
        } else {
            self.bind_break_or_continue_flow(current);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.findActiveLabel
    pub(crate) fn find_active_label(&self, name: &JsString) -> Option<usize> {
        let mut label = self.active_label_list;
        while let Some(index) = label {
            if self.labels[index].name == *name {
                return Some(index);
            }
            label = self.labels[index].next;
        }
        None
    }
    // port: tsc/internal/binder/binder.go:Binder.bindBreakOrContinueFlow
    pub(crate) fn bind_break_or_continue_flow(&mut self, target: Option<BindingFlow<'scope>>) {
        if let Some(target) = target {
            self.add_antecedent(target, need(self.current_flow));
            self.current_flow = self.unreachable_flow;
            self.has_flow_effects = true;
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindTryStatement
    pub(crate) fn bind_try_statement(&mut self, node: BindingNode<'scope>) {
        let (statement_finally_block, statement_try_block, statement_catch_clause) = target_payload!(self, node, as_try_statement; node: finally_block, node: try_block, node: catch_clause);
        let saved_return = self.current_return_target;
        let saved_exception = self.current_exception_target;
        let normal = self.create_branch_label();
        let returned = self.create_branch_label();
        let mut exception = self.create_branch_label();
        if statement_finally_block.is_some() {
            self.current_return_target = Some(returned);
        }
        self.add_antecedent(exception, need(self.current_flow));
        self.current_exception_target = Some(exception);
        self.bind_optional_target(statement_try_block);
        self.add_antecedent(normal, need(self.current_flow));
        if statement_catch_clause.is_some() {
            self.current_flow = Some(self.finish_flow_label(exception));
            exception = self.create_branch_label();
            self.add_antecedent(exception, need(self.current_flow));
            self.current_exception_target = Some(exception);
            self.bind_optional_target(statement_catch_clause);
            self.add_antecedent(normal, need(self.current_flow));
        }
        self.current_return_target = saved_return;
        self.current_exception_target = saved_exception;
        if statement_finally_block.is_some() {
            let finally = self.create_branch_label();
            let exceptions_and_returns = self.combine_flow_lists(
                self.flow_antecedents(exception),
                self.flow_antecedents(returned),
            );
            let all =
                self.combine_flow_lists(self.flow_antecedents(normal), exceptions_and_returns);
            self.set_flow_antecedents(finally, all);
            self.current_flow = Some(finally);
            self.bind_optional_target(statement_finally_block);
            if self.flow(need(self.current_flow)).flags() & F::UNREACHABLE != 0 {
                self.current_flow = self.unreachable_flow;
            } else {
                if let Some(target) = self
                    .current_return_target
                    .filter(|_| self.flow_antecedents(returned).is_some())
                {
                    let reduced = self.create_reduce_label(
                        finally,
                        self.flow_antecedents(returned),
                        need(self.current_flow),
                    );
                    self.add_antecedent(target, reduced);
                }
                if let Some(target) = self
                    .current_exception_target
                    .filter(|_| self.flow_antecedents(exception).is_some())
                {
                    let reduced = self.create_reduce_label(
                        finally,
                        self.flow_antecedents(exception),
                        need(self.current_flow),
                    );
                    self.add_antecedent(target, reduced);
                }
                self.current_flow = if self.flow_antecedents(normal).is_some() {
                    Some(self.create_reduce_label(
                        finally,
                        self.flow_antecedents(normal),
                        need(self.current_flow),
                    ))
                } else {
                    self.unreachable_flow
                };
            }
        } else {
            self.current_flow = Some(self.finish_flow_label(normal));
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindSwitchStatement
    pub(crate) fn bind_switch_statement(&mut self, node: BindingNode<'scope>) {
        let (statement_expression, statement_case_block) =
            target_payload!(self, node, as_switch_statement; node: expression, node: case_block);
        let post = self.create_branch_label();
        self.bind_optional_target(statement_expression);
        let saved_break = self.current_break_target;
        let saved_pre = self.pre_switch_case_flow;
        self.current_break_target = Some(post);
        self.pre_switch_case_flow = self.current_flow;
        self.bind_optional_target(statement_case_block);
        self.add_antecedent(post, need(self.current_flow));
        let (case_block_clauses,) =
            target_payload!(self, need(statement_case_block), as_case_block; list: clauses);
        let clauses = self.target_list_edges(Some(need(case_block_clauses)));
        let has_default = (0..clauses.len())
            .any(|index| self.node_kind(need(self.edge(clauses, index))) == K::DefaultClause);
        if !has_default {
            let clause =
                self.create_flow_switch_clause(need(self.pre_switch_case_flow), node, 0, 0);
            self.add_antecedent(post, clause);
        }
        self.current_break_target = saved_break;
        self.pre_switch_case_flow = saved_pre;
        self.current_flow = Some(self.finish_flow_label(post));
    }
    // port: tsc/internal/binder/binder.go:Binder.bindCaseBlock
    pub(crate) fn bind_case_block(&mut self, node: BindingNode<'scope>) {
        let statement = need(self.node_parent(node));
        let expression = need(self.node_expression(statement));
        let clauses = self.target_list_edges(Some(need(
            target_payload!(self, node, as_case_block; list: clauses).0,
        )));
        let narrowing = self.node_kind(expression) == K::TrueKeyword
            || self.is_narrowing_expression(expression);
        let mut fallthrough = self.unreachable_flow;
        let mut index = 0;
        while index < clauses.len() {
            let start = index;
            while self
                .target_list_edges(self.node_statement_list(need(self.edge(clauses, index))))
                .is_empty()
                && index + 1 < clauses.len()
            {
                if self.same_flow(fallthrough, self.unreachable_flow) {
                    self.current_flow = self.pre_switch_case_flow;
                }
                self.bind_optional_target(self.edge(clauses, index));
                index += 1;
            }
            let pre_case = self.create_branch_label();
            let pre_flow = if narrowing {
                self.create_flow_switch_clause(
                    need(self.pre_switch_case_flow),
                    statement,
                    start as i64,
                    (index + 1) as i64,
                )
            } else {
                need(self.pre_switch_case_flow)
            };
            self.add_antecedent(pre_case, pre_flow);
            self.add_antecedent(pre_case, need(fallthrough));
            self.current_flow = Some(self.finish_flow_label(pre_case));
            let clause = need(self.edge(clauses, index));
            self.bind_optional_target(Some(clause));
            fallthrough = self.current_flow;
            if self.flow(need(self.current_flow)).flags() & F::UNREACHABLE == 0
                && index != clauses.len() - 1
            {
                self.set_node_fallthrough_flow(self.node_id(clause), self.current_flow);
            }
            index += 1;
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindCaseOrDefaultClause
    pub(crate) fn bind_case_or_default_clause(&mut self, node: BindingNode<'scope>) {
        let (clause_expression, clause_statements) = target_payload!(self, node, as_case_or_default_clause; node: expression, list: statements);
        if clause_expression.is_some() {
            let saved = self.current_flow;
            self.current_flow = self.pre_switch_case_flow;
            self.bind_optional_target(clause_expression);
            self.current_flow = saved;
        }
        self.bind_target_list(Some(need(clause_statements)));
    }
    // port: tsc/internal/binder/binder.go:Binder.bindExpressionStatement
    pub(crate) fn bind_expression_statement(&mut self, node: BindingNode<'scope>) {
        let expression = self.node_expression(node);
        self.bind_optional_target(expression);
        self.maybe_bind_expression_flow_if_call(need(expression));
    }
    // port: tsc/internal/binder/binder.go:Binder.bindLabeledStatement
    pub(crate) fn bind_labeled_statement(&mut self, node: BindingNode<'scope>) {
        let (statement_label, statement_statement) =
            target_payload!(self, node, as_labeled_statement; node: label, node: statement);
        let post = self.create_branch_label();
        let index = self.labels.len();
        self.labels.push(ActiveLabel {
            next: self.active_label_list,
            name: self.target_text(need(statement_label)),
            break_target: Some(post),
            continue_target: None,
            referenced: false,
        });
        self.active_label_list = Some(index);
        self.bind_optional_target(statement_label);
        self.bind_optional_target(statement_statement);
        let active = need(self.active_label_list);
        if !self.labels[active].referenced {
            let label = need(statement_label);
            self.set_binding_flags(label, self.node_flags(label) | node_flags::UNREACHABLE);
        }
        self.active_label_list = self.labels[active].next;
        self.add_antecedent(post, need(self.current_flow));
        self.current_flow = Some(self.finish_flow_label(post));
    }
}
