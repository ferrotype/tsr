//! Flags and enumerations of `tsc/internal/checker/types.go`, transcribed at the
//! pinned commit with the source's names and bit positions. Values are checked
//! against the pinned Go package by `flag_tests.rs`, which reads
//! `data/s08/checker-flag-observations.json`. Bit positions are not free to
//! change: `TypeFlags` order is the `CompareTypes` sort order for union
//! constituents, and several `ObjectFlags` bits are reused per type kind.

/// `TypeFlags` (`types.go`).
pub type TypeFlags = u32;

pub mod type_flags {
    use super::TypeFlags;

    pub const NONE: TypeFlags = 0;
    pub const ANY: TypeFlags = 1 << 0;
    pub const UNKNOWN: TypeFlags = 1 << 1;
    pub const UNDEFINED: TypeFlags = 1 << 2;
    pub const NULL: TypeFlags = 1 << 3;
    pub const VOID: TypeFlags = 1 << 4;
    pub const STRING: TypeFlags = 1 << 5;
    pub const NUMBER: TypeFlags = 1 << 6;
    pub const BIG_INT: TypeFlags = 1 << 7;
    pub const BOOLEAN: TypeFlags = 1 << 8;
    pub const ES_SYMBOL: TypeFlags = 1 << 9;
    pub const STRING_LITERAL: TypeFlags = 1 << 10;
    pub const NUMBER_LITERAL: TypeFlags = 1 << 11;
    pub const BIG_INT_LITERAL: TypeFlags = 1 << 12;
    pub const BOOLEAN_LITERAL: TypeFlags = 1 << 13;
    pub const UNIQUE_ES_SYMBOL: TypeFlags = 1 << 14;
    pub const ENUM_LITERAL: TypeFlags = 1 << 15;
    pub const ENUM: TypeFlags = 1 << 16;
    pub const NON_PRIMITIVE: TypeFlags = 1 << 17;
    pub const NEVER: TypeFlags = 1 << 18;
    pub const TYPE_PARAMETER: TypeFlags = 1 << 19;
    pub const OBJECT: TypeFlags = 1 << 20;
    pub const INDEX: TypeFlags = 1 << 21;
    pub const TEMPLATE_LITERAL: TypeFlags = 1 << 22;
    pub const STRING_MAPPING: TypeFlags = 1 << 23;
    pub const SUBSTITUTION: TypeFlags = 1 << 24;
    pub const INDEXED_ACCESS: TypeFlags = 1 << 25;
    pub const CONDITIONAL: TypeFlags = 1 << 26;
    pub const UNION: TypeFlags = 1 << 27;
    pub const INTERSECTION: TypeFlags = 1 << 28;
    pub const RESERVED1: TypeFlags = 1 << 29;
    pub const RESERVED2: TypeFlags = 1 << 30;
    pub const RESERVED3: TypeFlags = 1 << 31;
    pub const ANY_OR_UNKNOWN: TypeFlags = ANY | UNKNOWN;
    pub const NULLABLE: TypeFlags = UNDEFINED | NULL;
    pub const LITERAL: TypeFlags =
        STRING_LITERAL | NUMBER_LITERAL | BIG_INT_LITERAL | BOOLEAN_LITERAL;
    pub const UNIT: TypeFlags = ENUM | LITERAL | UNIQUE_ES_SYMBOL | NULLABLE;
    pub const FRESHABLE: TypeFlags = ENUM | LITERAL;
    pub const STRING_OR_NUMBER_LITERAL: TypeFlags = STRING_LITERAL | NUMBER_LITERAL;
    pub const STRING_OR_NUMBER_LITERAL_OR_UNIQUE: TypeFlags =
        STRING_LITERAL | NUMBER_LITERAL | UNIQUE_ES_SYMBOL;
    pub const DEFINITELY_FALSY: TypeFlags = STRING_LITERAL
        | NUMBER_LITERAL
        | BIG_INT_LITERAL
        | BOOLEAN_LITERAL
        | VOID
        | UNDEFINED
        | NULL;
    pub const POSSIBLY_FALSY: TypeFlags = DEFINITELY_FALSY | STRING | NUMBER | BIG_INT | BOOLEAN;
    pub const INTRINSIC: TypeFlags = ANY
        | UNKNOWN
        | STRING
        | NUMBER
        | BIG_INT
        | ES_SYMBOL
        | VOID
        | UNDEFINED
        | NULL
        | NEVER
        | NON_PRIMITIVE;
    pub const STRING_LIKE: TypeFlags = STRING | STRING_LITERAL | TEMPLATE_LITERAL | STRING_MAPPING;
    pub const NUMBER_LIKE: TypeFlags = NUMBER | NUMBER_LITERAL | ENUM;
    pub const BIG_INT_LIKE: TypeFlags = BIG_INT | BIG_INT_LITERAL;
    pub const BOOLEAN_LIKE: TypeFlags = BOOLEAN | BOOLEAN_LITERAL;
    pub const ENUM_LIKE: TypeFlags = ENUM | ENUM_LITERAL;
    pub const ES_SYMBOL_LIKE: TypeFlags = ES_SYMBOL | UNIQUE_ES_SYMBOL;
    pub const VOID_LIKE: TypeFlags = VOID | UNDEFINED;
    pub const PRIMITIVE: TypeFlags = STRING_LIKE
        | NUMBER_LIKE
        | BIG_INT_LIKE
        | BOOLEAN_LIKE
        | ENUM_LIKE
        | ES_SYMBOL_LIKE
        | VOID_LIKE
        | NULL;
    pub const DEFINITELY_NON_NULLABLE: TypeFlags = STRING_LIKE
        | NUMBER_LIKE
        | BIG_INT_LIKE
        | BOOLEAN_LIKE
        | ENUM_LIKE
        | ES_SYMBOL_LIKE
        | OBJECT
        | NON_PRIMITIVE;
    pub const DISJOINT_DOMAINS: TypeFlags = NON_PRIMITIVE
        | STRING_LIKE
        | NUMBER_LIKE
        | BIG_INT_LIKE
        | BOOLEAN_LIKE
        | ES_SYMBOL_LIKE
        | VOID_LIKE
        | NULL;
    pub const UNION_OR_INTERSECTION: TypeFlags = UNION | INTERSECTION;
    pub const STRUCTURED_TYPE: TypeFlags = OBJECT | UNION | INTERSECTION;
    pub const TYPE_VARIABLE: TypeFlags = TYPE_PARAMETER | INDEXED_ACCESS;
    pub const INSTANTIABLE_NON_PRIMITIVE: TypeFlags = TYPE_VARIABLE | CONDITIONAL | SUBSTITUTION;
    pub const INSTANTIABLE_PRIMITIVE: TypeFlags = INDEX | TEMPLATE_LITERAL | STRING_MAPPING;
    pub const INSTANTIABLE: TypeFlags = INSTANTIABLE_NON_PRIMITIVE | INSTANTIABLE_PRIMITIVE;
    pub const STRUCTURED_OR_INSTANTIABLE: TypeFlags = STRUCTURED_TYPE | INSTANTIABLE;
    pub const OBJECT_FLAGS_TYPE: TypeFlags = ANY | NULLABLE | NEVER | OBJECT | UNION | INTERSECTION;
    pub const SIMPLIFIABLE: TypeFlags = INDEXED_ACCESS | CONDITIONAL | INDEX;
    pub const SINGLETON: TypeFlags = ANY
        | UNKNOWN
        | STRING
        | NUMBER
        | BOOLEAN
        | BIG_INT
        | ES_SYMBOL
        | VOID
        | UNDEFINED
        | NULL
        | NEVER
        | NON_PRIMITIVE;
    pub const NARROWABLE: TypeFlags = ANY
        | UNKNOWN
        | STRUCTURED_OR_INSTANTIABLE
        | STRING_LIKE
        | NUMBER_LIKE
        | BIG_INT_LIKE
        | BOOLEAN_LIKE
        | ES_SYMBOL
        | UNIQUE_ES_SYMBOL
        | NON_PRIMITIVE;
    pub const INCLUDES_MASK: TypeFlags = ANY
        | UNKNOWN
        | PRIMITIVE
        | NEVER
        | OBJECT
        | UNION
        | INTERSECTION
        | NON_PRIMITIVE
        | TEMPLATE_LITERAL
        | STRING_MAPPING;
    pub const INCLUDES_MISSING_TYPE: TypeFlags = TYPE_PARAMETER;
    pub const INCLUDES_NON_WIDENING_TYPE: TypeFlags = INDEX;
    pub const INCLUDES_WILDCARD: TypeFlags = INDEXED_ACCESS;
    pub const INCLUDES_EMPTY_OBJECT: TypeFlags = CONDITIONAL;
    pub const INCLUDES_INSTANTIABLE: TypeFlags = SUBSTITUTION;
    pub const INCLUDES_CONSTRAINED_TYPE_VARIABLE: TypeFlags = RESERVED1;
    pub const INCLUDES_ERROR: TypeFlags = RESERVED2;
    pub const NOT_PRIMITIVE_UNION: TypeFlags =
        ANY | UNKNOWN | VOID | NEVER | OBJECT | INTERSECTION | INCLUDES_INSTANTIABLE;
}

