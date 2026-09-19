use crate::{flags, internal_flags, Flags, InternalFlags};

/// The flag values recorded from the pinned Go package (`data/s08/checker-flag-observations.json`).
fn observed(kind: &str) -> serde_json::Map<String, serde_json::Value> {
    let text = include_str!("../../../data/s08/checker-flag-observations.json");
    let root: serde_json::Value = serde_json::from_str(text).expect("valid observation JSON");
    root["flags"][kind]
        .as_object()
        .unwrap_or_else(|| panic!("observation set {kind}"))
        .clone()
}

#[test]
fn flags_match_the_pinned_go_values() {
    let expected = observed("nodebuilder.Flags");
    let ours: &[(&str, Flags)] = &[
        ("FlagsNone", flags::NONE),
        ("FlagsNoTruncation", flags::NO_TRUNCATION),
        (
            "FlagsWriteArrayAsGenericType",
            flags::WRITE_ARRAY_AS_GENERIC_TYPE,
        ),
        (
            "FlagsGenerateNamesForShadowedTypeParams",
            flags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS,
        ),
        ("FlagsUseStructuralFallback", flags::USE_STRUCTURAL_FALLBACK),
        (
            "FlagsForbidIndexedAccessSymbolReferences",
            flags::FORBID_INDEXED_ACCESS_SYMBOL_REFERENCES,
        ),
        (
            "FlagsWriteTypeArgumentsOfSignature",
            flags::WRITE_TYPE_ARGUMENTS_OF_SIGNATURE,
        ),
        (
            "FlagsUseFullyQualifiedType",
            flags::USE_FULLY_QUALIFIED_TYPE,
        ),
        (
            "FlagsUseOnlyExternalAliasing",
            flags::USE_ONLY_EXTERNAL_ALIASING,
        ),
        (
            "FlagsSuppressAnyReturnType",
            flags::SUPPRESS_ANY_RETURN_TYPE,
        ),
        (
            "FlagsWriteTypeParametersInQualifiedName",
            flags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME,
        ),
        (
            "FlagsMultilineObjectLiterals",
            flags::MULTILINE_OBJECT_LITERALS,
        ),
        (
            "FlagsWriteClassExpressionAsTypeLiteral",
            flags::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL,
        ),
        ("FlagsUseTypeOfFunction", flags::USE_TYPE_OF_FUNCTION),
        (
            "FlagsOmitParameterModifiers",
            flags::OMIT_PARAMETER_MODIFIERS,
        ),
        (
            "FlagsUseAliasDefinedOutsideCurrentScope",
            flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
        ),
        (
            "FlagsUseSingleQuotesForStringLiteralType",
            flags::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE,
        ),
        ("FlagsNoTypeReduction", flags::NO_TYPE_REDUCTION),
        (
            "FlagsUseInstantiationExpressions",
            flags::USE_INSTANTIATION_EXPRESSIONS,
        ),
        ("FlagsOmitThisParameter", flags::OMIT_THIS_PARAMETER),
        (
            "FlagsWriteCallStyleSignature",
            flags::WRITE_CALL_STYLE_SIGNATURE,
        ),
        (
            "FlagsAllowThisInObjectLiteral",
            flags::ALLOW_THIS_IN_OBJECT_LITERAL,
        ),
        (
            "FlagsAllowQualifiedNameInPlaceOfIdentifier",
            flags::ALLOW_QUALIFIED_NAME_IN_PLACE_OF_IDENTIFIER,
        ),
        (
            "FlagsAllowAnonymousIdentifier",
            flags::ALLOW_ANONYMOUS_IDENTIFIER,
        ),
        (
            "FlagsAllowEmptyUnionOrIntersection",
            flags::ALLOW_EMPTY_UNION_OR_INTERSECTION,
        ),
        ("FlagsAllowEmptyTuple", flags::ALLOW_EMPTY_TUPLE),
        (
            "FlagsAllowUniqueESSymbolType",
            flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE,
        ),
        (
            "FlagsAllowEmptyIndexInfoType",
            flags::ALLOW_EMPTY_INDEX_INFO_TYPE,
        ),
        (
            "FlagsAllowNodeModulesRelativePaths",
            flags::ALLOW_NODE_MODULES_RELATIVE_PATHS,
        ),
        ("FlagsIgnoreErrors", flags::IGNORE_ERRORS),
        ("FlagsInObjectTypeLiteral", flags::IN_OBJECT_TYPE_LITERAL),
        ("FlagsInTypeAlias", flags::IN_TYPE_ALIAS),
        ("FlagsInInitialEntityName", flags::IN_INITIAL_ENTITY_NAME),
    ];
    assert_eq!(ours.len(), expected.len(), "every observed flag has a port");
    for (name, value) in ours {
        assert_eq!(expected[*name].as_u64(), Some(u64::from(*value)), "{name}");
    }
}

#[test]
fn internal_flags_match_the_pinned_go_values() {
    let expected = observed("nodebuilder.InternalFlags");
    let ours: &[(&str, InternalFlags)] = &[
        ("InternalFlagsNone", internal_flags::NONE),
        (
            "InternalFlagsWriteComputedProps",
            internal_flags::WRITE_COMPUTED_PROPS,
        ),
        (
            "InternalFlagsNoSyntacticPrinter",
            internal_flags::NO_SYNTACTIC_PRINTER,
        ),
        (
            "InternalFlagsDoNotIncludeSymbolChain",
            internal_flags::DO_NOT_INCLUDE_SYMBOL_CHAIN,
        ),
        (
            "InternalFlagsAllowUnresolvedNames",
            internal_flags::ALLOW_UNRESOLVED_NAMES,
        ),
    ];
    assert_eq!(ours.len(), expected.len());
    for (name, value) in ours {
        assert_eq!(expected[*name].as_i64(), Some(i64::from(*value)), "{name}");
    }
}
