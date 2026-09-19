"""Add named binder observations to a copied, exact accepted CP1 source tree.

This is a source-level diagnostic: it changes code generation and adds recorder
work. Events are successful operations at the named sites, not a claim to cover
every binder access or a basis for timing attribution. The recorder owns the
active single-worker binding domain; calls outside it are suppressed.
"""
from hashlib import sha256
from pathlib import Path


SOURCE_SHA256 = {
    "crates/tsr_binder/src/state.rs":
        "68d0c8210ec3fbe34eaa12230552d1e6721352f0b25ce9762a91d45e58962c1d",
    "crates/tsr_binder/src/dispatch.rs":
        "bdf74dc270a8097f6faf0eda83c27f9ad313899f8db7ce901105016f6bf95efe",
    "crates/tsr_binder/src/containers.rs":
        "b21df5d50164231d193886e2238db680bea8582566d20c2ef4649e66a116c459",
}

# Every replacement contains the original access exactly once. Recording never
# calls a second node getter to obtain an ID, kind, flag, or payload. Metadata in
# syntax events comes from the already copied NodeSlice descriptor.
REPLACEMENTS = {
    "crates/tsr_binder/src/state.rs": [
        (
            '''    pub fn n(&self, id: NodeId) -> NodeRead<'_> {
        self.builder.node(id).expect("binder node is retained")
    }''',
            '''    pub fn n(&self, id: NodeId) -> NodeRead<'_> {
        let node = self.builder.node(id).expect("binder node is retained");
        tsr_ast::access_trace::event(100, 100, id.bits(), 0, 0, 0);
        node
    }''',
        ),
        (
            '''    pub fn set_flags(&mut self, node: NodeId, flags: u32) {
        if self.n(node).flags() != flags {
            self.builder
                .set_node_flags(node, flags)
                .expect("binder writes its own file");
        }
    }''',
            '''    pub fn set_flags(&mut self, node: NodeId, flags: u32) {
        let previous_flags = self.n(node).flags();
        tsr_ast::access_trace::event(102, 105, node.bits(), u64::from(previous_flags), 0, 0);
        if previous_flags != flags {
            self.builder
                .set_node_flags(node, flags)
                .expect("binder writes its own file");
            tsr_ast::access_trace::event(103, 106, node.bits(), u64::from(flags), 0, 0);
        }
    }''',
        ),
    ],
    "crates/tsr_binder/src/dispatch.rs": [
        (
            '''        let mut has_error = self.n(node).flags() & nf::THIS_NODE_HAS_ERROR != 0;''',
            '''        let node_flags = self.n(node).flags();
        tsr_ast::access_trace::event(102, 103, node.bits(), u64::from(node_flags), 0, 0);
        let mut has_error = node_flags & nf::THIS_NODE_HAS_ERROR != 0;''',
        ),
        (
            '''                self.n(node).flags() | nf::THIS_NODE_OR_ANY_SUB_NODES_HAS_ERROR,''',
            '''                (|node_flags: u32| {
                    tsr_ast::access_trace::event(102, 104, node.bits(), u64::from(node_flags), 0, 0);
                    node_flags | nf::THIS_NODE_OR_ANY_SUB_NODES_HAS_ERROR
                })(self.n(node).flags()),''',
        ),
        (
            '''        let kind = self.n(node).kind();
        match kind.known() {''',
            '''        let kind = self.n(node).kind();
        // Preserve all open i16 kind bits; this is not a SyntaxKind conversion.
        tsr_ast::access_trace::event(101, 101, node.bits(), u64::from(kind.raw() as u16), 0, 0);
        match kind.known() {''',
        ),
        (
            '''                    .is_none_or(|id| self.n(id).kind() != K::SourceFile)''',
            '''                    .is_none_or(|id| {
                        let kind = self.n(id).kind();
                        tsr_ast::access_trace::event(101, 102, id.bits(), u64::from(kind.raw() as u16), 0, 0);
                        kind != K::SourceFile
                    })''',
        ),
    ],
    "crates/tsr_binder/src/containers.rs": [
        (
            '''        let _ = parsed.node_slice(nodes).expect("retained syntax slice");
        nodes''',
            '''        let _ = parsed.node_slice(nodes).expect("retained syntax slice");
        tsr_ast::access_trace::event(
            104, 107, list.map_or(0, |id| id.bits()),
            nodes.backing_id().map_or(0, |id| id.bits()),
            u64::from(nodes.start()), nodes.len() as u64,
        );
        nodes''',
        ),
        (
            '''    pub(crate) fn syntax_node(&self, nodes: NodeSlice, index: usize) -> Option<NodeId> {
        self.parsed_view()
            .node_slice(nodes)
            .expect("retained syntax slice")[index]
    }''',
            '''    pub(crate) fn syntax_node(&self, nodes: NodeSlice, index: usize) -> Option<NodeId> {
        let node = self.parsed_view()
            .node_slice(nodes)
            .expect("retained syntax slice")[index];
        // NodeSlice stores u32 start/len; a successful index is below that len.
        tsr_ast::access_trace::event(
            105, 108, nodes.backing_id().map_or(0, |id| id.bits()),
            (u64::from(nodes.start()) << 32) | nodes.len() as u64,
            index as u64, node.map_or(0, |id| id.bits()),
        );
        node
    }''',
        ),
        (
            '''        parsed.node_slice(nodes).expect("retained syntax slice")
    }
    // port: tsc/internal/binder/binder.go:Binder.bindContainer''',
            '''        let values = parsed.node_slice(nodes).expect("retained syntax slice");
        tsr_ast::access_trace::event(
            106, 109, list.map_or(0, |id| id.bits()),
            nodes.backing_id().map_or(0, |id| id.bits()),
            u64::from(nodes.start()), nodes.len() as u64,
        );
        values
    }
    // port: tsc/internal/binder/binder.go:Binder.bindContainer''',
        ),
        (
            '''            self.builder
                .set_node_flow(node, flow)
                .expect("binder flow and target belong to result");''',
            '''            self.builder
                .set_node_flow(node, flow)
                .expect("binder flow and target belong to result");
            tsr_ast::access_trace::event(107, 110, node.bits(), flow.map_or(0, |id| id.bits()), 0, 0);''',
        ),
    ],
}


def apply(stage):
    """Validate all three original files, then modify a separate staging tree.

    No writes occur on source/hash/replacement drift. This is not a transaction
    against filesystem write failures; callers discard a failed staging tree.
    """
    stage = Path(stage).resolve(strict=True)
    repository = Path(__file__).resolve().parents[4]
    if stage == repository or (stage / ".git").exists():
        raise ValueError("binder hooks require an isolated staging copy")
    # A frozen build's source directory is evidence, not a disposable stage.
    if stage.name == "source" and (stage.parent / "manifest.json").exists():
        raise ValueError("binder hooks must not modify a frozen source bundle")
    changed = {}
    for name, expected in SOURCE_SHA256.items():
        path = stage / name
        if path.resolve(strict=True) != path:
            raise ValueError(f"binder hook input must not be symlinked: {name}")
        original = path.read_bytes()
        if sha256(original).hexdigest() != expected:
            raise ValueError(f"binder hook source changed: {name}")
        content = original.decode("utf-8")
        for before, after in REPLACEMENTS[name]:
            if content.count(before) != 1:
                raise ValueError(f"binder hook expected one exact source match: {name}")
            content = content.replace(before, after, 1)
        changed[name] = content
    for name, content in changed.items():
        (stage / name).write_text(content, encoding="utf-8")
    return sorted(changed)
