// Phase 1 operation tables: group diagnostics (AST diagnostics).
// Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/diagnostics.rs and the spec is
// data/phase1/tables/diagnostics.json.
package main

func init() {
	Register("diagnostics")
}
