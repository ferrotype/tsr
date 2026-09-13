//! Checker-owned lexical scopes used only during type serialization. Their
//! bindings are separate from immutable program bindings; the factory retains
//! every source parent before an edge to it is created.
use crate::{CheckerState, Error};
use ts_ast::{
    node_flags as nf, Factory, FactoryMethods, JsString, NodeBinding, NodeId, RuntimeFactory,
    SymbolId, SymbolTable, SyntaxKind as K,
};

#[derive(Default)]
pub(crate) struct SyntheticScopes {
    pub(crate) bindings: crate::types::Map<NodeId, NodeBinding>,
    pub(crate) signature_kinds: crate::types::Map<NodeId, &'static str>,
}
impl SyntheticScopes {
    #[cfg(any(test, feature = "storage-pilot"))]
    pub(crate) fn census(&self, census: &mut crate::census::Census) {
        census.add(
            "query_links",
            self.bindings.len(),
            self.bindings.allocation_size(),
        );
        census.add(
            "query_links",
            self.signature_kinds.len(),
            self.signature_kinds.allocation_size(),
        );
        // Local names/entries are in CheckerState.tables, counted there once.
    }
}
impl CheckerState {
    /// The native `node.Locals()`/`node.Symbol()` read for either a source node or
    /// a synthetic checker node. Valid factory nodes without bindings return nil.
    pub(crate) fn checker_node_binding(&self, node: NodeId) -> Result<Option<NodeBinding>, Error> {
        if node.arena() == self.factory.id().arena() {
            self.factory.view().node(node)?;
            return Ok(self.synthetic_scopes.bindings.get(&node).copied());
        }
        Ok(self.program()?.bound(node)?.node_binding(node)?)
    }
    // Source: tsc/internal/checker/nodebuilderscopes.go:NodeBuilderImpl.enterNewScope (scope allocation)
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformExpandoAssignment
    #[allow(
        clippy::too_many_arguments,
        reason = "The native scope has independent syntax, parent, bindings and signature-scope classification"
    )]
    pub(crate) fn create_emit_scope(
        &mut self,
        parent: NodeId,
        kind: K,
        name: Option<JsString>,
        symbol: Option<SymbolId>,
        locals: SymbolTable,
        signature_kind: Option<&'static str>,
    ) -> Result<NodeId, Error> {
        self.ast(parent)?.node(parent)?;
        if !matches!(kind, K::Block | K::ModuleDeclaration)
            || kind == K::Block && (name.is_some() || symbol.is_some())
            || kind == K::ModuleDeclaration && (name.is_none() || signature_kind.is_some())
        {
            return Err(ts_arena::Error::InvalidGraph.into());
        }
        if let Some(symbol) = symbol {
            self.symbol(symbol)?;
        }
        for symbol in locals.values().flatten() {
            self.symbol(*symbol)?;
        }
        if parent.arena() != self.factory.id().arena() {
            self.retain_flow_source(parent)?;
        }
        let empty = self.factory.alloc_nodes(Vec::new());
        let list = self
            .factory
            .alloc_list(ts_core::TextRange::new(-1, -1), empty);
        let node = if kind == K::Block {
            self.factory.new_block(Some(list), false)
        } else {
            let name = self
                .factory
                .new_identifier(name.expect("validated namespace name"));
            let body = self.factory.new_module_block(Some(list));
            let node = self.factory.new_module_declaration(
                None,
                K::NamespaceKeyword.into(),
                Some(name),
                None,
                Some(body),
            );
            self.factory.set_node_parent(name, Some(node));
            self.factory.set_node_parent(body, Some(node));
            self.factory.add_node_flags(name, nf::SYNTHESIZED);
            self.factory.add_node_flags(body, nf::SYNTHESIZED);
            node
        };
        self.factory.add_node_flags(node, nf::SYNTHESIZED);
        self.factory.set_node_parent(node, Some(parent));
        let locals = self.tables.alloc(locals);
        self.synthetic_scopes.bindings.insert(
            node,
            NodeBinding {
                symbol,
                locals: Some(locals),
                ..Default::default()
            },
        );
        if let Some(kind) = signature_kind {
            self.synthetic_scopes.signature_kinds.insert(node, kind);
        }
        Ok(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synthetic_scope_bindings_are_local_and_validate_symbols_before_allocating(
    ) -> Result<(), Error> {
        let counters = ts_arena::Counters::new();
        let generation = ts_arena::Generation::new(&counters);
        let identity = ts_arena::CheckerIdentity::new(generation, &counters);
        let owner = std::sync::Arc::new(crate::CheckerOwner::new(
            identity,
            &counters,
            crate::CheckerOptions::default(),
        )?);
        let mut operation = owner.operation()?;
        let state = operation.state_mut();
        let parent = state.factory.new_block(None, false);
        let name = JsString::from_bytes(b"T".as_slice());
        let outer = state.new_symbol(ts_ast::symbol_flags::TYPE_PARAMETER, name.clone())?;
        let inner = state.new_symbol(ts_ast::symbol_flags::TYPE_PARAMETER, name.clone())?;
        let scope = state.create_emit_scope(
            parent,
            K::Block,
            None,
            None,
            [(name.clone(), Some(outer))].into(),
            Some("typeParams"),
        )?;
        let child = state.create_emit_scope(
            scope,
            K::Block,
            None,
            None,
            [(name.clone(), Some(inner))].into(),
            Some("params"),
        )?;
        assert_eq!(state.ast(child)?.node(child)?.parent(), Some(scope));
        assert_eq!(
            state.synthetic_scopes.signature_kinds.get(&child),
            Some(&"params")
        );
        let outer_locals = state.checker_node_binding(scope)?.unwrap().locals.unwrap();
        let inner_locals = state.checker_node_binding(child)?.unwrap().locals.unwrap();
        assert_eq!(
            state.table(outer_locals)?.get(name.as_bytes()),
            Some(Some(outer))
        );
        assert_eq!(
            state.table(inner_locals)?.get(name.as_bytes()),
            Some(Some(inner))
        );
        assert!(state.checker_node_binding(parent)?.is_none());
        let foreign = ts_arena::SymbolArena::<u8>::new(&counters);
        let foreign_symbol = SymbolId::from_parts(foreign.id(), 1)?;
        let count = state.factory.node_count();
        assert!(state
            .create_emit_scope(
                child,
                K::Block,
                None,
                None,
                [(name, Some(foreign_symbol))].into(),
                Some("params")
            )
            .is_err());
        assert_eq!(state.factory.node_count(), count);
        Ok(())
    }
}
