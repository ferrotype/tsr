use crate::*;

fn observed(kind: &str) -> serde_json::Map<String, serde_json::Value> {
    let text = include_str!("../../../data/s08/checker-flag-observations.json");
    let root: serde_json::Value = serde_json::from_str(text).expect("valid observation JSON");
    root[kind]
        .as_object()
        .unwrap_or_else(|| panic!("observation set {kind}"))
        .clone()
}

fn check(kind: &str, ours: &[(&str, i64)]) {
    let expected = observed("flags");
    let expected = expected[kind]
        .as_object()
        .unwrap_or_else(|| panic!("observation set {kind}"));
    assert_eq!(
        ours.len(),
        expected.len(),
        "{kind}: every observed constant has a port"
    );
    for (name, value) in ours {
        assert_eq!(expected[*name].as_i64(), Some(*value), "{kind}::{name}");
    }
}

#[test]
fn type_flags_match_the_pinned_go_values() {
    check(
        "checker.TypeFlags",
        &[
            ("TypeFlagsNone", i64::from(type_flags::NONE)),
            ("TypeFlagsAny", i64::from(type_flags::ANY)),
            ("TypeFlagsUnknown", i64::from(type_flags::UNKNOWN)),
            ("TypeFlagsUndefined", i64::from(type_flags::UNDEFINED)),
            ("TypeFlagsNull", i64::from(type_flags::NULL)),
            ("TypeFlagsVoid", i64::from(type_flags::VOID)),
            ("TypeFlagsString", i64::from(type_flags::STRING)),
            ("TypeFlagsNumber", i64::from(type_flags::NUMBER)),
            ("TypeFlagsBigInt", i64::from(type_flags::BIG_INT)),
            ("TypeFlagsBoolean", i64::from(type_flags::BOOLEAN)),
            ("TypeFlagsESSymbol", i64::from(type_flags::ES_SYMBOL)),
            (
                "TypeFlagsStringLiteral",
                i64::from(type_flags::STRING_LITERAL),
            ),
            (
                "TypeFlagsNumberLiteral",
                i64::from(type_flags::NUMBER_LITERAL),
            ),
            (
                "TypeFlagsBigIntLiteral",
                i64::from(type_flags::BIG_INT_LITERAL),
            ),
            (
                "TypeFlagsBooleanLiteral",
                i64::from(type_flags::BOOLEAN_LITERAL),
            ),
            (
                "TypeFlagsUniqueESSymbol",
                i64::from(type_flags::UNIQUE_ES_SYMBOL),
            ),
            ("TypeFlagsEnumLiteral", i64::from(type_flags::ENUM_LITERAL)),
            ("TypeFlagsEnum", i64::from(type_flags::ENUM)),
            (
                "TypeFlagsNonPrimitive",
                i64::from(type_flags::NON_PRIMITIVE),
            ),
            ("TypeFlagsNever", i64::from(type_flags::NEVER)),
            (
                "TypeFlagsTypeParameter",
                i64::from(type_flags::TYPE_PARAMETER),
            ),
            ("TypeFlagsObject", i64::from(type_flags::OBJECT)),
            ("TypeFlagsIndex", i64::from(type_flags::INDEX)),
            (
                "TypeFlagsTemplateLiteral",
                i64::from(type_flags::TEMPLATE_LITERAL),
            ),
            (
                "TypeFlagsStringMapping",
                i64::from(type_flags::STRING_MAPPING),
            ),
            ("TypeFlagsSubstitution", i64::from(type_flags::SUBSTITUTION)),
            (
                "TypeFlagsIndexedAccess",
                i64::from(type_flags::INDEXED_ACCESS),
            ),
            ("TypeFlagsConditional", i64::from(type_flags::CONDITIONAL)),
            ("TypeFlagsUnion", i64::from(type_flags::UNION)),
            ("TypeFlagsIntersection", i64::from(type_flags::INTERSECTION)),
            ("TypeFlagsReserved1", i64::from(type_flags::RESERVED1)),
            ("TypeFlagsReserved2", i64::from(type_flags::RESERVED2)),
            ("TypeFlagsReserved3", i64::from(type_flags::RESERVED3)),
            (
                "TypeFlagsAnyOrUnknown",
                i64::from(type_flags::ANY_OR_UNKNOWN),
            ),
            ("TypeFlagsNullable", i64::from(type_flags::NULLABLE)),
            ("TypeFlagsLiteral", i64::from(type_flags::LITERAL)),
            ("TypeFlagsUnit", i64::from(type_flags::UNIT)),
            ("TypeFlagsFreshable", i64::from(type_flags::FRESHABLE)),
            (
                "TypeFlagsStringOrNumberLiteral",
                i64::from(type_flags::STRING_OR_NUMBER_LITERAL),
            ),
            (
                "TypeFlagsStringOrNumberLiteralOrUnique",
                i64::from(type_flags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE),
            ),
            (
                "TypeFlagsDefinitelyFalsy",
                i64::from(type_flags::DEFINITELY_FALSY),
            ),
            (
                "TypeFlagsPossiblyFalsy",
                i64::from(type_flags::POSSIBLY_FALSY),
            ),
            ("TypeFlagsIntrinsic", i64::from(type_flags::INTRINSIC)),
            ("TypeFlagsStringLike", i64::from(type_flags::STRING_LIKE)),
            ("TypeFlagsNumberLike", i64::from(type_flags::NUMBER_LIKE)),
            ("TypeFlagsBigIntLike", i64::from(type_flags::BIG_INT_LIKE)),
            ("TypeFlagsBooleanLike", i64::from(type_flags::BOOLEAN_LIKE)),
            ("TypeFlagsEnumLike", i64::from(type_flags::ENUM_LIKE)),
            (
                "TypeFlagsESSymbolLike",
                i64::from(type_flags::ES_SYMBOL_LIKE),
            ),
            ("TypeFlagsVoidLike", i64::from(type_flags::VOID_LIKE)),
            ("TypeFlagsPrimitive", i64::from(type_flags::PRIMITIVE)),
            (
                "TypeFlagsDefinitelyNonNullable",
                i64::from(type_flags::DEFINITELY_NON_NULLABLE),
            ),
            (
                "TypeFlagsDisjointDomains",
                i64::from(type_flags::DISJOINT_DOMAINS),
            ),
            (
                "TypeFlagsUnionOrIntersection",
                i64::from(type_flags::UNION_OR_INTERSECTION),
            ),
            (
                "TypeFlagsStructuredType",
                i64::from(type_flags::STRUCTURED_TYPE),
            ),
            (
                "TypeFlagsTypeVariable",
                i64::from(type_flags::TYPE_VARIABLE),
            ),
            (
                "TypeFlagsInstantiableNonPrimitive",
                i64::from(type_flags::INSTANTIABLE_NON_PRIMITIVE),
            ),
            (
                "TypeFlagsInstantiablePrimitive",
                i64::from(type_flags::INSTANTIABLE_PRIMITIVE),
            ),
            ("TypeFlagsInstantiable", i64::from(type_flags::INSTANTIABLE)),
            (
                "TypeFlagsStructuredOrInstantiable",
                i64::from(type_flags::STRUCTURED_OR_INSTANTIABLE),
            ),
            (
                "TypeFlagsObjectFlagsType",
                i64::from(type_flags::OBJECT_FLAGS_TYPE),
            ),
            ("TypeFlagsSimplifiable", i64::from(type_flags::SIMPLIFIABLE)),
            ("TypeFlagsSingleton", i64::from(type_flags::SINGLETON)),
            ("TypeFlagsNarrowable", i64::from(type_flags::NARROWABLE)),
            (
                "TypeFlagsIncludesMask",
                i64::from(type_flags::INCLUDES_MASK),
            ),
            (
                "TypeFlagsIncludesMissingType",
                i64::from(type_flags::INCLUDES_MISSING_TYPE),
            ),
            (
                "TypeFlagsIncludesNonWideningType",
                i64::from(type_flags::INCLUDES_NON_WIDENING_TYPE),
            ),
            (
                "TypeFlagsIncludesWildcard",
                i64::from(type_flags::INCLUDES_WILDCARD),
            ),
            (
                "TypeFlagsIncludesEmptyObject",
                i64::from(type_flags::INCLUDES_EMPTY_OBJECT),
            ),
            (
                "TypeFlagsIncludesInstantiable",
                i64::from(type_flags::INCLUDES_INSTANTIABLE),
            ),
            (
                "TypeFlagsIncludesConstrainedTypeVariable",
                i64::from(type_flags::INCLUDES_CONSTRAINED_TYPE_VARIABLE),
            ),
            (
                "TypeFlagsIncludesError",
                i64::from(type_flags::INCLUDES_ERROR),
            ),
            (
                "TypeFlagsNotPrimitiveUnion",
                i64::from(type_flags::NOT_PRIMITIVE_UNION),
            ),
        ],
    );
}

