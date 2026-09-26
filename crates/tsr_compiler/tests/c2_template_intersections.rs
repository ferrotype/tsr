//! B04: template reductions preserve the native diagnostics and order.
#[path = "support/c2_native_diagnostics.rs"]
mod native_diagnostics;

#[test]
fn template_intersections_match_native_reduction_and_display() {
    native_diagnostics::assert_native_diagnostics(
        include_str!("fixtures/c2/template_intersections.ts"),
        include_str!("fixtures/c2/template_intersections.native.json"),
        "/template_intersections.ts",
    );
}
