//! `transformers/modifiervisitor.go`.
use crate::transformer::{Failure, Transformer};
use tsr_ast::{modifier_flags, modifier_to_flag, NodeId, NodeListId, NodeVisitor, RuntimeFactory};
use tsr_printer::EmitContext;

// port: tsc/internal/transformers/modifiervisitor.go:modifierVisitor.visit
fn visit(
    allowed_modifiers: u32,
    visitor: &mut NodeVisitor<'_>,
    node: Option<NodeId>,
) -> Option<NodeId> {
    let id = node.expect("runtime error: invalid memory address or nil pointer dereference");
    let flags = modifier_to_flag(visitor.factory().node(id).kind());
    if flags != modifier_flags::NONE && flags & allowed_modifiers == 0 {
        return None;
    }
    node
}

/// The modifiers of `modifiers` that `allowed` admits; a list with every
/// modifier admitted is returned unchanged.
// port: tsc/internal/transformers/modifiervisitor.go:ExtractModifiers
pub fn extract_modifiers(
    emit_context: &EmitContext,
    factory: &mut dyn RuntimeFactory,
    modifiers: Option<NodeListId>,
    allowed: u32,
) -> Option<NodeListId> {
    modifiers?;
    let tx = Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| visit(allowed, visitor, node),
        Some(emit_context.clone()),
        Failure::default(),
    );
    tx.visitor(factory).visit_modifiers(modifiers)
}
