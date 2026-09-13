//! Per-source binding publication. Private staging owns all mutable binder data;
//! published results contain non-owning graph identities and borrow their file.
use crate::compact::binding::{BindingArenas, BindingWrite};
use crate::flow::{FlowId, FlowLists, FlowNodes};
use crate::node_map::NodeMap;
use crate::symbol_store::{SymbolRead, Symbols, SymbolsMut, SymbolsRead};
use crate::symbols::{DeclarationLists, SymbolTableId, SymbolTables};
use crate::{AstFile, AstView, Diagnostic, Node, NodeId, NodeRead, ParsedFile, SourceFileRead};
use std::{
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    sync::OnceLock,
};
use ts_arena::{Error, InitializationDomain, InitializationGuard, SymbolId};

#[path = "local_bind.rs"]
pub mod local_bind;

/// Fields absent from the parsed syntax representation, indexed by stable NodeId.
#[derive(Clone, Copy, Debug, Default)]
pub struct NodeBinding {
    pub symbol: Option<SymbolId>,
    pub local_symbol: Option<SymbolId>,
    pub locals: Option<SymbolTableId>,
    pub next_container: Option<NodeId>,
    pub flow_node: Option<FlowId>,
    pub return_flow_node: Option<FlowId>,
    pub end_flow_node: Option<FlowId>,
    pub fallthrough_flow_node: Option<FlowId>,
}

#[derive(Clone, Debug, Default)]
pub struct PatternAmbientModule {
    pub pattern: ts_core::pattern::Pattern,
    pub symbol: Option<SymbolId>,
}

#[derive(Debug)]
pub struct BindResult {
    source: NodeId,
    multiple_sources: bool,
    direct_nodes: bool,
    used_local_scope: bool,
    nodes: NodeMap<Node>,
    bindings: NodeMap<NodeBinding>,
    flow_bindings: NodeMap<FlowId>,
    pub(crate) symbols: Symbols,
    pub(crate) tables: SymbolTables,
    pub(crate) declarations: DeclarationLists,
    pub(crate) flows: FlowNodes,
    pub(crate) flow_lists: FlowLists,
    pub(crate) common_js_module_indicator: Option<NodeId>,
    pub(crate) global_exports: Option<SymbolTableId>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) symbol_count: isize,
    pub(crate) pattern_ambient_modules: Vec<PatternAmbientModule>,
}

enum ColdBindingWrite {
    Absent,
    Applied,
    NeedsMaterialization,
}

