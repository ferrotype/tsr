//! Binding scope transitions and allocation-free syntax-child traversal.
use crate::flow_access::BindingFlow;
use crate::{backend::Backend, target::BindingNode};
use crate::{need, Binder, ContainerFlags as C};
use std::ops::ControlFlow;
use ts_arena::Error;
use ts_ast::{
    flow_flags as F, modifier_flags, node_flags as N, symbol_flags as S, utilities as u, AstView,
    ChildVisitor, FlowData, JsString, NodeAccess, NodeDataRead, NodeId, NodeListId, NodeSlice,
    SyntaxKind as K,
};

// port: tsc/internal/binder/binder.go:GetContainerFlags
pub fn get_container_flags(view: AstView<'_>, id: NodeId) -> Result<C, Error> {
    use crate::container_classification::{container_rule, ContainerRule};
    let node = view.node(id)?;
    let rule = container_rule(node.kind());
    let fact = match rule {
        ContainerRule::Fixed(flags) => return Ok(C(flags)),
        ContainerRule::MethodParent => {
            u::is_object_literal_or_class_expression_method_or_accessor(view, id)?
        }
        ContainerRule::PropertyInitializer => node.initializer().is_some(),
        ContainerRule::BlockParent => {
            let parent = view.node(need(node.parent()))?;
            u::is_function_like_kind(parent.kind())
                || parent.kind() == K::ClassStaticBlockDeclaration
        }
    };
    Ok(rule.flags(fact))
}

// FlowNodeData is a payload interface. Open SyntaxKind values can disagree with
// the payload, so this follows the generated Go embedding graph, not kind ranges.
pub(crate) fn has_flow_node_data(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.data(),
        NodeDataRead::Identifier(_)
            | NodeDataRead::QualifiedName(_)
            | NodeDataRead::EmptyStatement(_)
            | NodeDataRead::IfStatement(_)
            | NodeDataRead::DoStatement(_)
            | NodeDataRead::WhileStatement(_)
            | NodeDataRead::ForStatement(_)
            | NodeDataRead::ForInOrOfStatement(_)
            | NodeDataRead::BreakStatement(_)
            | NodeDataRead::ContinueStatement(_)
            | NodeDataRead::ReturnStatement(_)
            | NodeDataRead::WithStatement(_)
            | NodeDataRead::SwitchStatement(_)
            | NodeDataRead::ThrowStatement(_)
            | NodeDataRead::TryStatement(_)
            | NodeDataRead::DebuggerStatement(_)
            | NodeDataRead::LabeledStatement(_)
            | NodeDataRead::ExpressionStatement(_)
            | NodeDataRead::Block(_)
            | NodeDataRead::VariableStatement(_)
            | NodeDataRead::BindingElement(_)
            | NodeDataRead::MissingDeclaration(_)
            | NodeDataRead::FunctionDeclaration(_)
            | NodeDataRead::ClassDeclaration(_)
            | NodeDataRead::InterfaceDeclaration(_)
            | NodeDataRead::TypeAliasDeclaration(_)
            | NodeDataRead::EnumDeclaration(_)
            | NodeDataRead::ModuleBlock(_)
            | NodeDataRead::NotEmittedStatement(_)
            | NodeDataRead::ImportDeclaration(_)
            | NodeDataRead::ExportAssignment(_)
            | NodeDataRead::NamespaceExportDeclaration(_)
            | NodeDataRead::GetAccessorDeclaration(_)
            | NodeDataRead::SetAccessorDeclaration(_)
            | NodeDataRead::MethodDeclaration(_)
            | NodeDataRead::KeywordExpression(_)
            | NodeDataRead::ArrowFunction(_)
            | NodeDataRead::FunctionExpression(_)
            | NodeDataRead::PropertyAccessExpression(_)
            | NodeDataRead::ElementAccessExpression(_)
            | NodeDataRead::MetaProperty(_)
            | NodeDataRead::ModuleDeclaration(_)
            | NodeDataRead::ImportEqualsDeclaration(_)
            | NodeDataRead::ExportDeclaration(_)
    )
}
fn has_body_data(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.data(),
        NodeDataRead::FunctionDeclaration(_)
            | NodeDataRead::ConstructorDeclaration(_)
            | NodeDataRead::GetAccessorDeclaration(_)
            | NodeDataRead::SetAccessorDeclaration(_)
            | NodeDataRead::MethodDeclaration(_)
            | NodeDataRead::ArrowFunction(_)
            | NodeDataRead::FunctionExpression(_)
            | NodeDataRead::ModuleDeclaration(_)
    )
}

