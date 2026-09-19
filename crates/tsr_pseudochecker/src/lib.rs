//! Syntactic type skeletons from pinned `internal/pseudochecker`. A skeleton
//! defers semantic resolution and use-site checks to its consumer. It never
//! silently substitutes an inferred type for a failed syntactic operation.
mod lookup;
mod types;
pub use lookup::{could_already_refer_to_undefined_type, is_in_const_context};
pub use types::*;

use ts_arena::NodeId;
use ts_ast::{AstView, NodeKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    MissingLink(&'static str),
    InvalidKind {
        node: NodeId,
        kind: NodeKind,
        operation: &'static str,
    },
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

/// Reads retain their AST owner through the host. Accessor pairing deliberately
/// uses raw declaration symbols, preserving the pinned pseudochecker's boundary
/// around semantic symbol merging and late-bound names.
pub trait Host {
    type Error: From<Error> + From<ts_arena::Error>;
    fn ast(&self, node: NodeId) -> Result<AstView<'_>, Self::Error>;
    fn raw_symbol_declarations(&self, node: NodeId) -> Result<Option<Vec<NodeId>>, Self::Error>;
}

// port: tsc/internal/pseudochecker/checker.go:NewPseudoChecker
#[derive(Clone, Copy, Debug)]
pub struct PseudoChecker {
    pub strict_null_checks: bool,
    /// Stored but not consulted by the pinned native implementation.
    pub exact_optional_property_types: bool,
}
impl PseudoChecker {
    pub const fn new(strict_null_checks: bool, exact_optional_property_types: bool) -> Self {
        Self {
            strict_null_checks,
            exact_optional_property_types,
        }
    }
}
