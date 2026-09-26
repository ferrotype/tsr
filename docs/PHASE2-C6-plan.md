# Phase 2 C6: checker execution and cancellation

Checkpoint C6 of the [Phase 2 plan](PHASE2-plan.md), written after the
[C2 plan](PHASE2-C2-plan.md) (PR #62, branch `phase2-c2-plan`) and its review
amendments, alongside the C3, C4 and C5 drafts, over the reviewed-source C1
capture (`target/phase2/rust`: 12,459 of 13,432 rows match in every domain,
regression 9,367 of 9,367). Upstream is Corsa
`1f70213d4922b434345f639b441681e470c7cfc1`. C6 is production work in
`crates/tsr_compiler` (the compiler checker pool), `crates/tsr_checker` (the
cancellation and tracing seams) and `crates/tsr_project` (disposal of a
canceled checker), paired with its witnesses. C6 assumes the checker
semantics of C1 to C5; it changes how checkers run, never what they report.

## 1. Objective and exit

Integrate the pinned compiler partitioning and pool policy, including its
FENNEL heuristic and constants, with the ownership primitives that exist
(`tsr_arena` generations and leases, `tsr_checker` owners and operations,
`tsr_project` slots and retirement), and keep it distinct from the editor and
API lifetime pool. Port `compiler/checkerpool.go` and `checker/tracer.go` from
scratch, the tracer behind a trace-sink seam (ADR 0014) so that checker trace
production does not pull the Phase 4 tracing package forward. Implement the
pin's cancellation contract: the checker polls per statement and per deferred
node, a canceled checker stays poisoned, its diagnostics are nil and reuse
panics, and the pool disposes that checker. Keep panic retirement of a
generation (ADR 0012) a separate contract. Verify serial and multiple checker
execution, deterministic outputs, context polling, cancellation, tracing and
panic retirement through production entry points.

C6 exits when all of the following hold, recorded by the owner through
`cargo xtask run checker`:

| Exit condition | Measured by |
| --- | --- |
| Current, full, authenticated and recorded captures with no harness errors, in both execution modes | `inventory_frozen`, `native_verified`, `harness_valid`, `result_recorded` and `blockers_named` are true for the single-threaded run; `native_verified_concurrent` and `harness_valid_concurrent` for the concurrent run; prerequisites of `c6_complete` |
| Existing cases preserved | `run.checker.regression_parity == 1` (9,367 of 9,367) in the single-threaded run |
| No previously matching domain of any executed row becomes a non-match | `run.checker.c6_regressions == 0` against the authenticated C6-start row report `data/phase2/c6-baseline.json.gz` |
| The full comparison passes in both of upstream's modes: the concurrent-mode Rust run reports, row by row and domain by domain, exactly what the single-threaded run reports, and each run is compared with the native capture of its own mode | `run.checker.c6_mode_parity == true` (zero row-domain differences between the two Rust reports; every difference between the two native captures enumerated and attributed in `data/phase2/c6-claims.json`) |
| File-to-checker assignments equal pinned Go's on the same GOOS/GOARCH for every corpus program and the synthetic witness set, with the toolchain and arithmetic behavior recorded | `run.checker.c6_assignments == true` over `data/phase2/c6-assignments.json` |
| Interrupted work preserves the pin's contract: no invalid state published, no retired generation reused | `run.checker.c6_contracts == true` (the cancellation and retirement contracts of C6.9), from the recorded v2 receipt |
| No unexplained production failure in a C6 claim, in either mode | `run.checker.c6_failures == 0` |
| The C6 function groups of section 4 are audited and the `checkerpool.go` ledger entry is moved to Phase 2 with the generated phase tables consistent | `run.checker.c6_audit_complete == true` over `data/phase2/c6-audit.json`; `cargo xtask validate` |

`run.checker.c6_complete` is the conjunction. It binds `P2B-C6` in
`sprints/P2B.toml`. The concurrent-test-programs configuration and the
single-threaded default move here from PLAN's Phase 4 gate with the
`checkerpool.go` ledger entry (Phase 2 plan decision 4, review finding 4). No
performance threshold is introduced: C6 measures parity and contracts; the
multi-checker speedup is Phase 7's to measure.

## 2. Starting point

Everything below exists and is consumed as is. C6 extends it; it re-derives
nothing.

| Asset | Where | What C6 takes from it |
| --- | --- | --- |
| The pinned compiler pool | `tsc/internal/compiler/checkerpool.go` (491 lines): the `CheckerPool` interface (`GetChecker(ctx, file)` returns an exclusively held checker and its release), the five calibrated constants (text weight divisor 100, source-file weight multiplier 4, balance penalty 16, prioritized-source penalty 12, strong-balance minimum of 4 checkers), the three policy regimes, the FENNEL assignment with gamma 3/2 and its deterministic ties, the source-first order, base weights from node count and text length, import-unit normalization, the checker count (4 by default, 1 when single-threaded, the internal `checkers` option, clamped to at least 1 and at most the smaller of the file count and 256), the once-only creation of checkers in a work group, the undirected import adjacency from resolved in-program modules, exclusive and non-exclusive acquisition, `forEachCheckerGroupDo` with one task per checker and files in program order, and the concatenated, sorted and deduplicated global diagnostics | the algorithm C6.3 ports line by line; ADR 0009 requires the same constants |
| The pinned program driving | `compiler/program.go`: `collectCheckerDiagnostics` (one file: that file's checker exclusively; all files: grouped by checker when the compiler pool is in use, per-file acquisition otherwise), `collectDiagnosticsFromFiles` with its concurrency flag, `GetTypeChecker`, `GetTypeCheckerForFile`, `GetTypeCheckerForFileExclusive`, `GetCheckerPool`, `SingleThreaded`; `core/workgroup.go`: the parallel work group and the single-threaded one, which runs its queued tasks last-in first-out | the driving C6.4 ports; the LIFO order is observable where task order is |
| The pinned cancellation contract | `checker.go`: `checkSourceFile` installs the context and clears it at the end; `isCanceled` (`utilities.go`) is polled in `checkSourceElements` per statement, in `checkDeferredNodes` per deferred node, in `checkContextualDeprecations`, and before the unused-identifier passes; `wasCanceled` becomes sticky at the end of a canceled check; `getDiagnostics` and `GetGlobalDiagnostics` first call `checkNotCanceled`, which panics with `Checker was previously cancelled`; a canceled check returns nil diagnostics; `WasCanceled` is exported (`exports.go`); the project pool disposes a canceled checker | the seams C6.5 ports; the polling sites are the pin's, not a Rust choice |
| The pinned tracer | `checker/tracer.go` (366 lines: `NewTracer`, `RecordType`, `Push`, `Instant`, the checker-index argument helpers and the `wrapType` accessors implementing `tracing.TracedType`); 18 gated call sites (`checker.go` 11, `relater.go` 6, `flow.go` 1) of the form `if tr := c.tracer; tr != nil`; `relater.go` `traceUnionsOrIntersectionsTooLarge` (C1 audit `later: C6`); `internal/tracing` (Phase 4, crate `tsr_tracing`, planned) writes `trace.json`, `types.json` and `legend.json` and is enabled by `generateTrace` | the events and type records C6.2 reproduces through the seam; the writer stays Phase 4's |
| The editor and API pool | `tsc/internal/project/checkerpool.go` (530 lines, Phase 5): one diagnostics checker, ephemeral query checkers with an idle timeout, one persistent API checker; `crates/tsr_project` already implements its slot lifetime and generation retirement (`CheckerSlot::{Diagnostics, Query, Api}`, `acquire`, `evict_idle`, `Project`, `Snapshot`), with scheduling, affinity and idle timers left to Phase 5 | the pool C6 must not conflate with the compiler pool; the disposal C6.5 adds for a canceled checker |
| The ownership primitives | `tsr_arena`: `Generation` (`retire`, `enter`, `validate_checker`), `CheckerIdentity`, `Counters`, the retirement contention observer; `tsr_checker`: `CheckerOwner` (identity and all mutable checker state) and `Operation` (the exclusive permit and the state lock, dropped together; inside an operation the checker is a single-threaded `&mut` state machine); S09-1 retained results, S09-2 builder caches across generations, S09-4 pool generation, snapshots and API commitment | the primitives C6.3 composes; no new ownership rule |
| Existing witnesses | `c1_contracts.rs`: `two_checkers_merge_independently_over_one_program`, `an_injected_panic_retires_the_generation_and_a_fresh_checker_succeeds`, `deep_relations_grow_the_stack_and_keep_the_checker_usable`; `tsr_arena` generation tests (nested gates rejected, retirement waits for the committing gate, a poisoned gate never recovers); `tsr_project` tests (initializer panic retires the generation, idle replacement rejects old types, the API checker persists, snapshots share the pool, a panic retires shared snapshots, callback resume rechecks retirement); E3's criteria (`shared_pool_panic_retirement`, `wrong_owner_rejected`, `stale_and_recycled_ids_rejected`, `release_boundaries`, `miri`, `address_sanitizer`) in the ownership harness (`scripts/s08_ownership.py`) | the contracts C6.9 extends to a multi-checker compiler pool through production entry points |
| The two native modes | the harness leaves programs single-threaded unless `TS_TEST_PROGRAM_SINGLE_THREADED` says otherwise or the race build is on (`testutil.go`); the C0 native capture records `single_threaded: true`, `goos: darwin`, `goarch: arm64`, `go1.27.1`; the compiler runner's tests run in parallel across programs in both modes | the single-threaded capture is reused; C6.0 takes the concurrent one |
| The Rust corpus driver | one `CheckerOwner::for_program` per program, checked single-threaded (`tsr_compiler/examples/p2`, `tools/s08/p4/executor.rs`); the E2 small-stack and ADR 0011 reserved-stack rules | the driver C6.4 gives a mode switch |
| The cancellation seams that exist | `tsr_testhost` session (`cancellation_requested`, the advisory cancel of an update), `tsr_embed` session (explicit cancellation distinct from dropping a retention root), `tsr_wasm` (`retire` cancels further use) | the transport side (Phase 5); C6 supplies the checker side they will call |

The C6-start capture is a fresh named output directory; no existing capture is
moved or overwritten.

## 3. What C6 owns

C6 owns no inventory rows and no semantic cause. It owns the execution
contract over every executed row:

| Unit | Count | State at the C6 start |
| --- | ---: | --- |
| The concurrent-mode native capture | 13,432 rows | not taken; C6.0 captures and verifies it under `TS_TEST_PROGRAM_SINGLE_THREADED=false` with the C0 sharding, and records `data/phase2/native-provenance-concurrent.json` |
| Rows whose native observation differs between the two modes | unknown, expected 0 in `errors`, `types` and `symbols` (the pin asserts its committed baselines in both modes) and possibly nonzero in `display`, `union_ordering` and `parent_pointers` (Phase 2's extra domains) | enumerated by C6.0; each difference attributed to the pin behavior that causes it (per-checker type identities reaching an ADR 0010 fallback, checker-index-dependent order) and recorded as a claim, never as a Rust divergence |
| Rows whose Rust observation differs between the two modes | 0 required | `c6_mode_parity` |
| Assignment witnesses | every corpus program with more than one checker (the clamp makes programs with a single file single-checker), plus a synthetic set over 2, 4 and 8 checkers | `data/phase2/c6-assignments.json` |
| Blockers | none | C6 registers a blocker only for an observed missing operation of its own |

`data/phase2/c6-claims.json` records the mode differences of the native
captures with the pin's cause, the Rust mode differences while any exist
(status `open`), and the assignment differences while any exist, each with the
reproduction argv, the capture identities and the trace digest, following the
validator fields of the C2 plan. Status labels never exempt a current
difference.

The seven domains are `errors`, `types`, `symbols`, `display`, `trace`,
`union_ordering` and `parent_pointers`. C6 adds no domain: the two-mode
comparison is over the same seven.

## 4. Work items

Each item names what exists, what to build, its artifact and its executable
exit check. Functions are named by their pinned Go names; `port:` markers and
the ledger stay the mapping authority.

### C6.0 The concurrent native capture and the gap map

- Exists: the C0 native capture and its read-only validator;
  `phase2_native.build_oracle` and `run_shard` with environment control;
  `phase2_compare.py baseline`.
- Build: `phase2_native.py capture --mode concurrent` sets
  `TS_TEST_PROGRAM_SINGLE_THREADED=false` for the harness overlay, runs the
  full inventory with the C0 sharding, verifies with `--shards 7 --scheme
  interleaved --jobs 7`, and writes `target/phase2/native-concurrent` with its
  provenance (`single_threaded: false`, GOOS, GOARCH, toolchain, the
  `checkers` count in effect); `phase2_compare.py modes --native
  target/phase2/native --native-concurrent target/phase2/native-concurrent`
  compares the two captures' `row_sha256` inventories and per-domain
  observations and writes the enumerated differences; one full Rust
  single-threaded run at the C6 head into a fresh directory, frozen as
  `data/phase2/c6-baseline.json.gz`; `data/phase2/c6-claims.json` with the
  native mode differences attributed.
- Exit: the concurrent capture verifies; every native mode difference has a
  pinned cause; 0 harness errors and 0 regressions in the Rust baseline run.

### C6.1 Audit and the ledger move

- Exists: `scripts/phase2_audit.py` with the C1 to C5 scope bindings;
  `PORTS.toml` with `checkerpool.go` at Phase 4 (`tsr_compiler`, planned),
  `tracer.go` at Phase 2 (`tsr_checker`, planned), `tracing.go` at Phase 4
  (`tsr_tracing`), `project/checkerpool.go` at Phase 5 (`tsr_project`).
- Build: `data/phase2/c6-audit.json`, with a separately reviewed C6 scope
  binding, over these groups: `compiler/checkerpool.go`, the complete file
  (`CheckerPool`, `checkerPool`, `getCheckerAssociationPolicy`,
  `getCheckerAssociationsInOrder`, `getCheckerAssociationOrder`,
  `getCheckerAssociationBaseWeight`, `shouldPrioritizeSourceFiles`,
  `getCheckerAssociationWeights`, `newCheckerPool`, `newCheckerPoolWithTracing`,
  `GetChecker`, `getCheckerForFileNonExclusive`, `getCheckerForFileExclusive`,
  `getCheckerNonExclusive`, `createCheckers`, `getImportAdjacency`,
  `forEachCheckerParallel`, `GetGlobalDiagnostics`, `forEachCheckerGroupDo`);
  `checker/tracer.go`, the complete file; the program driving
  (`collectDiagnostics`, `collectDiagnosticsFromFiles`, `collectCheckerDiagnostics`,
  `collectCheckerDiagnosticsFromFiles`, `GetTypeChecker`, `GetTypeCheckerForFile`,
  `GetTypeCheckerForFileExclusive`, `GetCheckerPool`, `SingleThreaded`,
  `filterAndSortDiagnostics`); `core/workgroup.go` (`WorkGroup`, `NewWorkGroup`,
  the parallel and single-threaded groups); the cancellation functions
  (`checkSourceFile`'s context handling, `isCanceled`, `checkNotCanceled`,
  `WasCanceled`, the polling sites); the 18 tracer call sites and
  `traceUnionsOrIntersectionsTooLarge`; the checker constructor's id and lock
  (`NewChecker` returns the checker and its mutex; `nextCheckerID`). The
  ledger entry of `checkerpool.go` moves to Phase 2 with its Rust files, and
  the generated phase tables are regenerated by `cargo xtask`. `tracing.go`
  stays Phase 4: C6 ports only the seam and the in-memory sink.
- Exit: `phase2_audit.py check --audit data/phase2/c6-audit.json` passes with
  no `gap`; `cargo xtask validate` accepts the moved entry and the
  regenerated tables.

### C6.2 The trace-sink seam and the tracer

- Exists: nothing in Rust; the pin's 18 gated call sites and the
  `TracedType` accessor shape (`Id`, `FormatFlags`, `IsConditional`, `Symbol`,
  `AliasSymbol`, `AliasTypeArguments`, `IntrinsicName`, `UnionTypes`,
  `IntersectionTypes` and the rest).
- Build: a `TraceSink` trait in `tsr_checker` with the shapes of `tracing.Tracing`'s
  `Push` and `Instant` and of `tracing.Tracer`'s `RecordType`, object-safe
  and `Send + Sync` as ADR 0014 asks, taken by the checker constructor as an
  optional sink exactly where `NewChecker` takes an optional `*Tracer`; a
  `None` sink is the pin's nil tracer and every call site keeps its gate; the
  `Tracer` port (checker index, the separate begin and end events, the
  argument copies) and the traced-type view over the checker's types; the 18
  call sites with the pin's phases, names and arguments;
  `traceUnionsOrIntersectionsTooLarge`; an in-memory sink for the contracts and
  a JSON-lines sink that emits the pin's event and type records (the
  `trace.json` events, `types.json` records and `legend.json` entries)
  without timestamps, process or thread ids, so a native trace directory can be
  compared after the same normalization. The file writer, `generateTrace` in
  the CLI and the thread-name bookkeeping stay Phase 4's.
- Exit: the tracing contract of C6.9 passes; tracing on and off produce
  identical checker results on the sample.

### C6.3 The compiler checker pool and partitioning

- Exists: `tsr_project::CheckerPool` (the editor and API pool);
  `CheckerOwner`, `Operation`, `Generation`; the program adapter's retained
  resolutions (`tsr_checker/src/host.rs`, `tsr_compiler/src/checker_host.rs`);
  the bound file's syntactic import list and node count.
- Build: `tsr_compiler::checker_pool` (a different type from
  `tsr_project::CheckerPool`; both implement one `CheckerPool` trait with the
  pin's `GetChecker` shape): the checker count rule; base weights, the
  source-file multiplier, import-unit normalization, the policy regimes and
  the source-first order; the FENNEL assignment with the pin's expression
  order over `f64` (`alpha` from the penalty multiplier, half the edge count,
  the square root of the checker count and the total weight to the power
  three halves; the maximum checker weight as the larger of the largest file
  and average plus one percent; the score as placed-neighbor count minus the
  convex load increment; ties to the lower load, then the lower index; the
  least-loaded fallback); the undirected adjacency from resolved in-program
  modules through the program adapter, self edges and unresolved targets
  excluded; once-only creation of the checkers, each owner on its own thread
  with the ADR 0011 reserved stack when not single-threaded, each with its
  optional trace sink and checker index; the file associations; exclusive
  acquisition (one mutex per checker, released once), non-exclusive
  acquisition for the emit resolver, `forEachCheckerParallel` and
  `forEachCheckerGroupDo`; global diagnostics concatenated across checkers,
  sorted and deduplicated as the pin does. Where the single-threaded work
  group's last-in first-out order is observable (checker creation order and
  therefore checker ids and the order of per-checker global diagnostics
  before sorting), the Rust group reproduces it.
- Exit: the assignment contract of C6.9 passes; `c6_assignments` true on the
  corpus programs.

### C6.4 Program driving in both modes

- Exists: `tsr_compiler/src/checker_diagnostics.rs` (diagnostic selection and
  directives around one checker operation), `declaration_diagnostics.rs`,
  the corpus driver's single owner per program.
- Build: the program's checker entry points (`GetTypeChecker`,
  `GetTypeCheckerForFile`, `GetTypeCheckerForFileExclusive`, `GetCheckerPool`,
  `SingleThreaded`); `collectCheckerDiagnostics` and
  `collectCheckerDiagnosticsFromFiles` (one file through its checker
  exclusively; all files grouped by checker with the compiler pool, per-file
  acquisition with any other pool); `collectDiagnosticsFromFiles` with the
  pin's concurrency flag per diagnostic kind; the declaration-diagnostics and
  emit-resolver paths acquire the file's checker non-exclusively as the pin
  does; the corpus driver (`phase2_checker` and the harness request) gains
  `--mode single|concurrent`, mirroring `TS_TEST_PROGRAM_SINGLE_THREADED`, and
  `phase2_corpus.py run --mode concurrent` writes a capture whose provenance
  names the mode and the checker count per program.
- Exit: the concurrent full run completes with 0 harness errors;
  `phase2_compare.py modes --rust A --rust-concurrent B` reports 0 row-domain
  differences.

### C6.5 Cancellation

- Exists: no checker-side token; the transport-side seams of section 2.
- Build: a `CancellationToken` in `tsr_core` (`Send + Sync`, polled without a
  lock), carried by the operation as the pin carries `ctx` for the duration
  of `checkSourceFile`; polls at exactly the pin's sites (per statement in
  `checkSourceElements`, per deferred node in `checkDeferredNodes`, in
  `checkContextualDeprecations`, before the unused-identifier passes); the
  sticky `was_canceled` set at the end of a canceled check; `getDiagnostics`
  and `GetGlobalDiagnostics` refuse a previously canceled checker with the
  pin's panic (`Checker was previously cancelled`) and return no diagnostics
  for the canceled check; `WasCanceled` (the C5.2 query gets its semantics
  here); `tsr_project::CheckerPool` evicts a canceled checker at release and
  never hands it out again, replacing the slot as it does for idle eviction;
  the compiler pool abandons a canceled compile (the pin has no reuse path
  there). A cancellation never publishes partial results: retained results
  taken before the cancel stay valid, results of the canceled check are not
  produced.
