//! Each recorded operation through its Rust entry point. Arguments follow the
//! pinned Go signature; results come back as `Actual`s in the order the Go
//! function returns them.
use super::values::{
    bool_arg, int_arg, opt, signatures, str_arg, string, symbols, types, Actual, Outcome, Replay,
};
use serde_json::{json, Value};
use tsr_checker::{
    BuilderRequest, Operation, SignatureKind, SymbolRef, UnionReduction, VerbosityContext,
};
use tsr_printer::emit_resolver::{
    ConstantValue, DeclarationEmitResolver, SymbolAccessibilityResult,
};

fn verbosity(value: &Value) -> Result<Option<VerbosityContext>, Outcome> {
    if value.is_null() {
        return Ok(None);
    }
    let v = &value["verbosity"];
    Ok(Some(VerbosityContext {
        level: int_arg(&v["level"])? as i32,
        max_truncation_length: int_arg(&v["max_truncation_length"])? as usize,
        can_increase_verbosity: bool_arg(&v["can_increase"])?,
        truncated: bool_arg(&v["truncated"])?,
    }))
}

fn described_verbosity(v: &VerbosityContext) -> Value {
    json!({"verbosity": {
        "level": v.level,
        "max_truncation_length": v.max_truncation_length,
        "can_increase": v.can_increase_verbosity,
        "truncated": v.truncated,
    }})
}

fn accessibility(replay: &Replay, result: &SymbolAccessibilityResult) -> Actual {
    Actual::Fields(vec![
        (
            "struct",
            Actual::Json(json!("printer.SymbolAccessibilityResult")),
        ),
        (
            "fields",
            Actual::Fields(vec![
                ("Accessibility", Actual::Int(result.accessibility as i64)),
                (
                    "AliasesToMakeVisible",
                    Actual::List(
                        result
                            .aliases_to_make_visible
                            .iter()
                            .map(|&n| Actual::Node(n))
                            .collect(),
                    ),
                ),
                (
                    "ErrorSymbolName",
                    string(result.error_symbol_name.as_bytes()),
                ),
                ("ErrorNode", opt(result.error_node, Actual::Node)),
                (
                    "ErrorModuleName",
                    string(result.error_module_name.as_bytes()),
                ),
            ]),
        ),
    ])
    .with_program(replay)
}

fn constant(value: Option<&ConstantValue>) -> Actual {
    match value {
        None => Actual::Null,
        Some(ConstantValue::String(text)) => string(text.as_bytes()),
        Some(ConstantValue::Number(number)) => Actual::Number(number.to_string()),
    }
}

fn diagnostics(replay: &Replay, list: &[tsr_ast::Diagnostic]) -> Actual {
    Actual::List(
        list.iter()
            .map(|d| Actual::Json(diagnostic(replay, d)))
            .collect(),
    )
}

fn diagnostic(replay: &Replay, d: &tsr_ast::Diagnostic) -> Value {
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    let mut result = serde_json::Map::new();
    result.insert("code".into(), json!(d.code));
    result.insert("category".into(), json!(d.category));
    result.insert("key".into(), json!(text(d.message_key.as_bytes())));
    result.insert(
        "arguments".into(),
        if d.message_args.is_empty() {
            Value::Null
        } else {
            json!(d
                .message_args
                .iter()
                .map(|a| text(a.as_bytes()))
                .collect::<Vec<_>>())
        },
    );
    result.insert("pos".into(), json!(d.loc.pos()));
    result.insert("end".into(), json!(d.loc.end()));
    if !d.message_text.is_empty() {
        result.insert("message".into(), json!(text(d.message_text.as_bytes())));
    }
    if let Some(file) = d.file {
        if let Some(name) = replay.program.describe(file)["node"]["file"].as_str() {
            result.insert("file".into(), json!(name));
        }
    }
    if !d.message_chain.is_empty() {
        result.insert(
            "chain".into(),
            json!(d
                .message_chain
                .iter()
                .map(|c| diagnostic(replay, c))
                .collect::<Vec<_>>()),
        );
    }
    if !d.related_information.is_empty() {
        result.insert(
            "related".into(),
            json!(d
                .related_information
                .iter()
                .map(|c| diagnostic(replay, c))
                .collect::<Vec<_>>()),
        );
    }
    Value::Object(result)
}

trait WithProgram {
    fn with_program(self, replay: &Replay) -> Self;
}
impl WithProgram for Actual {
    fn with_program(self, _replay: &Replay) -> Self {
        self
    }
}

/// Replays one call and compares its results with the recorded ones.
pub fn replay(replay: &mut Replay, op: &mut Operation<'_>, name: &str, event: &Value) -> Outcome {
    let Some(results) = event.get("results").and_then(Value::as_array) else {
        return Outcome::Unsupported("the pin panicked".to_string());
    };
    let empty = Vec::new();
    let args = event["args"].as_array().unwrap_or(&empty);
    let mut after = None;
    let actual = match call(replay, op, name, args, &mut after) {
        Ok(actual) => actual,
        Err(outcome) => return outcome,
    };
    if actual.len() != results.len() {
        return Outcome::Mismatch(format!(
            "go {} results rust {}",
            results.len(),
            actual.len()
        ));
    }
    for (i, (recorded, actual)) in results.iter().zip(&actual).enumerate() {
        if let Err(reason) = replay.compare(op, recorded, actual, &format!("result[{i}]")) {
            return Outcome::Mismatch(reason);
        }
    }
    if let Some(after) = after {
        let recorded = event["args_after"].get(0).cloned().unwrap_or(Value::Null);
        if recorded != after {
            return Outcome::Mismatch(format!("verbosity after: go {recorded} rust {after}"));
        }
    }
    Outcome::Match
}