#[test]
fn object_flags_match_the_pinned_go_values() {
    check(
        "checker.ObjectFlags",
        &[
            ("ObjectFlagsNone", i64::from(object_flags::NONE)),
            ("ObjectFlagsClass", i64::from(object_flags::CLASS)),
            ("ObjectFlagsInterface", i64::from(object_flags::INTERFACE)),
            ("ObjectFlagsReference", i64::from(object_flags::REFERENCE)),
            ("ObjectFlagsTuple", i64::from(object_flags::TUPLE)),
            ("ObjectFlagsAnonymous", i64::from(object_flags::ANONYMOUS)),
            ("ObjectFlagsMapped", i64::from(object_flags::MAPPED)),
            (
                "ObjectFlagsInstantiated",
                i64::from(object_flags::INSTANTIATED),
            ),
            (
                "ObjectFlagsObjectLiteral",
                i64::from(object_flags::OBJECT_LITERAL),
            ),
            (
                "ObjectFlagsEvolvingArray",
                i64::from(object_flags::EVOLVING_ARRAY),
            ),
            (
                "ObjectFlagsObjectLiteralPatternWithComputedProperties",
                i64::from(object_flags::OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES),
            ),
            (
                "ObjectFlagsReverseMapped",
                i64::from(object_flags::REVERSE_MAPPED),
            ),
            (
                "ObjectFlagsJsxAttributes",
                i64::from(object_flags::JSX_ATTRIBUTES),
            ),
            ("ObjectFlagsJSLiteral", i64::from(object_flags::JS_LITERAL)),
            (
                "ObjectFlagsFreshLiteral",
                i64::from(object_flags::FRESH_LITERAL),
            ),
            (
                "ObjectFlagsArrayLiteral",
                i64::from(object_flags::ARRAY_LITERAL),
            ),
            (
                "ObjectFlagsPrimitiveUnion",
                i64::from(object_flags::PRIMITIVE_UNION),
            ),
            (
                "ObjectFlagsContainsWideningType",
                i64::from(object_flags::CONTAINS_WIDENING_TYPE),
            ),
            (
                "ObjectFlagsContainsObjectOrArrayLiteral",
                i64::from(object_flags::CONTAINS_OBJECT_OR_ARRAY_LITERAL),
            ),
            (
                "ObjectFlagsNonInferrableType",
                i64::from(object_flags::NON_INFERRABLE_TYPE),
            ),
            (
                "ObjectFlagsCouldContainTypeVariablesComputed",
                i64::from(object_flags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED),
            ),
            (
                "ObjectFlagsCouldContainTypeVariables",
                i64::from(object_flags::COULD_CONTAIN_TYPE_VARIABLES),
            ),
            (
                "ObjectFlagsMembersResolved",
                i64::from(object_flags::MEMBERS_RESOLVED),
            ),
            (
                "ObjectFlagsClassOrInterface",
                i64::from(object_flags::CLASS_OR_INTERFACE),
            ),
            (
                "ObjectFlagsRequiresWidening",
                i64::from(object_flags::REQUIRES_WIDENING),
            ),
            (
                "ObjectFlagsPropagatingFlags",
                i64::from(object_flags::PROPAGATING_FLAGS),
            ),
            (
                "ObjectFlagsInstantiatedMapped",
                i64::from(object_flags::INSTANTIATED_MAPPED),
            ),
            (
                "ObjectFlagsObjectTypeKindMask",
                i64::from(object_flags::OBJECT_TYPE_KIND_MASK),
            ),
            (
                "ObjectFlagsContainsSpread",
                i64::from(object_flags::CONTAINS_SPREAD),
            ),
            (
                "ObjectFlagsObjectRestType",
                i64::from(object_flags::OBJECT_REST_TYPE),
            ),
            (
                "ObjectFlagsInstantiationExpressionType",
                i64::from(object_flags::INSTANTIATION_EXPRESSION_TYPE),
            ),
            (
                "ObjectFlagsSingleSignatureType",
                i64::from(object_flags::SINGLE_SIGNATURE_TYPE),
            ),
            (
                "ObjectFlagsIsClassInstanceClone",
                i64::from(object_flags::IS_CLASS_INSTANCE_CLONE),
            ),
            (
                "ObjectFlagsIdenticalBaseTypeCalculated",
                i64::from(object_flags::IDENTICAL_BASE_TYPE_CALCULATED),
            ),
            (
                "ObjectFlagsIdenticalBaseTypeExists",
                i64::from(object_flags::IDENTICAL_BASE_TYPE_EXISTS),
            ),
            (
                "ObjectFlagsUnresolvedMembers",
                i64::from(object_flags::UNRESOLVED_MEMBERS),
            ),
            (
                "ObjectFlagsFromTypeNode",
                i64::from(object_flags::FROM_TYPE_NODE),
            ),
            (
                "ObjectFlagsIsGenericTypeComputed",
                i64::from(object_flags::IS_GENERIC_TYPE_COMPUTED),
            ),
            (
                "ObjectFlagsIsGenericObjectType",
                i64::from(object_flags::IS_GENERIC_OBJECT_TYPE),
            ),
            (
                "ObjectFlagsIsGenericIndexType",
                i64::from(object_flags::IS_GENERIC_INDEX_TYPE),
            ),
            (
                "ObjectFlagsIsGenericType",
                i64::from(object_flags::IS_GENERIC_TYPE),
            ),
            (
                "ObjectFlagsContainsIntersections",
                i64::from(object_flags::CONTAINS_INTERSECTIONS),
            ),
            (
                "ObjectFlagsIsUnknownLikeUnionComputed",
                i64::from(object_flags::IS_UNKNOWN_LIKE_UNION_COMPUTED),
            ),
            (
                "ObjectFlagsIsUnknownLikeUnion",
                i64::from(object_flags::IS_UNKNOWN_LIKE_UNION),
            ),
            (
                "ObjectFlagsIsUniformEnumComputed",
                i64::from(object_flags::IS_UNIFORM_ENUM_COMPUTED),
            ),
            (
                "ObjectFlagsIsUniformEnum",
                i64::from(object_flags::IS_UNIFORM_ENUM),
            ),
            (
                "ObjectFlagsIsNeverIntersectionComputed",
                i64::from(object_flags::IS_NEVER_INTERSECTION_COMPUTED),
            ),
            (
                "ObjectFlagsIsNeverIntersection",
                i64::from(object_flags::IS_NEVER_INTERSECTION),
            ),
            (
                "ObjectFlagsIsConstrainedTypeVariable",
                i64::from(object_flags::IS_CONSTRAINED_TYPE_VARIABLE),
            ),
        ],
    );
}

