use crate::auxiliary::{AuxRead, AuxValue};
use crate::compact::{CompactContext, FieldKey, StoredNode};
use crate::NodeMut;
use crate::{
    AstStorageData, FactoryHooks, FileInfo, JSDocRoots, JsString, Node, NodeData, NodeId, NodeList,
    NodeListId, NodeListRead, NodeRead, NodeSlice, NodeSliceRead, TextSlice, TextSliceRead,
};
use std::sync::Arc;
use ts_arena::{
    AuxId, Counters, Error, StorageBuilder, StorageHandle, StorageRead, StorageTransaction,
    StorageView,
};
use ts_core::TextRange;
use ts_jsstring::SourceText;

#[cfg(test)]
mod parent_tests;

/// Exclusive syntax construction. Hooks exist only during this exclusive phase.
pub struct AstBuilder {
    pub(crate) storage: StorageBuilder<StoredNode>,
    pub(crate) hooks: Option<Arc<dyn FactoryHooks>>,
    frame: AuxId,
    // Constructors validate payload edges and backing ranges before insertion.
    // Unrestricted syntax mutation can only invalidate this proof, never restore it.
    construction_edges_valid: bool,
}
impl std::fmt::Debug for AstBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.storage.fmt(f)
    }
}
impl AstBuilder {
    pub fn new(source: SourceText, counters: &Counters) -> Self {
        Self::from_hooks(source, counters, None)
    }
    pub fn with_hooks(
        source: SourceText,
        counters: &Counters,
        hooks: Arc<dyn FactoryHooks>,
    ) -> Self {
        Self::from_hooks(source, counters, Some(hooks))
    }
    fn from_hooks(
        source: SourceText,
        counters: &Counters,
        hooks: Option<Arc<dyn FactoryHooks>>,
    ) -> Self {
        let mut storage = StorageBuilder::<StoredNode>::from_source_text(source, counters);
        let auxiliary = storage.view().auxiliary_arena();
        let record = storage
            .store_mut()
            .auxiliary
            .push(AstStorageData::File(FileInfo::default()), auxiliary);
        let frame = storage.push_aux(record);
        storage
            .set_metadata(frame)
            .expect("new file frame belongs to core storage");
        let construction_edges_valid = hooks.is_none();
        Self {
            storage,
            hooks,
            frame,
            construction_edges_valid,
        }
    }
    pub(crate) fn push_auxiliary(&mut self, value: AstStorageData) -> AuxId {
        let owner = self.storage.view().auxiliary_arena();
        let record = self.storage.store_mut().auxiliary.push(value, owner);
        self.storage.push_aux(record)
    }
    pub(crate) fn auxiliary_mut(&mut self, id: AuxId) -> Result<&mut AstStorageData, Error> {
        let (record, store) = self.storage.aux_and_store_mut(id)?;
        store.auxiliary.full_mut(record)
    }
    pub fn view(&self) -> AstView<'_> {
        AstView(self.storage.view(), None)
    }
    pub fn id(&self) -> ts_arena::FileId {
        self.storage.id()
    }
    /// Retain a published dependency before storing any of its identities.
    /// Importing a mapped member retains its complete bundle and dependencies.
    pub fn retain_file(&mut self, file: AstFile) {
        self.storage.retain_file(file.0);
        self.construction_edges_valid = false;
    }
    pub fn node_mut(&mut self, id: NodeId) -> Result<NodeMut<'_>, Error> {
        let header = self.storage.core_node(id)?;
        let value = NodeRead::resolved(id, &StorageRead::borrowed(header), self.storage.view())
            .to_owned_preserving_identity();
        let auxiliary = self.storage.view().auxiliary_arena();
        let (header, store, source) = self.storage.node_store_and_source_mut(id)?;
        self.construction_edges_valid = false;
        Ok(NodeMut::core(value, header, store, source, id, auxiliary))
    }
    /// Called only after factory payload validation; the input is parentless.
    pub(crate) fn push_node(&mut self, node: Node) -> NodeId {
        let nodes = self.id().arena();
        let auxiliary = self.storage.view().auxiliary_arena();
        let (store, source) = self.storage.store_and_source_mut();
        let (payloads, mut context) = store.packing_parts(nodes, auxiliary, source, node.end());
        let (shape, ordinal) = payloads.insert(node.data, &mut context);
        let id = self.storage.push(StoredNode {
            kind: node.kind,
            flags: node.flags,
            pos: node.pos,
            end: node.end,
            parent: 0,
            shape,
            ordinal,
        });
        if node.parent.is_some() {
            self.write_parent(id, node.parent).expect("new core header");
        }
        id
    }
    /// Generated concrete entries validate all supplied edges before calling.
    /// Node counts and row/header allocation keep the generic factory's order.
    pub(crate) fn new_typed_node_before_hook(
        &mut self,
        kind: crate::NodeKind,
        pack: impl FnOnce(
            &mut crate::AstPayloadStore,
            &mut crate::compact::PackingContext<'_>,
        ) -> (u16, u32),
    ) -> NodeId {
        let frame = self.frame_mut();
        frame.node_count = frame.node_count.wrapping_add(1);
        let nodes = self.id().arena();
        let auxiliary = self.storage.view().auxiliary_arena();
        let (store, source) = self.storage.store_and_source_mut();
        let (payloads, mut context) = store.packing_parts(nodes, auxiliary, source, -1);
        let (shape, ordinal) = pack(payloads, &mut context);
        self.storage.push(StoredNode {
            kind,
            shape,
            flags: 0,
            pos: -1,
            end: -1,
            parent: 0,
            ordinal,
        })
    }
    pub(crate) fn write_parent(&mut self, id: NodeId, parent: Option<NodeId>) -> Result<(), Error> {
        let auxiliary = self.storage.view().auxiliary_arena();
        let (header, store, source) = self.storage.node_store_and_source_mut(id)?;
        let (_, mut context) = store.packing_parts(id.arena(), auxiliary, source, header.end);
        header.parent = context.encode_node(FieldKey::parent(id.slot()), parent);
        Ok(())
    }
    /// Construction validated every child and list before insertion. With no
    /// escaped links, only core headers need mutation: payload and list borrows
    /// remain valid throughout traversal. Dirty and exceptional owners retain
    /// the factory's gather-before-write path, including its failure behavior.
    pub(crate) fn override_core_parents(&mut self, parent: NodeId) -> bool {
        if !self.construction_edges_valid
            || !self.storage.is_core_only()
            || self.storage.store().has_link_escapes()
            || parent.slot() == u32::MAX
        {
            return false;
        }
        let (nodes, data) = self.storage.split_core_mut();
        let header = nodes
            .get(parent)
            .expect("factory node belongs to retained storage");
        let (kind, shape, ordinal, end) = (
            header.kind,
            header.actual_shape(),
            header.ordinal,
            header.end,
        );
        let context = CompactContext {
            nodes: nodes.id(),
            auxiliary: data.auxiliary_arena(),
            source: data.source(),
            store: data.store(),
        };
        let mut visitor = CoreParents {
            nodes,
            data,
            parent,
        };
        let _ = context.store.payloads.for_each_stored_child(
            kind,
            shape,
            ordinal,
            end,
            context,
            &mut visitor,
        );
        true
    }
    pub(crate) fn finish_header(
        &mut self,
        id: NodeId,
        range: TextRange,
        flags: u32,
        replace_flags: bool,
    ) -> Result<(), Error> {
        let auxiliary = self.storage.view().auxiliary_arena();
        let (header, store, source) = self.storage.node_store_and_source_mut(id)?;
        let new_end = range.end() as i32;
        if header.end != new_end
            && crate::AstPayloadStore::has_source_relative_text(header.actual_shape())
        {
            let (payloads, mut context) =
                store.packing_parts(id.arena(), auxiliary, source, new_end);
            payloads.change_text_end(
                header.actual_shape(),
                header.ordinal,
                header.end,
                new_end,
                &mut context,
            );
        }
        header.pos = range.pos() as i32;
        header.end = new_end;
        header.flags = if replace_flags {
            flags
        } else {
            header.flags | flags
        };
        Ok(())
    }
    pub(crate) fn frame_mut(&mut self) -> &mut FileInfo {
        match self.auxiliary_mut(self.frame).expect("core file frame") {
            AstStorageData::File(info) => info,
            _ => unreachable!("file frame record kind"),
        }
    }
    pub fn node_count(&self) -> i64 {
        self.view().file_info().node_count
    }
    pub fn text_count(&self) -> i64 {
        self.view().file_info().text_count
    }
    #[allow(clippy::needless_pass_by_value)] // Construction transfers and releases its temporary vector.
    pub fn node_slice(&mut self, nodes: Vec<Option<NodeId>>) -> Result<NodeSlice, Error> {
        self.node_slice_from_slice(&nodes)
    }
    /// Copy borrowed edges after validating every ID. An empty input receives
    /// an allocated-empty backing identity, just like the consuming constructor.
    /// See `StorageBuilder::adopt_source`: the text of a file constructed in a
    /// storage that was built without any.
    pub fn adopt_source(&mut self, source: SourceText) -> Result<(), Error> {
        self.storage.adopt_source(source)
    }
    pub fn node_slice_from_slice(&mut self, nodes: &[Option<NodeId>]) -> Result<NodeSlice, Error> {
        for &id in nodes.iter().flatten() {
            self.view().node(id)?;
        }
        let len = checked_len(nodes.len())?;
        let owner = self.id().arena();
        let compact = self.storage.store_mut().edges.append(owner, nodes)?;
        let backing = self.push_auxiliary(AstStorageData::CompactNodes(compact));
        Ok(NodeSlice {
            backing: Some(backing),
            start: 0,
            len,
        })
    }
    pub fn text_slice(&mut self, text: Vec<JsString>) -> Result<TextSlice, Error> {
        let len = checked_len(text.len())?;
        let backing = self.push_auxiliary(AstStorageData::Text(text.into_boxed_slice()));
        Ok(TextSlice {
            backing: Some(backing),
            start: 0,
            len,
        })
    }
    pub fn new_list(&mut self, loc: TextRange, nodes: NodeSlice) -> Result<NodeListId, Error> {
        self.view().node_slice(nodes)?;
        Ok(NodeListId(self.push_auxiliary(AstStorageData::List(
            NodeList::new(loc, nodes),
        ))))
    }
    pub fn clone_list(&mut self, list: NodeListId) -> Result<NodeListId, Error> {
        let header = self.view().list(list)?.to_owned();
        Ok(NodeListId(
            self.push_auxiliary(AstStorageData::List(header)),
        ))
    }
    pub fn list_mut(&mut self, id: NodeListId) -> Result<&mut NodeList, Error> {
        let owner = self.storage.view().auxiliary_arena();
        let (record, store) = self.storage.aux_and_store_mut(id.0)?;
        let list = store.auxiliary.list_mut(record, owner)?;
        self.construction_edges_valid = false;
        Ok(list)
    }
    pub fn set_list_nodes(&mut self, id: NodeListId, nodes: NodeSlice) -> Result<(), Error> {
        self.view().node_slice(nodes)?;
        let owner = self.storage.view().auxiliary_arena();
        let (record, store) = self.storage.aux_and_store_mut(id.0)?;
        store.auxiliary.set_nodes(record, nodes, owner)
    }
    /// Location and cached modifier flags cannot change syntax edges.
    pub fn set_list_location(&mut self, id: NodeListId, loc: TextRange) -> Result<(), Error> {
        let (record, store) = self.storage.aux_and_store_mut(id.0)?;
        store.auxiliary.set_location(record, loc)
    }
    pub fn set_list_modifier_flags(&mut self, id: NodeListId, flags: u32) -> Result<(), Error> {
        let (record, store) = self.storage.aux_and_store_mut(id.0)?;
        store.auxiliary.set_flags(record, flags)
    }
    pub fn mark_list_missing(&mut self, id: NodeListId) -> Result<(), Error> {
        let owner = self.storage.view().auxiliary_arena();
        let (record, store) = self.storage.aux_and_store_mut(id.0)?;
        store
            .auxiliary
            .set_nodes(record, NodeSlice::missing(), owner)
    }
    pub fn seed_jsdoc(&mut self, parent: NodeId, roots: Vec<NodeId>) -> Result<(), Error> {
        self.storage.seed_jsdoc(parent, roots)
    }
    pub fn seed_source_jsdoc(
        &mut self,
        source: NodeId,
        parent: NodeId,
        roots: Vec<NodeId>,
    ) -> Result<(), Error> {
        self.view().source_file(source)?;
        self.storage.seed_source_jsdoc(source, parent, roots)
    }
    /// Complete a source-file or fragment parse without freezing binder mutation.
    pub fn complete(mut self, root: NodeId) -> Result<ParsedFile, Error> {
        self.view().node(root)?;
        self.frame_mut().root = Some(root);
        let checked = self.construction_edges_valid && self.storage.is_core_only();
        self.view().validate_core_with_construction_edges(checked)?;
        Ok(ParsedFile {
            builder: self,
            validated: true,
        })
    }
}

