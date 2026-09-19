//! Emit flags (`tsc/internal/printer/emitflags.go`): per-node instructions the
//! node builder and transforms leave for the printer. Values are the pinned
//! source's bit positions and are checked against the compiled Go package by
//! the printer observation fixture.

pub type EmitFlags = u32;

pub const NONE: EmitFlags = 0;
/// The contents of this node should be emitted on a single line.
pub const SINGLE_LINE: EmitFlags = 1 << 0;
/// The contents of this node should be emitted on multiple lines.
pub const MULTI_LINE: EmitFlags = 1 << 1;
pub const NO_LEADING_SOURCE_MAP: EmitFlags = 1 << 2;
pub const NO_TRAILING_SOURCE_MAP: EmitFlags = 1 << 3;
pub const NO_NESTED_SOURCE_MAPS: EmitFlags = 1 << 4;
pub const NO_TOKEN_LEADING_SOURCE_MAPS: EmitFlags = 1 << 5;
pub const NO_TOKEN_TRAILING_SOURCE_MAPS: EmitFlags = 1 << 6;
pub const NO_LEADING_COMMENTS: EmitFlags = 1 << 7;
pub const NO_TRAILING_COMMENTS: EmitFlags = 1 << 8;
pub const NO_NESTED_COMMENTS: EmitFlags = 1 << 9;
/// The identifier refers to an unscoped emit helper.
pub const HELPER_NAME: EmitFlags = 1 << 10;
pub const EXPORT_NAME: EmitFlags = 1 << 11;
pub const LOCAL_NAME: EmitFlags = 1 << 12;
/// Adds an explicit extra indentation level when printing.
pub const INDENTED: EmitFlags = 1 << 13;
pub const NO_INDENTATION: EmitFlags = 1 << 14;
pub const REUSE_TEMP_VARIABLE_SCOPE: EmitFlags = 1 << 15;
pub const CUSTOM_PROLOGUE: EmitFlags = 1 << 16;
/// Synthesized literal text is written with ASCII escaping substitutions.
pub const NO_ASCII_ESCAPING: EmitFlags = 1 << 17;
pub const EXTERNAL_HELPERS: EmitFlags = 1 << 18;
/// Start this node on a new line.
pub const START_ON_NEW_LINE: EmitFlags = 1 << 19;
pub const INDIRECT_CALL: EmitFlags = 1 << 20;
pub const ASYNC_FUNCTION_BODY: EmitFlags = 1 << 21;
pub const NO_LEXICAL_ARGUMENTS: EmitFlags = 1 << 22;
pub const TRANSFORM_PRIVATE_STATIC_ELEMENTS: EmitFlags = 1 << 23;
pub const NO_LEXICAL_THIS: EmitFlags = 1 << 24;

pub const NO_SOURCE_MAP: EmitFlags = NO_LEADING_SOURCE_MAP | NO_TRAILING_SOURCE_MAP;
pub const NO_TOKEN_SOURCE_MAPS: EmitFlags =
    NO_TOKEN_LEADING_SOURCE_MAPS | NO_TOKEN_TRAILING_SOURCE_MAPS;
pub const NO_COMMENTS: EmitFlags = NO_LEADING_COMMENTS | NO_TRAILING_COMMENTS;
