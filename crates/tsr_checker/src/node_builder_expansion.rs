//! Hover expansion control (`VerbosityContext`, the context's
//! `maxExpansionDepth`, `depth` and `typeStack`). A request without a
//! verbosity has a maximum expansion depth of -1 and expands nothing; level 0
//! detects expandability without expanding and level N expands N named types
//! deep. Only the verbosity-taking printer entry points set one.
use super::NodeBuilder;
use crate::{object_flags as of, type_flags as tf, CheckerState, Error, TypeId};
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{symbol_flags as sf, SyntaxKind as K};

/// `VerbosityContext`: the requested expansion level and truncation length,
/// and the two signals a request reports back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VerbosityContext {
    /// 0 detects expandability without expanding; N expands N levels.
    pub level: i32,
    /// 0 uses the default truncation length.
    pub max_truncation_length: usize,
    /// Output: a higher level would reveal more.
    pub can_increase_verbosity: bool,
    /// Output: the output was truncated.
    pub truncated: bool,
}

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.propagateVerbosityOut
    /// Copies the request's expansion signals into the verbosity, only ever
    /// setting them, as several calls share one context.
    pub(crate) fn propagate_verbosity_out(&mut self) {
        if let Some(verbosity) = self.verbosity.as_mut() {
            if self.can_increase_expansion_depth {
                verbosity.can_increase_verbosity = true;
            }
            if self.expansion_truncated {
                verbosity.truncated = true;
            }
        }
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.exitContextCheck
    /// A request that was not allowed to truncate reports that it did.
    pub(crate) fn exit_context_check(&mut self) {
        if self.truncating && self.flags & tsr_nodebuilder::flags::NO_TRUNCATION != 0 {
            self.report(tsr_printer::emit_resolver::DeclarationTrackerEvent::Truncation);
        }
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.exitContext
    /// Ends a public request: its verbosity signals go out, a forbidden
    /// truncation is reported and an unsuccessful result is absent.
    pub(crate) fn exit_context(&mut self, result: NodeId) -> Option<NodeId> {
        self.propagate_verbosity_out();
        self.exit_context_check();
        (!self.encountered_error).then_some(result)
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.exitContextSlice
    pub(crate) fn exit_context_slice(&mut self, result: Vec<NodeId>) -> Option<Vec<NodeId>> {
        self.propagate_verbosity_out();
        self.exit_context_check();
        (!self.encountered_error).then_some(result)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.checkTruncationLengthIfExpanding
    pub(crate) fn check_truncation_if_expanding(&mut self) -> bool {
        if self.max_expansion_depth >= 0 && self.check_truncation() {
            self.expansion_truncated = true;
            return true;
        }
        false
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.isExpandableType
    fn is_expandable_type(&mut self, ty: TypeId, is_alias: bool) -> Result<bool, Error> {
        if is_alias {
            let symbol = crate::type_display::alias_symbol(self.checker.types.alias_of(ty)?);
            return Ok(!self.checker.is_lib_symbol_for_hover_verbosity(symbol)?);
        }
        if self.checker.is_lib_type_for_hover_verbosity(ty)? {
            return Ok(false);
        }
        let record = *self.checker.types.get(ty)?;
        if record.flags & tf::ENUM_LIKE != 0
            || record.object_flags & (of::REFERENCE | of::CLASS_OR_INTERFACE) != 0
        {
            return Ok(true);
        }
        if record.object_flags & of::ANONYMOUS != 0 {
            if let Some(symbol) = record.symbol {
                return Ok(self.checker.symbol(symbol)?.flags()
                    & (sf::CLASS | sf::ENUM | sf::VALUE_MODULE | sf::FUNCTION | sf::METHOD)
                    != 0);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.isTypeOnStack
    /// Whether `ty` is being expanded already, excluding the last entry (the
    /// type `type_node` is serializing now).
    fn is_type_on_stack(&self, ty: TypeId) -> bool {
        let len = self.type_stack.len();
        len > 1 && self.type_stack[..len - 1].contains(&Some(ty))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.shouldExpandType
    /// Whether to expand `ty` at this depth; at the boundary it records that a
    /// higher level would reveal more.
    pub(crate) fn should_expand_type(&mut self, ty: TypeId, is_alias: bool) -> Result<bool, Error> {
        if self.max_expansion_depth < 0 {
            return Ok(false);
        }
        if !self.is_expandable_type(ty, is_alias)? || self.is_type_on_stack(ty) {
            return Ok(false);
        }
        if self.depth < self.max_expansion_depth {
            return Ok(true);
        }
        self.can_increase_expansion_depth = true;
        Ok(false)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.isActivelyExpanding
    /// Whether this depth expands, so annotation reuse is skipped.
    pub(crate) fn is_actively_expanding(&self) -> bool {
        self.max_expansion_depth > 0 && self.depth < self.max_expansion_depth
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.checkTypeExpandability
    /// After annotation reuse, where `should_expand_type` never ran: probes
    /// `ty` and the type arguments of a reference.
    pub(crate) fn check_type_expandability(&mut self, ty: Option<TypeId>) -> Result<(), Error> {
        let Some(ty) = ty else {
            return Ok(());
        };
        if self.max_expansion_depth < 0 || self.can_increase_expansion_depth {
            return Ok(());
        }
        self.type_stack.push(Some(ty));
        let result = self.check_type_expandability_worker(ty);
        self.type_stack.pop();
        result
    }

    fn check_type_expandability_worker(&mut self, ty: TypeId) -> Result<(), Error> {
        if self.is_type_on_stack(ty) {
            return Ok(());
        }
        if self.checker.types.alias_of(ty)?.is_some() {
            self.should_expand_type(ty, true)?;
        }
        if !self.can_increase_expansion_depth {
            self.should_expand_type(ty, false)?;
        }
        if self.can_increase_expansion_depth {
            return Ok(());
        }
        if self.checker.types.get(ty)?.object_flags & of::REFERENCE != 0 {
            for argument in self.checker.get_type_arguments(ty)?.iter().copied() {
                self.check_type_expandability(Some(argument))?;
                if self.can_increase_expansion_depth {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/nodecopy.go:NodeBuilderImpl.walkNodeForExpandability
    /// Probes a reused annotation's type references, predicates and import
    /// types until one could be expanded.
    pub(crate) fn walk_node_for_expandability(&mut self, node: NodeId) -> Result<(), Error> {
        if self.can_increase_expansion_depth {
            return Ok(());
        }
        let kind = self.checker.node(node)?.kind();
        if matches!(
            kind.known(),
            Some(
                K::TypeReference
                    | K::ExpressionWithTypeArguments
                    | K::TypePredicate
                    | K::ImportType
            )
        ) {
            if let Some(ty) = self.reuse_type_from_node(node, false)? {
                self.check_type_expandability(Some(ty))?;
                if self.can_increase_expansion_depth {
                    return Ok(());
                }
            }
        }
        for child in self.checker.source_children(node)? {
            self.walk_node_for_expandability(child)?;
            if self.can_increase_expansion_depth {
                break;
            }
        }
        Ok(())
    }
}

impl CheckerState {
    // port: tsc/internal/checker/services.go:Checker.IsLibSymbolForHoverVerbosity
    /// Whether a declaration of `symbol` is in a default library file.
    pub(crate) fn is_lib_symbol_for_hover_verbosity(
        &self,
        symbol: Option<SymbolId>,
    ) -> Result<bool, Error> {
        let Some(symbol) = symbol else {
            return Ok(false);
        };
        let host = self.program()?.host.clone();
        for declaration in self.symbol_declarations(symbol)?.iter().flatten() {
            let view = self.ast(declaration)?;
            if let Some(file) =
                tsr_ast::utilities::get_source_file_of_node(view, Some(declaration))?
            {
                let path = view.source_file(file)?.parse_options().path.clone();
                if host.is_source_file_default_library(path.as_bytes()) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/services.go:Checker.IsLibTypeForHoverVerbosity
    /// Whether `ty` is declared in a library file (Array, Promise) or is a
    /// tuple; hover treats those as opaque.
    pub(crate) fn is_lib_type_for_hover_verbosity(&self, ty: TypeId) -> Result<bool, Error> {
        let record = self.types.get(ty)?;
        let symbol = if record.object_flags & of::REFERENCE != 0 {
            self.types.get(self.types.target(ty)?)?.symbol
        } else {
            record.symbol
        };
        if self.is_lib_symbol_for_hover_verbosity(symbol)? {
            return Ok(true);
        }
        self.is_tuple_type(ty)
    }
}
