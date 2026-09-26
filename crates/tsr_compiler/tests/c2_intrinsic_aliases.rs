//! Intrinsic aliases validate syntax names and arities, including user modules.
#[path = "support/c2_native_diagnostics.rs"]
mod native_diagnostics;

#[test]
fn intrinsic_aliases_match_native_names_and_arities() {
    native_diagnostics::assert_native_diagnostics(
        include_str!("fixtures/c2/intrinsic_aliases.ts"),
        include_str!("fixtures/c2/intrinsic_aliases.native.json"),
        "/intrinsic_aliases.ts",
    );
}