/// `ObjectFlags` (`types.go`).
pub type ObjectFlags = u32;

pub mod object_flags {
    use super::ObjectFlags;

    pub const NONE: ObjectFlags = 0;
    pub const CLASS: ObjectFlags = 1 << 0;
    pub const INTERFACE: ObjectFlags = 1 << 1;
    pub const REFERENCE: ObjectFlags = 1 << 2;
    pub const TUPLE: ObjectFlags = 1 << 3;
    pub const ANONYMOUS: ObjectFlags = 1 << 4;
    pub const MAPPED: ObjectFlags = 1 << 5;
    pub const INSTANTIATED: ObjectFlags = 1 << 6;
    pub const OBJECT_LITERAL: ObjectFlags = 1 << 7;
    pub const EVOLVING_ARRAY: ObjectFlags = 1 << 8;
    pub const OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES: ObjectFlags = 1 << 9;
    pub const REVERSE_MAPPED: ObjectFlags = 1 << 10;
    pub const JSX_ATTRIBUTES: ObjectFlags = 1 << 11;
    pub const JS_LITERAL: ObjectFlags = 1 << 12;
    pub const FRESH_LITERAL: ObjectFlags = 1 << 13;
    pub const ARRAY_LITERAL: ObjectFlags = 1 << 14;
    pub const PRIMITIVE_UNION: ObjectFlags = 1 << 15;
    pub const CONTAINS_WIDENING_TYPE: ObjectFlags = 1 << 16;
    pub const CONTAINS_OBJECT_OR_ARRAY_LITERAL: ObjectFlags = 1 << 17;
    pub const NON_INFERRABLE_TYPE: ObjectFlags = 1 << 18;
    pub const COULD_CONTAIN_TYPE_VARIABLES_COMPUTED: ObjectFlags = 1 << 19;
    pub const COULD_CONTAIN_TYPE_VARIABLES: ObjectFlags = 1 << 20;
    pub const MEMBERS_RESOLVED: ObjectFlags = 1 << 21;
    pub const CLASS_OR_INTERFACE: ObjectFlags = CLASS | INTERFACE;
    pub const REQUIRES_WIDENING: ObjectFlags =
        CONTAINS_WIDENING_TYPE | CONTAINS_OBJECT_OR_ARRAY_LITERAL;
    pub const PROPAGATING_FLAGS: ObjectFlags =
        CONTAINS_WIDENING_TYPE | CONTAINS_OBJECT_OR_ARRAY_LITERAL | NON_INFERRABLE_TYPE;
    pub const INSTANTIATED_MAPPED: ObjectFlags = MAPPED | INSTANTIATED;
    pub const OBJECT_TYPE_KIND_MASK: ObjectFlags = CLASS_OR_INTERFACE
        | REFERENCE
        | TUPLE
        | ANONYMOUS
        | MAPPED
        | REVERSE_MAPPED
        | EVOLVING_ARRAY
        | INSTANTIATION_EXPRESSION_TYPE
        | SINGLE_SIGNATURE_TYPE;
    pub const CONTAINS_SPREAD: ObjectFlags = 1 << 22;
    pub const OBJECT_REST_TYPE: ObjectFlags = 1 << 23;
    pub const INSTANTIATION_EXPRESSION_TYPE: ObjectFlags = 1 << 24;
    pub const SINGLE_SIGNATURE_TYPE: ObjectFlags = 1 << 25;
    pub const IS_CLASS_INSTANCE_CLONE: ObjectFlags = 1 << 26;
    pub const IDENTICAL_BASE_TYPE_CALCULATED: ObjectFlags = 1 << 27;
    pub const IDENTICAL_BASE_TYPE_EXISTS: ObjectFlags = 1 << 28;
    pub const UNRESOLVED_MEMBERS: ObjectFlags = 1 << 29;
    pub const FROM_TYPE_NODE: ObjectFlags = 1 << 30;
    pub const IS_GENERIC_TYPE_COMPUTED: ObjectFlags = 1 << 22;
    pub const IS_GENERIC_OBJECT_TYPE: ObjectFlags = 1 << 23;
    pub const IS_GENERIC_INDEX_TYPE: ObjectFlags = 1 << 24;
    pub const IS_GENERIC_TYPE: ObjectFlags = IS_GENERIC_OBJECT_TYPE | IS_GENERIC_INDEX_TYPE;
    pub const CONTAINS_INTERSECTIONS: ObjectFlags = 1 << 25;
    pub const IS_UNKNOWN_LIKE_UNION_COMPUTED: ObjectFlags = 1 << 26;
    pub const IS_UNKNOWN_LIKE_UNION: ObjectFlags = 1 << 27;
    pub const IS_UNIFORM_ENUM_COMPUTED: ObjectFlags = 1 << 28;
    pub const IS_UNIFORM_ENUM: ObjectFlags = 1 << 29;
    pub const IS_NEVER_INTERSECTION_COMPUTED: ObjectFlags = 1 << 25;
    pub const IS_NEVER_INTERSECTION: ObjectFlags = 1 << 26;
    pub const IS_CONSTRAINED_TYPE_VARIABLE: ObjectFlags = 1 << 27;
}

