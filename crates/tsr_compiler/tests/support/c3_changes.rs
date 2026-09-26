//! One native-observed case per documented JavaScript difference of the pin
//! (`upstream/tsc/CHANGES.md`, C3.6), per option combination (C3.7) and per
//! flow, iteration and class family the corpus observes only through errors
//! (C3.8 contracts 2 to 5). Each fixture's native diagnostics were recorded by
//! `fixtures/c3/regenerate.py` with the pinned `tsgo`.
use super::native;

#[test]
fn changes_conflicting_declarations_error_at_every_site() {
    native::assert_fixture(
        "changes_conflicting_declarations.ts",
        include_str!("../fixtures/c3/changes_conflicting_declarations.ts"),
        include_str!("../fixtures/c3/changes_conflicting_declarations.ts.native.json"),
    );
}

#[test]
fn changes_template_inference_keeps_a_code_point() {
    native::assert_fixture(
        "changes_template_code_point.ts",
        include_str!("../fixtures/c3/changes_template_code_point.ts"),
        include_str!("../fixtures/c3/changes_template_code_point.ts.native.json"),
    );
}

#[test]
fn changes_non_strict_js_cannot_omit_unknown_arguments() {
    native::assert_fixture(
        "changes_strict_false_omission.js",
        include_str!("../fixtures/c3/changes_strict_false_omission.js"),
        include_str!("../fixtures/c3/changes_strict_false_omission.js.native.json"),
    );
}

#[test]
fn changes_jsdoc_values_are_not_types() {
    native::assert_fixture(
        "changes_jsdoc_values_as_types.js",
        include_str!("../fixtures/c3/changes_jsdoc_values_as_types.js"),
        include_str!("../fixtures/c3/changes_jsdoc_values_as_types.js.native.json"),
    );
}

#[test]
fn changes_arguments_does_not_imply_rest() {
    native::assert_fixture(
        "changes_arguments_no_rest.js",
        include_str!("../fixtures/c3/changes_arguments_no_rest.js"),
        include_str!("../fixtures/c3/changes_arguments_no_rest.js.native.json"),
    );
}

#[test]
fn changes_variadic_jsdoc_type_is_an_array() {
    native::assert_fixture(
        "changes_variadic_array_synonym.js",
        include_str!("../fixtures/c3/changes_variadic_array_synonym.js"),
        include_str!("../fixtures/c3/changes_variadic_array_synonym.js.native.json"),
    );
}

#[test]
fn changes_variadic_jsdoc_type_is_not_rest() {
    native::assert_fixture(
        "changes_variadic_not_rest.js",
        include_str!("../fixtures/c3/changes_variadic_not_rest.js"),
        include_str!("../fixtures/c3/changes_variadic_not_rest.js.native.json"),
    );
}

#[test]
fn changes_postfix_equals_makes_the_parameter_optional() {
    native::assert_fixture(
        "changes_postfix_equals_optional.js",
        include_str!("../fixtures/c3/changes_postfix_equals_optional.js"),
        include_str!("../fixtures/c3/changes_postfix_equals_optional.js.native.json"),
    );
}

#[test]
fn changes_asserts_on_the_declaring_variable() {
    native::assert_fixture(
        "changes_asserts_on_arrow.js",
        include_str!("../fixtures/c3/changes_asserts_on_arrow.js"),
        include_str!("../fixtures/c3/changes_asserts_on_arrow.js.native.json"),
    );
}

#[test]
fn changes_async_non_promise_uses_the_ts_error() {
    native::assert_fixture(
        "changes_async_non_promise.js",
        include_str!("../fixtures/c3/changes_async_non_promise.js"),
        include_str!("../fixtures/c3/changes_async_non_promise.js.native.json"),
    );
}

#[test]
fn changes_typedef_in_a_class_body_is_hoisted() {
    native::assert_fixture(
        "changes_typedef_in_class_hoisted.js",
        include_str!("../fixtures/c3/changes_typedef_in_class_hoisted.js"),
        include_str!("../fixtures/c3/changes_typedef_in_class_hoisted.js.native.json"),
    );
}

#[test]
fn changes_class_tag_is_not_a_constructor() {
    native::assert_fixture(
        "changes_class_tag_not_constructor.js",
        include_str!("../fixtures/c3/changes_class_tag_not_constructor.js"),
        include_str!("../fixtures/c3/changes_class_tag_not_constructor.js.native.json"),
    );
}

#[test]
fn changes_param_applies_to_one_function() {
    native::assert_fixture(
        "changes_param_one_function.js",
        include_str!("../fixtures/c3/changes_param_one_function.js"),
        include_str!("../fixtures/c3/changes_param_one_function.js.native.json"),
    );
}

#[test]
fn changes_type_assertion_prevents_narrowing() {
    native::assert_fixture(
        "changes_type_assertion_no_narrowing.js",
        include_str!("../fixtures/c3/changes_type_assertion_no_narrowing.js"),
        include_str!("../fixtures/c3/changes_type_assertion_no_narrowing.js.native.json"),
    );
}