struct CoreParents<'a> {
    nodes: ts_arena::CoreNodesMut<'a, StoredNode>,
    data: ts_arena::CoreDataRead<'a, StoredNode>,
    // Resolved against nodes before traversal; its slot fits the local codec.
    parent: NodeId,
}
impl crate::ChildVisitor for CoreParents<'_> {
    fn visit_node(&mut self, node: NodeId) -> std::ops::ControlFlow<()> {
        self.nodes
            .get_mut(node)
            .expect("factory owns mutable core node")
            .parent = self.parent.slot();
        std::ops::ControlFlow::Continue(())
    }
    fn visit_list(&mut self, list: NodeListId) -> std::ops::ControlFlow<()> {
        let header = list_read(AuxRead::Core {
            record: self.data.auxiliary(list.0).expect("factory list"),
            store: self.data.store(),
            owner: self.data.auxiliary_arena(),
        })
        .expect("factory list");
        self.visit_node_slice(header.nodes())
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> std::ops::ControlFlow<()> {
        let record = nodes.backing.map(|backing| AuxRead::Core {
            record: self.data.auxiliary(backing).expect("factory slice"),
            store: self.data.store(),
            owner: self.data.auxiliary_arena(),
        });
        let values = node_slice_read(
            nodes,
            record,
            Some((&self.data.store().edges, self.nodes.id())),
        )
        .expect("factory slice");
        for child in values.iter().flatten() {
            self.visit_node(child)?;
        }
        std::ops::ControlFlow::Continue(())
    }
}