/// `SignatureFlags` (`types.go`).
pub type SignatureFlags = u32;

pub mod signature_flags {
    use super::SignatureFlags;

    pub const NONE: SignatureFlags = 0;
    pub const HAS_REST_PARAMETER: SignatureFlags = 1 << 0;
    pub const HAS_LITERAL_TYPES: SignatureFlags = 1 << 1;
    pub const CONSTRUCT: SignatureFlags = 1 << 2;
    pub const ABSTRACT: SignatureFlags = 1 << 3;
    pub const IS_INNER_CALL_CHAIN: SignatureFlags = 1 << 4;
    pub const IS_OUTER_CALL_CHAIN: SignatureFlags = 1 << 5;
    pub const IS_UNTYPED_SIGNATURE_IN_JS_FILE: SignatureFlags = 1 << 6;
    pub const IS_NON_INFERRABLE: SignatureFlags = 1 << 7;
    pub const IS_SIGNATURE_CANDIDATE_FOR_OVERLOAD_FAILURE: SignatureFlags = 1 << 8;
    pub const PROPAGATING_FLAGS: SignatureFlags = HAS_REST_PARAMETER
        | HAS_LITERAL_TYPES
        | CONSTRUCT
        | ABSTRACT
        | IS_UNTYPED_SIGNATURE_IN_JS_FILE
        | IS_SIGNATURE_CANDIDATE_FOR_OVERLOAD_FAILURE;
    pub const CALL_CHAIN_FLAGS: SignatureFlags = IS_INNER_CALL_CHAIN | IS_OUTER_CALL_CHAIN;
}

