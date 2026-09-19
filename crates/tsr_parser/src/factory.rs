use crate::list_buffer::ListBuffer;
use ts_ast::{
    AstBuilder, AstTransaction, BorrowedFactory, JsString, NodeListId, NodeSlice, RuntimeFactory,
    TextSlice,
};

/// Eager and lazy parsers share AST factory/list operations. These additional
/// methods select parser construction buffers and expose text backing and
/// exclusive list mutation.
pub(crate) trait ParserFactory: RuntimeFactory {
    fn start_list_buffer(&self) -> ListBuffer {
        ListBuffer::heap()
    }
    fn finish_list_buffer(&mut self, nodes: ListBuffer) -> NodeSlice {
        if nodes.is_empty() {
            NodeSlice::empty()
        } else {
            self.alloc_nodes(nodes.into_vec().into_iter().map(Some).collect())
        }
    }
    fn alloc_text(&mut self, text: Vec<JsString>) -> TextSlice;
    fn set_list_nodes(&mut self, id: NodeListId, nodes: NodeSlice);
    fn mark_list_missing(&mut self, id: NodeListId);
}
impl ParserFactory for AstBuilder {
    fn start_list_buffer(&self) -> ListBuffer {
        ListBuffer::inline()
    }
    fn finish_list_buffer(&mut self, nodes: ListBuffer) -> NodeSlice {
        if nodes.is_empty() {
            return NodeSlice::empty();
        }
        if let Some(values) = nodes.inline_nodes() {
            self.node_slice_from_slice(values)
                .expect("factory slice edges")
        } else {
            self.alloc_nodes(nodes.into_vec().into_iter().map(Some).collect())
        }
    }
    fn alloc_text(&mut self, text: Vec<JsString>) -> TextSlice {
        self.text_slice(text).expect("parser text fits its owner")
    }
    fn set_list_nodes(&mut self, id: NodeListId, nodes: NodeSlice) {
        AstBuilder::set_list_nodes(self, id, nodes).expect("parser owns mutable list");
    }
    fn mark_list_missing(&mut self, id: NodeListId) {
        AstBuilder::mark_list_missing(self, id).expect("parser owns missing list");
    }
}
impl ParserFactory for AstTransaction<'_, '_> {
    fn alloc_text(&mut self, text: Vec<JsString>) -> TextSlice {
        self.text_slice(text)
            .expect("parser text fits its transaction")
    }
    fn set_list_nodes(&mut self, id: NodeListId, nodes: NodeSlice) {
        AstTransaction::set_list_nodes(self, id, nodes).expect("parser owns staged list");
    }
    fn mark_list_missing(&mut self, id: NodeListId) {
        AstTransaction::mark_list_missing(self, id).expect("parser owns missing staged list");
    }
}

impl<F: ParserFactory + ?Sized> ParserFactory for BorrowedFactory<'_, F> {
    fn start_list_buffer(&self) -> ListBuffer {
        self.0.start_list_buffer()
    }
    fn finish_list_buffer(&mut self, nodes: ListBuffer) -> NodeSlice {
        self.0.finish_list_buffer(nodes)
    }
    fn alloc_text(&mut self, text: Vec<JsString>) -> TextSlice {
        self.0.alloc_text(text)
    }
    fn set_list_nodes(&mut self, id: NodeListId, nodes: NodeSlice) {
        self.0.set_list_nodes(id, nodes);
    }
    fn mark_list_missing(&mut self, id: NodeListId) {
        self.0.mark_list_missing(id);
    }
}

#[cfg(test)]
#[path = "factory_tests.rs"]
mod tests;
