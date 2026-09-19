//! Checked boundaries for binder helpers still using ordinary identities.
//! These preserve side-record precedence and leave syntax-edge validation intact.
use super::{super::ColdBindingWrite, LocalBind};
use crate::compact::binding::BindingWrite;
use crate::symbol_store::{SymbolsMut, SymbolsRead};
use crate::{
    AstView, BindResult, DeclarationLists, Diagnostic, FlowData, FlowId, FlowList, FlowListId,
    FlowLists, FlowNode, FlowNodes, NodeBinding, NodeId, NodeRead, PatternAmbientModule, SymbolId,
    SymbolTable, SymbolTableId, SymbolTableMut, SymbolTables,
};
use ts_arena::Error;

impl LocalBind<'_, '_> {
    pub fn result(&self) -> &BindResult {
        self.result
    }

    pub fn parsed_view(&self) -> AstView<'_> {
        AstView(self.core.view(), None)
    }

    #[inline]
    pub fn general_node(&self, id: NodeId) -> Result<NodeRead<'_>, Error> {
        if id.arena() == self.result.source.arena() {
            let view = self.core.view();
            let header = view.core_node(id)?;
            Ok(NodeRead::core(id, header, view.physical_owner()))
        } else {
            self.view().node(id)
        }
    }

    /// Flags cannot change child edges. Lazy records retain the existing cold
    /// header-copy path; no unrestricted mutable syntax capability is exposed.
    pub fn general_set_node_flags(&mut self, id: NodeId, flags: u32) -> Result<(), Error> {
        if id.arena() == self.result.source.arena() {
            let local = self.core.check(id)?;
            self.core.get_mut(local).set_flags(flags);
            return Ok(());
        }
        self.validate_compatibility_write_owner(id)?;
        let parsed = AstView(self.core.view(), None);
        let node = match self.result.nodes.entry(id) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(parsed.node(id)?.copy_for_binding())
            }
        };
        node.set_flags(flags);
        Ok(())
    }

    pub fn binding(&self, id: NodeId) -> Result<Option<NodeBinding>, Error> {
        self.parsed_view().node(id)?;
        Ok(self.result.node_binding(self.parsed_view(), id))
    }

    pub fn node_symbol(&self, id: NodeId) -> Result<Option<SymbolId>, Error> {
        let node = self.general_node(id)?;
        if let Some(binding) = self.result.bindings.get(&id) {
            return Ok(binding.symbol);
        }
        Ok(if id.arena() == self.result.source.arena() {
            node.inline_symbol()
        } else {
            None
        })
    }

    pub fn node_locals(&self, id: NodeId) -> Result<Option<SymbolTableId>, Error> {
        let node = self.general_node(id)?;
        if let Some(binding) = self.result.bindings.get(&id) {
            return Ok(binding.locals);
        }
        Ok(if id.arena() == self.result.source.arena() {
            node.inline_locals()
        } else {
            None
        })
    }

    pub fn node_flow(&self, id: NodeId) -> Result<Option<FlowId>, Error> {
        let node = self.general_node(id)?;
        if let Some(binding) = self.result.cold_node_binding(id) {
            return Ok(binding.flow_node);
        }
        Ok(if id.arena() == self.result.source.arena() {
            node.inline_flow()
        } else {
            None
        })
    }

    pub fn set_node_symbol(&mut self, id: NodeId, value: Option<SymbolId>) -> Result<(), Error> {
        self.set_compatibility_binding_field(id, BindingWrite::Symbol(value))
    }
    pub fn set_node_local_symbol(
        &mut self,
        id: NodeId,
        value: Option<SymbolId>,
    ) -> Result<(), Error> {
        self.set_compatibility_binding_field(id, BindingWrite::LocalSymbol(value))
    }
    pub fn set_node_locals(
        &mut self,
        id: NodeId,
        value: Option<SymbolTableId>,
    ) -> Result<(), Error> {
        self.set_compatibility_binding_field(id, BindingWrite::Locals(value))
    }
    pub fn set_node_next_container(
        &mut self,
        id: NodeId,
        value: Option<NodeId>,
    ) -> Result<(), Error> {
        self.set_compatibility_binding_field(id, BindingWrite::NextContainer(value))
    }
    pub fn set_node_return_flow(&mut self, id: NodeId, value: Option<FlowId>) -> Result<(), Error> {
        self.set_compatibility_binding_field(id, BindingWrite::ReturnFlow(value))
    }
    pub fn set_node_end_flow(&mut self, id: NodeId, value: Option<FlowId>) -> Result<(), Error> {
        self.set_compatibility_binding_field(id, BindingWrite::EndFlow(value))
    }
    pub fn set_node_fallthrough_flow(
        &mut self,
        id: NodeId,
        value: Option<FlowId>,
    ) -> Result<(), Error> {
        self.set_compatibility_binding_field(id, BindingWrite::FallthroughFlow(value))
    }
    pub fn general_set_node_flow(
        &mut self,
        id: NodeId,
        value: Option<FlowId>,
    ) -> Result<(), Error> {
        self.set_compatibility_binding_field(id, BindingWrite::Flow(value))
    }

    fn set_compatibility_binding_field(
        &mut self,
        id: NodeId,
        write: BindingWrite,
    ) -> Result<(), Error> {
        // Match BindBuilder's public failure order: target ownership/slot,
        // referenced identity, then any mutation or side-record promotion.
        self.validate_compatibility_write_owner(id)?;
        self.result
            .validate_binding_value(self.parsed_view(), write)?;
        match self.result.write_existing_binding(id, write) {
            ColdBindingWrite::Applied => return Ok(()),
            ColdBindingWrite::NeedsMaterialization => {
                write.apply(self.binding_mut(id)?);
                return Ok(());
            }
            ColdBindingWrite::Absent => {}
        }
        if id.arena() == self.result.source.arena() {
            let auxiliary = self.core.auxiliary_arena();
            let local = self.core.check(id)?;
            let (header, store, source) = self.core.node_store_and_source_mut(local);
            if store.write_binding(header, id, auxiliary, source, write) {
                return Ok(());
            }
        }
        let inline = self.general_node(id)?.inline_binding();
        self.result.write_fallback_binding(id, write, inline);
        Ok(())
    }

    pub fn binding_mut(&mut self, id: NodeId) -> Result<&mut NodeBinding, Error> {
        self.validate_compatibility_write_owner(id)?;
        let inline = self.general_node(id)?.inline_binding();
        Ok(self.result.materialize_binding(id, inline))
    }

    fn validate_compatibility_write_owner(&self, id: NodeId) -> Result<(), Error> {
        if id.arena() == self.result.source.arena() {
            self.core.check(id)?;
            return Ok(());
        }
        let owner = self.parsed_view().for_node_owner(id)?;
        if owner.0.id() != self.core.view().id() {
            return Err(Error::WrongOwner);
        }
        // Local-scope entry rejects multiple logical sources. Lazy nodes can
        // still appear during binding and must belong to this physical owner.
        Ok(())
    }

    pub fn symbols(&self) -> SymbolsRead<'_> {
        self.result.symbols()
    }
    pub fn symbols_mut(&mut self) -> SymbolsMut<'_> {
        self.result.symbols.write(&mut self.result.tables)
    }
    pub fn tables(&self) -> &SymbolTables {
        &self.result.tables
    }
    pub fn alloc_table(&mut self, value: SymbolTable) -> SymbolTableId {
        self.result.tables.alloc(value)
    }
    pub fn table_mut(&mut self, id: SymbolTableId) -> Result<SymbolTableMut<'_>, Error> {
        self.result.tables.get_mut(id)
    }
    pub fn declarations(&self) -> &DeclarationLists {
        &self.result.declarations
    }
    pub fn declarations_mut(&mut self) -> &mut DeclarationLists {
        &mut self.result.declarations
    }
    pub fn flows(&self) -> &FlowNodes {
        &self.result.flows
    }
    pub fn flow_lists(&self) -> &FlowLists {
        &self.result.flow_lists
    }

    // Keep branded flow identities valid for the entire scope: grant individual
    // operations without allowing replacement of the result's owning arenas.
    pub fn push_flow(&mut self, value: FlowNode) -> FlowId {
        self.result.flows.push(value)
    }
    pub fn flow_flags_mut(&mut self, id: FlowId) -> Result<&mut u32, Error> {
        self.result.flows.flags_mut(id)
    }
    pub fn set_flow_data(&mut self, id: FlowId, value: Option<FlowData>) -> Result<(), Error> {
        self.result.flows.set_node(id, value)
    }
    pub fn set_flow_antecedents(
        &mut self,
        id: FlowId,
        value: Option<FlowListId>,
    ) -> Result<(), Error> {
        self.result.flows.set_antecedents(id, value)
    }
    pub fn push_flow_list(&mut self, value: FlowList) -> FlowListId {
        self.result.flow_lists.push(value)
    }
    pub fn set_flow_list_next(
        &mut self,
        id: FlowListId,
        value: Option<FlowListId>,
    ) -> Result<(), Error> {
        self.result.flow_lists.set_next(id, value)
    }
    pub fn pattern_ambient_modules_mut(&mut self) -> &mut Vec<PatternAmbientModule> {
        &mut self.result.pattern_ambient_modules
    }
    pub fn diagnostics_mut(&mut self) -> &mut Vec<Diagnostic> {
        &mut self.result.diagnostics
    }
    pub fn set_symbol_count(&mut self, count: isize) {
        self.result.symbol_count = count;
    }
    pub fn set_common_js_module_indicator(&mut self, node: Option<NodeId>) {
        self.result.common_js_module_indicator = node;
    }
    pub fn set_global_exports(&mut self, table: Option<SymbolTableId>) {
        self.result.global_exports = table;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AstBuilder, FactoryMethods, JsString, ParsedFile, SourceFileParseOptions, SyntaxKind,
    };
    use ts_arena::Counters;
    use ts_core::TextRange;
    use ts_jsstring::SourceText;

    fn parsed(counters: &Counters) -> (ParsedFile, NodeId, NodeId, NodeId) {
        let text = SourceText::from_loaded_bytes(b"x".as_slice());
        let mut builder = AstBuilder::new(text.clone(), counters);
        let identifier = builder.new_identifier(JsString::from_bytes(b"x".as_slice()));
        let token = builder.new_token(SyntaxKind::Unknown.into());
        let statement = builder.new_expression_statement(Some(identifier));
        let nodes = builder.node_slice(vec![Some(statement)]).unwrap();
        let list = builder.new_list(TextRange::new(0, 1), nodes).unwrap();
        let source = builder.new_source_file(
            SourceFileParseOptions {
                file_name: JsString::from_bytes(b"/local-binding.ts".as_slice()),
                ..SourceFileParseOptions::default()
            },
            text,
            Some(list),
            None,
        );
        builder
            .node_mut(identifier)
            .unwrap()
            .set_parent(Some(statement));
        builder
            .node_mut(statement)
            .unwrap()
            .set_parent(Some(source));
        (builder.complete(source).unwrap(), source, identifier, token)
    }

    #[test]
    fn compatibility_promotes_flow_only_bindings_and_keeps_cleared_records() {
        let counters = Counters::new();
        let (parsed, _, identifier, token) = parsed(&counters);
        let completed = parsed
            .bind_and_publish(|builder| {
                builder
                    .with_local_scope(|mut local| {
                        let flow = local.push_flow(FlowNode::new(crate::flow_flags::START));
                        let branded_flow = local.import_flow(flow)?;
                        let branded_identifier = local.import_node(identifier)?;
                        assert!(local.set_flow(branded_identifier, Some(branded_flow)));
                        assert_eq!(local.binding_mut(identifier)?.flow_node, Some(flow));
                        local.general_set_node_flow(identifier, None)?;
                        assert_eq!(local.node_flow(identifier)?, None);
                        assert!(local.binding(identifier)?.is_some());
                        assert!(local.set_flow(branded_identifier, Some(branded_flow)));
                        assert_eq!(local.node_flow(identifier)?, Some(flow));
                        assert!(local.set_flow(branded_identifier, None));
                        assert_eq!(local.node_flow(identifier)?, None);
                        // The token has no inline binding fields: a lone flow is sparse,
                        // then a next-container write promotes it without dropping flow.
                        local.general_set_node_flow(token, Some(flow))?;
                        assert_eq!(local.result.flow_bindings.get(&token), Some(&flow));
                        local.set_node_next_container(token, Some(identifier))?;
                        assert!(!local.result.flow_bindings.contains_key(&token));
                        let binding = local.binding(token)?.unwrap();
                        assert_eq!(binding.flow_node, Some(flow));
                        assert_eq!(binding.next_container, Some(identifier));
                        local.general_set_node_flow(token, None)?;
                        assert!(local.binding(token)?.is_some());
                        assert_eq!(local.node_flow(token)?, None);
                        Ok(())
                    })
                    .expect("eligible compact source")
            })
            .unwrap();
        assert!(completed.bound_with_local_scope());
    }

    #[test]
    fn scope_selection_records_actual_callback_entry_not_in_place_binding() {
        let counters = Counters::new();
        let (mut parsed, _, _, _) = parsed(&counters);
        // Requesting unrestricted construction invalidates the local-entry
        // proof even if the caller makes no edit. Completion can revalidate it.
        let _ = parsed.builder_mut();
        let completed = parsed
            .bind_and_publish(|builder| {
                assert!(builder
                    .with_local_scope(|_| panic!("ineligible callback"))
                    .is_none());
                Ok(())
            })
            .unwrap();
        assert!(completed.bound_in_place());
        assert!(!completed.bound_with_local_scope());
    }

    #[test]
    fn compatibility_checks_target_before_value_and_does_not_mutate_on_errors() {
        let counters = Counters::new();
        let (parsed, source, identifier, _) = parsed(&counters);
        let mut foreign_flows = FlowNodes::new(&counters);
        let foreign_flow = foreign_flows.push(FlowNode::new(crate::flow_flags::START));
        let foreign_owner =
            ts_arena::StorageBuilder::<ts_arena::Node<()>>::new(Vec::new().into(), &counters);
        let wrong_owner = NodeId::from_parts(foreign_owner.id().arena(), u32::MAX).unwrap();
        let missing = NodeId::from_parts(source.arena(), u32::MAX).unwrap();
        parsed
            .bind_and_publish(|builder| {
                builder
                    .with_local_scope(|mut local| {
                        assert_eq!(
                            local.general_set_node_flow(wrong_owner, Some(foreign_flow)),
                            Err(Error::WrongOwner)
                        );
                        assert_eq!(
                            local.general_set_node_flow(missing, Some(foreign_flow)),
                            Err(Error::InvalidSlot)
                        );
                        assert_eq!(
                            local.general_set_node_flow(identifier, Some(foreign_flow)),
                            Err(Error::WrongOwner)
                        );
                        assert!(local.result.bindings.is_empty());
                        assert!(local.result.flow_bindings.is_empty());
                        assert_eq!(local.node_flow(identifier)?, None);
                        Ok(())
                    })
                    .expect("eligible compact source")
            })
            .unwrap();
    }

    #[test]
    fn compatibility_routes_lazy_nodes_created_after_scope_entry() {
        let counters = Counters::new();
        let (parsed, source, identifier, _) = parsed(&counters);
        parsed
            .bind_and_publish(|builder| {
                builder
                    .with_local_scope(|mut local| {
                        let lazy =
                            local
                                .view()
                                .source_jsdoc(source, identifier, |transaction| {
                                    let lazy = transaction
                                        .new_identifier(JsString::from_bytes(b"lazy".as_slice()));
                                    transaction.node_mut(lazy)?.set_parent(Some(identifier));
                                    Ok(vec![lazy])
                                })?[0];
                        assert!(matches!(local.import_node(lazy), Err(Error::WrongOwner)));
                        let parsed_flags = local.parsed_view().node(lazy)?.flags();
                        local.general_set_node_flags(
                            lazy,
                            parsed_flags | crate::node_flags::AMBIENT,
                        )?;
                        assert_eq!(local.parsed_view().node(lazy)?.flags(), parsed_flags);
                        assert_eq!(
                            local.general_node(lazy)?.flags(),
                            parsed_flags | crate::node_flags::AMBIENT
                        );
                        let flow = local.push_flow(FlowNode::new(crate::flow_flags::START));
                        local.general_set_node_flow(lazy, Some(flow))?;
                        assert_eq!(local.node_flow(lazy)?, Some(flow));
                        local.general_set_node_flow(lazy, None)?;
                        assert_eq!(local.node_flow(lazy)?, None);
                        Ok(())
                    })
                    .expect("eligible compact source")
            })
            .unwrap();
    }
}
