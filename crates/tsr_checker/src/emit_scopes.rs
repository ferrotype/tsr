//! Checker-owned lexical scopes used only during type serialization. Their
//! bindings are separate from immutable program bindings; the factory retains
//! every source parent before an edge to it is created.
use crate::{CheckerState, Error};
use std::hash::BuildHasher;
use tsr_ast::{
    node_flags as nf, Factory, FactoryMethods, JsString, NodeBinding, NodeId, RuntimeFactory,
    SymbolId, SymbolTable, SyntaxKind as K,
};

#[derive(Default)]
pub(crate) struct SyntheticScopes {
    pub(crate) bindings: crate::types::Map<NodeId, NodeBinding>,
    pub(crate) signature_kinds: crate::types::Map<NodeId, &'static str>,
    /// Narrow candidates by their complete creation context. Exact live-table
    /// comparison below handles hash collisions and temporary signature locals.
    created: crate::types::Map<ScopeKey, Vec<NodeId>>,
}

#[derive(PartialEq, Eq, Hash)]
struct ScopeKey {
    parent: NodeId,
    kind: K,
    name: Option<JsString>,
    symbol: Option<SymbolId>,
    signature_kind: Option<&'static str>,
    locals_len: usize,
    locals_hash: u64,
}

/// SymbolTable iteration has no semantic order. Summing entry hashes makes
/// equal tables share a bucket without sorting or cloning their names. The
/// fingerprint is only an index; it never establishes table equality.
fn locals_fingerprint(locals: &SymbolTable) -> u64 {
    let hasher = crate::types::FastState::default();
    locals.iter().fold(0u64, |hash, entry| {
        hash.wrapping_add(hasher.hash_one(entry))
    })
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
        census.add(
            "query_links",
            self.created.len(),
            self.created.allocation_size()
                + self
                    .created
                    .values()
                    .map(|nodes| nodes.capacity() * size_of::<NodeId>())
                    .sum::<usize>(),
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
        self.node(parent)?;
        if !matches!(kind, K::Block | K::ModuleDeclaration)
            || kind == K::Block && (name.is_some() || symbol.is_some())
            || kind == K::ModuleDeclaration && (name.is_none() || signature_kind.is_some())
        {
            return Err(tsr_arena::Error::InvalidGraph.into());
        }
        if let Some(symbol) = symbol {
            self.symbol(symbol)?;
        }
        for symbol in locals.values().flatten() {
            self.symbol(*symbol)?;
        }
        // Each emit asks for the same scopes again. The pin's belong to the
        // request's factory and go with it; this checker's storage is never
        // freed, so an identical request gets the scope it had before and
        // repeated emits add nothing.
        let key = ScopeKey {
            parent,
            kind,
            name,
            symbol,
            signature_kind,
            locals_len: locals.len(),
            locals_hash: locals_fingerprint(&locals),
        };
        if let Some(scope) = self.identical_emit_scope(&key, &locals)? {
            return Ok(scope);
        }
        if parent.arena() != self.factory.id().arena() {
            self.retain_flow_source(parent)?;
        }
        let empty = self.factory.alloc_nodes(Vec::new());
        let list = self
            .factory
            .alloc_list(tsr_core::TextRange::new(-1, -1), empty);
        let node = if kind == K::Block {
            self.factory.new_block(Some(list), false)
        } else {
            let name = self
                .factory
                .new_identifier(key.name.clone().expect("validated namespace name"));
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
        self.synthetic_scopes
            .created
            .entry(key)
            .or_default()
            .push(node);
        Ok(node)
    }

    /// A scope created with the same parent, kinds, name and symbol whose
    /// locals are now the requested ones. A signature scope's table is
    /// extended only while a nested signature is serialized and restored
    /// afterwards, so it is compared as it is now.
    fn identical_emit_scope(
        &self,
        key: &ScopeKey,
        locals: &SymbolTable,
    ) -> Result<Option<NodeId>, Error> {
        let Some(created) = self.synthetic_scopes.created.get(key) else {
            return Ok(None);
        };
        for &scope in created {
            let table = self
                .synthetic_scopes
                .bindings
                .get(&scope)
                .and_then(|binding| binding.locals)
                .ok_or(Error::MissingLink("emit scope locals"))?;
            let current = self.tables.get(table)?;
            if current.len() == locals.len()
                && locals
                    .iter()
                    .all(|(name, symbol)| current.get(name.as_bytes()) == Some(*symbol))
            {
                return Ok(Some(scope));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synthetic_scope_bindings_are_local_and_validate_symbols_before_allocating(
    ) -> Result<(), Error> {
        let counters = tsr_arena::Counters::new();
        let generation = tsr_arena::Generation::new(&counters);
        let identity = tsr_arena::CheckerIdentity::new(generation, &counters);
        let owner = std::sync::Arc::new(crate::CheckerOwner::new(
            identity,
            &counters,
            crate::CheckerOptions::default(),
        )?);
        let mut operation = owner.operation()?;
        let state = operation.state_mut();
        let parent = state.factory.new_block(None, false);
        let name = JsString::from_bytes(b"T".as_slice());
        let outer = state.new_symbol(tsr_ast::symbol_flags::TYPE_PARAMETER, name.clone())?;
        let inner = state.new_symbol(tsr_ast::symbol_flags::TYPE_PARAMETER, name.clone())?;
        let scope = state.create_emit_scope(
            parent,
            K::Block,
            None,
            None,
            tsr_ast::SymbolTable::from_iter([(name.clone(), Some(outer))]),
            Some("typeParams"),
        )?;
        let child = state.create_emit_scope(
            scope,
            K::Block,
            None,
            None,
            tsr_ast::SymbolTable::from_iter([(name.clone(), Some(inner))]),
            Some("params"),
        )?;
        assert_eq!(state.node(child)?.parent(), Some(scope));
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
        // Temporary extension must never make a stale creation fingerprint
        // sufficient for reuse. Compare the live table, then reuse the old
        // scope again when its original bindings have been restored.
        state
            .tables
            .get_mut(outer_locals)?
            .insert(name.clone(), Some(inner));
        let replacement = state.create_emit_scope(
            parent,
            K::Block,
            None,
            None,
            SymbolTable::from_iter([(name.clone(), Some(outer))]),
            Some("typeParams"),
        )?;
        assert_ne!(replacement, scope);
        state
            .tables
            .get_mut(outer_locals)?
            .insert(name.clone(), Some(outer));
        assert_eq!(
            state.create_emit_scope(
                parent,
                K::Block,
                None,
                None,
                SymbolTable::from_iter([(name.clone(), Some(outer))]),
                Some("typeParams")
            )?,
            scope
        );

        // Tables with opposite insertion order describe the same scope.
        let other = JsString::from_bytes(b"U".as_slice());
        let locals = [(name.clone(), Some(outer)), (other.clone(), None)];
        let ordered = state.create_emit_scope(
            parent,
            K::Block,
            None,
            None,
            SymbolTable::from_iter(locals.clone()),
            Some("params"),
        )?;
        assert_eq!(
            state.create_emit_scope(
                parent,
                K::Block,
                None,
                None,
                locals.into_iter().rev().collect(),
                Some("params")
            )?,
            ordered
        );

        // A colliding fingerprint still cannot substitute a different table.
        let different = SymbolTable::from_iter([(other, Some(inner))]);
        let collision = ScopeKey {
            parent,
            kind: K::Block,
            name: None,
            symbol: None,
            signature_kind: Some("typeParams"),
            locals_len: different.len(),
            locals_hash: locals_fingerprint(&different),
        };
        state
            .synthetic_scopes
            .created
            .entry(collision)
            .or_default()
            .push(scope);
        let distinct =
            state.create_emit_scope(parent, K::Block, None, None, different, Some("typeParams"))?;
        assert_ne!(distinct, scope);
        assert!(state.checker_node_binding(parent)?.is_none());
        let foreign = tsr_arena::SymbolArena::<u8>::new(&counters);
        let foreign_symbol = SymbolId::from_parts(foreign.id(), 1)?;
        let count = state.factory.node_count();
        assert!(state
            .create_emit_scope(
                child,
                K::Block,
                None,
                None,
                tsr_ast::SymbolTable::from_iter([(name, Some(foreign_symbol))]),
                Some("params")
            )
            .is_err());
        assert_eq!(state.factory.node_count(), count);
        Ok(())
    }
}
