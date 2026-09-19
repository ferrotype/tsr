//! Control-flow graph construction from the pinned binder. Links are checked,
//! non-owning identities in the private binding result.
use crate::flow_access::{BindingFlow, BindingFlowList};
use crate::target::BindingNode;
use crate::{need, Binder};
use ts_ast::{
    flow_flags as F, FlowData, FlowReduceLabelData, FlowSwitchClauseData, SyntaxKind as K,
};

impl<'scope> Binder<'_, 'scope, '_> {
    // port: tsc/internal/binder/binder.go:Binder.newFlowNode
    pub(crate) fn new_flow_node(&mut self, flags: u32) -> BindingFlow<'scope> {
        self.new_flow_node_ex(flags, None, None)
    }
    // port: tsc/internal/binder/binder.go:Binder.createLoopLabel
    pub(crate) fn create_loop_label(&mut self) -> BindingFlow<'scope> {
        self.new_flow_node(F::LOOP_LABEL)
    }
    // port: tsc/internal/binder/binder.go:Binder.createBranchLabel
    pub(crate) fn create_branch_label(&mut self) -> BindingFlow<'scope> {
        self.new_flow_node(F::BRANCH_LABEL)
    }
    // port: tsc/internal/binder/binder.go:Binder.createReduceLabel
    pub(crate) fn create_reduce_label(
        &mut self,
        target: BindingFlow<'scope>,
        antecedents: Option<BindingFlowList<'scope>>,
        antecedent: BindingFlow<'scope>,
    ) -> BindingFlow<'scope> {
        self.new_flow_node_ex(
            F::REDUCE_LABEL,
            Some(FlowData::ReduceLabel(FlowReduceLabelData::new(
                Some(self.flow_id(target)),
                antecedents.map(|list| self.flow_list_id(list)),
            ))),
            Some(antecedent),
        )
    }
    // port: tsc/internal/binder/binder.go:Binder.createFlowCondition
    pub(crate) fn create_flow_condition(
        &mut self,
        flags: u32,
        antecedent: BindingFlow<'scope>,
        expression: Option<BindingNode<'scope>>,
    ) -> BindingFlow<'scope> {
        if self.flow(antecedent).flags() & F::UNREACHABLE != 0 {
            return antecedent;
        }
        let Some(expression) = expression else {
            return if flags & F::TRUE_CONDITION != 0 {
                antecedent
            } else {
                need(self.unreachable_flow)
            };
        };
        let kind = self.node_kind(expression);
        if (kind == K::TrueKeyword && flags & F::FALSE_CONDITION != 0
            || kind == K::FalseKeyword && flags & F::TRUE_CONDITION != 0)
            && !self.target_is_expression_of_optional_chain_root(expression)
            && !self.target_is_nullish_coalesce(need(self.node_parent(expression)))
        {
            return need(self.unreachable_flow);
        }
        if !self.is_narrowing_expression(expression) {
            return antecedent;
        }
        self.set_flow_node_referenced(antecedent);
        self.new_flow_node_ex(
            flags,
            Some(FlowData::Ast(self.node_id(expression))),
            Some(antecedent),
        )
    }
    // port: tsc/internal/binder/binder.go:Binder.createFlowMutation
    pub(crate) fn create_flow_mutation(
        &mut self,
        flags: u32,
        antecedent: BindingFlow<'scope>,
        node: BindingNode<'scope>,
    ) -> BindingFlow<'scope> {
        self.set_flow_node_referenced(antecedent);
        self.has_flow_effects = true;
        let result = self.new_flow_node_ex(
            flags,
            Some(FlowData::Ast(self.node_id(node))),
            Some(antecedent),
        );
        if let Some(target) = self.current_exception_target {
            self.add_antecedent(target, result);
        }
        result
    }
    // port: tsc/internal/binder/binder.go:Binder.createFlowSwitchClause
    pub(crate) fn create_flow_switch_clause(
        &mut self,
        antecedent: BindingFlow<'scope>,
        statement: BindingNode<'scope>,
        start: i64,
        end: i64,
    ) -> BindingFlow<'scope> {
        self.set_flow_node_referenced(antecedent);
        self.new_flow_node_ex(
            F::SWITCH_CLAUSE,
            Some(FlowData::SwitchClause(FlowSwitchClauseData::new(
                Some(self.node_id(statement)),
                start,
                end,
            ))),
            Some(antecedent),
        )
    }
    // port: tsc/internal/binder/binder.go:Binder.createFlowCall
    pub(crate) fn create_flow_call(
        &mut self,
        antecedent: BindingFlow<'scope>,
        node: BindingNode<'scope>,
    ) -> BindingFlow<'scope> {
        self.set_flow_node_referenced(antecedent);
        self.has_flow_effects = true;
        self.new_flow_node_ex(
            F::CALL,
            Some(FlowData::Ast(self.node_id(node))),
            Some(antecedent),
        )
    }
    // port: tsc/internal/binder/binder.go:Binder.combineFlowLists
    pub(crate) fn combine_flow_lists(
        &mut self,
        head: Option<BindingFlowList<'scope>>,
        tail: Option<BindingFlowList<'scope>>,
    ) -> Option<BindingFlowList<'scope>> {
        crate::recursion::guarded(|| self.combine_flow_lists_worker(head, tail))
    }
    fn combine_flow_lists_worker(
        &mut self,
        head: Option<BindingFlowList<'scope>>,
        tail: Option<BindingFlowList<'scope>>,
    ) -> Option<BindingFlowList<'scope>> {
        let Some(head) = head else {
            return tail;
        };
        let head = self.flow_list(head);
        let rest = self.combine_flow_lists(head.next, tail);
        Some(self.new_flow_list(head.flow, rest))
    }
    // port: tsc/internal/binder/binder.go:setFlowNodeReferenced
    pub(crate) fn set_flow_node_referenced(&mut self, flow: BindingFlow<'scope>) {
        let flags = self.flow_flags_mut(flow);
        *flags |= if *flags & F::REFERENCED == 0 {
            F::REFERENCED
        } else {
            F::SHARED
        };
    }
    // port: tsc/internal/binder/binder.go:Binder.addAntecedent
    pub(crate) fn add_antecedent(
        &mut self,
        label: BindingFlow<'scope>,
        antecedent: BindingFlow<'scope>,
    ) {
        if self.flow(antecedent).flags() & F::UNREACHABLE != 0 {
            return;
        }
        let mut last = None;
        let mut list = self.flow_antecedents(label);
        while let Some(id) = list {
            let entry = self.flow_list(id);
            if self.same_flow(entry.flow, Some(antecedent)) {
                return;
            }
            last = Some(id);
            list = entry.next;
        }
        let new = self.new_flow_list(Some(antecedent), None);
        if let Some(last) = last {
            self.set_flow_list_next(last, Some(new));
        } else {
            self.set_flow_antecedents(label, Some(new));
        }
        self.set_flow_node_referenced(antecedent);
    }
    // port: tsc/internal/binder/binder.go:Binder.finishFlowLabel
    pub(crate) fn finish_flow_label(&self, label: BindingFlow<'scope>) -> BindingFlow<'scope> {
        let Some(list) = self.flow_antecedents(label) else {
            return need(self.unreachable_flow);
        };
        let list = self.flow_list(list);
        if list.next.is_none() {
            need(list.flow)
        } else {
            label
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_ast::{AstFile, FlowNode, NodeId, SourceFileParseOptions};
    use ts_core::ScriptKind;
    use ts_jsstring::SourceText;

    fn parse(text: &[u8]) -> (AstFile, NodeId) {
        let parsed = ts_parser::parse_source_file(
            SourceText::from_loaded_bytes(text),
            ScriptKind::TS,
            SourceFileParseOptions {
                file_name: ts_ast::JsString::from_bytes(b"/flow.ts".as_slice()),
                ..SourceFileParseOptions::default()
            },
        );
        let source = parsed.root();
        (parsed.publish_unbound(), source)
    }

    #[test]
    fn flow_labels_preserve_unique_order_reference_flags_and_shared_list_tails() {
        let (file, source) = parse(b"x;");
        file.bind_with(source, |builder| {
            let mut b = Binder::new(builder);
            let unreachable = b.new_flow_node(F::UNREACHABLE);
            b.unreachable_flow = Some(unreachable);
            let start = b.new_flow_node(F::START);
            let first = b.create_branch_label();
            assert_eq!(b.finish_flow_label(first), unreachable);
            b.add_antecedent(first, unreachable);
            assert!(b.flow_antecedents(first).is_none());
            b.add_antecedent(first, start);
            let original = b.flow_antecedents(first).unwrap();
            assert_eq!(b.flow(start).flags(), F::START | F::REFERENCED);
            assert_eq!(b.finish_flow_label(first), start);
            b.add_antecedent(first, start);
            assert_eq!(b.flow_antecedents(first), Some(original));
            assert_eq!(b.flow(start).flags(), F::START | F::REFERENCED);
            let second = b.create_branch_label();
            b.add_antecedent(second, start);
            assert_eq!(b.flow(start).flags(), F::START | F::REFERENCED | F::SHARED);
            let other = b.new_flow_node(F::START);
            b.add_antecedent(first, other);
            assert_eq!(b.finish_flow_label(first), first);
            let next = b.flow_list(original).next.unwrap();
            assert_eq!(b.flow_list(next).flow, Some(other));
            assert_eq!(b.flow_list(next).next, None);
            let tail = b.flow_antecedents(second);
            assert_eq!(b.combine_flow_lists(None, tail), tail);
            let copied = b.combine_flow_lists(Some(original), tail).unwrap();
            assert_ne!(copied, original);
            let copied_next = b.flow_list(copied).next.unwrap();
            assert_ne!(copied_next, next);
            assert_eq!(b.flow_list(copied_next).next, tail);
            assert_eq!(b.flow_list(next).next, None);
            assert_eq!(b.flow_list(copied).flow, Some(start));
            assert_eq!(b.flow_list(copied_next).flow, Some(other));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn mutations_feed_exception_labels_and_synthetic_nodes_preserve_their_edges() {
        let (file, source) = parse(b"x;");
        file.bind_with(source, |builder| {
            let mut b = Binder::new(builder);
            b.unreachable_flow = Some(b.new_flow_node(F::UNREACHABLE));
            let start = b.new_flow_node(F::START);
            let exception = b.create_branch_label();
            b.current_exception_target = Some(exception);
            let mutation = b.create_flow_mutation(F::ASSIGNMENT, start, b.binding_node(source));
            assert!(b.has_flow_effects);
            assert_eq!(b.flow(mutation).node(), Some(FlowData::Ast(source)));
            assert_eq!(b.flow_antecedent(mutation), Some(start));
            assert_eq!(b.finish_flow_label(exception), mutation);
            b.has_flow_effects = false;
            let call = b.create_flow_call(mutation, b.binding_node(source));
            assert!(b.has_flow_effects);
            assert_eq!(b.finish_flow_label(exception), mutation); // Calls do not append exception antecedents.
            let switch = b.create_flow_switch_clause(
                call,
                b.binding_node(source),
                i64::from(i32::MAX) + 1,
                -1,
            );
            let Some(FlowData::SwitchClause(data)) = b.flow(switch).node() else {
                panic!("switch payload")
            };
            assert_eq!(
                (data.switch_statement, data.clause_start, data.clause_end),
                (Some(source), i32::MIN, -1)
            );
            let antecedents = b.flow_antecedents(exception);
            let reduce = b.create_reduce_label(exception, antecedents, switch);
            let Some(FlowData::ReduceLabel(data)) = b.flow(reduce).node() else {
                panic!("reduce payload")
            };
            assert_eq!(
                (data.target, data.antecedents),
                (
                    Some(b.flow_id(exception)),
                    antecedents.map(|list| b.flow_list_id(list))
                )
            );
            assert_eq!(b.flow_antecedent(reduce), Some(switch));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn constant_conditions_distinguish_optional_and_nullish_roots_and_unreachable_entries() {
        let (file, source) = parse(b"x; true; false; false?.x; true ?? x;");
        file.bind_with(source, |builder| {
            let mut b = Binder::new(builder);
            let statements = b.syntax_nodes(b.n(source).statement_list());
            let expression = |index: usize| b.n(need(statements.at(index))).expression().unwrap();
            let (name, yes, no, optional, nullish) = (
                expression(0),
                expression(1),
                expression(2),
                expression(3),
                expression(4),
            );
            let optional_base = need(b.n(optional).expression());
            let nullish_base = need(
                b.n(nullish)
                    .data_source()
                    .as_binary_expression()
                    .unwrap()
                    .left(),
            );
            let unreachable = b.new_flow_node(F::UNREACHABLE);
            b.unreachable_flow = Some(unreachable);
            let start = b.new_flow_node(F::START);
            assert_eq!(
                b.create_flow_condition(F::TRUE_CONDITION, unreachable, Some(b.binding_node(name))),
                unreachable
            );
            assert_eq!(
                b.create_flow_condition(F::TRUE_CONDITION, start, None),
                start
            );
            assert_eq!(
                b.create_flow_condition(F::FALSE_CONDITION, start, None),
                unreachable
            );
            assert_eq!(
                b.create_flow_condition(F::FALSE_CONDITION, start, Some(b.binding_node(yes))),
                unreachable
            );
            assert_eq!(
                b.create_flow_condition(F::TRUE_CONDITION, start, Some(b.binding_node(no))),
                unreachable
            );
            assert_eq!(
                b.create_flow_condition(
                    F::TRUE_CONDITION,
                    start,
                    Some(b.binding_node(optional_base))
                ),
                start
            );
            assert_eq!(
                b.create_flow_condition(
                    F::FALSE_CONDITION,
                    start,
                    Some(b.binding_node(nullish_base))
                ),
                start
            );
            let condition =
                b.create_flow_condition(F::TRUE_CONDITION, start, Some(b.binding_node(name)));
            assert_eq!(
                b.flow(condition).to_owned(),
                FlowNode::new_ex(
                    F::TRUE_CONDITION,
                    Some(FlowData::Ast(name)),
                    Some(b.flow_id(start))
                )
            );
            Ok(())
        })
        .unwrap();
    }
}
