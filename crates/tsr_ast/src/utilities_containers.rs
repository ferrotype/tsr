//! AST utilities: containers, function flags and outer expressions.
//!
//! Ports of `tsc/internal/ast/utilities.go`, witnessed by the `containers` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).
use crate::{modifier_flags, node_flags, AstView, NodeId, SyntaxKind as K};
use tsr_arena::Error;

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// Go's `FunctionFlags`.
pub mod function_flags {
    pub const NORMAL: u32 = 0;
    pub const GENERATOR: u32 = 1 << 0;
    pub const ASYNC: u32 = 1 << 1;
    pub const INVALID: u32 = 1 << 2;
    pub const ASYNC_GENERATOR: u32 = ASYNC | GENERATOR;
}

/// port: tsc/internal/ast/functionflags.go:GetFunctionFlags
pub fn get_function_flags(view: AstView<'_>, node: Option<NodeId>) -> Result<u32, Error> {
    use function_flags as F;
    let Some(node) = node else {
        return Ok(F::INVALID);
    };
    let read = view.node(node)?;
    let data = read.data_source();
    // The kinds whose data embeds BodyBase.
    let asterisk = match read.kind().known() {
        Some(K::FunctionDeclaration) => data
            .as_function_declaration()
            .ok_or(Error::InvalidGraph)?
            .asterisk_token(),
        Some(K::FunctionExpression) => data
            .as_function_expression()
            .ok_or(Error::InvalidGraph)?
            .asterisk_token(),
        Some(K::MethodDeclaration) => data
            .as_method_declaration()
            .ok_or(Error::InvalidGraph)?
            .asterisk_token(),
        Some(
            K::ArrowFunction
            | K::Constructor
            | K::GetAccessor
            | K::SetAccessor
            | K::ModuleDeclaration,
        ) => None,
        _ => return Ok(F::INVALID),
    };
    let mut flags = F::NORMAL;
    match read.kind().known() {
        Some(
            K::FunctionDeclaration
            | K::FunctionExpression
            | K::MethodDeclaration
            | K::ArrowFunction,
        ) => {
            if asterisk.is_some() {
                flags |= F::GENERATOR;
            }
            if crate::utilities::has_syntactic_modifier(view, node, modifier_flags::ASYNC)? {
                flags |= F::ASYNC;
            }
        }
        _ => {}
    }
    if read.body().is_none() {
        flags |= F::INVALID;
    }
    Ok(flags)
}

/// Recurses once per nested statement level.
/// port: tsc/internal/ast/utilities.go:ForEachReturnStatement
pub fn for_each_return_statement(
    view: AstView<'_>,
    body: NodeId,
    visitor: &mut dyn FnMut(NodeId) -> bool,
) -> Result<bool, Error> {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        Ok(match view.node(body)?.kind().known() {
            Some(K::ReturnStatement) => visitor(body),
            Some(
                K::CaseBlock
                | K::Block
                | K::IfStatement
                | K::DoStatement
                | K::WhileStatement
                | K::ForStatement
                | K::ForInStatement
                | K::ForOfStatement
                | K::WithStatement
                | K::SwitchStatement
                | K::CaseClause
                | K::DefaultClause
                | K::LabeledStatement
                | K::TryStatement
                | K::CatchClause,
            ) => {
                for child in crate::source_file_tables::children(view, body)? {
                    if for_each_return_statement(view, child, visitor)? {
                        return Ok(true);
                    }
                }
                false
            }
            _ => false,
        })
    })
}

