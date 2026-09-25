// Phase 1 operation tables: the tsoptions bridge.
//
// scripts/phase1_mutation_go.py copies this file into internal/tsoptions of
// the git-archive export the table driver is built from (upstream/ is never
// edited). A bridge exposes an unexported operation that no exported caller
// can enter on a table row (tsoptions/showconfig.go:computeFn runs at package
// init), so that a column can call it inside its column segment. Bridge files
// are outside data/go-functions.tsv, so only the wrapped function counts as
// entered.
package tsoptions