#[test]
fn signature_flags_match_the_pinned_go_values() {
    check(
        "checker.SignatureFlags",
        &[
            ("SignatureFlagsNone", i64::from(signature_flags::NONE)),
            (
                "SignatureFlagsHasRestParameter",
                i64::from(signature_flags::HAS_REST_PARAMETER),
            ),
            (
                "SignatureFlagsHasLiteralTypes",
                i64::from(signature_flags::HAS_LITERAL_TYPES),
            ),
            (
                "SignatureFlagsConstruct",
                i64::from(signature_flags::CONSTRUCT),
            ),
            (
                "SignatureFlagsAbstract",
                i64::from(signature_flags::ABSTRACT),
            ),
            (
                "SignatureFlagsIsInnerCallChain",
                i64::from(signature_flags::IS_INNER_CALL_CHAIN),
            ),
            (
                "SignatureFlagsIsOuterCallChain",
                i64::from(signature_flags::IS_OUTER_CALL_CHAIN),
            ),
            (
                "SignatureFlagsIsUntypedSignatureInJSFile",
                i64::from(signature_flags::IS_UNTYPED_SIGNATURE_IN_JS_FILE),
            ),
            (
                "SignatureFlagsIsNonInferrable",
                i64::from(signature_flags::IS_NON_INFERRABLE),
            ),
            (
                "SignatureFlagsIsSignatureCandidateForOverloadFailure",
                i64::from(signature_flags::IS_SIGNATURE_CANDIDATE_FOR_OVERLOAD_FAILURE),
            ),
            (
                "SignatureFlagsPropagatingFlags",
                i64::from(signature_flags::PROPAGATING_FLAGS),
            ),
            (
                "SignatureFlagsCallChainFlags",
                i64::from(signature_flags::CALL_CHAIN_FLAGS),
            ),
        ],
    );
}

