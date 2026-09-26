//! One native-observed case per documented JavaScript difference of the pin
//! (`upstream/tsc/CHANGES.md`, C3.6), per option combination (C3.7) and per
//! flow, iteration and class family the corpus observes only through errors
//! (C3.8 contracts 2 to 5). Each fixture's native diagnostics were recorded by
//! `fixtures/c3/regenerate.py` with the pinned `tsgo`.
use super::native;

macro_rules! fixture_case {
    ($test:ident, $file:literal) => {
        #[test]
        fn $test() {
            native::assert_fixture(
                $file,
                include_str!(concat!("../fixtures/c3/", $file)),
                include_str!(concat!("../fixtures/c3/", $file, ".native.json")),
            );
        }
    };
}

fixture_case!(
    changes_conflicting_declarations_error_at_every_site,
    "changes_conflicting_declarations.ts"
);
fixture_case!(
    changes_template_inference_keeps_a_code_point,
    "changes_template_code_point.ts"
);
fixture_case!(
    changes_non_strict_js_cannot_omit_unknown_arguments,
    "changes_strict_false_omission.js"
);
fixture_case!(
    changes_jsdoc_values_are_not_types,
    "changes_jsdoc_values_as_types.js"
);
fixture_case!(
    changes_arguments_does_not_imply_rest,
    "changes_arguments_no_rest.js"
);
fixture_case!(
    changes_variadic_jsdoc_type_is_an_array,
    "changes_variadic_array_synonym.js"
);
fixture_case!(
    changes_variadic_jsdoc_type_is_not_rest,
    "changes_variadic_not_rest.js"
);
fixture_case!(
    changes_postfix_equals_makes_the_parameter_optional,
    "changes_postfix_equals_optional.js"
);
fixture_case!(
    changes_asserts_on_the_declaring_variable,
    "changes_asserts_on_arrow.js"
);
fixture_case!(
    changes_async_non_promise_uses_the_ts_error,
    "changes_async_non_promise.js"
);
fixture_case!(
    changes_typedef_in_a_class_body_is_hoisted,
    "changes_typedef_in_class_hoisted.js"
);
fixture_case!(
    changes_class_tag_is_not_a_constructor,
    "changes_class_tag_not_constructor.js"
);
fixture_case!(
    changes_param_applies_to_one_function,
    "changes_param_one_function.js"
);
fixture_case!(
    changes_type_assertion_prevents_narrowing,
    "changes_type_assertion_no_narrowing.js"
);
fixture_case!(changes_overload_on_arrows, "changes_overload_arrow.js");
fixture_case!(
    changes_constructor_functions_are_unsupported,
    "changes_constructor_function.js"
);
fixture_case!(
    changes_void_zero_expando_creates_a_property,
    "changes_void_zero_expando.js"
);
fixture_case!(
    changes_this_property_annotation_creates_no_property,
    "changes_this_property_annotation.js"
);
fixture_case!(
    changes_mixed_module_exports_assignments,
    "changes_mixed_module_exports.js"
);
fixture_case!(
    options_exact_optional_property_types,
    "options_exact_optional.ts"
);
fixture_case!(
    options_no_unchecked_indexed_access,
    "options_no_unchecked_indexed.ts"
);
fixture_case!(
    options_use_unknown_in_catch_variables,
    "options_unknown_catch.ts"
);
fixture_case!(
    options_verbatim_module_syntax,
    "options_verbatim_module_syntax.ts"
);
fixture_case!(options_isolated_modules, "options_isolated_modules.ts");
fixture_case!(
    options_no_fallthrough_cases_in_switch,
    "options_no_fallthrough.ts"
);
fixture_case!(
    contract_2_definite_assignment,
    "flow_definite_assignment.ts"
);
fixture_case!(
    contract_3_evolving_arrays_and_discriminants,
    "flow_narrowing.ts"
);
fixture_case!(contract_4_iteration_types_over_the_lib, "flow_iteration.ts");
fixture_case!(contract_5_class_checks, "flow_classes.ts");
fixture_case!(
    lookup_of_a_global_alias_resolves_its_target,
    "lookup_alias_globals.ts"
);
