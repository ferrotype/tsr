# C6 implementation record

## Starting point

C6 starts from the recorded C5 exit (`target/phase2/rust-c5`: 13,417 of 13,432
variants match every enabled domain, and all 9,367 S08 regression variants
match). The owner confirmed the seven decisions of the plan's section 9 as
proposed:

1. `compiler/checkerpool.go` moves to Phase 2 in the ledger.
2. The tracer gets a `TraceSink` seam.
3. The concurrent native capture runs on this Mac.
4. The FENNEL arithmetic is reproduced per architecture.
5. Cancellation is a `Send + Sync` token in `tsr_core`: the project pool
   disposes a canceled checker and the compiler pool abandons it.
6. ThreadSanitizer stays Phase 4, and C6 uses the E3 Miri and
   AddressSanitizer lanes.
7. Two exit captures, one per mode, are recorded by the owner.

C6 owns no rows.

## The ledger move and the concurrent native capture (C6.0, C6.1)

`75bcaf1b` moves `compiler/checkerpool.go` to Phase 2
(`FILE_PHASES` in `scripts/ledger-init.py`) and regenerates the ledger.
`data/upstream.json` is an input of every native capture, so the move staled
them. The single-threaded capture was retaken (`target/phase2/native`, row
digests identical to C0; the C0 capture is kept at `target/phase2/native-c0`).
The S07 observations were re-exported byte-identical, with their provenance
replaced, and the S07 subset review re-frozen (finding
`PHASE2-C6-2026-09-29-ledger-provenance-refresh`). The C2 creation traces
were re-frozen: one witness's raw sort input order follows Go map iteration,
and its summaries are identical. The owner chose to keep the C1 to C5
baselines as history, since each Rust capture binds the native digest it was
compared with. So the producer reports C1 to C5 accounting as unavailable.
The 22 recorded runs that bind `data/upstream.json` read stale until they are
re-recorded.

`675f849e` adds `scripts/phase2_native_concurrent.py`: the C0 oracle with
`TS_TEST_PROGRAM_SINGLE_THREADED=false`, the same requests, sharding and
validation, and a second sharding for verification. It lives beside
`phase2_native.py`, not in it, because that script is an input of the recorded
single-threaded capture. The two modes differ only in union ordering's checker
and union counts; every verdict and every other domain is identical, so the
pin shows no mode difference that the comparison sees
(`data/phase2/native-provenance-concurrent.json`, `data/phase2/c6-claims.json`).
`cdc4e971` freezes the C6-start baseline (the C5 exit's observations on every
row), binds the audit scope (seven groups, 89 functions) and registers the C6
authorities.

## The tracer (C6.2)

`e282671b` ports `tracer.go` behind a `TraceSink` trait (`push`, `pop`,
`instant`, `record_type`) with an in-memory sink and a JSON-lines sink. The
file writer stays Phase 4's. The pin's 16 gated call sites are all ported,
including `RecordType` in `newType`. Contract 7 checks one witness program
with a `lib.d.ts` stub against the pinned tsgo's normalized
`--generateTrace` output: its 8 events and 148 type records, in
`fixtures/c6/trace`, with a `regenerate.py`. Tracing on and off leave the
checker's results identical. Matching the pin's type records exposed three
places where Rust created types the pin does not. `keyof` checking resolved
its operand as a type node; keyword and literal type nodes created their
types when checked; `ThisType` was not resolved when checked. All three now
follow the pin.

## Cancellation (C6.5)

`72ebfc41` adds `tsr_core::CancellationToken`. The checker polls it where the
pin polls its context:

- per statement in `checkSourceElements`, which every source-element loop
  now goes through, so a canceled body stops at its first statement;
- per deferred node;
- per property of the contextual deprecation check;
- before the renamed-binding and unused-identifier passes.

A canceled check returns no diagnostics and leaves `wasCanceled` set. From
then on diagnostics, global diagnostics and the node builder's recovery
boundary return `Error::PreviouslyCanceled`, the pin's message, without
retiring the generation. The project pool disposes a canceled checker at
release. Contracts 4 and 5 agree with the pinned tsgo's uncanceled
diagnostics. Nine mutants are all killed: each poll, the sticky flag, each
refusal and the disposal.

## The compiler checker pool and its assignments (C6.3, C6.7)

`18d15cc7` adds `tsr_compiler::CompilerCheckerPool`, a port of
`checkerpool.go`:

- the checker count rule;
- base weights, the source-file multiplier and import normalization;
- the three calibrated regimes and the source-first order;
- the weighted FENNEL assignment with the pin's ties, one-percent slack and
  least-loaded fallback.

