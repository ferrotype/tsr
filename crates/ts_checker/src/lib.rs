//! The checker (`tsc/internal/checker`): checker-owned symbols, types and
//! signatures over S07's immutable bound files, with `.types` and `.errors.txt`
//! parity on the frozen subset as S08's acceptance.
//!
//! S08 P1 storage and the P2/P3 production semantic slices. What exists:
//!
//! - [`CheckerOwner`]: one owner, one exclusive operation, the reserved checker
//!   identity adopted as the symbol arena exactly once (ADR 0007, plan §4.1).
//! - [`ResolutionStack`]: the `pushTypeResolution` cycle guard, keyed on entity
//!   and property, ported exactly (ADR 0008).
//! - [`LinkStore`]: checker-local side tables keyed by arena identity and slot,
//!   paged on first use (`links.go`).
//! - [`TypeStore`]: the common type record, per-kind payload tables, aliases,
//!   `Arc` lists and the interning caches; [`SignatureStore`] for signatures,
//!   index infos and type predicates.
//! - The type-creating prefix of `NewChecker`, and the constructors it and the
//!   frozen storage trace need: intrinsics, string/number/bigint/boolean
//!   literals with fresh/regular links, anonymous object types, tuple targets,
//!   type references, type parameters, unions and ordered intersections, template
//!   literal types, synthetic call signatures and synthetic expressions. Every
//!   constructor sorts and interns the way upstream does.
//! - Flags and enums transcribed from `types.go`, checked against values read
//!   out of the pinned Go package (`data/s08/checker-flag-observations.json`).
//! - [`CheckerHost`]: the program surface the checker requires, mapped method by
//!   method to the retained compiler program.
//!
//! - Program initialization, checker-local source-symbol merges, primitive and
//!   anonymous-object source queries, and bounded source checking with structured
//!   diagnostics. Type display builds syntax through `ts_nodebuilder` and prints
//!   it through `ts_printer`.
//!
//! Source arrays/tuples, full relations/body checking, loaded generic library
//! operations and declaration emit remain pending. Unported branches return
//! named `Error::Unsupported` failures. The named P2 programs do not certify the
//! frozen E2 denominator; see `docs/S08-P2.md` and `docs/S08-P3.md`.
//!
//! Design notes: `docs/design/symbols.md`, `docs/design/ownership.md`; plan:
//! `docs/S08-implementation-plan.md`.

