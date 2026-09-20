//! The type-creating part of `NewChecker` (`tsc/internal/checker/checker.go`):
//! the intrinsic types, literal seeds, empty object types, marker type
//! parameters, seed signatures and index infos every checker creates in the same
//! order, so their ids are the same in every checker and in Go.
//!
//! What follows them upstream is not here yet: the type mappers (P3), the
//! global-type resolvers, `initializeClosures`, `initializeIterationResolvers`
//! and `initializeChecker` (P2), which merge the program's globals and create the
//! `globalThis` object type and the array types. Until then a Rust checker holds
//! exactly the types created before `initializeClosures`, and the storage oracle
//! compares against a Go checker prepared the same way.

use crate::{
    object_flags, type_flags, CheckerState, Error, IndexInfoId, SignatureId, TypeId, TypePredicate,
    TypePredicateId, TypePredicateKind,
};
use std::collections::HashMap;
use tsr_arena::SymbolId;
use tsr_ast::{check_flags, internal_symbol_names, symbol_flags, JsString, SymbolTableId};
use tsr_jsnum::{Number, PseudoBigInt};

/// The named types, symbols, signatures and index infos `NewChecker` creates.
///
/// Fields are plain ids. Before initialization writes a field it holds a
/// sentinel no store ever issues, so a premature read fails with an invalid
/// slot instead of aliasing another type; upstream would dereference nil.
#[derive(Debug)]
pub(crate) struct Builtins {
    pub undefined_symbol: SymbolId,
    pub arguments_symbol: SymbolId,
    pub require_symbol: SymbolId,
    pub unknown_symbol: SymbolId,
    /// `primitiveTypeAliasSuggestions`: transient type aliases named after the
    /// primitives, suggested when a global builtin object name is misspelled.
    /// Created on first use, outside `SymbolCount`, as upstream's heap symbols are.
    pub primitive_alias_suggestions: Vec<(&'static [u8], SymbolId)>,
    pub global_this_symbol: SymbolId,
    pub globals: Option<SymbolTableId>,
    pub any_type: TypeId,
    pub auto_type: TypeId,
    pub wildcard_type: TypeId,
    pub blocked_string_type: TypeId,
    pub error_type: TypeId,
    pub unresolved_type: TypeId,
    pub non_inferrable_any_type: TypeId,
    pub intrinsic_marker_type: TypeId,
    pub unknown_type: TypeId,
    pub undefined_type: TypeId,
    pub undefined_widening_type: TypeId,
    pub missing_type: TypeId,
    pub undefined_or_missing_type: TypeId,
    pub optional_type: TypeId,
    pub null_type: TypeId,
    pub null_widening_type: TypeId,
    pub string_type: TypeId,
    pub number_type: TypeId,
    pub bigint_type: TypeId,
    pub regular_false_type: TypeId,
    pub false_type: TypeId,
    pub regular_true_type: TypeId,
    pub true_type: TypeId,
    pub boolean_type: TypeId,
    pub es_symbol_type: TypeId,
    pub void_type: TypeId,
    pub never_type: TypeId,
    pub silent_never_type: TypeId,
    pub implicit_never_type: TypeId,
    pub unreachable_never_type: TypeId,
    pub non_primitive_type: TypeId,
    pub string_or_number_type: TypeId,
    pub string_number_symbol_type: TypeId,
    pub number_or_big_int_type: TypeId,
    pub numeric_string_type: TypeId,
    pub template_constraint_type: TypeId,
    pub unique_literal_type: TypeId,
    pub empty_object_type: TypeId,
    pub empty_jsx_object_type: TypeId,
    pub empty_fresh_jsx_object_type: TypeId,
    pub empty_type_literal_type: TypeId,
    pub unknown_empty_object_type: TypeId,
    pub unknown_union_type: TypeId,
    pub empty_generic_type: TypeId,
    pub any_function_type: TypeId,
    pub no_constraint_type: TypeId,
    pub circular_constraint_type: TypeId,
    pub resolving_default_type: TypeId,
    pub marker_super_type: TypeId,
    pub marker_sub_type: TypeId,
    pub marker_other_type: TypeId,
    pub marker_super_type_for_check: TypeId,
    pub marker_sub_type_for_check: TypeId,
    pub no_type_predicate: TypePredicateId,
    pub any_signature: SignatureId,
    pub unknown_signature: SignatureId,
    pub resolving_signature: SignatureId,
    pub silent_never_signature: SignatureId,
    pub enum_number_index_info: IndexInfoId,
    pub any_base_type_index_info: IndexInfoId,
    pub empty_string_type: TypeId,
    pub zero_type: TypeId,
    pub zero_big_int_type: TypeId,
    pub typeof_type: TypeId,
}

/// The `typeof` result strings (`typeofNEFacts` in `flow.go`), in the sorted
/// order `NewChecker` unions them.
pub(crate) const TYPEOF_NAMES: [&[u8]; 8] = [
    b"bigint",
    b"boolean",
    b"function",
    b"number",
    b"object",
    b"string",
    b"symbol",
    b"undefined",
];

impl Builtins {
    /// `checker_arena` is the checker's own symbol arena, so the symbol sentinel
    /// names an unpublished slot of the right owner.
    pub(crate) fn uninitialized(checker_arena: tsr_arena::ArenaId) -> Self {
        let t = TypeId::new(u32::MAX).expect("sentinel");
        let s = SignatureId::new(u32::MAX).expect("sentinel");
        let i = IndexInfoId::new(u32::MAX).expect("sentinel");
        let p = TypePredicateId::new(u32::MAX).expect("sentinel");
        let sym = SymbolId::from_parts(checker_arena, u32::MAX).expect("sentinel symbol");
        Self {
            undefined_symbol: sym,
            arguments_symbol: sym,
            require_symbol: sym,
            unknown_symbol: sym,
            primitive_alias_suggestions: Vec::new(),
            global_this_symbol: sym,
            globals: None,
            any_type: t,
            auto_type: t,
            wildcard_type: t,
            blocked_string_type: t,
            error_type: t,
            unresolved_type: t,
            non_inferrable_any_type: t,
            intrinsic_marker_type: t,
            unknown_type: t,
            undefined_type: t,
            undefined_widening_type: t,
            missing_type: t,
            undefined_or_missing_type: t,
            optional_type: t,
            null_type: t,
            null_widening_type: t,
            string_type: t,
            number_type: t,
            bigint_type: t,
            regular_false_type: t,
            false_type: t,
            regular_true_type: t,
            true_type: t,
            boolean_type: t,
            es_symbol_type: t,
            void_type: t,
            never_type: t,
            silent_never_type: t,
            implicit_never_type: t,
            unreachable_never_type: t,
            non_primitive_type: t,
            string_or_number_type: t,
            string_number_symbol_type: t,
            number_or_big_int_type: t,
            numeric_string_type: t,
            template_constraint_type: t,
            unique_literal_type: t,
            empty_object_type: t,
            empty_jsx_object_type: t,
            empty_fresh_jsx_object_type: t,
            empty_type_literal_type: t,
            unknown_empty_object_type: t,
            unknown_union_type: t,
            empty_generic_type: t,
            any_function_type: t,
            no_constraint_type: t,
            circular_constraint_type: t,
            resolving_default_type: t,
            marker_super_type: t,
            marker_sub_type: t,
            marker_other_type: t,
            marker_super_type_for_check: t,
            marker_sub_type_for_check: t,
            no_type_predicate: p,
            any_signature: s,
            unknown_signature: s,
            resolving_signature: s,
            silent_never_signature: s,
            enum_number_index_info: i,
            any_base_type_index_info: i,
            empty_string_type: t,
            zero_type: t,
            zero_big_int_type: t,
            typeof_type: t,
        }
    }
}

impl Builtins {
    /// A type by its upstream `Checker` field name, for tests and oracles.
    pub(crate) fn type_by_name(&self, name: &str) -> Option<TypeId> {
        Some(match name {
            "anyType" => self.any_type,
            "autoType" => self.auto_type,
            "wildcardType" => self.wildcard_type,
            "blockedStringType" => self.blocked_string_type,
            "errorType" => self.error_type,
            "unresolvedType" => self.unresolved_type,
            "nonInferrableAnyType" => self.non_inferrable_any_type,
            "intrinsicMarkerType" => self.intrinsic_marker_type,
            "unknownType" => self.unknown_type,
            "undefinedType" => self.undefined_type,
            "undefinedWideningType" => self.undefined_widening_type,
            "missingType" => self.missing_type,
            "undefinedOrMissingType" => self.undefined_or_missing_type,
            "optionalType" => self.optional_type,
            "nullType" => self.null_type,
            "nullWideningType" => self.null_widening_type,
            "stringType" => self.string_type,
            "numberType" => self.number_type,
            "bigintType" => self.bigint_type,
            "regularFalseType" => self.regular_false_type,
            "falseType" => self.false_type,
            "regularTrueType" => self.regular_true_type,
            "trueType" => self.true_type,
            "booleanType" => self.boolean_type,
            "esSymbolType" => self.es_symbol_type,
            "voidType" => self.void_type,
            "neverType" => self.never_type,
            "silentNeverType" => self.silent_never_type,
            "implicitNeverType" => self.implicit_never_type,
            "unreachableNeverType" => self.unreachable_never_type,
            "nonPrimitiveType" => self.non_primitive_type,
            "stringOrNumberType" => self.string_or_number_type,
            "stringNumberSymbolType" => self.string_number_symbol_type,
            "numberOrBigIntType" => self.number_or_big_int_type,
            "numericStringType" => self.numeric_string_type,
            "templateConstraintType" => self.template_constraint_type,
            "uniqueLiteralType" => self.unique_literal_type,
            "emptyObjectType" => self.empty_object_type,
            "emptyJsxObjectType" => self.empty_jsx_object_type,
            "emptyFreshJsxObjectType" => self.empty_fresh_jsx_object_type,
            "emptyTypeLiteralType" => self.empty_type_literal_type,
            "unknownEmptyObjectType" => self.unknown_empty_object_type,
            "unknownUnionType" => self.unknown_union_type,
            "emptyGenericType" => self.empty_generic_type,
            "anyFunctionType" => self.any_function_type,
            "noConstraintType" => self.no_constraint_type,
            "circularConstraintType" => self.circular_constraint_type,
            "resolvingDefaultType" => self.resolving_default_type,
            "markerSuperType" => self.marker_super_type,
            "markerSubType" => self.marker_sub_type,
            "markerOtherType" => self.marker_other_type,
            "markerSuperTypeForCheck" => self.marker_super_type_for_check,
            "markerSubTypeForCheck" => self.marker_sub_type_for_check,
            "emptyStringType" => self.empty_string_type,
            "zeroType" => self.zero_type,
            "zeroBigIntType" => self.zero_big_int_type,
            "typeofType" => self.typeof_type,
            _ => return None,
        })
    }
}

/// The `Checker` type fields `NewChecker` creates before `initializeClosures`,
/// in creation order.
pub const BUILTIN_TYPE_NAMES: [&str; 57] = [
    "anyType",
    "autoType",
    "wildcardType",
    "blockedStringType",
    "errorType",
    "unresolvedType",
    "nonInferrableAnyType",
    "intrinsicMarkerType",
    "unknownType",
    "undefinedType",
    "undefinedWideningType",
    "missingType",
    "undefinedOrMissingType",
    "optionalType",
    "nullType",
    "nullWideningType",
    "stringType",
    "numberType",
    "bigintType",
    "regularFalseType",
    "falseType",
    "regularTrueType",
    "trueType",
    "booleanType",
    "esSymbolType",
    "voidType",
    "neverType",
    "silentNeverType",
    "implicitNeverType",
    "unreachableNeverType",
    "nonPrimitiveType",
    "stringOrNumberType",
    "stringNumberSymbolType",
    "numberOrBigIntType",
    "numericStringType",
    "templateConstraintType",
    "uniqueLiteralType",
    "emptyObjectType",
    "emptyJsxObjectType",
    "emptyFreshJsxObjectType",
    "emptyTypeLiteralType",
    "unknownEmptyObjectType",
    "unknownUnionType",
    "emptyGenericType",
    "anyFunctionType",
    "noConstraintType",
    "circularConstraintType",
    "resolvingDefaultType",
    "markerSuperType",
    "markerSubType",
    "markerOtherType",
    "markerSuperTypeForCheck",
    "markerSubTypeForCheck",
    "emptyStringType",
    "zeroType",
    "zeroBigIntType",
    "typeofType",
];

fn name(bytes: &[u8]) -> JsString {
    JsString::from_bytes(bytes)
}

impl CheckerState {
    /// The type-creating prefix of `NewChecker`, in upstream order. Closures,
    /// mappers, global resolvers and `initializeChecker` are pending (module doc).
    // port: tsc/internal/checker/checker.go:NewChecker
    pub(crate) fn initialize(&mut self) -> Result<(), Error> {
        let exact_optional = self.options.exact_optional_property_types;
        let undefined_symbol = self.new_symbol(symbol_flags::PROPERTY, name(b"undefined"))?;
        let arguments_symbol = self.new_symbol(symbol_flags::PROPERTY, name(b"arguments"))?;
        let require_symbol = self.new_symbol(symbol_flags::PROPERTY, name(b"require"))?;
        let unknown_symbol = self.new_symbol(symbol_flags::PROPERTY, name(b"unknown"))?;
        let global_this_symbol = self.new_symbol_ex(
            symbol_flags::MODULE,
            name(b"globalThis"),
            check_flags::READONLY,
        )?;
        let mut globals_table = HashMap::new();
        globals_table.insert(name(b"globalThis"), Some(global_this_symbol));
        let globals = self.alloc_symbol_table(globals_table);
        self.symbol_mut(global_this_symbol)?.exports = Some(globals);
        self.builtins.undefined_symbol = undefined_symbol;
        self.builtins.arguments_symbol = arguments_symbol;
        self.builtins.require_symbol = require_symbol;
        self.builtins.unknown_symbol = unknown_symbol;
        self.builtins.global_this_symbol = global_this_symbol;
        self.builtins.globals = Some(globals);

        self.builtins.any_type = self.new_intrinsic_type(type_flags::ANY, b"any")?;
        self.builtins.auto_type =
            self.new_intrinsic_type_ex(type_flags::ANY, b"any", object_flags::NON_INFERRABLE_TYPE)?;
        self.builtins.wildcard_type = self.new_intrinsic_type(type_flags::ANY, b"any")?;
        self.builtins.blocked_string_type = self.new_intrinsic_type(type_flags::ANY, b"any")?;
        self.builtins.error_type = self.new_intrinsic_type(type_flags::ANY, b"error")?;
        self.builtins.unresolved_type = self.new_intrinsic_type(type_flags::ANY, b"unresolved")?;
        self.builtins.non_inferrable_any_type = self.new_intrinsic_type_ex(
            type_flags::ANY,
            b"any",
            object_flags::CONTAINS_WIDENING_TYPE,
        )?;
        self.builtins.intrinsic_marker_type =
            self.new_intrinsic_type(type_flags::ANY, b"intrinsic")?;
        self.builtins.unknown_type = self.new_intrinsic_type(type_flags::UNKNOWN, b"unknown")?;
        self.builtins.undefined_type =
            self.new_intrinsic_type(type_flags::UNDEFINED, b"undefined")?;
        self.builtins.undefined_widening_type =
            self.create_widening_type(self.builtins.undefined_type)?;
        self.builtins.missing_type =
            self.new_intrinsic_type(type_flags::UNDEFINED, b"undefined")?;
        self.builtins.undefined_or_missing_type = if exact_optional {
            self.builtins.missing_type
        } else {
            self.builtins.undefined_type
        };
        self.builtins.optional_type =
            self.new_intrinsic_type(type_flags::UNDEFINED, b"undefined")?;
        self.builtins.null_type = self.new_intrinsic_type(type_flags::NULL, b"null")?;
        self.builtins.null_widening_type = self.create_widening_type(self.builtins.null_type)?;
        self.builtins.string_type = self.new_intrinsic_type(type_flags::STRING, b"string")?;
        self.builtins.number_type = self.new_intrinsic_type(type_flags::NUMBER, b"number")?;
        self.builtins.bigint_type = self.new_intrinsic_type(type_flags::BIG_INT, b"bigint")?;

        let regular_false = self.new_literal_type(
            type_flags::BOOLEAN_LITERAL,
            crate::LiteralValue::Boolean(false),
            None,
        )?;
        let false_type = self.new_literal_type(
            type_flags::BOOLEAN_LITERAL,
            crate::LiteralValue::Boolean(false),
            Some(regular_false),
        )?;
        self.types.literal_mut(regular_false)?.fresh = Some(false_type);
        self.types.literal_mut(false_type)?.fresh = Some(false_type);
        let regular_true = self.new_literal_type(
            type_flags::BOOLEAN_LITERAL,
            crate::LiteralValue::Boolean(true),
            None,
        )?;
        let true_type = self.new_literal_type(
            type_flags::BOOLEAN_LITERAL,
            crate::LiteralValue::Boolean(true),
            Some(regular_true),
        )?;
        self.types.literal_mut(regular_true)?.fresh = Some(true_type);
        self.types.literal_mut(true_type)?.fresh = Some(true_type);
        self.builtins.regular_false_type = regular_false;
        self.builtins.false_type = false_type;
        self.builtins.regular_true_type = regular_true;
        self.builtins.true_type = true_type;
        self.builtins.boolean_type = self.get_union_type(&[regular_false, regular_true])?;

        self.builtins.es_symbol_type = self.new_intrinsic_type(type_flags::ES_SYMBOL, b"symbol")?;
        self.builtins.void_type = self.new_intrinsic_type(type_flags::VOID, b"void")?;
        self.builtins.never_type = self.new_intrinsic_type(type_flags::NEVER, b"never")?;
        self.builtins.silent_never_type = self.new_intrinsic_type_ex(
            type_flags::NEVER,
            b"never",
            object_flags::NON_INFERRABLE_TYPE,
        )?;
        self.builtins.implicit_never_type = self.new_intrinsic_type(type_flags::NEVER, b"never")?;
        self.builtins.unreachable_never_type =
            self.new_intrinsic_type(type_flags::NEVER, b"never")?;
        self.builtins.non_primitive_type =
            self.new_intrinsic_type(type_flags::NON_PRIMITIVE, b"object")?;
        let (string, number, bigint, boolean, null, undefined, es_symbol) = (
            self.builtins.string_type,
            self.builtins.number_type,
            self.builtins.bigint_type,
            self.builtins.boolean_type,
            self.builtins.null_type,
            self.builtins.undefined_type,
            self.builtins.es_symbol_type,
        );
        self.builtins.string_or_number_type = self.get_union_type(&[string, number])?;
        self.builtins.string_number_symbol_type =
            self.get_union_type(&[string, number, es_symbol])?;
        self.builtins.number_or_big_int_type = self.get_union_type(&[number, bigint])?;
        // The `${number}` type.
        self.builtins.numeric_string_type =
            self.get_template_literal_type(&[name(b""), name(b"")], &[number])?;
        self.builtins.template_constraint_type =
            self.get_union_type(&[string, number, boolean, bigint, null, undefined])?;
        // Special `never` flagged by union reduction to behave as a literal.
        self.builtins.unique_literal_type = self.new_intrinsic_type(type_flags::NEVER, b"never")?;
        // uniqueLiteralMapper, reportUnreliableMapper, reportUnmeasurableMapper,
        // restrictiveMapper and permissiveMapper arrive with instantiation (P3).

        self.builtins.empty_object_type = self.new_anonymous_type(None, None, &[], &[], &[])?;
        self.builtins.empty_jsx_object_type = self.new_anonymous_type(None, None, &[], &[], &[])?;
        self.builtins.empty_fresh_jsx_object_type =
            self.new_anonymous_type(None, None, &[], &[], &[])?;
        let type_literal_symbol = self.new_symbol(
            symbol_flags::TYPE_LITERAL,
            name(internal_symbol_names::TYPE),
        )?;
        self.builtins.empty_type_literal_type =
            self.new_anonymous_type(Some(type_literal_symbol), None, &[], &[], &[])?;
        self.builtins.unknown_empty_object_type =
            self.new_anonymous_type(None, None, &[], &[], &[])?;
        self.builtins.unknown_union_type = self.create_unknown_union_type()?;
        self.builtins.empty_generic_type = self.new_anonymous_type(None, None, &[], &[], &[])?;
        self.types
            .object_mut(self.builtins.empty_generic_type)?
            .instantiations = Some(Box::default());
        self.builtins.any_function_type = self.new_anonymous_type(None, None, &[], &[], &[])?;
        self.types
            .get_mut(self.builtins.any_function_type)?
            .object_flags |= object_flags::NON_INFERRABLE_TYPE;
        self.builtins.no_constraint_type = self.new_anonymous_type(None, None, &[], &[], &[])?;
        self.builtins.circular_constraint_type =
            self.new_anonymous_type(None, None, &[], &[], &[])?;
        self.builtins.resolving_default_type =
            self.new_anonymous_type(None, None, &[], &[], &[])?;

        self.builtins.marker_super_type = self.new_type_parameter(None)?;
        self.builtins.marker_sub_type = self.new_type_parameter(None)?;
        self.types
            .type_parameter_mut(self.builtins.marker_sub_type)?
            .constraint = Some(self.builtins.marker_super_type);
        self.builtins.marker_other_type = self.new_type_parameter(None)?;
        self.builtins.marker_super_type_for_check = self.new_type_parameter(None)?;
        self.builtins.marker_sub_type_for_check = self.new_type_parameter(None)?;
        self.types
            .type_parameter_mut(self.builtins.marker_sub_type_for_check)?
            .constraint = Some(self.builtins.marker_super_type_for_check);

        let any = self.builtins.any_type;
        self.builtins.no_type_predicate = self.signatures.new_type_predicate(TypePredicate {
            kind: TypePredicateKind::Identifier,
            parameter_index: 0,
            parameter_name: name(b"<<unresolved>>"),
            t: Some(any),
        })?;
        let (error, silent_never) = (self.builtins.error_type, self.builtins.silent_never_type);
        self.builtins.any_signature =
            self.signatures
                .new_signature(0, None, None, None, None, Some(any), None, 0)?;
        self.builtins.unknown_signature =
            self.signatures
                .new_signature(0, None, None, None, None, Some(error), None, 0)?;
        self.builtins.resolving_signature =
            self.signatures
                .new_signature(0, None, None, None, None, Some(any), None, 0)?;
        self.builtins.silent_never_signature = self.signatures.new_signature(
            0,
            None,
            None,
            None,
            None,
            Some(silent_never),
            None,
            0,
        )?;
        self.builtins.enum_number_index_info = self
            .signatures
            .new_index_info(number, string, true, None, None)?;
        self.builtins.any_base_type_index_info = self
            .signatures
            .new_index_info(string, any, false, None, None)?;
        self.builtins.empty_string_type = self.get_string_literal_type(name(b""))?;
        self.builtins.zero_type = self.get_number_literal_type(Number::new(0.0))?;
        self.builtins.zero_big_int_type = self.get_big_int_literal_type(PseudoBigInt::default())?;
        let mut typeof_types = Vec::with_capacity(TYPEOF_NAMES.len());
        for text in TYPEOF_NAMES {
            typeof_types.push(self.get_string_literal_type(name(text))?);
        }
        self.builtins.typeof_type = self.get_union_type(&typeof_types)?;
        Ok(())
    }
}

/// Builtin object names paired with the primitive alias suggested in their place.
pub(crate) const PRIMITIVE_ALIAS_SUGGESTIONS: [(&[u8], &[u8]); 6] = [
    (b"String", b"string"),
    (b"Number", b"number"),
    (b"Boolean", b"boolean"),
    (b"Object", b"object"),
    (b"BigInt", b"bigint"),
    (b"Symbol", b"symbol"),
];