On aarch64 the score arithmetic reproduces go1.27.1's arm64 fusion, two
`FMSUBD`s: the `oldWeight*sqrt(oldWeight)` product and the `alpha*(…)` product
each fused. Every other architecture rounds each step, as amd64 does at v1
and v3. The evidence, `data/phase2/c6-fusion-arm64-go1.27.1.txt`, is now
extracted from the toolchain's assembly by
`scripts/phase2_assignments.py fusion --record`, and `fusion --check`
re-extracts it.

The pool creates its checkers once, under one generation, in a
`tsr_core::workgroup::WorkGroup`. That is the pin's `NewWorkGroup`: parallel
tasks run on threads with the ADR 0011 reserved stack, and a single-threaded
group runs its tasks on the caller, last queued first. `tsr_checker::CheckerPool`
is the pin's `GetChecker` with a scoped release, and both pools implement it
(`tsr_project` through `Project`). `ProgramOptions` gains the pin's
`single_threaded`, which the compiler option supplies when unknown.

`scripts/phase2_assignments.py record` builds a diagnostic overlay of the pin
in which `createCheckers` reports its inputs and results. It runs every corpus
row in the concurrent mode, which gives 26,864 records, one each for the
pre-emit and post-emit programs. It also runs the association step over a
synthetic set of 27 graphs at 2, 4 and 8 checkers. In that set, ties, the
fallback, the slack, each regime, import normalization and its clamp each
decide an assignment. The set also has two constructed near-ties that only
the fused arithmetic decides: loads of perfect squares, so both load
increments are exact, and a total weight of 34², so the penalty factor is one
rounded division. No corpus program depends on the fusion. The synthetic step
is trusted only after it reproduces all 10,044 distinct multi-checker corpus
records. `compare` runs the Rust partitioner over each row's program, loaded
as the corpus loads it, and over the synthetic inputs. 13,417 programs are
equal, the 15 content-mapper programs are the known unsupported loads, and
all 27 synthetic cases are equal. Seven partitioner mutants are each killed,
the fusion one by exactly the two constructed cases.

## Program driving in both modes (C6.4)

`76e28426` adds `tsr_compiler::CheckedProgram`: a program with its checker
pool, the pin's `checkerPool` and `compilerCheckerPool` fields. It sits beside
the immutable `Program` because the checkers own the program. It ports:

- `initCheckerPool` and `GetCheckerPool`;
- `GetTypeChecker`, `GetTypeCheckerForFile` and
  `GetTypeCheckerForFileExclusive`;
- `ForEachCheckerParallel`;
- `collectDiagnosticsFromFiles`, `collectCheckerDiagnostics` and
  `collectCheckerDiagnosticsFromFiles`;
- the program's semantic, suggestion, declaration and global diagnostics.

A Rust checker access always holds the checker's operation. So the pin's
lock-free hand-out to the emit resolver holds the file's checker for that
file's transform, which is one of the interleavings the pin's per-call locks
allow. `FileCheckers` holds every checker's operation for a walk and serves
each file its own checker. It has no `Deref` to the current checker: the first
draft had one, which silently turned the walker's per-file selector back into
a single checker.

The corpus driver checks a request that names a mode through the program's
pool, as the pin's harness does:

- **Single mode.** The program's own setting is true: one checker, and
  single-threaded work groups, so declaration diagnostics and emit run last
  file first.
- **Concurrent mode.** The setting is unknown, as in `harnessutil.createProgram`,
  so the compiler option decides: the pool's checker count and parallel groups.

Both modes cover the diagnostic phases, the post-emit program and its emit
schedule, the walk (each file on its own checker) and union ordering over
every checker. A request without a mode keeps the single owner that the S08
producers and checkerbench measure. `phase2_corpus.py run --mode` records the
mode and each row's checker count. `phase2_compare.py modes` compares each run
with its own mode's native capture and counts the per-row, per-domain outcome
differences. The C6 audit is complete.

## Retirement under concurrency (C6.6)

`cbff103d` gives each group task of the compiler pool a generation check
before each file. A task stops at a retired generation, and the group fails
instead of returning part of a retired generation's results. The E3 pool
scenarios run over the compiler pool as crate tests (`checker_pool::ownership`):

- shared-pool panic retirement, with the arena's retirement contention
  observer on real pool threads;
- wrong-owner rejection across sibling checkers;
- release boundaries;
- a retirement between files.

They are the S09 ownership suite `compiler_pool` (manifest version 5), part of
both pool criteria. They pass in debug, release, AddressSanitizer and Miri.

## Contracts (C6.9)

All nine contracts are in `crates/tsr_compiler/tests/c6_contracts.rs` and pass
in debug and release:

1. Partitioning over the recorded graphs and the count rule.
2. Acquisition and file affinity.
3. Work-group order and merged global diagnostics.
4. Cancellation between statements.
5. Cancellation in deferred nodes and before the unused passes.
6. Panic retirement in a multi-checker pool.
7. Tracing.
8. The two modes, over a three-file corpus program frozen from both verified
   native captures (`fixtures/c6/modes`: 73 unions on one checker, 94 over
   four).
