//! The existing binder can cross explicit checked boundaries while its hot
//! paths migrate to local handles. One exclusive scope spans the whole bind.
use ts_arena::Error;
use ts_ast::local_bind::LocalBind;
use ts_ast::{
    AstView, BindBuilder, BindResult, DeclarationLists, Diagnostic, FlowData, FlowId, FlowList,
    FlowListId, FlowLists, FlowNode, FlowNodes, NodeId, NodeRead, PatternAmbientModule, SymbolId,
    SymbolTable, SymbolTableId, SymbolTableMut, SymbolTables, SymbolsMut, SymbolsRead,
};

pub(crate) enum Backend<'build, 'scope, 'ast> {
    Checked(&'build mut BindBuilder<'ast>),
    Local(LocalBind<'scope, 'build>),
}

macro_rules! read_services {
    ($(fn $name:ident($($arg:ident: $ty:ty),*) -> $result:ty;)+) => {$(
        pub fn $name(&self, $($arg: $ty),*) -> $result {
            match self {
                Self::Checked(builder) => builder.$name($($arg),*),
                Self::Local(builder) => builder.$name($($arg),*),
            }
        }
    )+};
}
macro_rules! write_services {
    ($(fn $name:ident($($arg:ident: $ty:ty),*) -> $result:ty;)+) => {$(
        pub fn $name(&mut self, $($arg: $ty),*) -> $result {
            match self {
                Self::Checked(builder) => builder.$name($($arg),*),
                Self::Local(builder) => builder.$name($($arg),*),
            }
        }
    )+};
}

impl Backend<'_, '_, '_> {
    pub fn source(&self) -> NodeId {
        match self {
            Self::Checked(builder) => builder.source(),
            Self::Local(builder) => builder.node_id(builder.source()),
        }
    }
    pub fn node(&self, id: NodeId) -> Result<NodeRead<'_>, Error> {
        match self {
            Self::Checked(builder) => builder.node(id),
            Self::Local(builder) => builder.general_node(id),
        }
    }
    pub fn set_node_flags(&mut self, id: NodeId, flags: u32) -> Result<(), Error> {
        match self {
            Self::Checked(builder) => builder.set_node_flags(id, flags),
            Self::Local(builder) => builder.general_set_node_flags(id, flags),
        }
    }
    pub fn set_node_flow(&mut self, id: NodeId, flow: Option<FlowId>) -> Result<(), Error> {
        match self {
            Self::Checked(builder) => builder.set_node_flow(id, flow),
            Self::Local(builder) => builder.general_set_node_flow(id, flow),
        }
    }
    read_services! {
        fn parsed_view() -> AstView<'_>;
        fn view() -> AstView<'_>;
        fn result() -> &BindResult;
        fn node_symbol(node: NodeId) -> Result<Option<SymbolId>, Error>;
        fn node_locals(node: NodeId) -> Result<Option<SymbolTableId>, Error>;
        fn symbols() -> SymbolsRead<'_>;
        fn tables() -> &SymbolTables;
        fn declarations() -> &DeclarationLists;
        fn flows() -> &FlowNodes;
        fn flow_lists() -> &FlowLists;
    }
    write_services! {
        fn set_node_symbol(node: NodeId, value: Option<SymbolId>) -> Result<(), Error>;
        fn set_node_local_symbol(node: NodeId, value: Option<SymbolId>) -> Result<(), Error>;
        fn set_node_locals(node: NodeId, value: Option<SymbolTableId>) -> Result<(), Error>;
        fn set_node_next_container(node: NodeId, value: Option<NodeId>) -> Result<(), Error>;
        fn set_node_return_flow(node: NodeId, value: Option<FlowId>) -> Result<(), Error>;
        fn set_node_end_flow(node: NodeId, value: Option<FlowId>) -> Result<(), Error>;
        fn set_node_fallthrough_flow(node: NodeId, value: Option<FlowId>) -> Result<(), Error>;
        fn symbols_mut() -> SymbolsMut<'_>;
        fn declarations_mut() -> &mut DeclarationLists;
        fn pattern_ambient_modules_mut() -> &mut Vec<PatternAmbientModule>;
        fn diagnostics_mut() -> &mut Vec<Diagnostic>;
        fn set_symbol_count(count: isize) -> ();
        fn set_common_js_module_indicator(node: Option<NodeId>) -> ();
        fn set_global_exports(table: Option<SymbolTableId>) -> ();
    }
    pub fn alloc_table(&mut self, value: SymbolTable) -> SymbolTableId {
        match self {
            Self::Checked(builder) => builder.tables_mut().alloc(value),
            Self::Local(builder) => builder.alloc_table(value),
        }
    }
    pub fn table_mut(&mut self, id: SymbolTableId) -> Result<SymbolTableMut<'_>, Error> {
        match self {
            Self::Checked(builder) => builder.tables_mut().get_mut(id),
            Self::Local(builder) => builder.table_mut(id),
        }
    }
    pub fn push_flow(&mut self, flow: FlowNode) -> FlowId {
        match self {
            Self::Checked(builder) => builder.flows_mut().push(flow),
            Self::Local(builder) => builder.push_flow(flow),
        }
    }
    pub fn flow_flags_mut(&mut self, flow: FlowId) -> Result<&mut u32, Error> {
        match self {
            Self::Checked(builder) => builder.flows_mut().flags_mut(flow),
            Self::Local(builder) => builder.flow_flags_mut(flow),
        }
    }
    pub fn set_flow_data(&mut self, flow: FlowId, value: Option<FlowData>) -> Result<(), Error> {
        match self {
            Self::Checked(builder) => builder.flows_mut().set_node(flow, value),
            Self::Local(builder) => builder.set_flow_data(flow, value),
        }
    }
    pub fn set_flow_antecedents(
        &mut self,
        flow: FlowId,
        value: Option<FlowListId>,
    ) -> Result<(), Error> {
        match self {
            Self::Checked(builder) => builder.flows_mut().set_antecedents(flow, value),
            Self::Local(builder) => builder.set_flow_antecedents(flow, value),
        }
    }
    pub fn push_flow_list(&mut self, list: FlowList) -> FlowListId {
        match self {
            Self::Checked(builder) => builder.flow_lists_mut().push(list),
            Self::Local(builder) => builder.push_flow_list(list),
        }
    }
    pub fn set_flow_list_next(
        &mut self,
        list: FlowListId,
        next: Option<FlowListId>,
    ) -> Result<(), Error> {
        match self {
            Self::Checked(builder) => builder.flow_lists_mut().set_next(list, next),
            Self::Local(builder) => builder.set_flow_list_next(list, next),
        }
    }
}

/// Distinguishes a local nil/value from a required checked compatibility path.
pub(crate) enum ScopedValue<T> {
    Local(Option<T>),
    Checked,
}