impl BindResult {
    /// The writer checks the target first. This shared step checks only the
    /// referenced namespace and must finish before any binding record changes.
    fn validate_binding_value(
        &self,
        parsed: AstView<'_>,
        write: BindingWrite,
    ) -> Result<(), Error> {
        match write {
            BindingWrite::Symbol(Some(value)) | BindingWrite::LocalSymbol(Some(value)) => {
                self.symbols().get(value)?;
            }
            BindingWrite::Locals(Some(value)) => {
                self.tables.get(value)?;
            }
            BindingWrite::NextContainer(Some(value)) => {
                parsed.node(value)?;
            }
            BindingWrite::Flow(Some(value))
            | BindingWrite::ReturnFlow(Some(value))
            | BindingWrite::EndFlow(Some(value))
            | BindingWrite::FallthroughFlow(Some(value)) => {
                self.flows.get(value)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// A materialized record supersedes every inline field. A flow-only record
    /// can be updated here, but promotion waits for the caller's checked read.
    #[inline]
    fn write_existing_binding(&mut self, id: NodeId, write: BindingWrite) -> ColdBindingWrite {
        if !self.bindings.is_empty() {
            if let Some(binding) = self.bindings.get_mut(&id) {
                write.apply(binding);
                return ColdBindingWrite::Applied;
            }
        }
        if !self.flow_bindings.is_empty() && self.flow_bindings.contains_key(&id) {
            if let BindingWrite::Flow(flow) = write {
                if let Some(flow) = flow {
                    self.flow_bindings.insert(id, flow);
                } else {
                    self.flow_bindings.remove(&id);
                }
                return ColdBindingWrite::Applied;
            }
            return ColdBindingWrite::NeedsMaterialization;
        }
        ColdBindingWrite::Absent
    }

    fn materialize_binding(&mut self, id: NodeId, inline: Option<NodeBinding>) -> &mut NodeBinding {
        let inline = inline.unwrap_or_default();
        let flow = self.flow_bindings.remove(&id);
        self.bindings.entry(id).or_insert_with(|| NodeBinding {
            flow_node: flow.or(inline.flow_node),
            ..inline
        })
    }

    /// Finish a checked write when the concrete shape cannot store it inline.
    /// Clearing a sparse flow removes it; clearing a materialized record keeps
    /// that record's presence and the other fields inherited from the header.
    fn write_fallback_binding(
        &mut self,
        id: NodeId,
        write: BindingWrite,
        inline: Option<NodeBinding>,
    ) {
        if let BindingWrite::Flow(flow) = write {
            if let Some(mut binding) = inline {
                binding.flow_node = flow;
                self.bindings.insert(id, binding);
            } else if let Some(flow) = flow {
                self.flow_bindings.insert(id, flow);
            } else {
                self.flow_bindings.remove(&id);
            }
        } else {
            write.apply(self.materialize_binding(id, inline));
        }
    }

    /// Development-only table capacities, not malloc or resident-byte metrics.
    #[cfg(feature = "layout-profile")]
    pub fn layout_profile(&self) -> [usize; 3] {
        [
            self.nodes.capacity(),
            self.bindings.capacity(),
            self.flow_bindings.capacity(),
        ]
    }
    fn new(view: AstView<'_>, source: NodeId) -> Self {
        let counters = view.0.counters();
        let source_map = view
            .file_info()
            .source_files
            .expect("binding source has a source map");
        let multiple_sources = match view
            .auxiliary(source_map)
            .expect("binding source map is retained")
            .full()
        {
            Some(crate::AstStorageData::SourceFiles(files)) => files.len() > 1,
            _ => unreachable!("validated source map"),
        };
        let mut result = Self {
            source,
            multiple_sources,
            direct_nodes: false,
            used_local_scope: false,
            nodes: NodeMap::default(),
            bindings: NodeMap::default(),
            flow_bindings: NodeMap::default(),
            symbols: Symbols::new(counters),
            tables: SymbolTables::new(counters),
            declarations: DeclarationLists::new(counters),
            flows: FlowNodes::new(counters),
            flow_lists: FlowLists::new(counters),
            common_js_module_indicator: view
                .source_file(source)
                .expect("validated binding source")
                .common_js_module_indicator(),
            global_exports: None,
            diagnostics: Vec::new(),
            symbol_count: 0,
            pattern_ambient_modules: Vec::new(),
        };
        result.symbols.initialize_reference_arenas(
            source.arena(),
            result.tables.id(),
            result.declarations.id(),
        );
        result.tables.configure_symbols(result.symbols.id());
        result
            .flows
            .initialize_reference_arenas(source.arena(), result.flow_lists.id());
        result.flow_lists.initialize_flow_arena(result.flows.id());
        result
    }
    pub fn source(&self) -> NodeId {
        self.source
    }
    /// Return non-owning graph links by value; this does not retain any owner.
    fn cold_node_binding(&self, id: NodeId) -> Option<NodeBinding> {
        self.bindings.get(&id).copied().or_else(|| {
            self.flow_bindings.get(&id).map(|&flow| NodeBinding {
                flow_node: Some(flow),
                ..NodeBinding::default()
            })
        })
    }
    pub fn node_binding(&self, view: AstView<'_>, id: NodeId) -> Option<NodeBinding> {
        self.cold_node_binding(id).or_else(|| {
            (self.direct_nodes && id.arena() == self.source.arena())
                .then(|| view.node(id).ok().and_then(|node| node.inline_binding()))
                .flatten()
        })
    }
    pub fn symbols(&self) -> SymbolsRead<'_> {
        self.symbols.read(&self.tables)
    }
    pub fn tables(&self) -> &SymbolTables {
        &self.tables
    }
    pub fn declarations(&self) -> &DeclarationLists {
        &self.declarations
    }
    pub fn flows(&self) -> &FlowNodes {
        &self.flows
    }
    pub fn flow_lists(&self) -> &FlowLists {
        &self.flow_lists
    }
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
    pub fn pattern_ambient_modules(&self) -> &[PatternAmbientModule] {
        &self.pattern_ambient_modules
    }
    pub fn symbol_count(&self) -> isize {
        self.symbol_count
    }
    pub fn global_exports(&self) -> Option<SymbolTableId> {
        self.global_exports
    }
    fn cold_bindings(&self) -> impl Iterator<Item = (NodeId, NodeBinding)> + '_ {
        self.bindings
            .iter()
            .map(|(&id, &binding)| (id, binding))
            .chain(self.flow_bindings.iter().map(|(&id, &flow)| {
                (
                    id,
                    NodeBinding {
                        flow_node: Some(flow),
                        ..NodeBinding::default()
                    },
                )
            }))
    }
    pub fn bindings<'a>(
        &'a self,
        view: AstView<'a>,
    ) -> impl Iterator<Item = (NodeId, NodeBinding)> + 'a {
        self.cold_bindings().chain(
            self.direct_nodes
                .then(|| view.0.for_arena(self.source.arena()).ok())
                .flatten()
                .into_iter()
                .flat_map(move |owner| {
                    owner
                        .core_nodes()
                        .enumerate()
                        .filter_map(move |(index, _)| {
                            let slot = u32::try_from(index + 1).expect("allocated core node slot");
                            let id = NodeId::from_parts(self.source.arena(), slot)
                                .expect("nonzero core slot");
                            if !self.bindings.is_empty() && self.bindings.contains_key(&id) {
                                return None;
                            }
                            if !self.flow_bindings.is_empty()
                                && self.flow_bindings.contains_key(&id)
                            {
                                return None;
                            }
                            let node = AstView(owner, None).node(id).expect("published core node");
                            node.inline_binding().map(|binding| (id, binding))
                        })
                }),
        )
    }
    /// Exclusive binding wrote this source's core directly. Other physical
    /// owners and lazy nodes still require their normal binding selection.
    #[inline]
    pub(crate) fn reads_core_directly(&self, id: NodeId) -> bool {
        self.direct_nodes && id.arena() == self.source.arena()
    }
    pub(crate) fn overlay(&self, id: NodeId) -> Option<&Node> {
        if self.reads_core_directly(id) {
            None
        } else {
            self.nodes.get(&id)
        }
    }
    pub(crate) fn is_single_source_owner(&self, id: NodeId) -> bool {
        !self.multiple_sources && id.arena() == self.source.arena()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindError {
    Storage(Error),
    Failed,
}
impl From<Error> for BindError {
    fn from(error: Error) -> Self {
        Self::Storage(error)
    }
}
impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => error.fmt(f),
            Self::Failed => {
                f.write_str("source file binding failed; initialization cannot be retried")
            }
        }
    }
}
impl std::error::Error for BindError {}

