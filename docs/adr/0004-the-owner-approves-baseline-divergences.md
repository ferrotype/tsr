# ADR 0004: The owner approves baseline divergences

Status: Accepted (2026-09-05)
Plan: section 5

## Context

Upstream records its own accepted divergences from TypeScript 6 in `testdata/submoduleAccepted.txt`. The Rust port will produce intentional differences of its own, and every one of them makes a baseline comparison fail.

## Decision

Intentional differences are recorded in an allow-list in this repository, one entry per case with the reason, and each entry is approved by the repository owner. A baseline failure without an approved entry is a failure; the allow-list cannot be edited in the same change that introduces the difference without the owner's approval on record.

`data/divergences.toml` is the shared allow-list. Entries default to the E2
domain; command-line transcript entries specify `domain = "phase4"` and
`metric = "baseline_parity"`. The owner's 2026-10-02 eager-binding trace-order
approval is recorded there as two exact native/Rust transcript hash pairs
(Phase 4 decision 15). Raw results remain `different`; only those pairs enter
`baseline_accepted`. A matching case name alone or an approval in another
domain cannot waive a new difference.

## Consequences

The compiler runner and the status tool treat allow-listed cases as expected differences and count everything else as failures. Coverage numbers quote the allow-list size alongside the pass rate.

## Evidence

`tsc/testdata/submoduleAccepted.txt` and `submoduleTriaged.txt` in the pinned checkout.
