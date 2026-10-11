# ADR 0003: Memory, WebAssembly and embedding are all required

Status: Accepted (2026-09-05)
Plan: sections 2, 5 and 9

## Context

The rewrite's case rests on three benefits Go cannot give this project: lower memory and no collector pauses, a WebAssembly build, and in-process embedding by Rust tools. The owner declared all three primary.

## Decision

The spike must pass bounded experiments for all three (E5 to E8) before the rest of the plan starts. Parser-level measurements do not establish whole-checker performance or whole-compiler WebAssembly support, so the exit criteria for cut-over additionally require: a WebAssembly library that runs the compiler, conformance and transpile corpus through an in-memory host and matches the oracle's baselines without native processes or filesystem access; a separate Rust consumer that links the embedding crate and runs the same comparisons in process, including ownership, malformed-input, cancellation and panic-invalidation tests; and size, latency and retained-memory budgets fixed for named workloads before Phase 7 and met for four consecutive weekly runs. Neither deliverable may be deferred past cut-over.

## Amendment, owner decision 2026-10-11

Weekly performance runs and the four-consecutive-run requirement above are
postponed indefinitely. They are not active closure requirements while deferred;
resumption requires an explicit owner request. The budgets, comparable-work
and measurement-validity rules, required deliverables, and the separate
four-week dogfood requirement remain unchanged. See Phase 7 plan decision 4.

## Consequences

Phase 7 includes full checking and emit through both entry points. `tsr_wasm` and `tsr_embed` exist as crates from the spike onward, with parser and checker-slice entry points first.

## Evidence

Plan section 5, exit criteria; section 9, Phase 0 experiments and Phase 7.
