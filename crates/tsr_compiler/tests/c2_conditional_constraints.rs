//! Conditional constraints preserve lazy enclosing-parameter resolution.
#[path = "support/c2_native_diagnostics.rs"]
mod c2_native_diagnostics;

#[test]
fn infer_in_mapped_constraints_matches_native() {
    c2_native_diagnostics::assert_native_diagnostics(
        include_str!("fixtures/c2/conditional_constraints/infer_mapped.ts"),
        include_str!("fixtures/c2/conditional_constraints/infer_mapped.native.json"),
        "/infer_mapped.ts",
    );
}

#[test]
fn distributive_conditional_base_constraints_match_native() {
    c2_native_diagnostics::assert_native_diagnostics(
        include_str!("fixtures/c2/conditional_constraints/distributive.ts"),
        include_str!("fixtures/c2/conditional_constraints/distributive.native.json"),
        "/distributive.ts",
    );
}

#[test]
fn mapped_key_array_constraints_match_native() {
    c2_native_diagnostics::assert_native_diagnostics(
        include_str!("fixtures/c2/conditional_constraints/mapped_keys.ts"),
        include_str!("fixtures/c2/conditional_constraints/mapped_keys.native.json"),
        "/mapped_keys.ts",
    );
}