mod access_expressions;
mod access_symbols;
mod apparent;
mod arithmetic;
mod array_literals;
mod arrays;
mod assertions;
mod assignment_context;
mod assignment_declarations;
mod await_expressions;
mod binary;
mod binding_checks;
mod binding_rest;
mod bindings;
mod call_arguments;
mod call_errors;
mod call_failure;
mod call_spread;
mod call_tagged;
mod calls;
mod check;
mod check_bodies;
mod check_generics;
mod check_indexes;
mod check_interfaces;
mod check_statements;
mod check_type_syntax;
mod class_accessibility;
mod class_check;
mod class_context;
mod class_expressions;
mod class_grammar;
mod class_members;
mod class_overrides;
mod class_properties;
mod class_property_flow;
mod classes;
#[cfg(feature = "relation-probe")]
mod comparator_probe;
mod compare;
mod compound_signatures;
mod conditional;
mod constraints;
mod construct;
mod constructor_checks;
mod declaration_checks;
mod deferred_checks;
mod delete_expressions;
mod destructuring_assignments;
mod diagnostics;
mod discriminants;
mod element_errors;
mod emit_checks;
mod emit_reference;
mod emit_resolver;
mod emit_scopes;
mod emit_visibility;
mod enum_eval;
mod enums;
mod expression_context;
mod expression_errors;
mod expressions;
mod external_aliases;
mod external_resolution;
mod flags;
mod flow;
mod flow_arrays;
mod flow_assignments;
mod flow_destructuring;
mod flow_discriminant;
mod flow_effects;
mod flow_equality;
mod flow_facts;
mod flow_initial;
mod flow_instanceof;
mod flow_narrow;
mod flow_predicates;
mod flow_reference;
mod flow_switch;
mod flow_symbol;
mod function_symbol_checks;
mod generators;
mod grammar_lists;
mod grammar_modifiers;
mod grammar_variables;
mod handles;
mod higher_order_inference;
mod host;
mod ids;
mod import_attributes;
mod import_calls;
mod import_types;
mod index_access_errors;
mod indexes;
mod infer_candidates;
mod infer_constraints;
mod infer_helpers;
mod infer_matching;
mod infer_objects;
mod infer_reverse;
mod infer_signatures;
mod infer_templates;
mod infer_tuples;
mod infer_types;
mod inference;
mod init;
mod instanceof;
mod instantiate;
mod instantiation_expressions;
mod intersection;
mod iteration;
mod iteration_protocol;
mod jsdoc_checks;
mod jsdoc_types;
mod key;
mod late_indexes;
mod late_members;
mod links;
mod mapped;
mod mapper;
mod members;
mod merge;
mod meta_properties;
mod module_alias_like;
mod module_aliases;
mod module_augmentations;
mod module_exports;
mod module_specifiers;
mod module_wrappers;
mod name_errors;
mod name_qualified;
mod name_resolution;
mod name_scopes;
mod narrowable_references;
mod node_builder;
mod normalize;
mod object_context;
mod object_discriminants;
mod object_grammar;
mod object_literals;
mod object_members;
mod object_spread;
mod optional_chain;
mod other_operators;
mod owner;
mod parameter_checks;
mod private_access;
mod program;
mod program_init;
mod promises;
mod property_symbols;
mod query;
mod query_location;
mod query_names;
mod reduction;
mod references;
mod regular_expressions;
mod relater;
mod relater_compound;
mod relater_conditional;
mod relater_excess;
mod relater_mapped;
mod relater_properties;
mod relater_signatures;
mod relater_structure;
mod relater_tuples;
mod relater_variance;
mod relation_error_target;
mod relation_errors;
mod relation_helpers;
mod resolution;
mod return_inference;
mod signature_identity;
mod signature_jsdoc;
mod signature_parameters;
mod signatures;
mod source_alias_checks;
mod source_imports;
mod source_module_aliases;
mod source_modules;
mod source_signatures;
mod state;
mod string_mapping;
mod substitution;
mod symbols;
mod template;
mod template_expressions;
mod template_relation;
mod truthiness;
pub mod type_facts;
mod type_only_uses;
mod unary;
mod union_reduction;
mod unreachable;
mod value_references;
mod variable_errors;
mod variables;
mod variance;
mod widening;
pub use relater::RelationKind;
mod type_display;
mod type_parameters;
mod types;
mod union;
mod value_links;

#[cfg(any(test, feature = "storage-pilot"))]
mod census;
/// The P1 storage-families trace and census (`docs/S08-P1.md`).
#[cfg(any(test, feature = "storage-pilot"))]
pub mod storage_families;
/// A bounded constructor diagnostic; it does not expose checker-local identities.
#[cfg(feature = "storage-pilot")]
pub mod storage_pilot;