- Exit: the cancellation contracts of C6.9 pass in debug and release.

### C6.6 Panic retirement under concurrent execution

- Exists: ADR 0012's generation retirement; the C1 contract that an injected
  panic retires the generation and a fresh checker succeeds; the E3 criteria.
- Build: the multi-checker case through production entry points: a panic in
  one checker of the compiler pool retires the pool's generation; checkers on
  other threads observe the retirement at their next generation gate and
  stop; no diagnostics of the retired generation are published by the
  program; a fresh program and pool succeed; the retirement contention
  observer of `tsr_arena` is exercised with real pool threads; the E3
  `shared_pool_panic_retirement`, `wrong_owner_rejected` and
  `release_boundaries` scenarios run against the compiler pool as well as
  the ownership harness. Cancellation and retirement stay distinct: a
  canceled checker is poisoned but its generation is not retired.
- Exit: the retirement contracts of C6.9 pass in debug and release, and
  under the E3 Miri and AddressSanitizer lanes where applicable.

### C6.7 Assignment witnesses on the same GOOS/GOARCH

- Exists: the diagnostic Go overlay pattern of C2.10.
- Build: `scripts/phase2_assignments.py record` builds an overlay that, after
  `createCheckers`, dumps per program the checker count, the policy regime,
  the base and final weights, the order, the adjacency and the file-to-checker
  associations, runs it over every corpus program with more than one checker
  and over a synthetic set (graphs at 2, 4 and 8 checkers designed so that
  ties, the least-loaded fallback, the one-percent slack and the fused
  multiply-subtract of the score matter), and writes
  `data/phase2/c6-assignments.json` with GOOS, GOARCH, the Go toolchain and
  the overlay fingerprint; `compare` runs the Rust partitioner over the same
  inputs and reports equality per program. The pin's score arithmetic fuses
  the multiply and subtract on arm64 (`FMSUBD`) and not on amd64; the Rust
  partitioner reproduces the pin's per-architecture behavior with a fused
  multiply-add on `aarch64` only, at the sites the record names, so that
  assignments equal Go's on each host (decision 4). A normalized
  cross-architecture policy is not proposed; it would need an ADR 0009
  amendment.
