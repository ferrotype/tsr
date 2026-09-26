//! B16: only bound names in parameter binding patterns produce TS1230.
//! Property names and initializer references remain missing parameters (TS1225).
#[path = "support/c2_native_diagnostics.rs"]
mod c2_native_diagnostics;

#[test]
fn binding_pattern_type_predicates_match_native_diagnostics() {
    // The complete inventory distinguishes bound names from property/initializer
    // names, skips omissions, and preserves normal and rest parameter behavior.
    c2_native_diagnostics::assert_native_diagnostics(
        include_str!("fixtures/c2/type_predicates.ts"),
        include_str!("fixtures/c2/type_predicates.native.json"),
        "/type_predicates.ts",
    );
}
