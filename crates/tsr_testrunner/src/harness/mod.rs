//! The baseline writers and runner sub-tests ported in Phases 2 and 3,
//! moved in from `tools/s08/p5`, `tools/phase3/harness` and
//! `tools/phase2` (docs/EVIDENCE-plan.md, section 4). They take Rust
//! programs and files, never a Go observation:
//!
//! | Module | Port of |
//! |---|---|
//! | `baselines` (with `patience`, `sourcemap_record`) | `tsbaseline/js_emit_baseline.go`, `sourcemap_baseline.go`, `sourcemap_record_baseline.go`, `harnessutil.go:newCompilationResult`, `baseline.go:DiffText` |
//! | `typebaseline` | `tsbaseline/type_symbol_baseline.go` |
//! | `errors` | `tsbaseline/error_baseline.go`, `contentmapper_baseline.go` |
//! | `paths` | `tsbaseline/util.go:removeTestPathPrefixes` |
//! | `subtests` | `compiler_runner.go:verifyUnionOrdering`, `verifyParentPointers`, `harnessutil.go:TracerForBaselining` |
//! | `incremental` | `harnessutil.go:createProgram` and the test build-info reader |
//! | `program_view` | the writers' `ProgramView` over a loaded program |
//! | `transpile` | `transpile_runner.go:runKind`, `appendTranspileSection` |
//! | `config_host` | the `ParseConfigHost` over the in-memory file system |
pub mod baselines;
pub mod config_host;
pub mod errors;
pub mod incremental;
pub mod paths;
pub mod program_view;
pub mod subtests;
pub mod transpile;
pub mod typebaseline;
