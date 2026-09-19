use super::*;
use ts_arena::Counters;
use ts_ast::{
    Factory, FactoryMethods, NodeData, NodeId, NodeKind, NodeList, NodeListRead, NodeMut, NodeRead,
    NodeSliceRead, SyntaxKind,
};
use ts_core::TextRange;
use ts_jsstring::SourceText;

struct RecordingFactory {
    inner: AstBuilder,
    allocations: Vec<(usize, usize)>,
}
impl RecordingFactory {
    fn new() -> Self {
        Self {
            inner: AstBuilder::new(SourceText::default(), &Counters::new()),
            allocations: Vec::new(),
        }
    }
}
impl Factory for RecordingFactory {
    fn node(&self, id: NodeId) -> NodeRead<'_> {
        self.inner.node(id)
    }
    fn node_count(&self) -> i64 {
        self.inner.node_count()
    }
    fn text_count(&self) -> i64 {
        self.inner.text_count()
    }
    fn node_mut(&mut self, id: NodeId) -> NodeMut<'_> {
        Factory::node_mut(&mut self.inner, id)
    }
    fn new_node(&mut self, kind: NodeKind, data: NodeData) -> NodeId {
        self.inner.new_node(kind, data)
    }
    fn increment_text_count(&mut self) {
        self.inner.increment_text_count();
    }
    fn set_node_flags(&mut self, id: NodeId, flags: u32) {
        Factory::set_node_flags(&mut self.inner, id, flags);
    }
    fn finish_update(&mut self, updated: NodeId, original: NodeId) -> NodeId {
        self.inner.finish_update(updated, original)
    }
    fn finish_clone(&mut self, updated: NodeId, original: NodeId) -> NodeId {
        self.inner.finish_clone(updated, original)
    }
}
impl RuntimeFactory for RecordingFactory {
    fn read_list(&self, id: NodeListId) -> NodeListRead<'_> {
        self.inner.read_list(id)
    }
    fn read_nodes(&self, nodes: NodeSlice) -> NodeSliceRead<'_> {
        self.inner.read_nodes(nodes)
    }
    fn alloc_nodes(&mut self, nodes: Vec<Option<NodeId>>) -> NodeSlice {
        self.allocations
            .push((nodes.as_ptr().addr(), nodes.capacity()));
        self.inner.alloc_nodes(nodes)
    }
    fn alloc_list(&mut self, loc: TextRange, nodes: NodeSlice) -> NodeListId {
        self.inner.alloc_list(loc, nodes)
    }
    fn mutable_list(&mut self, id: NodeListId) -> &mut NodeList {
        self.inner.mutable_list(id)
    }
    fn clone_source(&mut self, original: NodeId) -> NodeId {
        self.inner.clone_source(original)
    }
    fn update_source(
        &mut self,
        original: NodeId,
        statements: Option<NodeListId>,
        eof: Option<NodeId>,
    ) -> NodeId {
        self.inner.update_source(original, statements, eof)
    }
}
impl ParserFactory for RecordingFactory {
    fn alloc_text(&mut self, text: Vec<JsString>) -> TextSlice {
        self.inner.alloc_text(text)
    }
    fn set_list_nodes(&mut self, id: NodeListId, nodes: NodeSlice) {
        ParserFactory::set_list_nodes(&mut self.inner, id, nodes);
    }
    fn mark_list_missing(&mut self, id: NodeListId) {
        ParserFactory::mark_list_missing(&mut self.inner, id);
    }
}

fn finish_heap_buffer(factory: &mut impl ParserFactory, ids: &[NodeId]) -> (usize, usize) {
    let mut buffer = factory.start_list_buffer();
    assert!(matches!(buffer, ListBuffer::Heap(_)));
    for &id in ids {
        buffer.push(id);
    }
    let ListBuffer::Heap(nodes) = &buffer else {
        unreachable!("default parser buffer stays on the heap");
    };
    let allocation = (nodes.as_ptr().addr(), nodes.capacity());
    let slice = factory.finish_list_buffer(buffer);
    assert_eq!(
        factory.read_nodes(slice).iter().collect::<Vec<_>>(),
        ids.iter().copied().map(Some).collect::<Vec<_>>()
    );
    assert!(factory
        .finish_list_buffer(factory.start_list_buffer())
        .is_nil());
    allocation
}

#[test]
fn custom_and_borrowed_custom_keep_default_heap_dispatch_and_vec_allocation() {
    let mut factory = RecordingFactory::new();
    let first = factory.new_token(SyntaxKind::Unknown.into());
    let second = factory.new_token(SyntaxKind::EndOfFile.into());
    let ids = [first, second, first];
    let direct = finish_heap_buffer(&mut factory, &ids);
    assert_eq!(factory.allocations, vec![direct]);
    let borrowed = finish_heap_buffer(&mut BorrowedFactory(&mut factory), &ids);
    assert_eq!(factory.allocations, vec![direct, borrowed]);
}

#[test]
fn borrowed_builder_forwards_inline_opt_in_and_preserves_empty_nil() {
    let mut builder = AstBuilder::new(SourceText::default(), &Counters::new());
    let child = builder.new_token(SyntaxKind::Unknown.into());
    let mut factory = BorrowedFactory(&mut builder);
    let mut buffer = factory.start_list_buffer();
    assert!(matches!(buffer, ListBuffer::Inline { .. }));
    buffer.push(child);
    let nodes = factory.finish_list_buffer(buffer);
    assert_eq!(factory.read_nodes(nodes).at(0), Some(child));
    assert!(!nodes.is_nil());
    assert!(factory
        .finish_list_buffer(factory.start_list_buffer())
        .is_nil());
}

#[test]
fn lazy_transaction_keeps_default_heap_buffer_and_publishes_its_edges() {
    let mut builder = AstBuilder::new(SourceText::default(), &Counters::new());
    let parent = builder.new_token(SyntaxKind::EndOfFile.into());
    let child = builder.new_token(SyntaxKind::Unknown.into());
    let file = builder.complete(parent).unwrap().publish_unbound();
    let root = file
        .view()
        .jsdoc(parent, |transaction| {
            let mut buffer = transaction.start_list_buffer();
            assert!(matches!(buffer, ListBuffer::Heap(_)));
            buffer.push(child);
            let nodes = transaction.finish_list_buffer(buffer);
            assert_eq!(transaction.read_nodes(nodes).at(0), Some(child));
            assert!(transaction
                .finish_list_buffer(transaction.start_list_buffer())
                .is_nil());
            let list = transaction.new_list(TextRange::new(0, 1), nodes)?;
            let root = transaction.new_array_literal_expression(Some(list), false);
            transaction.node_mut(root)?.set_parent(Some(parent));
            Ok(vec![root])
        })
        .unwrap()[0];
    let list = file.view().node(root).unwrap().element_list().unwrap();
    assert_eq!(
        file.view()
            .node_slice(file.view().list(list).unwrap().nodes())
            .unwrap()
            .at(0),
        Some(child)
    );
}