/// Parse completion keeps exclusive core mutation available to the later binder.
/// The whole value can move to a worker; none of its views can escape that move.
///
/// ```compile_fail
/// use ts_ast::{AstBuilder, FactoryMethods, NodeRead, SyntaxKind};
/// fn escaped() -> NodeRead<'static> {
///     let mut builder = AstBuilder::new(ts_jsstring::SourceText::from_loaded_bytes(&b""[..]), &ts_arena::Counters::new());
///     let root = builder.new_token(SyntaxKind::Unknown.into());
///     builder.view().node(root).unwrap()
/// }
/// ```
///
/// ```compile_fail
/// use ts_ast::{AstBuilder, FactoryMethods, SyntaxKind};
/// let mut builder = AstBuilder::new(ts_jsstring::SourceText::from_loaded_bytes(&b""[..]), &ts_arena::Counters::new());
/// let root = builder.new_token(SyntaxKind::Unknown.into());
/// let published = builder.complete(root).unwrap().publish_unbound();
/// published.node_mut(root).unwrap().set_flags(1);
/// ```
#[derive(Debug)]
pub struct ParsedFile {
    builder: AstBuilder,
    validated: bool,
}
impl ParsedFile {
    pub(crate) fn local_binding_eligible(&self) -> bool {
        self.validated
            && self.exclusive_core_only()
            && self.builder.storage.store().local_binding_eligible()
    }

    pub(crate) fn with_local_core<R>(
        &mut self,
        operation: impl for<'scope> FnOnce(ts_arena::CoreScopeMut<'scope, '_, StoredNode>) -> R,
    ) -> R {
        // The caller checked eligibility. Only the AST's narrow local writer
        // receives this scope, preserving the completed syntax validation.
        self.builder.storage.with_core_scope(operation)
    }

    pub(crate) fn initialize_binding_storage(
        &mut self,
        arenas: crate::compact::binding::BindingArenas,
    ) {
        self.builder.storage.store_mut().initialize_binding(arenas);
    }
    pub(crate) fn write_binding_field(
        &mut self,
        id: NodeId,
        write: crate::compact::binding::BindingWrite,
    ) -> Result<bool, Error> {
        let auxiliary = self.builder.storage.view().auxiliary_arena();
        let (header, store, source) = self.builder.storage.node_store_and_source_mut(id)?;
        Ok(store.write_binding(header, id, auxiliary, source, write))
    }
    pub(crate) fn exclusive_core_only(&self) -> bool {
        self.builder.hooks.is_none() && self.builder.storage.is_core_only()
    }
    pub(crate) fn core_node(&self, id: NodeId) -> Result<&StoredNode, Error> {
        self.builder.storage.core_node(id)
    }
    pub(crate) fn core_node_read(&self, id: NodeId) -> Result<NodeRead<'_>, Error> {
        let record = self.builder.storage.core_node(id)?;
        let owner = self.builder.storage.view();
        Ok(NodeRead::core(id, record, owner.physical_owner()))
    }
    pub(crate) fn set_node_flags(&mut self, id: NodeId, flags: u32) -> Result<(), Error> {
        self.builder.storage.node_mut(id)?.set_flags(flags);
        // Do not restore a proof invalidated by an earlier unrestricted edit.
        Ok(())
    }
    pub fn view(&self) -> AstView<'_> {
        self.builder.view()
    }
    pub fn builder_mut(&mut self) -> &mut AstBuilder {
        self.validated = false;
        &mut self.builder
    }
    pub fn root(&self) -> NodeId {
        self.view().file_info().root.expect("completed parse root")
    }
    /// Parser-tool publication deliberately makes no claim that binding ran.
    pub fn publish_unbound(self) -> AstFile {
        self.try_publish_unbound()
            .expect("published AST references belong to retained storage")
    }
    /// Revalidate every core record and auxiliary edge after exclusive mutations.
    /// A freshly completed parse carries its validation through publication;
    /// requesting its mutable builder invalidates that proof conservatively.
    pub fn try_publish_unbound(self) -> Result<AstFile, Error> {
        if !self.validated {
            self.view().validate_core()?;
        }
        Ok(AstFile(self.builder.storage.finish()))
    }
    /// Publish a mapped group under one retention root, without asserting binding.
    pub fn publish_bundle_unbound(self, supplemental: Vec<Self>) -> AstBundle {
        self.try_publish_bundle_unbound(supplemental)
            .expect("published bundle references belong to retained members")
    }
    pub fn try_publish_bundle_unbound(self, supplemental: Vec<Self>) -> Result<AstBundle, Error> {
        let mut members = vec![&self.builder.storage];
        members.extend(supplemental.iter().map(|file| &file.builder.storage));
        StorageBuilder::with_group_views(&members, |views| {
            for view in views {
                AstView(*view, None).validate_core()?;
            }
            Ok(())
        })?;
        Ok(AstBundle(ts_arena::StorageBundle::new(
            self.builder.storage,
            supplemental
                .into_iter()
                .map(|file| file.builder.storage)
                .collect(),
        )))
    }
}

#[derive(Clone, Debug)]
pub struct AstBundle(Arc<ts_arena::StorageBundle<StoredNode>>);
impl AstBundle {
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn file(&self, index: usize) -> Option<AstFile> {
        self.0.file(index).map(AstFile)
    }
}

/// An explicitly retained file or mapped bundle. Nodes, list backing and the file
/// frame remain below this storage root, including after a node escapes its scope.
/// Raw arena writers cannot bypass AST validation after publication.
///
/// ```compile_fail
/// fn raw_storage(file: &ts_ast::AstFile) {
///     let _: &ts_arena::StorageHandle<ts_ast::Node> = file.storage();
/// }
/// ```
///
/// Generic arena storage has no AST graph-validation guarantee and cannot be
/// adopted as a published AST file.
///
/// ```compile_fail
/// fn unchecked_adoption(storage: ts_arena::StorageHandle<ts_ast::Node>) {
///     let _ = ts_ast::AstFile::from_storage(storage);
/// }
/// ```
#[derive(Clone, Debug)]
pub struct AstFile(pub(crate) StorageHandle<StoredNode>);
impl AstFile {
    pub fn view(&self) -> AstView<'_> {
        AstView(self.0.view(), None)
    }
    pub fn root(&self) -> Option<NodeId> {
        self.view().file_info().root
    }
    pub fn retain_node(&self, id: NodeId) -> Result<RetainedNode, Error> {
        let record = self.0.resolved_node(id)?.retain();
        let owner = self.0.view().for_arena(id.arena())?;
        let fallback = if id.arena() == owner.id().arena() {
            None
        } else {
            let aux = AuxId::from_parts(owner.lazy_auxiliary_arena(), record.ordinal)?;
            match AuxRead::resolved(owner.aux(aux)?, owner).full() {
                Some(AstStorageData::FallbackNode(node)) => Some(node.clone()),
                _ => return Err(Error::InvalidGraph),
            }
        };
        Ok(RetainedNode { record, fallback })
    }
    /// Follow a mapped-file or imported-file identity within this retention root.
    pub fn file(&self, id: ts_arena::FileId) -> Option<Self> {
        self.0.file(id).map(Self)
    }
}

