//! Pinned node bits, including context-specific reuse of the same bit.
// port: tsc/internal/ast/nodeflags.go

pub type NodeFlags = u32;

pub const NONE: NodeFlags = 0;
pub const LET: NodeFlags = 1 << 0;
pub const CONST: NodeFlags = 1 << 1;
pub const USING: NodeFlags = 1 << 2;
pub const REPARSED: NodeFlags = 1 << 3;
pub const SYNTHESIZED: NodeFlags = 1 << 4;
pub const OPTIONAL_CHAIN: NodeFlags = 1 << 5;
pub const EXPORT_CONTEXT: NodeFlags = 1 << 6;
pub const CONTAINS_THIS: NodeFlags = 1 << 7;
pub const HAS_IMPLICIT_RETURN: NodeFlags = 1 << 8;
pub const HAS_EXPLICIT_RETURN: NodeFlags = 1 << 9;
pub const DISALLOW_IN_CONTEXT: NodeFlags = 1 << 10;
pub const YIELD_CONTEXT: NodeFlags = 1 << 11;
pub const DECORATOR_CONTEXT: NodeFlags = 1 << 12;
pub const AWAIT_CONTEXT: NodeFlags = 1 << 13;
pub const DISALLOW_CONDITIONAL_TYPES_CONTEXT: NodeFlags = 1 << 14;
pub const THIS_NODE_HAS_ERROR: NodeFlags = 1 << 15;
pub const JAVA_SCRIPT_FILE: NodeFlags = 1 << 16;
pub const THIS_NODE_OR_ANY_SUB_NODES_HAS_ERROR: NodeFlags = 1 << 17;
pub const HAS_ASYNC_FUNCTIONS: NodeFlags = 1 << 18;
pub const POSSIBLY_CONTAINS_DYNAMIC_IMPORT: NodeFlags = 1 << 19;
pub const POSSIBLY_CONTAINS_IMPORT_META: NodeFlags = 1 << 20;
pub const HAS_JS_DOC: NodeFlags = 1 << 21;
pub const JS_DOC: NodeFlags = 1 << 22;
pub const AMBIENT: NodeFlags = 1 << 23;
pub const IN_WITH_STATEMENT: NodeFlags = 1 << 24;
pub const JSON_FILE: NodeFlags = 1 << 25;
pub const POSSIBLY_CONTAINS_DEPRECATED_TAG: NodeFlags = 1 << 26;
pub const UNREACHABLE: NodeFlags = 1 << 27;
pub const REPARSER_TRANSFORMED_LITERAL: NodeFlags = 1 << 28;

pub const BLOCK_SCOPED: NodeFlags = LET | CONST | USING;
pub const CONSTANT: NodeFlags = CONST | USING;
pub const AWAIT_USING: NodeFlags = CONST | USING;
pub const REACHABILITY_CHECK_FLAGS: NodeFlags = HAS_IMPLICIT_RETURN | HAS_EXPLICIT_RETURN;
pub const REACHABILITY_AND_EMIT_FLAGS: NodeFlags = REACHABILITY_CHECK_FLAGS | HAS_ASYNC_FUNCTIONS;
pub const CONTEXT_FLAGS: NodeFlags = DISALLOW_IN_CONTEXT
    | DISALLOW_CONDITIONAL_TYPES_CONTEXT
    | YIELD_CONTEXT
    | DECORATOR_CONTEXT
    | AWAIT_CONTEXT
    | JAVA_SCRIPT_FILE
    | IN_WITH_STATEMENT
    | AMBIENT;
pub const TYPE_EXCLUDES_FLAGS: NodeFlags = YIELD_CONTEXT | AWAIT_CONTEXT;
pub const PERMANENTLY_SET_INCREMENTAL_FLAGS: NodeFlags =
    POSSIBLY_CONTAINS_DYNAMIC_IMPORT | POSSIBLY_CONTAINS_IMPORT_META;
pub const IDENTIFIER_HAS_EXTENDED_UNICODE_ESCAPE: NodeFlags = CONTAINS_THIS;
pub const IDENTIFIER_IS_IN_JS_DOC_NAMESPACE: NodeFlags = HAS_ASYNC_FUNCTIONS;
pub const NESTED_NAMESPACE: NodeFlags = OPTIONAL_CHAIN;