/// `TypeFormatFlags` (`types.go`).
pub type TypeFormatFlags = u32;

pub mod type_format_flags {
    use super::TypeFormatFlags;

    pub const NONE: TypeFormatFlags = 0;
    pub const NO_TRUNCATION: TypeFormatFlags = 1 << 0;
    pub const WRITE_ARRAY_AS_GENERIC_TYPE: TypeFormatFlags = 1 << 1;
    pub const GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS: TypeFormatFlags = 1 << 2;
    pub const USE_STRUCTURAL_FALLBACK: TypeFormatFlags = 1 << 3;
    pub const WRITE_TYPE_ARGUMENTS_OF_SIGNATURE: TypeFormatFlags = 1 << 5;
    pub const USE_FULLY_QUALIFIED_TYPE: TypeFormatFlags = 1 << 6;
    pub const SUPPRESS_ANY_RETURN_TYPE: TypeFormatFlags = 1 << 8;
    pub const MULTILINE_OBJECT_LITERALS: TypeFormatFlags = 1 << 10;
    pub const WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL: TypeFormatFlags = 1 << 11;
    pub const USE_TYPE_OF_FUNCTION: TypeFormatFlags = 1 << 12;
    pub const OMIT_PARAMETER_MODIFIERS: TypeFormatFlags = 1 << 13;
    pub const USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE: TypeFormatFlags = 1 << 14;
    pub const USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE: TypeFormatFlags = 1 << 28;
    pub const NO_TYPE_REDUCTION: TypeFormatFlags = 1 << 29;
    pub const USE_INSTANTIATION_EXPRESSIONS: TypeFormatFlags = 1 << 30;
    pub const OMIT_THIS_PARAMETER: TypeFormatFlags = 1 << 25;
    pub const WRITE_CALL_STYLE_SIGNATURE: TypeFormatFlags = 1 << 27;
    pub const ALLOW_UNIQUE_ES_SYMBOL_TYPE: TypeFormatFlags = 1 << 20;
    pub const ADD_UNDEFINED: TypeFormatFlags = 1 << 17;
    pub const WRITE_ARROW_STYLE_SIGNATURE: TypeFormatFlags = 1 << 18;
    pub const IN_ARRAY_TYPE: TypeFormatFlags = 1 << 19;
    pub const IN_ELEMENT_TYPE: TypeFormatFlags = 1 << 21;
    pub const IN_FIRST_TYPE_ARGUMENT: TypeFormatFlags = 1 << 22;
    pub const IN_TYPE_ALIAS: TypeFormatFlags = 1 << 23;
    pub const NODE_BUILDER_FLAGS_MASK: TypeFormatFlags = NO_TRUNCATION
        | WRITE_ARRAY_AS_GENERIC_TYPE
        | GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS
        | USE_STRUCTURAL_FALLBACK
        | WRITE_TYPE_ARGUMENTS_OF_SIGNATURE
        | USE_FULLY_QUALIFIED_TYPE
        | SUPPRESS_ANY_RETURN_TYPE
        | MULTILINE_OBJECT_LITERALS
        | WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL
        | USE_TYPE_OF_FUNCTION
        | OMIT_PARAMETER_MODIFIERS
        | USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE
        | ALLOW_UNIQUE_ES_SYMBOL_TYPE
        | IN_TYPE_ALIAS
        | USE_INSTANTIATION_EXPRESSIONS
        | USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE
        | NO_TYPE_REDUCTION
        | OMIT_THIS_PARAMETER;
}