- Exit: `compare` reports equality for every recorded program on the capture
  host (darwin/arm64) and, when the owner runs the Linux host capture, on
  linux/amd64; differences, if any, are listed with the site and the
  rounding they come from.

### C6.8 Deterministic outputs across modes

- Exists: the ADR 0010 comparators and the C2 creation-trace mode; per-checker
  type and symbol id spaces.
- Build: the two-mode comparison of C6.0 and C6.4 as a producer input; every
  Rust mode difference is traced with the creation-trace mode to the
  comparator fallback or the checker-index-dependent order that causes it and
  fixed in the checker it belongs to (the fix is that checkpoint's, C6 owns the
  finding); every native mode difference is a pin behavior recorded in the
  claims file and matched by Rust in the same mode.
- Exit: `c6_mode_parity` true.

### C6.9 Direct contracts

- Exists: the C1 to C5 contracts; the `recursion-probe` feature; the
  ownership harness.
- Build: `crates/tsr_compiler/tests/c6_contracts.rs`, one test per contract,
  each over production entry points with a pinned Go counterpart named in its
  doc comment:
  1. partitioning: the policy regimes, weights, order and FENNEL assignment
     over the recorded graphs equal the native associations at 2, 4 and 8
     checkers, including ties, the fallback and the clamp;
  2. acquisition: exclusive acquisition serializes two threads on one
     checker, file affinity is stable across calls, non-exclusive acquisition
     for the resolver does not wait, and one task per checker visits its files
     in program order;
  3. the single-threaded work group runs last-in first-out where the pin's
     order is observable, and the concurrent group produces the same sorted
     global diagnostics;
  4. cancellation between statements: the checker stops at the next poll,
     returns no diagnostics, reports `WasCanceled`, panics with the pin's
     message on reuse, and the project pool evicts it and serves a fresh one;
  5. cancellation during deferred nodes and during the unused-identifier
     passes, with the same outcomes; retained results taken before the cancel
     stay valid;
  6. panic retirement in a multi-checker pool: one thread's panic retires the
     generation, other threads stop at their next gate, nothing of that
     generation is published, and a fresh pool succeeds;
  7. tracing: for a witness program the events and recorded types through
     the in-memory sink equal the pin's normalized trace files, and tracing
     on or off leaves the checker's results identical;
  8. two modes: a multi-file program checked single-threaded and with four
     checkers gives identical `errors`, `types`, `symbols`, display and union
     ordering;
  9. stacks and lifecycle: pool checkers run on threads with the ADR 0011
     reserved stacks, the deep C1 and C3 contracts pass on a pool thread, the
     pool is created once per program and released with it, and wrong-owner
     handles across two checkers of one pool are rejected.