#[test]
fn type_format_flags_match_the_pinned_go_values() {
    check(
        "checker.TypeFormatFlags",
        &[
            ("TypeFormatFlagsNone", i64::from(type_format_flags::NONE)),
            (
                "TypeFormatFlagsNoTruncation",
                i64::from(type_format_flags::NO_TRUNCATION),
            ),
            (
                "TypeFormatFlagsWriteArrayAsGenericType",
                i64::from(type_format_flags::WRITE_ARRAY_AS_GENERIC_TYPE),
            ),
            (
                "TypeFormatFlagsGenerateNamesForShadowedTypeParams",
                i64::from(type_format_flags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS),
            ),
            (
                "TypeFormatFlagsUseStructuralFallback",
                i64::from(type_format_flags::USE_STRUCTURAL_FALLBACK),
            ),
            (
                "TypeFormatFlagsWriteTypeArgumentsOfSignature",
                i64::from(type_format_flags::WRITE_TYPE_ARGUMENTS_OF_SIGNATURE),
            ),
            (
                "TypeFormatFlagsUseFullyQualifiedType",
                i64::from(type_format_flags::USE_FULLY_QUALIFIED_TYPE),
            ),
            (
                "TypeFormatFlagsSuppressAnyReturnType",
                i64::from(type_format_flags::SUPPRESS_ANY_RETURN_TYPE),
            ),
            (
                "TypeFormatFlagsMultilineObjectLiterals",
                i64::from(type_format_flags::MULTILINE_OBJECT_LITERALS),
            ),
            (
                "TypeFormatFlagsWriteClassExpressionAsTypeLiteral",
                i64::from(type_format_flags::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL),
            ),
            (
                "TypeFormatFlagsUseTypeOfFunction",
                i64::from(type_format_flags::USE_TYPE_OF_FUNCTION),
            ),
            (
                "TypeFormatFlagsOmitParameterModifiers",
                i64::from(type_format_flags::OMIT_PARAMETER_MODIFIERS),
            ),
            (
                "TypeFormatFlagsUseAliasDefinedOutsideCurrentScope",
                i64::from(type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE),
            ),
            (
                "TypeFormatFlagsUseSingleQuotesForStringLiteralType",
                i64::from(type_format_flags::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE),
            ),
            (
                "TypeFormatFlagsNoTypeReduction",
                i64::from(type_format_flags::NO_TYPE_REDUCTION),
            ),
            (
                "TypeFormatFlagsUseInstantiationExpressions",
                i64::from(type_format_flags::USE_INSTANTIATION_EXPRESSIONS),
            ),
            (
                "TypeFormatFlagsOmitThisParameter",
                i64::from(type_format_flags::OMIT_THIS_PARAMETER),
            ),
            (
                "TypeFormatFlagsWriteCallStyleSignature",
                i64::from(type_format_flags::WRITE_CALL_STYLE_SIGNATURE),
            ),
            (
                "TypeFormatFlagsAllowUniqueESSymbolType",
                i64::from(type_format_flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE),
            ),
            (
                "TypeFormatFlagsAddUndefined",
                i64::from(type_format_flags::ADD_UNDEFINED),
            ),
            (
                "TypeFormatFlagsWriteArrowStyleSignature",
                i64::from(type_format_flags::WRITE_ARROW_STYLE_SIGNATURE),
            ),
            (
                "TypeFormatFlagsInArrayType",
                i64::from(type_format_flags::IN_ARRAY_TYPE),
            ),
            (
                "TypeFormatFlagsInElementType",
                i64::from(type_format_flags::IN_ELEMENT_TYPE),
            ),
            (
                "TypeFormatFlagsInFirstTypeArgument",
                i64::from(type_format_flags::IN_FIRST_TYPE_ARGUMENT),
            ),
            (
                "TypeFormatFlagsInTypeAlias",
                i64::from(type_format_flags::IN_TYPE_ALIAS),
            ),
            (
                "TypeFormatFlagsNodeBuilderFlagsMask",
                i64::from(type_format_flags::NODE_BUILDER_FLAGS_MASK),
            ),
        ],
    );
}