/// An escaped AST node retaining its complete file/bundle and dependencies.
/// The private arena handle cannot expose unchecked lazy publication.
///
/// ```compile_fail
/// fn raw_owner(node: &ts_ast::RetainedNode) {
///     let _ = node.owner();
/// }
/// ```
#[derive(Clone, Debug)]
pub struct RetainedNode {
    record: ts_arena::RetainedRecord<StoredNode>,
    fallback: Option<Arc<Node>>,
}
impl RetainedNode {
    pub fn id(&self) -> NodeId {
        self.record.id()
    }
    /// Escaped lazy payloads already retain their stable backing; this read
    /// never reacquires a lazy publication lock or increments a reference count.
    pub fn read(&self) -> NodeRead<'_> {
        let owner = self
            .record
            .owner()
            .view()
            .for_arena(self.id().arena())
            .expect("retained node owner");
        if let Some(node) = &self.fallback {
            NodeRead::owned(self.id(), node, owner.id(), owner.source())
        } else {
            NodeRead::resolved(self.id(), &StorageRead::borrowed(&self.record), owner)
        }
    }
    pub fn file(&self) -> AstFile {
        AstFile(self.record.owner().clone())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AstView<'a>(
    pub(crate) StorageView<'a, StoredNode>,
    pub(crate) Option<&'a crate::BindResult>,
);
impl<'a> AstView<'a> {
    /// Development-only layout inventory. Excluded from normal production builds.
    #[cfg(feature = "layout-profile")]
    pub fn layout_profile(self) -> std::collections::BTreeMap<&'static str, usize> {
        let mut counts = std::collections::BTreeMap::new();
        let context = CompactContext {
            nodes: self.0.id().arena(),
            auxiliary: self.0.auxiliary_arena(),
            source: self.source(),
            store: self.0.store(),
        };
        for header in self.0.core_nodes() {
            let data = context.store.payloads.read(
                header.actual_shape(),
                header.ordinal,
                context,
                header.end,
            );
            *counts.entry(data.name()).or_default() += 1;
        }
        counts
    }
    pub fn for_node_owner(self, node: NodeId) -> Result<Self, Error> {
        self.0.for_node_owner(node).map(|view| Self(view, self.1))
    }
    pub fn source(self) -> &'a SourceText {
        self.0.source()
    }
    pub fn position_map(self) -> &'a ts_jsstring::PositionMap {
        self.0.position_map()
    }
    #[inline]
    pub fn node(self, id: NodeId) -> Result<NodeRead<'a>, Error> {
        if id.arena() == self.0.id().arena()
            && self.1.is_none_or(|binding| binding.reads_core_directly(id))
        {
            return Ok(NodeRead::core(
                id,
                self.0.core_node(id)?,
                self.0.physical_owner(),
            ));
        }
        self.node_compatibility(id)
    }
    #[inline(never)]
    fn node_compatibility(self, id: NodeId) -> Result<NodeRead<'a>, Error> {
        if let Some(node) = self
            .binding_for_node(id)?
            .and_then(|result| result.overlay(id))
        {
            // The overlay's identity was validated when it was inserted. Select
            // its retained owner without reading the parsed record or locking
            // its lazy directory a second time.
            let owner = self.0.for_arena(id.arena())?;
            return Ok(NodeRead::owned(id, node, owner.id(), owner.source()));
        }
        let (record, owner) = self.0.node_with_owner(id)?;
        Ok(NodeRead::resolved(id, &record, owner))
    }
    pub(crate) fn binding_for_node(
        self,
        id: NodeId,
    ) -> Result<Option<&'a crate::BindResult>, Error> {
        let Some(active) = self.1 else {
            return Ok(None);
        };
        if active.is_single_source_owner(id) {
            return Ok(Some(active));
        }
        let source = match self.owning_source(id) {
            Ok(source) => source,
            // A general AST read may inspect a parentless synthetic node. Only
            // bound retention requires an unambiguous logical source.
            Err(Error::InvalidGraph) => return Ok(None),
            Err(error) => return Err(error),
        };
        if source == active.source() {
            return Ok(Some(active));
        }
        let parsed = AstView(self.0, None);
        Ok(parsed.source_file(source)?.state_ref().binding.result())
    }
    /// Resolve a logical source through the immutable syntax parent chain. Do
    /// not guess an arena's canonical root for orphans or parent cycles.
    pub(crate) fn owning_source(self, id: NodeId) -> Result<NodeId, Error> {
        let mut slow = Some(id);
        let mut fast = Some(id);
        loop {
            let current = slow.ok_or(Error::InvalidGraph)?;
            let node = AstView(self.0, None).node(current)?;
            if node.kind() == crate::SyntaxKind::SourceFile {
                AstView(self.0, None).source_file(current)?;
                return Ok(current);
            }
            slow = node.parent();
            for _ in 0..2 {
                fast = match fast {
                    Some(id) => {
                        let node = AstView(self.0, None).node(id)?;
                        if node.kind() == crate::SyntaxKind::SourceFile {
                            None
                        } else {
                            node.parent()
                        }
                    }
                    None => None,
                };
            }
            if slow.is_some() && slow == fast {
                return Err(Error::InvalidGraph);
            }
        }
    }
    pub(crate) fn auxiliary(self, id: AuxId) -> Result<AuxRead<'a>, Error> {
        let (record, owner) = self.0.aux_with_owner(id)?;
        Ok(AuxRead::resolved(record, owner))
    }
    pub fn file_info(self) -> FileInfo {
        let id = self.0.metadata().expect("AST storage has a file frame");
        match self.auxiliary(id).expect("core file frame").full() {
            Some(AstStorageData::File(info)) => *info,
            _ => unreachable!("file frame record kind"),
        }
    }
    pub fn list(self, id: NodeListId) -> Result<NodeListRead<'a>, Error> {
        list_read(self.auxiliary(id.0)?)
    }
    // port: tsc/internal/ast/ast.go:NodeList.HasTrailingComma
    pub fn list_has_trailing_comma(self, id: NodeListId) -> Result<bool, Error> {
        let list = self.list(id)?;
        let nodes = self.node_slice(list.nodes())?;
        let Some(last) = nodes.last() else {
            return Ok(false);
        };
        let last = last.expect("nil last node in NodeList.HasTrailingComma");
        Ok(i64::from(self.node(last)?.end()) < list.loc().end())
    }
    pub fn node_slice(self, nodes: NodeSlice) -> Result<NodeSliceRead<'a>, Error> {
        let (record, compact) = match nodes.backing {
            Some(id) => {
                let (record, owner) = self.0.aux_with_owner(id)?;
                (
                    Some(AuxRead::resolved(record, owner)),
                    Some((&owner.store().edges, owner.id().arena())),
                )
            }
            None => (None, None),
        };
        node_slice_read(nodes, record, compact)
    }
    pub fn text_slice(self, text: TextSlice) -> Result<TextSliceRead<'a>, Error> {
        text_slice_read(text, text.backing.map(|id| self.auxiliary(id)).transpose()?)
    }
    pub fn eager_jsdoc(self, parent: NodeId) -> Result<Option<JSDocRoots>, Error> {
        Ok(self.0.eager_jsdoc(parent)?.map(JSDocRoots::from_shared))
    }
    pub fn source_eager_jsdoc(
        self,
        source: NodeId,
        parent: NodeId,
    ) -> Result<Option<JSDocRoots>, Error> {
        self.source_file(source)?;
        Ok(self
            .0
            .source_eager_jsdoc(source, parent)?
            .map(JSDocRoots::from_shared))
    }
    pub fn source_jsdoc(
        self,
        source: NodeId,
        parent: NodeId,
        initialize: impl FnOnce(&mut AstTransaction<'_, '_>) -> Result<Vec<NodeId>, Error>,
    ) -> Result<JSDocRoots, Error> {
        self.source_file(source)?;
        self.0
            .source_jsdoc(source, parent, |storage| {
                let mut transaction = AstTransaction {
                    storage,
                    node_count: 0,
                    text_count: 0,
                };
                let roots = initialize(&mut transaction)?;
                transaction.validate_staged()?;
                Ok(roots)
            })
            .map(JSDocRoots::from_shared)
    }
    /// Call this on the chosen execution thread before any lazy lock is acquired.
    /// The initializer may use only its transaction to read this file's lazy data.
    pub fn jsdoc(
        self,
        parent: NodeId,
        initialize: impl FnOnce(&mut AstTransaction<'_, '_>) -> Result<Vec<NodeId>, Error>,
    ) -> Result<JSDocRoots, Error> {
        self.0
            .jsdoc(parent, |storage| {
                let mut transaction = AstTransaction {
                    storage,
                    node_count: 0,
                    text_count: 0,
                };
                let roots = initialize(&mut transaction)?;
                transaction.validate_staged()?;
                Ok(roots)
            })
            .map(JSDocRoots::from_shared)
    }
    pub(crate) fn validate_data(self, data: &NodeData) -> Result<(), Error> {
        validate_data(
            data,
            |id| self.node(id).map(|_| ()),
            |id| self.list(id).map(|_| ()),
            |slice| self.node_slice(slice).map(|_| ()),
            |slice| self.text_slice(slice).map(|_| ()),
        )
    }
    fn validate_core(self) -> Result<(), Error> {
        self.validate_core_with_construction_edges(false)
    }
    /// Parent links and source metadata can change after construction and are
    /// always checked in the original node/auxiliary order. Only immutable or
    /// narrowly updated syntax edges can carry their construction proof here.
    fn validate_backing(self, backing: crate::compact::lists::CompactNodes) -> Result<(), Error> {
        let edges = &self.0.store().edges;
        let end = backing
            .start
            .checked_add(backing.len as usize)
            .ok_or(Error::InvalidGraph)?;
        let range = backing.start..end;
        if !edges.valid_range(range.clone()) {
            return Err(Error::InvalidGraph);
        }
        for node in edges.iter(self.0.id().arena(), range).flatten() {
            self.node(node)?;
        }
        Ok(())
    }
    fn validate_core_with_construction_edges(self, checked: bool) -> Result<(), Error> {
        let context = CompactContext {
            nodes: self.0.id().arena(),
            auxiliary: self.0.auxiliary_arena(),
            source: self.source(),
            store: self.0.store(),
        };
        for (index, header) in self.0.core_nodes().enumerate() {
            let id = NodeId::from_parts(
                self.0.id().arena(),
                u32::try_from(index + 1).map_err(|_| Error::InvalidSlot)?,
            )?;
            if let Some(parent) = context.decode_node(FieldKey::parent(id.slot()), header.parent) {
                self.node(parent)?;
            }
            if !checked {
                context.store.payloads.validate_references(
                    header,
                    context,
                    |id| self.node(id).map(|_| ()),
                    |id| self.list(id).map(|_| ()),
                    |slice| self.node_slice(slice).map(|_| ()),
                    |slice| self.text_slice(slice).map(|_| ()),
                )?;
            }
        }
        for record in self.0.core_auxiliary() {
            let value = self
                .0
                .store()
                .auxiliary
                .value(record, self.0.auxiliary_arena());
            let value = match value {
                AuxValue::List(list) => {
                    if !checked {
                        self.node_slice(list.nodes())?;
                    }
                    continue;
                }
                AuxValue::CompactNodes(backing) => {
                    if !checked {
                        self.validate_backing(backing)?;
                    }
                    continue;
                }
                AuxValue::Full(value) => value,
            };
            match value {
                AstStorageData::List(list) => {
                    if !checked {
                        self.node_slice(list.nodes())?;
                    }
                }
                AstStorageData::Nodes(nodes) => {
                    if !checked {
                        for &node in nodes.iter().flatten() {
                            self.node(node)?;
                        }
                    }
                }
                AstStorageData::CompactNodes(backing) => {
                    if !checked {
                        let edges = &self.0.store().edges;
                        let range = backing.start
                            ..backing
                                .start
                                .checked_add(backing.len as usize)
                                .ok_or(Error::InvalidGraph)?;
                        if !edges.valid_range(range.clone()) {
                            return Err(Error::InvalidGraph);
                        }
                        for node in edges.iter(self.0.id().arena(), range).flatten() {
                            self.node(node)?;
                        }
                    }
                }
                AstStorageData::FallbackNode(_) => return Err(Error::InvalidGraph),
                AstStorageData::SourceMetadata(data) => data.validate(self)?,
                AstStorageData::Text(_) => {}
                AstStorageData::File(info) => {
                    if let Some(root) = info.root {
                        self.node(root)?;
                    }
                    if let Some(map) = info.source_files {
                        if !matches!(
                            self.auxiliary(map)?.full(),
                            Some(AstStorageData::SourceFiles(_))
                        ) {
                            return Err(Error::InvalidGraph);
                        }
                    }
                }
                AstStorageData::SourceFiles(files) => {
                    for (&node, source) in files {
                        self.node(node)?;
                        source.validate_references(self)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// A borrowed transaction is never sent to a worker or used to reacquire its lock.
pub struct AstTransaction<'a, 'storage> {
    pub(crate) storage: &'a mut StorageTransaction<'storage, StoredNode>,
    pub(crate) node_count: i64,
    pub(crate) text_count: i64,
}
impl AstTransaction<'_, '_> {
    pub fn node(&self, id: NodeId) -> Result<NodeRead<'_>, Error> {
        let header = self.storage.node(id)?;
        if id.arena() == self.storage.owner_id().arena() {
            Ok(NodeRead::transaction_core(id, header, self.storage))
        } else {
            let aux = AuxId::from_parts(self.storage.lazy_auxiliary_arena(), header.ordinal)?;
            match self.auxiliary(aux)?.full_borrowed() {
                Some(AstStorageData::FallbackNode(node)) => Ok(NodeRead::owned(
                    id,
                    node,
                    self.storage.owner_id(),
                    self.storage.source(),
                )),
                _ => Err(Error::InvalidGraph),
            }
        }
    }
    pub fn node_mut(&mut self, id: NodeId) -> Result<NodeMut<'_>, Error> {
        let ordinal = self.storage.node_mut(id)?.ordinal;
        let aux = AuxId::from_parts(self.storage.lazy_auxiliary_arena(), ordinal)?;
        let (header, payload) = self.storage.node_and_aux_mut(id, aux)?;
        match payload {
            AstStorageData::FallbackNode(node) => Ok(NodeMut::lazy(
                Arc::get_mut(node).expect("unpublished payload is exclusive"),
                header,
            )),
            _ => Err(Error::InvalidGraph),
        }
    }
    pub(crate) fn push_node(&mut self, node: Node) -> NodeId {
        let header = Self::stage_node(self.storage, node);
        self.storage.push(header)
    }
    /// A token's header and payload, staged by the token cache under the lazy
    /// arena's publication lock.
    pub(crate) fn stage_token(
        storage: &mut StorageTransaction<'_, StoredNode>,
        node: Node,
    ) -> StoredNode {
        Self::stage_node(storage, node)
    }
    fn stage_node(storage: &mut StorageTransaction<'_, StoredNode>, node: Node) -> StoredNode {
        let mut header = StoredNode::fallback(&node, 0);
        let aux = storage.push_aux(AstStorageData::FallbackNode(Arc::new(node)));
        header.ordinal = aux.slot();
        header
    }
    fn auxiliary(&self, id: AuxId) -> Result<AuxRead<'_>, Error> {
        Ok(AuxRead::with_store(
            self.storage.aux(id)?,
            self.storage.store(),
            self.storage.core_auxiliary_arena(),
        ))
    }
    pub fn list(&self, id: NodeListId) -> Result<NodeListRead<'_>, Error> {
        list_read(self.auxiliary(id.0)?)
    }
    pub fn node_slice_read(&self, nodes: NodeSlice) -> Result<NodeSliceRead<'_>, Error> {
        node_slice_read(
            nodes,
            nodes.backing.map(|id| self.auxiliary(id)).transpose()?,
            Some((&self.storage.store().edges, self.storage.owner_id().arena())),
        )
    }
    pub fn text_slice_read(&self, text: TextSlice) -> Result<TextSliceRead<'_>, Error> {
        text_slice_read(text, text.backing.map(|id| self.auxiliary(id)).transpose()?)
    }
    pub fn node_slice(&mut self, nodes: Vec<Option<NodeId>>) -> Result<NodeSlice, Error> {
        for &id in nodes.iter().flatten() {
            self.storage.node(id)?;
        }
        let len = checked_len(nodes.len())?;
        let backing = self
            .storage
            .push_aux(AstStorageData::Nodes(nodes.into_boxed_slice()));
        Ok(NodeSlice {
            backing: Some(backing),
            start: 0,
            len,
        })
    }
    pub fn text_slice(&mut self, text: Vec<JsString>) -> Result<TextSlice, Error> {
        let len = checked_len(text.len())?;
        let backing = self
            .storage
            .push_aux(AstStorageData::Text(text.into_boxed_slice()));
        Ok(TextSlice {
            backing: Some(backing),
            start: 0,
            len,
        })
    }
    pub fn new_list(&mut self, loc: TextRange, nodes: NodeSlice) -> Result<NodeListId, Error> {
        self.node_slice_read(nodes)?;
        Ok(NodeListId(
            self.storage
                .push_aux(AstStorageData::List(NodeList::new(loc, nodes))),
        ))
    }
    pub fn list_mut(&mut self, id: NodeListId) -> Result<&mut NodeList, Error> {
        match self.storage.aux_mut(id.0)? {
            AstStorageData::List(list) => Ok(list),
            _ => Err(Error::InvalidGraph),
        }
    }
    pub fn set_list_nodes(&mut self, id: NodeListId, nodes: NodeSlice) -> Result<(), Error> {
        self.node_slice_read(nodes)?;
        self.list_mut(id)?.set_nodes(nodes);
        Ok(())
    }
    pub fn mark_list_missing(&mut self, id: NodeListId) -> Result<(), Error> {
        self.list_mut(id)?.set_missing(true);
        Ok(())
    }
    pub fn node_count(&self) -> i64 {
        self.node_count
    }
    pub fn text_count(&self) -> i64 {
        self.text_count
    }
    pub(crate) fn validate_data(&self, data: &NodeData) -> Result<(), Error> {
        validate_data(
            data,
            |id| self.node(id).map(|_| ()),
            |id| self.list(id).map(|_| ()),
            |slice| self.node_slice_read(slice).map(|_| ()),
            |slice| self.text_slice_read(slice).map(|_| ()),
        )
    }
    fn validate_staged(&self) -> Result<(), Error> {
        for header in self.storage.staged_nodes() {
            let aux = AuxId::from_parts(self.storage.lazy_auxiliary_arena(), header.ordinal)?;
            if !matches!(
                self.auxiliary(aux)?.full(),
                Some(AstStorageData::FallbackNode(_))
            ) {
                return Err(Error::InvalidGraph);
            }
        }
        for value in self.storage.staged_aux() {
            match value {
                AstStorageData::FallbackNode(node) => {
                    if let Some(parent) = node.parent() {
                        self.storage.node(parent)?;
                    }
                    self.validate_data(node.data())?;
                }
                AstStorageData::List(list) => {
                    self.node_slice_read(list.nodes())?;
                }
                AstStorageData::Nodes(nodes) => {
                    for &node in nodes.iter().flatten() {
                        self.storage.node(node)?;
                    }
                }
                AstStorageData::Text(_) => {}
                AstStorageData::CompactNodes(_)
                | AstStorageData::File(_)
                | AstStorageData::SourceFiles(_)
                | AstStorageData::SourceMetadata(_) => {
                    return Err(Error::InvalidGraph);
                }
            }
        }
        Ok(())
    }
}

