//! Branded node/value pairs justify skipping repeated owner validation. Cold
//! records, full-width encodings, and binding presence still use shared storage rules.
use super::{super::ColdBindingWrite, BindNode, BindSymbol, BindTable, LocalBind};
use crate::{
    compact::{binding::BindingWrite, CompactContext},
    NodeBinding, SymbolId, SymbolTableId,
};

impl<'scope> LocalBind<'scope, '_> {
    fn binding_context(&self) -> CompactContext<'_> {
        CompactContext {
            nodes: self.result.source.arena(),
            auxiliary: self.core.auxiliary_arena(),
            source: self.core.source(),
            store: self.core.store(),
        }
    }
    fn scoped_inline_binding(&self, node: BindNode<'scope>) -> Option<NodeBinding> {
        let (_, header) = self
            .core
            .resolve_slot(node.word.get())
            .expect("live scoped node");
        let context = self.binding_context();
        context.store.node_binding(header, node.word.get(), context)
    }
    /// The returned ordinary identity is a checked-import boundary: an existing
    /// compatibility record can contain references outside the scoped arena.
    pub fn local_node_symbol(&self, node: BindNode<'scope>) -> Option<SymbolId> {
        if !self.result.bindings.is_empty() {
            if let Some(binding) = self.result.bindings.get(&self.node_id(node)) {
                return binding.symbol;
            }
        }
        let (_, header) = self
            .core
            .resolve_slot(node.word.get())
            .expect("live scoped node");
        let context = self.binding_context();
        context.store.node_symbol(header, node.word.get(), context)
    }
    pub fn local_node_locals(&self, node: BindNode<'scope>) -> Option<SymbolTableId> {
        if !self.result.bindings.is_empty() {
            if let Some(binding) = self.result.bindings.get(&self.node_id(node)) {
                return binding.locals;
            }
        }
        let (_, header) = self
            .core
            .resolve_slot(node.word.get())
            .expect("live scoped node");
        let context = self.binding_context();
        context.store.node_locals(header, node.word.get(), context)
    }
    /// Inline binding words are written only from validated identities while
    /// this core is exclusive. Compatibility records remain checked imports.
    pub fn scoped_node_symbol(
        &self,
        node: BindNode<'scope>,
    ) -> Result<Option<BindSymbol<'scope>>, ts_arena::Error> {
        if self.result.bindings.is_empty() && !self.core.store().has_binding_overrides() {
            let (_, header) = self
                .core
                .resolve_slot(node.word.get())
                .expect("live scoped node");
            match self
                .core
                .store()
                .payloads
                .local_symbol_word(header.actual_shape(), header.ordinal)
            {
                None | Some(0) => return Ok(None),
                Some(u32::MAX) => {}
                Some(word) => return Ok(Some(BindSymbol::from_slot(word))),
            }
        }
        self.local_node_symbol(node)
            .map(|id| self.import_symbol(id))
            .transpose()
    }
    pub fn scoped_node_locals(
        &self,
        node: BindNode<'scope>,
    ) -> Result<Option<BindTable<'scope>>, ts_arena::Error> {
        if self.result.bindings.is_empty() && !self.core.store().has_binding_overrides() {
            let (_, header) = self
                .core
                .resolve_slot(node.word.get())
                .expect("live scoped node");
            match self
                .core
                .store()
                .payloads
                .local_locals_word(header.actual_shape(), header.ordinal)
            {
                None | Some(0) => return Ok(None),
                Some(u32::MAX) => {}
                Some(word) => return Ok(Some(BindTable::from_slot(word))),
            }
        }
        self.local_node_locals(node)
            .map(|id| self.import_table(id))
            .transpose()
    }
    pub fn set_symbol(&mut self, node: BindNode<'scope>, symbol: Option<BindSymbol<'scope>>) {
        self.set_scoped_binding_field(
            node,
            ScopedField::Symbol,
            symbol.map_or(0, BindSymbol::slot),
        );
    }
    pub fn set_local_symbol(&mut self, node: BindNode<'scope>, symbol: Option<BindSymbol<'scope>>) {
        self.set_scoped_binding_field(
            node,
            ScopedField::LocalSymbol,
            symbol.map_or(0, BindSymbol::slot),
        );
    }
    pub fn set_locals(&mut self, node: BindNode<'scope>, table: Option<BindTable<'scope>>) {
        self.set_scoped_binding_field(node, ScopedField::Locals, table.map_or(0, BindTable::slot));
    }
    fn set_scoped_binding_field(&mut self, node: BindNode<'scope>, field: ScopedField, word: u32) {
        // A materialized record supersedes every inline field, including clears.
        if !self.result.bindings.is_empty() || !self.result.flow_bindings.is_empty() {
            let id = self.node_id(node);
            let write = field.checked_write(self, word);
            match self.result.write_existing_binding(id, write) {
                ColdBindingWrite::Applied => return,
                ColdBindingWrite::NeedsMaterialization => {
                    let inline = self.scoped_inline_binding(node);
                    write.apply(self.result.materialize_binding(id, inline));
                    return;
                }
                ColdBindingWrite::Absent => {}
            }
        }
        let local = self
            .core
            .check_slot(node.word.get())
            .expect("live scoped node");
        if word != u32::MAX
            && !self.core.store().has_link_escapes()
            && !self.core.store().has_binding_overrides()
        {
            let (header, store, _) = self.core.node_store_and_source_mut(local);
            let shape = header.actual_shape();
            let ordinal = header.ordinal;
            let stored = match field {
                ScopedField::Symbol => store.payloads.set_symbol(shape, ordinal, word),
                ScopedField::LocalSymbol => store.payloads.set_local_symbol(shape, ordinal, word),
                ScopedField::Locals => store.payloads.set_locals(shape, ordinal, word),
            };
            if stored {
                // All three setters materialize binding presence even on nil.
                header.shape |= 0x8000;
                return;
            }
        }
        // Exceptional encodings and unsupported shapes retain the complete codec
        // and promotion rules; ordinary scoped writes never reconstruct full IDs.
        let id = self.node_id(node);
        let write = field.checked_write(self, word);
        let auxiliary = self.core.auxiliary_arena();
        let (header, store, source) = self.core.node_store_and_source_mut(local);
        if store.write_binding(header, id, auxiliary, source, write) {
            return;
        }
        let inline = self.scoped_inline_binding(node);
        self.result.write_fallback_binding(id, write, inline);
    }
}

