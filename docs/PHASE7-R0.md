# Phase 7 R0: the measurement of record

The record of [PLAN Phase 7](../PLAN.md#phase-7-hardening-webassembly-embedding-cut-over)'s
first checkpoint ([docs/PHASE7-plan.md](PHASE7-plan.md), R0): the five
benchmarking scenarios frozen and measured against Go in both modes at 2, 4 and
8 checkers, the pin's JS API benchmarks measured against both servers, and
where Phase 5's L8 audit stands. Owner decisions 7, 9 and 11 were accepted on
2026-10-10 (section 8 of the plan).

## The scenarios

Each scenario is a descriptor under `tools/phase7/scenarios/`: the source
commit, the dependency install, the project, the arguments of each mode and the
oracle's frozen work. `scripts/phase7_scenarios.py provision` downloads the
commit's archive and installs the dependencies in a pinned Node container
(`node:24-bookworm@sha256:3d27e5…`) that sees only the checkout, as
typescript-benchmarking sandboxes its installs; `freeze` runs the oracle, tsgo
built from the pin, at four checkers and records its work; `verify` reruns it
and fails on any difference.

| Scenario | Source | Project | Files | Diagnostics (exit) | Types (oracle) | Emitted files |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| vscode | microsoft/vscode `a44adf7f` (the S07 parse-bind commit) | `src` | 9,628 | 347 (2) | 2,796,771 | 8,616 |
| self-compiler | microsoft/TypeScript `b465fdbf` (branch `strada`) | `src/compiler` | 249 | 0 (0) | 198,830 | 156 |
| mui-docs | mui/material-ui `67801955` | `docs` | 16,766 | 0 (0) | 1,564,419 | 13,399 (13,364 in every oracle run) |
| xstate | statelyai/xstate `da4c74ef` | `.` | 1,536 | 0 (0) | 387,778 | 253 |
| bluesky | bluesky-social/social-app `c1c090dd` | `.` | 6,090 | 263 (2) | 1,186,682 | 2,669 |

vscode, mui-docs and xstate are typescript-benchmarking's scenarios (vscode and
xstate unpinned there, frozen here; mui-docs at its pinned commit);
self-compiler is its `strada-compiler`, renamed from `self-compiler` in August;
bluesky is not in that repository and is frozen here. The installs follow
typescript-benchmarking: vscode and mui-docs without dependency scripts, xstate
and self-compiler with them, bluesky without them but with its root
`postinstall`, which generates the lexicons and locale catalogs its sources
import. vscode keeps the benchmark's 181 Electron diagnostics that its
`--ignore-scripts` install leaves.

The modes are `--noEmit` (full checking) and `--noEmit false --outDir <fresh>`
(checking plus emit), adjusted where a configuration would not otherwise check
cold or check at all, each recorded in the descriptor's notes:

- mui-docs (`incremental`) and self-compiler (`composite`) run with both off,
  so no build information makes a later run incremental or lands in the
  checkout.
- mui-docs' emit sets `rootDir` to the checkout: its `allowJs` program reaches
  workspace sources outside `docs/`, the pin requires an explicit `rootDir`
  (TS5011) and maps those files onto themselves (TS5055), and an options error
  makes the compiler emit without checking.
- xstate's and bluesky's emits add `rewriteRelativeImportExtensions`, without
  which `allowImportingTsExtensions` is an options error for the same reason.

A frozen descriptor whose emit mode checks less than half of its check mode's
types is rejected, and every run must leave the checkout as provisioned.

## The measurement contract

`tools/phase7/bench/capture.py run` measures each scenario, mode and checker
count in alternating serial pairs of fresh processes, after one discarded
warm-up pair: seven pairs, extended once to fourteen when the elapsed ratio's
95% bootstrap interval straddles 1.0 or either runtime's relative MAD exceeds
5% (the S07 rule, `scripts/s07_benchmark_stats.py`).

