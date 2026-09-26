//! Array inference sites fix later callback parameters, including nested arrays.
#[path = "support/c2_native_diagnostics.rs"]
mod native_diagnostics;

#[test]
fn array_inference_sites_match_native_contextual_types() {
    native_diagnostics::assert_native_diagnostics(
        include_str!("fixtures/c2/array_inference.ts"),
        include_str!("fixtures/c2/array_inference.native.json"),
        "/array_inference.ts",
    );
}
