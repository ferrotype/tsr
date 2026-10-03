# ADR 0004: The owner approves baseline divergences

Status: Amended (2026-10-03, ADR 0023)
Plan: section 5

## Context

Upstream records its own accepted divergences from TypeScript 6 in `testdata/submoduleAccepted.txt`. The Rust port will produce intentional differences of its own, and every one of them makes a baseline comparison fail.

## Decision

Intentional differences are recorded in an allow-list in this repository, one entry per case with the reason, and each entry is approved by the repository owner. A baseline failure without an approved entry is a failure; the allow-list cannot be edited in the same change that introduces the difference without the owner's approval on record.

**Amended 2026-10-03 (ADR 0023).** The allow-list is the `approved` field of
a failing entry in the suite's expectation file, `status/parity/<suite>.json`
(docs/EVIDENCE-plan.md, section 3): the entry names the exact sub-test, its
`reason` says how the Rust output differs and why that is acceptable, and
`approved` carries the owner's words and date. The owner approves by merging
the pull request that adds the field; CI rejects a change in that sub-test's
outcome either way, so an approval covers one difference, not a case name.
`data/divergences.toml` carried the earlier entries and is retired as they
move; the 2026-10-02 eager-binding trace-order approval (Phase 4 decision 15)
is the first carried entry, in `status/parity/tsc.json`.

## Consequences

The compiler runner and the status tool treat allow-listed cases as expected differences and count everything else as failures. Coverage numbers quote the allow-list size alongside the pass rate.

## Evidence

`tsc/testdata/submoduleAccepted.txt` and `submoduleTriaged.txt` in the pinned checkout.
