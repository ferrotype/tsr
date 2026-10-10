# Phase 7: hardening, WebAssembly, embedding, cut-over

Status: **R0 in progress** (2026-10-10); owner decisions 7, 9 and 11 accepted,
the others open. The detailed plan for
[PLAN Phase 7](../PLAN.md#phase-7-hardening-webassembly-embedding-cut-over),
the last phase: it turns PLAN section 5's exit criteria and the
[acceptance matrix](S12-acceptance.md) of ADR 0020 into checkpoints with
work items, witnesses and exit checks. Section 8 distinguishes standing
owner decisions from proposals that still need approval.

Reviewed and amended 2026-10-09 against the pin, the current entry points,
ADR 0020 and the October measurements. Existing accepted budgets remain in
force; a proposed choice below is not an owner approval. References to
"architecture item" name section 3; "owner decision" names section 8.

Planning reference: `main` at `89144d8f`: Phase 6 A0 to A5 merged
(#105 to #111 with the review fixes #110 and #113), the October profiling
passes (#112, #114, #115, #116) and the refreshed performance records (#117).
Upstream remains Corsa `1f70213d4922b434345f639b441681e470c7cfc1`.

## 1. Outcome

Cut-over is the state in which the Rust binary replaces the pinned Go binary
for the pin's own clients and for the owner's projects, and the repository
can say so from committed files. At the end of Phase 7:

- **Correctness.** Every suite's expectation file holds only owner-approved
  entries (true today for all seven suites, with two approved `tsc` trace
  differences). Four suites are added: compiler and transpile separately
  through the bare WebAssembly instance and through an external Rust
  consumer of `tsr_embed`, graded by the same runner and the same baselines
  as the native suite. A browser smoke covers parse, check, emit and deep
  input in each supported engine.
- **Performance.** The five TypeScript-benchmarking scenarios (vscode,
  self-compiler, mui-docs, xstate, bluesky) are frozen and recorded under
  `status/perf/` for full checking and checking plus emit at 2, 4 and 8
  checkers, and meet PLAN's budgets (elapsed at most 1.0 of Go, peak RSS at
  most 0.70) for four consecutive weekly runs. The five LSP scenarios meet
  their 1.0 targets. The WebAssembly and embedding budgets of ADR 0020 are
  recorded and met on the same scenarios. The pin's JS API benchmark files
  are recorded against the Rust server.
- **Hardening.** Differential fuzzing against the pinned Go binary runs in
  the repository with its findings as fixtures; crash reports replay; the
  deep-input stress covers native and WebAssembly stacks; the sanitizer and
  Miri lanes run on the ownership contracts; profiling is either available
  or explicitly out of scope by decision.
- **Release.** The four native targets of ADR 0002 are built and tested in
  CI, the Linux binaries honour the glibc 2.28 floor, the release layout is
  what the pinned `packages/typescript` client and the VS Code extension
  resolve, and the lockstep crates.io release carries it.
- **Closure.** The ledger has no planned or in-progress file: every pinned
  file is ported or owner-approved out of scope with a reason, Phase 5's
  remaining L8 work included. The dogfood period of PLAN section 5 (four
  weeks, no open P1 crash) is recorded. If the owner chooses the pre-cut-over
  rehearsal in owner decision 12, a separate pin-bump branch exercises
  PLAN section 4's maintenance loop ("the target moves").

## 2. Starting point

### What exists

- `tsrust`: the command line, `--lsp --stdio` and `--api` with the pin's
  client suites passing (`fourslash` 4,534, `lsp` 13, `jsapi` 12 files with
  706 cases), the compiler and transpile corpus (13,432 variants in both
  checker modes, 28 transpile cases) and 516 recorded command-line
  scenarios. `tsrust` 0.3.0 and the 47 library crates are on crates.io
  (2026-10-05); the repository has 53 crates, 5 of them repository-only.
- `tsr_embed`: parser-only by default, a `checker` feature with an immutable
  program session, scoped queries, retained handles and an in-memory emit;
  `tools/s10/rust-consumer` with its own manifest. `tsr_wasm`: the bare
  wasm32 build with the same two surfaces, the factory wrapper and
  `tools/s10/wasm/instance.mjs`; the checker build is 23.3 MB raw and
  20.0 MB after Binaryen. Both ran S10's frozen Phase 0 subset (10,728
  variants, all applicable comparisons exact); neither has run the full
  corpus, and their emit carries no acceptance claim (Phase 3 decision 9).
- Measurement: `scripts/parity.py` and `tsr_testrunner` for the suites;
  `scripts/perf.py` and the dispatch-only `perf.yml` for three recorded
  workloads (`parse-bind`, `checker`, `lsp`) with `status/perf/thresholds.toml`;
  the S07 parse-bind harness with its Go bridge built from the pinned
  sources; the S08 checker query workload; the Phase 5 latency capture
  (`tools/phase5/latency`); the profiling recipes and the concurrent-pair
  method of `docs/PERF-profiling-2026-10.md`.
- Ownership and failure contracts: E3's cases, the shared-pool panic
  retirement through the real API client (Phase 6 A5), the replay corpus of
  Phase 5, the growth guards and reserved stacks of ADR 0011.
- CI: lint, sharded Rust tests, script tests, MSRV on Linux and macOS, the
  runner build, the seven parity suites, the `tsc` suite and native crates
  on `ubuntu-latest` and `macos-latest`, the pages job.

### What is missing

- The five benchmarking scenarios: no frozen inputs, no harness, no records.
  The S12 matrix says the VS Code parse-bind inventory cannot stand in for
  them. Both binaries already expose `--checkers`; R0 must exercise that
  option and record the effective pool size, not add another pool control.
- Full-corpus acceptance through WebAssembly and the consumer, their emit,
  the browser smoke, and every budget of ADR 0020 for them (artifact bytes
  against the Go `GOOS=js` build, elapsed and memory in Node, the consumer's
  elapsed, RSS, retained bytes and linked size).
- Two of the four native targets in CI (macOS x64 and Linux arm64 are
  "paused", not removed, per S12), the glibc 2.28 image run, the release
  layout, the client-resolution check, the dogfood.
- Differential fuzzing and replay for compiler, API and portable-entry-point
  crashes. The LSP replay corpus and runner already exist. Production builds
  use stable Rust; Miri/ASan already use the separate pinned nightly in
  `data/s04/toolchains.toml`. Also missing is the deep-input stress on
  WebAssembly (Node runs with `--stack-size=4096`, a measured requirement,
  not a browser result), the four pending technical decisions of PLAN
  section 13 (storage layout, interning, trampolines, the `checkchildren`
  replacement).
- Profiling: `--pprofDir` and the three API profiling methods answer "not
  available"; `internal/pprof` is the one file the ledger assigns to Phase 7.
- Ledger closure: Phase 5 shows 74 planned and 41 in-progress files (its L8
  bookkeeping: ledger homes, the 93 direct-test assignments, port
  completion), Phase 1 shows 38 planned and 60 in progress, `project/dirty`
  and `semver` are mapped at 0%; 8,464 of 11,485 functions carry a marker.

### The measured state

Rust over Go unless stated; records of 2026-10-08 on the owner's host
(`status/perf/`), development numbers marked as such. The checker capture
explicitly reports `host_busy: true` (one-minute load 5.16–7.17, limit 2.0);
its 1.626 ratio is an observation, not a quiet-host acceptance run. Keep the
qualification from [the refresh record](PERF-profiling-2026-10.md#full-performance-refresh-8-october).

| Workload | Measure | Today | Target | Authority |
| --- | --- | ---: | ---: | --- |
| parse-bind (VS Code tree) | wall, 1 worker | 1.101 | 1.25 | ADR 0021 |
| | wall, 8 workers | 1.262 | 1.45 | ADR 0021 |
| | peak RSS | 0.672 | 0.85 | ADR 0021 |
| | allocated bytes | **0.874** | 0.85 | ADR 0021; about 69 MB over this workload's budget; sealing is a candidate to measure in R1 |
| checker query workload (S08) | elapsed | 1.626 | none recorded | the full-checking budget applies to the scenarios, not to this slice |
| | throughput | 0.615 | | |
| | retained bytes | 1.422 | none for this slice | ADR 0020 requires a separate consumer measurement with equivalent work and live roots |
| | type footprint | 0.813 | 0.85 | ADR 0022 |
| LSP (Phase 5 fixture) | first diagnostics | 1.257 | 1.0 | L7.6.3 |
| | completion | 2.330 | 1.0 | |
| | hover | 1.823 | 1.0 | |
| | references | 1.552 | 1.0 | |
| | rename | 1.297 | 1.0 | |
| `tsc -p` on one project (development, 2026-10-08) | single-threaded | about 2× | 1.0 at 2/4/8 checkers on the scenarios | PLAN section 5 |
| native binary | `tsrust` 46.8 MB against `tsgo` 39.1 MB (macOS arm64) | 1.20 | none in PLAN; owner decision 6 | |
| WebAssembly parser build (S10, 2026-09) | artifact bytes | 0.268 | 0.30 | ADR 0020, parser-only |
| | parse throughput | 1.68 | ≥1.5 | ADR 0020, parser-only |
| Node in-process parse latency (S10) | against the socket path | 0.34 | ≤0.40 | ADR 0020 |

The earlier profiles identify candidates: node access, binding, flow and
name resolution, plus project load and navigation for LSP. Their phase
ratios and self-time shares predate some of the retained changes and are
not an attribution of the refreshed whole-workload numbers. Re-profile on
the R0 control before ranking candidates, including the kind-hint design
(`docs/design/node-kind-bits.md`); do not add savings from separate batches
or call a candidate the only remaining lever. Nothing measured says the 1.0
budgets are reachable; nothing says they are not. Phase 7 finds out with
the scenarios, and a budget change is a separate owner decision with an ADR,
as ADR 0021 and 0022 were.

## 3. Architecture decisions

Proposed for the owner's acceptance; section 8 lists what each needs.

1. **The scenarios are frozen before anything is tuned.** Each of the five
   scenarios is a committed descriptor (`tools/phase7/scenarios/<name>.json`:
   the source revision, the dependency closure and its digest, the compiler
   options, the requested output modes, the oracle's work digest) and a
   provisioned checkout under the workload cache, as S07's VS Code inventory
   is. The oracle is `tsgo` built from the pin. Checking and checking plus
   emit are separate modes; nothing is aggregated across scenarios.
2. **Use the existing checker-count option.** Both CLIs accept `--checkers`
   (`upstream/tsc/internal/tsoptions/declscompiler.go`,
   `crates/tsr_tsoptions/src/option_declarations_generated.rs`); both pools
   read it, default to four, and clamp to the file count and 256. R0 uses
   `--checkers 2|4|8` with `singleThreaded` disabled and records the effective
   count. Library drivers set the same existing option. Use ordinary release
   binaries for end-to-end runs. If separate operation clocks or allocation
   counters need a bridge, restrict it to observation around those same
   production entry points, record its identity and compare its work with
   the ordinary binary. Do not replace the pool or its scheduling policy.
3. **One measurement protocol, two uses.** The record of truth is
   `perf.py record` from a serial, alternating capture on the owner's host
   with the host load retained in the record, as #117's refresh did.
   Development comparisons use concurrent pairs (baseline and candidate
   started together, scored per pair). Every number in a record or a PR
   says which it is. Thresholds stay informational per PR and are judged
   on the weekly runs. Before timing, R0 fixes warmups, pair counts,
   confidence/stability rules, any bounded extension, and host eligibility,
   reusing the existing harness rules where applicable. Build, provision and
   profile outside timing; exclude other timed jobs on the measurement host.
   A busy-host or inconclusive run is retained but does not qualify a weekly
   acceptance result. Scheduling a job does not establish a quiet host.
4. **Acceptance through the existing runner.** The WebAssembly instance and
   the consumer do not get their own comparators: `tsr_testrunner` expands
   the cases and composes and compares the baselines as it does for the
   native suites, and a compile backend compiles through the in-memory host
   of `tsr_wasm` (in Node) or through `tsr_embed`'s public API (in the
   external consumer process). Use `compiler-wasm`, `transpile-wasm`,
   `compiler-embed` and `transpile-embed`, each with its expectation file and
   native suite's applicable case/subtest roster. An `Unsupported` result
   is a failing entry, never a skip. The two `tsc` trace-order approvals do
   not apply to these suites.

   Keep case expansion, baseline rendering and comparison in the host runner;
   send requests to the tested artifact and receive its diagnostics, query
   observations and emitted bytes. The runner must not check or emit a missing
   result on its behalf. `tsr_testrunner` and `s10_corpus` currently depend on
   compiler internals, so neither belongs in the acceptance consumer's
   dependency graph. Its own manifest links public `tsr_embed` operations and
   public host/value types needed by that API; production compile/query/emit
   calls go through `tsr_embed`. Any shared request/result DTO library must
   be independent of the compiler. Audit all applicable baseline domains,
   including resolution traces and content-mapped outputs, and supply missing
   public observations rather than silently dropping them. List the native
   runner's additional internal assertions separately and state which public
   observation or native-only test owns each; do not report an unexecuted
   assertion as a portable pass.

   The wasm adapter uses the same in-memory test-mapper behavior as native
   through portable host callbacks; no native process or filesystem fallback.
   A compiler-side API gap belongs in the deliverable. Prove the boundary by
   making the tested artifact fail: the corresponding row must fail too.
5. **Bounded differential fuzzing.** Start with a runner that mutates corpus
   bytes/tokens and generates small programs, with deterministic seeds and
   per-case time/memory limits. Use isolated CLI compilations for whole
   programs and narrow pinned-package drivers for scanner/parser observations;
   `tsr_testhost` has no compilation RPC. Compare diagnostics, emit and
   termination behavior. Coverage-guided mutation and a Go differential oracle
   are compatible; evaluate coverage-guided targets with the existing pinned
   instrumentation nightly if blind mutations stall. Keep any new dependency
   decision under ADR 0017, not a claim that the repository has no nightly.
6. **Replay the failing entry point.** LSP sessions use the existing runner
   under `tools/phase5/replay`. Compiler failures become compiler/transpile
   fixtures; JS API failures become client regressions; wasm traps and
   consumer lifetime failures become portable/public-API regressions. Retain
   each reproducer's inputs, options, operation sequence and failing boundary.
   Running a native compiler fixture alone cannot certify a wasm or public
   lifetime fix. Use the existing suite and unit-test machinery for each route.
7. **Profiling.** Two admissible answers, the owner's choice (owner decision 2):
   write pprof-format CPU and heap profiles from a sampling profiler behind
   the existing flags and API methods (a dependency, recorded under
   ADR 0017), or declare `internal/pprof` out of scope in the ledger and
   keep the explicit "not available" answers, which the pin's clients accept.
8. **Release layout.** The release artifact mirrors upstream's platform
   package (`lib/tsc` beside the lib files), verified by pointing the pinned
   `packages/typescript` client's `getExePath` and the extension at a local
   package; publishing under upstream names stays a non-goal.
9. **Ledger closure rule.** Audit remaining operations and direct-test routes
   in each touched file; touching one function is not completion of the file.
   Keep partial files in progress until all required behavior and tests are
   accounted for. Out-of-scope decisions require the owner and a reason
   independent of whether the current suites reach the code. R7's exit is a
   ledger with no planned or in-progress entry and `cargo xtask status`
   showing it; L8 may uncover implementation work as well as missing markers.

## 4. Working rules

- **Who.** Proposed in owner decision 11; the plan is written so that each
  checkpoint is one agent's, with reviews and records by another.
- **Order and overlap.** R0 lands first. R1 and R2 run in parallel after
  it; R3 and R4 start their correctness halves after R0 and take their
  budgets after R1; R5 runs alongside; R6 follows the code of R1 to R5;
  R7 is last and includes the four-week acceptance period. R0 establishes
  measurements; an optimization miss does not block its completion.
- **Checks per commit.** The crate's tests and clippy with warnings denied,
  `cargo fmt --all --check`, `cargo xtask validate`, `parity.py run <suite>
  --id` for touched variants; CI runs the full suites. A performance change
  carries its concurrent-pair number in the commit message and keeps the
  output hashes of the workloads it touches.
- **Records.** One record per checkpoint, `docs/PHASE7-R<n>.md`, with the
  commands, the numbers at exit and what the next checkpoint inherits.
  `status/perf` recordings are the owner's host's; a development number is
  labelled as such everywhere it appears.
- **Measurement cost.** Use focused parity and bounded concurrent candidate/
  control pairs while developing. Screen connected storage/CPU changes as
  one combined candidate; component measurements explain costs, not additive
  savings. Run the complete affected acceptance matrix at checkpoint exits
  and weekly acceptance, not after every small landing. Preflight one sample
  of each new mode, validate its work and estimate the full run before starting
  it. Preserve valid completed captures and do not repeat them for doc edits.
- **Expectation files.** `parity.py accept` per PR so the files are exact;
  reasons are causes; `approved` is the owner's word.
- **No new evidence machinery.** The suites, `perf.py`, the records and the
  status page are the whole of it.

## 5. Checkpoints

```text
R0 -> { R1, R2, R3, R4, R5 } -> R6 -> R7
```

### R0 — the measurement of record

Work items:

1. Freeze the five scenarios (architecture item 1): descriptors, provisioning
   (`scripts/phase7_scenarios.py provision`), the oracle build, and the
   work digest of each (the files, options and modes the oracle compiles).
2. The harness (`tools/phase7/bench`): the existing checker-count option
   for 2, 4 and 8 checkers (architecture item 2), the two modes, cold full
   checking in a fresh process per sample. Declare launch/startup and operation
   boundaries identically: operation work includes load, parse, bind, check
   and, for the emit mode, emit. Report startup separately. Peak process RSS
   includes the program and its inputs; allocation traffic and retained bytes
   are separate, with GC/disposal and live roots declared as S12 asks. Compare
   file sets, options, diagnostics, exit status and emitted bytes on every
   sample before keeping a time. A planted wrong result or omitted operation
   must fail preflight before the full capture begins.
3. `perf.py` workloads `check-<scenario>` and `emit-<scenario>` with ratios
   `elapsed_2`, `elapsed_4`, `elapsed_8`, `peak_rss_2`, `peak_rss_4`,
   `peak_rss_8`; `thresholds.toml` rows at PLAN's 1.0 and 0.70; `perf.yml`
   runs them by dispatch and, if owner decision 4 says so, weekly on the owner's
   runner.
4. The API benchmark workload: enumerate named tasks in the pin's
   `test/sync/api.bench.ts` and `test/async/api.bench.ts`, record one ratio
   per server-dependent task and sync/async mode under `api` (target in owner
   decision 7). Keep spawn, project load, transfer and query costs separate;
   a file-wide average must not hide a slow operation. Freeze the client,
   Node version, dependency fixture, setup/warmup and response work checks.
   The `TS - ...` tasks run the legacy JS compiler; `test/nodelist.bench.ts`
   constructs and decodes a buffer entirely inside the JS client and starts
   neither server. Record these as client controls, without Rust/Go server
   ratios or credit toward compiler acceptance. Do not alter the pinned
   task bodies to turn client-only work into a server benchmark.
5. Phase 5's L8 work, if owner decision 9 puts it here: audit ledger homes
   and the 93 WORK test routes, port missing behavior/tests and update the
   Phase 5 status only when resolved. An exact existing test may discharge
   a route; a source reference or a green client suite alone may not. If
   this uncovers substantial work, track it alongside R1–R5, required before
   R6, rather than blocking measurement bring-up or marking it done.

Witnesses: the five descriptors reproduce their digests from a clean cache;
each scenario's first record in both modes at 2/4/8 with the misses visible;
the `api` per-task records and client controls; the harness rejecting a planted
difference before a full capture.

Exit: every scenario recorded once per mode on the owner's host, the
accepted thresholds in place, proposed thresholds clearly labelled pending
their owner decision, and `docs/PHASE7-R0.md` with the starting table and
sampling contract. Preserve the parse-bind allocation miss for R1; fixing
it is not a prerequisite for the other checkpoints.

### R1 — full checking

The budget is 1.0 elapsed and 0.70 peak RSS at 2, 4 and 8 checkers on every
scenario. The method is the October passes': profile the scenario, attribute
against the pin's profile function by function, change one thing, measure in
pairs, keep what pays.

Candidate work items; order them by fresh attribution of the R0 control:

1. The binder (1.8x in the earlier parse-bind profile): node reads through
   the view, the hashed name and symbol tables, the symbol-graph validation;
   the bounded experiments of #116 are the starting point.
2. Kind hints on node edges (`docs/design/node-kind-bits.md`), as a bounded
   prototype first, with the recount it asks for.
3. The checker's remaining spread: flow analysis after the reference shape,
   name resolution, the relater's member vectors, the deep-expression
   walks; `resolve_object_type_members` and the relater copy member vectors
   the pin shares.
4. Memory: diagnose the parse-bind 0.874 allocation miss and the scenario
   memory profiles. Moving a single populated page at seal, with a measured
   first-page capacity policy, is a candidate rather than a prescribed fix;
   account for slack, transient traffic and CPU together. The checker slice's
   retained ratio of 1.42 identifies a risk, not a measurement against R4's
   consumer budget. Type and symbol storage layouts (PLAN section 13 item 6)
   are decided on the scenarios' census, and interning (item 8) on measured
   string duplication. A layout change also checks parse-bind and checker
   output equality and their existing thresholds.
5. Multi-checker scaling: the pool's file association and the program's
   parallel phases on the scenarios at 2/4/8; the lockstep lesson of the
   parse-bind harness (idle time is the harness's, not the port's) is
   checked for the real pool.

Witnesses: each candidate's bounded pair results, rejected attempts retained;
the complete affected scenario matrix at checkpoint exit; output hashes
unchanged and the full suites green. Small samples and the checker top-60
diagnostic cannot establish acceptance of a storage change.

Exit: every scenario at or under the budgets in both modes, or the measured
residual gap attributed function by function with the owner's decision on it
(a re-base is an ADR, not a note). `docs/PHASE7-R1.md`.

### R2 — editor and API latency

Work items:

1. Profile the current LSP scenarios, starting with completion (2.33) and
   hover (1.82). Earlier profiles point to the navigator's general view
   routing and a vector of children per level; test reading each child once
   through the file's direct view. Check the shared paths in references
   (1.55) and rename (1.30), and project load/checker construction in first
   diagnostics (1.26), before choosing changes.
2. Profile the per-task API benchmarks. Candidates from the earlier work
   are symbol registration/node handles, a per-request file-table cache
   and the sync protocol's inline callback path.
3. Project load on the owner's projects (the dogfood fixtures of R7), timed
   against the pin with the Phase 5 capture driver.

Witnesses: bounded pairs for candidates; the Phase 5 capture
(`tools/phase5/latency/capture.py`, twenty alternating pairs with its declared
extension rule, response and traffic equality) at checkpoint exit; per-task
`api` records; the `fourslash`, `lsp` and `jsapi` suites green.

Exit: the five LSP ratios at or under 1.0 on the fixture and the `api` ratios
at their approved targets, or an explicit owner ADR amending a target after
attribution. A measured residual alone does not close acceptance.
`docs/PHASE7-R2.md`.

### R3 — WebAssembly acceptance

Work items:

1. The `compiler-wasm` and `transpile-wasm` suites (architecture item 4):
   runner backends that compile each variant in a Node-hosted `tsr_wasm`
   instance through the in-memory host (files, libraries, symlinks, directory enumeration and package
   metadata supplied in memory, as `MemorySnapshot` and `BundledFs` do
   natively), with emit (`.js`, `.map`, `.d.ts`) through the session's write
   callback, and expose both transpile-module and transpile-declaration
   operations through the wasm API. Start with the native single-checker
   rosters (13,432 compiler variants and 28 transpile variants at the planning
   pin), including every applicable subtest. Test-case expansion and baseline
   rendering stay on the host; compilation and queries stay in the instance.
   Keep source bytes and ordered options intact across that boundary.
   Both expectation files start exact and must close with zero unapproved
   failures; no `tsc` approval is inherited. Run both in CI. A missing result,
   trap, timeout or mismatch fails its row, and a new instance handles the next
   case. Native selection guards remain visible; implementation gaps do not
   change the denominator.
2. Deep input on wasm32: the stress fixtures of ADR 0011 through the
   instance with the stack configuration recorded (Node's `--stack-size`,
   the instance's own stack), failures declared rather than inferred.
3. The browser smoke (owner decision 3): parse, check, emit and deep input
   in each supported engine, driven by a committed page and the engines'
   automation; results recorded with the engine versions.
4. Budgets: the full artifact against an equivalent Go `GOOS=js` build of the pin
   (uncompressed, glue declared separately, corpus adapters excluded from
   both); checking and checking plus emit on each scenario at one checker in
   the same pinned Node, elapsed and peak and retained process RSS, linear
   memory and allocator counters reported beside them. Use an equivalent
   parse/check/emit surface on each side, excluding CLI/watch services and
   test adapters from both artifacts. Report startup/module compilation
   separately from operation latency, and declare identical live results,
   GC/disposal and post-operation checkpoints for retained process RSS.
   Each of artifact bytes, operation elapsed, peak process RSS and retained
   process RSS has the ADR 0020 limit 1.0; wasm linear memory is not the RSS
   denominator. Workloads `wasm-size` and `wasm-<scenario>` under
   `status/perf` report the two operation modes separately.

Witnesses: the suite in CI; the smoke's recorded results; the records.

Exit: `compiler-wasm` and `transpile-wasm` exact with only approved entries;
the smoke passing in every approved engine; all budget rows recorded and met,
or an explicit owner-approved ADR amendment. Attribution alone is not a pass.
`docs/PHASE7-R3.md`.

### R4 — embedding acceptance

Work items:

1. The `compiler-embed` and `transpile-embed` suites (architecture item 4):
   the external consumer (`tools/s10/rust-consumer`, own manifest and lockfile)
   gains a suite mode that compiles each variant through `tsr_embed`'s public API with a
   consumer-supplied host and emits through the session's in-memory emit;
   the host runner grades it. Add public transpile operations and run both
   transpile kinds, not a full-program substitute for them. Remove the
   consumer's current `s10_corpus` dependency for this acceptance path; it
   reaches compiler internals. Verify from the consumer's source and Cargo
   dependency graph that no compile/query/emit fallback bypasses `tsr_embed`,
   including through test dependencies. Needed public host/value crates are
   allowed, as architecture item 4 specifies. Documented operations and host
   callbacks are enumerated in the consumer and each is exercised by at least
   one variant or lifetime case; an operation the consumer cannot reach is a public API
   gap, fixed in `tsr_embed`.
2. Lifetime and failure contracts through the public entry points: applicable
   E3 cases, malformed inputs, cancellation, reentry, retained results across
   drops, repeated create/query/drop with the owner and storage counters back
   at baseline and native panic invalidation. Enumerate each case's public
   route; an internal-only test is not the public-entry-point witness. Run
   applicable native cases in release, Miri and AddressSanitizer (the existing
   ownership recipes); run trap invalidation and fresh-instance recovery in
   R3's actual wasm host. Native instrumentation does not certify wasm traps.
3. The facade (`tsr`) and `tsr_embed` documentation: the public surface,
   what a host supplies, what a dropped session revokes and what it does
   not, with doctests that compile.
4. Budgets: the consumer at one checker against one-checker Go on each
   scenario (elapsed 1.0, RSS 0.70), retained bytes against the pin's live
   results (1.0), the linked consumer executable against a Go consumer built
   for the same work (1.0). Fixed positive comparable live roots, GC/disposal
   and a declared post-operation checkpoint make retained bytes comparable;
   report allocation traffic separately. Workloads `embed-<scenario>` (check
   and check+emit separately) and `embed-size`.

Witnesses: the suite in CI; the lanes; the records; the doctests.

Exit: `compiler-embed` and `transpile-embed` exact with only approved entries;
every applicable lifetime case passing through the public API; budgets met
or an explicit owner-approved ADR amendment. Attribution alone is not a pass.
`docs/PHASE7-R4.md`.

### R5 — hardening

Work items:

1. Differential fuzzing (architecture item 5, owner decision 5):
   `tools/phase7/fuzz` with its generators and isolated compiler drivers,
   a scheduled or dispatch job with a bounded budget, findings minimized
   into `testdata`-style fixtures that
   the parity suites or unit tests then own. The first targets are the
   scanner and parser on mutated corpus files, then the checker on generated
   programs, then emit.
2. Crash replay (architecture item 6): route dogfood and fuzz failures to the
   affected entry point's runner. Add a bounded regression that demonstrates
   failure detection for each route used; no requirement to manufacture a
   real fuzz finding before R5 can complete. Minimized discovered failures
   remain regression fixtures after their fixes.
3. Resource limits: the reserved stacks and growth guards audited on the
   scenarios and the fuzzer's deepest inputs; the watcher's descriptor use
   on the owner's largest project; memory under `--watch` over repeated
   equivalent edit cycles. After warmup and dropping superseded roots, require
   bounded live owners/storage and report the RSS trend separately; allocator
   high-water RSS is not proof of a leak or a zero-byte teardown contract.
4. PLAN section 13's pending decisions, with evidence: trampolines at the
   four left-operand sites (item 11, the stress fixtures decide), the
   `checkchildren` replacement (item 14: a `dylint` or `rustc_driver` lint
   detecting omitted child checks, tested with a planted omission). Replacing
   that automated check with review alone requires a separate owner amendment
   to PLAN, not a completion claim. Record the storage layout and interning
   decisions from R1 here too (items 6 and 8).
5. Profiling (owner decision 2) implemented or declared, with the ledger entry for
   `internal/pprof` moved accordingly.
6. The `Unsupported` and "not available" paths enumerated (`rg` over the
   crates), each either implemented, out of scope by decision, or a
   documented limit.

Witnesses: the bounded fuzz job's seeds, executions, coverage where available,
and retained/minimized failures; regression runners detecting injected failures
at the relevant entry points; limit measurements and the lint's mutation test.

Exit: the fuzz and replay jobs in the workflows; the four decisions recorded
as ADRs; no `Unsupported` path without a decision. `docs/PHASE7-R5.md`.

### R6 — native release

Work items:

1. Four targets in CI: macOS x64 and Linux arm64 restored (S12: paused, not
   removed). All required native suites execute on all four target architectures,
   not merely cross-compile. A missing runner is a release blocker, not an
   exemption. Linux arm64 may be cross-built, but still needs an execution
   runner. Both Linux architectures' final ELF files are checked for imports
   above glibc 2.28 and their suites execute on that floor, as ADR 0002 says.
2. The release layout (owner decision 8): the artifact staged as the platform
   package the pinned client expects, `getExePath` and the extension
   resolving it from a local install, the scripts of `tools/packaging`
   extended to produce it.
3. Binary size (owner decision 6): `tsrust` against `tsgo` on each target, the
   release profile's choices (LTO, codegen units, symbol stripping, panic
   strategy) measured for size and speed together, nothing traded for speed
   that R1 did not account for.
4. Prepare the next lockstep release (proposed 0.4.0, owner decision 8),
   re-check publication policy and build the packaged archives and external
   consumers (`scripts/package_assets.py --check`, `scripts/package_verify.py`).
   Keep already published release records unchanged. Registry publication
   needs the owner's release authorization; R6's exit is the verified candidate,
   not an automatic publish or a claim that four-week acceptance has completed.

Witnesses: the four CI jobs green; the glibc check failing on a planted
newer symbol; the client and the extension running from the staged package.

Exit: artifacts for the four targets from CI, the layout verified, the
release notes in `docs/RELEASE-0.4.0.md`. `docs/PHASE7-R6.md`.

### R7 — dogfood and cut-over

Work items:

1. The dogfood (owner decision 10): the owner's projects and the VS Code
   extension on the Rust binary for four weeks; crash reports into R5's
   replay; the P1 log in the record.
2. The weekly runs: four consecutive weekly collections on the frozen
   scenarios, each with all required release budgets met and correctness
   checks passing on its candidate revision. Retain all misses and invalid/
   inconclusive samples. A busy host does not qualify merely because a point
   estimate passes; a week with an unmet required budget does not qualify.
   Do not assemble closure from each workload's best four disjoint weeks.
3. If owner decision 12 places the pin bump rehearsal here, move `upstream/`
   to a later Corsa commit on an isolated branch, regenerate the ledger,
   port the diff for one small package end to end, run the suites, and record
   what the process cost.
   The rehearsal branch need not merge; the record says what moving the pin
   takes and records unresolved diff outside the selected package. Rehearsal
   results do not replace current-pin acceptance. Merging a new release pin
   requires rechecking the affected expectations and comparable workloads.
4. Closure: the ledger with no planned or in-progress entry (owner decision 9),
   PLAN's phase table and README updated, the status page final,
   `docs/PHASE7-R7.md` as the cut-over record naming the commit, the four
   records per workload and the dogfood log.

Exit: every row of section 6 checked, by the owner.

## 6. Acceptance and counting

| Required result | How it is checked |
| --- | --- |
| Compiler, conformance, transpile, config, tsc, build and watch baselines | The seven expectation files hold only owner-approved entries on the cut-over commit (CI) |
| At least 99.5% of fourslash with the rest triaged | `fourslash.json` as Phase 5 counts it: N from the Go run, F the distinct non-passing tests, approval does not remove a test from F |
| The pinned client suites, the LSP and project suites, the replay corpus | `jsapi.json`, `lsp.json`, the replay runner, in CI |
| Benchmark and memory targets on every scenario for four consecutive weekly runs | Four complete weekly collections under `status/perf`, with all required rows qualifying under the declared sampling/host rules on that week's candidate revision; a changed budget requires an owner ADR and new qualifying runs |
| Four weeks of dogfood with no open P1 crash | The dogfood log in `docs/PHASE7-R7.md`; a P1 is a crash, a wrong diagnostic or wrong emit against the pin on the owner's projects |
| The WebAssembly library runs the corpus through an in-memory host, emit included, no native process or file system; Node runs it; a browser smoke passes | `compiler-wasm.json` and `transpile-wasm.json` exact with only owner-approved failures; smoke and trap-invalidation results per engine |
| A separate Rust consumer runs the same comparisons through the public API; applicable E3, malformed-input, cancellation, panic-invalidation and create/drop tests pass through it | `compiler-embed.json` and `transpile-embed.json` exact with only owner-approved failures; the consumer's public-only dependency/call audit and lifetime suite; applicable sanitizer and Miri lanes |
| WebAssembly and embedding size, latency and retained-memory budgets fixed and met for four consecutive weekly runs | The `wasm-*` and `embed-*` workloads under `status/perf` with ADR 0020's rows in `thresholds.toml` |
| Four native targets | Required suites execute on all four; both Linux targets pass the glibc 2.28 check and suites on that floor; the pinned client resolves the staged layout |
| Required operations and direct tests accounted for | L8 and other partial-file audits resolved; `cargo xtask validate` passes and every file is ported or owner-approved out of scope; markers alone do not certify behavior |

Counting rules: a budget is per scenario and per mode, never aggregated; a
record on another host class or a busy host is informational; an `Unsupported`
result is a failing entry. Preserve development results but do not count them
as release acceptance. Trace approvals stay scoped to their named `tsc` rows.

## 7. Main risks

| Risk | Answer |
| --- | --- |
| The 1.0 checking budget is structural: the port runs the same algorithm with slower operations (`Result` on every accessor, arena-indexed types, hashed names) and the October passes took the large single-thread gains | R1 attributes the residual function by function on the real scenarios before anything is re-based; a re-base is the owner's ADR with the attribution attached, as ADR 0021 and 0022 were |
| The scenario inputs have not been provisioned for release measurement | R0 fixes the inputs and measurement boundaries first; implementation does not wait for R1 to fix an existing performance miss |
| A requested checker count is silently clamped or overridden | Use the existing `--checkers` option, disable `singleThreaded`, and record effective pool size and the workload's file set |
| WebAssembly runs the corpus without threads and on a fixed stack; a trap is terminal | The work groups run on the calling thread as program loading's do; the stack configuration is recorded; a trap is a failing entry, and the deep-input fixtures run first |
| The consumer's public API is narrower than the corpus needs | Architecture item 4 makes every applicable gap a failure in `compiler-embed` or `transpile-embed`, fixed in the public API; no compiler-internal fallback |
| The weekly runs need a quiet owner host for four weeks | Owner decision 4 chooses scheduling; host eligibility and exclusive measurement are checked separately, and busy-host observations cannot qualify |
| The dogfood finds crashes late | R5 starts before R7; minimize and replay each crash through the affected entry point |
| Blind mutations stop reaching new behavior | Differential comparison can be combined with coverage-guided mutation; use coverage and minimized failures to decide, not an assumption that the methods conflict |
| The ledger closure uncovers unported behaviour behind passing suites | Port and test the behavior, or obtain a scoped owner exclusion; an uncovered path is not automatically out of scope |
| Profiling as a dependency against ADR 0017 | Owner decision 2 chooses; the "not available" answers are already accepted by the pin's clients |

## 8. Decisions

Standing decisions and remaining choices for the owner. Numbers are stable
for the references above; already accepted requirements do not need approval
again.

1. **Budgets stand.** PLAN's 1.0 elapsed and 0.70 peak RSS at 2, 4 and 8
   checkers, the 1.0 LSP targets and ADR 0020's rows are the Phase 7
   targets; a change is an ADR after R1's or R2's attribution, not before.
   Already accepted by PLAN, ADR 0020 and the Phase 5 performance transfer;
   no threshold change is proposed here.
2. **Profiling.** Implement pprof-format profiles behind `--pprofDir` and the
   three API methods with a sampling-profiler dependency, or declare
   `internal/pprof` out of scope and keep the explicit "not available"
   answers. Recommended: out of scope; the pin's clients accept the answer
   and the owner profiles with native tools.
3. **Browser engines for the smoke.** Chromium, Firefox and WebKit through
   their automation, as repository-only tooling. Recommended: all three,
   results recorded per engine version.
4. **Weekly runs.** A scheduled `perf.yml` on a dedicated measurement runner
   or four weekly manual dispatches. Recommended: scheduled if an exclusive
   measurement host is available. The existing `runs-on` input can select it;
   `vars.PARITY_RUNNER` alone does not reserve a quiet host. Use a separate
   performance runner setting for schedules and prevent overlap with builds,
   parity and other timed work; retain and enforce the host-eligibility rules.
5. **Fuzzing approach.** Start with the bounded differential runner of
   architecture item 5; evaluate coverage-guided mutation on the existing
   instrumentation nightly if needed. Recommended: this staged approach,
   with dependency additions handled under ADR 0017.
6. **Native binary size.** Not in PLAN or ADR 0020; proposed target 1.0 of
   `tsgo` per target, measured in R6 after the release profile is chosen.
7. **API benchmark targets.** Not in PLAN or ADR 0020. **Decided by the
   owner 2026-10-10:** elapsed at most 1.25 of the pin per server-dependent
   named task and sync/async mode, and the servers' peak memory at most 0.85,
   recorded as workload `api`. Client-only controls carry no server budget.
8. **Release layout and packaging.** The staged platform package resolved by
   the pinned client and extension; npm publishing stays a non-goal;
   0.4.0 as the proposed next lockstep version, prepared at R6. Publishing
   the verified candidate remains an explicit release decision.
9. **Phase 5's L8 work** starts alongside R0 (recommended) or before it.
   **Decided by the owner 2026-10-10:** alongside R0.
   The audit may require implementation and direct-test ports; it is complete
   before R6, and Phase 5's status line changes only when it is resolved.
10. **Dogfood scope.** Which projects, and that the four weeks start when R6
    is merged; crashes get reproducers for their actual entry points.
11. **Allocation.** Proposed: Claude builds R0 and R7's records and reviews
    every checkpoint PR; Astra builds R1, R2 and R6; R3, R4 and R5 are split
    by crate after R0, as Phase 6's A5 was. **Accepted by the owner
    2026-10-10.**
12. **The pin bump rehearsal** in R7 (recommended) or after cut-over.
13. **Plan review:** completed by Codex/Astra on 2026-10-09, with amendments
    on this PR. This review does not approve the remaining owner choices.

## 9. After cut-over

What PLAN defers past this phase and what this plan does not claim: the
pin cadence as a maintenance loop (rehearsal timing is owner decision 12,
the cadence the owner's), Windows and musl (non-goals), publishing under
upstream names, and the query-based incremental architecture PLAN section 12 deferred until after
cut-over.