/// `SymbolFormatFlags` (`types.go`).
pub type SymbolFormatFlags = u32;

pub mod symbol_format_flags {
    use super::SymbolFormatFlags;

    pub const NONE: SymbolFormatFlags = 0;
    pub const WRITE_TYPE_PARAMETERS_OR_ARGUMENTS: SymbolFormatFlags = 1 << 0;
    pub const USE_ONLY_EXTERNAL_ALIASING: SymbolFormatFlags = 1 << 1;
    pub const ALLOW_ANY_NODE_KIND: SymbolFormatFlags = 1 << 2;
    pub const USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE: SymbolFormatFlags = 1 << 3;
    pub const WRITE_COMPUTED_PROPS: SymbolFormatFlags = 1 << 4;
    pub const DO_NOT_INCLUDE_SYMBOL_CHAIN: SymbolFormatFlags = 1 << 5;
}

/// `VarianceFlags` (`types.go`).
pub type VarianceFlags = u32;

pub mod variance_flags {
    use super::VarianceFlags;

    pub const INVARIANT: VarianceFlags = 0;
    pub const COVARIANT: VarianceFlags = 1 << 0;
    pub const CONTRAVARIANT: VarianceFlags = 1 << 1;
    pub const BIVARIANT: VarianceFlags = COVARIANT | CONTRAVARIANT;
    pub const INDEPENDENT: VarianceFlags = 1 << 2;
    pub const VARIANCE_MASK: VarianceFlags = INVARIANT | COVARIANT | CONTRAVARIANT | INDEPENDENT;
    pub const UNMEASURABLE: VarianceFlags = 1 << 3;
    pub const UNRELIABLE: VarianceFlags = 1 << 4;
    pub const ALLOWS_STRUCTURAL_FALLBACK: VarianceFlags = UNMEASURABLE | UNRELIABLE;
}