#[derive(Debug, Default)]
pub(crate) struct BindCell(OnceLock<Result<BindResult, ()>>);
impl BindCell {
    pub(crate) fn result(&self) -> Option<&BindResult> {
        self.0.get().and_then(|result| result.as_ref().ok())
    }
    fn initialize(
        &self,
        view: AstView<'_>,
        source: NodeId,
        initialize: impl FnOnce(&mut BindBuilder<'_>) -> Result<(), Error>,
    ) -> Result<&BindResult, BindError> {
        if let Some(result) = self.0.get() {
            return result.as_ref().map_err(|()| BindError::Failed);
        }
        InitializationGuard::assert_inactive(
            view.0.id().arena(),
            InitializationDomain::Binding,
            source.bits(),
        );
        let mut original_panic = None;
        let mut original_error: Option<Error> = None;
        let result = self.0.get_or_init(|| {
            let _guard = InitializationGuard::enter(
                view.0.id().arena(),
                InitializationDomain::Binding,
                source.bits(),
            );
            match catch_unwind(AssertUnwindSafe(|| {
                let mut builder = BindBuilder {
                    storage: BindStorage::Published(view),
                    result: BindResult::new(view, source),
                };
                initialize(&mut builder)?;
                builder.validate()?;
                Ok(builder.result)
            })) {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(error)) => {
                    original_error = Some(error);
                    Err(())
                }
                Err(payload) => {
                    original_panic = Some(payload);
                    Err(())
                }
            }
        });
        if let Some(payload) = original_panic {
            resume_unwind(payload);
        }
        if let Some(error) = original_error {
            return Err(error.into());
        }
        result.as_ref().map_err(|()| BindError::Failed)
    }
}

