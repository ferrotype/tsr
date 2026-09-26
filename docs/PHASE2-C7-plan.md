# Phase 2 C7: full correctness and readiness

Checkpoint C7 of the [Phase 2 plan](PHASE2-plan.md), written after the
[C2 plan](PHASE2-C2-plan.md) (PR #62, branch `phase2-c2-plan`) and its review
amendments, alongside the C3 to C6 drafts, over the recorded C2 exit capture (`target/phase2/rust`: 12,647 of 13,432 rows match in every domain, regression 9,367 of 9,367; `P2B-C2` recorded complete with three emit-order rows handed to C5). Upstream is Corsa `1f70213d4922b434345f639b441681e470c7cfc1`.
C7 is closure work: it adds no checker semantics of its own. It drives every
remaining difference to a checkpoint or to an owner-approved scope, runs the
complete acceptance on the final inputs, publishes the operation and
dependency disposition, records the evidence that closes sprints P2A and P2B,
and reports what Phase 3, 4 and 5 can consume and what Phase 7 still owns.

## 1. Objective and exit

Resolve the remaining categorized failures, run complete acceptance on the
final relevant inputs in both execution modes, validate the ordering and
parent-pointer sub-tests and the lifecycle and recursion checks, and publish
the operation and dependency disposition. Every required Phase 2 gate passes;
any retained difference has its exact owner-approved scope. Report what Phase
3, 4 and 5 can now consume and the measured performance risks that Phase 7
still owns.

C7 exits when all of the following hold on the final recorded runs:

| Exit condition | Measured by |
| --- | --- |
| Stage A closes: the denominator is frozen, the native contract is verified, one complete Rust run is recorded and every withheld observation is a named blocker | `sprint.P2A.done == 1` (`inventory_frozen`, `native_verified`, `harness_valid`, `result_recorded`, `blockers_named`), from the same recorded `checker` run that closes stage B |
| PLAN's Phase 2 gate: every executed variant matches in `errors`, `types` and `symbols` with approved divergences only, plus `display`, the module-resolution trace, union ordering and parent pointers | `run.checker.errors_parity == 1`, `types_parity == 1`, `symbols_parity == 1`, `display_parity == 1`, `trace_parity == 1`, `ordering == 1`, `parent_pointers == 1`, `unsupported_required == 0`, `harness_valid == true` (the P2B `exit` list) |
| Every checkpoint closed on its own recorded run | each `P2B-Cn` item's `done_when` holds on a recorded `checker` run identified by its evidence id (`recorded.checker.cN_complete == true`, the tracker extension of C7.7); the final run does not recompute `cN_complete` or `c2_measured` |
| No executed row differs in any domain without an owner-approved scope | `run.checker.c7_residuals == 0` over `data/phase2/residuals.json` |
| The runner's skips are listed explicitly with their native guard reasons and stay informational (S12 matrix) | `run.checker.c7_informational_listed == true` over `data/phase2/informational.json` |
| The operation and dependency disposition is published: every Phase 2 ledger file `ported` with `verify` checks that derive `verified`, every function of the Phase 2 packages mapped, equivalent or handed with an owner, the blocker register holding only cross-phase joint entries with owners, the divergence ledger valid against the final comparison | `run.checker.c7_dispositions == true`, `c7_divergences_valid == true`; `cargo xtask status` derives `verified` for the Phase 2 files |
| The phase-end green-up is done: no stale evidence among the producers whose fingerprints Phase 2 changed, both runners green, S01 to S06 still close | `run.checker.c7_evidence_current == true`; `cargo xtask check P2A` and `check P2B` pass; `status --check-committed` passes in CI |
| The C7 record exists with the per-area dashboard, the downstream consumption report and the performance risk report bound to the recorded captures | `run.checker.c7_report == true` |

`run.checker.c7_complete` is the conjunction. It binds `P2B-C7` in
`sprints/P2B.toml`. No ratio threshold and no performance threshold are
introduced: Phase 2 is a correctness gate, and C7 reports the performance
figures side by side without converting them.

## 2. Starting point

Everything below exists and is consumed as is. C7 extends it; it re-derives
nothing.

| Asset | Where | What C7 takes from it |
| --- | --- | --- |
| The gate wiring | `sprints/P2A.toml` (exit: the five capture-integrity metrics), `sprints/P2B.toml` (exit: `sprint.P2A.done == 1`, `harness_valid`, the seven parity and sub-test metrics, `unsupported_required == 0`; items C1 to C7 bound to `cN_complete`); `status/runs.toml` `[checker]` (`phase2_producers.py checker`, its inputs and sources); `cargo xtask check P2A|P2B`, `check-metrics`, `run <id>`, `status --record`, `status --check-committed`, `validate` | the exit C7 closes and the commands that assert it |
| The current gap | the C2 exit capture's categories: `errors` 12,648 match, 20 different, 764 unsupported; `types` 12,188 match, 8 different, 557 unsupported, 679 native-disabled; `symbols` 12,196 / 0 / 557 / 679; `display` 12,196 / 0 / 557 / 679; `parent_pointers` and `union_ordering` 13,417 match, 15 unsupported; `trace` 154 match, 13,278 disabled; 6 unsupported operations; 12,647 rows matching in every domain; every recorded producer currently `stale` in `status/status.json` | the distance the checkpoints close before C7; the staleness C7.6 clears |
| The denominator and its skips | `data/phase2/inventory.json`: 15,206 effective variants, 13,432 executed (`native_selection: runs`), 1,774 informational (1,720 `option_guard_skip`, 52 `filename_skip`, 2 `not_enumerated`; 39 rejected-option rows are a subset of the filename skips); the guard-only option keys (`baseUrl` on 38 rows, `moduleSuffixes` on 15) | the explicit skip list C7.0 publishes |
| The native contract | `target/phase2/native` with `data/phase2/native-provenance.json` (13,432 executed, 0 input mismatches, 0 reference disagreements, 0 parent-pointer failures, 0 inconsistent unions, 3 pre/post-emit differences, 154 trace rows, 1,754 declaration requests); the concurrent capture C6.0 adds | the final inputs; reused only when the read-only validator accepts them |
| The sub-tests | `tools/phase2/subtests.rs`: union ordering (every interned union reproduced by sorting its reversed list and ten seeded shuffles with the production comparator; one checker per program at C0, per checker at C6), parent pointers (below each non-default-library root every node the generated child visitor reaches has the traversal parent as its recorded parent; the root is not checked; the walk stops at its first failure), module-resolution trace (the loader's trace localized and sanitized as the baseline tracer writes it); `phase2_compare.py compare_subtest` compares verdicts, not counts | the Rust definition the review asked C7 to state; C7 validates it, it does not redefine it |
| The lifecycle and recursion witnesses | the C1 contracts (cycle diagnostic, repeated-query identity, relation caches, two checkers over one program, a 400-level relation on the E2 small stack, the pin's semantic limits, panic retirement), the C3, C5 and C6 contracts, E3's criteria in the ownership harness and, after C6, through the compiler pool; the E2 obligations replay (8 residual families, 7 matrices, 77 permutations) | rerun on the final executable in C7.2 |
| The divergence ledger | `data/divergences.toml` (ADR 0004): empty; an entry needs an exact observation witness per variant and metric; failures, missing operations and native-unavailable cases cannot be waived; unused witnesses block | the only way a retained difference can be approved |
| The ledger and the function inventory | `PORTS.toml` (31 Phase 2 files, `checkerpool.go` joining from Phase 4 under C6; `ported` requires Rust paths; `verified` is derived only when the file is synchronized to the pin and every nonempty `verify` check passes against current evidence); `data/go-functions.tsv` (14,595 functions); `status/unmapped-functions.json` (internal/checker 1,337 unmapped, internal/compiler 203, pseudochecker 50, tracing 20, core 40; 4,903 of 11,506 mapped overall); the C1 to C6 audits (`data/phase2/cN-audit.json`: `mapped`, `equivalent`, `later` with owner) | the disposition C7.4 publishes |
| The claims, blockers and handoffs | `data/phase2/cN-claims.json` for C1 to C6 with `handed`, `blocked` and `incoming` entries carrying the validator's fields; `data/phase2/blockers.json` rebuilt from evidence, never edited by hand; B06 (content-mapper execution, Phase 5, 15 C3 rows) and B09 (emit order, C5 with Phase 3, 3 rows) as the cross-phase entries | the residual list C7.1 consolidates |
| Performance captures | `[checkerbench]` (elapsed time and type footprint over the S08 workload on the quiet host; the C2 exit capture), `[e5]` (peak RSS, allocated bytes and the type footprint at 0.85, ADR 0022), `[e6]` (parse and bind wall time at 1.25 and 1.45, ADR 0021); full checking remains extrapolated at those gates | the figures C7.5 reports side by side |
| CI | the `status` workflow: committed views current, the declared minimum Rust on each OS, the producers job (packaged assets, `xtask validate`, tracker self-tests, Phase 1 contracts, the workspace build, per-crate contracts in debug and release, the Go oracle parity suites, formatting, clippy, dependency policy, Miri and AddressSanitizer, the regenerated views, S01 to S06 closing on the runner's evidence), the bare-wasm and embedding job | the phase-end green-up C7.6 targets on both runners |

C7 uses fresh capture directories for its final runs; nothing is moved or
overwritten. The recorded `checker` run is the owner's.

## 3. What C7 owns

C7 owns no rows and no semantic cause. It owns closure:

| Unit | State at the C7 start | C7's obligation |
| --- | --- | --- |
| Rows that still differ in any domain at the C6 head | expected: the 15 content-mapper rows (register entry B04 at the C2 exit, B06 in the C1 capture), the three emit-order rows (B05 at the C2 exit, B09 in the C1 capture) if C5's decision 3 was declined, and any row a checkpoint handed to Phase 3 or Phase 5 | consolidated in `data/phase2/residuals.json` with owner and scope; a residual owned by a checkpoint reopens that checkpoint's `cN_open`; a cross-phase residual is resolved by decision 1 |
| The informational rows | 1,774, listed in the inventory with their native selection | published with the guard reason and the option keys that trigger the guard |
| The sub-tests and lifecycle contracts | passing per checkpoint | validated once more on the final executable, in both modes |
| The divergence ledger | empty | validated against the final comparison; entries only with owner approval and witnesses |
| The ledger, the function inventory, the blockers, the handoffs | per checkpoint | one published disposition |
| The recorded evidence | all `stale` | the phase-end green-up and the owner's recordings |
| The report | none | `docs/PHASE2-C7.md` and the status views |

The ownership rule of the checkpoint plans applies unchanged: C7 never marks
a row closed by editing a status; a row closes when the final comparison shows
it matching, and a difference is retained only through the divergence ledger
or an owner-approved sprint-exit amendment that names the joint blocker.

**Completion and handoffs across captures.** Every handoff and the C2
measurement are bound to one Rust capture: the blocker builder's
`validated_handoffs` drops a handoff whose `capture_sha256` differs from the
current comparison's, and `c2_measured` binds the checkerbench record to the
corpus capture it was verified against. Two rules follow, and every plan from
C3 on uses them. First, a checkpoint's completion is a recorded historical
fact: `P2B-Cn` closes on the `checker` run recorded at that checkpoint's exit,
and no later checkpoint recomputes `cN_complete` or `cN_measured` on its own
capture; C7.7 extends the tracker so that an item's `done_when` can name a
recorded run (`recorded.checker.cN_complete == true`, with the evidence
identity) instead of the current one. Second, at every later checkpoint's item
0, `phase2_claims.py rebind` re-validates each open handoff against the fresh
capture: it re-runs the row's reproduction, checks that the new raw observation
still differs only in the covered domains, and rewrites the capture, request,
observation and trace digests in both the handing checkpoint's claims file and
the receiving checkpoint's `incoming` entry; a handoff that no longer holds is
reported and its row counts as open for the receiving checkpoint. C2.12 is
complete without a rebind step; C3.9 lands it in the per-checkpoint helper.

## 4. Work items

### C7.0 Final inputs, the denominator and the skip list

- Exists: the inventory, both native captures and their validators; the
  inventory's `informational_reason`, `native_selection`,
  `config_option_keys` and `options` per row.
- Build: `phase2_inventory.py check` at the final pin; `phase2_native.py
  verify` on both captures; `data/phase2/informational.json` listing every
  non-executed row with its native reason (`option_guard_skip`,
  `filename_skip`, `not_enumerated`), the rejected-option subset, the option
  keys the guard fires on and the file-name rule, generated by
  `phase2_inventory.py informational` and required to match the inventory
  byte for byte; a check that no executed row changed classification since
  C0 (the inventory digest in every claims file equals the current one).
- Exit: `c7_informational_listed` true; the denominator is 13,432 executed
  rows, unchanged.

### C7.1 Residual closure

- Exists: the C1 to C6 claims files with `handed`, `blocked` and `incoming`
  entries; the blocker register.
- Build: `phase2_residuals.py build` first runs `phase2_claims.py rebind`
  against the final capture, so that every open handoff is re-validated (the
  rule stated in section 3 of the C3 plan), then reads the final comparison
  and every claims file and writes `data/phase2/residuals.json`: each row not matching
  in some domain, with the checkpoint or phase that owns its cause, the
  validated trace that attributes it, the blocker identity if one withholds
  it, and the resolution path (`checkpoint` when a `cN_open` reopens,
  `divergence` when an ADR 0004 entry is proposed, `joint` when the cause is
  outside Phase 2). The residual list is the worklist of the last weeks of
  the phase: checkpoint-owned residuals go back to their checkpoint and keep
  it incomplete; `joint` residuals go to decision 1.
- Exit: `c7_residuals == 0`, meaning every executed row matches in every
  domain, or carries an approved divergence witness, or is named by the
  owner-approved sprint-exit amendment of decision 1.

### C7.2 Sub-tests, lifecycle and recursion validation

- Exists: `tools/phase2/subtests.rs`, `phase2_compare.py compare_subtest`;
  the C1, C3, C5 and C6 contracts; the E2 obligations replay; the E3
  scenarios; the relation contract.
- Build: run the union-ordering and parent-pointer sub-tests on the final
  executable in both modes (per checker in concurrent mode, as C6 wires),
  and make the parent-pointer report name the first failing node's kind,
  file and position so that a failure is attributable; add one direct
  witness per sub-test that fails on purpose, injected through a test-only
  path of `tools/phase2/subtests.rs` (a comparator override and a parent
  override the test passes in; never a cargo feature, because the exit and
  clippy builds enable every feature) to show the checks observe what they
  claim; rerun every direct contract
  suite (`c1_contracts` to `c6_contracts`, `checker_semantics`,
  `checker_display`, `checker_baselines`, `phase2_subtests`) in debug and
  release with fresh v2 receipts; rerun the E2 obligations replay, the E3
  scenarios (harness and compiler pool) and the relater parity; confirm the
  module-resolution trace rows (154) match.
- Exit: `ordering == 1`, `parent_pointers == 1`, `trace_parity == 1`; every
  receipt current; the two negative witnesses fail as intended.

### C7.3 Divergences and approved scope

- Exists: `data/divergences.toml`, empty; the E2 producer's validation of
  witnesses.
- Build: `phase2_divergences.py check` validates every entry against the
  final comparison: scope, kind, rationale, approval, pin, and one
  observation witness per variant and metric whose native and Rust digests
  equal the comparison's; unused or changed witnesses fail the check;
  failures, unsupported and native-unavailable rows are rejected as entries.
  C7 expects the ledger to stay empty; a proposal is written as a draft
  entry with its witnesses and goes to the owner.
- Exit: `c7_divergences_valid` true.

### C7.4 The operation and dependency disposition

- Exists: `PORTS.toml`, the six audits, the claims files, the blocker
  register, the inherited Phase 1 items (the 23 loader-side project-reference
  operations stay Phase 1; the fourth scoped `typeParameterSymbolList` went to
  C5).
- Build: (a) the ledger: every Phase 2 file (`internal/checker`, the
  `pseudochecker` files, `compiler/checkerpool.go`, the `core/workgroup.go`
  share) set `ported` with its Rust paths and `verify` checks bound to
  `run.checker` metrics (`errors_parity == 1`, `types_parity == 1`,
  `symbols_parity == 1` for checker files; `display_parity == 1` for the
  node-builder files; `c6_mode_parity == true` for the pool files;
  `c5_services == true` for `services.go`; no Phase 2 file binds
  `trace_parity`, whose traced behavior is the Phase 1 resolver, so that gate
  stays a sprint exit metric only), so `cargo xtask status` derives `verified`; (b) the
  functions: `data/phase2/dispositions.json`, generated by
  `phase2_dispositions.py build` from `data/go-functions.tsv`, the port
  markers and the six audits, giving every function of the Phase 2 packages
  one disposition (`mapped` with its marker site, `equivalent` with its Rust
  site and reason, `later` with its phase and owner) and rejecting a function
  with none; the unmapped worklist regenerated; (c) the blocker register
  rebuilt for the last time, holding only cross-phase joint entries with
  owner, kind, operation, variants and domains; (d) the handoffs to Phase 3,
  4 and 5 consolidated from the claims files with their validator fields;
  (e) the Phase 1 inheritances closed or returned with a named owner.
- Exit: `c7_dispositions` true; `cargo xtask status` shows the Phase 2 files
  verified; `cargo xtask validate` passes.

### C7.5 The dashboard and the C7 record

- Exists: the comparison's `dimensions` (outcomes by family, suite, options,
  configuration), `by_checkpoint` and `buckets`; the `docs/status.html`
  renderer; the checkerbench, E5 and E6 captures.
- Build: `phase2_report.py build` writes `docs/PHASE2-C7.md` and
  `data/phase2/c7-report.json` (digest-bound to the recorded captures):
  the per-area pass-rate dashboard PLAN's Phase 2 tooling names (rates by
  family, by checkpoint, by suite and by configuration dimension, for both
  modes), the final counts per domain, the residual and divergence lists,
  the disposition summary, and two reports:
  - what Phase 3, 4 and 5 can consume: the emit resolver and the
    declaration-diagnostics path with B09's state (Phase 3), the compiler
    pool, the cancellation token, the trace-sink seam and the `checkers` and
    single-threaded switches (Phase 4), the services operations, the recorded
    fourslash replay, hover expansion, the project pool's cancellation
    disposal and the public API (Phase 5 and 6), the creation-trace mode and
    the assignment witnesses (diagnostics), the E3 contracts through the
    compiler pool;
  - the measured performance risks Phase 7 owns: the C2 checkerbench capture
    (`data/phase2/c2-benchmark.json`: elapsed 2.136, requested bytes 0.574,
    retained bytes 1.415, type-storage bytes per reachable type 0.813, Rust
    over Go, with its `host_busy` qualification), the last E5 figures against 0.85 (ADR 0022) and
    the last E6 figures against 1.25 and 1.45 (ADR 0021), each with its
    capture date and host, with the statement that full checking remains
    extrapolated at those gates and that the multi-checker speedup is
    unmeasured until Phase 7's benchmarking scenarios; no conversion between
    elapsed time, retained bytes and peak RSS; no new benchmark unless the
    owner runs one (decision 5).
  The status renderer includes the dashboard section from the JSON.
- Exit: `c7_report` true; the record's digests equal the recorded captures.

### C7.6 The phase-end green-up and the recordings

- Exists: every producer's fingerprint rules; the memory that mid-phase
  staleness is expected and cleared at the phase end; the CI `status`
  workflow on both runners.
- Build: rerun every producer whose fingerprint Phase 2 changed and record
  it: `workspace`, `oracle`, `fmt`, `clippy`, `deny`, `config`, `binder`,
  `checkertext`, `e1`, `e2`, `e3`, and the owner's `checkerbench` on the
  quiet host, `bindworkload` and `benchmark` in the S07 chain order, and
  `checker` in both modes; `cargo xtask status --record`; `cargo xtask
  check P2A` and `check P2B`; `status --check-committed` in CI; S01 to S06
  still close on each runner's evidence; the Linux host capture owed since
  Phase 1 is taken for the assignment witnesses and the producers that run
  there.
- Exit: `c7_evidence_current` true; both runners green; the sprints close.

### C7.7 Producer wiring

- Exists: the per-checkpoint helper; `P2B-C7` waiting on
  `run.checker.c7_complete`.
- Build: the producer reads `data/phase2/residuals.json`,
  `informational.json`, `dispositions.json`, `c7-report.json`, the
  divergence ledger and the evidence states of a fingerprinted set of other
  prerequisite producers (the C7.6 list without `checker`: the tracker writes
  `checker.latest` as an incomplete attempt before running the producer, so
  the producer cannot certify its own run, and reading previously generated
  status would certify an old snapshot; the tracker establishes the checker
  run's own freshness after recording it), and emits `c7_residuals`,
  `c7_informational_listed`, `c7_dispositions`, `c7_divergences_valid`,
  `c7_evidence_current`, `c7_report` and `c7_complete` (the P2B exit
  conjunction, every `P2B-Cn` item closed on a recorded run, and the six
  above). The tracker extension: `cargo xtask check` accepts
  `recorded.<run>.<metric> == <value>` in an item's `done_when`, satisfied by
  a recorded evidence artifact of that run whose report holds the value, and
  the sprint view names the artifact. All new authorities are `[checker]`
  inputs. C7.7 lands first so the metrics exist while the residuals close.
- Exit: `scripts/tests/test_phase2_c7.py` shows that one residual without an
  approved scope keeps `c7_complete` false, that an unused divergence witness
  fails `c7_divergences_valid`, that a function without a disposition fails
  `c7_dispositions`, that a stale prerequisite producer fails
  `c7_evidence_current` while the checker run itself is excluded, that a
  complete stale-to-current recording sequence (rerun, record, regenerate the
  status views, check) ends current, and that changing each new input
  invalidates the recorded result.

## 5. Dependencies and owners

| Dependency | Owner | State for C7 |
| --- | --- | --- |
| C1 to C6 complete on the final head | the checkpoints | C7 starts its wiring and skip list early; its closure waits for C6 |
| Content-mapper execution (register entry B04 at the C2 exit, B06 in the C1 capture; 15 executed C3 rows) | Phase 5 | decision 1: pulled forward as a C7 prerequisite, or named in an owner-approved amendment of the P2B exit |
| The emit-order rows (B05 at the C2 exit, B09 in the C1 capture) | C5 with Phase 3 | closed under C5's decision 3, or named in the same amendment |
| The Linux host capture and the loader-side project-reference operations | Phase 1 | the capture is taken in C7.6; the operations stay Phase 1 with a named owner in the disposition |
| The owner's recordings (`checker` in both modes, `e3`, `checkerbench`, `bindworkload`, `benchmark`, `status --record`) and the quiet host | owner | C7.6 |
| Owner decisions of section 9 | owner | before C7.1 |

## 6. Delivery order

1. C7.7 the wiring and C7.0 the skip list, as soon as C6's plan is accepted;
   both are independent of the checkpoints' state.
2. C7.1 the residual list, refreshed at every checkpoint exit from C3 on, so
   the joint residuals reach the owner early (decision 1).
3. C7.4 the disposition, built incrementally as each audit closes; the
   ledger `verify` bindings land with C6's ledger move.
4. After C6: C7.2 the validation on the final executable, C7.3 the ledger
   check, C7.5 the report, then C7.6 the green-up and the recordings.

Full runs are the two final runs of C7.2 (single-threaded and concurrent);
everything else replays recorded captures.

## 7. Executable exit checks

```sh
export PATH="$(mise where go)/bin:$PATH"
python3 scripts/phase2_inventory.py check
python3 scripts/phase2_inventory.py informational --check                     # data/phase2/informational.json matches the inventory
python3 scripts/phase2_native.py verify --capture target/phase2/native --shards 7 --scheme interleaved --jobs 7
python3 scripts/phase2_native.py verify --capture target/phase2/native-concurrent --shards 7 --scheme interleaved --jobs 7
python3 scripts/phase2_corpus.py run --native target/phase2/native --output target/phase2/rust-c7
python3 scripts/phase2_corpus.py run --native target/phase2/native-concurrent --output target/phase2/rust-c7-concurrent --mode concurrent
python3 scripts/phase2_compare.py report --native target/phase2/native --rust target/phase2/rust-c7 --record
python3 scripts/phase2_compare.py report --native target/phase2/native-concurrent --rust target/phase2/rust-c7-concurrent
python3 scripts/phase2_compare.py modes --rust target/phase2/rust-c7 --rust-concurrent target/phase2/rust-c7-concurrent
python3 scripts/phase2_blockers.py build --native target/phase2/native --rust target/phase2/rust-c7 --record   # cross-phase joint entries only
python3 scripts/phase2_residuals.py build --rust target/phase2/rust-c7 --check                                 # c7_residuals 0
python3 scripts/phase2_divergences.py check --rust target/phase2/rust-c7
python3 scripts/phase2_dispositions.py build --check
for n in 1 2 3 4 5 6; do python3 scripts/phase2_audit.py check --audit "data/phase2/c${n}-audit.json"; done
for w in c1-contracts c2-contracts c3-contracts c4-contracts c5-contracts c6-contracts; do python3 scripts/phase2_producers.py observe --witness "$w"; done   # each suite with its receipt's exact feature set, debug and release
python3 scripts/phase2_services.py verify && python3 scripts/phase2_services.py replay --output target/phase2/services-c7
python3 scripts/phase2_assignments.py compare
python3 scripts/s08_e2.py obligations --output target/s08/e2-c7
python3 scripts/s08_ownership.py
python3 scripts/s08_relater.py build  --output target/s08/relater-c7 && python3 scripts/s08_relater.py parity --output target/s08/relater-c7
python3 scripts/phase2_report.py build --rust target/phase2/rust-c7 --rust-concurrent target/phase2/rust-c7-concurrent
python3 scripts/phase2_producers.py checker --rust target/phase2/rust-c7 --rust-concurrent target/phase2/rust-c7-concurrent
python3 -m pytest scripts/tests -q
python3 scripts/checks.py fmt && python3 scripts/checks.py clippy
cargo xtask validate
cargo xtask check P2A && cargo xtask check P2B                                # after the owner's recordings
cargo xtask status --check-committed
```

`cargo xtask run checker` (both modes), `run e3`, `run checkerbench`, the S07
chain (`bindworkload`, `benchmark`), `cargo xtask status --record` and the
Linux host capture are the owner's.

## 8. Evidence reuse rules

- Both native captures are reused only when their read-only validators
  succeed at the final pin; C7 takes no new native capture unless a canonical
  input changed.
- The final Rust captures are fresh; every recorded metric derives from them
  and from replayed, source-stable captures; a sample never feeds a C7
  metric.
- A retained difference exists only as an approved divergence with its
  witnesses or as a joint blocker named by an owner-approved sprint-exit
  amendment; a status label, a bucket or a free-text owner is never a scope.
- Recorded evidence is current only by fingerprint; C7 clears staleness by
  rerunning producers, never by editing views.
- The performance report cites recorded captures by digest and date; it
  introduces no threshold and converts no measure into another.
- Mapping progress is reported by markers and the ledger, never as behavioral
  coverage; the two-mode corpus comparison, the sub-tests, the contracts, the
  replay and the E3 scenarios are the behavioral evidence.

## 9. Owner decisions before C7 starts

1. **The content-mapper rows.** `unsupported_required == 0` cannot hold
   while the content-mapper entry withholds 15 executed rows, and the divergence ledger cannot
   waive an unsupported operation. Either Phase 5's content-mapper execution
   (the child-process plugins over JSON-RPC and their span maps) is scheduled
   as a C7 prerequisite, or the owner amends the P2B exit to name the
   content-mapper entry as an explicit joint blocker with its 15 rows.
   Proposed: amend, with the rows listed by identity, and keep the same
   choice available for the emit-order entry if C5's decision 3 is declined.
2. **Recorded completion.** The P2A and P2B exit lists close on the final
   recorded `checker` run; each `P2B-Cn` item closes on the run recorded at
   its own exit (C2's already is) through the tracker extension of C7.7.
   Where an item was never recorded on its own exit run (the C0 and C1
   recordings were owed), the item closes on the earliest recorded run whose
   report holds its metrics, and the C7 record names that run. Confirm the
   extension and this rule.
3. **Ledger `verify` bindings.** The metric-per-file bindings of C7.4, so
   that `verified` derives from `run.checker` evidence rather than from a
   hand-written status. Confirm the mapping.
4. **The dashboard's home.** `scripts/phase2_report.py` writes
   `docs/PHASE2-C7.md` and a JSON the status renderer includes, rather than a
   hand-maintained page. Confirm.
5. **The performance report.** Only recorded captures are cited (the C2
   checkerbench capture, the last E5 and E6); no new benchmark is run for C7
   unless the owner runs one on the quiet host. Confirm.
6. **The green-up set and hosts.** The producers listed in C7.6 rerun on the
   macOS runner and on the Linux host; the owner producers are the owner's.
   Confirm the set and who runs what.
7. **Exit recording and Phase 7.** `c7_complete` is computed only from the
   recorded runs; C7 does not start Phase 7's four-week acceptance, it hands
   over the report. Confirm.