- Exit: `cargo test -p tsr_compiler --features recursion-probe --test
  c6_contracts` in debug and release; the v2 receipt is recorded by the
  producer (C6.10).

### C6.10 Producer wiring

- Exists: the per-checkpoint metric helper (C2.12); `sprints/P2B.toml` with
  `P2B-C6` waiting on `run.checker.c6_complete`; `[checker]` `inputs`.
- Build: the producer reads the concurrent native provenance and capture, the
  concurrent Rust capture, `data/phase2/c6-claims.json`,
  `data/phase2/c6-assignments.json`, `data/phase2/c6-audit.json`,
  `data/phase2/c6-baseline.json.gz` and the contracts receipt
  (`observe --witness c6-contracts`), and emits `native_verified_concurrent`,
  `harness_valid_concurrent`, `c6_mode_parity`, `c6_assignments`,
  `c6_regressions`, `c6_failures` (in either mode), `c6_audit_complete`,
  `c6_contracts` and `c6_complete`. `phase2_compare.py modes` and
  `phase2_assignments.py compare` are the new comparison entry points; all new
  authorities are added to `[checker]` `inputs`.
- Exit: `cargo xtask validate`; `phase2_producers.py checker` emits the nine
  metrics; `scripts/tests/test_phase2_c6.py` shows that one row-domain
  difference between modes keeps `c6_mode_parity` false, that a stale
  concurrent capture keeps `harness_valid_concurrent` false, that an
  assignment difference keeps `c6_assignments` false, and that changing each
  new input invalidates the recorded result.