fn arg(args: &[Value], i: usize) -> &Value {
    args.get(i).unwrap_or(&Value::Null)
}

#[allow(clippy::too_many_lines)]
fn call(
    r: &mut Replay,
    op: &mut Operation<'_>,
    name: &str,
    args: &[Value],
    after: &mut Option<Value>,
) -> Result<Vec<Actual>, Outcome> {
    let a = |i: usize| arg(args, i);
    macro_rules! ty {
        ($i:expr) => {
            r.type_arg(op, a($i))?
        };
    }
    macro_rules! sym {
        ($i:expr) => {
            r.symbol_arg(op, a($i))?
        };
    }
    macro_rules! node {
        ($i:expr) => {
            r.node_arg(a($i))?
        };
    }
    macro_rules! opt_node {
        ($i:expr) => {
            r.opt_node_arg(a($i))?
        };
    }
    macro_rules! sig {
        ($i:expr) => {
            r.signature_arg(a($i))?
        };
    }
    let one = |actual: Actual| Ok(vec![actual]);
    match name {
        "Checker.IsArgumentsSymbol" => {
            let s = sym!(0);
            one(Actual::Bool(op.is_arguments_symbol(s)?))
        }
        "Checker.IsUndefinedSymbol" => {
            let s = sym!(0);
            one(Actual::Bool(op.is_undefined_symbol(s)?))
        }
        "Checker.IsUnknownSymbol" => {
            let s = sym!(0);
            one(Actual::Bool(op.is_unknown_symbol(s)?))
        }
        "SkipAlias" | "Checker.SkipAlias" => {
            let s = sym!(0);
            one(Actual::Symbol(op.skip_alias(s)?))
        }
        "Checker.IsDeprecatedDeclaration" => {
            let n = node!(0);
            one(Actual::Bool(op.is_deprecated_declaration(n)?))
        }
        "Checker.GetRootSymbols" => {
            let s = sym!(0);
            one(symbols(op.get_root_symbols(s)?))
        }
        "Checker.GetSymbolAtLocation" => {
            let n = node!(0);
            one(opt(op.get_symbol_at_location(n)?, Actual::Symbol))
        }
        "Checker.GetShorthandAssignmentValueSymbol" => {
            let n = opt_node!(0);
            one(opt(
                op.get_shorthand_assignment_value_symbol(n)?,
                Actual::Symbol,
            ))
        }
        "Checker.GetTypeAtLocation" => {
            let n = node!(0);
            one(Actual::Type(op.get_type_at_location(n)?))
        }
        "Checker.GetPropertyOfType" => {
            let t = ty!(0);
            let name = str_arg(a(1))?;
            one(opt(
                op.get_property_of_type(t, name.as_bytes())?,
                Actual::Symbol,
            ))
        }
        "Checker.WasCanceled" => one(Actual::Bool(op.was_canceled())),
        "Checker.GetGlobalDiagnostics" => {
            let list = op.get_global_diagnostics()?;
            one(diagnostics(r, &list))
        }
        "Checker.GetDiagnostics" | "Checker.GetSuggestionDiagnostics" => {
            if a(0)["context"]["canceled"].as_bool() == Some(true) {
                return Err(Outcome::Unsupported("canceled request".to_string()));
            }
            let file = node!(1);
            let list = if name == "Checker.GetDiagnostics" {
                op.get_diagnostics(file)?
            } else {
                op.get_suggestion_diagnostics(file)?
            };
            one(diagnostics(r, &list))
        }
        "Checker.GetContextualType" => {
            let n = node!(0);
            let flags = int_arg(a(1))? as u32;
            one(opt(op.get_contextual_type(n, flags)?, Actual::Type))
        }
        "Checker.GetTypeOfSymbolAtLocation" => {
            let s = sym!(0);
            let n = opt_node!(1);
            one(Actual::Type(op.get_type_of_symbol_at_location(s, n)?))
        }
        "Checker.IsValidPropertyAccessForCompletions" => {
            let n = node!(0);
            let t = ty!(1);
            let s = sym!(2);
            one(Actual::Bool(
                op.is_valid_property_access_for_completions(n, t, s)?,
            ))
        }
        "Checker.IsValidPropertyAccess" => {
            let n = node!(0);
            let name = str_arg(a(1))?;
            one(Actual::Bool(
                op.is_valid_property_access(n, name.as_bytes())?,
            ))
        }
        "Checker.GetEmitResolver" | "NewNodeBuilder" | "NewNodeBuilderEx" => one(Actual::Object),
        "Checker.SymbolToStringEx" => {
            let s = sym!(0);
            let n = opt_node!(1);
            let meaning = int_arg(a(2))? as u32;
            let flags = int_arg(a(3))? as u32;
            one(string(
                op.symbol_to_string_at(s, n, meaning, flags)?.as_bytes(),
            ))
        }
        "Checker.SymbolToString" => {
            let s = sym!(0);
            one(string(op.symbol_to_string(s)?.as_bytes()))
        }
        "Checker.TypeToString" => {
            let t = ty!(0);
            one(string(op.type_to_string_default(t)?.as_bytes()))
        }
        "Checker.TypeToStringEx" => {
            let t = ty!(0);
            let n = opt_node!(1);
            let flags = int_arg(a(2))? as u32;
            let mut v = verbosity(a(3))?;
            let text = op.type_to_string_ex(t, n, flags, v.as_mut())?;
            *after = v.as_ref().map(described_verbosity);
            one(string(text.as_bytes()))
        }
        "Checker.SignatureToStringEx" => {
            let s = sig!(0);
            let n = opt_node!(1);
            let flags = int_arg(a(2))? as u32;
            let mut v = verbosity(a(3))?;
            let text = op.signature_to_string_ex(s, n, flags, v.as_mut())?;
            *after = v.as_ref().map(described_verbosity);
            one(string(text.as_bytes()))
        }
        "Checker.TypeParameterToStringEx" => {
            let t = ty!(0);
            let n = opt_node!(1);
            let mut v = verbosity(a(2))?;
            let text = op.type_parameter_to_string_ex(t, n, v.as_mut())?;
            *after = v.as_ref().map(described_verbosity);
            one(string(text.as_bytes()))
        }
        "Checker.ExpandSymbolForHover" => {
            let s = sym!(0);
            let meaning = int_arg(a(1))? as u32;
            let mut v = verbosity(a(2))?;
            let text = op.expand_symbol_for_hover(s, meaning, v.as_mut())?;
            *after = v.as_ref().map(described_verbosity);
            one(string(text.as_bytes()))
        }
        "Checker.TypePredicateToString" => {
            let p = r.predicate_arg(a(0))?;
            one(string(op.type_predicate_to_string(p)?.as_bytes()))
        }
        "Checker.GetCallSignatures" => {
            let t = ty!(0);
            one(signatures(op.get_call_signatures(t)?))
        }
        "Checker.GetSignaturesOfType" => {
            let t = ty!(0);
            let kind = if int_arg(a(1))? == 0 {
                SignatureKind::Call
            } else {
                SignatureKind::Construct
            };
            one(signatures(op.get_signatures_of_type(t, kind)?))
        }
        "Checker.GetNonNullableType" => {
            let t = ty!(0);
            one(Actual::Type(op.get_non_nullable_type(t)?))
        }
        "Checker.GetSymbolsInScope" => {
            let n = node!(0);
            let meaning = int_arg(a(1))? as u32;
            one(Actual::Unordered(op.get_symbols_in_scope(n, meaning)?))
        }
        "Checker.GetExportsOfModule" => {
            let s = sym!(0);
            one(Actual::Unordered(op.get_exports_of_module(s)?))
        }
        "Checker.GetExportsAndPropertiesOfModule" => {
            let s = sym!(0);
            one(Actual::Unordered(
                op.get_exports_and_properties_of_module(s)?,
            ))
        }
        "Checker.GetAllPossiblePropertiesOfTypes" => {
            let list = r.types_arg(op, a(0))?;
            one(Actual::Unordered(
                op.get_all_possible_properties_of_types(&list)?,
            ))
        }
        "Checker.GetAmbientModules" => one(Actual::Unordered(op.get_ambient_modules()?)),
        "Checker.GetDeclaredTypeOfSymbol" => {
            let s = sym!(0);
            one(Actual::Type(op.get_declared_type_of_symbol(s)?))
        }
        "Checker.IsEmptyAnonymousObjectType" => {
            let t = ty!(0);
            one(Actual::Bool(op.is_empty_anonymous_object_type(t)?))
        }
        "Checker.TryGetThisTypeAtEx" => {
            let n = node!(0);
            let include = bool_arg(a(1))?;
            let container = opt_node!(2);
            one(opt(
                op.try_get_this_type_at_ex(n, include, container)?,
                Actual::Type,
            ))
        }
        "Checker.GetMergedSymbol" => {
            if a(0).is_null() {
                return one(Actual::Null);
            }
            let s = sym!(0);
            one(Actual::Symbol(op.get_merged_symbol(s)?))
        }
        "Checker.GetReturnTypeOfSignature" => {
            let s = sig!(0);
            one(Actual::Type(op.get_return_type_of_signature(s)?))
        }
        "Checker.GetTypePredicateOfSignature" => {
            let s = sig!(0);
            one(opt(
                op.get_type_predicate_of_signature(s)?,
                Actual::Predicate,
            ))
        }
        "Checker.GetPropertySymbolsFromContextualType" => {
            let n = node!(0);
            let t = ty!(1);
            let ok = bool_arg(a(2))?;
            one(symbols(
                op.get_property_symbols_from_contextual_type(n, t, ok)?,
            ))
        }
        "Checker.GetApparentProperties" => {
            let t = ty!(0);
            one(symbols(op.get_apparent_properties(t)?))
        }
        "Checker.HasEffectiveRestParameter" => {
            let s = sig!(0);
            one(Actual::Bool(op.has_effective_rest_parameter(s)?))
        }
        "Checker.GetExpandedParameters" => {
            let s = sig!(0);
            let skip = bool_arg(a(1))?;
            let lists = op.get_expanded_parameters(s, skip)?;
            one(Actual::List(lists.into_iter().map(symbols).collect()))
        }
        "Checker.GetStringIndexType" => {
            let t = ty!(0);
            one(opt(op.get_string_index_type(t)?, Actual::Type))
        }
        "Checker.GetNumberIndexType" => {
            let t = ty!(0);
            one(opt(op.get_number_index_type(t)?, Actual::Type))
        }
        "Checker.GetResolvedSignature" => {
            let n = node!(0);
            one(Actual::Signature(op.get_resolved_signature(n)?))
        }
        "Checker.GetBaseTypes" => {
            let t = ty!(0);
            one(types(op.get_base_types(t)?))
        }
        "GetResolvedSignatureForSignatureHelp" => {
            let n = node!(0);
            let count = int_arg(a(1))? as usize;
            let (signature, candidates) = op.get_resolved_signature_for_signature_help(n, count)?;
            Ok(vec![
                opt(signature, Actual::Signature),
                signatures(candidates),
            ])
        }
        "Checker.TryFindAmbientModule" => {
            let name = str_arg(a(0))?;
            one(opt(
                op.try_find_ambient_module(name.as_bytes())?,
                Actual::Symbol,
            ))
        }
        "Checker.ResolveName" => {
            let name = str_arg(a(0))?;
            let n = opt_node!(1);
            let meaning = int_arg(a(2))? as u32;
            let exclude = bool_arg(a(3))?;
            one(opt(
                op.resolve_name(name.as_bytes(), n, meaning, exclude)?,
                Actual::Symbol,
            ))
        }
        "Checker.IsDeclarationUsed" => {
            let file = node!(0);
            let identifier = node!(1);
            let jsx = bool_arg(a(2))?;
            let explicit = bool_arg(a(3))?;
            one(Actual::Bool(
                op.is_declaration_used(file, identifier, jsx, explicit)?,
            ))
        }
        "Checker.GetPropertySymbolOfDestructuringAssignment" => {
            let n = node!(0);
            one(opt(
                op.get_property_symbol_of_destructuring_assignment(n)?,
                Actual::Symbol,
            ))
        }
        "Checker.GetPropertiesOfType" => {
            let t = ty!(0);
            one(symbols(op.get_properties_of_type(t)?))
        }
        "Checker.GetPromisedTypeOfPromise" => {
            let t = ty!(0);
            one(opt(op.get_promised_type_of_promise(t)?, Actual::Type))
        }
        "Checker.GetSignatureFromDeclaration" => {
            let n = node!(0);
            one(Actual::Signature(op.get_signature_from_declaration(n)?))
        }
        "Checker.GetNonOptionalType" => {
            let t = ty!(0);
            one(Actual::Type(op.get_non_optional_type(t)?))
        }
        "Checker.IsNullableType" => {
            let t = ty!(0);
            one(Actual::Bool(op.is_nullable_type(t)?))
        }
        "Checker.GetTypeOfSymbol" => {
            let s = sym!(0);
            one(Actual::Type(op.get_type_of_symbol(s)?))
        }
        "Checker.GetStringType" => one(Actual::Type(op.get_string_type())),
        "Checker.GetNumberType" => one(Actual::Type(op.get_number_type())),
        "Checker.GetUnionType" => {
            let list = r.types_arg(op, a(0))?;
            one(Actual::Type(op.get_union_type(&list)?))
        }
        "Checker.GetUnionTypeEx" => {
            let list = r.types_arg(op, a(0))?;
            let reduction = match int_arg(a(1))? {
                0 => UnionReduction::None,
                1 => UnionReduction::Literal,
                _ => UnionReduction::Subtype,
            };
            one(Actual::Type(op.get_union_type_ex(&list, reduction)?))
        }
        "Checker.GetWidenedType" => {
            let t = ty!(0);
            one(Actual::Type(op.get_widened_type(t)?))
        }
        "Checker.GetWidenedLiteralType" => {
            let t = ty!(0);
            one(Actual::Type(op.get_widened_literal_type(t)?))
        }
        "Checker.IsLibTypeForHoverVerbosity" => {
            let t = ty!(0);
            one(Actual::Bool(op.is_lib_type_for_hover_verbosity(t)?))
        }
        "Checker.GetImmediateAliasedSymbol" => {
            let s = sym!(0);
            one(opt(op.get_immediate_aliased_symbol(s)?, Actual::Symbol))
        }
        "Checker.GetAliasedSymbol" => {
            let s = sym!(0);
            one(Actual::Symbol(op.get_aliased_symbol(s)?))
        }
        "Checker.ResolveAlias" => {
            let s = sym!(0);
            let (resolved, ok) = op.resolve_alias(s)?;
            Ok(vec![Actual::Symbol(resolved), Actual::Bool(ok)])
        }
        "Checker.GetTypeArgumentConstraint" => {
            let n = node!(0);
            one(opt(op.get_type_argument_constraint(n)?, Actual::Type))
        }
        "Checker.GetExportSpecifierLocalTargetSymbol" => {
            let n = node!(0);
            one(opt(
                op.get_export_specifier_local_target_symbol(n)?,
                Actual::Symbol,
            ))
        }
        "Checker.GetGlobalSymbol" => {
            if !a(2).is_null() {
                return Err(Outcome::Unsupported(
                    "a diagnostic message argument".to_string(),
                ));
            }
            let name = str_arg(a(0))?;
            let meaning = int_arg(a(1))? as u32;
            one(opt(
                op.get_global_symbol(name.as_bytes(), meaning, None)?,
                Actual::Symbol,
            ))
        }
        "Checker.RemoveMissingOrUndefinedType" => {
            let t = ty!(0);
            one(Actual::Type(op.remove_missing_or_undefined_type(t)?))
        }
        "Checker.GetIndexInfoOfType" => {
            let t = ty!(0);
            let key = ty!(1);
            one(opt(op.get_index_info_of_type(t, key)?, Actual::Index))
        }
        "Checker.IsTypeAssignableTo" => {
            let s = ty!(0);
            let t = ty!(1);
            one(Actual::Bool(op.is_type_assignable_to(s, t)?))
        }
        "Checker.GetConstraintOfTypeParameter" => {
            let t = ty!(0);
            one(opt(op.get_constraint_of_type_parameter(t)?, Actual::Type))
        }
        "Checker.GetDefaultFromTypeParameter" => {
            let t = ty!(0);
            one(opt(op.get_default_from_type_parameter(t)?, Actual::Type))
        }
        "Checker.GetSymbolFlags" => {
            let s = sym!(0);
            one(Actual::Int(i64::from(op.get_symbol_flags(s)?)))
        }
        "Checker.GetBaseConstructorTypeOfClass" => {
            let t = ty!(0);
            one(Actual::Type(op.get_base_constructor_type_of_class(t)?))
        }
        "Checker.GetApparentType" => {
            let t = ty!(0);
            one(Actual::Type(op.get_apparent_type(t)?))
        }
        "Checker.IsArrayLikeType" => {
            let t = ty!(0);
            one(Actual::Bool(op.is_array_like_type(t)?))
        }
        "Checker.GetTypeOnlyAliasDeclaration" => {
            let s = sym!(0);
            one(opt(op.get_type_only_alias_declaration(s)?, Actual::Node))
        }
        "Checker.IsTypeInvalidDueToUnionDiscriminant" => {
            let t = ty!(0);
            let n = node!(1);
            one(Actual::Bool(
                op.is_type_invalid_due_to_union_discriminant(t, n)?,
            ))
        }
        "Checker.TypeHasCallOrConstructSignatures" => {
            let t = ty!(0);
            one(Actual::Bool(op.type_has_call_or_construct_signatures(t)?))
        }
        "Checker.GetAccessibleSymbolChain" => {
            let s = sym!(0);
            let n = opt_node!(1);
            let meaning = int_arg(a(2))? as u32;
            let external = bool_arg(a(3))?;
            one(symbols(
                op.get_accessible_symbol_chain(s, n, meaning, external)?,
            ))
        }
        "Checker.IsSymbolAccessible" => {
            let s = r.opt_symbol_arg(op, a(0))?;
            let n = opt_node!(1);
            let meaning = int_arg(a(2))? as u32;
            let compute = bool_arg(a(3))?;
            let result = op.is_symbol_accessible(s, n, meaning, compute)?;
            one(accessibility(r, &result))
        }
        "Checker.GetTypeParameterAtPosition" => {
            let s = sig!(0);
            let pos = int_arg(a(1))? as usize;
            one(Actual::Type(op.get_type_parameter_at_position(s, pos)?))
        }
        "Checker.GetTypeAliasTypeParameters" => {
            let s = sym!(0);
            one(types(op.get_type_alias_type_parameters(s)?))
        }
        "Checker.GetLocalTypeParametersOfClassOrInterfaceOrTypeAlias" => {
            let s = sym!(0);
            one(types(
                op.get_local_type_parameters_of_class_or_interface_or_type_alias(s)?,
            ))
        }
        "Checker.GetMemberOverrideModifierStatus" => {
            let class = node!(0);
            let member = node!(1);
            let s = r.opt_symbol_arg(op, a(2))?;
            one(Actual::Int(
                op.get_member_override_modifier_status(class, member, s)? as i64,
            ))
        }
        "Checker.GetUnknownSymbol" => one(Actual::Symbol(op.get_unknown_symbol()?)),
        "Checker.GetExportSymbolOfSymbol" => {
            let s = sym!(0);
            one(Actual::Symbol(op.get_export_symbol_of_symbol(s)?))
        }
        "Checker.GetConstantValue" => {
            let n = node!(0);
            one(constant(op.constant_value(n)?.as_ref()))
        }
        "Checker.GetIndexSignaturesAtLocation" => {
            let n = node!(0);
            one(Actual::List(
                op.get_index_signatures_at_location(n)?
                    .into_iter()
                    .map(Actual::Node)
                    .collect(),
            ))
        }
        "Checker.GetSymbolsOfParameterPropertyDeclaration" => {
            let n = node!(0);
            let name = str_arg(a(1))?;
            let (parameter, property) =
                op.get_symbols_of_parameter_property_declaration(n, name.as_bytes())?;
            Ok(vec![Actual::Symbol(parameter), Actual::Symbol(property)])
        }
        "Checker.GetFirstTypeArgumentFromKnownType" => {
            let t = ty!(0);
            one(opt(
                op.get_first_type_argument_from_known_type(t)?,
                Actual::Type,
            ))
        }
        "Checker.IsPropertyAccessible" => {
            let n = node!(0);
            let is_super = bool_arg(a(1))?;
            let is_write = bool_arg(a(2))?;
            let t = ty!(3);
            let s = sym!(4);
            one(Actual::Bool(
                op.is_property_accessible(n, is_super, is_write, t, s)?,
            ))
        }
        "Checker.GetJsxIntrinsicTagNamesAt" => {
            let n = node!(0);
            one(symbols(op.get_jsx_intrinsic_tag_names_at(n)?))
        }
        "Checker.GetCandidateSignaturesForStringLiteralCompletions" => {
            let call = node!(0);
            let editing = node!(1);
            one(signatures(
                op.get_candidate_signatures_for_string_literal_completions(call, editing)?,
            ))
        }
        "Checker.GetTypeFromTypeNode" => {
            let n = node!(0);
            one(Actual::Type(op.get_type_from_type_node(n)?))
        }
        "Checker.GetTypeArguments" => {
            let t = ty!(0);
            one(types(op.get_type_arguments(t)?))
        }
        "Checker.GetTypeOfPropertyOfContextualType" => {
            let t = ty!(0);
            let name = str_arg(a(1))?;
            one(opt(
                op.get_type_of_property_of_contextual_type(t, name.as_bytes())?,
                Actual::Type,
            ))
        }
        "Checker.GetTypeOfPropertyOfType" => {
            let t = ty!(0);
            let name = str_arg(a(1))?;
            one(opt(
                op.get_type_of_property_of_type(t, name.as_bytes())?,
                Actual::Type,
            ))
        }
        "Checker.GetJsxNamespace" => {
            let n = opt_node!(0);
            one(string(op.get_jsx_namespace(n)?.as_bytes()))
        }
        "Checker.GetContextualTypeForArgumentAtIndex" => {
            let n = node!(0);
            let index = int_arg(a(1))? as usize;
            one(opt(
                op.get_contextual_type_for_argument_at_index(n, index)?,
                Actual::Type,
            ))
        }
        "Checker.GetContextualTypeForObjectLiteralElement" => {
            let n = node!(0);
            let flags = int_arg(a(1))? as u32;
            one(opt(
                op.get_contextual_type_for_object_literal_element(n, flags)?,
                Actual::Type,
            ))
        }
        "Checker.GetContextualTypeForArrayLiteralAtPosition" => {
            let t = r.opt_type_arg(op, a(0))?;
            let n = node!(1);
            let position = int_arg(a(2))?;
            one(opt(
                op.get_contextual_type_for_array_literal_at_position(t, n, position)?,
                Actual::Type,
            ))
        }
        "Checker.FillMissingTypeArguments" => {
            let arguments = r.types_arg(op, a(0))?;
            let parameters = r.types_arg(op, a(1))?;
            let min = int_arg(a(2))? as usize;
            let js = bool_arg(a(3))?;
            one(types(op.fill_missing_type_arguments(
                &arguments,
                &parameters,
                min,
                js,
            )?))
        }
        "Checker.ResolveExternalModuleName" => {
            let n = node!(0);
            let attributes = r.opt_type_arg(op, a(1))?;
            one(opt(
                op.resolve_external_module_name(n, attributes)?,
                Actual::Symbol,
            ))
        }
        "Checker.RequiresAddingImplicitUndefined" => {
            let n = node!(0);
            one(Actual::Bool(op.requires_adding_implicit_undefined(n)?))
        }
        "Checker.TryGetMemberInModuleExportsAndProperties" => {
            let name = str_arg(a(0))?;
            let s = sym!(1);
            one(opt(
                op.try_get_member_in_module_exports_and_properties(name.as_bytes(), s)?,
                Actual::Symbol,
            ))
        }
        "Checker.GetResolvedSymbol" => {
            let n = node!(0);
            one(Actual::Symbol(op.get_resolved_symbol(n)?))
        }
        "Checker.GetNameTypeOfSymbol" => {
            let s = sym!(0);
            one(opt(op.get_name_type_of_symbol(s)?, Actual::Type))
        }
        "Checker.GetElementTypeOfArrayType" => {
            let t = ty!(0);
            one(opt(op.get_element_type_of_array_type(t)?, Actual::Type))
        }
        "Checker.GetMappedTypeSymbolOfProperty" => {
            let s = sym!(0);
            one(opt(
                op.get_mapped_type_symbol_of_property(s)?,
                Actual::Symbol,
            ))
        }
        "Checker.TypeToTypeNode" | "Checker.TypeToTypeNodeEx" => {
            let t = ty!(0);
            let n = opt_node!(1);
            let flags = int_arg(a(2))? as u32;
            let internal = if name == "Checker.TypeToTypeNodeEx" {
                int_arg(a(3))? as i32
            } else {
                0
            };
            let mut builder = op.node_builder();
            let result = builder.type_to_type_node(t, n, flags, internal)?;
            one(opt(result, |id| {
                Actual::Tree(super::values::tree(builder.view(), id))
            }))
        }
        "NodeBuilder.TypeToTypeNode" => {
            let t = ty!(0);
            let request = builder_request(r, a(1), a(2), a(3))?;
            let mut builder = op.node_builder();
            let result = builder.type_to_type_node(
                t,
                request.enclosing,
                request.flags,
                request.internal_flags,
            )?;
            one(opt(result, |id| {
                Actual::Tree(super::values::tree(builder.view(), id))
            }))
        }
        "NodeBuilder.SymbolToNode" => {
            let s = sym!(0);
            let meaning = int_arg(a(1))? as u32;
            let request = builder_request(r, a(2), a(3), a(4))?;
            let mut builder = op.node_builder();
            let result = builder.symbol_to_node(s, meaning, request)?;
            one(opt(result, |id| {
                Actual::Tree(super::values::tree(builder.view(), id))
            }))
        }
        "NodeBuilder.SignatureToSignatureDeclaration" => {
            let s = sig!(0);
            let kind = kind_arg(a(1))?;
            let request = builder_request(r, a(2), a(3), a(4))?;
            let mut builder = op.node_builder();
            let result = builder.signature_to_signature_declaration(s, kind, request)?;
            one(opt(result, |id| {
                Actual::Tree(super::values::tree(builder.view(), id))
            }))
        }
        "NodeBuilder.TypeParameterToDeclaration" => {
            let t = ty!(0);
            let request = builder_request(r, a(1), a(2), a(3))?;
            let mut builder = op.node_builder();
            let result = builder.type_parameter_to_declaration(t, request)?;
            one(opt(result, |id| {
                Actual::Tree(super::values::tree(builder.view(), id))
            }))
        }
        "NodeBuilder.IndexInfoToIndexSignatureDeclaration" => {
            let info = r.index_info_arg(a(0))?;
            let request = builder_request(r, a(1), a(2), a(3))?;
            let mut builder = op.node_builder();
            let result = builder.index_info_to_index_signature_declaration(info, request)?;
            one(opt(result, |id| {
                Actual::Tree(super::values::tree(builder.view(), id))
            }))
        }
        "Checker.TypePredicateToTypePredicateNode" => {
            let p = r.predicate_arg(a(0))?;
            let request = builder_request(r, a(1), a(2), &Value::Null)?;
            let mut builder = op.node_builder();
            let result = builder.type_predicate_to_type_predicate_node(p, request)?;
            one(opt(result, |id| {
                Actual::Tree(super::values::tree(builder.view(), id))
            }))
        }
        "EmitResolver.IsDeclarationVisible" => {
            let n = node!(0);
            one(Actual::Bool(op.is_declaration_visible(n)?))
        }
        "EmitResolver.GetEffectiveDeclarationFlags" => {
            let n = node!(0);
            let flags = int_arg(a(1))? as u32;
            one(Actual::Int(i64::from(
                op.effective_declaration_flags(n, flags)?,
            )))
        }
        "EmitResolver.IsLiteralConstDeclaration" => {
            let n = node!(0);
            one(Actual::Bool(op.literal_const_declaration(n)?))
        }
        "EmitResolver.IsImplementationOfOverload" => {
            let n = node!(0);
            one(Actual::Bool(op.implementation_of_overload(n)?))
        }
        "EmitResolver.IsOptionalParameter" => {
            let n = node!(0);
            one(Actual::Bool(op.optional_parameter(n)?))
        }
        "EmitResolver.IsExpandoFunctionDeclaration" => {
            let n = node!(0);
            one(Actual::Bool(op.expando_function_declaration(n)?))
        }
        "EmitResolver.IsExpandoFunctionDeclarationUnsafe" => {
            let n = node!(0);
            one(Actual::Bool(op.expando_function_declaration_unsafe(n)?))
        }
        "EmitResolver.PrecalculateDeclarationEmitVisibility" => {
            let file = node!(0);
            op.precalculate_declaration_emit_visibility(file)?;
            Ok(Vec::new())
        }
        "EmitResolver.IsEntityNameVisible" => {
            let n = node!(0);
            let enclosing = node!(1);
            let result = op.entity_name_visible(n, enclosing)?;
            one(accessibility(r, &result))
        }
        "EmitResolver.GetReferencedValueDeclaration" => {
            let n = node!(0);
            one(opt(op.referenced_value_declaration(n)?, Actual::Node))
        }
        "EmitResolver.RequiresAddingImplicitUndefined" => {
            let n = node!(0);
            let s = r.opt_symbol_arg(op, a(1))?.map(SymbolRef::id);
            let enclosing = opt_node!(2);
            one(Actual::Bool(
                DeclarationEmitResolver::requires_adding_implicit_undefined(op, n, s, enclosing)?,
            ))
        }
        "EmitResolver.IsNameResolvable" => {
            let n = node!(0);
            let name = str_arg(a(1))?;
            one(Actual::Bool(op.name_resolvable(n, name.as_bytes())?))
        }
        "EmitResolver.IsImportRequiredByAugmentation" => {
            let n = node!(0);
            one(Actual::Bool(op.import_required_by_augmentation(n)?))
        }
        "EmitResolver.GetPropertiesOfContainerFunction" => {
            let n = node!(0);
            let list = op.properties_of_container_function(n)?;
            let list = list
                .into_iter()
                .map(|id| op.symbol_ref(id))
                .collect::<Result<Vec<_>, _>>()?;
            one(symbols(list))
        }
        "EmitResolver.GetElementAccessExpressionName" => {
            let n = node!(0);
            one(string(op.element_access_expression_name(n)?.as_bytes()))
        }
        "EmitResolver.GetEnumMemberValue" => {
            let n = node!(0);
            let value = op.enum_member_value(n)?;
            one(Actual::Fields(vec![
                ("struct", Actual::Json(json!("evaluator.Result"))),
                (
                    "fields",
                    Actual::Fields(vec![
                        ("Value", constant(value.value.as_ref())),
                        (
                            "IsSyntacticallyString",
                            Actual::Bool(value.is_syntactically_string),
                        ),
                        (
                            "ResolvedOtherFiles",
                            Actual::Bool(value.resolved_other_files),
                        ),
                        (
                            "HasExternalReferences",
                            Actual::Bool(value.has_external_references),
                        ),
                    ]),
                ),
            ]))
        }
        "EmitResolver.CreateTypeOfDeclaration"
        | "EmitResolver.CreateReturnTypeOfSignatureDeclaration"
        | "EmitResolver.CreateTypeOfExpression"
        | "EmitResolver.CreateLateBoundIndexSignatures" => {
            let n = node!(1);
            let enclosing = r
                .opt_node_arg(a(2))?
                .ok_or_else(|| Outcome::Unsupported("no enclosing declaration".to_string()))?;
            let flags = int_arg(a(3))? as u32;
            let internal = int_arg(a(4))? as i32;
            let (mut output, mut emit) = declaration_output(r, op, n)?;
            let mut tracker = ReplayTracker;
            let result = match name {
                "EmitResolver.CreateTypeOfDeclaration" => op.create_type_of_declaration(
                    &mut output,
                    &mut emit,
                    n,
                    enclosing,
                    flags,
                    internal,
                    &mut tracker,
                )?,
                "EmitResolver.CreateReturnTypeOfSignatureDeclaration" => op
                    .create_return_type_of_signature(
                        &mut output,
                        &mut emit,
                        n,
                        enclosing,
                        flags,
                        internal,
                        &mut tracker,
                    )?,
                "EmitResolver.CreateTypeOfExpression" => op.create_type_of_expression(
                    &mut output,
                    &mut emit,
                    n,
                    enclosing,
                    flags,
                    internal,
                    &mut tracker,
                )?,
                _ => {
                    let nodes = op.create_late_bound_index_signatures(
                        &mut output,
                        &mut emit,
                        n,
                        enclosing,
                        flags,
                        internal,
                        &mut tracker,
                    )?;
                    let list = Actual::List(
                        nodes
                            .into_iter()
                            .map(|id| Actual::Tree(super::values::tree(output.view(), id)))
                            .collect(),
                    );
                    return one(list);
                }
            };
            one(opt(result, |id| {
                Actual::Tree(super::values::tree(output.view(), id))
            }))
        }
        "EmitResolver.CreateLiteralConstValue" => {
            let n = node!(1);
            let (mut output, mut emit) = declaration_output(r, op, n)?;
            let mut tracker = ReplayTracker;
            let result = op.create_literal_const_value(&mut output, &mut emit, n, &mut tracker)?;
            one(opt(result, |id| {
                Actual::Tree(super::values::tree(output.view(), id))
            }))
        }
        other => Err(Outcome::Unsupported(format!("operation {other}"))),
    }
}

