// Phase 1 operation tables: group modules (modules, imports, type-only, augmentations, symbol names).
// Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/modules.rs and the spec is
// data/phase1/tables/modules.json.
package main

func init() {
	Register("modules")
}
