//! `transformers/estransforms/optionalcatch.go`: a `catch` clause without a
//! binding gets a temporary one.
use super::utilities::subtree_facts;
use crate::transformer::{Failure, TransformOptions, Transformer};
use std::cell::RefCell;
use std::rc::Rc;
use tsr_ast::{subtree_flags, FactoryMethods, NodeId, NodeVisitor, SyntaxKind as K};
use tsr_printer::EmitContext;

struct OptionalCatchTransformer {
    emit_context: RefCell<EmitContext>,
    failure: Failure,
}

impl OptionalCatchTransformer {
    // port: tsc/internal/transformers/estransforms/optionalcatch.go:optionalCatchTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        if self.failure.is_set() {
            return node;
        }
        let id = node.expect("runtime error: invalid memory address or nil pointer dereference");
        let Some(facts) = self.failure.ok(subtree_facts(visitor.factory(), id)) else {
            return node;
        };
        if facts & subtree_flags::MISSING_CATCH_CLAUSE_VARIABLE == 0 {
            return node;
        }
        match visitor.factory().node(id).kind().known() {
            Some(K::CatchClause) => self.visit_catch_clause(visitor, id),
            _ => visitor.visit_each_child(node),
        }
    }

    // port: tsc/internal/transformers/estransforms/optionalcatch.go:optionalCatchTransformer.visitCatchClause
    fn visit_catch_clause(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> Option<NodeId> {
        let (variable_declaration, block) = {
            let read = visitor.factory().node(node);
            let data = read.as_catch_clause().expect("CatchClause payload");
            (data.variable_declaration(), data.block())
        };
        if variable_declaration.is_none() {
            let name = self
                .emit_context
                .borrow_mut()
                .new_temp_variable(visitor.factory_mut());
            let declaration =
                visitor
                    .factory_mut()
                    .new_variable_declaration(Some(name), None, None, None);
            let block = visit(visitor, block);
            return Some(
                visitor
                    .factory_mut()
                    .new_catch_clause(Some(declaration), block),
            );
        }
        visitor.visit_each_child(Some(node))
    }
}

/// `NodeVisitor.Visit`: the visit function itself, without `VisitNode`'s
/// handling of a nil node or a lifted list.
fn visit(visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
    let visit = visitor
        .visit
        .expect("runtime error: invalid memory address or nil pointer dereference");
    visit(visitor, node)
}

// port: tsc/internal/transformers/estransforms/optionalcatch.go:newOptionalCatchTransformer
pub fn new_optional_catch_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let tx = Rc::new(OptionalCatchTransformer {
        emit_context: RefCell::new(opts.context.clone()),
        failure: opts.failure.clone(),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| tx.visit(visitor, node),
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}