#[test]
fn symbol_format_flags_match_the_pinned_go_values() {
    check(
        "checker.SymbolFormatFlags",
        &[
            (
                "SymbolFormatFlagsNone",
                i64::from(symbol_format_flags::NONE),
            ),
            (
                "SymbolFormatFlagsWriteTypeParametersOrArguments",
                i64::from(symbol_format_flags::WRITE_TYPE_PARAMETERS_OR_ARGUMENTS),
            ),
            (
                "SymbolFormatFlagsUseOnlyExternalAliasing",
                i64::from(symbol_format_flags::USE_ONLY_EXTERNAL_ALIASING),
            ),
            (
                "SymbolFormatFlagsAllowAnyNodeKind",
                i64::from(symbol_format_flags::ALLOW_ANY_NODE_KIND),
            ),
            (
                "SymbolFormatFlagsUseAliasDefinedOutsideCurrentScope",
                i64::from(symbol_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE),
            ),
            (
                "SymbolFormatFlagsWriteComputedProps",
                i64::from(symbol_format_flags::WRITE_COMPUTED_PROPS),
            ),
            (
                "SymbolFormatFlagsDoNotIncludeSymbolChain",
                i64::from(symbol_format_flags::DO_NOT_INCLUDE_SYMBOL_CHAIN),
            ),
        ],
    );
}

