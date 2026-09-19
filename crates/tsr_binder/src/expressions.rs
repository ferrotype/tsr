//! Expression evaluation order and narrowing predicates from the pinned binder.
use crate::flow_access::BindingFlow;
use crate::target::{target_payload, BindingNode};
use crate::{need, Binder};
use ts_ast::{flow_flags as F, node_flags, utilities as u, SyntaxKind as K};

impl<'scope> Binder<'_, 'scope, '_> {
    // port: tsc/internal/binder/binder.go:Binder.bindAssignmentTargetFlow
    pub(crate) fn bind_assignment_target_flow(&mut self, node: BindingNode<'scope>) {
        match self.node_kind(node).known() {
            Some(K::ArrayLiteralExpression) => {
                let elements = self.target_list_edges(self.node_element_list(node));
                for index in 0..elements.len() {
                    let element = need(self.edge(elements, index));
                    if self.node_kind(element) == K::SpreadElement {
                        self.bind_assignment_target_flow(need(self.node_expression(element)));
                    } else {
                        self.bind_destructuring_target_flow(element);
                    }
                }
            }
            Some(K::ObjectLiteralExpression) => {
                let properties = self.target_list_edges(self.node_property_list(node));
                for index in 0..properties.len() {
                    let property = need(self.edge(properties, index));
                    match self.node_kind(property).known() {
                        Some(K::PropertyAssignment) => self
                            .bind_destructuring_target_flow(need(self.node_initializer(property))),
                        Some(K::ShorthandPropertyAssignment) => {
                            self.bind_assignment_target_flow(need(self.node_name(property)));
                        }
                        Some(K::SpreadAssignment) => {
                            self.bind_assignment_target_flow(need(self.node_expression(property)));
                        }
                        _ => {}
                    }
                }
            }
            _ => {
                if self.is_narrowable_reference(node) {
                    self.current_flow = Some(self.create_flow_mutation(
                        F::ASSIGNMENT,
                        need(self.current_flow),
                        node,
                    ));
                }
            }
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindDestructuringTargetFlow
    pub(crate) fn bind_destructuring_target_flow(&mut self, node: BindingNode<'scope>) {
        if self.node_kind(node) == K::BinaryExpression {
            let (binary_operator_token, binary_left) =
                target_payload!(self, node, as_binary_expression; node: operator_token, node: left);
            if self.node_kind(need(binary_operator_token)) == K::EqualsToken {
                self.bind_assignment_target_flow(need(binary_left));
                return;
            }
        }
        self.bind_assignment_target_flow(node);
    }
    // port: tsc/internal/binder/binder.go:Binder.maybeBindExpressionFlowIfCall
    pub(crate) fn maybe_bind_expression_flow_if_call(&mut self, node: BindingNode<'scope>) {
        if self.node_kind(node) == K::CallExpression {
            let expression = need(self.node_expression(node));
            if self.node_kind(expression) != K::SuperKeyword
                && self.target_is_dotted_name(expression)
            {
                self.current_flow = Some(self.create_flow_call(need(self.current_flow), node));
            }
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindPrefixUnaryExpressionFlow
    pub(crate) fn bind_prefix_unary_expression_flow(&mut self, node: BindingNode<'scope>) {
        let (expression_operator, expression_operand) = target_payload!(self, node, as_prefix_unary_expression; scalar: operator, node: operand);
        if expression_operator == K::ExclamationToken {
            let saved_true = self.current_true_target;
            std::mem::swap(
                &mut self.current_true_target,
                &mut self.current_false_target,
            );
            self.bind_each_child_target(node);
            self.current_false_target = self.current_true_target;
            self.current_true_target = saved_true;
        } else {
            self.bind_each_child_target(node);
            if matches!(
                expression_operator.known(),
                Some(K::PlusPlusToken | K::MinusMinusToken)
            ) {
                self.bind_assignment_target_flow(need(expression_operand));
            }
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindPostfixUnaryExpressionFlow
    pub(crate) fn bind_postfix_unary_expression_flow(&mut self, node: BindingNode<'scope>) {
        let (expression_operator, expression_operand) = target_payload!(self, node, as_postfix_unary_expression; scalar: operator, node: operand);
        self.bind_each_child_target(node);
        if matches!(
            expression_operator.known(),
            Some(K::PlusPlusToken | K::MinusMinusToken)
        ) {
            self.bind_assignment_target_flow(need(expression_operand));
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindDestructuringAssignmentFlow
    pub(crate) fn bind_destructuring_assignment_flow(&mut self, node: BindingNode<'scope>) {
        let (expression_operator_token, expression_right, expression_left, expression_type) = target_payload!(self, node, as_binary_expression; node: operator_token, node: right, node: left, node: r#type);
        if self.in_assignment_pattern {
            self.in_assignment_pattern = false;
            self.bind_optional_target(expression_operator_token);
            self.bind_optional_target(expression_right);
            self.in_assignment_pattern = true;
            self.bind_optional_target(expression_left);
            self.bind_optional_target(expression_type);
        } else {
            self.in_assignment_pattern = true;
            self.bind_optional_target(expression_left);
            self.bind_optional_target(expression_type);
            self.in_assignment_pattern = false;
            self.bind_optional_target(expression_operator_token);
            self.bind_optional_target(expression_right);
        }
        self.bind_assignment_target_flow(need(expression_left));
    }
    // port: tsc/internal/binder/binder.go:Binder.bindBinaryExpressionFlow
    pub(crate) fn bind_binary_expression_flow(&mut self, node: BindingNode<'scope>) {
        let (expression_operator_token,) =
            target_payload!(self, node, as_binary_expression; node: operator_token);
        let operator = self.node_kind(need(expression_operator_token));
        if u::is_logical_or_coalescing_binary_operator(operator)
            || ts_ast::is_logical_or_coalescing_assignment_operator(operator)
        {
            if self.is_top_level_logical_expression(node) {
                let post = self.create_branch_label();
                let saved_flow = self.current_flow;
                let saved_effects = self.has_flow_effects;
                self.has_flow_effects = false;
                self.bind_logical_like_expression(node, post, post);
                self.current_flow = if self.has_flow_effects {
                    Some(self.finish_flow_label(post))
                } else {
                    saved_flow
                };
                self.has_flow_effects |= saved_effects;
            } else {
                self.bind_logical_like_expression(
                    node,
                    need(self.current_true_target),
                    need(self.current_false_target),
                );
            }
        } else {
            self.bind_binary_expression_target(node);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindLogicalLikeExpression
    pub(crate) fn bind_logical_like_expression(
        &mut self,
        node: BindingNode<'scope>,
        true_target: BindingFlow<'scope>,
        false_target: BindingFlow<'scope>,
    ) {
        let (expression_operator_token, expression_left, expression_right) = target_payload!(self, node, as_binary_expression; node: operator_token, node: left, node: right);
        let operator = self.node_kind(need(expression_operator_token));
        let pre_right = self.create_branch_label();
        if matches!(
            operator.known(),
            Some(K::AmpersandAmpersandToken | K::AmpersandAmpersandEqualsToken)
        ) {
            self.bind_condition(expression_left, pre_right, false_target);
        } else {
            self.bind_condition(expression_left, true_target, pre_right);
        }
        self.current_flow = Some(self.finish_flow_label(pre_right));
        self.bind_optional_target(expression_operator_token);
        if ts_ast::is_logical_or_coalescing_assignment_operator(operator) {
            self.do_with_conditional_branches(
                Self::bind_optional_target,
                expression_right,
                true_target,
                false_target,
            );
            self.bind_assignment_target_flow(need(expression_left));
            let true_flow =
                self.create_flow_condition(F::TRUE_CONDITION, need(self.current_flow), Some(node));
            self.add_antecedent(true_target, true_flow);
            let false_flow =
                self.create_flow_condition(F::FALSE_CONDITION, need(self.current_flow), Some(node));
            self.add_antecedent(false_target, false_flow);
        } else {
            self.bind_condition(expression_right, true_target, false_target);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindDeleteExpressionFlow
    pub(crate) fn bind_delete_expression_flow(&mut self, node: BindingNode<'scope>) {
        let expression = need(self.node_expression(node));
        self.bind_each_child_target(node);
        if self.node_kind(expression) == K::PropertyAccessExpression {
            self.bind_assignment_target_flow(expression);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindConditionalExpressionFlow
    pub(crate) fn bind_conditional_expression_flow(&mut self, node: BindingNode<'scope>) {
        let (
            expression_condition,
            expression_question_token,
            expression_when_true,
            expression_colon_token,
            expression_when_false,
        ) = target_payload!(self, node, as_conditional_expression; node: condition, node: question_token, node: when_true, node: colon_token, node: when_false);
        let true_label = self.create_branch_label();
        let false_label = self.create_branch_label();
        let post = self.create_branch_label();
        let saved_flow = self.current_flow;
        let saved_effects = self.has_flow_effects;
        self.has_flow_effects = false;
        self.bind_condition(expression_condition, true_label, false_label);
        self.current_flow = Some(self.finish_flow_label(true_label));
        self.bind_optional_target(expression_question_token);
        self.bind_optional_target(expression_when_true);
        self.add_antecedent(post, need(self.current_flow));
        self.current_flow = Some(self.finish_flow_label(false_label));
        self.bind_optional_target(expression_colon_token);
        self.bind_optional_target(expression_when_false);
        self.add_antecedent(post, need(self.current_flow));
        self.current_flow = if self.has_flow_effects {
            Some(self.finish_flow_label(post))
        } else {
            saved_flow
        };
        self.has_flow_effects |= saved_effects;
    }
    // port: tsc/internal/binder/binder.go:Binder.bindVariableDeclarationFlow
    pub(crate) fn bind_variable_declaration_flow(&mut self, node: BindingNode<'scope>) {
        self.bind_each_child_target(node);
        if self.node_initializer(node).is_some()
            || self
                .node_parent(need(self.node_parent(node)))
                .is_some_and(|parent| u::is_for_in_or_of_statement_kind(self.node_kind(parent)))
        {
            self.bind_initialized_variable_flow(node);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindInitializedVariableFlow
    pub(crate) fn bind_initialized_variable_flow(&mut self, node: BindingNode<'scope>) {
        // This descent follows nested binding patterns without reentering bind.
        crate::recursion::guarded(|| self.bind_initialized_variable_flow_worker(node));
    }
    fn bind_initialized_variable_flow_worker(&mut self, node: BindingNode<'scope>) {
        let name = if matches!(
            self.node_kind(node).known(),
            Some(K::VariableDeclaration | K::BindingElement)
        ) {
            self.node_name(node)
        } else {
            None
        };
        if let Some(name) = name.filter(|&name| u::is_binding_pattern_kind(self.node_kind(name))) {
            let elements = self.target_list_edges(self.node_element_list(name));
            for index in 0..elements.len() {
                self.bind_initialized_variable_flow(need(self.edge(elements, index)));
            }
        } else {
            self.current_flow =
                Some(self.create_flow_mutation(F::ASSIGNMENT, need(self.current_flow), node));
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindAccessExpressionFlow
    pub(crate) fn bind_access_expression_flow(&mut self, node: BindingNode<'scope>) {
        if self.target_is_optional_chain(node) {
            self.bind_optional_chain_flow(node);
        } else {
            self.bind_each_child_target(node);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindOptionalChainFlow
    pub(crate) fn bind_optional_chain_flow(&mut self, node: BindingNode<'scope>) {
        if self.is_top_level_logical_expression(node) {
            let post = self.create_branch_label();
            let saved_flow = self.current_flow;
            let saved_effects = self.has_flow_effects;
            // The source deliberately does not clear hasFlowEffects on this path.
            self.bind_optional_chain(node, post, post);
            self.current_flow = if self.has_flow_effects {
                Some(self.finish_flow_label(post))
            } else {
                saved_flow
            };
            self.has_flow_effects |= saved_effects;
        } else {
            self.bind_optional_chain(
                node,
                need(self.current_true_target),
                need(self.current_false_target),
            );
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindOptionalChain
    pub(crate) fn bind_optional_chain(
        &mut self,
        node: BindingNode<'scope>,
        true_target: BindingFlow<'scope>,
        false_target: BindingFlow<'scope>,
    ) {
        let pre_chain = if self.target_is_optional_chain_root(node) {
            Some(self.create_branch_label())
        } else {
            None
        };
        self.bind_optional_expression(
            need(self.node_expression(node)),
            pre_chain.unwrap_or(true_target),
            false_target,
        );
        if let Some(pre_chain) = pre_chain {
            self.current_flow = Some(self.finish_flow_label(pre_chain));
        }
        self.do_with_conditional_branches(
            Self::bind_optional_chain_rest,
            Some(node),
            true_target,
            false_target,
        );
        if self.target_is_outermost_optional_chain(node) {
            let yes =
                self.create_flow_condition(F::TRUE_CONDITION, need(self.current_flow), Some(node));
            self.add_antecedent(true_target, yes);
            let no =
                self.create_flow_condition(F::FALSE_CONDITION, need(self.current_flow), Some(node));
            self.add_antecedent(false_target, no);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindOptionalExpression
    pub(crate) fn bind_optional_expression(
        &mut self,
        node: BindingNode<'scope>,
        true_target: BindingFlow<'scope>,
        false_target: BindingFlow<'scope>,
    ) {
        self.do_with_conditional_branches(
            Self::bind_optional_target,
            Some(node),
            true_target,
            false_target,
        );
        if !self.target_is_optional_chain(node) || self.target_is_outermost_optional_chain(node) {
            let yes =
                self.create_flow_condition(F::TRUE_CONDITION, need(self.current_flow), Some(node));
            self.add_antecedent(true_target, yes);
            let no =
                self.create_flow_condition(F::FALSE_CONDITION, need(self.current_flow), Some(node));
            self.add_antecedent(false_target, no);
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindOptionalChainRest
    pub(crate) fn bind_optional_chain_rest(&mut self, node: Option<BindingNode<'scope>>) -> bool {
        let node = need(node);
        match self.node_kind(node).known() {
            Some(K::PropertyAccessExpression) => {
                self.bind_optional_target(self.node_question_dot_token(node));
                self.bind_optional_target(self.node_name(node));
            }
            Some(K::ElementAccessExpression) => {
                let (data_question_dot_token, data_argument_expression) = target_payload!(self, node, as_element_access_expression; node: question_dot_token, node: argument_expression);
                self.bind_optional_target(data_question_dot_token);
                self.bind_optional_target(data_argument_expression);
            }
            Some(K::CallExpression) => {
                let (data_question_dot_token, data_type_arguments, data_arguments) = target_payload!(self, node, as_call_expression; node: question_dot_token, list: type_arguments, list: arguments);
                self.bind_optional_target(data_question_dot_token);
                self.bind_target_list(data_type_arguments);
                self.bind_target_list(data_arguments);
            }
            _ => {}
        }
        false
    }
    // port: tsc/internal/binder/binder.go:Binder.bindCallExpressionFlow
    pub(crate) fn bind_call_expression_flow(&mut self, node: BindingNode<'scope>) {
        let (call_expression, call_type_arguments, call_arguments) = target_payload!(self, node, as_call_expression; node: expression, list: type_arguments, list: arguments);
        let expression = need(call_expression);
        if self.target_is_optional_chain(node) {
            self.bind_optional_chain_flow(node);
        } else {
            let target = self.target_skip_parentheses(expression);
            if matches!(
                self.node_kind(target).known(),
                Some(K::FunctionExpression | K::ArrowFunction)
            ) {
                self.bind_target_list(call_type_arguments);
                self.bind_target_list(Some(need(call_arguments)));
                self.bind_optional_target(call_expression);
            } else {
                self.bind_each_child_target(node);
                if self.node_kind(expression) == K::SuperKeyword {
                    self.current_flow = Some(self.create_flow_call(need(self.current_flow), node));
                }
            }
        }
        if self.node_kind(expression) == K::PropertyAccessExpression {
            let (name, target) = target_payload!(self, expression, as_property_access_expression; node: name, node: expression);
            if self.node_kind(need(name)) == K::Identifier
                && self.is_narrowable_operand(need(target))
                && self.target_is_push_or_unshift_identifier(need(name))
            {
                self.current_flow = Some(self.create_flow_mutation(
                    F::ARRAY_MUTATION,
                    need(self.current_flow),
                    node,
                ));
            }
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.bindNonNullExpressionFlow
    pub(crate) fn bind_non_null_expression_flow(&mut self, node: BindingNode<'scope>) {
        self.bind_access_expression_flow(node);
    }
    // port: tsc/internal/binder/binder.go:Binder.bindBindingElementFlow
    pub(crate) fn bind_binding_element_flow(&mut self, node: BindingNode<'scope>) {
        let (element_dot_dot_dot_token, element_property_name, element_initializer, element_name) = target_payload!(self, node, as_binding_element; node: dot_dot_dot_token, node: property_name, node: initializer, node: name);
        self.bind_optional_target(element_dot_dot_dot_token);
        self.bind_optional_target(element_property_name);
        self.bind_initializer(element_initializer);
        self.bind_optional_target(element_name);
    }
    // port: tsc/internal/binder/binder.go:Binder.bindParameterFlow
    pub(crate) fn bind_parameter_flow(&mut self, node: BindingNode<'scope>) {
        let (
            parameter_modifiers,
            parameter_dot_dot_dot_token,
            parameter_question_token,
            parameter_type,
            parameter_initializer,
            parameter_name,
        ) = target_payload!(self, node, as_parameter_declaration; list: modifiers, node: dot_dot_dot_token, node: question_token, node: r#type, node: initializer, node: name);
        self.bind_target_list(parameter_modifiers);
        self.bind_optional_target(parameter_dot_dot_dot_token);
        self.bind_optional_target(parameter_question_token);
        self.bind_optional_target(parameter_type);
        self.bind_initializer(parameter_initializer);
        self.bind_optional_target(parameter_name);
    }
    // port: tsc/internal/binder/binder.go:Binder.bindInitializer
    pub(crate) fn bind_initializer(&mut self, node: Option<BindingNode<'scope>>) {
        if node.is_none() {
            return;
        }
        let entry = self.current_flow;
        self.bind_optional_target(node);
        if self.same_flow(entry, self.unreachable_flow) || self.same_flow(entry, self.current_flow)
        {
            return;
        }
        let exit = self.create_branch_label();
        self.add_antecedent(exit, need(entry));
        self.add_antecedent(exit, need(self.current_flow));
        self.current_flow = Some(self.finish_flow_label(exit));
    }
    // port: tsc/internal/binder/binder.go:isNarrowingExpression
    pub(crate) fn is_narrowing_expression(&self, node: BindingNode<'scope>) -> bool {
        match self.node_kind(node).known() {
            Some(K::Identifier | K::ThisKeyword) => true,
            Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                self.contains_narrowable_reference(node)
            }
            Some(K::CallExpression) => self.has_narrowable_argument(node),
            Some(K::ParenthesizedExpression | K::NonNullExpression | K::TypeOfExpression) => {
                self.is_narrowing_expression(need(self.node_expression(node)))
            }
            Some(K::BinaryExpression) => self.is_narrowing_binary_expression(node),
            Some(K::PrefixUnaryExpression) => {
                let (p_operator, p_operand) = target_payload!(self, node, as_prefix_unary_expression; scalar: operator, node: operand);
                p_operator == K::ExclamationToken && self.is_narrowing_expression(need(p_operand))
            }
            _ => false,
        }
    }
    // port: tsc/internal/binder/binder.go:containsNarrowableReference
    pub(crate) fn contains_narrowable_reference(&self, node: BindingNode<'scope>) -> bool {
        if self.is_narrowable_reference(node) {
            return true;
        }
        self.node_flags(node) & node_flags::OPTIONAL_CHAIN != 0
            && matches!(
                self.node_kind(node).known(),
                Some(
                    K::PropertyAccessExpression
                        | K::ElementAccessExpression
                        | K::CallExpression
                        | K::NonNullExpression
                )
            )
            && self.contains_narrowable_reference(need(self.node_expression(node)))
    }
    // port: tsc/internal/binder/binder.go:isNarrowableReference
    pub(crate) fn is_narrowable_reference(&self, node: BindingNode<'scope>) -> bool {
        match self.node_kind(node).known() {
            Some(K::Identifier | K::ThisKeyword | K::SuperKeyword | K::MetaProperty) => true,
            Some(
                K::PropertyAccessExpression | K::ParenthesizedExpression | K::NonNullExpression,
            ) => self.is_narrowable_reference(need(self.node_expression(node))),
            Some(K::ElementAccessExpression) => {
                let (access_argument_expression, access_expression) = target_payload!(self, node, as_element_access_expression; node: argument_expression, node: expression);
                u::is_string_or_numeric_literal_like_kind(
                    self.node_kind(need(access_argument_expression)),
                ) || self.target_is_entity_name_expression(need(access_argument_expression))
                    && self.is_narrowable_reference(need(access_expression))
            }
            Some(K::BinaryExpression) => {
                let (binary_operator_token, binary_right, binary_left) = target_payload!(self, node, as_binary_expression; node: operator_token, node: right, node: left);
                let operator = self.node_kind(need(binary_operator_token));
                operator == K::CommaToken && self.is_narrowable_reference(need(binary_right))
                    || ts_ast::is_assignment_operator(operator)
                        && self.target_is_left_hand_side_expression(need(binary_left))
            }
            _ => false,
        }
    }
    // port: tsc/internal/binder/binder.go:hasNarrowableArgument
    pub(crate) fn has_narrowable_argument(&self, node: BindingNode<'scope>) -> bool {
        let (call_arguments, call_expression) =
            target_payload!(self, node, as_call_expression; list: arguments, node: expression);
        let arguments = self.target_list_edges(Some(need(call_arguments)));
        for index in 0..arguments.len() {
            if self.contains_narrowable_reference(need(self.edge(arguments, index))) {
                return true;
            }
        }
        self.node_kind(need(call_expression)) == K::PropertyAccessExpression
            && self.contains_narrowable_reference(need(self.node_expression(need(call_expression))))
    }
    // port: tsc/internal/binder/binder.go:isNarrowingBinaryExpression
    pub(crate) fn is_narrowing_binary_expression(&self, node: BindingNode<'scope>) -> bool {
        let (binary_left, binary_right, binary_operator_token) = target_payload!(self, node, as_binary_expression; node: left, node: right, node: operator_token);
        let left = need(binary_left);
        let right = need(binary_right);
        match self.node_kind(need(binary_operator_token)).known() {
            Some(
                K::EqualsToken
                | K::BarBarEqualsToken
                | K::AmpersandAmpersandEqualsToken
                | K::QuestionQuestionEqualsToken,
            ) => self.contains_narrowable_reference(left),
            Some(
                K::EqualsEqualsToken
                | K::ExclamationEqualsToken
                | K::EqualsEqualsEqualsToken
                | K::ExclamationEqualsEqualsToken,
            ) => {
                let left = self.target_skip_parentheses(left);
                let right = self.target_skip_parentheses(right);
                self.is_narrowable_operand(left)
                    || self.is_narrowable_operand(right)
                    || self.is_narrowing_type_of_operands(right, left)
                    || self.is_narrowing_type_of_operands(left, right)
                    || u::is_boolean_literal_kind(self.node_kind(right))
                        && self.is_narrowing_expression(left)
                    || u::is_boolean_literal_kind(self.node_kind(left))
                        && self.is_narrowing_expression(right)
            }
            Some(K::InstanceOfKeyword) => self.is_narrowable_operand(left),
            Some(K::InKeyword | K::CommaToken) => self.is_narrowing_expression(right),
            _ => false,
        }
    }
    // port: tsc/internal/binder/binder.go:isNarrowableOperand
    pub(crate) fn is_narrowable_operand(&self, node: BindingNode<'scope>) -> bool {
        match self.node_kind(node).known() {
            Some(K::ParenthesizedExpression) => {
                return self.is_narrowable_operand(need(self.node_expression(node)))
            }
            Some(K::BinaryExpression) => {
                let (binary_operator_token, binary_left, binary_right) = target_payload!(self, node, as_binary_expression; node: operator_token, node: left, node: right);
                match self.node_kind(need(binary_operator_token)).known() {
                    Some(K::EqualsToken) => return self.is_narrowable_operand(need(binary_left)),
                    Some(K::CommaToken) => return self.is_narrowable_operand(need(binary_right)),
                    _ => {}
                }
            }
            _ => {}
        }
        self.contains_narrowable_reference(node)
    }
    // port: tsc/internal/binder/binder.go:isNarrowingTypeOfOperands
    pub(crate) fn is_narrowing_type_of_operands(
        &self,
        first: BindingNode<'scope>,
        second: BindingNode<'scope>,
    ) -> bool {
        self.node_kind(first) == K::TypeOfExpression
            && self.is_narrowable_operand(need(self.node_expression(first)))
            && u::is_string_literal_like_kind(self.node_kind(second))
    }
    // port: tsc/internal/binder/binder.go:isStatementCondition
    pub(crate) fn is_statement_condition(&self, node: BindingNode<'scope>) -> bool {
        let parent = need(self.node_parent(node));
        match self.node_kind(parent).known() {
            Some(K::IfStatement | K::WhileStatement | K::DoStatement) => {
                self.same_node(self.node_expression(parent), Some(node))
            }
            Some(K::ForStatement) => self.same_node(
                target_payload!(self, parent, as_for_statement; node: condition).0,
                Some(node),
            ),
            Some(K::ConditionalExpression) => self.same_node(
                target_payload!(self, parent, as_conditional_expression; node: condition).0,
                Some(node),
            ),
            _ => false,
        }
    }
    // port: tsc/internal/binder/binder.go:isTopLevelLogicalExpression
    pub(crate) fn is_top_level_logical_expression(&self, mut node: BindingNode<'scope>) -> bool {
        loop {
            let parent = need(self.node_parent(node));
            if self.node_kind(parent) == K::ParenthesizedExpression
                || self.node_kind(parent) == K::PrefixUnaryExpression
                    && target_payload!(self, parent, as_prefix_unary_expression; scalar: operator).0
                        == K::ExclamationToken
            {
                node = parent;
            } else {
                break;
            }
        }
        let parent = need(self.node_parent(node));
        !(self.is_statement_condition(node)
            || self.target_is_logical_expression(parent)
            || self.target_is_optional_chain(parent)
                && self.same_node(self.node_expression(parent), Some(node)))
    }
    // port: tsc/internal/binder/binder.go:isLogicalAssignmentExpression
    pub(crate) fn is_logical_assignment_expression(&self, node: BindingNode<'scope>) -> bool {
        self.target_is_logical_or_coalescing_assignment_expression(
            self.target_skip_parentheses(node),
        )
    }
}
