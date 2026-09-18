//! Rule types (`format/rule.go`). A rule looks at a left and a right token in a
//! context and says what the whitespace between them should be.

use crate::{context::FormattingContext, Error};
use ts_ast::SyntaxKind;

/// A filter narrowing where a rule applies. Upstream's predicates read lazily
/// cached facts through the context pointer, so they take it mutably here.
pub(crate) type ContextPredicate = fn(&mut FormattingContext<'_, '_, '_>) -> Result<bool, Error>;

pub(crate) mod action {
    pub(crate) const NONE: u32 = 0;
    pub(crate) const STOP_PROCESSING_SPACE_ACTIONS: u32 = 1 << 0;
    pub(crate) const STOP_PROCESSING_TOKEN_ACTIONS: u32 = 1 << 1;
    pub(crate) const INSERT_SPACE: u32 = 1 << 2;
    pub(crate) const INSERT_NEW_LINE: u32 = 1 << 3;
    pub(crate) const DELETE_SPACE: u32 = 1 << 4;
    pub(crate) const DELETE_TOKEN: u32 = 1 << 5;
    pub(crate) const INSERT_TRAILING_SEMICOLON: u32 = 1 << 6;
    pub(crate) const STOP_ACTION: u32 =
        STOP_PROCESSING_SPACE_ACTIONS | STOP_PROCESSING_TOKEN_ACTIONS;
    pub(crate) const MODIFY_SPACE_ACTION: u32 = INSERT_SPACE | INSERT_NEW_LINE | DELETE_SPACE;
    pub(crate) const MODIFY_TOKEN_ACTION: u32 = DELETE_TOKEN | INSERT_TRAILING_SEMICOLON;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuleFlags {
    None,
    CanDeleteNewLines,
}

pub(crate) struct Rule {
    pub(crate) debug_name: &'static str,
    pub(crate) context: &'static [ContextPredicate],
    pub(crate) action: u32,
    pub(crate) flags: RuleFlags,
}

/// The tokens one side of a rule accepts. A range that names its tokens one by
/// one is specific; the broad ranges (any token, keywords, operators) are not,
/// and the rules map orders specific rules first.
#[derive(Clone)]
pub(crate) struct TokenRange {
    pub(crate) tokens: Vec<SyntaxKind>,
    pub(crate) is_specific: bool,
}

pub(crate) struct RuleSpec {
    pub(crate) left: TokenRange,
    pub(crate) right: TokenRange,
    pub(crate) rule: Rule,
}
