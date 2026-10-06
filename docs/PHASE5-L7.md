# L7 implementation record

Work started on `codex/phase5-l7` after L6 merged. The owner approved the
acceptance rules in [the plan](PHASE5-L7-plan.md#2-decisions-for-the-owner) on
2026-10-06. No new individual difference is approved by this record.

## Harness bring-up

The carried overlay builds the pinned fourslash test package without replacing
any test case or expected baseline. It shares the transport with the L2 client
checks, uses the retained private Rust server, and renders project state through
the original native writer. The new supervisor consumes actual `test2json`
events, restarts a crashed worker, bounds active-test deadlines, and emits
explicit test/subtest/baseline rows. `parity.py` supports these batch suites
without changing the compiler/transpile/tsc execution paths.

Local checks performed during bring-up:

- `TestAutoCloseFragment` and `TestAutoCloseTag` pass in one retained Rust worker.
- `TestNavigationBarJsDoc`, `TestHoverOptionalMembers` and
  `TestContentMapperCompletions` pass against Go and Rust.
- `TestImplementationsAcrossProjects` passes against Go and fails against Rust:
  the actual state baseline differs in project/config retention after a
  cross-project search. The projection is not normalized to hide it.
- The hover and completion controls `TestQuickCatchInfo` and
  `TestCompletionListAfterFunction` pass. Injecting one wrong response makes
  their unchanged Go assertions fail. Corrupting a navigation baseline also
  fails. Making the baseline output directory a regular file produces an
  explicit baseline write-error row, including when actual bytes match.
- A native package failure writing its tracking file, after all individual
  tests passed, was rejected as a harness failure. The runner creates the
  tracking directory before execution.

Commands and ownership are in [the harness README](../tools/phase5/harness/README.md).
Raw bring-up output is disposable development output under
`target/phase5/harness/`; it is not an acceptance archive or an approval registry.
The completed 222-test seam/feature sample has 209 native-executed tests and
13 native skips. Rust passes 153 and fails 56 of those 209 parents; no native
reference failed. The 137 failing result rows count only once per parent.
The slowest parent took 0.4 seconds locally in the release build. These are
functional bring-up timings, not latency evidence. There is no full-suite
acceptance claim yet. CI routing follows measured runtime checks.

A twenty-test mixed batch agrees with twenty single runs on both runtimes,
including exact baseline bytes and nested parallel tests. All 13 routed LSP
client tests pass against both runtimes; their initial expectation file is
recorded. This does not include replay or the direct internal-test ports.

Two full-run attempts exposed supervisor handling gaps: very long ordinary
assertion lines, and `test2json` synthesizing a named terminal failure before
EOF on a process panic. Both were corrected and regression-tested against the
saved event streams. Incomplete attempts have no final result metadata and are
not accepted. An underlying empty-directory callback from root import-path
completion is the first production fix identified by those runs.

## Replay bring-up

Three self-contained fixtures exercise references across projects, check-JS and
a small monorepo. Sessions run ordinary stdio LSP, retain raw frames and actual
sent requests, and compare both UTF encodings. Root/correlation normalization
is named explicitly; diagnostic ordering is preserved per document and arrays
are never sorted. Supported error-only logging prevents timing logs at source.
Malformed frames, duplicate JSON keys, unexpected responses and booleans in
place of numeric protocol values fail the harness.

Repeated native captures agree for each fixture. The initial check-JS Rust
comparison differs in pushed config diagnostics: native counts `[32,32]`, Rust
`[22,32,0,0]`. The first Rust publication lacks ten global-type diagnostics, and
validation-off transitions publish extra empties. These are open production
findings, not accepted normalization. See the [replay README](../tools/phase5/replay/README.md).

## Still to complete

The planned seam batches, native full roster execution, exact failing sets,
residual fixes, direct-test routing, CI jobs and latency workload remain in
progress. The source roster is not the executed denominator; implicit Go file
constraints and actual runtime skips must be reported before computing it.
