//! Scanner metadata from ast/tokenflags.go and ast/ast.go at the source pin.

use ts_core::TextRange;

pub type TokenFlags = i32;

pub mod token_flags {
    use super::TokenFlags;
    pub const NONE: TokenFlags = 0;
    pub const PRECEDING_LINE_BREAK: TokenFlags = 1 << 0;
    pub const PRECEDING_JSDOC_COMMENT: TokenFlags = 1 << 1;
    pub const UNTERMINATED: TokenFlags = 1 << 2;
    pub const EXTENDED_UNICODE_ESCAPE: TokenFlags = 1 << 3;
    pub const SCIENTIFIC: TokenFlags = 1 << 4;
    pub const OCTAL: TokenFlags = 1 << 5;
    pub const HEX_SPECIFIER: TokenFlags = 1 << 6;
    pub const BINARY_SPECIFIER: TokenFlags = 1 << 7;
    pub const OCTAL_SPECIFIER: TokenFlags = 1 << 8;
    pub const CONTAINS_SEPARATOR: TokenFlags = 1 << 9;
    pub const UNICODE_ESCAPE: TokenFlags = 1 << 10;
    pub const CONTAINS_INVALID_ESCAPE: TokenFlags = 1 << 11;
    pub const HEX_ESCAPE: TokenFlags = 1 << 12;
    pub const CONTAINS_LEADING_ZERO: TokenFlags = 1 << 13;
    pub const CONTAINS_INVALID_SEPARATOR: TokenFlags = 1 << 14;
    pub const PRECEDING_JSDOC_LEADING_ASTERISKS: TokenFlags = 1 << 15;
    pub const SINGLE_QUOTE: TokenFlags = 1 << 16;
    pub const PRECEDING_JSDOC_WITH_DEPRECATED: TokenFlags = 1 << 17;
    pub const PRECEDING_JSDOC_WITH_SEE_OR_LINK: TokenFlags = 1 << 18;
    pub const BINARY_OR_OCTAL_SPECIFIER: TokenFlags = BINARY_SPECIFIER | OCTAL_SPECIFIER;
    pub const WITH_SPECIFIER: TokenFlags = HEX_SPECIFIER | BINARY_OR_OCTAL_SPECIFIER;
    pub const STRING_LITERAL_FLAGS: TokenFlags = UNTERMINATED
        | HEX_ESCAPE
        | UNICODE_ESCAPE
        | EXTENDED_UNICODE_ESCAPE
        | CONTAINS_INVALID_ESCAPE
        | SINGLE_QUOTE;
    pub const NUMERIC_LITERAL_FLAGS: TokenFlags = SCIENTIFIC
        | OCTAL
        | CONTAINS_LEADING_ZERO
        | WITH_SPECIFIER
        | CONTAINS_SEPARATOR
        | CONTAINS_INVALID_SEPARATOR;
    pub const TEMPLATE_LITERAL_LIKE_FLAGS: TokenFlags = UNTERMINATED
        | HEX_ESCAPE
        | UNICODE_ESCAPE
        | EXTENDED_UNICODE_ESCAPE
        | CONTAINS_INVALID_ESCAPE;
    pub const REGULAR_EXPRESSION_LITERAL_FLAGS: TokenFlags = UNTERMINATED;
    pub const IS_INVALID: TokenFlags =
        OCTAL | CONTAINS_LEADING_ZERO | CONTAINS_INVALID_SEPARATOR | CONTAINS_INVALID_ESCAPE;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
pub enum CommentDirectiveKind {
    #[default]
    Unknown = 0,
    ExpectError = 1,
    Ignore = 2,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommentDirective {
    pub loc: TextRange,
    pub kind: CommentDirectiveKind,
}