/// `AccessFlags` (`types.go`).
pub type AccessFlags = u32;

pub mod access_flags {
    use super::AccessFlags;

    pub const NONE: AccessFlags = 0;
    pub const INCLUDE_UNDEFINED: AccessFlags = 1 << 0;
    pub const NO_INDEX_SIGNATURES: AccessFlags = 1 << 1;
    pub const WRITING: AccessFlags = 1 << 2;
    pub const CACHE_SYMBOL: AccessFlags = 1 << 3;
    pub const ALLOW_MISSING: AccessFlags = 1 << 4;
    pub const EXPRESSION_POSITION: AccessFlags = 1 << 5;
    pub const REPORT_DEPRECATED: AccessFlags = 1 << 6;
    pub const SUPPRESS_NO_IMPLICIT_ANY_ERROR: AccessFlags = 1 << 7;
    pub const CONTEXTUAL: AccessFlags = 1 << 8;
    pub const PERSISTENT: AccessFlags = INCLUDE_UNDEFINED;
}

/// `NodeCheckFlags` (`types.go`).
pub type NodeCheckFlags = u32;

pub mod node_check_flags {
    use super::NodeCheckFlags;

    pub const NONE: NodeCheckFlags = 0;
    pub const TYPE_CHECKED: NodeCheckFlags = 1 << 0;
    pub const CONTEXT_CHECKED: NodeCheckFlags = 1 << 6;
    pub const ENUM_VALUES_COMPUTED: NodeCheckFlags = 1 << 10;
    pub const ASSIGNMENTS_MARKED: NodeCheckFlags = 1 << 17;
    pub const CONTAINS_CLASS_WITH_PRIVATE_IDENTIFIERS: NodeCheckFlags = 1 << 20;
    pub const CONTAINS_SUPER_PROPERTY_IN_STATIC_INITIALIZER: NodeCheckFlags = 1 << 21;
    pub const IN_CHECK_IDENTIFIER: NodeCheckFlags = 1 << 22;
    pub const INITIALIZER_IS_UNDEFINED: NodeCheckFlags = 1 << 24;
    pub const INITIALIZER_IS_UNDEFINED_COMPUTED: NodeCheckFlags = 1 << 25;
}

/// `ContextFlags` (`types.go`).
pub type ContextFlags = u32;

pub mod context_flags {
    use super::ContextFlags;

    pub const NONE: ContextFlags = 0;
    pub const SIGNATURE: ContextFlags = 1 << 0;
    pub const NO_CONSTRAINTS: ContextFlags = 1 << 1;
    pub const IGNORE_NODE_INFERENCES: ContextFlags = 1 << 2;
    pub const SKIP_BINDING_PATTERNS: ContextFlags = 1 << 3;
}

