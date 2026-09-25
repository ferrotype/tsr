//! AST utilities: names, type and expression positions, access kinds.
//!
//! Ports of `tsc/internal/ast/utilities.go`, witnessed by the `positions` group
//! of the Phase 1 operation tables (`docs/PHASE1-mutation-witnesses.md`,
//! section 9).
use crate::{AstView, NodeId};
use tsr_arena::Error;

/// Go reads `name.Parent` unconditionally: every parsed node other than the
/// source file has a parent, and the source file is rejected first, so a
/// parentless node here is the nil dereference Go would panic on.
/// port: tsc/internal/ast/utilities.go:IsDeclarationName
pub fn is_declaration_name(view: AstView<'_>, name: NodeId) -> Result<bool, Error> {
    let node = view.node(name)?;
    if crate::is_source_file(&node) || crate::utilities::is_binding_pattern(&node) {
        return Ok(false);
    }
    let parent = view.node(
        node.parent()
            .expect("runtime error: invalid memory address or nil pointer dereference"),
    )?;
    Ok(crate::is_declaration(&parent) && parent.name() == Some(name))
}
