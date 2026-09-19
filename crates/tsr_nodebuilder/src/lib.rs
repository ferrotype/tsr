//! Node-builder flags and the symbol-tracker contract.
//!
//! Upstream keeps these in `tsc/internal/nodebuilder/types.go`, a package with
//! no implementation, so that the declarations transformer and the printer's
//! emit resolver can name them without importing the checker. This crate keeps
//! that boundary: the concrete node builder lives in `ts_checker`, the Phase 3
//! declarations transformer depends on this crate only.
//!
//! Flag values are the pinned source's bit positions. `Flags` must stay aligned
//! with the checker's `TypeFormatFlags`; the checker converts one into the other
//! by masking, not by translation.

use ts_ast::{NodeId, SymbolFlags, SymbolId};

/// Open 32-bit node-builder flags (`nodebuilder.Flags`).
pub type Flags = u32;

pub mod flags {
    use super::Flags;

    pub const NONE: Flags = 0;
    // Options
    pub const NO_TRUNCATION: Flags = 1 << 0;
    pub const WRITE_ARRAY_AS_GENERIC_TYPE: Flags = 1 << 1;
    pub const GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS: Flags = 1 << 2;
    pub const USE_STRUCTURAL_FALLBACK: Flags = 1 << 3;
    pub const FORBID_INDEXED_ACCESS_SYMBOL_REFERENCES: Flags = 1 << 4;
    pub const WRITE_TYPE_ARGUMENTS_OF_SIGNATURE: Flags = 1 << 5;
    pub const USE_FULLY_QUALIFIED_TYPE: Flags = 1 << 6;
    pub const USE_ONLY_EXTERNAL_ALIASING: Flags = 1 << 7;
    pub const SUPPRESS_ANY_RETURN_TYPE: Flags = 1 << 8;
    pub const WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME: Flags = 1 << 9;
    pub const MULTILINE_OBJECT_LITERALS: Flags = 1 << 10;
    pub const WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL: Flags = 1 << 11;
    pub const USE_TYPE_OF_FUNCTION: Flags = 1 << 12;
    pub const OMIT_PARAMETER_MODIFIERS: Flags = 1 << 13;
    pub const USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE: Flags = 1 << 14;
    pub const USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE: Flags = 1 << 28;
    pub const NO_TYPE_REDUCTION: Flags = 1 << 29;
    pub const USE_INSTANTIATION_EXPRESSIONS: Flags = 1 << 30;
    pub const OMIT_THIS_PARAMETER: Flags = 1 << 25;
    pub const WRITE_CALL_STYLE_SIGNATURE: Flags = 1 << 27;
    // Error handling
    pub const ALLOW_THIS_IN_OBJECT_LITERAL: Flags = 1 << 15;
    pub const ALLOW_QUALIFIED_NAME_IN_PLACE_OF_IDENTIFIER: Flags = 1 << 16;
    pub const ALLOW_ANONYMOUS_IDENTIFIER: Flags = 1 << 17;
    pub const ALLOW_EMPTY_UNION_OR_INTERSECTION: Flags = 1 << 18;
    pub const ALLOW_EMPTY_TUPLE: Flags = 1 << 19;
    pub const ALLOW_UNIQUE_ES_SYMBOL_TYPE: Flags = 1 << 20;
    pub const ALLOW_EMPTY_INDEX_INFO_TYPE: Flags = 1 << 21;
    // Errors (cont.)
    pub const ALLOW_NODE_MODULES_RELATIVE_PATHS: Flags = 1 << 26;
    pub const IGNORE_ERRORS: Flags = ALLOW_THIS_IN_OBJECT_LITERAL
        | ALLOW_QUALIFIED_NAME_IN_PLACE_OF_IDENTIFIER
        | ALLOW_ANONYMOUS_IDENTIFIER
        | ALLOW_EMPTY_UNION_OR_INTERSECTION
        | ALLOW_EMPTY_TUPLE
        | ALLOW_EMPTY_INDEX_INFO_TYPE
        | ALLOW_NODE_MODULES_RELATIVE_PATHS;
    // State
    pub const IN_OBJECT_TYPE_LITERAL: Flags = 1 << 22;
    pub const IN_TYPE_ALIAS: Flags = 1 << 23;
    pub const IN_INITIAL_ENTITY_NAME: Flags = 1 << 24;
}

/// Internal node-builder flags (`nodebuilder.InternalFlags`, a Go `int32`).
pub type InternalFlags = i32;

pub mod internal_flags {
    use super::InternalFlags;

    pub const NONE: InternalFlags = 0;
    pub const WRITE_COMPUTED_PROPS: InternalFlags = 1 << 0;
    pub const NO_SYNTACTIC_PRINTER: InternalFlags = 1 << 1;
    pub const DO_NOT_INCLUDE_SYMBOL_CHAIN: InternalFlags = 1 << 2;
    pub const ALLOW_UNRESOLVED_NAMES: InternalFlags = 1 << 3;
}

/// The node builder reports accessibility and serialization events to its
/// caller through this contract (`nodebuilder.SymbolTracker`). Every method is
/// required upstream; implementations that ignore an event do so explicitly.
///
/// Symbols and nodes are identities, not retained handles: a tracker that keeps
/// one beyond the call must retain the owning checker or file itself. Absent
/// nodes are `None` where upstream passes a nil `*ast.Node`. Names cross this
/// boundary as bytes because upstream strings may carry arbitrary bytes.
pub trait SymbolTracker {
    fn track_symbol(
        &mut self,
        symbol: SymbolId,
        enclosing_declaration: Option<NodeId>,
        meaning: SymbolFlags,
    ) -> bool;
    fn report_inaccessible_this_error(&mut self);
    fn report_private_in_base_of_class_expression(&mut self, property_name: &[u8]);
    fn report_inaccessible_unique_symbol_error(&mut self);
    fn report_cyclic_structure_error(&mut self);
    fn report_likely_unsafe_import_required_error(&mut self, specifier: &[u8], symbol_name: &[u8]);
    fn report_truncation_error(&mut self);
    fn report_nonlocal_augmentation(
        &mut self,
        containing_file: NodeId,
        parent_symbol: SymbolId,
        augmenting_symbol: SymbolId,
    );
    fn report_non_serializable_property(&mut self, property_name: &[u8]);
    fn report_inference_fallback(&mut self, node: NodeId);
    fn push_error_fallback_node(&mut self, node: Option<NodeId>);
    fn pop_error_fallback_node(&mut self);
}

#[cfg(test)]
mod tests;