## 5. Dependencies and owners

| Dependency | Owner | State for C6 |
| --- | --- | --- |
| Checker semantics in every family | C1 to C5 | C6's design (C6.1, C6.2, C6.3, C6.5) starts early; its two-mode exit runs only over a checker whose single-threaded result is at parity, so the exit follows C5 |
| The ADR 0010 creation-trace mode | C2 | the diagnostic for any Rust mode difference (C6.8) |
| `WasCanceled` and the public API | C5 | C5.2 exposes the query, C6.5 gives it its semantics |
| Retained resolutions and syntactic imports for the adjacency and weights | Phase 1 | present through the program adapter; the resolver is never called during checking, so the F3b guard does not apply |
| The tracing file writer, `generateTrace`, `--singleThreaded` and the `checkers` option in the CLI, the ThreadSanitizer build of the race-mode job | Phase 4 | not C6's; C6 ships the seam, the in-memory and JSON-lines sinks, and the corpus driver's mode switch |
| Editor pool scheduling, affinity, idle timers, transport cancellation | Phase 5 | not C6's; C6 adds disposal of a canceled checker to the existing slots |
| The concurrent native capture | C6 (compute), owner (recording) | C6.0 |
| The Linux host assignment capture | owner | C6.7, informational until run |
| Owner decisions of section 9 | owner | before C6.1 |

