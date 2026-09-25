//! Stable local table identities; nullable entries retain their absent/tombstone distinction.
use crate::{
    backend::{Backend, ScopedValue},
    symbol_access::BindingSymbol,
    target::BindingNode,
    Binder,
};
use tsr_ast::{
    local_bind::BindTable, JsString, SymbolId, SymbolTable, SymbolTableId, SymbolTableRead,
};

#[derive(Clone, Copy, Debug)]
pub(crate) enum BindingTable<'scope> {
    Local(BindTable<'scope>),
    Checked(SymbolTableId),
}

impl<'scope> Binder<'_, 'scope, '_> {
    pub(crate) fn binding_table(&self, id: SymbolTableId) -> BindingTable<'scope> {
        if let Backend::Local(local) = &self.builder {
            if let Ok(table) = local.import_table(id) {
                return BindingTable::Local(table);
            }
        }
        BindingTable::Checked(id)
    }
    pub(crate) fn table_id(&self, table: BindingTable<'scope>) -> SymbolTableId {
        match table {
            BindingTable::Local(table) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                local.table_id(table)
            }
            BindingTable::Checked(id) => id,
        }
    }
    pub(crate) fn table_binding(&self, table: BindingTable<'scope>) -> SymbolTableRead<'_> {
        match table {
            BindingTable::Local(table) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                local.table(table)
            }
            BindingTable::Checked(id) => self.table(id),
        }
    }
    pub(crate) fn new_binding_table(&mut self) -> BindingTable<'scope> {
        match &mut self.builder {
            Backend::Local(local) => BindingTable::Local(local.new_table()),
            Backend::Checked(builder) => {
                BindingTable::Checked(builder.tables_mut().alloc(SymbolTable::default()))
            }
        }
    }
    #[allow(
        clippy::option_option,
        reason = "absent table keys and present nil entries have distinct source semantics"
    )]
    pub(crate) fn binding_table_get(
        &self,
        table: BindingTable<'scope>,
        name: &[u8],
    ) -> Option<Option<BindingSymbol<'scope>>> {
        self.table_binding(table)
            .get(name)
            .map(|entry| entry.map(|id| self.binding_symbol(id)))
    }
    #[allow(
        clippy::option_option,
        reason = "absent table keys and present nil entries have distinct source semantics"
    )]
    pub(crate) fn binding_table_insert(
        &mut self,
        table: BindingTable<'scope>,
        name: JsString,
        symbol: Option<BindingSymbol<'scope>>,
    ) -> Option<Option<BindingSymbol<'scope>>> {
        let symbol = symbol.map(|symbol| self.symbol_id(symbol));
        let previous = match table {
            BindingTable::Local(table) => {
                let Backend::Local(local) = &mut self.builder else {
                    unreachable!("local binder scope")
                };
                local.local_table_mut(table).insert(name, symbol)
            }
            BindingTable::Checked(id) => self.table_mut(id).insert(name, symbol),
        };
        previous.map(|entry| entry.map(|id| self.binding_symbol(id)))
    }
    /// GetExports: the symbol's exports, created and stored on first use.
    pub(crate) fn ensure_binding_exports(
        &mut self,
        symbol: BindingSymbol<'scope>,
    ) -> BindingTable<'scope> {
        if let Some(table) = self.s_binding(symbol).exports() {
            return self.binding_table(table);
        }
        let table = self.new_binding_table();
        // port: tsc/internal/ast/utilities.go:GetExports
        self.set_binding_symbol_exports(symbol, Some(table));
        table
    }
    /// GetMembers: the symbol's members, created and stored on first use.
    pub(crate) fn ensure_binding_members(
        &mut self,
        symbol: BindingSymbol<'scope>,
    ) -> BindingTable<'scope> {
        if let Some(table) = self.s_binding(symbol).members() {
            return self.binding_table(table);
        }
        let table = self.new_binding_table();
        // port: tsc/internal/ast/utilities.go:GetMembers
        self.set_binding_symbol_members(symbol, Some(table));
        table
    }
    pub(crate) fn target_has_locals(&self, node: BindingNode<'scope>) -> bool {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                local.node(node).is_locals_container()
            }
            BindingNode::Checked(id) => tsr_ast::is_locals_container(&self.n(id)),
        }
    }
    /// GetLocals: the container's locals, created and stored on first use
    /// (GetSymbolTable's get-or-create).
    pub(crate) fn ensure_binding_locals(
        &mut self,
        node: BindingNode<'scope>,
    ) -> BindingTable<'scope> {
        match node {
            BindingNode::Local(node) => {
                let Backend::Local(local) = &self.builder else {
                    unreachable!("local binder scope")
                };
                match local.scoped_node_locals(node) {
                    Ok(Some(table)) => return BindingTable::Local(table),
                    Ok(None) => {}
                    Err(_) => {
                        return BindingTable::Checked(
                            local
                                .local_node_locals(node)
                                .expect("failed reference import has a value"),
                        )
                    }
                }
            }
            BindingNode::Checked(id) => {
                if let Some(table) = self.locals(id) {
                    return self.binding_table(table);
                }
            }
        }
        assert!(
            self.target_has_locals(node),
            "locals-container payload required"
        );
        let table = self.new_binding_table();
        if let (BindingNode::Local(node), BindingTable::Local(table)) = (node, table) {
            let Backend::Local(local) = &mut self.builder else {
                unreachable!("local binder scope")
            };
            local.set_locals(node, Some(table));
        } else {
            // port: tsc/internal/ast/utilities.go:GetSymbolTable
            // port: tsc/internal/ast/utilities.go:GetLocals
            self.set_node_locals(self.node_id(node), Some(self.table_id(table)));
        }
        table
    }
    pub fn ensure_exports(&mut self, symbol: SymbolId) -> SymbolTableId {
        let table = self.ensure_binding_exports(self.binding_symbol(symbol));
        self.table_id(table)
    }
    pub fn ensure_members(&mut self, symbol: SymbolId) -> SymbolTableId {
        let table = self.ensure_binding_members(self.binding_symbol(symbol));
        self.table_id(table)
    }
}
pub(crate) fn local_table(value: Option<BindingTable<'_>>) -> ScopedValue<BindTable<'_>> {
    match value {
        None => ScopedValue::Local(None),
        Some(BindingTable::Local(value)) => ScopedValue::Local(Some(value)),
        Some(BindingTable::Checked(_)) => ScopedValue::Checked,
    }
}
