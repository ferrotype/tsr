//! One complete contextual argument operation over live checker state.
//!
//! An argument's node and contextual type do not capture its semantic inputs:
//! the active inference, flow, expression mode, and checker caches all remain
//! live. Experiments invoke this same operation at its original call site.

use crate::{type_flags as tf, CheckerState, Error, InferenceId, TypeId};
use tsr_arena::NodeId;
use tsr_ast::SyntaxKind as K;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ArgumentRequest {
    pub(crate) node: NodeId,
    pub(crate) contextual: TypeId,
    pub(crate) inference: Option<InferenceId>,
    pub(crate) mode: u32,
}

pub(crate) fn check(state: &mut CheckerState, request: ArgumentRequest) -> Result<TypeId, Error> {
    check_with_expression(state, request, CheckerState::check_expression_ex)
}

// port: tsc/internal/checker/checker.go:Checker.checkExpressionWithContextualType
fn check_with_expression(
    state: &mut CheckerState,
    request: ArgumentRequest,
    expression: impl FnOnce(&mut CheckerState, NodeId, u32) -> Result<TypeId, Error>,
) -> Result<TypeId, Error> {
    let ArgumentRequest {
        node,
        contextual,
        inference,
        mode,
    } = request;
    let context_node = contextual_node_of(state, node)?;
    state.calls.contexts.push(crate::calls::ArgumentContext {
        node: context_node,
        ty: Some(contextual),
        is_cache: false,
    });
    state
        .calls
        .inference_contexts
        .push((context_node, inference));
    let result = (|| {
        let mode = mode | 1 | if inference.is_some() { 2 } else { 0 };
        let mut ty = expression(state, node, mode)?;
        if let Some(inference) = inference {
            state
                .inference_context_mut(inference)?
                .intra_expression_sites
                .clear();
        }
        // Go short-circuits before contextual instantiation. Non-literals
        // cannot lose freshness, so instantiating their context is unused
        // work and can create otherwise unrequested types.
        if state.maybe_type_of_kind(ty, tf::LITERAL)? {
            let contextual =
                state.instantiate_call_contextual_type(contextual, inference, false)?;
            if state.literal_of_context(ty, Some(contextual))? {
                ty = state.get_regular_type_of_literal_type(ty)?;
            }
        }
        Ok(ty)
    })();
    state.calls.inference_contexts.pop();
    state.calls.contexts.pop();
    result
}

/// Attributes with children apply their context to the containing element,
/// encompassing both the attributes and children.
// port: tsc/internal/checker/checker.go:Checker.getContextNode
fn contextual_node_of(state: &CheckerState, node: NodeId) -> Result<NodeId, Error> {
    let read = state.node(node)?;
    if read.kind() == K::JsxAttributes {
        let parent = read
            .parent()
            .ok_or(Error::MissingLink("JSX attributes parent"))?;
        if state.node(parent)?.kind() != K::JsxSelfClosingElement {
            return state
                .node(parent)?
                .parent()
                .ok_or(Error::MissingLink("JSX opening element parent"));
        }
    }
    Ok(node)
}

/// Select and retain one immutable core owner for this complete operation.
/// Nested contextual operations reuse it; foreign, factory and lazy reads
/// retain their ordinary routing. Retention and release are part of this call.
/// As in the ordinary checker, a panic retires the enclosing checker operation;
/// only normal results and `Error` returns restore reusable state here.
#[cfg(feature = "operation-experiments")]
pub(crate) fn check_scoped(
    state: &mut CheckerState,
    request: ArgumentRequest,
) -> Result<TypeId, Error> {
    if state.argument_read_scope.is_some() {
        return check(state, request);
    }
    let scope = crate::program::ArgumentReadScope::for_node(state, request.node);
    let Some(scope) = scope else {
        return check(state, request);
    };
    check_in_scope(state, request, scope)
}

/// Run the actual expression callbacks with explicit reusable continuations.
/// Nested contextual calls inherit the mode; returning an ordinary error must
/// restore the caller's selection just as it restores its contextual stacks.
#[cfg(feature = "operation-experiments")]
pub(crate) fn check_execution(
    state: &mut CheckerState,
    request: ArgumentRequest,
) -> Result<TypeId, Error> {
    let previous = std::mem::replace(&mut state.argument_explicit_execution, true);
    let result = check_scoped(state, request);
    state.argument_explicit_execution = previous;
    result
}

#[cfg(feature = "operation-experiments")]
fn check_in_scope(
    state: &mut CheckerState,
    request: ArgumentRequest,
    scope: crate::program::ArgumentReadScope,
) -> Result<TypeId, Error> {
    let previous = state.argument_read_scope.replace(scope);
    let result = check(state, request);
    state.argument_read_scope = previous;
    result
}

#[cfg(test)]
mod tests;