pub use flags::*;
pub use handles::{
    MemberSpec, NodeRef, RetainedNode, RetainedSignature, RetainedSymbol, RetainedType,
    RetainedTypeList, SignatureRef, SymbolRef, TypeNodeBuilder, TypeRef,
};
pub use host::{CheckerHost, ModuleSpecifierPath};
pub(crate) use ids::{
    AliasId, ConditionalRootId, IndexInfoId, InferenceId, MapperId, RelationFrameId, SignatureId,
    TypeId, TypePredicateId,
};
pub(crate) use init::Builtins;
pub use init::BUILTIN_TYPE_NAMES;
pub(crate) use key::CacheKey;
pub use links::{LinkKey, LinkStore};
pub use owner::{CheckerOwner, Operation};
#[cfg(test)]
use resolution::TypeResolution;
pub use resolution::TypeSystemPropertyName;
pub(crate) use resolution::{ResolutionStack, TypeSystemEntity};
#[cfg(any(test, feature = "storage-pilot"))]
pub(crate) use signatures::{IndexInfo, Signature};
pub(crate) use signatures::{SignatureStore, TypePredicate};
pub use state::CheckerOptions;
pub(crate) use state::CheckerState;
#[cfg(test)]
use type_display::{alias_symbol, alias_type_arguments};
pub use type_display::{
    to_node_builder_flags, DEFAULT_MAXIMUM_TRUNCATION_LENGTH, MAX_SERIALIZATION_LEVEL,
    NO_TRUNCATION_MAXIMUM_TRUNCATION_LENGTH,
};
pub use types::{element_flags, ElementFlags, TypeKind};
pub(crate) use types::{
    InterfaceData, IntrinsicData, LiteralData, LiteralValue, NumberKey, ObjectData, Payload,
    ReferenceData, SymbolList, TemplateLiteralData, TupleData, TupleElementInfo, TypeAlias,
    TypeList, TypeParameterData, TypeStore, UnionData, UnionOfUnionKey, UnionOrIntersectionMembers,
};
#[cfg(any(test, feature = "storage-pilot"))]
pub(crate) use types::{StructuredMembers, TypeRecord};
pub use union::UnionReduction;
pub(crate) use value_links::ValueSymbolLinks;

/// Checker-boundary failures. Arena identity, generation and bounds failures pass
/// through unchanged; the checker adds the failures only it can observe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Host(ts_vfs::Error),
    Arena(ts_arena::Error),
    Printer(ts_printer::Error),
    Pseudo(ts_pseudochecker::Error),
    /// The same thread asked for a second operation on an owner whose operation
    /// it already holds. Detected before waiting; ordinary contention waits.
    Reentry,
    /// A checker-local identity space is exhausted; IDs never wrap or recycle.
    IdExhausted,
    /// A required operation the port does not implement yet, by upstream name.
    /// A frozen case that reaches one is a named failure, never a pass.
    Unsupported(&'static str),
    /// A type of a kind this operation cannot take (upstream's `As*` panics).
    UnexpectedType {
        context: &'static str,
        kind: TypeKind,
    },
    /// A link upstream would have set before this read (a nil dereference there).
    MissingLink(&'static str),
}

impl From<ts_vfs::Error> for Error {
    fn from(error: ts_vfs::Error) -> Self {
        Self::Host(error)
    }
}

impl From<ts_arena::Error> for Error {
    fn from(error: ts_arena::Error) -> Self {
        match error {
            ts_arena::Error::Reentry => Self::Reentry,
            error => Self::Arena(error),
        }
    }
}

impl From<ts_printer::Error> for Error {
    fn from(error: ts_printer::Error) -> Self {
        Self::Printer(error)
    }
}

impl From<ts_pseudochecker::Error> for Error {
    fn from(error: ts_pseudochecker::Error) -> Self {
        Self::Pseudo(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host(error) => error.fmt(output),
            Self::Arena(error) => error.fmt(output),
            Self::Printer(error) => error.fmt(output),
            Self::Pseudo(error) => error.fmt(output),
            Self::Reentry => {
                output.write_str("the checker operation is already held by this thread")
            }
            Self::IdExhausted => output.write_str("checker-local identity space exhausted"),
            Self::Unsupported(name) => write!(output, "unsupported checker operation: {name}"),
            Self::UnexpectedType { context, kind } => {
                write!(output, "unexpected {kind:?} type in {context}")
            }
            Self::MissingLink(what) => write!(output, "missing {what}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod families_tests;
#[cfg(test)]
mod flag_tests;
mod lib_features;
#[cfg(test)]
mod tests;
mod unused_identifiers;
