# ADR 0003: Memory, WebAssembly and embedding are all required

Status: Amended (2026-10-11; accepted 2026-09-05)
Plan: sections 2, 5 and 9

## Context

The rewrite's case rests on three benefits Go cannot give this project: lower memory and no collector pauses, a WebAssembly build, and in-process embedding by Rust tools. The owner declared all three primary.

## Decision

The spike must pass bounded experiments for all three (E5 to E8) before the rest of the plan starts. Parser-level measurements do not establish whole-checker performance or whole-compiler WebAssembly support. The original cut-over decision required the production WebAssembly library to run every corpus and the named budgets to pass four consecutive weekly runs. The two amendments below replace those corpus-routing and repetition requirements. In-process embedding comparisons, ownership, malformed-input, cancellation and panic-invalidation tests remain required, as do the declared production size, latency and retained-memory budgets.

## Amendment, owner decision 2026-10-11

Weekly performance runs and the four-consecutive-run requirement above are
postponed indefinitely. They are not active closure requirements while deferred;
resumption requires an explicit owner request. The budgets, comparable-work
and measurement-validity rules, required deliverables, and the separate
four-week dogfood requirement remain unchanged. See Phase 7 plan decision 4.

## Compiler corpus amendment, owner decision 2026-10-11

Compiler/conformance acceptance may use the test-only WebAssembly consumer of
public `tsr_embed`, sharing the native external consumer's executor, walker and
query order. Production `tsr_wasm` remains required for transpile acceptance,
browser smoke, and production size and performance budgets. The test-only
compiler consumer does not certify those production budgets. Both corpus
routes use an in-memory host without native processes or filesystem access.
See [Phase 7 architecture item 4](../PHASE7-plan.md).

## Consequences

Phase 7 includes full checking and emit through both entry points. `tsr_wasm` and `tsr_embed` exist as crates from the spike onward, with parser and checker-slice entry points first.

## Evidence

Plan section 5, exit criteria; section 9, Phase 0 experiments and Phase 7.