- **Elapsed** runs from process launch to exit. The binary's own `Total time`
  (configuration through emit) is kept as the operation time, and the rest as
  startup and exit, both raw in the run file.
- **Peak RSS** is the child's own peak from `wait4`.
- **Effective checkers** are `min(n, files, 256)`, from the run's file count.
- **Correctness first.** Every sample is compared with the frozen work before
  its time is kept: exit status, the printed diagnostics and file list
  (root-relative), the file count, the emitted files and their bytes, and
  evidence that checking ran (a checking time and at least half the oracle's
  type count). Preflight runs one sample of each runtime per configuration,
  compares both runtimes' resolved options with the oracle's, and plants an
  omitted operation (`--noCheck`, `--noEmit`) and a changed result, which must
  be rejected before any measurement.
- **Host.** Before each configuration the capture records the CPU of every
  other process (identified by PID, reported by name only). Other compiler and
  Python processes are included; only the capture process itself is excluded.
  The owner accepted these ten R0 records without a load qualification on
  2026-10-11. Their raw host observations remain recorded.

The pin's emit on mui-docs is not deterministic: twelve oracle runs emitted
13,372 to 13,399 of the same 13,399 files, different ones each time, all
workspace sources the docs project reaches through both `node_modules`
symlinks and their real paths. Rust emits all of them every run. The
descriptor declares it (`oracle_emit_varies`); Rust must emit the union
exactly, and the oracle may omit up to 1% of it but change no file.

## Correctness found by the scenarios

Two Rust checker differences, fixed before any measurement and reduced to
`crates/tsr_compiler/tests/phase7_scenario_regressions.rs` (both fail without
the fixes; the compiler suite stays at 13,432/13,432):

- vscode: `key in data` with a `unique symbol` key did not narrow a type whose
  only index signature is a string one (TS2538). The port of
  `isTypePresencePossible` looked up a string literal key for every name; the
  pin's `getApplicableIndexInfoForName` selects symbol index signatures for a
  late-bound name.
- bluesky: an import used only by the heritage clause of a `declare class` was
  kept. `checkIdentifier` marks aliases through `markLinkedReferences`, which
  ignores ambient references; the Rust identifier path called its port of
  `markIdentifierAliasReferenced` directly.

## Results

One capture of all five scenarios in both modes (`target/phase7/bench/capture-r0`,
revision `5468cf02`, 2026-10-10 18:54–19:55 UTC, the owner's macOS arm64 host
with 18 CPUs), recorded as the ten run files `status/perf/check-<scenario>/` and
`status/perf/emit-<scenario>/2026-10-10T19-55-37Z-5468cf02.json`. Every sample of
all 30 configurations matched the oracle's frozen work; three configurations
were extended to 14 pairs. Ratios are Rust over Go, with the medians in
parentheses (Rust / Go); the operation ratio is the binaries' own `Total time` at
four checkers, and "outside" is the rest of the elapsed time (startup and exit).