/// Exclusive binder staging; no ownership capability or raw arena writer escapes
/// publication. A failed initializer drops this entire value before completion.
///
/// S07 binding roots own their logical parsed syntax. A shallow transformed
/// SourceFile that shares children parented to another source is outside this
/// binding contract; source cloning itself remains supported by AST factories.
/// See docs/S07-binding-operations.md for the pinned successful Go counterexample.
pub struct BindBuilder<'a> {
    storage: BindStorage<'a>,
    result: BindResult,
}
enum BindStorage<'a> {
    Published(AstView<'a>),
    Exclusive(&'a mut ParsedFile),
}
impl BindBuilder<'_> {
    /// Borrow syntax only until the next binder mutation. On the consuming path,
    /// headers already include earlier writes; child edges remain unchanged.
    pub fn parsed_view(&self) -> AstView<'_> {
        match &self.storage {
            BindStorage::Published(view) => *view,
            BindStorage::Exclusive(builder) => builder.view(),
        }
    }
    pub fn result(&self) -> &BindResult {
        &self.result
    }
    pub fn view(&self) -> AstView<'_> {
        AstView(self.parsed_view().0, Some(&self.result))
    }
    pub fn source(&self) -> NodeId {
        self.result.source
    }
    #[inline]
    pub fn node(&self, id: NodeId) -> Result<NodeRead<'_>, Error> {
        match &self.storage {
            // Exclusive binding writes the core directly. Keep its checked
            // owner/slot access and the borrow bounded by this builder; lazy
            // records created during initialization still use ordinary routing.
            BindStorage::Exclusive(parsed) if id.arena() == self.result.source.arena() => {
                parsed.core_node_read(id)
            }
            _ => self.node_compatibility(id),
        }
    }
    #[inline(never)]
    fn node_compatibility(&self, id: NodeId) -> Result<NodeRead<'_>, Error> {
        self.view().node(id)
    }
    pub fn node_mut(&mut self, id: NodeId) -> Result<crate::NodeMut<'_>, Error> {
        let parsed = match &mut self.storage {
            BindStorage::Exclusive(parsed) => {
                if id.arena() == self.result.source.arena() {
                    return parsed.builder_mut().node_mut(id);
                }
                parsed.view()
            }
            BindStorage::Published(parsed) => *parsed,
        };
        // The immutable backend still preserves the original parsed headers.
        let owner = parsed.for_node_owner(id)?;
        if owner.0.id() != parsed.0.id()
            || (self.result.multiple_sources && parsed.owning_source(id)? != self.result.source)
        {
            return Err(Error::WrongOwner);
        }
        match self.result.nodes.entry(id) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                Ok(crate::NodeMut::owned(entry.into_mut()))
            }
            std::collections::hash_map::Entry::Vacant(entry) => Ok(crate::NodeMut::owned(
                entry.insert(parsed.node(id)?.copy_for_binding()),
            )),
        }
    }

    /// Header flags contain no graph edges. The exclusive backend can preserve
    /// its parse validation proof while unrestricted `node_mut` still dirties it.
    pub fn set_node_flags(&mut self, id: NodeId, flags: u32) -> Result<(), Error> {
        match &mut self.storage {
            BindStorage::Exclusive(parsed) if id.arena() == self.result.source.arena() => {
                parsed.set_node_flags(id, flags)
            }
            BindStorage::Exclusive(_) | BindStorage::Published(_) => {
                self.node_mut(id)?.set_flags(flags);
                Ok(())
            }
        }
    }
    pub fn binding(&self, id: NodeId) -> Result<Option<NodeBinding>, Error> {
        self.parsed_view().node(id)?;
        Ok(self.result.node_binding(self.parsed_view(), id))
    }
    pub fn node_symbol(&self, id: NodeId) -> Result<Option<SymbolId>, Error> {
        let node = self.node(id)?;
        if !self.result.bindings.is_empty() {
            if let Some(binding) = self.result.bindings.get(&id) {
                return Ok(binding.symbol);
            }
        }
        Ok(
            if self.result.direct_nodes && id.arena() == self.result.source.arena() {
                node.inline_symbol()
            } else {
                None
            },
        )
    }
    pub fn node_locals(&self, id: NodeId) -> Result<Option<SymbolTableId>, Error> {
        let node = self.node(id)?;
        if !self.result.bindings.is_empty() {
            if let Some(binding) = self.result.bindings.get(&id) {
                return Ok(binding.locals);
            }
        }
        Ok(
            if self.result.direct_nodes && id.arena() == self.result.source.arena() {
                node.inline_locals()
            } else {
                None
            },
        )
    }
    pub fn node_flow(&self, id: NodeId) -> Result<Option<FlowId>, Error> {
        let node = self.node(id)?;
        if let Some(binding) = self.result.cold_node_binding(id) {
            return Ok(binding.flow_node);
        }
        Ok(
            if self.result.direct_nodes && id.arena() == self.result.source.arena() {
                node.inline_flow()
            } else {
                None
            },
        )
    }
    pub fn set_node_symbol(&mut self, id: NodeId, value: Option<SymbolId>) -> Result<(), Error> {
        self.set_binding_field(id, BindingWrite::Symbol(value))
    }
    pub fn set_node_local_symbol(
        &mut self,
        id: NodeId,
        value: Option<SymbolId>,
    ) -> Result<(), Error> {
        self.set_binding_field(id, BindingWrite::LocalSymbol(value))
    }
    pub fn set_node_locals(
        &mut self,
        id: NodeId,
        value: Option<SymbolTableId>,
    ) -> Result<(), Error> {
        self.set_binding_field(id, BindingWrite::Locals(value))
    }
    pub fn set_node_next_container(
        &mut self,
        id: NodeId,
        value: Option<NodeId>,
    ) -> Result<(), Error> {
        self.set_binding_field(id, BindingWrite::NextContainer(value))
    }
    pub fn set_node_return_flow(&mut self, id: NodeId, value: Option<FlowId>) -> Result<(), Error> {
        self.set_binding_field(id, BindingWrite::ReturnFlow(value))
    }
    pub fn set_node_end_flow(&mut self, id: NodeId, value: Option<FlowId>) -> Result<(), Error> {
        self.set_binding_field(id, BindingWrite::EndFlow(value))
    }
    pub fn set_node_fallthrough_flow(
        &mut self,
        id: NodeId,
        value: Option<FlowId>,
    ) -> Result<(), Error> {
        self.set_binding_field(id, BindingWrite::FallthroughFlow(value))
    }
    /// The overwhelmingly common identifier/access edge needs only one FlowId.
    /// Promotion to a full declaration/container record preserves that edge.
    pub fn set_node_flow(&mut self, id: NodeId, flow: Option<FlowId>) -> Result<(), Error> {
        self.set_binding_field(id, BindingWrite::Flow(flow))
    }
    fn set_binding_field(&mut self, id: NodeId, write: BindingWrite) -> Result<(), Error> {
        self.validate_write_owner(id)?;
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
        if let BindStorage::Exclusive(parsed) = &mut self.storage {
            if id.arena() == self.result.source.arena() && parsed.write_binding_field(id, write)? {
                return Ok(());
            }
        }
        // Rare unsupported shapes and already-published owners keep the original
        // side records. A compatibility record supersedes every inline field.
        let inline = self.node(id)?.inline_binding();
        self.result.write_fallback_binding(id, write, inline);
        Ok(())
    }
    pub fn binding_mut(&mut self, id: NodeId) -> Result<&mut NodeBinding, Error> {
        self.validate_write_owner(id)?;
        let inline = self.node(id)?.inline_binding();
        Ok(self.result.materialize_binding(id, inline))
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
    pub fn tables_mut(&mut self) -> &mut SymbolTables {
        &mut self.result.tables
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
    pub fn flows_mut(&mut self) -> &mut FlowNodes {
        &mut self.result.flows
    }
    pub fn flow_lists(&self) -> &FlowLists {
        &self.result.flow_lists
    }
    pub fn flow_lists_mut(&mut self) -> &mut FlowLists {
        &mut self.result.flow_lists
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
    fn validate_write_owner(&self, id: NodeId) -> Result<(), Error> {
        if let BindStorage::Exclusive(builder) = &self.storage {
            if id.arena() == self.result.source.arena() {
                builder.core_node(id)?;
                return Ok(());
            }
        }
        let owner = self.parsed_view().for_node_owner(id)?;
        if owner.0.id() != self.parsed_view().0.id() {
            return Err(Error::WrongOwner);
        }
        if self.result.multiple_sources
            && self.parsed_view().owning_source(id)? != self.result.source
        {
            return Err(Error::WrongOwner);
        }
        Ok(())
    }
    fn validate_symbol_graph(&self) -> Result<(), Error> {
        for (_, symbol) in self.result.symbols().iter() {
            self.result.declarations.get(symbol.declarations())?;
            if let Some(node) = symbol.value_declaration() {
                self.parsed_view().node(node)?;
            }
            for table in [symbol.members(), symbol.exports()].into_iter().flatten() {
                self.result.tables.get(table)?;
            }
            for symbol in [symbol.parent(), symbol.export_symbol()]
                .into_iter()
                .flatten()
            {
                self.result.symbols().get(symbol)?;
            }
        }
        for (_, nodes) in self.result.declarations.iter() {
            for node in nodes.iter().flatten() {
                self.parsed_view().node(node)?;
            }
        }
        for (_, table) in self.result.tables.iter() {
            for symbol in table.iter().filter_map(|(_, symbol)| symbol) {
                self.result.symbols().get(symbol)?;
            }
        }
        Ok(())
    }
    fn validate_flow_graph(&self) -> Result<(), Error> {
        use crate::FlowData;
        for (_, flow) in self.result.flows.iter() {
            if let Some(antecedent) = flow.antecedent() {
                self.result.flows.get(antecedent)?;
            }
            if let Some(list) = flow.antecedents() {
                self.result.flow_lists.get(list)?;
            }
            match flow.node() {
                Some(FlowData::Ast(node)) => {
                    self.parsed_view().node(node)?;
                }
                Some(FlowData::SwitchClause(data)) => {
                    if let Some(node) = data.switch_statement {
                        self.parsed_view().node(node)?;
                    }
                }
                Some(FlowData::ReduceLabel(data)) => {
                    if let Some(flow) = data.target {
                        self.result.flows.get(flow)?;
                    }
                    if let Some(list) = data.antecedents {
                        self.result.flow_lists.get(list)?;
                    }
                }
                None => {}
            }
        }
        for (_, list) in self.result.flow_lists.iter() {
            if let Some(flow) = list.flow() {
                self.result.flows.get(flow)?;
            }
            if let Some(next) = list.next() {
                self.result.flow_lists.get(next)?;
            }
        }
        Ok(())
    }
    fn validate(&self) -> Result<(), Error> {
        for (&id, node) in &self.result.nodes {
            self.validate_write_owner(id)?;
            if let Some(parent) = node.parent() {
                self.parsed_view().node(parent)?;
            }
            self.parsed_view().validate_data(node.data())?;
        }
        // Narrow inline writes validate their target IDs at the write. Only the
        // compatibility records can still carry unchecked field assignments.
        for (id, binding) in self.result.cold_bindings() {
            self.validate_write_owner(id)?;
            for symbol in [binding.symbol, binding.local_symbol].into_iter().flatten() {
                self.result.symbols().get(symbol)?;
            }
            if let Some(table) = binding.locals {
                self.result.tables.get(table)?;
            }
            if let Some(next) = binding.next_container {
                self.parsed_view().node(next)?;
            }
            for flow in [
                binding.flow_node,
                binding.return_flow_node,
                binding.end_flow_node,
                binding.fallthrough_flow_node,
            ]
            .into_iter()
            .flatten()
            {
                self.result.flows.get(flow)?;
            }
        }
        if let Some(indicator) = self.result.common_js_module_indicator {
            self.parsed_view().node(indicator)?;
        }
        if let Some(table) = self.result.global_exports {
            self.result.tables.get(table)?;
        }
        for module in &self.result.pattern_ambient_modules {
            if let Some(symbol) = module.symbol {
                self.result.symbols().get(symbol)?;
            }
        }
        self.validate_symbol_graph()?;
        self.validate_flow_graph()?;
        let mut diagnostics: Vec<_> = self.result.diagnostics.iter().collect();
        while let Some(diagnostic) = diagnostics.pop() {
            if let Some(file) = diagnostic.file {
                self.parsed_view().node(file)?;
            }
            diagnostics.extend(diagnostic.message_chain.iter().map(AsRef::as_ref));
            diagnostics.extend(diagnostic.related_information.iter().map(AsRef::as_ref));
        }
        Ok(())
    }
}

/// A checked borrowed view of one completed logical SourceFile. The parsed view
/// remains separately available on AstFile and does not claim binding state.
#[derive(Clone, Copy, Debug)]
pub struct BoundView<'a> {
    pub(crate) ast: AstView<'a>,
    pub(crate) result: &'a BindResult,
}
impl<'a> BoundView<'a> {
    pub fn ast(self) -> AstView<'a> {
        self.ast
    }
    pub fn result(self) -> &'a BindResult {
        self.result
    }
    pub fn node(self, id: NodeId) -> Result<NodeRead<'a>, Error> {
        self.ast.node(id)
    }
    pub fn node_binding(self, id: NodeId) -> Result<Option<NodeBinding>, Error> {
        self.ast.0.node(id)?;
        Ok(self
            .ast
            .binding_for_node(id)?
            .and_then(|result| result.node_binding(self.ast, id)))
    }
    pub fn source_file(self) -> Result<SourceFileRead<'a>, Error> {
        self.ast.source_file(self.result.source)
    }
    pub fn symbol(self, id: SymbolId) -> Result<SymbolRead<'a>, Error> {
        self.result.symbols().get(id)
    }
}

