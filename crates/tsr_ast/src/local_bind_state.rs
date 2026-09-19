//! Binding-state identities are minted by allocation or checked import. The
//! scope owns stable arenas; only individual records can be changed. Raw links
//! returned by compatibility readers must be imported before becoming local.
//!
//! Binding identities cannot escape their fresh scope.
//! ```compile_fail
//! use ts_ast::{BindBuilder, local_bind::BindSymbol};
//! fn escape(builder: &mut BindBuilder<'_>) -> BindSymbol<'static> {
//!     builder.with_local_scope(|mut local| local.new_symbol(0, Default::default())).unwrap()
//! }
//! ```
//!
//! Binding identities from two scopes cannot be exchanged.
//! ```compile_fail
//! use ts_ast::BindBuilder;
//! fn cross(first: &mut BindBuilder<'_>, second: &mut BindBuilder<'_>) {
//!     first.with_local_scope(|mut left| {
//!         let table = left.new_table();
//!         second.with_local_scope(|right| { right.table(table); });
//!     });
//! }
//! ```
//!
//! Flow and flow-list namespaces remain distinct.
//! ```compile_fail
//! use ts_ast::local_bind::{BindFlow, LocalBind};
//! fn namespace<'s>(local: &LocalBind<'s, '_>, flow: BindFlow<'s>) {
//!     local.flow_list(flow);
//! }
//! ```
//!
//! A borrowed state row excludes writes until its final observation.
//! ```compile_fail
//! use ts_ast::local_bind::{BindFlow, LocalBind};
//! fn overlapping<'s>(local: &mut LocalBind<'s, '_>, flow: BindFlow<'s>) {
//!     let read = local.flow(flow);
//!     *local.local_flow_flags_mut(flow) = 1;
//!     assert_eq!(read.flags(), 1);
//! }
//! ```
//!
//! An existing handle cannot be invalidated by replacing its owning table arena.
//! ```compile_fail
//! use ts_ast::local_bind::LocalBind;
//! fn replace(local: &mut LocalBind<'_, '_>) {
//!     let _ = std::mem::replace(local.tables_mut(), ts_ast::SymbolTables::new(&Default::default()));
//! }
//! ```
//!
//! Identities can cross narrow mutations after a read borrow ends.
//! ```
//! use ts_ast::local_bind::{BindFlow, LocalBind};
//! fn separate<'s>(local: &mut LocalBind<'s, '_>, flow: BindFlow<'s>) {
//!     let flags = local.flow(flow).flags();
//!     *local.local_flow_flags_mut(flow) = flags | 1;
//! }
//! ```
use super::{BindFlow, BindNode, Brand, LocalBind};
use crate::{
    DeclarationSlice, FlowData, FlowList, FlowListId, FlowListRead, FlowNode, FlowNodeRead,
    JsString, Symbol, SymbolId, SymbolRead, SymbolTable, SymbolTableId, SymbolTableMut,
    SymbolTableRead,
};
use std::{marker::PhantomData, num::NonZeroU32};
use ts_arena::Error;

macro_rules! state_identity {
    ($name:ident) => {
        /// An identity belonging to the stable binding arena in this scope.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name<'scope> {
            word: NonZeroU32,
            brand: Brand<'scope>,
        }
        impl $name<'_> {
            pub(super) fn from_slot(slot: u32) -> Self {
                Self {
                    word: NonZeroU32::new(slot).expect("allocated nonzero binding slot"),
                    brand: PhantomData,
                }
            }
        }
    };
}
state_identity!(BindSymbol);
state_identity!(BindTable);
state_identity!(BindFlowList);
impl BindSymbol<'_> {
    pub(super) fn slot(self) -> u32 {
        self.word.get()
    }
}
impl BindTable<'_> {
    pub(super) fn slot(self) -> u32 {
        self.word.get()
    }
}