// The pinned schema's largest immediate visitor has nine fields (a method).
// Lists are one descriptor regardless of their length. The schema test below
// guards this bound; collection never retains a node, string or list backing.
const MAX_IMMEDIATE_CHILDREN: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImmediateChild {
    Node(NodeId),
    List(NodeListId),
    Slice(NodeSlice),
}

struct ImmediateChildren {
    entries: [Option<ImmediateChild>; MAX_IMMEDIATE_CHILDREN],
    len: usize,
}
impl ImmediateChildren {
    fn of(node: &impl NodeAccess) -> Self {
        let mut children = Self {
            entries: [None; MAX_IMMEDIATE_CHILDREN],
            len: 0,
        };
        let _ = node.for_each_child(&mut children);
        children
    }
    fn push(&mut self, child: ImmediateChild) -> ControlFlow<()> {
        let entry = self
            .entries
            .get_mut(self.len)
            .expect("schema immediate-child bound must include every visitor field");
        *entry = Some(child);
        self.len += 1;
        ControlFlow::Continue(())
    }
    fn visit(&self, visitor: &mut impl ChildVisitor) -> ControlFlow<()> {
        for child in self.entries[..self.len].iter().flatten() {
            match *child {
                ImmediateChild::Node(node) => visitor.visit_node(node)?,
                ImmediateChild::List(list) => visitor.visit_list(list)?,
                ImmediateChild::Slice(nodes) => visitor.visit_node_slice(nodes)?,
            }
        }
        ControlFlow::Continue(())
    }
}
impl ChildVisitor for ImmediateChildren {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.push(ImmediateChild::Node(node))
    }
    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        self.push(ImmediateChild::List(list))
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        self.push(ImmediateChild::Slice(nodes))
    }
}