impl AstFile {
    /// Initialize on the current worker. Contenders wait; the initiating caller
    /// resumes its original panic after recording terminal failure. Callers must
    /// dispatch to a reserved-stack worker before entering this boundary.
    /// The source must own its logical parsed syntax. Rebinding a shallow source
    /// clone whose child parents name another source is not supported in S07.
    pub fn bind_with(
        &self,
        source: NodeId,
        initialize: impl FnOnce(&mut BindBuilder<'_>) -> Result<(), Error>,
    ) -> Result<BoundView<'_>, BindError> {
        let parsed = self.view().for_node_owner(source)?;
        let parsed = AstView(parsed.0.owner_retention()?, None);
        let state = parsed.source_file(source)?.state_ref();
        let result = state.binding.initialize(parsed, source, initialize)?;
        Ok(BoundView {
            ast: AstView(parsed.0, Some(result)),
            result,
        })
    }
    pub fn bound_view(&self, source: NodeId) -> Result<Option<BoundView<'_>>, Error> {
        let parsed = self.view().for_node_owner(source)?;
        let parsed = AstView(parsed.0.owner_retention()?, None);
        let state = parsed.source_file(source)?.state_ref();
        Ok(state.binding.result().map(|result| BoundView {
            ast: AstView(parsed.0, Some(result)),
            result,
        }))
    }
    pub fn is_bound(&self, source: NodeId) -> Result<bool, Error> {
        Ok(self.bound_view(source)?.is_some())
    }
}

