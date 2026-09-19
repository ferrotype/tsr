//! Everything a checker mutates: the S08 subset of the `Checker` struct's fields
//! (`tsc/internal/checker/checker.go`). Reachable only through an
//! [`crate::Operation`], so inside it the checker is a plain single-threaded
//! `&mut` state machine (ADR 0008).

use crate::{
    Builtins, Error, LinkStore, ResolutionStack, SignatureStore, TypeId, TypeStore,
    ValueSymbolLinks,
};
use tsr_arena::{CheckerIdentity, Counters, NodeId, SymbolArena, SymbolId};
use tsr_ast::{AstBuilder, DeclarationLists, Symbol, SymbolTables};
use tsr_jsstring::SourceText;

/// The compiler options the P1 constructors read. `strictNullChecks` and
/// `exactOptionalPropertyTypes` are upstream's effective values
/// (`GetStrictOptionValue`); the compiler host supplies them in P2.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckerOptions {
    pub strict_null_checks: bool,
    pub exact_optional_property_types: bool,
}

pub(crate) struct CheckerState {
    pub(crate) counters: Counters,
    pub(crate) options: CheckerOptions,
    pub(crate) program: Option<crate::program::ProgramContext>,
    /// Checker-owned symbols: merged clones, transient and synthetic symbols.
    /// Its arena identity is the checker identity (`CheckerIdentity::id`).
    pub(crate) symbols: SymbolArena<Symbol>,
    /// `Checker.SymbolCount`.
    pub(crate) symbol_count: u32,
    /// Member tables the checker builds; disposed with the owner.
    pub(crate) tables: SymbolTables,
    pub(crate) declarations: DeclarationLists,
    pub(crate) merged_symbols: crate::types::Map<SymbolId, SymbolId>,
    pub(crate) deferred_checks: crate::deferred_checks::DeferredChecks,
    pub(crate) diagnostics: crate::diagnostics::DiagnosticStore,
    pub(crate) suggestions: crate::diagnostics::DiagnosticStore,
    pub(crate) serialization_level: u32,
    pub(crate) display_builder: crate::node_builder::cache::CachedBuilder,
    pub(crate) synthetic_scopes: crate::emit_scopes::SyntheticScopes,
    pub(crate) emit: crate::emit_visibility::EmitState,
    pub(crate) emit_checks: crate::emit_checks::EmitCheckState,
    pub(crate) calls: crate::calls::CallState,
    pub(crate) expression_mode: u32,
    pub(crate) promises: crate::promises::PromiseState,
    pub(crate) iteration: crate::iteration::IterationState,
    pub(crate) module_aliases: crate::module_aliases::ModuleAliasState,
    pub(crate) bindings: crate::bindings::BindingState,
    pub(crate) within_unreachable_code: bool,
    pub(crate) current_node: Option<NodeId>,
    pub(crate) late_members: crate::late_members::LateMemberState,
    pub(crate) query: crate::query::QueryState,
    pub(crate) inference: crate::inference::InferenceStore,
    pub(crate) conditional: crate::conditional::ConditionalState,
    pub(crate) variance: crate::variance::VarianceState,
    pub(crate) relations: crate::relater::Relations,
    pub(crate) instantiation: crate::instantiate::InstantiationState,
    pub(crate) flow: crate::flow::FlowAnalysis,
    pub(crate) enums: crate::enums::EnumState,
    pub(crate) body_checks: crate::check_bodies::BodyCheckState,
    pub(crate) source_checks: crate::types::Map<NodeId, crate::check::SourceCheckStatus>,
    pub(crate) types: TypeStore,
    pub(crate) signatures: SignatureStore,
    pub(crate) resolution: ResolutionStack,
    pub(crate) mapped_symbol_links: LinkStore<SymbolId, crate::mapped::MappedSymbolLinks>,
    pub(crate) value_symbol_links: LinkStore<SymbolId, ValueSymbolLinks>,
    /// `Checker.factory`: the checker's own synthetic AST arena, distinct from
    /// the node builder's. Synthetic signature declarations and synthetic
    /// expressions live here and share the owner's lifetime (plan §4.3).
    pub(crate) factory: AstBuilder,
    /// The checker type a synthetic expression embeds (`SyntheticExpression.Type`).
    pub(crate) synthetic_expression_types: crate::types::Map<NodeId, TypeId>,
    pub(crate) builtins: Builtins,
}