/// Go reads `node.Parent`, so the source file panics.
/// port: tsc/internal/ast/utilities.go:GetContainingFunction
pub fn get_containing_function(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>, Error> {
    let parent = view.node(node)?.parent().expect(NIL);
    crate::utilities::find_ancestor(view, Some(parent), |current| {
        crate::utilities::is_function_like(Some(current))
    })
}

/// port: tsc/internal/ast/utilities.go:GetEnclosingBlockScopeContainer
pub fn get_enclosing_block_scope_container(
    view: AstView<'_>,
    node: NodeId,
) -> Result<Option<NodeId>, Error> {
    let mut current = view.node(node)?.parent();
    while let Some(id) = current {
        let read = view.node(id)?;
        if is_block_scope(view, id, read.parent())? {
            return Ok(Some(id));
        }
        current = read.parent();
    }
    Ok(None)
}

/// port: tsc/internal/ast/utilities.go:GetNewTargetContainer
pub fn get_new_target_container(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>, Error> {
    let container = crate::binder_helpers::get_this_container(view, node, false, false)?;
    Ok(matches!(
        view.node(container)?.kind().known(),
        Some(K::Constructor | K::FunctionDeclaration | K::FunctionExpression)
    )
    .then_some(container))
}

/// port: tsc/internal/ast/utilities.go:GetReparsedNodeForNode
pub fn get_reparsed_node_for_node(
    view: AstView<'_>,
    node: Option<NodeId>,
) -> Result<Option<NodeId>, Error> {
    let Some(id) = node else {
        return Ok(None);
    };
    let read = view.node(id)?;
    if read.flags() & node_flags::JS_DOC != 0 && read.flags() & node_flags::REPARSED == 0 {
        if let Some(file) = crate::utilities::get_source_file_of_node(view, Some(id))? {
            let clones = view.source_file(file)?.reparsed_clones.clone();
            if !clones.is_empty() {
                let mut found = false;
                let mut low = 0;
                let mut high = clones.len();
                while low < high {
                    let middle = low + (high - low) / 2;
                    let order = crate::utilities_middle::compare_node_positions(
                        &view.node(clones[middle])?,
                        &read,
                    );
                    if order < 0 {
                        low = middle + 1;
                    } else {
                        if order == 0 {
                            found = true;
                        }
                        high = middle;
                    }
                }
                let mut position = low;
                if !found && position > 0 {
                    position -= 1;
                }
                let candidate = clones[position];
                let candidate_read = view.node(candidate)?;
                if contained_by(
                    read.pos(),
                    read.end(),
                    candidate_read.pos(),
                    candidate_read.end(),
                ) {
                    if let Some(reparsed) = find_clone_in_node(view, candidate, id)? {
                        return Ok(Some(reparsed));
                    }
                }
            }
        }
    }
    Ok(Some(id))
}

/// Go's `TextRange.ContainedBy`.
fn contained_by(pos: i32, end: i32, outer_pos: i32, outer_end: i32) -> bool {
    pos >= outer_pos && end <= outer_end
}

/// port: tsc/internal/ast/utilities.go:findCloneInNode
fn find_clone_in_node(
    view: AstView<'_>,
    node: NodeId,
    original: NodeId,
) -> Result<Option<NodeId>, Error> {
    let original_read = view.node(original)?;
    let mut current = node;
    loop {
        let read = view.node(current)?;
        if read.kind() == original_read.kind()
            && read.pos() == original_read.pos()
            && read.end() == original_read.end()
        {
            return Ok(Some(current));
        }
        let mut next = None;
        for child in crate::source_file_tables::children(view, current)? {
            let child_read = view.node(child)?;
            if contained_by(
                original_read.pos(),
                original_read.end(),
                child_read.pos(),
                child_read.end(),
            ) {
                next = Some(child);
                break;
            }
        }
        match next {
            Some(child) => current = child,
            None => return Ok(None),
        }
    }
}

/// port: tsc/internal/ast/utilities.go:GetSuperContainer
pub fn get_super_container(
    view: AstView<'_>,
    node: NodeId,
    stop_on_functions: bool,
) -> Result<Option<NodeId>, Error> {
    let mut current = view.node(node)?.parent();
    while let Some(id) = current {
        let read = view.node(id)?;
        let mut next = id;
        match read.kind().known() {
            Some(K::ComputedPropertyName) => next = read.parent().expect(NIL),
            Some(K::FunctionDeclaration | K::FunctionExpression | K::ArrowFunction) => {
                if stop_on_functions {
                    return Ok(Some(id));
                }
            }
            Some(
                K::PropertyDeclaration
                | K::PropertySignature
                | K::MethodDeclaration
                | K::MethodSignature
                | K::Constructor
                | K::GetAccessor
                | K::SetAccessor
                | K::ClassStaticBlockDeclaration,
            ) => return Ok(Some(id)),
            Some(K::Decorator) => {
                let parent = read.parent().expect(NIL);
                let parent_read = view.node(parent)?;
                if parent_read.kind() == K::Parameter
                    && crate::utilities::is_class_element(
                        &view.node(parent_read.parent().expect(NIL))?,
                    )
                {
                    next = parent_read.parent().expect(NIL);
                } else if crate::utilities::is_class_element(&parent_read) {
                    next = parent;
                }
            }
            _ => {}
        }
        current = view.node(next)?.parent();
    }
    Ok(None)
}

/// Go reads `Node.Parameters`, which panics outside the function-like kinds.
/// port: tsc/internal/ast/utilities.go:HasContextSensitiveParameters
pub fn has_context_sensitive_parameters(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    if read.type_parameter_list().is_none() {
        let parameters: Vec<NodeId> = view
            .node_slice(read.parameters(view)?)?
            .iter()
            .flatten()
            .collect();
        for &parameter in &parameters {
            if view.node(parameter)?.type_node().is_none() {
                return Ok(true);
            }
        }
        if read.kind() != K::ArrowFunction {
            let first = parameters.first().copied();
            let explicit_this = match first {
                Some(parameter) => crate::utilities_class::is_this_parameter(view, parameter)?,
                None => false,
            };
            if !explicit_this {
                return Ok(read.flags() & node_flags::CONTAINS_THIS != 0);
            }
        }
    }
    Ok(false)
}

/// port: tsc/internal/ast/utilities.go:IsBlockScope
pub fn is_block_scope(
    view: AstView<'_>,
    node: NodeId,
    parent_node: Option<NodeId>,
) -> Result<bool, Error> {
    Ok(match view.node(node)?.kind().known() {
        Some(
            K::SourceFile
            | K::CaseBlock
            | K::CatchClause
            | K::ModuleDeclaration
            | K::ForStatement
            | K::ForInStatement
            | K::ForOfStatement
            | K::Constructor
            | K::MethodDeclaration
            | K::GetAccessor
            | K::SetAccessor
            | K::FunctionDeclaration
            | K::FunctionExpression
            | K::ArrowFunction
            | K::PropertyDeclaration
            | K::ClassStaticBlockDeclaration,
        ) => true,
        Some(K::Block) => {
            let parent = parent_node.map(|parent| view.node(parent)).transpose()?;
            !crate::utilities::is_function_like_or_class_static_block_declaration(parent.as_ref())
        }
        _ => false,
    })
}
