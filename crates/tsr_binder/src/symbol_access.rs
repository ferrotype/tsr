//! Symbols stay local across declaration work. Raw graph links are imported once
//! when followed; invalid compatibility links retain their original failure point.
use crate::{
    backend::{Backend, ScopedValue},
    checked,
    table_access::BindingTable,
    target::BindingNode,
    Binder,
};
use ts_ast::{
    local_bind::BindSymbol, DeclarationSlice, JsString, NodeId, Symbol, SymbolId, SymbolRead,
    SymbolTableId,
};

#[derive(Clone, Copy, Debug)]
pub(crate) enum BindingSymbol<'scope> {
    Local(BindSymbol<'scope>),
    Checked(SymbolId),
}

impl<'scope> Binder<'_, 'scope, '_> {
    pub(crate) fn binding_symbol(&self, id: SymbolId) -> BindingSymbol<'scope> {
        if let Backend::Local(local) = &self.builder {
            if let Ok(symbol) = local.import_symbol(id) {
                return BindingSymbol::Local(symbol);
            }
        }
        BindingSymbol::Checked(id)
    }
    pub(crate) fn symbol_id(&self, symbol: BindingSymbol<'scope>) -> SymbolId {
        match symbol {
            BindingSymbol::Local(symbol) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                local.symbol_id(symbol)
            }
            BindingSymbol::Checked(id) => id,
        }
    }
    pub(crate) fn same_symbol(
        &self,
        left: Option<BindingSymbol<'scope>>,
        right: Option<BindingSymbol<'scope>>,
    ) -> bool {
        match (left, right) {
            (None, None) => true,
            (Some(BindingSymbol::Local(left)), Some(BindingSymbol::Local(right))) => left == right,
            (Some(left), Some(right)) => self.symbol_id(left) == self.symbol_id(right),
            _ => false,
        }
    }
    pub(crate) fn s_binding(&self, symbol: BindingSymbol<'scope>) -> SymbolRead<'_> {
        match symbol {
            BindingSymbol::Local(symbol) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                local.symbol(symbol)
            }
            BindingSymbol::Checked(id) => self.s(id),
        }
    }
    // port: tsc/internal/binder/binder.go:Binder.newSymbol
    pub(crate) fn new_binding_symbol(
        &mut self,
        flags: u32,
        name: JsString,
    ) -> BindingSymbol<'scope> {
        self.symbol_count = self.symbol_count.wrapping_add(1);
        match &mut self.builder {
            Backend::Local(local) => BindingSymbol::Local(local.new_symbol(flags, name)),
            Backend::Checked(builder) => {
                BindingSymbol::Checked(builder.symbols_mut().push(Symbol::new(flags, name)))
            }
        }
    }
    pub(crate) fn binding_symbol_flags_mut(&mut self, symbol: BindingSymbol<'scope>) -> &mut u32 {
        match symbol {
            BindingSymbol::Local(symbol) => {
                let Backend::Local(local) = &mut self.builder else {
                    unreachable!("local binder scope")
                };
                local.local_symbol_flags_mut(symbol)
            }
            BindingSymbol::Checked(id) => self.symbol_flags_mut(id),
        }
    }
    pub(crate) fn binding_symbol_parent(
        &self,
        symbol: BindingSymbol<'scope>,
    ) -> Option<BindingSymbol<'scope>> {
        self.s_binding(symbol)
            .parent()
            .map(|id| self.binding_symbol(id))
    }
    pub(crate) fn node_binding_symbol(
        &self,
        node: BindingNode<'scope>,
    ) -> Option<BindingSymbol<'scope>> {
        let symbol = match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                return match local.scoped_node_symbol(node) {
                    Ok(symbol) => symbol.map(BindingSymbol::Local),
                    Err(_) => local.local_node_symbol(node).map(BindingSymbol::Checked),
                };
            }
            BindingNode::Checked(id) => self.symbol(id),
        };
        symbol.map(|id| self.binding_symbol(id))
    }
    pub(crate) fn set_binding_node_symbol(
        &mut self,
        node: BindingNode<'scope>,
        symbol: Option<BindingSymbol<'scope>>,
    ) {
        if let (BindingNode::Local(node), ScopedValue::Local(symbol)) = (node, local_symbol(symbol))
        {
            let Backend::Local(local) = &mut self.builder else {
                unreachable!("local binder scope")
            };
            local.set_symbol(node, symbol);
            return;
        }
        let node = self.node_id(node);
        let symbol = symbol.map(|symbol| self.symbol_id(symbol));
        self.set_node_symbol(node, symbol);
    }
    pub(crate) fn set_binding_node_local_symbol(
        &mut self,
        node: BindingNode<'scope>,
        symbol: Option<BindingSymbol<'scope>>,
    ) {
        if let (BindingNode::Local(node), ScopedValue::Local(symbol)) = (node, local_symbol(symbol))
        {
            let Backend::Local(local) = &mut self.builder else {
                unreachable!("local binder scope")
            };
            local.set_local_symbol(node, symbol);
            return;
        }
        let node = self.node_id(node);
        let symbol = symbol.map(|symbol| self.symbol_id(symbol));
        self.set_node_local_symbol(node, symbol);
    }
    pub(crate) fn set_binding_symbol_declarations(
        &mut self,
        symbol: BindingSymbol<'scope>,
        value: DeclarationSlice,
    ) {
        match symbol {
            BindingSymbol::Local(symbol) => {
                let Backend::Local(local) = &mut self.builder else {
                    unreachable!("local binder scope")
                };
                checked(local.local_set_symbol_declarations(symbol, value));
            }
            BindingSymbol::Checked(id) => self.set_symbol_declarations(id, value),
        }
    }
    pub(crate) fn set_binding_symbol_value_declaration(
        &mut self,
        symbol: BindingSymbol<'scope>,
        value: Option<BindingNode<'scope>>,
    ) {
        if let BindingSymbol::Local(symbol) = symbol {
            let value = match value {
                None => ScopedValue::Local(None),
                Some(BindingNode::Local(node)) => ScopedValue::Local(Some(node)),
                Some(BindingNode::Checked(_)) => ScopedValue::Checked,
            };
            if let ScopedValue::Local(value) = value {
                let Backend::Local(local) = &mut self.builder else {
                    unreachable!("local binder scope")
                };
                local.local_set_symbol_value_declaration(symbol, value);
                return;
            }
        }
        let id = self.symbol_id(symbol);
        let value = value.map(|node| self.node_id(node));
        self.set_symbol_value_declaration(id, value);
    }
    pub(crate) fn set_binding_symbol_parent(
        &mut self,
        symbol: BindingSymbol<'scope>,
        value: Option<BindingSymbol<'scope>>,
    ) {
        if let (BindingSymbol::Local(symbol), ScopedValue::Local(value)) =
            (symbol, local_symbol(value))
        {
            let Backend::Local(local) = &mut self.builder else {
                unreachable!("local binder scope")
            };
            local.local_set_symbol_parent(symbol, value);
        } else {
            let id = self.symbol_id(symbol);
            let value = value.map(|value| self.symbol_id(value));
            self.set_symbol_parent(id, value);
        }
    }
    pub(crate) fn set_binding_symbol_export_symbol(
        &mut self,
        symbol: BindingSymbol<'scope>,
        value: Option<BindingSymbol<'scope>>,
    ) {
        if let (BindingSymbol::Local(symbol), ScopedValue::Local(value)) =
            (symbol, local_symbol(value))
        {
            let Backend::Local(local) = &mut self.builder else {
                unreachable!("local binder scope")
            };
            local.local_set_symbol_export_symbol(symbol, value);
        } else {
            let id = self.symbol_id(symbol);
            let value = value.map(|value| self.symbol_id(value));
            self.set_symbol_export_symbol(id, value);
        }
    }
    pub(crate) fn set_binding_symbol_members(
        &mut self,
        symbol: BindingSymbol<'scope>,
        value: Option<BindingTable<'scope>>,
    ) {
        if let (BindingSymbol::Local(symbol), ScopedValue::Local(value)) =
            (symbol, super::table_access::local_table(value))
        {
            let Backend::Local(local) = &mut self.builder else {
                unreachable!("local binder scope")
            };
            local.local_set_symbol_members(symbol, value);
        } else {
            let id = self.symbol_id(symbol);
            let value = value.map(|value| self.table_id(value));
            self.set_symbol_members(id, value);
        }
    }
    pub(crate) fn set_binding_symbol_exports(
        &mut self,
        symbol: BindingSymbol<'scope>,
        value: Option<BindingTable<'scope>>,
    ) {
        if let (BindingSymbol::Local(symbol), ScopedValue::Local(value)) =
            (symbol, super::table_access::local_table(value))
        {
            let Backend::Local(local) = &mut self.builder else {
                unreachable!("local binder scope")
            };
            local.local_set_symbol_exports(symbol, value);
        } else {
            let id = self.symbol_id(symbol);
            let value = value.map(|value| self.table_id(value));
            self.set_symbol_exports(id, value);
        }
    }
    // Explicit compatibility entry points for helpers outside this migration.
    pub fn new_symbol(&mut self, flags: u32, name: JsString) -> SymbolId {
        let symbol = self.new_binding_symbol(flags, name);
        self.symbol_id(symbol)
    }
    pub fn s(&self, id: SymbolId) -> SymbolRead<'_> {
        checked(self.builder.symbols().get(id))
    }
    pub fn symbol_flags_mut(&mut self, id: SymbolId) -> &mut u32 {
        checked(self.builder.symbols_mut().flags_mut(id))
    }
    pub fn set_symbol_declarations(&mut self, id: SymbolId, value: DeclarationSlice) {
        checked(self.builder.symbols_mut().set_declarations(id, value));
    }
    pub fn set_symbol_value_declaration(&mut self, id: SymbolId, value: Option<NodeId>) {
        checked(self.builder.symbols_mut().set_value_declaration(id, value));
    }
    pub fn set_symbol_members(&mut self, id: SymbolId, value: Option<SymbolTableId>) {
        checked(self.builder.symbols_mut().set_members(id, value));
    }
    pub fn set_symbol_exports(&mut self, id: SymbolId, value: Option<SymbolTableId>) {
        checked(self.builder.symbols_mut().set_exports(id, value));
    }
    pub fn set_symbol_parent(&mut self, id: SymbolId, value: Option<SymbolId>) {
        checked(self.builder.symbols_mut().set_parent(id, value));
    }
    pub fn set_symbol_export_symbol(&mut self, id: SymbolId, value: Option<SymbolId>) {
        checked(self.builder.symbols_mut().set_export_symbol(id, value));
    }
}

pub(crate) fn local_symbol(value: Option<BindingSymbol<'_>>) -> ScopedValue<BindSymbol<'_>> {
    match value {
        None => ScopedValue::Local(None),
        Some(BindingSymbol::Local(value)) => ScopedValue::Local(Some(value)),
        Some(BindingSymbol::Checked(_)) => ScopedValue::Checked,
    }
}

#[cfg(test)]
#[path = "symbol_access_tests.rs"]
mod tests;
