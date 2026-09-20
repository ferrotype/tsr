# S12 closure record

Closed by owner decision on 2026-09-20 using the preserved historical evidence.
[ADR 0020](adr/0020-phase-0-gate-decision.md) records the go decision and its
limits. The original decision packet from `d727434` is included in the closure
branch as `c176bcf`; owner approval was first recorded in `66de77e`.

## Accepted scope

The owner approved the 30% parser-size, 1.5-times parser-throughput and 40%
Node-latency limits, the [Phase 7 acceptance matrix](S12-acceptance.md), and
closing Phase 0 with the existing captures. No correctness corpus, ownership
suite or benchmark is rerun for this closure.

The [evidence index](S12-historical-evidence.md) identifies 22 immutable producer
records. Their hashes, successful execution status and reported metrics were
checked; cases-based records were checked against their historical manifests.
All 55 E1–E8 criteria and S01–S10 required-item/exit predicates are satisfied by
that declared collection under the approved limits. The historical S10 timing
confidence bounds also satisfy the amended limits. No ratios are combined
across measurement batches.

## Tracking policy

S12 now records the dated historical acceptance through ADR 0020. This replaces
its former requirement that all prerequisite evidence be current in one report
context. The change is confined to S12: producer freshness checks, experiment
criteria and other sprint checks remain in force. The historical evidence
files, timestamps, revisions and source/input hashes are unchanged.

A completed S12 can therefore coexist with stale live experiment or sprint
checks. This means Phase 0 was accepted; it does not mean the renamed/published
sources were measured by those older captures. Future work and Phase 7 retain
their own current-evidence requirements.

## Closure validation

Validate the ledgers, regenerate the committed views and record the milestone:

```sh
cargo xtask validate
cargo xtask status --record
cargo xtask check S12
cargo xtask status --check-committed
```

These are tracker checks, not new producer or benchmark runs.