9. Reserved stacks, and the pool's lifecycle and owners.

Each contract that passed on its first run was mutation-checked.

## Producer wiring (C6.10)

`c6c4e963` adds `native_verified_concurrent`, `harness_valid_concurrent`,
`c6_mode_parity` and `c6_assignments` to the shared accounting.
`c6_failures` counts rows failing in either mode, and `c6_complete` is the
conjunction. `c6_assignments` needs two things:

- the recorded witnesses, current with their inputs;
- a recorded Rust comparison (`phase2_assignments.py compare --record`,
  `data/phase2/c6-assignment-comparison.json`) bound to the record, to the
  current Rust sources and to the record's architecture.

The concurrent Rust run lives at `target/phase2/rust-concurrent`, beside
`target/phase2/rust`.

## Exit

Both exit captures ran on the final sources, each against its own mode's
verified native capture. Neither timed out, failed to execute, or had a
harness error, and both replay source-stable.

**Single-threaded mode.** `target/phase2/rust-c6`, now through the program's
pool:

- 13,417 of 13,432 variants match every enabled domain, and all 9,367 S08
  regression variants match.
- Against the C6-start report, no domain regressed and no observation
  changed, so the pin's last-first order for declaration diagnostics and emit
  changed no row.
- The 154 traced variants keep trace parity.

**Concurrent mode.** `target/phase2/rust-c6-concurrent`:

- 13,406 programs checked with four checkers, 4 with two and 7 with one; the
  15 content-mapper programs do not load.
- 13,417 variants match every enabled domain against the concurrent native
  capture.

**Mode parity (C6.8).** `phase2_compare.py modes` finds 0 per-row, per-domain
outcome differences between the two runs. Their raw observations differ only
in union ordering's counts, on 13,410 rows, which is where the pin's two modes
differ. So no row needed tracing, and `data/phase2/c6-claims.json` keeps no
Rust mode difference. The comparison is recorded in
`data/phase2/first-comparison.json`.

**Handoffs and the register.** The mode field changed every request, so all
21 handoffs were attributed afresh and rebound. Each request changed by that
field alone. The 15 C3 content-mapper handoffs keep byte-identical
observations. The three C2 handoffs to C5 and the three C5 incoming entries
match, and their raw rows gained the pooled path's `mode` and `checker_count`.
The rebuilt register still has its one entry, B01 (content-mapper execution,
Phase 5, 15 variants).

**Assignments (C6.7).** The witnesses were re-recorded on the final scripts
(26,864 program records, and the step check equal on all 10,044 distinct
multi-checker records). The Rust comparison is recorded on the final sources
(`data/phase2/c6-assignment-comparison.json`): 13,417 programs equal, 15
unsupported and 27 of 27 synthetic cases equal. `fusion --check` confirms the
committed evidence.

**Receipts and gates.** The C1 to C6 contract receipts were refreshed on the
final sources. Workspace formatting and clippy with warnings denied pass, and
so does `cargo xtask validate`.

**S07 anchors.** The C6 ports moved or added the Rust anchors of 39 functions
in `data/s07/operations.json`, so the operation inventory was regenerated and
the S07 subset review re-frozen (finding
`PHASE2-C6-2026-09-29-mapping-refresh`). The one change outside
`rust_mappings` is `NewWorkGroup`'s `mapping_status`, which its new port
marker turns into `source_marker_present`. `subset.json` and
`checker-obligations.json` keep their reviewed digests.

**Python suite.** With that refresh, the only failures are the three
Phase 1 tests that read the syntax schedule as stale against
`data/upstream.json`. That staleness comes from the ledger move the owner
chose to keep and re-record later. It belongs with the 22 runs, the Phase 1
native captures (the syntax captures also bind `Cargo.lock` and
`tools/s07/program/rust_observation.rs`, which C6 changed) and the E3 run
(now fingerprinting the relater prototype, with ownership manifest version 5)
for the end-of-phase re-record.

**The producer.** The checker producer reports `c6_complete = true`, with
every input true and every count zero:

- `native_verified_concurrent`, `harness_valid_concurrent`,
  `c6_mode_parity`, `c6_assignments`, `c6_audit_complete` and `c6_contracts`
  are true;
- `c6_open`, `c6_regressions`, `c6_failures`, `c6_handoffs` and
  `c6_blockers_open` are 0;
- every prerequisite is true, and `regression_parity` and `trace_parity`
  are 1.

C1 to C5 accounting is unavailable, as chosen when their baselines were kept
as history. The recording, `cargo xtask run checker` and
`cargo xtask status --record`, is the owner's.
