//! Rule types (`format/rule.go`). A rule looks at a left and a right token in a
//! context and says what the whitespace between them should be.

use crate::{
    context::FormattingContext,
    settings::{FormatCodeSettings, SemicolonPreference},
    Error,
};
use ts_ast::SyntaxKind;
use ts_core::Tristate;

type ContextFn = fn(&mut FormattingContext<'_, '_, '_>) -> Result<bool, Error>;
type OptionSelector = fn(&FormatCodeSettings) -> Tristate;

/// A filter narrowing where a rule applies. Upstream's predicates are closures,
/// some of them built from an option selector; the variants here are those
/// builders (`isOptionEnabled` and the rest) and the plain functions.
#[derive(Clone, Copy)]
pub(crate) enum Predicate {
    Fn(ContextFn),
    OptionEnabled(OptionSelector),
    OptionDisabled(OptionSelector),
    OptionDisabledOrUndefined(OptionSelector),
    OptionDisabledOrUndefinedOrTokensOnSameLine(OptionSelector),
    OptionEnabledOrUndefined(OptionSelector),
    SemicolonEquals(SemicolonPreference),
}

impl Predicate {
    /// Upstream reads the lazily cached line facts through the context
    /// pointer, so the context is taken mutably.
    pub(crate) fn test(self, context: &mut FormattingContext<'_, '_, '_>) -> Result<bool, Error> {
        Ok(match self {
            Self::Fn(predicate) => predicate(context)?,
            // port: tsc/internal/format/rulecontext.go:isOptionEnabled
            Self::OptionEnabled(option) => option(&context.options).is_true(),
            // port: tsc/internal/format/rulecontext.go:isOptionDisabled
            Self::OptionDisabled(option) => option(&context.options).is_false(),
            // port: tsc/internal/format/rulecontext.go:isOptionDisabledOrUndefined
            Self::OptionDisabledOrUndefined(option) => {
                option(&context.options).is_false_or_unknown()
            }
            // port: tsc/internal/format/rulecontext.go:isOptionDisabledOrUndefinedOrTokensOnSameLine
            Self::OptionDisabledOrUndefinedOrTokensOnSameLine(option) => {
                option(&context.options).is_false_or_unknown()
                    || context.tokens_are_on_same_line()?
            }
            // port: tsc/internal/format/rulecontext.go:isOptionEnabledOrUndefined
            Self::OptionEnabledOrUndefined(option) => option(&context.options).is_true_or_unknown(),
            // port: tsc/internal/format/rulecontext.go:optionEquals
            Self::SemicolonEquals(value) => context.options.semicolons == value,
        })
    }
}

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
    pub(crate) context: Vec<Predicate>,
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

impl From<SyntaxKind> for TokenRange {
    // port: tsc/internal/format/rule.go:toTokenRange
    fn from(token: SyntaxKind) -> Self {
        Self {
            tokens: vec![token],
            is_specific: true,
        }
    }
}

impl<const N: usize> From<[SyntaxKind; N]> for TokenRange {
    fn from(tokens: [SyntaxKind; N]) -> Self {
        Self {
            tokens: tokens.to_vec(),
            is_specific: true,
        }
    }
}

/// A rule takes two tokens and a context in which to look at them, and declares
/// the whitespace expected between them.
// port: tsc/internal/format/rule.go:rule
pub(crate) fn rule(
    debug_name: &'static str,
    left: impl Into<TokenRange>,
    right: impl Into<TokenRange>,
    context: Vec<Predicate>,
    action: u32,
    flags: RuleFlags,
) -> RuleSpec {
    RuleSpec {
        left: left.into(),
        right: right.into(),
        rule: Rule {
            debug_name,
            context,
            action,
            flags,
        },
    }
}