/// The declaration transformer's output tree over `node`'s source file.
fn declaration_output(
    r: &Replay,
    op: &Operation<'_>,
    node: tsr_arena::NodeId,
) -> Result<(tsr_ast::AstBuilder, tsr_printer::EmitContext), Outcome> {
    let source = r
        .program
        .source_of(node)
        .ok_or_else(|| Outcome::Unsupported("declaration outside the program".to_string()))?;
    let counters = tsr_arena::Counters::new();
    let mut output = tsr_ast::AstBuilder::new(
        tsr_jsstring::SourceText::from_loaded_bytes(Vec::new()),
        &counters,
    );
    op.retain_source(source, &mut output)?;
    Ok((output, tsr_printer::EmitContext::new()))
}

/// The declarations transformer's tracker as far as the checker can see it:
/// an inaccessible symbol is reported, which stops the builder's use of it.
struct ReplayTracker;

impl tsr_printer::emit_resolver::DeclarationSymbolTracker for ReplayTracker {
    fn track_symbol_without_accessibility(
        &mut self,
        _symbol: tsr_arena::SymbolId,
        flags: u32,
    ) -> bool {
        flags & tsr_ast::symbol_flags::TYPE_PARAMETER != 0
    }
    fn track_symbol(
        &mut self,
        _symbol: tsr_arena::SymbolId,
        _enclosing: Option<tsr_arena::NodeId>,
        _meaning: u32,
        result: SymbolAccessibilityResult,
    ) -> bool {
        use tsr_printer::emit_resolver::SymbolAccessibility as A;
        matches!(result.accessibility, A::NotAccessible | A::CannotBeNamed)
    }
    fn report(&mut self, _event: tsr_printer::emit_resolver::DeclarationTrackerEvent) {}
}

fn builder_request(
    r: &Replay,
    enclosing: &Value,
    flags: &Value,
    internal: &Value,
) -> Result<BuilderRequest, Outcome> {
    Ok(BuilderRequest {
        enclosing: r.opt_node_arg(enclosing)?,
        flags: int_arg(flags)? as u32,
        internal_flags: if internal.is_null() {
            0
        } else {
            int_arg(internal)? as i32
        },
    })
}

fn kind_arg(value: &Value) -> Result<tsr_ast::SyntaxKind, Outcome> {
    let raw = int_arg(value)?;
    i16::try_from(raw)
        .ok()
        .and_then(|raw| tsr_ast::NodeKind::from_raw(raw).known())
        .ok_or_else(|| Outcome::Unsupported(format!("kind {raw}")))
}
