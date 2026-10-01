//! The methods of `GeneratedIdentifierFlags` (`generatedidentifierflags.go`).
//! The flag values live in [`crate::generated_identifier_flags`]; this trait
//! gives the plain integer the pinned type's predicates.

use crate::generated_identifier_flags::{self as g, Flags};

/// Predicates of the pinned `GeneratedIdentifierFlags` type.
pub trait GeneratedIdentifierFlagsExt: Copy {
    fn kind(self) -> Flags;
    fn is_auto(self) -> bool;
    fn is_loop(self) -> bool;
    fn is_unique(self) -> bool;
    fn is_node(self) -> bool;
    fn is_reserved_in_nested_scopes(self) -> bool;
    fn is_optimistic(self) -> bool;
    fn is_file_level(self) -> bool;
    fn has_allow_name_substitution(self) -> bool;
}

impl GeneratedIdentifierFlagsExt for Flags {
    // port: tsc/internal/printer/generatedidentifierflags.go:GeneratedIdentifierFlags.Kind
    fn kind(self) -> Flags {
        self & g::KIND_MASK
    }

    // port: tsc/internal/printer/generatedidentifierflags.go:GeneratedIdentifierFlags.IsAuto
    fn is_auto(self) -> bool {
        self.kind() == g::AUTO
    }

    // port: tsc/internal/printer/generatedidentifierflags.go:GeneratedIdentifierFlags.IsLoop
    fn is_loop(self) -> bool {
        self.kind() == g::LOOP
    }

    // port: tsc/internal/printer/generatedidentifierflags.go:GeneratedIdentifierFlags.IsUnique
    fn is_unique(self) -> bool {
        self.kind() == g::UNIQUE
    }

    // port: tsc/internal/printer/generatedidentifierflags.go:GeneratedIdentifierFlags.IsNode
    fn is_node(self) -> bool {
        self.kind() == g::NODE
    }

    // port: tsc/internal/printer/generatedidentifierflags.go:GeneratedIdentifierFlags.IsReservedInNestedScopes
    fn is_reserved_in_nested_scopes(self) -> bool {
        self & g::RESERVED_IN_NESTED_SCOPES != 0
    }

    // port: tsc/internal/printer/generatedidentifierflags.go:GeneratedIdentifierFlags.IsOptimistic
    fn is_optimistic(self) -> bool {
        self & g::OPTIMISTIC != 0
    }

    // port: tsc/internal/printer/generatedidentifierflags.go:GeneratedIdentifierFlags.IsFileLevel
    fn is_file_level(self) -> bool {
        self & g::FILE_LEVEL != 0
    }

    // port: tsc/internal/printer/generatedidentifierflags.go:GeneratedIdentifierFlags.HasAllowNameSubstitution
    fn has_allow_name_substitution(self) -> bool {
        self & g::ALLOW_NAME_SUBSTITUTION != 0
    }
}
