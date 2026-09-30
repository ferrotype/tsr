# Phase 2 C7: full correctness and readiness

Checkpoint C7 of the [Phase 2 plan](PHASE2-plan.md). It was drafted with the
C3 to C6 plans over the C2 exit. This revision starts from the recorded C6
exit ([PHASE2-C6.md](PHASE2-C6.md)):

- `target/phase2/rust-c6` (single-threaded) and
  `target/phase2/rust-c6-concurrent` each match 13,417 of 13,432 executed rows
  in every domain;
- all 9,367 regression rows match, and there are 0 outcome differences
  between the modes;
- the only rows not matching are the 15 content-mapper rows of register entry
  B01;
- the owner's `checker` run `afc6eeb4` (at `886d9a90`) reports
  `c6_complete = true`.

Upstream is Corsa `1f70213d4922b434345f639b441681e470c7cfc1`.

C7 is closure work: it adds no checker semantics of its own. It drives every
remaining difference to a checkpoint, runs the complete acceptance on the
final inputs in both test-program modes, publishes the operation and
dependency disposition, records the evidence that closes sprints P2A and P2B,
and reports what Phase 3, 4 and 5 can consume and what Phase 7 still owns.
By the owner's decision 1 (2026-09-30), it also pulls one Phase 5 capability
forward: content-mapper execution (C7.8), so the last 15 rows run and must
match instead of being named in an exit amendment. The owner's decisions are
recorded in section 9.

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
| The gate holds in the concurrent mode too: the concurrent native capture is verified, a complete concurrent Rust run is valid, and the two runs have no outcome difference | `run.checker.native_verified_concurrent == true`, `harness_valid_concurrent == true`, `mode_parity == true`, computed on every run by C7.7 (C6's own metrics are computed only while C6 is the current checkpoint) |
| Every checkpoint closed on a recorded run | each `P2B-Cn` item's `done_when` holds on a recorded `checker` run identified by its evidence id (the tracker extension of C7.7; the runs are listed in section 2 and decision 2); the final run does not recompute `cN_complete` or `c2_measured` |
| Content-mapper execution is ported and B01's 15 rows match in every domain in both modes; the register is empty | `run.checker.unsupported_required == 0` (already in the P2B exit list), `run.checker.c7_content_mappers == true` (C7.8) |
| No executed row differs in any domain without an owner-approved scope | `run.checker.c7_residuals == 0` over `data/phase2/residuals.json` |
| The runner's skips are listed explicitly with their native guard reasons and stay informational (S12 matrix) | `run.checker.c7_informational_listed == true` over `data/phase2/informational.json` |
| The operation and dependency disposition is published: every Phase 2 ledger file `ported` with `verify` checks that derive `verified`, every function of the Phase 2 packages mapped, equivalent or handed with an owner, the blocker register holding only cross-phase joint entries with owners, the divergence ledger valid against the final comparison | `run.checker.c7_dispositions == true`, `c7_divergences_valid == true`; `cargo xtask status` derives `verified` for the Phase 2 files |
| The phase-end green-up is done: no stale evidence, both runners green, S01 to S06 still close | `run.checker.c7_evidence_current == true`; `cargo xtask check P2A` and `check P2B` pass; `status --check-committed` passes in CI |
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
| The gate wiring | `sprints/P2A.toml` (exit: the five capture-integrity metrics), `sprints/P2B.toml` (exit: `sprint.P2A.done == 1`, `harness_valid`, the seven parity and sub-test metrics, `unsupported_required == 0`; items C1 to C7 bound to `run.checker.cN_complete`); `status/runs.toml` `[checker]` (`phase2_producers.py checker`, which reads both modes' captures through `--native`, `--rust`, `--native-concurrent` and `--rust-concurrent`, its inputs and sources); `cargo xtask check P2A|P2B`, `check-metrics`, `run <id>`, `status --record`, `status --check-committed`, `validate` | the exit C7 closes and the commands that assert it |
| The current gap | the C6 exit's categories, identical in both modes: `errors` 13,417 match and 15 unsupported; `types`, `symbols` and `display` 12,743 match, 10 unsupported and 679 native-disabled; `parent_pointers` and `union_ordering` 13,417 match and 15 unsupported; `trace` 154 match and 13,278 disabled. One unsupported operation remains (content-mapper execution), and 13,417 rows match in every domain. 26 of the 27 recorded runs read `stale` in `status/status.json`, and only `checker` is current | the distance left, all of it B01, and the staleness C7.6 clears |
| The denominator and its skips | `data/phase2/inventory.json`: 15,206 effective variants, 13,432 executed (`native_selection: runs`), 1,774 informational (1,720 `option_guard_skip`, 52 `filename_skip`, 2 `not_enumerated`; 39 rejected-option rows are a subset of the filename skips); the guard-only option keys (`baseUrl` on 38 rows, `moduleSuffixes` on 15) | the explicit skip list C7.0 publishes |
| The native contract | `target/phase2/native` (`data/phase2/native-provenance.json`), retaken when C6.1 moved the pool's ledger entry, with row digests identical to C0's (the C0 capture is kept at `target/phase2/native-c0`): 13,432 executed, 0 input mismatches, 0 reference disagreements, 0 parent-pointer failures, 0 inconsistent unions, 3 pre/post-emit differences, 154 trace rows, 1,754 declaration requests. The concurrent capture `target/phase2/native-concurrent` (`data/phase2/native-provenance-concurrent.json`) differs from it only in union ordering's checker and union counts | the final inputs; reused only when the read-only validators accept them |
| The sub-tests | `tools/phase2/subtests.rs`: union ordering (every interned union reproduced by sorting its reversed list and ten seeded shuffles with the production comparator; one checker in the single mode, every checker of the pool in the concurrent mode, as the pin's `ForEachCheckerParallel`), parent pointers (below each non-default-library root every node the generated child visitor reaches has the traversal parent as its recorded parent; the root is not checked; the walk stops at its first failure), module-resolution trace (the loader's trace localized and sanitized as the baseline tracer writes it); `phase2_compare.py compare_subtest` compares verdicts, not counts | the Rust definition C7 validates; it does not redefine it |
| The lifecycle and recursion witnesses | the C1 to C6 contracts (among them the cycle diagnostic, repeated-query identity, relation caches, two checkers over one program, the deep relation and deep flow graph on the E2 small stack and on pool threads, the pin's semantic limits, panic retirement in one checker and across a pool, cancellation, tracing, the two modes), each with its receipt; E3's criteria in the ownership harness and, since C6.6, the `compiler_pool` suite of `data/s09/ownership-cases.json` over the compiler pool in the debug, release, Miri and AddressSanitizer lanes; the E2 obligations replay (8 residual families, 7 matrices, 77 permutations) | rerun on the final executable in C7.2 |
| The divergence ledger | `data/divergences.toml` (ADR 0004): no entries; an entry needs an exact observation witness per variant and metric; failures, missing operations and native-unavailable cases cannot be waived; unused witnesses block | the only way a retained difference can be approved |
| The ledger and the function inventory | `PORTS.toml`: 32 Phase 2 files (the 25 `internal/checker` files, `compiler/checkerpool.go` since C6.1, the five `modulespecifiers` files and `nodebuilder/types.go`), 25 `planned` and 7 `in-progress`, none `ported`, almost none with Rust paths. The `pseudochecker` files are Phase 3, and `core/workgroup.go` is Phase 1 (C6 ported its `WorkGroup` half into `tsr_core`). `status/unmapped-functions.json`: `internal/checker` 2,879 functions, 73.3% mapped (769 unmapped); `internal/compiler` 340, 50.0% (170). The C1 to C6 audits (`data/phase2/cN-audit.json`: `mapped`, `equivalent`, `later` with owner), all complete | the disposition C7.4 publishes |
| The claims, blockers and handoffs | `data/phase2/cN-claims.json` for C1 to C6. C3 has 15 `blocked` content-mapper handoffs. C2 has 3 `handed` emit-order rows, now matching since C5's post-emit schedule, whose C5 `incoming` entries are closed. C6 has no rows and records the two native modes. `data/phase2/blockers.json` is rebuilt from evidence, never edited by hand; its only entry is B01 (content-mapper execution, Phase 5, 15 executed C3 rows). The emit-order entry closed at the C5 exit | the residual list C7.1 consolidates |
| The assignment witnesses | `data/phase2/c6-assignments.json` (the pin's partition of every corpus program in the concurrent mode and 27 synthetic graphs, on darwin/arm64), `data/phase2/c6-assignment-comparison.json` (the Rust partitioner equal on 13,417 programs and every synthetic graph), `data/phase2/c6-fusion-arm64-go1.27.1.txt` | rerun on the final sources; the Linux host capture joins them in C7.6 |
| Content-mapper execution | The pin runs each mapper as a child process over JSON-RPC (`internal/contentmapper`: `contentmapper.go`, `host.go`, `hostimpl.go`, `transform.go`, about 2,000 lines; Phase 5 in the ledger). It uses `internal/ipc` (972 lines) and `internal/jsonrpc` (287 lines, both Phase 6) and `internal/spanmap` (818 lines, Phase 5, in progress). Tests opt in with `// @runExternalCode: true`. `harnessutil` then serves the mappers in process through `testutil/contentmappertest` (18 mappers, about 1,600 lines, Phase 1): the 15 rows use `compiler-test-mapper` (9 rows), `failing-mapper`, `lisp-mapper`, `supplemental-mapper`, `supplemental-diagnostics-mapper`, `supplemental-globals-mapper` and `supplemental-module-mapper`. `testutil/tsbaseline/contentmapper_baseline.go` is Phase 1. The compiler integration is in `compiler/fileloader.go`, `program.go` and `emitter.go`. In Rust, `tsr_tsoptions` parses `contentMappers`; the loader refuses a mapped program (`Error::Unsupported("content-mapper execution")`), and the corpus driver refuses a mapped program's error selection; no `tsr_contentmapper`, `tsr_ipc` or `tsr_jsonrpc` crate exists yet | C7.8's port and its ledger move |
| Performance captures | `data/phase2/c2-benchmark.json`: the owner's C2 checkerbench over the S08 workload, 2026-09-26, `host_busy`. Rust over Go: elapsed 2.136, allocated bytes 0.574, retained bytes 1.415, type footprint 0.813. Also `[e5]` (peak RSS, allocated bytes and the type footprint against 0.85, ADR 0022) and `[e6]` (parse and bind wall time against 1.25 and 1.45, ADR 0021); full checking remains extrapolated at those gates | the figures C7.5 reports side by side |
| CI | the `status` workflow. Its failing steps at the C6 head (run `36624002745`), identical on both producer runners: the Phase 1 manifest, harness and producer contracts (the coverage report and the syntax captures record Rust closures that the Phase 2 sources changed; the syntax schedule is stale against `data/upstream.json` after the ledger move); the Phase 1 operation audit; the binder, program and configuration contracts, where the C2 order contract's fixture still bound the pre-C6 trace build (fixed with this plan's revision); config, options and program loading for every frozen variant; the build, smoke and quality results; S03 to S06 closing on the runner's evidence. The committed-views check turns green once a recording is committed | the phase-end green-up C7.6 targets on both runners |

C7 uses fresh capture directories for its final runs; nothing is moved or
overwritten. The recorded `checker` run is the owner's.

**Recorded completions so far.** The recorded `checker` runs in
`status/evidence` hold:

| Item | Recorded run | What it holds |
| --- | --- | --- |
| P2B-C1 | `96b7c65f` (2026-09-26, `61140fd9`), again in `f9e38821` and `815eeb43` | `c1_complete = true` |
| P2B-C2 | `74c7960a` (2026-09-26, `64ade65e`) | `c2_complete = true`, `c2_measured = true` |
| P2B-C3, P2B-C4 | no run holds `c3_complete` or `c4_complete`: C3 and C4 were never recorded while they were the current checkpoint | `815eeb43` (2026-09-28, the C5 exit re-recorded at `6548b48d`) holds their whole accounting: open 0, regressions 0, failures 0, open blockers 0, handoffs 0, audit complete and contracts true, with `regression_parity = 1` and every prerequisite true |
| P2B-C5 | `f9e38821` and `815eeb43` | `c5_complete = true`, `c5_services = true` |
| P2B-C6 | `afc6eeb4` (2026-09-30, `886d9a90`) | `c6_complete = true` |

Since the ledger move, the C1 to C5 baselines are history. They bind the
C0-era native capture, and the owner chose to keep them rather than
re-derive them (2026-09-29), so current runs report C1 to C5 accounting as
unavailable. `P2B-C1` and `P2B-C5` read pending in `STATUS.md` until the
tracker extension of C7.7 lets their items name these runs; `P2B-C2` to
`P2B-C4` already did.

## 3. What C7 owns

C7 owns no rows and no semantic cause. It owns closure:

| Unit | State at the C7 start | C7's obligation |
| --- | --- | --- |
| Rows that still differ in any domain | the 15 content-mapper rows of B01, withheld as unsupported in `errors`, `parent_pointers` and `union_ordering` (and in the walker's domains on the 10 of them that request baselines) | closed by C7.8: the rows run and match in both modes; `data/phase2/residuals.json` lists any row still differing with its owning checkpoint |
| Content-mapper execution | Phase 5 and 6 in the ledger, unported | pulled forward (decision 1): the files the 15 rows execute move to Phase 2 and are ported, audited and contracted in C7.8 |
| The informational rows | 1,774, listed in the inventory with their native selection | published with the guard reason and the option keys that trigger the guard |
| The sub-tests and lifecycle contracts | passing per checkpoint, with current receipts at the C6 head | validated once more on the final executable, in both modes |
| The divergence ledger | empty | validated against the final comparison; entries only with owner approval and witnesses |
| The ledger, the function inventory, the blockers, the handoffs | per checkpoint; no Phase 2 ledger file `ported` | one published disposition |
| The recorded evidence | 26 of 27 runs stale | the phase-end green-up and the owner's recordings |
| The report | none | `docs/PHASE2-C7.md` and the status views |

The ownership rule of the checkpoint plans applies unchanged: C7 never marks
a row closed by editing a status; a row closes when the final comparison shows
it matching, and a difference is retained only through the divergence ledger
(decision 1 ruled out a sprint-exit amendment).

**Completion and handoffs across captures.** Every handoff and the C2
measurement are bound to one Rust capture: the blocker builder's
`validated_handoffs` drops a handoff whose `capture_sha256` differs from the
current comparison's, and `c2_measured` binds the checkerbench record to the
corpus capture it was verified against. Two rules follow. First, a
checkpoint's completion is a recorded historical fact: `P2B-Cn` closes on a
recorded `checker` run (section 2's table and decision 2), and no later
checkpoint recomputes `cN_complete` or `cN_measured` on its own capture.
Second, at each final capture `phase2_claims.py rebind` re-validates every open
handoff: the row's new raw observation must still differ only in the covered
domains, and the capture, request, observation and trace digests are
rewritten in both the handing checkpoint's claims file and the receiving
checkpoint's `incoming` entry. A request or raw observation that changed
goes to fresh attribution rather than an automatic rebind.

Rebinding uses the single-threaded capture. Concurrent-mode raw observations
are not stable from run to run in type ids alone. The pin lets the per-file
declaration and emit tasks that share a checker reach it in any order, and
two concurrent captures of the C6 follow-up differed on 3 rows, only in
swapped numeric type ids of the walker's query trace
(`jsFileCompilationEmitTrippleSlashReference` and
`nodeModulesGeneratedNameCollisions` configurations 1 and 3). The two-mode
comparison therefore compares outcomes, never raw digests.

## 4. Work items

### C7.0 Final inputs, the denominator and the skip list

- Exists: the inventory, both native captures and their validators; the
  inventory's `informational_reason`, `native_selection`,
  `config_option_keys` and `options` per row.
- Build: `phase2_inventory.py check` at the final pin; `phase2_native.py
  verify` on the single-threaded capture and `phase2_native_concurrent.py
  verify` on the concurrent one; `data/phase2/informational.json` listing every
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
  against the final single-threaded capture, so every open handoff is
  re-validated, then reads the final comparisons of both modes and every
  claims file and writes `data/phase2/residuals.json`: each row not matching
  in some domain in either mode, with the checkpoint or phase that owns its
  cause, the validated trace that attributes it, the blocker identity if one
  withholds it, and the resolution path (`checkpoint` when a `cN_open`
  reopens, `divergence` when an ADR 0004 entry is proposed, `joint` when the
  cause is outside Phase 2). At the C6 exit the list is B01's 15 rows, which
  C7.8 closes. A new residual goes back to its checkpoint and reopens it.
- Exit: `c7_residuals == 0`, meaning every executed row matches in every
  domain or carries an approved divergence witness. No sprint-exit amendment
  names a joint blocker (decision 1).

### C7.2 Sub-tests, lifecycle and recursion validation

- Exists: `tools/phase2/subtests.rs`, `phase2_compare.py compare_subtest`;
  the C1 to C6 contracts and receipts; the E2 obligations replay; the E3
  scenarios; the relation contract.
- Build:
  - Run the union-ordering and parent-pointer sub-tests on the final
    executable in both modes. Make the parent-pointer report name the first
    failing node's kind, file and position, so a failure is attributable.
  - Add one direct witness per sub-test that fails on purpose, to show the
    checks observe what they claim. Inject it through a test-only path of
    `tools/phase2/subtests.rs`: a comparator override and a parent override
    that the test passes in. Never use a cargo feature, because the exit and
    clippy builds enable every feature.
  - Rerun every direct contract suite in debug and release, each with its
    witness's exact feature set, with fresh v2 receipts: `c1_contracts` to
    `c6_contracts`, and C2's with `recursion-probe,creation-trace`, which
    compiles the order-contract module a plain `recursion-probe` build leaves
    out. Also rerun `checker_semantics`, `checker_display`,
    `checker_baselines` and `phase2_subtests`.
  - Make `phase2_producers.py observe` fail when a recorded run fails. Today it
    writes a receipt with the failing exit codes and succeeds, and only
    `receipt_current` notices. The C6 exit refreshed a failing C2 receipt
    this way unnoticed.
  - Rerun the E2 obligations replay, the E3 scenarios (the ownership harness
    and the `compiler_pool` suite, in all four lanes) and the relater parity.
    Confirm the module-resolution trace rows (154) match.
- Exit: `ordering == 1`, `parent_pointers == 1`, `trace_parity == 1` in both
  modes; every receipt current; the two negative witnesses fail as intended.

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
- Build:
  - (a) **The ledger.** Set every Phase 2 file `ported` (the 32 of section 2
    and the content-mapper files C7.8 moves), with its Rust paths and
    `verify` checks bound to run-level `run.checker` metrics, so that
    `cargo xtask status` derives `verified`:
    - the `internal/checker` files: `errors_parity == 1`, `types_parity == 1`
      and `symbols_parity == 1`;
    - the node-builder files, `printer.go`, the five `modulespecifiers` files
      and `nodebuilder/types.go`: `display_parity == 1`;
    - `compiler/checkerpool.go`: `mode_parity == true` and
      `assignments == true`;
    - `services.go`: `services == true`;
    - the content-mapper files C7.8 moves: `content_mappers == true`, the
      run-level form of C7.8's metric.

    C7.7 computes `mode_parity`, `assignments`, `services` and
    `content_mappers` on every run;
    completion metrics such as `c5_services` are not computed once their
    checkpoint is past. No Phase 2 file binds `trace_parity`, whose traced
    behavior is the Phase 1 resolver, so that gate stays a sprint exit metric
    only.
  - (b) **The functions.** `data/phase2/dispositions.json`, generated by
    `phase2_dispositions.py build` from `data/go-functions.tsv`, the port
    markers and the six audits, gives every function of the Phase 2 packages
    one disposition. That is `mapped` with its marker site, `equivalent` with
    its Rust site and reason, or `later` with its phase and owner, and a
    function with none is rejected. The unmapped worklist is regenerated.
  - (c) **The register.** Rebuilt for the last time. With C7.8 done, it is
    expected to be empty; any entry left must be a cross-phase joint entry
    with owner, kind, operation, variants and domains, and it keeps
    `c7_residuals` above 0.
  - (d) **The handoffs.** The handoffs to Phase 3, 4 and 5 are consolidated
    from the claims files with their validator fields.
  - (e) **The Phase 1 inheritances.** Each is closed or returned with a named
    owner.
- Exit: `c7_dispositions` true; `cargo xtask status` shows the Phase 2 files
  verified; `cargo xtask validate` passes.

### C7.5 The dashboard and the C7 record

- Exists: the comparison's `dimensions` (outcomes by family, suite, options,
  configuration), `by_checkpoint` and `buckets`; the `docs/status.html`
  renderer; the checkerbench, E5 and E6 captures.
- Build: `phase2_report.py build` writes `docs/PHASE2-C7.md` and
  `data/phase2/c7-report.json` (digest-bound to the recorded captures).
  They contain the per-area pass-rate dashboard that PLAN's Phase 2 tooling
  names (rates by family, by checkpoint, by suite and by configuration
  dimension, for both modes), the final counts per domain, the residual and
  divergence lists, the disposition summary, and two reports:
  - **What Phase 3, 4 and 5 can consume:**
    - the emit resolver's JavaScript half and the declaration-diagnostics
      path, with the harness's post-emit schedule (Phase 3);
    - `tsr_compiler::CheckedProgram`, the program's checker driving through
      the compiler pool or a supplied one, with `CheckerRequest` carrying the
      checker lifetime and the cancellation (Phase 4 and 5);
    - `CompilerCheckerPool`, with its count rule and the per-architecture
      FENNEL partition (Phase 4);
    - `tsr_core::CancellationToken` and the poisoned-checker contract (Phase 4
      and 5);
    - the `TraceSink` seam with its in-memory and JSON-lines sinks, whose file
      writer is Phase 4's;
    - the bounded `tsr_core::workgroup::WorkGroup` (Phase 4);
    - the `checkers` option and the program's own single-threaded setting
      (Phase 4);
    - the services operations, the recorded fourslash replay and hover
      expansion (Phase 5 and 6);
    - the content-mapper host, the IPC and JSON-RPC layers and span maps,
      ported for the harness's in-process mappers (C7.8), which the project
      system's child-process spawner completes (Phase 5);
    - the project pool's `CheckerPool` implementation and its disposal of a
      canceled checker (Phase 5);
    - the public API (Phase 6);
    - the creation-trace mode and the assignment witnesses (diagnostics);
    - the E3 contracts through the compiler pool.
  - **The measured performance risks Phase 7 owns:**
    - the C2 checkerbench capture (`data/phase2/c2-benchmark.json`: elapsed
      2.136, allocated bytes 0.574, retained bytes 1.415, type footprint
      0.813, Rust over Go, with its `host_busy` qualification);
    - the last E5 figures against 0.85 (ADR 0022) and the last E6 figures
      against 1.25 and 1.45 (ADR 0021), each with its capture date and host.

    State that full checking remains extrapolated at those gates, and that
    the multi-checker speedup is unmeasured until Phase 7's benchmarking
    scenarios. The corpus runs start a process per variant and measure no
    speedup. Do not convert between elapsed time, retained bytes and peak
    RSS, and run no new benchmark unless the owner runs one (decision 5).

  The status renderer includes the dashboard section from the JSON.
- Exit: `c7_report` true; the record's digests equal the recorded captures.

### C7.6 The phase-end green-up and the recordings

- Exists: every producer's fingerprint rules; the rule that mid-phase
  staleness is expected and cleared at the phase end; the CI `status`
  workflow on both runners; the staleness the C6 ledger move left
  (`data/upstream.json` is an input of 22 recorded runs, of Phase 1's five
  native captures and the syntax schedule, of the S07 manifests and of the C2
  order traces).
- Build:
  - Rerun and record every stale run: `binder`, `bindworkload`,
    `checkerbench`, `checkertext`, `clippy`, `config`, `deny`, `e1` to `e8`,
    `fmt`, `foundations`, `gen`, `oracle`, `program`, `relater`, `scanner`,
    `selftest`, `syntax`, `testhost` and `workspace`, and `checker` in both
    modes. `checkerbench` runs on the quiet host, and `bindworkload`, `e5` and
    `e6` run in the S07 chain order. Which runs are the owner's is decision 6.
  - Refresh the Phase 1 captures that bind the ledger-moved
    `data/upstream.json` or the Phase 2 Rust closure: the coverage report, the
    syntax smoke and full captures, and the syntax schedule.
  - `cargo xtask status --record`; `cargo xtask check P2A` and `check P2B`;
    `status --check-committed` in CI.
  - S01 to S06 still close on each runner's evidence. Every failing step of
    section 2's CI row turns green on both runners.
  - Take the Linux host capture owed since Phase 1, for the producers that run
    there and for the assignment witnesses (`phase2_assignments.py record` and
    `compare` on linux/amd64, where the score arithmetic is unfused).
- Exit: `c7_evidence_current` true; both runners green; the sprints close.

### C7.7 Producer and tracker wiring

- Exists: the per-checkpoint helper; `drop_historical_completion`, which keeps
  a past checkpoint's accounting and drops its `_complete`, `_measured` and
  `_services`; C6's `concurrent_metrics`, computed only while C6 is current;
  `P2B-C7` waiting on `run.checker.c7_complete`.
- Build:
  - **Run-level metrics.** Compute the properties of the final inputs on
    every run, whatever the current checkpoint: `native_verified_concurrent`,
    `harness_valid_concurrent`, `mode_parity` (C6's `c6_mode_parity`
    condition), `assignments` (C6's `c6_assignments` condition) and
    `services` (C5's `services_current` over the replay record) and
    `content_mappers` (C7.8's contracts receipt current and the 15 rows
    matching in both modes). The checkpoint-scoped names stay what they were
    when their checkpoints closed.
  - **The C7 metrics.** The producer reads `data/phase2/residuals.json`,
    `informational.json`, `dispositions.json`, `c7-report.json`, the
    divergence ledger, and the evidence states of a fingerprinted set of other
    prerequisite producers: the C7.6 list without `checker`. The tracker
    writes `checker.latest` as an incomplete attempt before running the
    producer, so the producer cannot certify its own run, and reading
    previously generated status would certify an old snapshot. The tracker
    establishes the checker run's own freshness after recording it. The
    producer emits `c7_residuals`, `c7_informational_listed`,
    `c7_dispositions`, `c7_divergences_valid`, `c7_evidence_current`,
    `c7_report`, `c7_content_mappers` (the run-level `content_mappers` and
    C7.8's audit complete) and `c7_complete`, the conjunction of the P2B
    exit, the two-mode metrics, every `P2B-Cn` item closed (C1, C2, C5 and C6
    on their recorded runs, C3 and C4 on the final one), and the seven above.
  - **The tracker extension.** `cargo xtask check` accepts
    `recorded.<run>.<metric> == <value>` in an item's `done_when`, satisfied
    by a recorded evidence artifact of that run (named by its evidence id)
    whose report holds the value, and the sprint view names the artifact.
    `sprints/P2B.toml`'s items C1, C2, C5 and C6 name their recorded runs.
    P2B-C3 and P2B-C4 close on the final run instead (decision 2): their
    `done_when` is the P2B exit list with `c7_residuals == 0`.
  - **Fingerprints.** The corpus capture's source set (`phase2_corpus.py`
    `SOURCE_PATTERNS`, `crates/**/*`) and the `[checker]` run's sources
    include test-only files, the `crates/*/tests/**` suites and their
    fixtures. A fixture fix therefore stales every corpus capture and the
    recorded `checker` run although the executable is unchanged; the C2
    order-contract fixture fix after the C6 exit did exactly that. Exclude
    test-only paths from the capture and run source sets, keeping the build
    inputs of the executables, and leave them to the contract receipts, which
    run those tests (decision 8). This lands before the final captures, and
    its own spec change stales the current recording once.
  - All new authorities are `[checker]` inputs. C7.7 lands first, so the
    metrics exist while the residuals close.
- Exit: `scripts/tests/test_phase2_c7.py` shows each of the following:
  - one residual without an approved scope keeps `c7_complete` false;
  - an unused divergence witness fails `c7_divergences_valid`;
  - a function without a disposition fails `c7_dispositions`;
  - a stale prerequisite producer fails `c7_evidence_current`, while the
    checker run itself is excluded;
  - the run-level two-mode and services metrics are computed when C7 is the
    current checkpoint, and one outcome difference keeps `mode_parity` false;
  - an item names a recorded run that does not hold its metric, or an
    evidence id that does not exist, and stays open;
  - a complete stale-to-current recording sequence (rerun, record, regenerate
    the status views, check) ends current;
  - changing each new input invalidates the recorded result.

### C7.8 Content-mapper execution (pulled forward from Phase 5)

The owner's decision 1: port content-mapper execution so the 15 rows of B01
run, rather than amending the P2B exit.

- Exists: `tsr_tsoptions`'s parse of `contentMappers` and the harness option
  `runExternalCode`; the loader's and the corpus driver's refusals; the
  program's `applyContentMapperDiagnosticDirectives` port; the pinned native
  observations of the 15 rows in both modes (executed natively, withheld only
  on the Rust side).
- Build:
  1. **C7.8.0, the ledger move.** It follows C6.1's recipe and comes first,
     because it stales every capture that binds `data/upstream.json`:
     - Trace from the 15 rows, with the same call-graph reach the S07
       operation inventory uses, the files they execute: the
       `contentmapper` files, `spanmap/spanmap.go`, the `jsonrpc` files, the
       `ipc` files the in-process spawner reaches (the Unix and Windows
       transports are not among them), the seven test mappers with the
       shared `contentmappertest` files (`registry.go`, `spawner.go`,
       `protocol.go`, `manifest.go` and what they call), and
       `tsbaseline/contentmapper_baseline.go`.
     - Move those files to Phase 2 with `FILE_PHASES` in
       `scripts/ledger-init.py`, then regenerate `PORTS.toml` and
       `data/upstream.json`.
     - Retake both native captures and verify them. Their row digests must
       equal the current ones.
     - Refresh the S07 observations' provenance and re-freeze the subset
       review.
     - Re-freeze the C2 order traces, and rebind every fixture that names
       their build, `fixtures/c2/contextual_audit.json` among them.
     - List the Phase 1 captures and recorded runs this stales for C7.6.
     - Bind a C7 audit scope over the moved files
       (`data/phase2/c7-audit.json`, as C6's).
  2. **C7.8.1, the protocol.** `tsr_jsonrpc` (the base protocol and message
     types) and `tsr_ipc` (the connection, the synchronous and asynchronous
     conns, the JSON-RPC protocol and timing), with the in-memory transport
     the in-process spawner uses.
  3. **C7.8.2, the host and span maps.** `tsr_contentmapper`:
     - the `Host` and `Project` with their lifecycle;
     - the `Spawner` seam and the logged, close-once process wrapper;
     - the mapping requests and responses, the mapped diagnostic directives
       and the per-mapper timing.

     `tsr_spanmap` completes the port of `spanmap.go`: the virtual-to-original
     span translation and its fidelity.
  4. **C7.8.3, the program integration.**
     - The loader serves mapped files through the project's host: the
       virtual texts, the supplemental files, the per-file extensions and
       the collisions.
     - The program's diagnostics translate spans (`filterAndSortDiagnostics`'
       fidelity rule and the content-mapper option diagnostics).
     - Declaration emit covers mapped files.
     - The loader's refusal is removed.
  5. **C7.8.4, the harness.** The seven test mappers, ported into a
     test-utility crate and served in process through the spawner seam, as
     `harnessutil` serves them. The corpus driver runs `runExternalCode`
     programs through the host shared by the pre-emit and post-emit programs.
     The content-mapper error baseline replaces the corpus driver's refusal.
  6. **C7.8.5, contracts.** `crates/tsr_compiler/tests/c7_contracts.rs` is one
     test per behavior over production entry points, each against pinned
     observations: a mapped program's diagnostics and their span translation,
     a failing mapper, supplemental diagnostics, globals and modules, the
     directives, and the host's lifecycle across the pre-emit and post-emit
     programs. It gets a `c7-contracts` witness and receipt.
- Exit: the 15 rows match in every domain in both modes; the register is
  empty; `c7_content_mappers` true (the contracts receipt current, the C7
  audit complete).

## 5. Dependencies and owners

| Dependency | Owner | State for C7 |
| --- | --- | --- |
| C1 to C6 complete | the checkpoints | done; each recorded (section 2's table); C1 to C5 accounting is history since the ledger move |
| Content-mapper execution (B01; 15 executed C3 rows) | C7.8, pulled forward from Phase 5 (decision 1) | the ledger move of C7.8.0, then the port |
| The C2 order-contract fixture binding the pre-C6 trace build | C6 follow-up | fixed with this plan's revision, all six receipts refreshed; C7.2 reruns C2's contracts with `creation-trace` |
| The Linux host capture and the loader-side project-reference operations | Phase 1 | the capture is taken in C7.6; the operations stay Phase 1 with a named owner in the disposition |
| The owner's recordings (`checker` in both modes, `e3`, `checkerbench`, `bindworkload`, `e5`, `e6`, `status --record`) and the quiet host | owner | C7.6 |
| Owner decisions of section 9 | owner | before C7.1 |

## 6. Delivery order

1. C7.8.0, the ledger move, with the native retakes and re-freezes it
   forces, together with C7.7's fingerprint narrowing (decision 8). Both stale
   the current evidence once, so they come first.
2. C7.7, the run-level metrics, the C7 metrics and the tracker extension, with
   `sprints/P2B.toml`'s items. Then C7.0, the skip list.
3. C7.8.1 to C7.8.5, the content-mapper port, with a C7-start capture as its
   regression baseline.
4. C7.1 the residual list, refreshed as C7.8 closes B01's rows.
5. C7.4 the disposition, with the ledger `verify` bindings.
6. C7.2 the validation on the final executable, C7.3 the ledger check, C7.5
   the report, then C7.6 the green-up and the recordings.

Full runs are the two final runs of C7.2 (single-threaded and concurrent);
everything else replays recorded captures.

## 7. Executable exit checks

```sh
export PATH="$(mise where go)/bin:$PATH"
python3 scripts/phase2_inventory.py check
python3 scripts/phase2_inventory.py informational --check                     # data/phase2/informational.json matches the inventory
python3 scripts/phase2_native.py verify --capture target/phase2/native --shards 7 --scheme interleaved --jobs 7
python3 scripts/phase2_native_concurrent.py verify --capture target/phase2/native-concurrent --shards 7 --scheme interleaved --jobs 7
python3 scripts/phase2_corpus.py run --native target/phase2/native --output target/phase2/rust-c7 --mode single
python3 scripts/phase2_corpus.py run --native target/phase2/native-concurrent --output target/phase2/rust-c7-concurrent --mode concurrent
python3 scripts/phase2_compare.py report --native target/phase2/native --rust target/phase2/rust-c7 --record
python3 scripts/phase2_compare.py report --native target/phase2/native-concurrent --rust target/phase2/rust-c7-concurrent
python3 scripts/phase2_compare.py modes --rust target/phase2/rust-c7 --rust-concurrent target/phase2/rust-c7-concurrent
for c in C2 C3 C5; do python3 scripts/phase2_claims.py rebind --checkpoint "$c" --rust target/phase2/rust-c7; done
python3 scripts/phase2_blockers.py build --native target/phase2/native --rust target/phase2/rust-c7 --record   # expected empty after C7.8
python3 scripts/phase2_residuals.py build --rust target/phase2/rust-c7 --rust-concurrent target/phase2/rust-c7-concurrent --check   # c7_residuals 0
python3 scripts/phase2_divergences.py check --rust target/phase2/rust-c7
python3 scripts/phase2_dispositions.py build --check
for n in 1 2 3 4 5 6 7; do python3 scripts/phase2_audit.py check --audit "data/phase2/c${n}-audit.json"; done
for w in c1-contracts c2-contracts c3-contracts c4-contracts c5-contracts c6-contracts c7-contracts; do python3 scripts/phase2_producers.py observe --witness "$w"; done   # each with its witness's exact feature set, debug and release
python3 scripts/phase2_services.py verify && python3 scripts/phase2_services.py replay --output target/phase2/services-c7
python3 scripts/phase2_assignments.py record --output target/phase2/assignments-c7 --record && python3 scripts/phase2_assignments.py compare --capture target/phase2/assignments-c7 --record
python3 scripts/phase2_assignments.py fusion --check
python3 scripts/s08_e2.py obligations --output target/s08/e2-c7
python3 scripts/s08_ownership.py                                               # E3, including the compiler_pool suite
python3 scripts/s08_relater.py build  --output target/s08/relater-c7 && python3 scripts/s08_relater.py parity --output target/s08/relater-c7
python3 scripts/phase2_report.py build --rust target/phase2/rust-c7 --rust-concurrent target/phase2/rust-c7-concurrent
ln -sfn rust-c7 target/phase2/rust && ln -sfn rust-c7-concurrent target/phase2/rust-concurrent
python3 scripts/phase2_producers.py checker
python3 -m pytest scripts/tests -q
python3 scripts/checks.py fmt && python3 scripts/checks.py clippy
cargo xtask validate
cargo xtask check P2A && cargo xtask check P2B                                # after the owner's recordings
cargo xtask status --check-committed
```

`cargo xtask run checker` (both modes), `run e3`, `run checkerbench`, the S07
chain (`bindworkload`, `e5`, `e6`), `cargo xtask status --record` and the
Linux host capture are the owner's.

## 8. Evidence reuse rules

- Both native captures are reused only when their read-only validators
  succeed at the final pin; C7 takes no new native capture unless a canonical
  input changed.
- The final Rust captures are fresh; every recorded metric derives from them
  and from replayed, source-stable captures; a sample never feeds a C7
  metric.
- The two modes are compared by outcome, never by raw digest: concurrent-mode
  raw observations may differ between runs in type ids alone (section 3).
  Handoffs rebind against the single-threaded capture.
- A retained difference exists only as an approved divergence with its
  witnesses; decision 1 rules out a sprint-exit amendment naming a joint
  blocker. A status label, a bucket or a free-text owner is never a scope.
- Recorded evidence is current only by fingerprint; C7 clears staleness by
  rerunning producers, never by editing views.
- A test fixture under `crates/` is a source of every corpus capture, every
  contract receipt and the `checker` run. So a fixture fix after a recording
  makes that recording stale, and waits for its own PR or for the next
  recording.
- The performance report cites recorded captures by digest and date; it
  introduces no threshold and converts no measure into another.
- Mapping progress is reported by markers and the ledger, never as behavioral
  coverage; the two-mode corpus comparison, the sub-tests, the contracts, the
  replay and the E3 scenarios are the behavioral evidence.

## 9. Owner decisions (2026-09-30)

1. **The content-mapper rows: pull Phase 5 forward.** Content-mapper
   execution becomes C7.8, so B01's 15 rows run and must match. The P2B exit
   is not amended. The files the rows execute move to Phase 2 in the ledger,
   as C6.1 moved `checkerpool.go`, with the same ripple: native retakes, S07
   and C2 re-freezes, and the recorded runs that bind `data/upstream.json`
   going stale until the green-up.
2. **Recorded completion.** The P2A and P2B exit lists close on the final
   recorded `checker` run. Through the tracker extension of C7.7, P2B-C1
   closes on `96b7c65f`, P2B-C2 on `74c7960a`, P2B-C5 on `815eeb43` and
   P2B-C6 on `afc6eeb4`. P2B-C3 and P2B-C4 close on the final run: the P2B
   exit list with `c7_residuals == 0`. They were never recorded while
   current, and their baselines are history.
3. **Ledger `verify` bindings.** Accepted as C7.4 proposes, with the
   run-level `mode_parity`, `assignments`, `services` and `content_mappers`
   of C7.7 for the pool, services and content-mapper files.
4. **The dashboard's home.** Confirmed: `scripts/phase2_report.py` writes
   `docs/PHASE2-C7.md` and `data/phase2/c7-report.json`, which the status
   renderer includes.
5. **The performance report.** Confirmed: only recorded captures are cited
   (the C2 checkerbench capture, the last E5 and E6), and no new benchmark is
   run unless the owner runs one on the quiet host.
6. **The green-up set and hosts.** Split as proposed. The implementer reruns
   the producers it can, including the Phase 1 capture refreshes. The owner
   runs `checkerbench` on the quiet host, `bindworkload`, `e5` and `e6` in the
   S07 chain order, `e3`, the `checker` recordings and the Linux host capture.
7. **Exit recording and Phase 7.** Confirmed: `c7_complete` is computed only
   from the recorded runs, and C7 hands over the report without starting
   Phase 7's four-week acceptance.
8. **Test-only paths out of the capture and run fingerprints.** Accepted:
   C7.7 narrows the corpus capture's and the `checker` run's source sets, so
   a test or fixture change stales only the contract receipts that run it.