/// Retains the complete file/bundle while selecting its completed logical source.
#[derive(Clone, Debug)]
pub struct BoundFile {
    file: AstFile,
    source: NodeId,
}
impl BoundFile {
    pub fn view(&self) -> BoundView<'_> {
        self.file
            .bound_view(self.source)
            .expect("retained source identity")
            .expect("binding completion is permanent")
    }
    pub fn source(&self) -> NodeId {
        self.source
    }
    pub fn parsed_file(&self) -> &AstFile {
        &self.file
    }
    pub fn retain_symbol(&self, id: SymbolId) -> Result<RetainedSymbol, Error> {
        self.view().symbol(id)?;
        Ok(RetainedSymbol {
            file: self.clone(),
            id,
        })
    }
    pub fn retain_node(&self, id: NodeId) -> Result<RetainedBoundNode, Error> {
        let source = self.file.view().owning_source(id)?;
        // Retention is a bound boundary: observing an unbound or failed sibling
        // cannot silently mint a handle that returns its parsed headers.
        self.file
            .bound_view(source)?
            .ok_or(Error::InvalidGraph)?
            .node(id)?;
        Ok(RetainedBoundNode {
            file: BoundFile {
                file: self.file.clone(),
                source,
            },
            id,
        })
    }
}
#[derive(Clone, Debug)]
pub struct RetainedSymbol {
    file: BoundFile,
    id: SymbolId,
}
impl RetainedSymbol {
    pub fn id(&self) -> SymbolId {
        self.id
    }
    pub fn file(&self) -> &BoundFile {
        &self.file
    }
}
impl RetainedSymbol {
    pub fn symbol(&self) -> SymbolRead<'_> {
        self.file
            .view()
            .symbol(self.id)
            .expect("validated retained symbol")
    }
}
#[derive(Clone, Debug)]
pub struct RetainedBoundNode {
    file: BoundFile,
    id: NodeId,
}
impl RetainedBoundNode {
    pub fn id(&self) -> NodeId {
        self.id
    }
    pub fn file(&self) -> &BoundFile {
        &self.file
    }
    pub fn node(&self) -> NodeRead<'_> {
        self.file
            .view()
            .node(self.id)
            .expect("validated retained bound node")
    }
}
impl AstFile {
    pub fn retain_bound(&self, source: NodeId) -> Result<BoundFile, BindError> {
        self.bound_view(source)?.ok_or(BindError::Failed)?;
        Ok(BoundFile {
            file: self.clone(),
            source,
        })
    }
}

