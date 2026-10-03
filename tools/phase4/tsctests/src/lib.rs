//! Phase 4 X0: the pinned command-line test harness
//! (`tsc/internal/execute/tsctests`) over the recorded scenarios
//! (`data/phase4/scenarios.json.gz`). A repository-only tool: it renders each
//! scenario's baseline as the pin's runner does and records one result row per
//! scenario (`src/main.rs`).
//!
//! One module per pinned file: [`runner`] (`runner.go`), [`sys`] (`sys.go`:
//! the fake system, its clock, the testing hooks and the output sanitizer),
//! [`fs`] (`fs.go`), [`readablebuildinfo`], [`mock_watch_backend`], and from
//! `testutil` [`fsbaselineutil`] (the file-system differ), [`harnessutil`]
//! (the baselining tracer) and [`baseline`] (`DiffText`, the Phase 3 port of
//! the patience diff, included as it is). [`execute`] holds the command-line
//! entry and the pinned interfaces it is called through, which Phase 4 X1
//! supplies; until then the entry refuses with a named operation.
pub mod execute;
pub mod fs;
pub mod fsbaselineutil;
pub mod goutil;
pub mod harnessutil;
pub mod mock_watch_backend;
pub mod program_view;
pub mod readablebuildinfo;
pub mod row;
pub mod runner;
pub mod scenario;
pub mod sys;

#[path = "../../../phase3/harness/patience.rs"]
#[allow(dead_code)]
mod patience;

/// `testutil/baseline`: the pinned `DiffText`.
pub mod baseline {
    pub use crate::patience::diff_text;
}

#[cfg(test)]
mod tests;