#[test]
fn variance_flags_match_the_pinned_go_values() {
    check(
        "checker.VarianceFlags",
        &[
            (
                "VarianceFlagsInvariant",
                i64::from(variance_flags::INVARIANT),
            ),
            (
                "VarianceFlagsCovariant",
                i64::from(variance_flags::COVARIANT),
            ),
            (
                "VarianceFlagsContravariant",
                i64::from(variance_flags::CONTRAVARIANT),
            ),
            (
                "VarianceFlagsBivariant",
                i64::from(variance_flags::BIVARIANT),
            ),
            (
                "VarianceFlagsIndependent",
                i64::from(variance_flags::INDEPENDENT),
            ),
            (
                "VarianceFlagsVarianceMask",
                i64::from(variance_flags::VARIANCE_MASK),
            ),
            (
                "VarianceFlagsUnmeasurable",
                i64::from(variance_flags::UNMEASURABLE),
            ),
            (
                "VarianceFlagsUnreliable",
                i64::from(variance_flags::UNRELIABLE),
            ),
            (
                "VarianceFlagsAllowsStructuralFallback",
                i64::from(variance_flags::ALLOWS_STRUCTURAL_FALLBACK),
            ),
        ],
    );
}

#[test]
fn access_flags_match_the_pinned_go_values() {
    check(
        "checker.AccessFlags",
        &[
            ("AccessFlagsNone", i64::from(access_flags::NONE)),
            (
                "AccessFlagsIncludeUndefined",
                i64::from(access_flags::INCLUDE_UNDEFINED),
            ),
            (
                "AccessFlagsNoIndexSignatures",
                i64::from(access_flags::NO_INDEX_SIGNATURES),
            ),
            ("AccessFlagsWriting", i64::from(access_flags::WRITING)),
            (
                "AccessFlagsCacheSymbol",
                i64::from(access_flags::CACHE_SYMBOL),
            ),
            (
                "AccessFlagsAllowMissing",
                i64::from(access_flags::ALLOW_MISSING),
            ),
            (
                "AccessFlagsExpressionPosition",
                i64::from(access_flags::EXPRESSION_POSITION),
            ),
            (
                "AccessFlagsReportDeprecated",
                i64::from(access_flags::REPORT_DEPRECATED),
            ),
            (
                "AccessFlagsSuppressNoImplicitAnyError",
                i64::from(access_flags::SUPPRESS_NO_IMPLICIT_ANY_ERROR),
            ),
            ("AccessFlagsContextual", i64::from(access_flags::CONTEXTUAL)),
            ("AccessFlagsPersistent", i64::from(access_flags::PERSISTENT)),
        ],
    );
}

#[test]
fn node_check_flags_match_the_pinned_go_values() {
    check(
        "checker.NodeCheckFlags",
        &[
            ("NodeCheckFlagsNone", i64::from(node_check_flags::NONE)),
            (
                "NodeCheckFlagsTypeChecked",
                i64::from(node_check_flags::TYPE_CHECKED),
            ),
            (
                "NodeCheckFlagsContextChecked",
                i64::from(node_check_flags::CONTEXT_CHECKED),
            ),
            (
                "NodeCheckFlagsEnumValuesComputed",
                i64::from(node_check_flags::ENUM_VALUES_COMPUTED),
            ),
            (
                "NodeCheckFlagsAssignmentsMarked",
                i64::from(node_check_flags::ASSIGNMENTS_MARKED),
            ),
            (
                "NodeCheckFlagsContainsClassWithPrivateIdentifiers",
                i64::from(node_check_flags::CONTAINS_CLASS_WITH_PRIVATE_IDENTIFIERS),
            ),
            (
                "NodeCheckFlagsContainsSuperPropertyInStaticInitializer",
                i64::from(node_check_flags::CONTAINS_SUPER_PROPERTY_IN_STATIC_INITIALIZER),
            ),
            (
                "NodeCheckFlagsInCheckIdentifier",
                i64::from(node_check_flags::IN_CHECK_IDENTIFIER),
            ),
            (
                "NodeCheckFlagsInitializerIsUndefined",
                i64::from(node_check_flags::INITIALIZER_IS_UNDEFINED),
            ),
            (
                "NodeCheckFlagsInitializerIsUndefinedComputed",
                i64::from(node_check_flags::INITIALIZER_IS_UNDEFINED_COMPUTED),
            ),
        ],
    );
}

