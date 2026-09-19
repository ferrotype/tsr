# ADR 0022: Checker per-type footprint threshold

Status: Accepted (2026-09-18)
Plan: section 9, Phase 0 experiment E5; S08 implementation plan section 6.1
Sprint: S08, item S08-7
Amends: `E5.type_footprint` in `status/experiments.toml` and the threshold in `data/s08/type-footprint.json`

## Decision

The owner requested an E5 memory limit of **0.85 Rust / Go**. Peak RSS and
allocated bytes for parse/bind already have that limit under ADR 0021. This
decision changes the remaining E5 criterion, the checker subset's per-type
footprint, from **≤0.80 to ≤0.85**. All three E5 memory criteria now use 0.85.
E6's CPU limits and host restrictions remain as specified in ADR 0021.

This is an explicit acceptance-policy change. It requires at least 15 percent
lower aggregate mean storage per reachable type, instead of 20 percent. The
recorded footprint ratio of 0.811623337650432 in E5 evidence
`408015fbeb12c2f558204c6778a2c651ee541704d6caf7a29a27cd59e82abc05`
missed the former limit and is numerically within the new limit. That
observation motivates no change to the measured value or its execution date;
the owner's authorization establishes the new limit.

The 9,369-variant workload, query schedule, census membership, shared-allocation
attribution, reachable-type denominator, sampling and availability rules stay
unchanged. The statistic remains the Rust aggregate mean charged type-storage
bytes per reachable type divided by the corresponding Go mean. It does not
establish a whole-checker or full-compiler memory saving of 15 percent.

## Evidence and tracking consequences

`status/experiments.toml` is the acceptance authority. The frozen footprint
method mirrors the maximum, and its P0 artifact digest changes only to record
this approved amendment. Other P0 artifacts, observations and source receipts
are preserved; this does not claim a fresh P0 execution or refreeze.

The footprint manifest participates in checkerbench capture authentication and
the checkerbench/E5 ledger inputs. The experiment ledger also participates in
the S07 graph and benchmark fingerprints and the bindworkload/E5/E6 inputs.
Therefore these edits stale the affected measurements under the existing
contracts, including E6 even though its numerical limits do not change.
The preceding observer-fingerprint fixes already require fresh checkerbench
evidence. Historical captures and evidence records must not be rewritten or
promoted to current evidence by changing their hashes.

Regenerated status views show the new policy and current provenance state.
S08-7 and E5 pass only when current evidence satisfies their requirements;
this decision alone closes neither. Refresh the checkerbench capture and its
producer, and the S07 graph → benchmark → verify-capture → E5/E6 chain on the
designated host, once the measurement inputs are ready. Preserve failures as
observed. Earlier experiment records retain their original limits and verdicts.