fn checked_len(len: usize) -> Result<u32, Error> {
    u32::try_from(len).map_err(|_| Error::InvalidSlot)
}
fn list_read(record: AuxRead<'_>) -> Result<NodeListRead<'_>, Error> {
    record.list()?;
    Ok(NodeListRead(record))
}
fn node_slice_read<'a>(
    nodes: NodeSlice,
    record: Option<AuxRead<'a>>,
    compact: Option<(&'a crate::compact::lists::EdgePages, ts_arena::ArenaId)>,
) -> Result<NodeSliceRead<'a>, Error> {
    let mut start = nodes.start as usize;
    let len = nodes.len();
    let mut selected_compact = None;
    match &record {
        None if len == 0 && (start == 0 || nodes.is_missing()) => {}
        Some(record) => match record.value() {
            AuxValue::Full(AstStorageData::Nodes(values))
                if start
                    .checked_add(len)
                    .is_some_and(|end| end <= values.len()) => {}
            value @ (AuxValue::CompactNodes(_)
            | AuxValue::Full(AstStorageData::CompactNodes(_))) => {
                let backing = match value {
                    AuxValue::CompactNodes(value) => value,
                    AuxValue::Full(AstStorageData::CompactNodes(value)) => *value,
                    _ => unreachable!(),
                };
                let (edges, owner) = compact.ok_or(Error::InvalidGraph)?;
                if start
                    .checked_add(len)
                    .is_none_or(|end| end > backing.len as usize)
                {
                    return Err(Error::InvalidGraph);
                }
                start = backing
                    .start
                    .checked_add(start)
                    .ok_or(Error::InvalidGraph)?;
                let end = start.checked_add(len).ok_or(Error::InvalidGraph)?;
                if !edges.valid_range(start..end) {
                    return Err(Error::InvalidGraph);
                }
                selected_compact = Some((edges, owner));
            }
            _ => return Err(Error::InvalidGraph),
        },
        None => return Err(Error::InvalidGraph),
    }
    Ok(NodeSliceRead {
        record,
        compact: selected_compact,
        start,
        len,
    })
}
fn text_slice_read(
    text: TextSlice,
    record: Option<AuxRead<'_>>,
) -> Result<TextSliceRead<'_>, Error> {
    let start = text.start as usize;
    let len = text.len();
    match &record {
        None if len == 0 && start == 0 => {}
        Some(record) => match record.full() {
            Some(AstStorageData::Text(values))
                if start
                    .checked_add(len)
                    .is_some_and(|end| end <= values.len()) => {}
            _ => return Err(Error::InvalidGraph),
        },
        None => return Err(Error::InvalidGraph),
    }
    Ok(TextSliceRead { record, start, len })
}

