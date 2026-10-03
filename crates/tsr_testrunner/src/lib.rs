//! The pinned test runners over the Rust compiler (docs/EVIDENCE-plan.md,
//! section 4): `internal/testrunner`, `internal/testutil/harnessutil`,
//! `internal/testutil/tsbaseline` and `internal/testutil/baseline`, so that
//! `tsr-testrunner run --id <variant>` does for one test what
//! `go test ./internal/testrunner` does for it: parse the test case, expand
//! its configurations, compile, compose every baseline the pin's runner
//! composes, and compare each with `testdata/baselines/reference`.
//!
//! One process runs one variant and prints one result line per sub-test
//! (`result::ResultLine`); `scripts/parity.py` drives the processes and keeps
//! the expectation file. Nothing here reads a Go observation: every input is
//! the pin's test data, every output is compared with the pin's committed
//! reference.
//!
//! Module map, by upstream file:
//!
//! | Module | Port of |
//! |---|---|
//! | `test_case_parser` | `testrunner/test_case_parser.go` |
//! | `harness_options`, `configurations` | `harnessutil.go` (options, `GetFileBasedTestConfigurations`, `SkipUnsupportedCompilerOptions`) |
//! | `enumerate` | `harnessutil.go:EnumerateFiles`, `compiler_runner.go` (suite, skip list, configured names) |
//! | `compile` | `harnessutil.go` (`CompileFiles`, `compileFilesWithHost`, `createProgram`, `newCompilationResult`, the test lib folder) |
//! | `compiler_runner` | `compiler_runner.go` (`newCompilerTest`, `runSingleConfigTest`, the `verify*` sub-tests) |
//! | `transpile_runner` | `testrunner/transpile_runner.go` |
//! | `baseline` | `testutil/baseline/baseline.go` |
//! | `harness` | the baseline writers and sub-tests ported in Phases 2 and 3, moved in from `tools/` |
#![allow(clippy::missing_panics_doc)]

use std::path::{Path, PathBuf};

pub mod baseline;
pub mod compile;
pub mod compiler_runner;
pub mod configurations;
pub mod enumerate;
pub mod harness;
pub mod harness_options;
pub mod result;
pub mod test_case_parser;
pub mod transpile_runner;

/// Posix-style path to sources under test (`compiler_runner.go:srcFolder`).
pub const SRC_FOLDER: &[u8] = b"/.src";
/// Posix-style path to additional test libraries (`harnessutil.go:testLibFolder`).
pub const TEST_LIB_FOLDER: &[u8] = b"/.lib";

/// The pin's `testdata` directory: `tests/cases/<suite>/**` are the inputs,
/// `baselines/reference/<suite>/` the committed truth, `baselines/local/`
/// where differing outputs are written, `tests/lib/` the `/.lib` fixtures.
#[derive(Clone, Debug)]
pub struct TestData {
    pub root: PathBuf,
}

impl TestData {
    /// `<repository>/upstream/tsc/testdata`.
    pub fn in_repository(repository: &Path) -> Self {
        Self {
            root: repository.join("upstream/tsc/testdata"),
        }
    }

    /// `repo.TestDataPath()`.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// `tests/cases/<suite>`: the test files of a suite.
    pub fn cases(&self, suite: &str) -> PathBuf {
        self.root.join("tests/cases").join(suite)
    }

    /// `baselines/reference`: `baseline.go:referenceRoot`.
    pub fn reference(&self) -> PathBuf {
        self.root.join("baselines/reference")
    }

    /// `tests/lib`: the files copied under `/.lib` (`harnessutil.go:testLibFolderMap`).
    pub fn lib(&self) -> PathBuf {
        self.root.join("tests/lib")
    }
}

/// How the pin's runner ends a test or a sub-test before comparing a
/// baseline: `t.Fatalf` (the test fails with this message) or `t.Skipf` (the
/// test is skipped, not counted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    Fatal(String),
    Skip(String),
}

impl Stop {
    pub fn fatal(message: impl Into<String>) -> Self {
        Self::Fatal(message.into())
    }

    pub fn skip(message: impl Into<String>) -> Self {
        Self::Skip(message.into())
    }
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stop::Fatal(message) => write!(f, "fatal: {message}"),
            Stop::Skip(message) => write!(f, "skip: {message}"),
        }
    }
}

/// The test-program mode: the pin's `TS_TEST_PROGRAM_SINGLE_THREADED`
/// (`testutil.TestProgramIsSingleThreaded`). The pin's default is `Single`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Single,
    Concurrent,
}

impl Mode {
    /// `createProgram`'s `SingleThreaded` tristate: true in single mode,
    /// unknown (the production default) otherwise.
    pub fn single_threaded(self) -> tsr_core::Tristate {
        match self {
            Mode::Single => tsr_core::Tristate::TRUE,
            Mode::Concurrent => tsr_core::Tristate::UNKNOWN,
        }
    }
}
