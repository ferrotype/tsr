//! Cross-product limits preserve reporting locations, result types and recovery.
#[path = "c2_native_diagnostics.rs"]
mod native_diagnostics;

#[test]
fn template_limits_match_native_limits_and_recovery() {
    native_diagnostics::assert_native_diagnostics(
        include_str!("../fixtures/c2/template_limits.ts"),
        include_str!("../fixtures/c2/template_limits.native.json"),
        "/template_limits.ts",
    );
}
