//! Binding links use the result's namespaces, independently of syntax arenas.

use super::{CompactContext, CoreStore, FieldKey, FullReference, PackingContext, StoredNode};
use crate::{AstPayloadStore, FlowId, NodeBinding, NodeId, SymbolId, SymbolTableId};
use ts_arena::ArenaId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BindingArenas {
    pub(crate) symbols: ArenaId,
    pub(crate) tables: ArenaId,
    pub(crate) flows: ArenaId,
}

impl CoreStore {
    pub(crate) fn binding_arenas_match(&self, arenas: BindingArenas) -> bool {
        self.binding_arenas == Some(arenas)
    }

    pub(crate) fn has_binding_overrides(&self) -> bool {
        !self.binding_overrides.is_empty()
    }

    pub(crate) fn initialize_binding(&mut self, arenas: BindingArenas) {
        assert!(
            self.binding_arenas.is_none(),
            "exclusive binding initializes once"
        );
        self.binding_arenas = Some(arenas);
    }
}

#[derive(Clone, Copy)]
pub(crate) enum BindingWrite {
    Symbol(Option<SymbolId>),
    LocalSymbol(Option<SymbolId>),
    Locals(Option<SymbolTableId>),
    NextContainer(Option<NodeId>),
    Flow(Option<FlowId>),
    ReturnFlow(Option<FlowId>),
    EndFlow(Option<FlowId>),
    FallthroughFlow(Option<FlowId>),
}

impl BindingWrite {
    pub(crate) fn is_flow(self) -> bool {
        matches!(self, Self::Flow(_))
    }
    pub(crate) fn apply(self, binding: &mut NodeBinding) {
        match self {
            Self::Symbol(value) => binding.symbol = value,
            Self::LocalSymbol(value) => binding.local_symbol = value,
            Self::Locals(value) => binding.locals = value,
            Self::NextContainer(value) => binding.next_container = value,
            Self::Flow(value) => binding.flow_node = value,
            Self::ReturnFlow(value) => binding.return_flow_node = value,
            Self::EndFlow(value) => binding.end_flow_node = value,
            Self::FallthroughFlow(value) => binding.fallthrough_flow_node = value,
        }
    }
    fn key(self, shape: u16, ordinal: u32) -> Option<FieldKey> {
        match self {
            Self::Symbol(_) => AstPayloadStore::symbol_key(shape, ordinal),
            Self::LocalSymbol(_) => AstPayloadStore::local_symbol_key(shape, ordinal),
            Self::Locals(_) => AstPayloadStore::locals_key(shape, ordinal),
            Self::NextContainer(_) => AstPayloadStore::next_container_key(shape, ordinal),
            Self::Flow(_) => AstPayloadStore::flow_node_key(shape, ordinal),
            Self::ReturnFlow(_) => AstPayloadStore::return_flow_node_key(shape, ordinal),
            Self::EndFlow(_) => AstPayloadStore::end_flow_node_key(shape, ordinal),
            Self::FallthroughFlow(_) => AstPayloadStore::fallthrough_flow_node_key(shape, ordinal),
        }
    }
    pub(crate) fn store(
        self,
        payloads: &mut AstPayloadStore,
        shape: u16,
        ordinal: u32,
        context: &mut PackingContext<'_>,
    ) -> bool {
        let Some(key) = self.key(shape, ordinal) else {
            return false;
        };
        match self {
            Self::Symbol(value) => {
                payloads.set_symbol(shape, ordinal, context.encode_symbol(key, value))
            }
            Self::LocalSymbol(value) => {
                payloads.set_local_symbol(shape, ordinal, context.encode_symbol(key, value))
            }
            Self::Locals(value) => {
                payloads.set_locals(shape, ordinal, context.encode_symbol_table(key, value))
            }
            Self::NextContainer(value) => {
                payloads.set_next_container(shape, ordinal, context.encode_node(key, value))
            }
            Self::Flow(value) => {
                payloads.set_flow_node(shape, ordinal, context.encode_flow(key, value))
            }
            Self::ReturnFlow(value) => {
                payloads.set_return_flow_node(shape, ordinal, context.encode_flow(key, value))
            }
            Self::EndFlow(value) => {
                payloads.set_end_flow_node(shape, ordinal, context.encode_flow(key, value))
            }
            Self::FallthroughFlow(value) => {
                payloads.set_fallthrough_flow_node(shape, ordinal, context.encode_flow(key, value))
            }
        }
    }
}

impl CoreStore {
    pub(crate) fn node_binding(
        &self,
        header: &StoredNode,
        slot: u32,
        context: CompactContext<'_>,
    ) -> Option<NodeBinding> {
        let override_binding = if self.binding_overrides.is_empty() {
            None
        } else {
            self.binding_overrides.get(&slot).copied()
        };
        let binding = override_binding.unwrap_or_else(|| {
            self.payloads
                .read_binding(header.actual_shape(), header.ordinal, context)
        });
        (header.shape & 0x8000 != 0 || binding.flow_node.is_some()).then_some(binding)
    }

