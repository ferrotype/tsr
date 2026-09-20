# S12 closure sequence

Status: closure work on merged main `e4f4dca`; ADR 0020 was accepted by the owner
on 2026-09-20. The original packet from `d727434` is included in this branch.
The three approved prototype thresholds are updated; evidence rules are
unchanged and no new measurement has started. The approved decision is
[ADR 0020](adr/0020-phase-0-gate-decision.md) and the
[full acceptance matrix](S12-acceptance.md).

## 1. Approved policy — complete

The owner approved the 30% / 1.5 times / 40% prototype limits and the independent
Phase 7 budgets. PLAN, the experiment ledger, ADR and its index agree. This
completes the owner-decision item S12-3. S12-1 and S12-2 still require current
passing evidence.

The experiment ledger participates in measurement fingerprints. Changing it
after a capture would require another refresh; settle it first. Freeze all
source/configuration changes before the final acceptance batch. The crate
rename and publication preparation have now landed. Preserve every existing
capture in its original form.

## 2. Audit reuse and prepare the final revision

The S11 CI snapshot proved E1, E3, E4, S01–S06 and S11 in its recorded source and
runner context. Normal CI E2 proves only the frozen denominator. It is not a
replacement for the full E2 checker capture.

The existing S08 measurement build records differ from main in Cargo inputs,
AST storage and parser code; checkerbench also differs in the program-loading
and corpus executor adapters. Existing S10 captures predate wasm error-handling,
Node adapter and capture-scope changes. The rename and package-manifest edits
also invalidate their declared inputs. These are not revision-name-only changes.
Keep those records historical; do not rewrite their fingerprints.

Before a long run, verify the inventory, native request ordering, source and
input fingerprints, toolchain availability, disk capacity and capture paths.
Use existing current Go observations where their normal preflight permits it.
Build every required normal/instrumented executable before its timed batch.
Do not compile or edit measured sources during timing.

## 3. Refresh correctness and ownership

Capture the complete E2 acceptance/informational corpus and obligations on the
final revision; retain raw diagnostics, type/symbol outputs and public display.
Record E2 only after verification succeeds.

Capture full wasm and external-Rust-consumer correctness against the validated
native observations, plus portable-host and applicable consumer lifetime checks.
Keep unsupported/unavailable native cases and the declared Node stack setting
visible. Reuse a current CI portable/lifetime capture only if its normal source,
binary, configuration and input checks all permit it.

Resolve the final status execution context deliberately. Archived CI records
cannot be relabeled as measurements from the designated local host. The current
tracker evaluates a live sprint using one host/toolchain/environment identity;
run any needed stale prerequisite producers in that identity rather than
silently weakening those checks or mixing saved metrics from several reports.

## 4. Run serial measurements on the designated host

1. Prepare/validate the full parse/bind graph prerequisite with `bindworkload`.
2. Capture paired Go/Rust parse/bind timing, RSS and allocation work; preserve
   one/eight-worker samples and apply the existing stability/stopping policy.
3. Capture checkerbench normal, phase and allocation modes over all 9,369
   acceptance variants; require identical work, outputs and complete census.
4. Capture the production and reference relaters over all 105 groups; require
   exact results, diagnostics, lazy construction and counters before comparing
   timing or memory.
5. Capture WASM parser and Node parse-and-encode measurements with the final
   approved thresholds, fresh inputs, byte parity and their prescribed samples.

Keep these timed batches serial and record host load. Reuse completed captures
only through the existing validators. Do not select favorable batches, add
separately measured savings or overwrite prior measurements.

## 5. Record and close

Run the replay producers for E2, checkerbench, relater, E5/E6 and E7/E8. Ensure
all required quality/generation/correctness/ownership producers are current in
the final report's context. Recompute status and inspect every S02–S10 required
item and E1–E8 criterion, not merely the aggregate experiment count.

If a measured gate still fails, report its exact value and stop advancement;
do not turn the accepted conditional go decision into a passing observation.
Once every non-ADR requirement passes, retain ADR 0020's approval date, record
the final immutable evidence identities, then
run `cargo xtask validate`, `cargo xtask status --record`,
`cargo xtask check S12` and `cargo xtask status --check-committed`.

Commit the decision, artifacts and generated views together in a focused PR.
No handwritten sprint-complete flag or exception to freshness substitutes for
that final check.