/// `ParseFlags` (`types.go`).
pub type ParseFlags = u32;

pub mod parse_flags {
    use super::ParseFlags;

    pub const NONE: ParseFlags = 0;
    pub const YIELD: ParseFlags = 1 << 0;
    pub const AWAIT: ParseFlags = 1 << 1;
    pub const TYPE: ParseFlags = 1 << 2;
    pub const IGNORE_MISSING_OPEN_BRACE: ParseFlags = 1 << 4;
    pub const JS_DOC: ParseFlags = 1 << 5;
}

/// `ExternalEmitHelpers` (`types.go`).
pub type ExternalEmitHelpers = u32;

pub mod external_emit_helpers {
    use super::ExternalEmitHelpers;

    pub const REST: ExternalEmitHelpers = 1 << 0;
    pub const DECORATE: ExternalEmitHelpers = 1 << 1;
    pub const METADATA: ExternalEmitHelpers = 1 << 2;
    pub const PARAM: ExternalEmitHelpers = 1 << 3;
    pub const AWAITER: ExternalEmitHelpers = 1 << 4;
    pub const AWAIT: ExternalEmitHelpers = 1 << 5;
    pub const ASYNC_GENERATOR: ExternalEmitHelpers = 1 << 6;
    pub const ASYNC_DELEGATOR: ExternalEmitHelpers = 1 << 7;
    pub const ASYNC_VALUES: ExternalEmitHelpers = 1 << 8;
    pub const EXPORT_STAR: ExternalEmitHelpers = 1 << 9;
    pub const IMPORT_STAR: ExternalEmitHelpers = 1 << 10;
    pub const IMPORT_DEFAULT: ExternalEmitHelpers = 1 << 11;
    pub const MAKE_TEMPLATE_OBJECT: ExternalEmitHelpers = 1 << 12;
    pub const CLASS_PRIVATE_FIELD_GET: ExternalEmitHelpers = 1 << 13;
    pub const CLASS_PRIVATE_FIELD_SET: ExternalEmitHelpers = 1 << 14;
    pub const CLASS_PRIVATE_FIELD_IN: ExternalEmitHelpers = 1 << 15;
    pub const SET_FUNCTION_NAME: ExternalEmitHelpers = 1 << 16;
    pub const PROP_KEY: ExternalEmitHelpers = 1 << 17;
    pub const ADD_DISPOSABLE_RESOURCE_AND_DISPOSE_RESOURCES: ExternalEmitHelpers = 1 << 18;
    pub const REWRITE_RELATIVE_IMPORT_EXTENSION: ExternalEmitHelpers = 1 << 19;
    pub const ES_DECORATE_AND_RUN_INITIALIZERS: ExternalEmitHelpers = DECORATE;
    pub const FIRST_EMIT_HELPER: ExternalEmitHelpers = REST;
    pub const LAST_EMIT_HELPER: ExternalEmitHelpers = REWRITE_RELATIVE_IMPORT_EXTENSION;
    pub const FOR_AWAIT_OF_INCLUDES: ExternalEmitHelpers = ASYNC_VALUES;
    pub const ASYNC_GENERATOR_INCLUDES: ExternalEmitHelpers = AWAIT | ASYNC_GENERATOR;
    pub const ASYNC_DELEGATOR_INCLUDES: ExternalEmitHelpers =
        AWAIT | ASYNC_DELEGATOR | ASYNC_VALUES;
}

/// `Ternary` (`types.go`).
pub type Ternary = i8;

pub mod ternary {
    use super::Ternary;

    pub const FALSE: Ternary = 0;
    pub const UNKNOWN: Ternary = 1;
    pub const MAYBE: Ternary = 3;
    pub const TRUE: Ternary = -1;
}

/// `SignatureKind` (`types.go`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum SignatureKind {
    Call = 0,
    Construct,
}