## 6. Delivery order

1. C6.0 the concurrent native capture (the long pole; it does not depend on
   Rust work) and the gap map; C6.1 the audit and the ledger move.
2. C6.2 the seam and the tracer, and C6.5 cancellation, because both are
   local to the checker and can land while C3 to C5 proceed.
3. C6.3 the compiler pool and C6.7 the assignment witnesses together, then
   C6.4 the program driving and the driver's mode switch.
4. C6.6 retirement under concurrency, then the first concurrent full run and
   C6.8.
5. C6.9 grows alongside 2–4, one contract per item; C6.10 last, then both
   exit full runs and the record.

Intermediate runs use the recorded 300-variant sample in both modes. Full
runs are C6.0 (single-threaded), the first concurrent run after step 3, and
the two exit runs.

## 7. Executable exit checks

`target/phase2/rust-c6-start/comparison.json` below is the C6-start report
from C6.0; use its actual saved path. Exit outputs are fresh directories.

```sh
export PATH="$(mise where go)/bin:$PATH"
python3 scripts/phase2_inventory.py check
python3 scripts/phase2_native.py verify --capture target/phase2/native --shards 7 --scheme interleaved --jobs 7
python3 scripts/phase2_native.py verify --capture target/phase2/native-concurrent --shards 7 --scheme interleaved --jobs 7
python3 scripts/phase2_compare.py modes --native target/phase2/native --native-concurrent target/phase2/native-concurrent   # native mode differences, all attributed in c6-claims.json
python3 scripts/phase2_corpus.py run --native target/phase2/native --output target/phase2/rust-c6
python3 scripts/phase2_corpus.py run --native target/phase2/native-concurrent --output target/phase2/rust-c6-concurrent --mode concurrent
python3 scripts/phase2_compare.py report --native target/phase2/native --rust target/phase2/rust-c6 --previous target/phase2/rust-c6-start/comparison.json --record
python3 scripts/phase2_compare.py report --native target/phase2/native-concurrent --rust target/phase2/rust-c6-concurrent
python3 scripts/phase2_compare.py modes --rust target/phase2/rust-c6 --rust-concurrent target/phase2/rust-c6-concurrent     # 0 row-domain differences
python3 scripts/phase2_blockers.py build --native target/phase2/native --rust target/phase2/rust-c6 --record
python3 scripts/phase2_assignments.py compare                                  # equal on this GOOS/GOARCH for every recorded program
python3 scripts/phase2_audit.py check --audit data/phase2/c6-audit.json
cargo test -p tsr_compiler --features recursion-probe --test c6_contracts --locked && cargo test -p tsr_compiler --features recursion-probe --test c6_contracts --locked --release
python3 scripts/phase2_producers.py observe --witness c6-contracts           # the v2 receipt, with the dependency and asset closure
python3 scripts/s08_ownership.py                                               # the E3 scenarios, now also over the compiler pool
python3 scripts/s08_relater.py build  --output target/s08/relater-c6
python3 scripts/s08_relater.py parity --output target/s08/relater-c6         # 105/105, all_cases_match true, both implementations
python3 scripts/phase2_producers.py checker --rust target/phase2/rust-c6 --rust-concurrent target/phase2/rust-c6-concurrent   # c6_mode_parity, c6_assignments, c6_regressions 0, c6_failures 0, c6_audit_complete, c6_contracts, c6_complete
python3 -m pytest scripts/tests/test_phase2_*.py -q
python3 scripts/checks.py fmt && python3 scripts/checks.py clippy
cargo xtask validate
# Assert the C6 metrics explicitly; `check P2B` stays pending until C7.
```