#[test]
fn context_flags_match_the_pinned_go_values() {
    check(
        "checker.ContextFlags",
        &[
            ("ContextFlagsNone", i64::from(context_flags::NONE)),
            ("ContextFlagsSignature", i64::from(context_flags::SIGNATURE)),
            (
                "ContextFlagsNoConstraints",
                i64::from(context_flags::NO_CONSTRAINTS),
            ),
            (
                "ContextFlagsIgnoreNodeInferences",
                i64::from(context_flags::IGNORE_NODE_INFERENCES),
            ),
            (
                "ContextFlagsSkipBindingPatterns",
                i64::from(context_flags::SKIP_BINDING_PATTERNS),
            ),
        ],
    );
}

#[test]
fn parse_flags_match_the_pinned_go_values() {
    check(
        "checker.ParseFlags",
        &[
            ("ParseFlagsNone", i64::from(parse_flags::NONE)),
            ("ParseFlagsYield", i64::from(parse_flags::YIELD)),
            ("ParseFlagsAwait", i64::from(parse_flags::AWAIT)),
            ("ParseFlagsType", i64::from(parse_flags::TYPE)),
            (
                "ParseFlagsIgnoreMissingOpenBrace",
                i64::from(parse_flags::IGNORE_MISSING_OPEN_BRACE),
            ),
            ("ParseFlagsJSDoc", i64::from(parse_flags::JS_DOC)),
        ],
    );
}

#[test]
fn external_emit_helpers_match_the_pinned_go_values() {
    check(
        "checker.ExternalEmitHelpers",
        &[
            (
                "ExternalEmitHelpersRest",
                i64::from(external_emit_helpers::REST),
            ),
            (
                "ExternalEmitHelpersDecorate",
                i64::from(external_emit_helpers::DECORATE),
            ),
            (
                "ExternalEmitHelpersMetadata",
                i64::from(external_emit_helpers::METADATA),
            ),
            (
                "ExternalEmitHelpersParam",
                i64::from(external_emit_helpers::PARAM),
            ),
            (
                "ExternalEmitHelpersAwaiter",
                i64::from(external_emit_helpers::AWAITER),
            ),
            (
                "ExternalEmitHelpersAwait",
                i64::from(external_emit_helpers::AWAIT),
            ),
            (
                "ExternalEmitHelpersAsyncGenerator",
                i64::from(external_emit_helpers::ASYNC_GENERATOR),
            ),
            (
                "ExternalEmitHelpersAsyncDelegator",
                i64::from(external_emit_helpers::ASYNC_DELEGATOR),
            ),
            (
                "ExternalEmitHelpersAsyncValues",
                i64::from(external_emit_helpers::ASYNC_VALUES),
            ),
            (
                "ExternalEmitHelpersExportStar",
                i64::from(external_emit_helpers::EXPORT_STAR),
            ),
            (
                "ExternalEmitHelpersImportStar",
                i64::from(external_emit_helpers::IMPORT_STAR),
            ),
            (
                "ExternalEmitHelpersImportDefault",
                i64::from(external_emit_helpers::IMPORT_DEFAULT),
            ),
            (
                "ExternalEmitHelpersMakeTemplateObject",
                i64::from(external_emit_helpers::MAKE_TEMPLATE_OBJECT),
            ),
            (
                "ExternalEmitHelpersClassPrivateFieldGet",
                i64::from(external_emit_helpers::CLASS_PRIVATE_FIELD_GET),
            ),
            (
                "ExternalEmitHelpersClassPrivateFieldSet",
                i64::from(external_emit_helpers::CLASS_PRIVATE_FIELD_SET),
            ),
            (
                "ExternalEmitHelpersClassPrivateFieldIn",
                i64::from(external_emit_helpers::CLASS_PRIVATE_FIELD_IN),
            ),
            (
                "ExternalEmitHelpersSetFunctionName",
                i64::from(external_emit_helpers::SET_FUNCTION_NAME),
            ),
            (
                "ExternalEmitHelpersPropKey",
                i64::from(external_emit_helpers::PROP_KEY),
            ),
            (
                "ExternalEmitHelpersAddDisposableResourceAndDisposeResources",
                i64::from(external_emit_helpers::ADD_DISPOSABLE_RESOURCE_AND_DISPOSE_RESOURCES),
            ),
            (
                "ExternalEmitHelpersRewriteRelativeImportExtension",
                i64::from(external_emit_helpers::REWRITE_RELATIVE_IMPORT_EXTENSION),
            ),
            (
                "ExternalEmitHelpersESDecorateAndRunInitializers",
                i64::from(external_emit_helpers::ES_DECORATE_AND_RUN_INITIALIZERS),
            ),
            (
                "ExternalEmitHelpersFirstEmitHelper",
                i64::from(external_emit_helpers::FIRST_EMIT_HELPER),
            ),
            (
                "ExternalEmitHelpersLastEmitHelper",
                i64::from(external_emit_helpers::LAST_EMIT_HELPER),
            ),
            (
                "ExternalEmitHelpersForAwaitOfIncludes",
                i64::from(external_emit_helpers::FOR_AWAIT_OF_INCLUDES),
            ),
            (
                "ExternalEmitHelpersAsyncGeneratorIncludes",
                i64::from(external_emit_helpers::ASYNC_GENERATOR_INCLUDES),
            ),
            (
                "ExternalEmitHelpersAsyncDelegatorIncludes",
                i64::from(external_emit_helpers::ASYNC_DELEGATOR_INCLUDES),
            ),
        ],
    );
}