#[derive(Clone, Copy)]
enum ScopedField {
    Symbol,
    LocalSymbol,
    Locals,
}
impl ScopedField {
    fn checked_write(self, local: &LocalBind<'_, '_>, word: u32) -> BindingWrite {
        match self {
            Self::Symbol | Self::LocalSymbol => {
                let value = (word != 0).then(|| {
                    SymbolId::from_parts(local.result.symbols.id(), word)
                        .expect("nonzero scoped symbol")
                });
                match self {
                    Self::Symbol => BindingWrite::Symbol(value),
                    _ => BindingWrite::LocalSymbol(value),
                }
            }
            Self::Locals => BindingWrite::Locals((word != 0).then(|| {
                SymbolTableId::from_parts(local.result.tables.id(), word)
                    .expect("nonzero scoped table")
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        AstBuilder, FactoryMethods, FlowNode, JsString, ParsedFile, SourceFileParseOptions,
        SyntaxKind,
    };
    use ts_arena::Counters;
    use ts_jsstring::SourceText;

    fn parsed() -> (ParsedFile, crate::NodeId, crate::NodeId) {
        let text = SourceText::default();
        let mut build = AstBuilder::new(text.clone(), &Counters::new());
        let token = build.new_token(SyntaxKind::Unknown.into());
        let identifier = build.new_identifier(JsString::from_bytes(b"x".as_slice()));
        let source = build.new_source_file(
            SourceFileParseOptions {
                file_name: JsString::from_bytes(b"/binding-state.ts".as_slice()),
                ..Default::default()
            },
            text,
            None,
            None,
        );
        build.node_mut(token).unwrap().set_parent(Some(source));
        build.node_mut(identifier).unwrap().set_parent(Some(source));
        (build.complete(source).unwrap(), token, identifier)
    }

    #[test]
    fn scoped_symbol_writes_keep_inline_presence_and_materialized_precedence() {
        let (parsed, _, _) = parsed();
        parsed
            .bind_and_publish(|builder| {
                builder
                    .with_local_scope(|mut local| {
                        let source = local.source();
                        let raw = local.node_id(source);
                        let symbol = local.new_symbol(0, JsString::default());
                        let table = local.new_table();
                        local.set_symbol(source, Some(symbol));
                        local.set_locals(source, Some(table));
                        assert_eq!(
                            local.local_node_symbol(source),
                            Some(local.symbol_id(symbol))
                        );
                        assert_eq!(local.local_node_locals(source), Some(local.table_id(table)));
                        assert_eq!(local.scoped_node_symbol(source)?, Some(symbol));
                        assert_eq!(local.scoped_node_locals(source)?, Some(table));
                        local.set_symbol(source, None);
                        assert_eq!(local.scoped_node_symbol(source)?, None);
                        assert!(local.binding(raw)?.is_some());
                        assert!(local.result.bindings.is_empty()); // Presence is inline.
                        local.set_local_symbol(source, Some(symbol)); // Cold field on SourceFile.
                        let full_symbol = local.symbol_id(symbol);
                        local.binding_mut(raw)?.symbol = Some(full_symbol);
                        local.binding_mut(raw)?.locals = None;
                        assert_eq!(local.scoped_node_symbol(source)?, Some(symbol));
                        assert_eq!(local.scoped_node_locals(source)?, None);
                        let foreign = crate::SymbolId::from_parts(raw.arena(), 1)?;
                        local.binding_mut(raw)?.symbol = Some(foreign);
                        assert_eq!(
                            local.scoped_node_symbol(source),
                            Err(ts_arena::Error::WrongOwner)
                        );
                        assert_eq!(local.local_node_symbol(source), Some(foreign));
                        local.set_symbol(source, Some(symbol));
                        assert_eq!(local.local_node_symbol(source), Some(full_symbol));
                        assert_eq!(local.local_node_locals(source), None);
                        local.set_symbol(source, None);
                        local.set_locals(source, Some(table));
                        assert_eq!(local.local_node_symbol(source), None);
                        assert_eq!(local.node_symbol(raw)?, None);
                        assert_eq!(local.local_node_locals(source), Some(local.table_id(table)));
                        assert_eq!(local.binding(raw)?.unwrap().local_symbol, Some(full_symbol));
                        assert!(local.result.bindings.contains_key(&raw));
                        Ok(())
                    })
                    .expect("eligible local core")
            })
            .unwrap();
    }

    #[test]
    fn unsupported_shapes_promote_flow_records_and_keep_explicit_empty_bindings() {
        let (parsed, token, _) = parsed();
        parsed
            .bind_and_publish(|builder| {
                builder
                    .with_local_scope(|mut local| {
                        let node = local.import_node(token)?;
                        let flow = local.push_flow(FlowNode::new(crate::flow_flags::START));
                        local.general_set_node_flow(token, Some(flow))?;
                        assert!(local.result.flow_bindings.contains_key(&token));
                        local.set_symbol(node, None);
                        assert!(!local.result.flow_bindings.contains_key(&token));
                        assert_eq!(local.binding(token)?.unwrap().flow_node, Some(flow));
                        assert_eq!(local.local_node_symbol(node), None);
                        local.general_set_node_flow(token, None)?;
                        local.set_locals(node, None);
                        assert!(local.binding(token)?.is_some());
                        assert!(local.result.bindings.contains_key(&token));
                        assert_eq!(local.local_node_locals(node), None);
                        Ok(())
                    })
                    .expect("eligible local core")
            })
            .unwrap();
    }
    #[test]
    fn reentry_rejects_replaced_binding_namespaces_without_aliasing_inline_links() {
        for replace_tables in [true, false] {
            let (parsed, _, identifier) = parsed();
            parsed
                .bind_and_publish(|builder| {
                    let source = builder.source();
                    let (symbol, table, flow) = builder
                        .with_local_scope(|mut local| {
                            let symbol = local.new_symbol(0, JsString::default());
                            let table = local.new_table();
                            let flow = local.new_flow(crate::flow_flags::START, None, None);
                            local.set_symbol(local.source(), Some(symbol));
                            local.set_locals(local.source(), Some(table));
                            assert!(local.set_flow(local.import_node(identifier)?, Some(flow)));
                            Ok((
                                local.symbol_id(symbol),
                                local.table_id(table),
                                local.flow_id(flow),
                            ))
                        })
                        .expect("initial local scope")?;
                    builder
                        .with_local_scope(|local| {
                            assert_eq!(
                                local
                                    .scoped_node_symbol(local.source())?
                                    .map(|id| local.symbol_id(id)),
                                Some(symbol)
                            );
                            assert_eq!(
                                local
                                    .scoped_node_locals(local.source())?
                                    .map(|id| local.table_id(id)),
                                Some(table)
                            );
                            Ok(())
                        })
                        .expect("unchanged namespaces allow reentry")?;
                    let counters = Counters::new();
                    if replace_tables {
                        let old = std::mem::replace(
                            builder.tables_mut(),
                            crate::SymbolTables::new(&counters),
                        );
                        let replacement = builder.tables_mut().alloc(crate::SymbolTable::default());
                        assert_eq!(replacement.slot(), table.slot());
                        assert!(builder
                            .with_local_scope(|_| panic!("replacement table namespace"))
                            .is_none());
                        assert_eq!(builder.node_locals(source)?, Some(table));
                        builder.set_node_locals(source, Some(replacement))?;
                        assert_eq!(builder.node_locals(source)?, Some(replacement));
                        builder.set_node_locals(source, None)?;
                        *builder.tables_mut() = old;
                    } else {
                        let old = std::mem::replace(
                            builder.flows_mut(),
                            crate::FlowNodes::new(&counters),
                        );
                        let replacement = builder
                            .flows_mut()
                            .push(FlowNode::new(crate::flow_flags::UNREACHABLE));
                        assert_eq!(replacement.slot(), flow.slot());
                        assert!(builder
                            .with_local_scope(|_| panic!("replacement flow namespace"))
                            .is_none());
                        assert_eq!(builder.node_flow(identifier)?, Some(flow));
                        builder.set_node_flow(identifier, Some(replacement))?;
                        assert_eq!(builder.node_flow(identifier)?, Some(replacement));
                        builder.set_node_flow(identifier, None)?;
                        *builder.flows_mut() = old;
                    }
                    builder
                        .with_local_scope(|local| {
                            assert_eq!(
                                local
                                    .scoped_node_symbol(local.source())?
                                    .map(|id| local.symbol_id(id)),
                                Some(symbol)
                            );
                            Ok(())
                        })
                        .expect("restored namespaces allow reentry after escaped links cleared")?;
                    Ok(())
                })
                .unwrap();
        }
    }
}