impl<'scope> Binder<'_, 'scope, '_> {
    pub(crate) fn syntax_slice(&self, list: Option<NodeListId>) -> NodeSlice {
        let parsed = self.parsed_view();
        let nodes = list.map_or_else(NodeSlice::empty, |list| {
            parsed.list(list).expect("retained syntax list").nodes()
        });
        // Preserve validation for allocated empty slices too, whose owner would
        // otherwise never be checked by the indexed loop.
        let _ = parsed.node_slice(nodes).expect("retained syntax slice");
        nodes
    }
    pub(crate) fn syntax_node(&self, nodes: NodeSlice, index: usize) -> Option<NodeId> {
        self.parsed_view()
            .node_slice(nodes)
            .expect("retained syntax slice")
            .at(index)
    }
    #[cfg(test)]
    pub(crate) fn syntax_nodes(&self, list: Option<NodeListId>) -> ts_ast::NodeSliceRead<'_> {
        let parsed = self.parsed_view();
        let nodes = list.map_or_else(NodeSlice::empty, |list| {
            parsed.list(list).expect("retained syntax list").nodes()
        });
        parsed.node_slice(nodes).expect("retained syntax slice")
    }
    // port: tsc/internal/binder/binder.go:Binder.bindContainer
    pub(crate) fn bind_container_target(&mut self, target: BindingNode<'scope>, flags: C) {
        let node = self.node_id(target);
        let flags = flags.0;
        let saved_container = self.container;
        let saved_this = self.this_container;
        let saved_block = self.block_scope_container;
        if flags & C::IS_CONTAINER != 0 {
            self.container = Some(node);
            self.block_scope_container = Some(node);
            if flags & C::HAS_LOCALS != 0 {
                self.add_to_container_chain(node);
            }
        } else if flags & C::IS_BLOCK_SCOPED_CONTAINER != 0 {
            self.block_scope_container = Some(node);
            self.add_to_container_chain(node);
        }
        if flags & C::IS_THIS_CONTAINER != 0 {
            self.this_container = Some(node);
        }
        if flags & C::IS_CONTROL_FLOW_CONTAINER != 0 {
            let saved_flow = self.current_flow;
            let saved_break = self.current_break_target;
            let saved_continue = self.current_continue_target;
            let saved_return = self.current_return_target;
            let saved_exception = self.current_exception_target;
            let saved_labels = self.active_label_list;
            let saved_explicit_return = self.has_explicit_return;
            let saved_seen_this = self.seen_this_keyword;
            let immediately_invoked = flags & C::IS_FUNCTION_EXPRESSION != 0
                && !match target {
                    BindingNode::Local(_) => {
                        self.target_has_syntactic_modifier(target, modifier_flags::ASYNC)
                    }
                    BindingNode::Checked(node) => {
                        u::has_syntactic_modifier(self.view(), node, modifier_flags::ASYNC)
                            .expect("retained function modifiers")
                    }
                }
                && !self.is_generator_function_expression(node)
                && ts_ast::get_immediately_invoked_function_expression(self.view(), node)
                    .expect("retained IIFE")
                    .is_some()
                || self.node_kind(target) == K::ClassStaticBlockDeclaration;
            if !immediately_invoked {
                let start = self.new_flow_node(F::START);
                self.current_flow = Some(start);
                if flags
                    & (C::IS_FUNCTION_EXPRESSION
                        | C::IS_OBJECT_LITERAL_OR_CLASS_EXPRESSION_METHOD_OR_ACCESSOR)
                    != 0
                {
                    self.set_flow_data(start, Some(FlowData::Ast(node)));
                }
            }
            self.current_return_target =
                if immediately_invoked || self.node_kind(target) == K::Constructor {
                    Some(self.new_flow_node(F::BRANCH_LABEL))
                } else {
                    None
                };
            self.current_exception_target = None;
            self.current_break_target = None;
            self.current_continue_target = None;
            self.active_label_list = None;
            self.has_explicit_return = false;
            self.seen_this_keyword = false;
            self.bind_children_target(target);
            let mut node_flags =
                self.node_flags(target) & !(N::REACHABILITY_AND_EMIT_FLAGS | N::CONTAINS_THIS);
            if self.flow(need(self.current_flow)).flags() & F::UNREACHABLE == 0
                && flags & C::IS_FUNCTION_LIKE != 0
                && has_body_data(&self.n(node))
            {
                let body = self.n(node).body();
                if body.is_some_and(|body| ts_ast::node_is_present(Some(&self.n(body)))) {
                    node_flags |= N::HAS_IMPLICIT_RETURN;
                    if self.has_explicit_return {
                        node_flags |= N::HAS_EXPLICIT_RETURN;
                    }
                    self.set_node_end_flow(node, self.current_flow);
                }
            }
            if self.seen_this_keyword {
                node_flags |= N::CONTAINS_THIS;
            }
            if self.node_kind(target) == K::SourceFile {
                node_flags |= self.emit_flags;
            }
            self.set_binding_flags(target, node_flags);
            if let Some(return_target) = self.current_return_target {
                self.add_antecedent(return_target, need(self.current_flow));
                self.current_flow = Some(self.finish_flow_label(return_target));
                if matches!(
                    self.node_kind(target).known(),
                    Some(K::Constructor | K::ClassStaticBlockDeclaration)
                ) {
                    self.set_return_flow_node(node, self.current_flow);
                }
            }
            if !immediately_invoked {
                self.current_flow = saved_flow;
            }
            self.current_break_target = saved_break;
            self.current_continue_target = saved_continue;
            self.current_return_target = saved_return;
            self.current_exception_target = saved_exception;
            self.active_label_list = saved_labels;
            self.has_explicit_return = saved_explicit_return;
            self.seen_this_keyword = if flags & C::PROPAGATES_THIS_KEYWORD != 0 {
                saved_seen_this || self.seen_this_keyword
            } else {
                saved_seen_this
            };
        } else if flags & C::IS_INTERFACE != 0 {
            let saved_seen_this = self.seen_this_keyword;
            self.seen_this_keyword = false;
            self.bind_children_target(target);
            let flags = if self.seen_this_keyword {
                self.node_flags(target) | N::CONTAINS_THIS
            } else {
                self.node_flags(target) & !N::CONTAINS_THIS
            };
            self.set_binding_flags(target, flags);
            self.seen_this_keyword = saved_seen_this;
        } else {
            self.bind_children_target(target);
        }
        if self.node_kind(target) == K::SourceFile
            && self.node_flags(target) & N::JAVA_SCRIPT_FILE != 0
        {
            let statements = self.syntax_slice(Some(need(self.n(node).statement_list())));
            for index in 0..statements.len() {
                let statement = need(self.syntax_node(statements, index));
                if self.n(statement).kind() == K::JSTypeAliasDeclaration {
                    self.bind_block_scoped_declaration(
                        statement,
                        S::TYPE_ALIAS,
                        S::TYPE_ALIAS_EXCLUDES,
                    );
                }
            }
            if self
                .view()
                .source_file(self.file)
                .expect("binder source")
                .common_js_module_indicator()
                .is_some()
            {
                self.declare_common_js_variable(JsString::from_bytes(b"module".as_slice()));
                self.declare_common_js_variable(JsString::from_bytes(b"exports".as_slice()));
            }
        }
        if self.node_kind(target) == K::SourceFile
            && u::is_external_or_common_js_module(
                &self.view().source_file(node).expect("source file"),
            )
            || match target {
                BindingNode::Local(_) => self.target_is_ambient_module(target),
                BindingNode::Checked(node) => {
                    ts_ast::is_ambient_module(self.view(), node).expect("retained module")
                }
            }
        {
            self.bind_common_js_type_exports(need(self.symbol(node)));
        }
        self.container = saved_container;
        self.this_container = saved_this;
        self.block_scope_container = saved_block;
    }
    // port: tsc/internal/binder/binder.go:Binder.bindChildren
    pub(crate) fn bind_children_target(&mut self, target: BindingNode<'scope>) {
        let node = self.node_id(target);
        let saved_pattern = self.in_assignment_pattern;
        self.in_assignment_pattern = false;
        if self.same_flow(self.current_flow, self.unreachable_flow) {
            self.set_target_flow(target, None);
            if ts_ast::is_potentially_executable_node(self.view(), node)
                .expect("retained executable node")
            {
                self.set_binding_flags(target, self.node_flags(target) | N::UNREACHABLE);
            }
            self.bind_each_child_target(target);
            self.in_assignment_pattern = saved_pattern;
            return;
        }
        let kind = self.node_kind(target);
        if kind.raw() >= K::FirstStatement as i16 && kind.raw() <= K::LastStatement as i16 {
            self.set_target_flow(target, self.current_flow);
        }
        match kind.known() {
            Some(K::WhileStatement) => self.bind_while_statement(target),
            Some(K::DoStatement) => self.bind_do_statement(target),
            Some(K::ForStatement) => self.bind_for_statement(target),
            Some(K::ForInStatement | K::ForOfStatement) => {
                self.bind_for_in_or_for_of_statement(target);
            }
            Some(K::IfStatement) => self.bind_if_statement(target),
            Some(K::ReturnStatement) => self.bind_return_statement(target),
            Some(K::ThrowStatement) => self.bind_throw_statement(target),
            Some(K::BreakStatement) => self.bind_break_statement(target),
            Some(K::ContinueStatement) => self.bind_continue_statement(target),
            Some(K::TryStatement) => self.bind_try_statement(target),
            Some(K::SwitchStatement) => self.bind_switch_statement(target),
            Some(K::CaseBlock) => self.bind_case_block(target),
            Some(K::CaseClause | K::DefaultClause) => self.bind_case_or_default_clause(target),
            Some(K::ExpressionStatement) => self.bind_expression_statement(target),
            Some(K::LabeledStatement) => self.bind_labeled_statement(target),
            Some(K::PrefixUnaryExpression) => self.bind_prefix_unary_expression_flow(target),
            Some(K::PostfixUnaryExpression) => self.bind_postfix_unary_expression_flow(target),
            Some(K::BinaryExpression) => {
                if ts_ast::is_destructuring_assignment(self.view(), node)
                    .expect("retained destructuring expression")
                {
                    self.in_assignment_pattern = saved_pattern;
                    self.bind_destructuring_assignment_flow(target);
                    return;
                }
                self.bind_binary_expression_flow(target);
            }
            Some(K::DeleteExpression) => self.bind_delete_expression_flow(target),
            Some(K::ConditionalExpression) => self.bind_conditional_expression_flow(target),
            Some(K::VariableDeclaration) => self.bind_variable_declaration_flow(target),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                self.bind_access_expression_flow(target);
            }
            Some(K::CallExpression) => self.bind_call_expression_flow(target),
            Some(K::NonNullExpression) => self.bind_non_null_expression_flow(target),
            Some(K::SourceFile) => {
                if let BindingNode::Local(node) = target {
                    self.bind_local_source_children(node);
                    self.in_assignment_pattern = saved_pattern;
                    return;
                }
                let n = self.n(node);
                let source = n
                    .data_source()
                    .as_source_file()
                    .expect("SourceFile payload");
                let statements = source.statements();
                let eof = source.end_of_file_token();
                drop(n);
                self.bind_each_statement_functions_first(need(statements));
                self.bind(eof);
            }
            Some(K::Block | K::ModuleBlock) => {
                if let BindingNode::Local(node) = target {
                    self.bind_local_block_children(node);
                    self.in_assignment_pattern = saved_pattern;
                    return;
                }
                self.bind_each_statement_functions_first(need(self.n(node).statement_list()));
            }
            Some(K::BindingElement) => self.bind_binding_element_flow(target),
            Some(K::Parameter) => self.bind_parameter_flow(target),
            Some(
                K::ObjectLiteralExpression
                | K::ArrayLiteralExpression
                | K::PropertyAssignment
                | K::SpreadElement,
            ) => {
                self.in_assignment_pattern = saved_pattern;
                self.bind_each_child_target(target);
            }
            _ => self.bind_each_child_target(target),
        }
        self.in_assignment_pattern = saved_pattern;
    }
    // port: tsc/internal/binder/binder.go:Binder.bindEachChild
    pub(crate) fn bind_each_child_target(&mut self, target: BindingNode<'scope>) {
        let node = match target {
            BindingNode::Local(node) => return self.bind_local_children(node),
            BindingNode::Checked(node) => node,
        };
        // Binding changes flags and binding fields, not these syntax edges or
        // their order. Copy only the immediate descriptors before recursively
        // mutating the exclusive owner; list elements are read when visited.
        let children = ImmediateChildren::of(
            &self
                .parsed_view()
                .node(node)
                .expect("binder syntax is retained"),
        );
        let _ = children.visit(self);
    }
    // port: tsc/internal/binder/binder.go:Binder.bindEachStatementFunctionsFirst
    pub(crate) fn bind_each_statement_functions_first(&mut self, statements: NodeListId) {
        if let Backend::Local(local) = &self.builder {
            if let Ok(statements) = local.import_list(statements) {
                self.bind_local_statements_functions_first(statements);
                return;
            }
        }
        let nodes = self.syntax_slice(Some(statements));
        for index in 0..nodes.len() {
            let node = self.syntax_node(nodes, index);
            if self.n(need(node)).kind() == K::FunctionDeclaration {
                self.bind(node);
            }
        }
        for index in 0..nodes.len() {
            let node = self.syntax_node(nodes, index);
            if self.n(need(node)).kind() != K::FunctionDeclaration {
                self.bind(node);
            }
        }
    }
    // port: tsc/internal/binder/binder.go:setFlowNode
    pub(crate) fn set_flow_node(&mut self, node: NodeId, flow: Option<BindingFlow<'scope>>) {
        self.set_target_flow(self.binding_node(node), flow);
    }
    pub(crate) fn set_target_flow(
        &mut self,
        target: BindingNode<'scope>,
        flow: Option<BindingFlow<'scope>>,
    ) {
        if self.try_set_target_flow(target, flow) {
            return;
        }
        let node = self.node_id(target);
        if has_flow_node_data(&self.n(node)) {
            let flow = flow.map(|flow| self.flow_id(flow));
            self.builder
                .set_node_flow(node, flow)
                .expect("binder flow and target belong to result");
        }
    }
    // port: tsc/internal/binder/binder.go:setReturnFlowNode
    pub(crate) fn set_return_flow_node(&mut self, node: NodeId, flow: Option<BindingFlow<'scope>>) {
        match self.n(node).kind().known() {
            Some(K::Constructor) => {
                self.n(node)
                    .data_source()
                    .as_constructor_declaration()
                    .expect("Constructor payload");
            }
            Some(K::FunctionDeclaration) => {
                self.n(node)
                    .data_source()
                    .as_function_declaration()
                    .expect("FunctionDeclaration payload");
            }
            Some(K::FunctionExpression) => {
                self.n(node)
                    .data_source()
                    .as_function_expression()
                    .expect("FunctionExpression payload");
            }
            Some(K::ClassStaticBlockDeclaration) => {
                self.n(node)
                    .data_source()
                    .as_class_static_block_declaration()
                    .expect("ClassStaticBlockDeclaration payload");
            }
            _ => return,
        }
        self.set_node_return_flow(node, flow);
    }
    // port: tsc/internal/binder/binder.go:isGeneratorFunctionExpression
    pub(crate) fn is_generator_function_expression(&self, node: NodeId) -> bool {
        self.n(node).kind() == K::FunctionExpression
            && self
                .n(node)
                .data_source()
                .as_function_expression()
                .expect("FunctionExpression payload")
                .asterisk_token()
                .is_some()
    }
    // port: tsc/internal/binder/binder.go:Binder.addToContainerChain
    pub(crate) fn add_to_container_chain(&mut self, next: NodeId) {
        if let Some(last) = self.last_container {
            assert!(
                ts_ast::is_locals_container(&self.n(last)),
                "locals-container payload required"
            );
            self.set_node_next_container(last, Some(next));
        }
        self.last_container = Some(next);
    }
}
impl ChildVisitor for Binder<'_, '_, '_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        if self.bind(Some(node)) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        let nodes = self
            .parsed_view()
            .list(list)
            .expect("retained child list")
            .nodes();
        self.visit_node_slice(nodes)
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        let len = self
            .parsed_view()
            .node_slice(nodes)
            .expect("retained child slice")
            .len();
        for index in 0..len {
            let node = self.syntax_node(nodes, index);
            if self.bind(node) {
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_arena::Counters;
    use ts_ast::Node;
    use ts_ast::{
        AstBuilder, FactoryMethods, JSDocParameterOrPropertyTagData, MethodDeclarationData,
    };
    use ts_core::TextRange;
    use ts_jsstring::SourceText;

    #[test]
    fn immediate_child_storage_covers_the_pinned_schema() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../data/s03/schema/ast.json")).unwrap();
        let maximum = schema["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|node| {
                node["members"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|member| member["child"] == true)
                    .count()
            })
            .max()
            .unwrap();
        assert_eq!(maximum, MAX_IMMEDIATE_CHILDREN);
    }

    struct Visits {
        entries: Vec<ImmediateChild>,
        stop_after: usize,
    }
    impl Visits {
        fn push(&mut self, child: ImmediateChild) -> ControlFlow<()> {
            self.entries.push(child);
            if self.entries.len() == self.stop_after {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        }
    }
    impl ChildVisitor for Visits {
        fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
            self.push(ImmediateChild::Node(node))
        }
        fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
            self.push(ImmediateChild::List(list))
        }
        fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
            self.push(ImmediateChild::Slice(nodes))
        }
    }
    fn assert_visits(node: &Node, expected: &[ImmediateChild]) {
        let children = ImmediateChildren::of(node);
        for stop_after in 1..=expected.len() + 1 {
            let mut visits = Visits {
                entries: Vec::new(),
                stop_after,
            };
            let outcome = children.visit(&mut visits);
            assert_eq!(visits.entries, expected[..stop_after.min(expected.len())]);
            assert_eq!(outcome.is_break(), stop_after <= expected.len());
        }
    }

    #[test]
    fn immediate_children_preserve_method_order_and_short_circuit() {
        let mut ast = AstBuilder::new(SourceText::default(), &Counters::new());
        let ids: [_; 6] = std::array::from_fn(|_| ast.new_identifier(JsString::default()));
        let lists: [_; 3] = std::array::from_fn(|_| {
            ast.new_list(TextRange::new(-1, -1), NodeSlice::empty())
                .unwrap()
        });
        let node = Node::from_factory_parts(
            K::MethodDeclaration.into(),
            MethodDeclarationData {
                modifiers: Some(lists[0]),
                asterisk_token: Some(ids[0]),
                name: Some(ids[1]),
                postfix_token: Some(ids[2]),
                type_parameters: Some(lists[1]),
                parameters: Some(lists[2]),
                r#type: Some(ids[3]),
                full_signature: Some(ids[4]),
                body: Some(ids[5]),
            }
            .into(),
        );
        assert_visits(
            &node,
            &[
                ImmediateChild::List(lists[0]),
                ImmediateChild::Node(ids[0]),
                ImmediateChild::Node(ids[1]),
                ImmediateChild::Node(ids[2]),
                ImmediateChild::List(lists[1]),
                ImmediateChild::List(lists[2]),
                ImmediateChild::Node(ids[3]),
                ImmediateChild::Node(ids[4]),
                ImmediateChild::Node(ids[5]),
            ],
        );
    }

    #[test]
    fn immediate_children_preserve_dynamic_jsdoc_order_and_raw_slices() {
        let mut ast = AstBuilder::new(SourceText::default(), &Counters::new());
        let ids: [_; 3] = std::array::from_fn(|_| ast.new_identifier(JsString::default()));
        let comment = ast
            .new_list(TextRange::new(-1, -1), NodeSlice::empty())
            .unwrap();
        for is_name_first in [false, true] {
            let node = Node::from_factory_parts(
                K::JSDocParameterTag.into(),
                JSDocParameterOrPropertyTagData {
                    tag_name: Some(ids[0]),
                    name: Some(ids[1]),
                    type_expression: Some(ids[2]),
                    is_name_first,
                    is_bracketed: false,
                    comment: Some(comment),
                }
                .into(),
            );
            let (first, second) = if is_name_first {
                (ids[1], ids[2])
            } else {
                (ids[2], ids[1])
            };
            assert_visits(
                &node,
                &[
                    ImmediateChild::Node(ids[0]),
                    ImmediateChild::Node(first),
                    ImmediateChild::Node(second),
                    ImmediateChild::List(comment),
                ],
            );
        }
        // A raw empty slice still emits a callback; it is not an absent field.
        let empty = ast.node_slice(Vec::new()).unwrap();
        let node = Node::from_factory_parts(
            K::JSDocTypeLiteral.into(),
            ts_ast::JSDocTypeLiteralData {
                js_doc_property_tags: empty,
                is_array_type: false,
            }
            .into(),
        );
        assert_visits(&node, &[ImmediateChild::Slice(empty)]);
    }
}
