# Phase 7: hardening, WebAssembly, embedding, cut-over

Status: **detailed plan, proposed** (2026-10-08). The detailed plan for
[PLAN Phase 7](../PLAN.md#phase-7-hardening-webassembly-embedding-cut-over),
the last phase: it turns PLAN section 5's exit criteria and the
[acceptance matrix](S12-acceptance.md) of ADR 0020 into checkpoints with
work items, witnesses and exit checks. Its decisions (section 8) are the
owner's and are open.

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
  differences). Two suites are added: the compiler and transpile corpus
  through the bare WebAssembly instance, and through an external Rust
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
  file is ported or out of scope with a reason, Phase 5's L8 bookkeeping
  included. The dogfood period of PLAN section 5 (four weeks, no open P1
  crash) is recorded, and the pin has been moved once, so the maintenance
  loop of PLAN section 4 ("the target moves") is exercised, not described.

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
  them. The pin fixes its checker count in `checkerpool.go`; neither binary
  exposes 2, 4 and 8 checkers as a flag.
- Full-corpus acceptance through WebAssembly and the consumer, their emit,
  the browser smoke, and every budget of ADR 0020 for them (artifact bytes
  against the Go `GOOS=js` build, elapsed and memory in Node, the consumer's
  elapsed, RSS, retained bytes and linked size).
- Two of the four native targets in CI (macOS x64 and Linux arm64 are
  "paused", not removed, per S12), the glibc 2.28 image run, the release
  layout, the client-resolution check, the dogfood.
- Fuzzing and crash replay (PLAN scope, nothing built; `cargo-fuzz` needs a
  nightly toolchain the repository does not use), the deep-input stress on
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
(`status/perf/`), development numbers marked as such.

| Workload | Measure | Today | Target | Authority |
| --- | --- | ---: | ---: | --- |
| parse-bind (VS Code tree) | wall, 1 worker | 1.101 | 1.25 | ADR 0021 |
| | wall, 8 workers | 1.262 | 1.45 | ADR 0021 |
| | peak RSS | 0.672 | 0.85 | ADR 0021 |
| | allocated bytes | **0.874** | 0.85 | ADR 0021; a miss since the sealed arena of #114 copies every node once (2.04 GB to 2.54 GB) |
| checker query workload (S08) | elapsed | 1.626 | none recorded | the full-checking budget applies to the scenarios, not to this slice |
| | throughput | 0.615 | | |
| | retained bytes | 1.422 | ≤1.0 on the consumer's retained results | ADR 0020 |
| | type footprint | 0.813 | 0.85 | ADR 0022 |
| LSP (Phase 5 fixture) | first diagnostics | 1.257 | 1.0 | L7.6.3 |
| | completion | 2.330 | 1.0 | |
| | hover | 1.823 | 1.0 | |
| | references | 1.552 | 1.0 | |
| | rename | 1.297 | 1.0 | |
| `tsc -p` on one project (development, 2026-10-08) | single-threaded | about 2× | 1.0 at 2/4/8 checkers on the scenarios | PLAN section 5 |
| native binary | `tsrust` 46.8 MB against `tsgo` 39.1 MB (macOS arm64) | 1.20 | none in PLAN; decision 6 | |
| WebAssembly parser build (S10, 2026-09) | artifact bytes | 0.268 | 0.30 | ADR 0020, parser-only |
| | parse throughput | 1.68 | ≥1.5 | ADR 0020, parser-only |
| Node in-process parse latency (S10) | against the socket path | 0.34 | ≤0.40 | ADR 0020 |

The profiling passes attribute the checker's remaining gap: node access
through the view is about a fifth of the check phase, the binder is 1.8x the
pin's where the parser is 1.09x, flow analysis and name resolution carry the
rest; the structural lever left is the kind hint on node edges
(`docs/design/node-kind-bits.md`). The LSP gap after the passes is in
project load, completion and the navigator. Nothing measured says the 1.0
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
2. **Checker counts through bridges built from the pinned sources.** The
   pin sizes its pool in `checkerpool.go`; a Go bridge (as
   `tools/s07/benchmark/main.go` is for parse-bind) and a `tsr_compiler`
   pool option expose 2, 4 and 8 checkers to the harness. `tsrust` and
   `tsgo` keep their defaults; the bridges are the measured binaries for the
   2/4/8 rows and are recorded by identity.