impl CheckerState {
    /// Adopts the identity's reserved arena number as the checker symbol arena
    /// and runs the type-creating part of `NewChecker`.
    pub(crate) fn new(
        identity: &CheckerIdentity,
        counters: &Counters,
        options: CheckerOptions,
    ) -> Result<Self, Error> {
        let mut state = Self::bare(identity, counters, options)?;
        state.initialize()?;
        Ok(state)
    }

    /// The state before `NewChecker` creates anything: empty stores and sentinel
    /// builtins. Only the storage pilot, which numbers its own types from 1 the
    /// way its Go counterpart does, uses it without `initialize`.
    pub(crate) fn bare(
        identity: &CheckerIdentity,
        counters: &Counters,
        options: CheckerOptions,
    ) -> Result<Self, Error> {
        let symbols = identity.adopt_symbol_arena(counters)?;
        let builtins = Builtins::uninitialized(symbols.id());
        Ok(Self {
            counters: counters.clone(),
            options,
            program: None,
            symbols,
            symbol_count: 0,
            tables: SymbolTables::new(counters),
            declarations: DeclarationLists::new(counters),
            merged_symbols: crate::types::Map::default(),
            deferred_checks: crate::deferred_checks::DeferredChecks::default(),
            diagnostics: crate::diagnostics::DiagnosticStore::default(),
            suggestions: crate::diagnostics::DiagnosticStore::default(),
            serialization_level: 0,
            display_builder: crate::node_builder::cache::CachedBuilder::default(),
            synthetic_scopes: crate::emit_scopes::SyntheticScopes::default(),
            emit: crate::emit_visibility::EmitState::default(),
            emit_checks: crate::emit_checks::EmitCheckState::default(),
            calls: crate::calls::CallState::default(),
            expression_mode: 0,
            promises: crate::promises::PromiseState::default(),
            iteration: crate::iteration::IterationState::default(),
            module_aliases: crate::module_aliases::ModuleAliasState::default(),
            bindings: crate::bindings::BindingState::default(),
            current_node: None,
            within_unreachable_code: false,
            query: crate::query::QueryState::default(),
            late_members: crate::late_members::LateMemberState::default(),
            instantiation: crate::instantiate::InstantiationState::default(),
            inference: crate::inference::InferenceStore::default(),
            conditional: crate::conditional::ConditionalState::default(),
            variance: crate::variance::VarianceState::default(),
            relations: crate::relater::Relations::default(),
            flow: crate::flow::FlowAnalysis::default(),
            enums: crate::enums::EnumState::default(),
            body_checks: crate::check_bodies::BodyCheckState::default(),
            source_checks: crate::types::Map::default(),
            types: TypeStore::new(),
            signatures: SignatureStore::new(),
            resolution: ResolutionStack::new(),
            value_symbol_links: LinkStore::new(),
            mapped_symbol_links: LinkStore::new(),
            factory: AstBuilder::new(SourceText::from_bytes(&b""[..]), counters),
            synthetic_expression_types: crate::types::Map::default(),
            builtins,
        })
    }

    #[cfg(test)]
    pub fn symbols(&self) -> &SymbolArena<Symbol> {
        &self.symbols
    }
    #[cfg(test)]
    pub fn types(&self) -> &TypeStore {
        &self.types
    }
    #[cfg(test)]
    pub fn resolution_mut(&mut self) -> &mut ResolutionStack {
        &mut self.resolution
    }
}
