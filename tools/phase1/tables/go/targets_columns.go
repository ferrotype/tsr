// Phase 1 operation tables: group targets (callee targets, JSX tags, JSDoc deprecation, member helpers).
// Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/targets.rs and the spec is
// data/phase1/tables/targets.json.
package main

func init() {
	Register("targets")
}