/// `MemberOverrideStatus` (`types.go`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum MemberOverrideStatus {
    None = 0,
    NeedsOverride,
    HasInvalidOverride,
}

/// `TypePredicateKind` (`types.go`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum TypePredicateKind {
    This = 0,
    Identifier,
    AssertsThis,
    AssertsIdentifier,
}

/// `ExhaustiveState` (`types.go`): switch exhaustiveness, computed lazily.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ExhaustiveState {
    #[default]
    Unknown = 0,
    Computing,
    False,
    True,
}

/// `MembersOrExportsResolutionKind` (`types.go`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum MembersOrExportsResolutionKind {
    ResolvedExports = 0,
    ResolvedMembers = 1,
}

const TYPE_FLAG_NAMES: [(TypeFlags, &str); 29] = [
    (type_flags::ANY, "Any"),
    (type_flags::UNKNOWN, "Unknown"),
    (type_flags::UNDEFINED, "Undefined"),
    (type_flags::NULL, "Null"),
    (type_flags::VOID, "Void"),
    (type_flags::STRING, "String"),
    (type_flags::NUMBER, "Number"),
    (type_flags::BIG_INT, "BigInt"),
    (type_flags::BOOLEAN, "Boolean"),
    (type_flags::ES_SYMBOL, "ESSymbol"),
    (type_flags::STRING_LITERAL, "StringLiteral"),
    (type_flags::NUMBER_LITERAL, "NumberLiteral"),
    (type_flags::BIG_INT_LITERAL, "BigIntLiteral"),
    (type_flags::BOOLEAN_LITERAL, "BooleanLiteral"),
    (type_flags::UNIQUE_ES_SYMBOL, "UniqueESSymbol"),
    (type_flags::ENUM_LITERAL, "EnumLiteral"),
    (type_flags::ENUM, "Enum"),
    (type_flags::NON_PRIMITIVE, "NonPrimitive"),
    (type_flags::NEVER, "Never"),
    (type_flags::TYPE_PARAMETER, "TypeParameter"),
    (type_flags::OBJECT, "Object"),
    (type_flags::INDEX, "Index"),
    (type_flags::TEMPLATE_LITERAL, "TemplateLiteral"),
    (type_flags::STRING_MAPPING, "StringMapping"),
    (type_flags::SUBSTITUTION, "Substitution"),
    (type_flags::INDEXED_ACCESS, "IndexedAccess"),
    (type_flags::CONDITIONAL, "Conditional"),
    (type_flags::UNION, "Union"),
    (type_flags::INTERSECTION, "Intersection"),
];

/// The individual flag names, or `["None"]`.
// port: tsc/internal/checker/types.go:FormatTypeFlags
pub fn format_type_flags(flags: TypeFlags) -> Vec<&'static str> {
    let mut result = Vec::with_capacity(flags.count_ones() as usize);
    for (flag, name) in TYPE_FLAG_NAMES {
        if flags & flag != 0 {
            result.push(name);
        }
    }
    if result.is_empty() {
        result.push("None");
    }
    result
}

/// Pipe-separated flag names.
// port: tsc/internal/checker/types.go:TypeFlags.String
pub fn type_flags_string(flags: TypeFlags) -> String {
    format_type_flags(flags).join("|")
}

// port: tsc/internal/checker/types.go:VarianceFlags.String
pub fn variance_flags_string(flags: VarianceFlags) -> String {
    let variance = flags & variance_flags::VARIANCE_MASK;
    let mut result = String::from(match variance {
        variance_flags::INVARIANT => "in out",
        variance_flags::BIVARIANT => "[bivariant]",
        variance_flags::CONTRAVARIANT => "in",
        variance_flags::COVARIANT => "out",
        variance_flags::INDEPENDENT => "[independent]",
        _ => "",
    });
    if flags & variance_flags::UNMEASURABLE != 0 {
        result.push_str(" (unmeasurable)");
    } else if flags & variance_flags::UNRELIABLE != 0 {
        result.push_str(" (unreliable)");
    }
    result
}