#[test]
fn ternary_match_the_pinned_go_values() {
    check(
        "checker.Ternary",
        &[
            ("TernaryFalse", i64::from(ternary::FALSE)),
            ("TernaryUnknown", i64::from(ternary::UNKNOWN)),
            ("TernaryMaybe", i64::from(ternary::MAYBE)),
            ("TernaryTrue", i64::from(ternary::TRUE)),
        ],
    );
}

#[test]
fn type_system_property_names_match_the_pinned_go_values() {
    check(
        "checker.TypeSystemPropertyName",
        &[
            (
                "TypeSystemPropertyNameType",
                TypeSystemPropertyName::Type as i64,
            ),
            (
                "TypeSystemPropertyNameResolvedBaseConstructorType",
                TypeSystemPropertyName::ResolvedBaseConstructorType as i64,
            ),
            (
                "TypeSystemPropertyNameDeclaredType",
                TypeSystemPropertyName::DeclaredType as i64,
            ),
            (
                "TypeSystemPropertyNameResolvedReturnType",
                TypeSystemPropertyName::ResolvedReturnType as i64,
            ),
            (
                "TypeSystemPropertyNameResolvedBaseConstraint",
                TypeSystemPropertyName::ResolvedBaseConstraint as i64,
            ),
            (
                "TypeSystemPropertyNameResolvedTypeArguments",
                TypeSystemPropertyName::ResolvedTypeArguments as i64,
            ),
            (
                "TypeSystemPropertyNameResolvedBaseTypes",
                TypeSystemPropertyName::ResolvedBaseTypes as i64,
            ),
            (
                "TypeSystemPropertyNameWriteType",
                TypeSystemPropertyName::WriteType as i64,
            ),
            (
                "TypeSystemPropertyNameInitializerIsUndefined",
                TypeSystemPropertyName::InitializerIsUndefined as i64,
            ),
            (
                "TypeSystemPropertyNameAliasTarget",
                TypeSystemPropertyName::AliasTarget as i64,
            ),
        ],
    );
}

#[test]
fn display_constants_match_the_pinned_go_values() {
    let consts = observed("consts");
    assert_eq!(
        consts["maxSerializationLevel"].as_i64(),
        Some(i64::from(MAX_SERIALIZATION_LEVEL))
    );
    assert_eq!(
        consts["defaultMaximumTruncationLength"].as_u64(),
        Some(DEFAULT_MAXIMUM_TRUNCATION_LENGTH as u64)
    );
    assert_eq!(
        consts["noTruncationMaximumTruncationLength"].as_u64(),
        Some(NO_TRUNCATION_MAXIMUM_TRUNCATION_LENGTH as u64)
    );
}

#[test]
fn flag_formatting_follows_upstream() {
    assert_eq!(format_type_flags(type_flags::NONE), ["None"]);
    assert_eq!(
        type_flags_string(type_flags::STRING_LITERAL | type_flags::ENUM_LITERAL),
        "StringLiteral|EnumLiteral"
    );
    assert_eq!(
        type_flags_string(type_flags::RESERVED1),
        "None",
        "reserved bits have no name"
    );
    assert_eq!(variance_flags_string(variance_flags::INVARIANT), "in out");
    assert_eq!(
        variance_flags_string(variance_flags::COVARIANT | variance_flags::UNRELIABLE),
        "out (unreliable)"
    );
    assert_eq!(
        variance_flags_string(
            variance_flags::BIVARIANT | variance_flags::UNMEASURABLE | variance_flags::UNRELIABLE
        ),
        "[bivariant] (unmeasurable)"
    );
    assert_eq!(
        variance_flags_string(variance_flags::INDEPENDENT),
        "[independent]"
    );
}