fn validate_data(
    data: &NodeData,
    node: impl FnMut(NodeId) -> Result<(), Error>,
    list: impl FnMut(NodeListId) -> Result<(), Error>,
    raw: impl FnMut(NodeSlice) -> Result<(), Error>,
    text: impl FnMut(TextSlice) -> Result<(), Error>,
) -> Result<(), Error> {
    data.validate_references(node, list, raw, text)
}

impl AstBuilder {
    pub fn structural_bytes_with(&self, census: &mut ts_arena::StorageCensus) -> (usize, usize) {
        self.storage
            .structural_bytes_with(&crate::compact::CoreStore::structural_bytes_with, census)
    }
    /// Structural storage of this builder's arenas and stores: known bytes and
    /// the count of entries whose allocation extent is not exposed.
    pub fn structural_bytes(&self) -> (usize, usize) {
        self.storage
            .structural_bytes(crate::compact::CoreStore::structural_bytes)
    }
}

impl AstFile {
    pub fn structural_bytes_with(&self, census: &mut ts_arena::StorageCensus) -> (usize, usize) {
        self.0
            .structural_bytes_with(&crate::compact::CoreStore::structural_bytes_with, census)
    }
    /// Structural storage of the published file(s) behind this handle.
    pub fn structural_bytes(&self) -> (usize, usize) {
        self.0
            .structural_bytes(crate::compact::CoreStore::structural_bytes)
    }
}