3. **One measurement protocol, two uses.** The record of truth is
   `perf.py record` from a serial, alternating capture on the owner's host
   with the host load retained in the record, as #117's refresh did.
   Development comparisons use concurrent pairs (baseline and candidate
   started together, scored per pair). Every number in a record or a PR
   says which it is. Thresholds stay informational per PR and are judged
   on the weekly runs.
4. **Acceptance through the existing runner.** The WebAssembly instance and
   the consumer do not get their own comparators: `tsr_testrunner` expands
   the cases and composes and compares the baselines as it does for the
   native suite, and a compile backend compiles through the in-memory host
   of `tsr_wasm` (in Node) or through `tsr_embed`'s public API (in the
   external consumer process). The two suites are `compiler-wasm` and
   `compiler-embed` with expectation files; an `Unsupported` result is a
   failing entry, never a skip. The consumer keeps its own manifest and
   links the runner's case and comparison library by path; it never links
   the compiler crates except through `tsr_embed`.
5. **Fuzzing without a nightly toolchain.** A differential runner in the
   repository generates inputs from the corpus (byte and token mutations,
   grammar-driven programs over the pin's node kinds) and compiles each with
   both binaries through the test-host protocol, comparing diagnostics and
   emit; a difference or a crash is minimized and committed as a fixture. No
   libFuzzer or `cargo-fuzz` dependency.
6. **Crash replay is the Phase 5 replay corpus.** A crash report from the
   dogfood or the fuzzer becomes a replay file under `tools/phase5/replay`
   and a parity entry until fixed; the witness is the existing replay runner.
7. **Profiling.** Two admissible answers, the owner's choice (decision 2):
   write pprof-format CPU and heap profiles from a sampling profiler behind
   the existing flags and API methods (a dependency, recorded under
   ADR 0017), or declare `internal/pprof` out of scope in the ledger and
   keep the explicit "not available" answers, which the pin's clients accept.
8. **Release layout.** The release artifact mirrors upstream's platform
   package (`lib/tsc` beside the lib files), verified by pointing the pinned
   `packages/typescript` client's `getExePath` and the extension at a local
   package; publishing under upstream names stays a non-goal.
9. **Ledger closure rule.** From R0 on, a checkpoint PR moves every file it
   touches to `ported` or `out of scope` with a reason; R7's exit is a
   ledger with no planned or in-progress entry and `cargo xtask status`
   showing it.

## 4. Working rules

- **Who.** Proposed in decision 11; the plan is written so that each
  checkpoint is one agent's, with reviews and records by another.
- **Order and overlap.** R0 lands first. R1 and R2 run in parallel after
  it; R3 and R4 start their correctness halves after R0 and take their
  budgets after R1; R5 runs alongside; R6 follows the code of R1 to R5;
  R7 is last and is mostly waiting, measured.
- **Checks per commit.** The crate's tests and clippy with warnings denied,
  `cargo fmt --all --check`, `cargo xtask validate`, `parity.py run <suite>
  --id` for touched variants; CI runs the full suites. A performance change
  carries its concurrent-pair number in the commit message and keeps the
  output hashes of the workloads it touches.
- **Records.** One record per checkpoint, `docs/PHASE7-R<n>.md`, with the
  commands, the numbers at exit and what the next checkpoint inherits.
  `status/perf` recordings are the owner's host's; a development number is
  labelled as such everywhere it appears.
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

1. Freeze the five scenarios (decision 1): descriptors, provisioning
   (`scripts/phase7_scenarios.py provision`), the oracle build, and the
   work digest of each (the files, options and modes the oracle compiles).
2. The harness (`tools/phase7/bench`): the Go bridge and the Rust pool option
   for 2, 4 and 8 checkers (decision 2), the two modes, cold full checking in
   a fresh process per sample, peak RSS and the allocator counters reported
   separately as S12 asks, output equality (diagnostics and emitted bytes)
   checked on every sample before a time is kept.
3. `perf.py` workloads `check-<scenario>` and `emit-<scenario>` with ratios
   `elapsed_2`, `elapsed_4`, `elapsed_8`, `peak_rss_2`, `peak_rss_4`,
   `peak_rss_8`; `thresholds.toml` rows at PLAN's 1.0 and 0.70; `perf.yml`
   runs them by dispatch and, if decision 4 says so, weekly on the owner's
   runner.
4. The API benchmark workload: the pin's `test/sync/api.bench.ts`,
   `test/async/api.bench.ts` and `test/nodelist.bench.ts` against both
   servers, recorded as `api` with one ratio per bench file (target per
   decision 7).
5. The parse-bind allocation miss: seal by moving the first page instead of
   copying when a file fits one page (size the first page from the source
   length), so the sealed arena keeps its read speed without the copy;
   measured in pairs on parse-bind and the checker top-60, then the
   `parse-bind` record refreshed by the owner.
6. Phase 5's L8 bookkeeping, if decision 9 puts it here: ledger homes for
   the Phase 5 files, the 93 direct-test assignments routed or ported, the
   Phase 5 plan's status line.

Witnesses: the five descriptors reproduce their digests from a clean cache;
each scenario's first record in both modes at 2/4/8 with the misses visible;
the `api` record; the harness's output-equality check failing on a planted
difference; the parse-bind pairs.

Exit: every scenario recorded once per mode on the owner's host, the
thresholds in place, the parse-bind `allocated_bytes` ratio back under 0.85,
`docs/PHASE7-R0.md` with the starting table of all scenarios.

### R1 — full checking

The budget is 1.0 elapsed and 0.70 peak RSS at 2, 4 and 8 checkers on every
scenario. The method is the October passes': profile the scenario, attribute
against the pin's profile function by function, change one thing, measure in
pairs, keep what pays.

Work items, in the order the attribution of #114 to #116 points at:

1. The binder (1.8x the pin's on parse-bind): node reads through the view,
   the hashed name and symbol tables, the symbol-graph validation; the
   bounded experiments of #116 are the starting point.
2. Kind hints on node edges (`docs/design/node-kind-bits.md`), as a bounded
   prototype first, with the recount it asks for.
3. The checker's remaining spread: flow analysis after the reference shape,
   name resolution, the relater's member vectors, the deep-expression
   walks; `resolve_object_type_members` and the relater copy member vectors
   the pin shares.
4. Retained memory: the checker slice retains 1.42x the pin's bytes; the
   consumer budget is 1.0. Type and symbol storage layouts (PLAN section 13
   item 6) are decided here on the scenarios' census, and interning (item 8)
   on measured string duplication.
5. Multi-checker scaling: the pool's file association and the program's
   parallel phases on the scenarios at 2/4/8; the lockstep lesson of the
   parse-bind harness (idle time is the harness's, not the port's) is
   checked for the real pool.

Witnesses: each change's pair number in its commit; the scenario records
after each landing; the output hashes unchanged; the full suites green.

Exit: every scenario at or under the budgets in both modes, or the measured
residual gap attributed function by function with the owner's decision on it
(a re-base is an ADR, not a note). `docs/PHASE7-R1.md`.

### R2 — editor and API latency

Work items:

1. The LSP scenarios, from the largest gap: completion (2.33) and hover
   (1.82) go through the navigator's general view routing and allocate a
   vector of children per level, where the pin reads pointers; a navigator
   that reads each child once through the file's direct view. References
   (1.55) and rename (1.30) follow the same path. First diagnostics (1.26)
   is project load and checker construction after the parse-ahead work.
2. The API benchmarks: symbol responses (registering symbols and building
   node handles is a third of the batched request), a per-request cache of
   the file tables, the sync protocol's inline callback path.
3. Project load on the owner's projects (the dogfood fixtures of R7), timed
   against the pin with the Phase 5 capture driver.

Witnesses: the Phase 5 capture (`tools/phase5/latency/capture.py`, twenty
alternating pairs, response and traffic equality) after each landing; the
`api` record; the `fourslash`, `lsp` and `jsapi` suites green.

Exit: the five LSP ratios at or under 1.0 on the fixture, the `api` ratios at
their target, or the attributed residual with the owner's decision.
`docs/PHASE7-R2.md`.

### R3 — WebAssembly acceptance

Work items:

1. The `compiler-wasm` suite (decision 4): a runner backend that compiles
   each variant in a Node-hosted `tsr_wasm` instance through the in-memory
   host (files, libraries, symlinks, directory enumeration and package
   metadata supplied in memory, as `MemorySnapshot` and `BundledFs` do
   natively), with emit (`.js`, `.map`, `.d.ts`) through the session's write
   callback. The expectation file starts exact and is driven to the native
   suite's two approved entries or to owner-approved wasm entries with
   reasons; the 13,432 variants and the 28 transpile cases run in CI on the
   parity runner.
2. Deep input on wasm32: the stress fixtures of ADR 0011 through the
   instance with the stack configuration recorded (Node's `--stack-size`,
   the instance's own stack), failures declared rather than inferred.
3. The browser smoke (decision 3): parse, check, emit and deep input in each
   supported engine, driven by a committed page and the engines' automation;
   results recorded with the engine versions.
4. Budgets: the full artifact against the Go `GOOS=js` build of the pin
   (uncompressed, glue declared separately, corpus adapters excluded from
   both); checking and checking plus emit on each scenario at one checker in
   the same pinned Node, elapsed and peak and retained process RSS, linear
   memory and allocator counters reported beside them. Workloads
   `wasm-size` and `wasm-<scenario>` under `status/perf`.

Witnesses: the suite in CI; the smoke's recorded results; the records.

Exit: `compiler-wasm` exact with only approved entries; the smoke passing in
every engine; the four budget rows recorded and met or attributed.
`docs/PHASE7-R3.md`.

### R4 — embedding acceptance

Work items:

1. The `compiler-embed` suite (decision 4): the external consumer
   (`tools/s10/rust-consumer`, own manifest and lockfile) gains a suite mode
   that compiles each variant through `tsr_embed`'s public API with a
   consumer-supplied host and emits through the session's in-memory emit;
   the runner grades it. Documented operations and host callbacks are
   enumerated in the consumer and each is exercised by at least one variant
   or lifetime case; an operation the consumer cannot reach is a public API
   gap, fixed in `tsr_embed`.
2. Lifetime and failure contracts through the public entry points: the E3
   cases, malformed inputs, cancellation, reentry, retained results across
   drops, repeated create/query/drop with the owner and storage counters back
   at baseline, panic and trap invalidation. Release, Miri and
   AddressSanitizer lanes on these (the Phase 2 Miri recipe).
3. The facade (`tsr`) and `tsr_embed` documentation: the public surface,
   what a host supplies, what a dropped session revokes and what it does
   not, with doctests that compile.
4. Budgets: the consumer at one checker against one-checker Go on each
   scenario (elapsed 1.0, RSS 0.70), retained bytes against the pin's live
   results (1.0), the linked consumer executable against a Go consumer built
   for the same work (1.0). Workloads `embed-<scenario>` and `embed-size`.

Witnesses: the suite in CI; the lanes; the records; the doctests.

Exit: `compiler-embed` exact with only approved entries; every lifetime case
passing through the public API; the budgets recorded and met or attributed.
`docs/PHASE7-R4.md`.

### R5 — hardening

Work items:

1. Differential fuzzing (decision 5): `tools/phase7/fuzz` with its
   generators and the test-host driver, a nightly or dispatch job with a
   bounded budget, findings minimized into `testdata`-style fixtures that
   the parity suites or unit tests then own. The first targets are the
   scanner and parser on mutated corpus files, then the checker on generated
   programs, then emit.
2. Crash replay (decision 6): the dogfood's crash reports and the fuzzer's
   crashes as replay files; the replay runner in CI.
3. Resource limits: the reserved stacks and growth guards audited on the
   scenarios and the fuzzer's deepest inputs; the watcher's descriptor use
   on the owner's largest project; memory under `--watch` over a long session
   (no growth across edits, measured).
4. PLAN section 13's pending decisions, with evidence: trampolines at the
   four left-operand sites (item 11, the stress fixtures decide), the
   `checkchildren` replacement (item 14: a `dylint` or `rustc_driver` lint,
   or the structural rule documented and checked by review), storage layout
   and interning (items 6 and 8, decided in R1).
5. Profiling (decision 2) implemented or declared, with the ledger entry for
   `internal/pprof` moved accordingly.
6. The `Unsupported` and "not available" paths enumerated (`grep` over the
   crates), each either implemented, out of scope by decision, or a
   documented limit.

Witnesses: the fuzz job's first findings and their fixtures; the replay
runner on a planted crash; the limit measurements.

Exit: the fuzz and replay jobs in the workflows; the four decisions recorded
as ADRs; no `Unsupported` path without a decision. `docs/PHASE7-R5.md`.

### R6 — native release

Work items:

1. Four targets in CI: macOS x64 and Linux arm64 restored (S12: paused, not
   removed), the Linux arm64 build cross-compiled as ADR 0002 says, the
   suites run on each where a runner exists and the `tsc` suite at least;
   the final ELF checked for symbol versions above glibc 2.28 and executed
   on a 2.28 image.
2. The release layout (decision 8): the artifact staged as the platform
   package the pinned client expects, `getExePath` and the extension
   resolving it from a local install, the scripts of `tools/packaging`
   extended to produce it.
3. Binary size (decision 6): `tsrust` against `tsgo` on each target, the
   release profile's choices (LTO, codegen units, symbol stripping, panic
   strategy) measured for size and speed together, nothing traded for speed
   that R1 did not account for.
4. The lockstep crates.io release (0.4.0) with the Phase 5 to 7 crates'
   publication policy re-checked (`scripts/package_assets.py --check`).

Witnesses: the four CI jobs green; the glibc check failing on a planted
newer symbol; the client and the extension running from the staged package.

Exit: artifacts for the four targets from CI, the layout verified, the
release notes in `docs/RELEASE-0.4.0.md`. `docs/PHASE7-R6.md`.

### R7 — dogfood and cut-over

Work items:

1. The dogfood (decision 10): the owner's projects and the VS Code
   extension on the Rust binary for four weeks; crash reports into R5's
   replay; the P1 log in the record.
2. The weekly runs: four consecutive runs of every `status/perf` workload on
   the frozen scenarios meeting their budgets, each a committed record; a
   miss restarts the count for that workload.
3. The pin bump rehearsal (decision 12): move `upstream/` to a later Corsa
   commit on a branch, regenerate the ledger, port the diff for one small
   package end to end, run the suites, and record what the process cost.
   The branch need not merge; the record says what moving the pin takes.
4. Closure: the ledger with no planned or in-progress entry (decision 9),
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
| Benchmark and memory targets on every scenario for four consecutive weekly runs | Four consecutive dated records per workload under `status/perf`, each at or under `thresholds.toml`, on the owner's host class; a miss or a re-based threshold is visible in the files and in an ADR |
| Four weeks of dogfood with no open P1 crash | The dogfood log in `docs/PHASE7-R7.md`; a P1 is a crash, a wrong diagnostic or wrong emit against the pin on the owner's projects |
| The WebAssembly library runs the corpus through an in-memory host, emit included, no native process or file system; Node runs it; a browser smoke passes | `compiler-wasm.json` exact; the smoke's recorded results per engine |
| A separate Rust consumer runs the same comparisons through the public API; E3, malformed-input, cancellation, panic-invalidation and create/drop tests pass through it | `compiler-embed.json` exact; the consumer's lifetime suite in CI; the sanitizer and Miri lanes |
| WebAssembly and embedding size, latency and retained-memory budgets fixed and met for four consecutive weekly runs | The `wasm-*` and `embed-*` workloads under `status/perf` with ADR 0020's rows in `thresholds.toml` |
| Four native targets | The CI matrix; the glibc check; the staged layout resolved by the pinned client |

Counting rules: a budget is per scenario and per mode, never aggregated; a
record on another host class is informational; an `Unsupported` result is a
failing entry; a development number is never a record.

## 7. Main risks

| Risk | Answer |
| --- | --- |
| The 1.0 checking budget is structural: the port runs the same algorithm with slower operations (`Result` on every accessor, arena-indexed types, hashed names) and the October passes took the large single-thread gains | R1 attributes the residual function by function on the real scenarios before anything is re-based; a re-base is the owner's ADR with the attribution attached, as ADR 0021 and 0022 were |
| The scenarios do not exist yet and their freezing is the work the budgets depend on | R0 is first and alone; nothing is tuned against an unfrozen input |
| The checker count is not a flag in either binary | Bridges built from the pinned sources, recorded by identity, as parse-bind already does |
| WebAssembly runs the corpus without threads and on a fixed stack; a trap is terminal | The work groups run on the calling thread as program loading's do; the stack configuration is recorded; a trap is a failing entry, and the deep-input fixtures run first |
| The consumer's public API is narrower than the corpus needs | Decision 4 makes every gap a failing entry in `compiler-embed`, fixed in `tsr_embed`, not worked around in the consumer |
| The weekly runs need a quiet owner host for four weeks | Decision 4 (a scheduled job on the owner's runner, host load retained in each record); the serial capture queue of #117 |
| The dogfood finds crashes late | R5's fuzzing starts before R7; every crash becomes a replay file the same week |
| Fuzzing without libFuzzer finds less | The differential oracle is the pin itself, which no coverage-guided harness has; findings are judged by what they fix |
| The ledger closure uncovers unported behaviour behind passing suites | A planned or in-progress file at R7 is either ported, or out of scope with the reason the suites pass without it |
| Profiling as a dependency against ADR 0017 | Decision 2 chooses; the "not available" answers are already accepted by the pin's clients |

## 8. Decisions

For the owner:

1. **Budgets stand.** PLAN's 1.0 elapsed and 0.70 peak RSS at 2, 4 and 8
   checkers, the 1.0 LSP targets and ADR 0020's rows are the Phase 7
   targets; a change is an ADR after R1's or R2's attribution, not before.
   Recommended: yes.
2. **Profiling.** Implement pprof-format profiles behind `--pprofDir` and the
   three API methods with a sampling-profiler dependency, or declare
   `internal/pprof` out of scope and keep the explicit "not available"
   answers. Recommended: out of scope; the pin's clients accept the answer
   and the owner profiles with native tools.
3. **Browser engines for the smoke.** Chromium, Firefox and WebKit through
   their automation, as repository-only tooling. Recommended: all three,
   results recorded per engine version.
4. **Weekly runs.** A scheduled `perf.yml` on the owner's runner
   (`vars.PARITY_RUNNER`) with the serial capture queue, or four manual
   dispatches. Recommended: scheduled, with the host load in each record.
5. **Fuzzing approach.** The differential runner of decision 5, without a
   nightly toolchain. Recommended: yes.
6. **Native binary size.** Not in PLAN or ADR 0020; proposed target 1.0 of
   `tsgo` per target, measured in R6 after the release profile is chosen.
7. **API benchmark targets.** Not in PLAN or ADR 0020; proposed 1.0 of the
   pin per bench file, recorded as workload `api`.
8. **Release layout and packaging.** The staged platform package resolved by
   the pinned client and extension; npm publishing stays a non-goal;
   0.4.0 as the lockstep release at R6.
9. **Phase 5's L8 bookkeeping** lands in R0 (recommended, one PR) or as its
   own PR before R0; either way Phase 5's status line changes with it.
10. **Dogfood scope.** Which projects, and that the four weeks start when R6
    is merged; crashes reported as replay files.
11. **Allocation.** Proposed: Claude builds R0 and R7's records and reviews
    every checkpoint PR; Astra builds R1, R2 and R6; R3, R4 and R5 are split
    by crate after R0, as Phase 6's A5 was.
12. **The pin bump rehearsal** in R7 (recommended) or after cut-over.
13. **Plan review**: one read-only review round by Astra before this plan is
    final, as Phase 6 had.

## 9. After cut-over

What PLAN defers past this phase and what this plan does not claim: the
pin cadence as a maintenance loop (one bump rehearsed in R7, the cadence the
owner's), Windows and musl (non-goals), publishing under upstream names, and
the query-based incremental architecture PLAN section 12 deferred until after
cut-over.
