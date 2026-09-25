// Phase 1 operation tables: group concurrency (core concurrency, request context, BFS).
// Columns are registered in init; the Rust side is
// tools/phase1/mutation/driver/src/table/concurrency.rs and the spec is
// data/phase1/tables/concurrency.json.
package main

func init() {
	Register("concurrency")
}