    pub(crate) fn write_binding(
        &mut self,
        header: &mut StoredNode,
        id: NodeId,
        auxiliary: ArenaId,
        source: &ts_jsstring::SourceText,
        write: BindingWrite,
    ) -> bool {
        let stored = if let Some(binding) = self.binding_overrides.get_mut(&id.slot()) {
            write.apply(binding);
            true
        } else {
            let (payloads, mut context) =
                self.packing_parts(id.arena(), auxiliary, source, header.end);
            write.store(
                payloads,
                header.actual_shape(),
                header.ordinal,
                &mut context,
            )
        };
        if stored && !write.is_flow() {
            header.shape |= 0x8000;
        }
        stored
    }

    /// Unrestricted shape changes preserve binding values, using a cold record
    /// only when the new concrete shape cannot carry an existing field.
    pub(crate) fn restore_binding(
        &mut self,
        header: &mut StoredNode,
        id: NodeId,
        auxiliary: ArenaId,
        source: &ts_jsstring::SourceText,
        binding: Option<NodeBinding>,
    ) {
        if !self.binding_overrides.is_empty() {
            self.binding_overrides.remove(&id.slot());
        }
        let Some(binding) = binding else {
            return;
        };
        let writes = [
            binding.symbol.map(|id| BindingWrite::Symbol(Some(id))),
            binding
                .local_symbol
                .map(|id| BindingWrite::LocalSymbol(Some(id))),
            binding.locals.map(|id| BindingWrite::Locals(Some(id))),
            binding
                .next_container
                .map(|id| BindingWrite::NextContainer(Some(id))),
            binding.flow_node.map(|id| BindingWrite::Flow(Some(id))),
            binding
                .return_flow_node
                .map(|id| BindingWrite::ReturnFlow(Some(id))),
            binding
                .end_flow_node
                .map(|id| BindingWrite::EndFlow(Some(id))),
            binding
                .fallthrough_flow_node
                .map(|id| BindingWrite::FallthroughFlow(Some(id))),
        ];
        if writes
            .iter()
            .flatten()
            .any(|write| write.key(header.actual_shape(), header.ordinal).is_none())
        {
            self.binding_overrides.insert(id.slot(), binding);
            return;
        }
        // Presence is preserved by the header. Replaying a flow-only record must
        // not turn it into a materialized empty record after its flow is cleared.
        let (payloads, mut context) = self.packing_parts(id.arena(), auxiliary, source, header.end);
        for write in writes.into_iter().flatten() {
            assert!(write.store(
                payloads,
                header.actual_shape(),
                header.ordinal,
                &mut context
            ));
        }
    }
}

macro_rules! binding_read {
    ($method:ident, $field:ident, $read:ident, $id:ty) => {
        impl CoreStore {
            pub(crate) fn $method(
                &self,
                header: &StoredNode,
                slot: u32,
                context: CompactContext<'_>,
            ) -> Option<$id> {
                if !self.binding_overrides.is_empty() {
                    if let Some(binding) = self.binding_overrides.get(&slot) {
                        return binding.$field;
                    }
                }
                self.payloads
                    .$read(header.actual_shape(), header.ordinal, context)
            }
        }
    };
}
binding_read!(node_symbol, symbol, read_symbol, SymbolId);
binding_read!(node_locals, locals, read_locals, SymbolTableId);
binding_read!(node_flow, flow_node, read_flow_node, FlowId);

macro_rules! binding_codec {
    ($encode:ident, $decode:ident, $id:ty, $arena:ident, $variant:ident) => {
        impl CompactContext<'_> {
            pub(crate) fn $decode(self, key: FieldKey, word: u32) -> Option<$id> {
                match word {
                    0 => None,
                    u32::MAX => match self
                        .store
                        .links
                        .get(&key)
                        .expect("binding reference escape")
                    {
                        FullReference::$variant(id) => Some(*id),
                        _ => unreachable!("binding escape has its field's identity type"),
                    },
                    slot => Some(
                        <$id>::from_parts(
                            self.store
                                .binding_arenas
                                .expect("inline binding namespaces")
                                .$arena,
                            slot,
                        )
                        .expect("nonzero inline binding slot"),
                    ),
                }
            }
        }
        impl PackingContext<'_> {
            pub(crate) fn $encode(&mut self, key: FieldKey, value: Option<$id>) -> u32 {
                self.remove_link(key);
                let Some(id) = value else {
                    return 0;
                };
                if id.arena()
                    == self
                        .binding_arenas
                        .expect("inline binding namespaces")
                        .$arena
                    && id.slot() != u32::MAX
                {
                    id.slot()
                } else {
                    self.links.insert(key, FullReference::$variant(id));
                    u32::MAX
                }
            }
        }
    };
}
binding_codec!(encode_symbol, decode_symbol, SymbolId, symbols, Symbol);
binding_codec!(
    encode_symbol_table,
    decode_symbol_table,
    SymbolTableId,
    tables,
    SymbolTable
);
binding_codec!(encode_flow, decode_flow, FlowId, flows, Flow);