| Scenario | Mode | Elapsed, 2 checkers | 4 checkers | 8 checkers | Peak RSS, 2 | 4 | 8 | Operation at 4; outside |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| vscode | check | 2.82 (17.8 s / 6.3 s) | 2.46 (10.8 s / 4.4 s) | 1.97 (7.1 s / 3.6 s) | 1.02 (5.5 / 5.4 GiB) | 1.01 (6.1 / 6.0 GiB) | 1.02 (6.8 / 6.7 GiB) | 2.77; 0.62 s / 0.72 s |
| vscode | emit | 4.15 (33.9 s / 8.2 s) | 3.75 (21.8 s / 5.8 s) | 3.03 (14.5 s / 4.8 s) | 0.76 (5.6 / 7.3 GiB) | 0.76 (6.1 / 8.0 GiB) | 0.75 (6.9 / 9.2 GiB) | 4.30; 0.64 s / 0.89 s |
| self-compiler | check | 3.31 (1.5 s / 0.5 s) | 3.19 (1.3 s / 0.4 s) | 2.96 (1.2 s / 0.4 s) | 0.74 (0.3 / 0.4 GiB) | 0.79 (0.4 / 0.5 GiB) | 0.85 (0.5 / 0.6 GiB) | 3.56; 0.04 s / 0.05 s |
| self-compiler | emit | 3.41 (1.6 s / 0.5 s) | 3.18 (1.4 s / 0.4 s) | 3.03 (1.2 s / 0.4 s) | 0.73 (0.3 / 0.5 GiB) | 0.78 (0.4 / 0.5 GiB) | 0.82 (0.5 / 0.6 GiB) | 3.55; 0.04 s / 0.06 s |
| mui-docs | check | 1.89 (16.0 s / 8.5 s) | 1.56 (12.1 s / 7.8 s) | 1.28 (13.1 s / 10.3 s) | 0.80 (3.8 / 4.8 GiB) | 0.76 (5.4 / 7.1 GiB) | 0.71 (7.2 / 10.1 GiB) | 1.63; 0.44 s / 0.60 s |
| mui-docs | emit | 1.84 (18.4 s / 10.0 s) | 1.43 (13.4 s / 9.3 s) | 1.29 (13.2 s / 10.2 s) | 0.79 (3.9 / 4.9 GiB) | 0.76 (5.4 / 7.1 GiB) | 0.70 (7.2 / 10.2 GiB) | 1.48; 0.44 s / 0.59 s |
| xstate | check | 2.01 (1.5 s / 0.7 s) | 1.84 (0.9 s / 0.5 s) | 1.73 (0.7 s / 0.4 s) | 1.03 (0.6 / 0.6 GiB) | 1.00 (0.7 / 0.7 GiB) | 1.00 (0.9 / 0.9 GiB) | 2.04; 0.08 s / 0.09 s |
| xstate | emit | 2.36 (1.8 s / 0.8 s) | 2.11 (1.1 s / 0.5 s) | 1.78 (0.8 s / 0.5 s) | 0.96 (0.6 / 0.7 GiB) | 0.94 (0.7 / 0.8 GiB) | 0.92 (0.9 / 1.0 GiB) | 2.37; 0.08 s / 0.09 s |
| bluesky | check | 2.42 (5.5 s / 2.3 s) | 2.11 (3.8 s / 1.8 s) | 1.84 (3.1 s / 1.7 s) | 1.01 (1.7 / 1.6 GiB) | 1.05 (2.1 / 2.0 GiB) | 0.97 (2.8 / 2.9 GiB) | 2.30; 0.24 s / 0.25 s |
| bluesky | emit | 2.80 (7.2 s / 2.6 s) | 2.05 (4.7 s / 2.3 s) | 1.86 (3.9 s / 2.1 s) | 0.99 (1.7 / 1.7 GiB) | 0.95 (2.1 / 2.2 GiB) | 0.96 (2.8 / 2.9 GiB) | 2.22; 0.24 s / 0.28 s |

Against PLAN's budgets (elapsed at most 1.0, peak RSS at most 0.70) every
ratio misses: elapsed at 1.28 to 4.15, and peak RSS closest on mui-docs at
eight checkers (0.705 in emit, 0.712 in check). What R1 inherits:

- **The gap is in the compile, not around it.** Startup and exit take about
  the same time in both runtimes (0.6 s against 0.7 s on vscode); the operation
  ratio at four checkers is within 0.3 of the elapsed one.
- **Emit is the largest single cost.** On vscode, emit adds 16.1 s to Rust's
  run against 1.9 s to Go's at two checkers, roughly eight times as slow; the
  other scenarios' emit costs are proportionate to their check costs.
- **Rust gains more from checkers than Go.** The elapsed ratio falls with every
  doubling (vscode check 2.82, 2.46, 1.97), so the single-checker work is where
  most of the gap is. On mui-docs both runtimes are slower at eight checkers than
  at four, as the pin's own `checkerpool.go` notes for MUI docs.