#[test]
fn changes_overload_on_arrows() {
    native::assert_fixture(
        "changes_overload_arrow.js",
        include_str!("../fixtures/c3/changes_overload_arrow.js"),
        include_str!("../fixtures/c3/changes_overload_arrow.js.native.json"),
    );
}

#[test]
fn changes_constructor_functions_are_unsupported() {
    native::assert_fixture(
        "changes_constructor_function.js",
        include_str!("../fixtures/c3/changes_constructor_function.js"),
        include_str!("../fixtures/c3/changes_constructor_function.js.native.json"),
    );
}

#[test]
fn changes_void_zero_expando_creates_a_property() {
    native::assert_fixture(
        "changes_void_zero_expando.js",
        include_str!("../fixtures/c3/changes_void_zero_expando.js"),
        include_str!("../fixtures/c3/changes_void_zero_expando.js.native.json"),
    );
}

#[test]
fn changes_this_property_annotation_creates_no_property() {
    native::assert_fixture(
        "changes_this_property_annotation.js",
        include_str!("../fixtures/c3/changes_this_property_annotation.js"),
        include_str!("../fixtures/c3/changes_this_property_annotation.js.native.json"),
    );
}

#[test]
fn changes_mixed_module_exports_assignments() {
    native::assert_fixture(
        "changes_mixed_module_exports.js",
        include_str!("../fixtures/c3/changes_mixed_module_exports.js"),
        include_str!("../fixtures/c3/changes_mixed_module_exports.js.native.json"),
    );
}

#[test]
fn options_exact_optional_property_types() {
    native::assert_fixture(
        "options_exact_optional.ts",
        include_str!("../fixtures/c3/options_exact_optional.ts"),
        include_str!("../fixtures/c3/options_exact_optional.ts.native.json"),
    );
}

#[test]
fn options_no_unchecked_indexed_access() {
    native::assert_fixture(
        "options_no_unchecked_indexed.ts",
        include_str!("../fixtures/c3/options_no_unchecked_indexed.ts"),
        include_str!("../fixtures/c3/options_no_unchecked_indexed.ts.native.json"),
    );
}

#[test]
fn options_use_unknown_in_catch_variables() {
    native::assert_fixture(
        "options_unknown_catch.ts",
        include_str!("../fixtures/c3/options_unknown_catch.ts"),
        include_str!("../fixtures/c3/options_unknown_catch.ts.native.json"),
    );
}

#[test]
fn options_verbatim_module_syntax() {
    native::assert_fixture(
        "options_verbatim_module_syntax.ts",
        include_str!("../fixtures/c3/options_verbatim_module_syntax.ts"),
        include_str!("../fixtures/c3/options_verbatim_module_syntax.ts.native.json"),
    );
}

#[test]
fn options_isolated_modules() {
    native::assert_fixture(
        "options_isolated_modules.ts",
        include_str!("../fixtures/c3/options_isolated_modules.ts"),
        include_str!("../fixtures/c3/options_isolated_modules.ts.native.json"),
    );
}

#[test]
fn options_no_fallthrough_cases_in_switch() {
    native::assert_fixture(
        "options_no_fallthrough.ts",
        include_str!("../fixtures/c3/options_no_fallthrough.ts"),
        include_str!("../fixtures/c3/options_no_fallthrough.ts.native.json"),
    );
}

#[test]
fn contract_2_definite_assignment() {
    native::assert_fixture(
        "flow_definite_assignment.ts",
        include_str!("../fixtures/c3/flow_definite_assignment.ts"),
        include_str!("../fixtures/c3/flow_definite_assignment.ts.native.json"),
    );
}

#[test]
fn contract_3_evolving_arrays_and_discriminants() {
    native::assert_fixture(
        "flow_narrowing.ts",
        include_str!("../fixtures/c3/flow_narrowing.ts"),
        include_str!("../fixtures/c3/flow_narrowing.ts.native.json"),
    );
}

#[test]
fn contract_4_iteration_types_over_the_lib() {
    native::assert_fixture(
        "flow_iteration.ts",
        include_str!("../fixtures/c3/flow_iteration.ts"),
        include_str!("../fixtures/c3/flow_iteration.ts.native.json"),
    );
}

#[test]
fn contract_5_class_checks() {
    native::assert_fixture(
        "flow_classes.ts",
        include_str!("../fixtures/c3/flow_classes.ts"),
        include_str!("../fixtures/c3/flow_classes.ts.native.json"),
    );
}

#[test]
fn lookup_of_a_global_alias_resolves_its_target() {
    native::assert_fixture(
        "lookup_alias_globals.ts",
        include_str!("../fixtures/c3/lookup_alias_globals.ts"),
        include_str!("../fixtures/c3/lookup_alias_globals.ts.native.json"),
    );
}