impl<'scope> LocalBind<'scope, '_> {
    pub fn import_symbol(&self, id: SymbolId) -> Result<BindSymbol<'scope>, Error> {
        self.result.symbols().get(id)?;
        Ok(BindSymbol::from_slot(id.slot()))
    }
    pub fn import_table(&self, id: SymbolTableId) -> Result<BindTable<'scope>, Error> {
        self.result.tables.get(id)?;
        Ok(BindTable::from_slot(id.slot()))
    }
    pub fn import_flow_list(&self, id: FlowListId) -> Result<BindFlowList<'scope>, Error> {
        self.result.flow_lists.get(id)?;
        Ok(BindFlowList::from_slot(id.slot()))
    }
    #[inline]
    pub fn symbol_id(&self, id: BindSymbol<'scope>) -> SymbolId {
        SymbolId::from_parts(self.result.symbols.id(), id.word.get()).expect("nonzero symbol")
    }
    #[inline]
    pub fn table_id(&self, id: BindTable<'scope>) -> SymbolTableId {
        SymbolTableId::from_parts(self.result.tables.id(), id.word.get()).expect("nonzero table")
    }
    #[inline]
    pub fn flow_list_id(&self, id: BindFlowList<'scope>) -> FlowListId {
        FlowListId::from_parts(self.result.flow_lists.id(), id.word.get()).expect("nonzero list")
    }
    #[inline]
    pub fn symbol(&self, id: BindSymbol<'scope>) -> SymbolRead<'_> {
        self.result
            .symbols()
            .get_slot(id.word.get())
            .expect("live scoped symbol")
    }
    #[inline]
    pub fn table(&self, id: BindTable<'scope>) -> SymbolTableRead<'_> {
        self.result
            .tables
            .get_slot(id.word.get())
            .expect("live scoped table")
    }
    /// Mutate one table without allowing its owning arena to be replaced. Its
    /// raw symbol values remain explicit checked-import boundaries.
    #[inline]
    pub fn local_table_mut(&mut self, id: BindTable<'scope>) -> SymbolTableMut<'_> {
        self.result
            .tables
            .get_slot_mut(id.word.get())
            .expect("live scoped table")
    }
    #[inline]
    pub fn flow(&self, id: BindFlow<'scope>) -> FlowNodeRead<'_> {
        self.result
            .flows
            .get_slot(id.word.get())
            .expect("live scoped flow")
    }
    #[inline]
    pub fn flow_list(&self, id: BindFlowList<'scope>) -> FlowListRead<'_> {
        self.result
            .flow_lists
            .get_slot(id.word.get())
            .expect("live scoped flow list")
    }
    pub fn new_symbol(&mut self, flags: u32, name: JsString) -> BindSymbol<'scope> {
        let id = self.symbols_mut().push(Symbol::new(flags, name));
        BindSymbol::from_slot(id.slot())
    }
    pub fn new_table(&mut self) -> BindTable<'scope> {
        let id = self.result.tables.alloc(SymbolTable::default());
        BindTable::from_slot(id.slot())
    }
    pub fn new_flow(
        &mut self,
        flags: u32,
        data: Option<FlowData>,
        antecedent: Option<BindFlow<'scope>>,
    ) -> BindFlow<'scope> {
        let antecedent = antecedent.map(|flow| self.flow_id(flow));
        let id = self
            .result
            .flows
            .push(FlowNode::new_ex(flags, data, antecedent));
        BindFlow::from_word(id.slot()).expect("allocated flow")
    }
    pub fn new_flow_list(
        &mut self,
        flow: Option<BindFlow<'scope>>,
        next: Option<BindFlowList<'scope>>,
    ) -> BindFlowList<'scope> {
        let flow = flow.map(|flow| self.flow_id(flow));
        let next = next.map(|next| self.flow_list_id(next));
        let id = self.result.flow_lists.push(FlowList { flow, next });
        BindFlowList::from_slot(id.slot())
    }
    #[inline]
    pub fn local_symbol_flags_mut(&mut self, id: BindSymbol<'scope>) -> &mut u32 {
        self.symbols_mut()
            .flags_slot_mut(id.word.get())
            .expect("live scoped symbol")
    }
    #[inline]
    pub fn local_symbol_check_flags_mut(&mut self, id: BindSymbol<'scope>) -> &mut u32 {
        self.symbols_mut()
            .check_flags_slot_mut(id.word.get())
            .expect("live scoped symbol")
    }
    pub fn local_set_symbol_parent(
        &mut self,
        id: BindSymbol<'scope>,
        value: Option<BindSymbol<'scope>>,
    ) {
        let value = value.map(|value| self.symbol_id(value));
        self.symbols_mut()
            .set_parent_slot(id.word.get(), value)
            .expect("live scoped symbol");
    }
    pub fn local_set_symbol_export_symbol(
        &mut self,
        id: BindSymbol<'scope>,
        value: Option<BindSymbol<'scope>>,
    ) {
        let value = value.map(|value| self.symbol_id(value));
        self.symbols_mut()
            .set_export_symbol_slot(id.word.get(), value)
            .expect("live scoped symbol");
    }
    pub fn local_set_symbol_members(
        &mut self,
        id: BindSymbol<'scope>,
        value: Option<BindTable<'scope>>,
    ) {
        let value = value.map(|value| self.table_id(value));
        self.symbols_mut()
            .set_members_slot(id.word.get(), value)
            .expect("live scoped symbol");
    }
    pub fn local_set_symbol_exports(
        &mut self,
        id: BindSymbol<'scope>,
        value: Option<BindTable<'scope>>,
    ) {
        let value = value.map(|value| self.table_id(value));
        self.symbols_mut()
            .set_exports_slot(id.word.get(), value)
            .expect("live scoped symbol");
    }
    pub fn local_set_symbol_value_declaration(
        &mut self,
        id: BindSymbol<'scope>,
        value: Option<BindNode<'scope>>,
    ) {
        let value = value.map(|value| self.node_id(value));
        self.symbols_mut()
            .set_value_declaration_slot(id.word.get(), value)
            .expect("live scoped symbol");
    }
    /// Declaration backing remains a raw compatibility boundary, with the same
    /// deferred reference validation as SymbolsMut::set_declarations.
    pub fn local_set_symbol_declarations(
        &mut self,
        id: BindSymbol<'scope>,
        value: DeclarationSlice,
    ) -> Result<(), Error> {
        self.symbols_mut()
            .set_declarations_slot(id.word.get(), value)
    }
    #[inline]
    pub fn local_flow_flags_mut(&mut self, id: BindFlow<'scope>) -> &mut u32 {
        self.result
            .flows
            .flags_slot_mut(id.word.get())
            .expect("live scoped flow")
    }
    pub fn local_set_flow_data(&mut self, id: BindFlow<'scope>, value: Option<FlowData>) {
        self.result
            .flows
            .set_node_slot(id.word.get(), value)
            .expect("live scoped flow");
    }
    pub fn local_set_flow_antecedent(
        &mut self,
        id: BindFlow<'scope>,
        value: Option<BindFlow<'scope>>,
    ) {
        let value = value.map(|value| self.flow_id(value));
        self.result
            .flows
            .set_antecedent_slot(id.word.get(), value)
            .expect("live scoped flow");
    }
    pub fn local_set_flow_antecedents(
        &mut self,
        id: BindFlow<'scope>,
        value: Option<BindFlowList<'scope>>,
    ) {
        let value = value.map(|value| self.flow_list_id(value));
        self.result
            .flows
            .set_antecedents_slot(id.word.get(), value)
            .expect("live scoped flow");
    }
    pub fn local_set_flow_list_flow(
        &mut self,
        id: BindFlowList<'scope>,
        value: Option<BindFlow<'scope>>,
    ) {
        let value = value.map(|value| self.flow_id(value));
        self.result
            .flow_lists
            .set_flow_slot(id.word.get(), value)
            .expect("live scoped flow list");
    }
    pub fn local_set_flow_list_next(
        &mut self,
        id: BindFlowList<'scope>,
        value: Option<BindFlowList<'scope>>,
    ) {
        let value = value.map(|value| self.flow_list_id(value));
        self.result
            .flow_lists
            .set_next_slot(id.word.get(), value)
            .expect("live scoped flow list");
    }
    // Raw compatibility setters may install foreign or missing links. These
    // boundaries validate them before minting a scoped identity, even though
    // the target record already belongs to the current scope.
    pub fn flow_antecedent(&self, id: BindFlow<'scope>) -> Result<Option<BindFlow<'scope>>, Error> {
        self.flow(id)
            .antecedent()
            .map(|value| self.import_flow(value))
            .transpose()
    }
    pub fn flow_antecedents(
        &self,
        id: BindFlow<'scope>,
    ) -> Result<Option<BindFlowList<'scope>>, Error> {
        self.flow(id)
            .antecedents()
            .map(|value| self.import_flow_list(value))
            .transpose()
    }
    pub fn flow_list_flow(
        &self,
        id: BindFlowList<'scope>,
    ) -> Result<Option<BindFlow<'scope>>, Error> {
        self.flow_list(id)
            .flow()
            .map(|value| self.import_flow(value))
            .transpose()
    }
    pub fn flow_list_next(
        &self,
        id: BindFlowList<'scope>,
    ) -> Result<Option<BindFlowList<'scope>>, Error> {
        self.flow_list(id)
            .next()
            .map(|value| self.import_flow_list(value))
            .transpose()
    }
}

#[cfg(test)]
#[path = "local_bind_state_tests.rs"]
mod tests;