/// Retention capability for the consuming parse/bind/publish path. It exposes
/// completed syntax only: there is no promise of a second, pristine parsed AST.
///
/// ```compile_fail
/// fn parsed(file: &ts_ast::CompletedFile) { file.parsed_file(); }
/// ```
#[derive(Clone, Debug)]
pub struct CompletedFile {
    bound: BoundFile,
}
impl crate::AstBuilder {
    /// Retain completed syntax before creating synthetic nodes that refer to it.
    /// This capability does not expose or promise a pristine parsed snapshot.
    pub fn retain_completed(&mut self, file: &CompletedFile) {
        self.retain_file(file.bound.file.clone());
    }
}
impl CompletedFile {
    pub fn view(&self) -> BoundView<'_> {
        self.bound.view()
    }
    pub fn source(&self) -> NodeId {
        self.bound.source()
    }
    /// Diagnostic provenance for the A0 experiment, not a parity metric.
    pub fn bound_in_place(&self) -> bool {
        self.view().result.direct_nodes
    }
    /// Records whether binding actually entered the scoped local backend.
    /// This describes path selection, not correctness or performance acceptance.
    pub fn bound_with_local_scope(&self) -> bool {
        self.view().result.used_local_scope
    }
    pub fn retain_symbol(&self, id: SymbolId) -> Result<CompletedSymbol, Error> {
        self.view().symbol(id)?;
        Ok(CompletedSymbol {
            file: self.clone(),
            id,
        })
    }
    pub fn retain_node(&self, id: NodeId) -> Result<CompletedNode, Error> {
        let retained = self.bound.retain_node(id)?;
        Ok(CompletedNode {
            file: Self {
                bound: retained.file,
            },
            id,
        })
    }
}
#[derive(Clone, Debug)]
pub struct CompletedSymbol {
    file: CompletedFile,
    id: SymbolId,
}
impl CompletedSymbol {
    pub fn id(&self) -> SymbolId {
        self.id
    }
    pub fn file(&self) -> &CompletedFile {
        &self.file
    }
}
impl CompletedSymbol {
    pub fn symbol(&self) -> SymbolRead<'_> {
        self.file.view().symbol(self.id).expect("retained symbol")
    }
}
#[derive(Clone, Debug)]
pub struct CompletedNode {
    file: CompletedFile,
    id: NodeId,
}
impl CompletedNode {
    pub fn id(&self) -> NodeId {
        self.id
    }
    pub fn file(&self) -> &CompletedFile {
        &self.file
    }
    pub fn node(&self) -> NodeRead<'_> {
        self.file.view().node(self.id).expect("retained node")
    }
}
impl ParsedFile {
    /// Consume a completed parse, bind, and publish. Selection happens before
    /// mutation. Imported, mapped, multiple-source and existing lazy storage
    /// retain the published binding path. Errors and panics drop the exclusive
    /// owner; no completed cache entry or half-bound file can escape.
    /// Call this on a reserved-stack worker, as with `AstFile::bind_with`.
    pub fn bind_and_publish(
        mut self,
        initialize: impl FnOnce(&mut BindBuilder<'_>) -> Result<(), Error>,
    ) -> Result<CompletedFile, BindError> {
        let source = self.root();
        self.view().source_file(source)?;
        let mut result = BindResult::new(self.view(), source);
        let eligible = !result.multiple_sources
            && self.exclusive_core_only()
            && self
                .view()
                .source_file(source)?
                .content_mapper_info()
                .is_none();
        if !eligible {
            let file = self.try_publish_unbound()?;
            file.bind_with(source, initialize)?;
            return Ok(CompletedFile {
                bound: file.retain_bound(source)?,
            });
        }
        result.direct_nodes = true;
        self.initialize_binding_storage(BindingArenas {
            symbols: result.symbols.id(),
            tables: result.tables.id(),
            flows: result.flows.id(),
        });
        let result = {
            let mut binding = BindBuilder {
                storage: BindStorage::Exclusive(&mut self),
                result,
            };
            initialize(&mut binding)?;
            binding.validate()?;
            binding.result
        };
        self.view()
            .source_file(source)?
            .state_ref()
            .binding
            .0
            .set(Ok(result))
            .map_err(|_| Error::InvalidGraph)?;
        let file = self.try_publish_unbound()?;
        Ok(CompletedFile {
            bound: file.retain_bound(source)?,
        })
    }
}