#[cfg(test)]
mod validation_proof_tests {
    use super::*;
    use crate::{
        node_flags, BorrowedFactory, Factory, FactoryMethods, PrefixUnaryExpressionData,
        RuntimeFactory, SourceFileParseOptions, SyntaxKind,
    };

    fn parsed(counters: &Counters) -> (ParsedFile, NodeId) {
        let mut builder = AstBuilder::new(SourceText::default(), counters);
        let root = builder.new_token(SyntaxKind::Unknown.into());
        (builder.complete(root).unwrap(), root)
    }

    fn source(builder: &mut AstBuilder) -> NodeId {
        builder.new_source_file(
            SourceFileParseOptions {
                file_name: JsString::from_bytes(b"/proof.ts".as_slice()),
                ..SourceFileParseOptions::default()
            },
            SourceText::default(),
            None,
            None,
        )
    }

    #[test]
    fn construction_errors_and_narrow_list_edits_preserve_the_edge_proof() {
        let counters = Counters::new();
        let mut other = AstBuilder::new(SourceText::default(), &counters);
        let foreign = other.new_token(SyntaxKind::Unknown.into());
        let foreign_slice = other.node_slice(vec![Some(foreign)]).unwrap();
        let mut builder = AstBuilder::new(SourceText::default(), &counters);
        let root = builder.new_token(SyntaxKind::ExportKeyword.into());
        let before = builder.node_count();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            builder.new_prefix_unary_expression(SyntaxKind::PlusToken.into(), Some(foreign));
        }))
        .unwrap_err();
        assert_eq!(
            panic.downcast_ref::<String>().unwrap(),
            "factory edges belong to retained storage: WrongOwner"
        );
        assert_eq!(builder.node_count(), before);
        assert_eq!(
            builder.node_slice(vec![Some(foreign)]),
            Err(Error::WrongOwner)
        );
        assert_eq!(
            builder.new_list(TextRange::new(0, 1), foreign_slice),
            Err(Error::WrongOwner)
        );
        assert!(builder.construction_edges_valid);
        let nodes = builder.node_slice(vec![Some(root), None]).unwrap();
        let modifiers = builder.node_slice(vec![Some(root)]).unwrap();
        let list = {
            let mut factory = BorrowedFactory(&mut builder);
            let list = factory.new_modifier_list(modifiers);
            factory.set_list_location(list, TextRange::new(-1, 9));
            factory.set_list_modifier_flags(list, 0x8123_4567);
            factory.finish_node(root, TextRange::new(-1, 8), 1);
            factory.add_node_flags(root, 2);
            list
        };
        assert_eq!(
            builder.view().list(list).unwrap().loc(),
            TextRange::new(-1, 9)
        );
        assert_eq!(
            builder.view().list(list).unwrap().modifier_flags(),
            0x8123_4567
        );
        builder.set_list_nodes(list, nodes).unwrap();
        let copied = builder.clone_list(list).unwrap();
        builder.mark_list_missing(copied).unwrap();
        assert!(builder.view().list(copied).unwrap().is_missing());
        assert!(builder.construction_edges_valid);
        let parsed = builder.complete(root).unwrap();
        assert!(parsed.validated);
        assert_eq!(parsed.view().node(root).unwrap().flags(), 3);
        assert!(parsed.try_publish_unbound().is_ok());
    }

    #[test]
    fn clean_completion_still_checks_final_parents_and_mutable_metadata_in_order() {
        let counters = Counters::new();
        let mut other = AstBuilder::new(SourceText::default(), &counters);
        let foreign = other.new_token(SyntaxKind::Unknown.into());
        for case in 0..3 {
            let mut builder = AstBuilder::new(SourceText::default(), &counters);
            let orphan = builder.new_token(SyntaxKind::Unknown.into());
            let root = source(&mut builder);
            if case == 1 {
                let nodes = builder.source_nodes(vec![Some(root)]).unwrap();
                builder.source_nodes_mut(nodes).unwrap()[0] = Some(foreign);
            } else {
                builder
                    .source_file_mut(root)
                    .unwrap()
                    .external_module_indicator = Some(foreign);
            }
            if case == 2 {
                let missing = NodeId::from_parts(builder.id().arena(), u32::MAX).unwrap();
                // This setter must keep its delayed-error behavior.
                Factory::set_node_parent(&mut builder, orphan, Some(missing));
            }
            assert!(builder.construction_edges_valid);
            let expected = if case == 2 {
                Error::InvalidSlot
            } else {
                Error::WrongOwner
            };
            assert_eq!(builder.view().validate_core(), Err(expected));
            assert_eq!(builder.complete(root).unwrap_err(), expected);
        }
    }

    #[test]
    fn unrestricted_node_edits_force_the_original_scan_and_never_restore_the_proof() {
        let counters = Counters::new();
        let mut other = AstBuilder::new(SourceText::default(), &counters);
        let foreign = other.new_token(SyntaxKind::Unknown.into());
        let mut builder = AstBuilder::new(SourceText::default(), &counters);
        let first = builder.new_token(SyntaxKind::Unknown.into());
        let later = builder.new_token(SyntaxKind::Unknown.into());
        *builder.node_mut(first).unwrap().data_mut() = PrefixUnaryExpressionData {
            operator: SyntaxKind::PlusToken.into(),
            operand: Some(foreign),
        }
        .into();
        let missing = NodeId::from_parts(builder.id().arena(), u32::MAX).unwrap();
        Factory::set_node_parent(&mut builder, later, Some(missing));
        Factory::finish_node(&mut builder, first, TextRange::new(0, 1), 0);
        builder.new_token(SyntaxKind::Unknown.into());
        assert!(!builder.construction_edges_valid);
        // The first node's payload failure precedes the later parent's error.
        assert_eq!(builder.view().validate_core(), Err(Error::WrongOwner));
        assert_eq!(builder.complete(later).unwrap_err(), Error::WrongOwner);
    }

    #[test]
    fn unrestricted_list_edits_force_backing_validation_and_rejected_edits_stay_clean() {
        let counters = Counters::new();
        let mut other = AstBuilder::new(SourceText::default(), &counters);
        let foreign = other.new_token(SyntaxKind::Unknown.into());
        let foreign_slice = other.node_slice(vec![Some(foreign)]).unwrap();
        let foreign_list = other.new_list(TextRange::new(0, 1), foreign_slice).unwrap();
        let mut builder = AstBuilder::new(SourceText::default(), &counters);
        let root = builder.new_token(SyntaxKind::Unknown.into());
        let list = builder
            .new_list(TextRange::new(0, 0), NodeSlice::empty())
            .unwrap();
        assert!(matches!(builder.node_mut(foreign), Err(Error::WrongOwner)));
        assert!(matches!(
            builder.list_mut(foreign_list),
            Err(Error::WrongOwner)
        ));
        assert!(builder.construction_edges_valid);
        *builder.list_mut(list).unwrap() = NodeList::new(TextRange::new(0, 1), foreign_slice);
        builder
            .set_list_location(list, TextRange::new(1, 2))
            .unwrap();
        builder.set_list_modifier_flags(list, 0).unwrap();
        builder.new_token(SyntaxKind::Unknown.into());
        assert!(!builder.construction_edges_valid);
        assert_eq!(builder.view().validate_core(), Err(Error::WrongOwner));
        assert_eq!(builder.complete(root).unwrap_err(), Error::WrongOwner);
    }

    #[test]
    fn hooks_and_imports_keep_full_completion_validation() {
        struct Hook(NodeId);
        impl FactoryHooks for Hook {
            fn on_create(&self, factory: &mut dyn Factory, node: NodeId) {
                *factory.node_mut(node).data_mut() = PrefixUnaryExpressionData {
                    operator: SyntaxKind::PlusToken.into(),
                    operand: Some(self.0),
                }
                .into();
            }
        }
        let counters = Counters::new();
        let mut other = AstBuilder::new(SourceText::default(), &counters);
        let foreign = other.new_token(SyntaxKind::Unknown.into());
        let mut hooked =
            AstBuilder::with_hooks(SourceText::default(), &counters, Arc::new(Hook(foreign)));
        let root = hooked.new_token(SyntaxKind::Unknown.into());
        assert!(!hooked.construction_edges_valid);
        assert_eq!(hooked.complete(root).unwrap_err(), Error::WrongOwner);
        let imported = other.complete(foreign).unwrap().publish_unbound();
        let mut builder = AstBuilder::new(SourceText::default(), &counters);
        builder.retain_file(imported);
        assert!(!builder.construction_edges_valid);
        let root = builder.new_prefix_unary_expression(SyntaxKind::PlusToken.into(), Some(foreign));
        assert!(builder.complete(root).is_ok());
    }

    #[test]
    fn lazy_initializers_keep_staged_validation_and_disable_core_only_completion() {
        let counters = Counters::new();
        let mut other = AstBuilder::new(SourceText::default(), &counters);
        let foreign = other.new_token(SyntaxKind::Unknown.into());
        let mut builder = AstBuilder::new(SourceText::default(), &counters);
        let root = builder.new_token(SyntaxKind::Unknown.into());
        let error = builder.view().jsdoc(root, |transaction| {
            let node = transaction.new_token(SyntaxKind::Unknown.into());
            *transaction.node_mut(node)?.data_mut() = PrefixUnaryExpressionData {
                operator: SyntaxKind::PlusToken.into(),
                operand: Some(foreign),
            }
            .into();
            Ok(vec![node])
        });
        assert!(matches!(error, Err(Error::WrongOwner)));
        // Failed lazy slots remain reserved so their identities are never reused.
        assert!(!builder.storage.is_core_only());
        let roots = builder
            .view()
            .jsdoc(root, |transaction| {
                Ok(vec![transaction.new_token(SyntaxKind::Unknown.into())])
            })
            .unwrap();
        assert!(!roots.is_empty());
        assert!(!builder.storage.is_core_only());
        assert!(builder.complete(root).is_ok());
    }

    #[test]
    fn narrow_flag_writes_preserve_the_completed_core_proof() {
        let counters = Counters::new();
        let baseline = counters.snapshot();
        let (mut parsed, root) = parsed(&counters);
        assert!(parsed.validated);
        parsed
            .set_node_flags(root, node_flags::UNREACHABLE)
            .unwrap();
        assert!(parsed.validated);
        assert_eq!(
            parsed.view().node(root).unwrap().flags(),
            node_flags::UNREACHABLE
        );
        let file = parsed.try_publish_unbound().unwrap();
        assert_eq!(
            file.view().node(root).unwrap().flags(),
            node_flags::UNREACHABLE
        );
        drop(file);
        assert_eq!(counters.snapshot(), baseline);
    }

    #[test]
    fn narrow_flag_writes_do_not_restore_a_dirty_core_proof() {
        let counters = Counters::new();
        let (mut parsed, root) = parsed(&counters);
        // Even a valid unrestricted mutation conservatively invalidates the
        // proof. A later narrow write must never turn that false back to true.
        parsed
            .builder_mut()
            .node_mut(root)
            .unwrap()
            .set_flags(node_flags::AMBIENT);
        assert!(!parsed.validated);
        parsed
            .set_node_flags(root, node_flags::UNREACHABLE)
            .unwrap();
        assert!(!parsed.validated);
        assert!(parsed.try_publish_unbound().is_ok());
    }

    #[test]
    fn narrow_flag_writes_reject_foreign_owners_without_changing_the_proof() {
        let counters = Counters::new();
        let (mut local, root) = parsed(&counters);
        let (foreign, foreign_root) = parsed(&counters);
        assert_eq!(
            local.set_node_flags(foreign_root, node_flags::UNREACHABLE),
            Err(Error::WrongOwner)
        );
        assert!(local.validated);
        assert_eq!(local.view().node(root).unwrap().flags(), 0);
        assert_eq!(foreign.view().node(foreign_root).unwrap().flags(), 0);
    }
}
