# ADR 0023: Expectation files computed by CI replace recorded evidence

Status: Accepted (2026-10-03)
Plan: [docs/EVIDENCE-plan.md](../EVIDENCE-plan.md)
Supersedes: ADR 0018 (tracking: evidence-driven ledger, producers, experiments, sprints and the dashboard)
Amends: ADR 0004 (where approved divergences are recorded)

## Context

ADR 0018's model recorded evidence: a person ran a producer on a chosen host,
the run's artifact was committed with its source fingerprint, and sprints and
experiments were evaluated against the recorded metrics. In practice the
fingerprints covered the whole tree, so every commit made every artifact
stale; the status views reported 0 files verified and 26 of 29 runs stale
while the compiler matched 13,426 of 13,432 corpus variants, and the only
way to a green view was a day of re-running everything on a quiet host. The
corpus itself was never in CI. The review that led here is section 1 of the
plan.

## Decision

1. **Suite parity is an expectation file.** For each suite the pin's test
   runner defines — `compiler` (single-threaded), `compiler-concurrent`,
   `transpile`, `tsc`, later `fourslash` and `lsp` — `status/parity/<suite>.json`
   names the pin, the number of variants and every failing sub-test with a
   reason. CI runs the suite on every pull request with a Rust port of the
   pin's runner (`crates/tsr_testrunner`, `tools/phase4/tsctests`) against
   the pin's committed `testdata/baselines/reference`, and fails when the
   observed failing set differs from the file in either direction. Progress
   is the diff of the file. No Go runs at CI time and nothing is replayed.
2. **Approved divergences are entries of those files.** A failing entry with
   an `approved` field carries the owner's words; the owner approves by
   merging the pull request that adds the field. `data/divergences.toml` and
   the per-phase approval registers are retired as their entries are carried
   over.
3. **Performance is a dispatch-only measurement**, written to
   `status/perf/<workload>/<run>.json` with the host named, never a
   pull-request gate.
4. **Nothing rendered is committed.** `cargo xtask status` renders the parity
   files, the perf runs and the port coverage from the markers; CI publishes
   the render. The ledger keeps `status`, `rust` and the provenance fields;
   its `verify` field and the derived `verified` state are gone.
5. **Checkpoints close by merged pull requests** that shrink an expectation
   file; sprint files, experiment criteria and recorded metrics are gone.

## Consequences

`status/evidence/`, `status/runs.toml`, `status/experiments.toml`,
`status/history.jsonl`, the committed views, `sprints/`, `docs/TRACKING.md`,
the `cargo xtask run`/`check`/`check-metrics` commands and the per-phase
producer, comparison and register scripts are deleted (plan, section 8).
`cargo xtask validate` still enforces ledger provenance and marker validity.
The pin's baselines are the only truth the suites compare against; a
difference the pin's runner would not see (a Go-only observation) is not a
suite failure.

## Evidence

The first expectation-file run (PR #84): 13,432 compiler variants, 119,602
sub-tests passing, 13 failing, in under two minutes on an 18-CPU host and in
four shards of a few minutes each on GitHub's runners.
