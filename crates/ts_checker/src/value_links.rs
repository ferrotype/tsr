//! `ValueSymbolLinks` (`tsc/internal/checker/types.go`): the checker's per-symbol
//! value information. Upstream stores these through `symbolArenaLinkStore`, a
//! paged table of pointers into an arena; here the record sits directly in the
//! page. The type mapper field arrives with instantiation (P3).

use crate::TypeId;
use ts_arena::SymbolId;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ValueSymbolLinks {
    /// Type of value symbol.
    pub resolved_type: Option<TypeId>,
    pub write_type: Option<TypeId>,
    pub target: Option<SymbolId>,
    pub mapper: Option<crate::MapperId>,
    pub name_type: Option<TypeId>,
    /// Mapped type for mapped type property, containing union or intersection
    /// type for synthetic property.
    pub containing_type: Option<TypeId>,
    pub function_or_constructor_checked: bool,
}
