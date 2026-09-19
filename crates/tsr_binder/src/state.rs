use crate::flow_access::BindingFlow;
use std::collections::HashSet;
use tsr_ast::{
    AstView, BindBuilder, JsString, NodeId, NodeRead, SymbolId, SymbolTable, SymbolTableId,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct ContainerFlags(pub i32);
impl ContainerFlags {
    pub const NONE: i32 = 0;
    pub const IS_CONTAINER: i32 = 1 << 0;
    pub const IS_BLOCK_SCOPED_CONTAINER: i32 = 1 << 1;
    pub const IS_CONTROL_FLOW_CONTAINER: i32 = 1 << 2;
    pub const IS_FUNCTION_LIKE: i32 = 1 << 3;
    pub const IS_FUNCTION_EXPRESSION: i32 = 1 << 4;
    pub const HAS_LOCALS: i32 = 1 << 5;
    pub const IS_INTERFACE: i32 = 1 << 6;
    pub const IS_OBJECT_LITERAL_OR_CLASS_EXPRESSION_METHOD_OR_ACCESSOR: i32 = 1 << 7;
    pub const IS_THIS_CONTAINER: i32 = 1 << 8;
    pub const PROPAGATES_THIS_KEYWORD: i32 = 1 << 9;
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExpandoAssignmentInfo {
    pub node: NodeId,
    pub container: Option<NodeId>,
    pub block_scope_container: Option<NodeId>,
}
#[derive(Debug)]
pub(crate) struct ActiveLabel<'scope> {
    pub next: Option<usize>,
    pub break_target: Option<BindingFlow<'scope>>,
    pub continue_target: Option<BindingFlow<'scope>>,
    pub name: JsString,
    pub referenced: bool,
}

pub(crate) struct Binder<'build, 'scope, 'ast> {
    pub builder: crate::backend::Backend<'build, 'scope, 'ast>,
    pub file: NodeId,
    pub unreachable_flow: Option<BindingFlow<'scope>>,
    pub container: Option<NodeId>,
    pub this_container: Option<NodeId>,
    pub block_scope_container: Option<NodeId>,
    pub last_container: Option<NodeId>,
    pub current_flow: Option<BindingFlow<'scope>>,
    pub current_break_target: Option<BindingFlow<'scope>>,
    pub current_continue_target: Option<BindingFlow<'scope>>,
    pub current_return_target: Option<BindingFlow<'scope>>,
    pub current_true_target: Option<BindingFlow<'scope>>,
    pub current_false_target: Option<BindingFlow<'scope>>,
    pub current_exception_target: Option<BindingFlow<'scope>>,
    pub pre_switch_case_flow: Option<BindingFlow<'scope>>,
    pub active_label_list: Option<usize>,
    pub labels: Vec<ActiveLabel<'scope>>,
    pub emit_flags: u32,
    pub seen_this_keyword: bool,
    pub has_explicit_return: bool,
    pub has_flow_effects: bool,
    pub in_assignment_pattern: bool,
    pub seen_parse_error: bool,
    pub source_has_parse_errors: bool,
    pub source_is_external_module: bool,
    pub symbol_count: isize,
    pub not_const_enum_only_modules: HashSet<SymbolId>,
    pub expando_assignments: Vec<ExpandoAssignmentInfo>,
}
impl<'build, 'scope, 'ast> Binder<'build, 'scope, 'ast> {
    pub fn new(builder: &'build mut BindBuilder<'ast>) -> Self {
        Self::from_backend(crate::backend::Backend::Checked(builder))
    }
    pub fn from_backend(builder: crate::backend::Backend<'build, 'scope, 'ast>) -> Self {
        let file = builder.source();
        let source = builder
            .view()
            .source_file(file)
            .expect("validated binding source");
        let source_has_parse_errors = !source.diagnostics.is_empty();
        let source_is_external_module = source.external_module_indicator.is_some();
        Self {
            builder,
            file,
            unreachable_flow: None,
            container: None,
            this_container: None,
            block_scope_container: None,
            last_container: None,
            current_flow: None,
            current_break_target: None,
            current_continue_target: None,
            current_return_target: None,
            current_true_target: None,
            current_false_target: None,
            current_exception_target: None,
            pre_switch_case_flow: None,
            active_label_list: None,
            labels: Vec::new(),
            emit_flags: 0,
            seen_this_keyword: false,
            has_explicit_return: false,
            has_flow_effects: false,
            in_assignment_pattern: false,
            seen_parse_error: false,
            source_has_parse_errors,
            source_is_external_module,
            symbol_count: 0,
            not_const_enum_only_modules: HashSet::new(),
            expando_assignments: Vec::new(),
        }
    }
    pub fn parsed_view(&self) -> AstView<'_> {
        self.builder.parsed_view()
    }
    pub fn view(&self) -> AstView<'_> {
        self.builder.view()
    }
    pub fn n(&self, id: NodeId) -> NodeRead<'_> {
        self.builder.node(id).expect("binder node is retained")
    }
    pub fn set_node_symbol(&mut self, id: NodeId, value: Option<SymbolId>) {
        self.builder
            .set_node_symbol(id, value)
            .expect("binder writes its own file");
    }
    pub fn set_node_local_symbol(&mut self, id: NodeId, value: Option<SymbolId>) {
        self.builder
            .set_node_local_symbol(id, value)
            .expect("binder writes its own file");
    }
    pub fn set_node_locals(&mut self, id: NodeId, value: Option<SymbolTableId>) {
        self.builder
            .set_node_locals(id, value)
            .expect("binder writes its own file");
    }
    pub fn set_node_next_container(&mut self, id: NodeId, value: Option<NodeId>) {
        self.builder
            .set_node_next_container(id, value)
            .expect("binder writes its own file");
    }
    pub fn set_node_end_flow(&mut self, id: NodeId, value: Option<BindingFlow<'scope>>) {
        let value = value.map(|flow| self.flow_id(flow));
        self.builder
            .set_node_end_flow(id, value)
            .expect("binder writes its own file");
    }
    pub fn set_node_return_flow(&mut self, id: NodeId, value: Option<BindingFlow<'scope>>) {
        let value = value.map(|flow| self.flow_id(flow));
        self.builder
            .set_node_return_flow(id, value)
            .expect("binder writes its own file");
    }
    pub fn set_node_fallthrough_flow(&mut self, id: NodeId, value: Option<BindingFlow<'scope>>) {
        let value = value.map(|flow| self.flow_id(flow));
        self.builder
            .set_node_fallthrough_flow(id, value)
            .expect("binder writes its own file");
    }
    pub fn symbol(&self, node: NodeId) -> Option<SymbolId> {
        self.builder
            .node_symbol(node)
            .expect("binder node is retained")
    }
    pub fn locals(&self, node: NodeId) -> Option<SymbolTableId> {
        self.builder
            .node_locals(node)
            .expect("binder node is retained")
    }
    pub fn table(&self, table: SymbolTableId) -> tsr_ast::SymbolTableRead<'_> {
        self.builder
            .tables()
            .get(table)
            .expect("binder symbol table belongs to result")
    }
    pub fn table_mut(&mut self, table: SymbolTableId) -> tsr_ast::SymbolTableMut<'_> {
        self.builder
            .table_mut(table)
            .expect("binder symbol table belongs to result")
    }
    // port: tsc/internal/ast/utilities.go:GetLocals
    pub fn ensure_locals(&mut self, node: NodeId) -> SymbolTableId {
        if let Some(table) = self.locals(node) {
            return table;
        }
        assert!(
            tsr_ast::is_locals_container(&self.n(node)),
            "locals-container payload required"
        );
        let table = self.builder.alloc_table(SymbolTable::default());
        self.set_node_locals(node, Some(table));
        table
    }
    pub fn set_flags(&mut self, node: NodeId, flags: u32) {
        if self.n(node).flags() != flags {
            self.builder
                .set_node_flags(node, flags)
                .expect("binder writes its own file");
        }
    }
    pub fn text(&self, node: NodeId) -> JsString {
        self.view()
            .node_text(node)
            .expect("text payload required")
            .into_js_string()
    }
}
