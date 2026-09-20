# S12 closure record

Closed on 2026-09-20 using the recent captures recorded on 2026-09-19.
The owner confirmed their applicability after the crate rename.
[ADR 0020](adr/0020-phase-0-gate-decision.md) records the go decision and its
limits. The original decision packet from `d727434` is included in the closure
branch as `c176bcf`; owner approval was first recorded in `66de77e`.

## Accepted scope

The owner approved the 30% parser-size, 1.5-times parser-throughput and 40%
Node-latency limits, the [Phase 7 acceptance matrix](S12-acceptance.md), and
closing Phase 0 with the existing captures. No correctness corpus, ownership
suite or benchmark is rerun for this closure.

The [evidence index](S12-evidence.md) identifies 22 immutable producer
records. Their hashes, successful execution status and reported metrics were
checked; cases-based records were checked against their recorded manifests.
All 55 E1–E8 criteria and S01–S10 required-item/exit predicates are satisfied by
that declared collection under the approved limits. The recorded S10 timing
confidence bounds also satisfy the amended limits. No ratios are combined
across measurement batches.

## Tracking policy

S12 records reuse of the recent evidence across the crate rename through ADR
0020. The tracker compares exact fingerprints, so a namespace change can mark
a recent capture `stale` even when its measurements remain applicable. That
label describes a fingerprint mismatch, not the age or quality of the evidence.

The change is confined to S12. Producer fingerprint checks, experiment criteria
and other sprint checks remain in force; capture files, timestamps, revisions
and source/input hashes remain unchanged. Future behavioral changes and Phase 7
retain their own evidence requirements.

## Closure validation

Validate the ledgers, regenerate the committed views and record the milestone:

```sh
cargo xtask validate
cargo xtask status --record
cargo xtask check S12
cargo xtask status --check-committed
```

These are tracker checks, not new producer or benchmark runs.
