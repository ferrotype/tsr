# ADR 0010: Order-sensitive outputs are enumerated; comparators are ported exactly

Status: Accepted (2026-09-05)
Plan: section 6, decision 5

## Context

Symbol tables in Corsa are plain Go maps with random iteration order, so the checker sorts wherever order is observable: `CompareTypes` for union constituents, `compareSymbols` for members, `CompareDiagnostics` for diagnostics. IDs are the final tie-break after the earlier comparison keys tie. Known cases include intrinsic types, reverse mapped types without symbol or mapper, and equal-named symbols with no declarations or first declarations that compare equal. Type IDs reflect checker-local creation order; symbol IDs are assigned lazily at first use, not at allocation. Draft 1 wrongly demanded that Rust reproduce Go's allocation sequence.

## Decision

Port `CompareTypes`, `compareSymbols`, `compareNodes` and `CompareDiagnostics` line by line and sort at exactly the points Corsa sorts. Construct intrinsic types in Corsa's order. List the residual id-sensitive cases and give each a test; keep a creation-trace mode in both binaries scoped to diagnosing those cases.

## Consequences

The Rust checker may allocate freely. The `union ordering` sub-test is a consistency check on exercised types, not a proof of total ordering for every type graph; the residual cases are known instabilities with their own tests.

## Evidence

`tsc/internal/checker/utilities.go` (`CompareTypes`, `compareSymbolsWorker`), `tsc/internal/checker/checker.go` (`getUnionType`; `compareTypeIds` with no callers), `tsc/internal/testrunner/compiler_runner.go` (`union ordering`).

## C2 pin clarification (2026-09-26)

The final ID return in `CompareTypes` is general, not restricted to the named
examples. `compareSymbolsWorker` also reaches its fallback when equal-named
symbols have first declarations for which `compareNodes` returns zero. The
creation-trace diagnostic observes actual fallback branches and distinguishes
symbol birth from the natural `GetSymbolId` assignment; requesting IDs early
would perturb the behavior under investigation. Instrumented, fingerprinted Go
overlay copies can observe these events without editing the pinned checkout.
This clarifies the pinned implementation; the accepted exact-comparator
decision and freedom from whole-program allocation-sequence parity are unchanged.