- **Memory.** Rust's peak RSS equals Go's in check mode on vscode, xstate and
  bluesky and is 0.70–0.85 of it on self-compiler and mui-docs; Go's emit peaks
  are higher, so the emit ratios are lower.

**Host acceptance — owner decision, 2026-10-11.** All ten scenario records
are accepted as R0's starting measurements, with no load qualification. The
raw CPU/load observations remain intact, including `fseventsd` and macOS
storage scans; the old busy labels have been removed. No sample, ratio,
confidence interval or performance budget changed. The separate quiet-host
rule for future weekly acceptance runs remains in the Phase 7 plan.

## The API benchmarks

One capture (`target/phase7/api/capture-r0`, revision `4c9bf18b`, 2026-10-10
20:13–20:21 UTC, the same host), recorded as
`status/perf/api/2026-10-10T20-21-01Z-4c9bf18b.json`. The pin's
`test/sync/api.bench.ts` and `test/async/api.bench.ts` ran unchanged with the
pin's Node and tinybench (6.1.3), three rounds per runtime in alternating order,
the pinned client spawning each server through `getExePath()`. A task's ratio
compares the medians over the rounds of tinybench's per-run median latency.
That original capture checked matching task names and 10,160-identifier
counts only; those do not establish answer parity. The corrected harness now
compares the loaded roots, all three transferred trees and every answer from
the one-location, sequential and batched symbol tasks before timing. It
preserves names, flags and declaration handles, renumbering only process-local
IDs (including the type-ID suffix in unique-symbol escaped names; ADR 0010).
Wrong-answer and omitted-operation controls must fail. The memory pass runs each file once more per
runtime in its single-iteration mode, through a wrapper that records every
spawned server's own peak RSS (12 per file); the ratio compares the largest.
Owner decision 7's targets are 1.25 elapsed and 0.85 peak RSS. Ratios are Rust
over Go, with the medians in parentheses (Rust / Go):

| Task | Sync | Async |
| --- | ---: | ---: |
| spawn API | 0.63 (0.61 / 0.97 ms) | control |
| load snapshot | 1.17 (0.05 / 0.04 ms) | 1.16 (0.12 / 0.11 ms) |
| transfer debug.ts | 1.62 (0.51 / 0.32 ms) | 1.44 (1.39 / 0.97 ms) |
| transfer program.ts | 1.95 (1.83 / 0.94 ms) | 1.60 (4.69 / 2.93 ms) |
| transfer checker.ts | 1.66 (21.0 / 12.7 ms) | 1.53 (54.1 / 35.4 ms) |
| getSymbolAtPosition, one location | 1.07 (0.01 / 0.01 ms) | 1.12 (0.08 / 0.07 ms) |
| getSymbolAtPosition, 10,160 identifiers | 1.21 (183 / 152 ms) | 1.18 (905 / 765 ms) |
| the same, batched | 1.60 (69.0 / 43.1 ms) | 1.69 (70.4 / 41.6 ms) |
| getSymbolAtLocation, 10,160 identifiers | 1.19 (124 / 104 ms) | 1.15 (866 / 754 ms) |
| the same, batched | 2.03 (34.5 / 16.9 ms) | 2.10 (36.7 / 17.4 ms) |
| Peak RSS, largest server | 0.61 (172 / 279 MiB) | 0.67 (193 / 287 MiB) |

Ten of the 21 ratios miss: every transfer and every batched query. What R2
inherits:

- **The query gap is per identifier.** Rust's extra time on the sync queries
  is about the same with and without batching (getSymbolAtPosition 31.5 ms one
  at a time, 25.9 ms batched; getSymbolAtLocation 20.2 and 17.5 ms), 1.7–2.6 µs
  per identifier; one request per identifier adds round trips that dilute the
  ratio. The async client adds about 10 µs per request on top (140 ms over the
  10,160 requests of getSymbolAtPosition), which the per-request ratios still
  absorb.
- **Transfers** take 1.4–2.0 times Go's time in both clients.
- **Memory.** The largest server, the one that loads the project, peaks at
  0.61–0.67 of Go's.