`cargo xtask run checker`, `cargo xtask run e3` and `cargo xtask status
--record` are the owner's, as is the Linux host assignment capture.

## 8. Evidence reuse rules

- The single-threaded native capture is reused only when its read-only
  validator succeeds; the concurrent capture is a second canonical capture
  with its own provenance and is verified the same way; neither is derived
  from the other.
- A Rust capture supplies current acceptance only when replay succeeds
  against the captured executable with `source_stable: true`, in the mode its
  provenance names.
- The C6-start baseline is reused only while it names the current native
  observation and inventory digests.
- The assignment record is reused while its overlay fingerprint, pin, GOOS,
  GOARCH and toolchain match; a different host needs its own record.
- Native mode differences are pin behavior, recorded in the claims file and
  matched in the same mode; they are not divergences. A Rust mode difference
  is never accepted through the divergence ledger (ADR 0004): it is a defect
  in the checker that owns the order.
- The relation contract, E2 and E3 may go stale under the phase-end rule when
  C6 touches fingerprinted sources; their parity is rerun in section 7 and
  their records refreshed at the phase-end green-up.
- Mapping progress is reported by markers and the ledger, never as behavioral
  coverage; the two-mode corpus comparison, the assignment record and the
  C6.9 contracts are the behavioral evidence.

