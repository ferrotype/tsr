# ADR 0024: Profiling compatibility is out of scope

Status: Accepted (2026-10-11)
Plan: Phase 7 owner decision 2, architecture item 7, R5 item 5

## Context

The pinned compiler exposes `--pprofDir` and API/LSP profiling operations.
The Rust entry points currently return explicit unavailable/not-implemented
responses. The pin's clients accept those refusals. Producing compatible
pprof CPU/heap profiles would require a separate profiling implementation and
a dependency decision under [ADR 0017](0017-dependency-policy.md).

The repository already documents native sampling with `samply` and macOS
`sample` in [the profiling record](../PERF-profiling-2026-10.md#method),
including the API server's binary protocol constraint. Native profiling drives
attribution work without implementing the pinned pprof service.

## Decision

The owner approved declaring `tsc/internal/pprof/pprof.go` compatibility out of
scope on 2026-10-11. Keep the existing explicit unavailable/refusal behavior
for `--pprofDir` and API/LSP profiling operations. Do not add a profiler
dependency or claim pprof-format compatibility. Document and use native
profiling tools for development attribution.

In `PORTS.toml`, mark the file's editable `status` as `out-of-scope`, with the
owner decision and this ADR as its reason. Retain the generated source `kind`,
pin, source hash and function inventory; an owner scope decision does not move
the upstream pin or require ledger regeneration.

## Consequences

This closes the profiling scope choice for Phase 7 R5. It does not waive
performance budgets, workload equivalence, source/executable identity or
failure/ownership requirements. Existing native profiling captures keep their
original qualifications; a sampled development profile is not an acceptance
measurement. Other refusal paths still require their own reviewed decisions.

## Evidence

- CLI: `crates/tsr_execute/src/command.rs:profiling_warning`.
- API: `crates/tsr_api/src/session/checker.rs`, the profiling-method dispatch.
- LSP: `crates/tsr_lsp/src/runtime.rs:unimplemented_method` and
  `crates/tsrust/src/lsp.rs`, the `pprofDir` refusal.
- Native recipes and protocol caveat:
  [PERF-profiling-2026-10.md](../PERF-profiling-2026-10.md#method).

No runtime responses are changed by this decision record.
