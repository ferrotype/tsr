//! Instantiation expressions are transparent to syntactic nullishness.
#[path = "support/c2_native_diagnostics.rs"]
mod native;

#[test]
fn nullish_instantiation_wrappers_match_native_diagnostics() {
    native::assert_native_diagnostics(
        include_str!("fixtures/c2/nullish_instantiation.ts"),
        include_str!("fixtures/c2/nullish_instantiation.native.json"),
        "/nullish_instantiation.ts",
    );
}