## 9. Owner decisions before C6 starts

1. **The ledger move.** `compiler/checkerpool.go` moves from Phase 4 to
   Phase 2 in `PORTS.toml`, as the Phase 2 plan's decision 4 records, and the
   generated phase tables follow; `tracing.go` and `project/checkerpool.go`
   stay where they are. Confirm.
2. **The trace-sink seam.** A `TraceSink` trait in `tsr_checker` with the
   pin's `Push`, `Instant` and `RecordType` shapes; the writer stays Phase
   4's; C6 ships in-memory and JSON-lines sinks for its witnesses. Confirm.
3. **The concurrent native capture.** A second full native capture under
   `TS_TEST_PROGRAM_SINGLE_THREADED=false`, verified with the C0 sharding and
   recorded as a standing `[checker]` input. It costs about one C0 capture.
   Confirm, and say whether it runs on this host or the Linux host.
4. **Assignment arithmetic.** Reproduce the pin's per-architecture rounding
   (a fused multiply-add on `aarch64` at the recorded sites, none on
   `x86_64`) so that assignments equal Go's on each host; no normalized
   cross-architecture policy. The alternative is an ADR 0009 amendment for a
   normalized policy. Proposed: reproduce.
5. **The cancellation token.** A `Send + Sync` token in `tsr_core`, polled
   at the pin's sites without a lock, carried by the operation; the project
   pool evicts a canceled checker at release; the compiler pool abandons.
   Confirm.
6. **Sanitizer lanes.** The ThreadSanitizer build mirroring upstream's
   race-mode job stays Phase 4's gate; C6 runs its concurrent contracts under
   the E3 Miri and AddressSanitizer lanes. Confirm.
7. **Exit runs.** Two recorded full captures, one per mode, computed only
   from recorded runs; the recordings are the owner's. Confirm.
