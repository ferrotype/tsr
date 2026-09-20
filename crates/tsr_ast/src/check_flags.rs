//! `ast.CheckFlags` (`tsc/internal/ast/checkflags.go`): flags only the checker
//! sets, on the transient symbols it creates.

use crate::CheckFlags;

pub const NONE: CheckFlags = 0;
/// Instantiated symbol.
pub const INSTANTIATED: CheckFlags = 1 << 0;
/// Property in union or intersection type.
pub const SYNTHETIC_PROPERTY: CheckFlags = 1 << 1;
/// Method in union or intersection type.
pub const SYNTHETIC_METHOD: CheckFlags = 1 << 2;
/// Readonly transient symbol.
pub const READONLY: CheckFlags = 1 << 3;
/// Synthetic property present in some but not all constituents.
pub const READ_PARTIAL: CheckFlags = 1 << 4;
/// Synthetic property present in some but only satisfied by an index signature in others.
pub const WRITE_PARTIAL: CheckFlags = 1 << 5;
/// Synthetic property with non-uniform type in constituents.
pub const HAS_NON_UNIFORM_TYPE: CheckFlags = 1 << 6;
/// Synthetic property with at least one literal type in constituents.
pub const HAS_LITERAL_TYPE: CheckFlags = 1 << 7;
pub const CONTAINS_PUBLIC: CheckFlags = 1 << 8;
pub const CONTAINS_PROTECTED: CheckFlags = 1 << 9;
pub const CONTAINS_PRIVATE: CheckFlags = 1 << 10;
pub const CONTAINS_WRITE_PUBLIC: CheckFlags = 1 << 11;
pub const CONTAINS_WRITE_PROTECTED: CheckFlags = 1 << 12;
pub const CONTAINS_WRITE_PRIVATE: CheckFlags = 1 << 13;
pub const CONTAINS_STATIC: CheckFlags = 1 << 14;
/// Late-bound symbol for a computed property with a dynamic name.
pub const LATE: CheckFlags = 1 << 15;
/// Property of reverse-inferred homomorphic mapped type.
pub const REVERSE_MAPPED: CheckFlags = 1 << 16;
pub const OPTIONAL_PARAMETER: CheckFlags = 1 << 17;
pub const REST_PARAMETER: CheckFlags = 1 << 18;
/// The type of this symbol is computed lazily; fetch it with `getTypeOfSymbolWithDeferredType`.
pub const DEFERRED_TYPE: CheckFlags = 1 << 19;
/// Synthetic property with at least one never type in constituents.
pub const HAS_NEVER_TYPE: CheckFlags = 1 << 20;
/// Property of mapped type.
pub const MAPPED: CheckFlags = 1 << 21;
/// Strip optionality in mapped property.
pub const STRIP_OPTIONAL: CheckFlags = 1 << 22;
/// Unresolved type alias symbol.
pub const UNRESOLVED: CheckFlags = 1 << 23;
pub const IS_DISCRIMINANT_COMPUTED: CheckFlags = 1 << 24;
pub const IS_DISCRIMINANT: CheckFlags = 1 << 25;
/// Synthetic property created from an index signature.
pub const INDEX_SYMBOL: CheckFlags = 1 << 26;
pub const SYNTHETIC: CheckFlags = SYNTHETIC_PROPERTY | SYNTHETIC_METHOD;
pub const NON_UNIFORM_AND_LITERAL: CheckFlags = HAS_NON_UNIFORM_TYPE | HAS_LITERAL_TYPE;
pub const PARTIAL: CheckFlags = READ_PARTIAL | WRITE_PARTIAL;