- **Spawn and snapshot.** The clients initialize on their first request: the
  sync `spawn API` measures the process launch alone, and the async one only
  constructs the client, so it is a control. `load snapshot` repeats
  `updateSnapshot` on an open, unchanged project (the first load falls in a
  warm-up iteration).

The client-only controls do the same work in both runtimes' runs and agree
within 7% (`materialize checker.ts` 16.1 against 15.6 ms, `TS - load project`
221 against 208 ms in the sync file and 206 against 208 in the async one), which
bounds this capture's noise; `test/nodelist.bench.ts`'s tasks take 0.07–0.10
ms. In this original capture, the sync spawn task closed only its last
server: about 700 Go or 1,300 Rust servers survived into later tasks per round.
These API timings need replacement after the harness fix; the owner's load
acceptance above applies to the scenario records, not this lifecycle defect.

The corrected runner calls the pin's teardown after every spawn iteration and
awaits child exit between iterations and phases. Both clients use `Bench.run()`
so cleanup can await exit; tinybench 6.1.3 still measures sync task bodies through
its same synchronous timer. The runner records spawned/reaped/peak child counts
and rejects overlapping or surviving servers. The task bodies, iteration
options and measurement boundaries are unchanged; process cleanup is untimed.

## Phase 5 L8

Owner decision 9 runs L8 alongside R0. Its audit compares each of the 93 `WORK`
rows of [docs/PHASE5-tests.md](PHASE5-tests.md) with the Rust tests assertion by
assertion; a row leaves `WORK` through a reviewed `DIRECT_AUDIT` entry in
`tools/phase5/harness/inventory.py` or a ported test that names it. The audit so
far:

- **Exact already.** The three stack-sanitizer tests (the Rust test runs the
  three pinned baselines they produce); several watcher, content-mapper and
  progress observations.
- **Partial.** Most watcher rows (an overflow-tagged event delivered, an empty
  registration, mixed recursion flags in one registration, exactly one
  ancestor watch, the target dropped after termination), the content-mapper
  parse tests (full messages, package directory), progress (two shutdown
  subtests missing).
- **Unported.** 18 of the 23 `lsproto` JSON tests have no case in the codec
  matrix (56 subtests; the matrix's expectations are generated from Go, so they
  are added there); most of `lsutil`'s preference parsing (over 50 subtests);
  the real-backend watcher cases; `TestTryRemoveDirectoryPrefixCaseFoldingShrinksPrefix`;
  the `lsconv` JS-reference comparison; and the `project` rows, about 220
  subtests across the session, snapshot file system, project, project
  references, collection builder, caches and lifetime, whose comparison with
  `tsr_project`'s tests continues.
- **Owner decision needed.** `project/dirty`'s `TestSyncMapProxyFor`: the ledger
  maps `project/dirty` at 0%, and the Rust project system tracks changes
  without its sync map, so the row needs a port or an out-of-scope entry with
  that reason. `project/logging`'s `TestLogTree` has an empty body.

This is substantial work, so, as the plan provides, it continues in its own
pull request alongside R1–R5 and is required before R6; Phase 5's status line
changes only when it is resolved.

## Reproduction

```sh
python3 scripts/phase7_scenarios.py provision          # about ten minutes, network and Docker
python3 tools/phase7/bench/capture.py build            # tsgo from the pin, tsrust from HEAD
python3 scripts/phase7_scenarios.py verify             # the frozen work still holds
python3 tools/phase7/bench/capture.py run --output target/phase7/bench/<capture>
python3 scripts/perf.py record check-vscode --capture target/phase7/bench/<capture> --label "…"   # one per workload
(cd upstream && npm ci --ignore-scripts)               # the API benchmarks' client dependencies
python3 tools/phase7/api/capture.py run --output target/phase7/api/<capture>
python3 scripts/perf.py record api --capture target/phase7/api/<capture> --label "…"
```
