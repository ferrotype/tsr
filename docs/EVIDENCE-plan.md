# Evidence: replacing the recorded-evidence machine with expectation files

Owner decisions of 2026-10-03, recorded in section 10. This plan replaces
ADR 0018's model, `docs/TRACKING.md` (producers, recorded artifacts, sprints,
experiments, derived verification) with one that CI computes on every pull
request. It is executed in the PR sequence of section 9; nothing in it waits
for another green-up.

## 1. Why

The machine, measured on main at `0b515acb`:

| | |
|---|---|
| Producers in `status/runs.toml` | 29; each mostly *replays* a capture a person made in `target/` |
| Distinct `run.*.*` metrics gated on | 187 (P4B's exit alone lists 34) |
| Evidence state rendered in `STATUS.md` | 26 stale, 2 never recorded, 1 current; 0 of 8 experiments; 0 files verified |
| State of the product | 13,426/13,432 checker, 13,426/13,432 emit, 512/516 tsc |
| CI `producers` job per push | 4 h on Linux, 5 h 39 min on macOS, failing |
| Committed evidence | 495 artifacts, 212 MB, one embeds 4.3 MB of stderr; `selftest` has 46 recorded attempts |
| Committed generated data | 391 MB; `.git` is 1.2 GB |
| Tooling | 55k lines of Python plus 28k lines / 1,823 tests of the tooling, plus ~4k lines of xtask evidence code, against 564k lines of Rust |
| Ledger | 366 of 542 files have an empty `verify`; Phase 1, closed, shows 83 planned / 62 in-progress / 8 ported |
| Approval formats | 8 files |

The causes are structural, not fixable by one more producer:

1. Staleness is tree-wide. Producers fingerprint `crates/**`, so "stale" means
   "a commit happened". The dashboard is red on any moving tree by
   construction and green only for the instant after the owner spends a day
   re-running everything on a quiet host.
2. Evidence is recorded, not computed. Numbers are captured by a person into
   committed JSON, then "replayed" by producers that check committed
   snapshots (`first-comparison`, `blockers`, `audit`, `claims`, `receipts`,
   `residuals`, `dispositions`, `inventory`, `native-provenance`) equal a
   regeneration. The truth lives in `target/`, outside the repository; CI can
   verify bookkeeping but never recompute a result. The one fact that matters,
   the 13,432-variant corpus, is not in CI at all.
3. Everything defends against tampering: hashes of hashes,
   `--check-committed`, "the harness cannot override", "writing `verified` is
   an error". CI computing the number is cheaper and stronger.
4. Four parallel implementations of "run Rust over a Go suite, compare,
   account for differences" (`phase1_*` to `phase4_*`), each with its own
   freezes, registers and producer. Phase 5 was planned as the fifth.
5. Sprints encode plan TODOs as metrics (`x1_complete`, `c7_complete`) that
   exist only so a gate can pass.

## 2. The model

Five facts are worth knowing about this port; nothing else outlives the phase
that produced it:

| Fact | Where it lives | Who computes it |
|---|---|---|
| Suite parity: the Rust passes the pinned suites the Go passes | `status/parity/<suite>.json`, one per suite | CI, on every pull request |
| Performance against Go on fixed workloads | `status/perf/<workload>/<run>.json` | a dispatch-only workflow, or the owner's host |
| Build quality: fmt, clippy, deny, tests, MSRV, Miri/ASan on the unsafe modules | CI job results | CI, on every pull request |
| Port coverage: mapped functions and ported tests, from markers | rendered from `PORTS.toml`, `data/go-functions.tsv` and the markers | `cargo xtask status`, locally and in CI |
| Approved divergences | entries of the parity files marked `approved` | the owner, by merging the PR that marks them |

**Expectation files.** A parity file names the suite, the pin, the suite's
size and every test that does not pass, each with a reason. CI runs the suite
and requires the observed failing set to equal the committed one: a new
failure fails CI, and a test that starts passing fails CI until the entry is
removed. The file is therefore exact for `HEAD` at all times; "stale" stops
being a concept. Progress is the diff of the file in the PR that made it. This
is how Servo and Deno track the web-platform tests.

**No Go at CI time for baselined suites.** The pin commits what its runner
compares against: 23,297 reference files under `testdata/baselines/reference/compiler`,
22,150 under `conformance`, and the `tsc`, `tsbuild`, `transpile`, `api`,
`fourslash` and `lsp` references. A Rust port of the pin's test runner
reads the test cases, composes the baselines and compares them with those
files, exactly as `go test ./internal/testrunner` does. Nothing is captured
from Go. The two runner assertions that are not baselines (union ordering,
parent pointers) are assertions in the ported runner, not observations to
replay.

**One tool.** `scripts/parity.py` drives every suite through one process
model and one expectation-file format. Suite-specific work is in the Rust
runner binaries it spawns.

## 3. File formats

### `status/parity/<suite>.json`

The entries below are illustrative; `accept` writes the real ones.

```json
{
  "suite": "compiler",
  "pin": "1f70213d4922b434345f639b441681e470c7cfc1",
  "total": 60000,
  "failing": {
    "compiler/intersectionConstructorReductionCrash.ts/types": {
      "reason": "deadline: 45 s alone, 60 s under 16 jobs"
    },
    "conformance/moduleResolution/bundler/outputPaths(module=esnext).ts/errors": {
      "reason": "extra TS6059 for a file outside rootDir",
      "approved": "owner 2026-10-03, pin reports it only for composite projects"
    }
  }
}
```

- `total` is the number of test ids the runner enumerates for the suite. A
  change in the denominator fails the check; the runner, a port of the pin's
  enumeration and skip list, decides it.
- A test id is `<suite>/<configured name>/<subtest>`, the pin's own subtest
  path: `compiler/foo(target=es5).ts/types`. One id per baseline the pin's
  runner checks, so emit progress and checker progress are visible
  separately and an id maps directly to a reference file.
- `failing` is sorted by id. `reason` is required. `approved` is optional and
  carries the owner's words; an approved entry is still a failing entry (the
  Rust output differs from the pin), rendered separately. Approval is the
  merge of the PR that adds the field. This replaces `data/divergences.toml`,
  both `approved-differences.json`, `dispositions.json`,
  `informational.json`, `services-approvals.json`,
  `baseline-exceptions.json` and `subset-review.json`.
- The tool writes the file (`accept`); nobody edits it by hand except to add
  `reason` or `approved` text.

### `status/perf/<workload>/<run>.json`

One file per measurement run, written by the benchmark harness and committed
by the owner or by the dispatch workflow's artifact, never regenerated:

```json
{
  "workload": "parse-bind-vscode",
  "pin": "1f70213d…",
  "revision": "0b515acb…",
  "host": {"os": "macos", "arch": "aarch64", "cpus": 18, "label": "owner quiet host"},
  "recorded_at": "2026-10-03T09:12:00Z",
  "ratios": {"one_thread_wall_time": 1.153, "eight_threads_wall_time": 1.399, "peak_rss": 0.81},
  "samples": {"rust": {"one_thread_wall_time": [ … ]}, "go": {"one_thread_wall_time": [ … ]}}
}
```

Thresholds, if the owner wants any enforced, live in one
`status/perf/thresholds.toml` read by the dispatch workflow; they are not a
pull-request gate.

## 4. The tool

### `scripts/parity.py`

```
parity.py list   <suite>                                  # every test id, one per line
parity.py run    <suite> --output DIR [--shard i/n] [--jobs J] [--timeout S] [--id ID …]
parity.py check  <suite> --results DIR [DIR …]            # merge shards, compare with status/parity/<suite>.json
parity.py accept <suite> --results DIR [DIR …]            # rewrite the expectation file from the results, keeping reasons
```

- `run` spawns the suite's runner binary once per test (the process model of
  today's corpus runners: a panic, a stack overflow or a deadline is one
  failed test, never a lost run) and writes one result line per id:
  `{"id", "state": "pass"|"fail", "reason"?, "detail"?}` plus the runner's
  `local/` output for failures, which is the diff a developer reads.
- `check` exits non-zero and prints the two sets that differ: ids failing
  now that the file does not name, and ids the file names that pass now. It
  also prints `passing / approved / failing / total` and writes the same to
  `$GITHUB_STEP_SUMMARY` when set.
- `accept` keeps every existing `reason` and `approved` text for ids that
  still fail, adds `reason: "unexplained; see local/<id>"` for new ones, and
  drops ids that pass.
- Suites are a small table in the script: runner binary, enumeration
  command, default timeout. No per-suite scripts.

### `tsr_testrunner`

A new crate, the port of the pin's `internal/testrunner`,
`testutil/harnessutil`, `testutil/tsbaseline` and `testutil/baseline`
(about 4,500 lines of Go, carried as `// port:` markers like any other
file; the ledger already names the crate, phase 1, kind `harness`). It
absorbs the Rust baseline writers that exist today in `tools/s08/p5/baseline`
(errors, types, symbols) and `tools/phase3/harness` (JavaScript output,
source maps, source-map record, declaration recompilation, reprint,
transpile) and the `phase2_checker`/`phase3_emit` example adapters, so one
binary runs what the pin's `CompilerBaselineRunner` and
`TranspileBaselineRunner` run:

```
tsr-testrunner list    --suite compiler|conformance|transpile
tsr-testrunner run     --suite … --id ID --baselines-local DIR [--mode single|concurrent]
```

`run` compiles the case as `harnessutil.CompileFiles` does, runs the nine
`verify*` sub-tests of `compiler_runner.go:runSingleConfigTest`, compares
each composed baseline with `upstream/tsc/testdata/baselines/reference`
byte for byte (`baseline.Run`), writes differing outputs under the local
directory, and prints one result line per sub-test. The `--mode` flag is the
pin's `TS_TEST_PROGRAM_SINGLE_THREADED`; the pin's default is `single`.

The existing per-phase Rust harness code becomes this crate's modules; the
Python layers above it (`phase2_corpus`, `phase3_corpus`, their `compare`,
`inventory`, `native*` and `producers`) go.

### `tools/phase4/tsctests`

Already a runner that replays the recorded scenarios against committed
references; it gains `list` and per-scenario `run` with the result-line
contract and becomes the `tsc` suite's binary. The recorded inventory
`data/phase4/scenarios.json.gz` stays: the pin's `tsc` tests are Go code,
and the recording is their data form, a function of the pin alone.

## 5. Suites

| Suite | Runner | Ids | On PRs |
|---|---|---|---|
| `compiler` | `tsr-testrunner`, mode `single` | compiler and conformance cases × configurations × sub-tests (≈13,432 variants) | yes, sharded |
| `compiler-concurrent` | `tsr-testrunner`, mode `concurrent` | the same | yes, sharded |
| `transpile` | `tsr-testrunner --suite transpile` | the transpile cases × 28 configurations | yes |
| `tsc` | `tools/phase4/tsctests` | the 516 recorded scenarios (the pin's `tsc`, `tsbuild`, `tsbuildWatch` tests) | yes |
| `fourslash`, `lsp` | Phase 5's runners | the pin's fourslash and lsp tests | Phase 5 adds them |
| `api` | later | the encoder baselines | when wanted |

Unit tests ported from the pin's `_test.go` files run under `cargo test`;
their count comes from the `// source:` markers and is a coverage number,
not a suite.

## 6. CI

`.github/workflows/ci.yml` replaces `status.yml`:

| Job | Runner | Does |
|---|---|---|
| `quality` | `ubuntu-latest` | `cargo fmt --check`, the clippy gate, `cargo deny`, `cargo test --workspace`, `cargo xtask validate` (markers, ledger provenance) |
| `msrv` | ubuntu, macOS | `cargo check` on the declared minimum Rust |
| `parity` | matrix: suite × shard, `ubuntu-latest` | build the runner in release, `parity.py run … --shard i/n`, upload the results directory |
| `parity-check` | `ubuntu-latest` | download all shards of each suite, `parity.py check`, job summary |
| `native` | macOS and ubuntu | `cargo test` for `tsr_fswatch`, `tsr`; the `tsc` suite on both hosts |
| `pages` (main only) | `ubuntu-latest` | `cargo xtask status`, publish the render to GitHub Pages |

Runner choice: GitHub-hosted runners for now (the repository is public:
Linux 4 vCPU / 16 GB, macOS 3 vCPU arm64). A full Rust run of the corpus
takes about 180 s per mode on 18 CPUs ([PHASE3-T8.md](PHASE3-T8.md)); four
shards keep each parity job near 10 minutes. `runs-on` is a workflow-level
variable so a self-hosted runner can be swapped in without editing jobs.
Build caching uses `Swatinem/rust-cache`.

`.github/workflows/perf.yml`, dispatch only, takes `runs-on` and `workload`
inputs, runs the S07/S08 benchmark harnesses, writes a `status/perf` file,
uploads it as an artifact and prints the ratios in the job summary. The
first dispatch answers whether GitHub's runners can host it at all: the
8-thread E6 measurement cannot run on a 4 vCPU runner, so the expectation
is that only the single-thread workloads run there and the rest stay on the
owner's host or a self-hosted runner.

## 7. Status

Nothing rendered is committed. `cargo xtask status` reads `status/parity/*.json`,
`status/perf/**/*.json`, `PORTS.toml`, `data/go-functions.tsv` and the
markers, and writes `target/status/index.html` and `target/status/STATUS.md`:
per-suite passing / approved / failing, the failing ids with reasons, the
perf runs per workload with host labels, mapped functions by package, ported
tests by package. The `pages` job publishes it; `README.md` links to it.
`STATUS.md`, `status/status.json`, `docs/status.html`, `status/history.jsonl`
and `status/unmapped-functions.json` leave the repository.

Rationale (owner left this to me): a committed render needs a check that it
matches its inputs, which is the `--check-committed` machinery again; the
parity files themselves are small, diffable and already carry the counts a
reader wants.

## 8. What stays and what goes

Stays:

- `PORTS.toml` without the `verify` field and the `verified` derivation;
  `status` keeps `planned`, `in-progress`, `ported`, `out-of-scope`.
  `data/go-functions.tsv`, `data/upstream.json`, `scripts/ledger-init.py`,
  the `// port:` and `// source:` markers, `cargo xtask validate`.
- `cargo xtask gen` and `xtask/src/gen/**` (code generation, unrelated to
  evidence).
- The runners: `tsr_testrunner` (new, absorbing the harness code listed in
  section 4), `tools/phase4/tsctests`, `tools/phase4/recorder` and
  `data/phase4/scenarios.json.gz`, the S07/S08 benchmark harnesses and their
  workload manifests (`data/workloads.toml`, `data/s08/checker-workload.json`,
  `data/s07/vscode-*.json`).
- `scripts/s09_format.py compare` and its oracle, as the developer tool
  CLAUDE.md documents, until Phase 5's fourslash format tests cover it.
- ADRs. ADR 0018 (tracking) is superseded by a new ADR recording this plan;
  ADR 0004 (owner approves divergences) is amended to name the `approved`
  field.
- The phase plan and record documents under `docs/`, as history.

Goes (the rule for anything not listed: a file under `scripts/`, `tools/` or
`data/` survives only if a remaining command reads it; `git grep` decides):

- `status/evidence/**`, `status/runs.toml`, `status/experiments.toml`,
  `status/history.jsonl`, `status/status.json`,
  `status/unmapped-functions.json`, `STATUS.md`, `docs/status.html`.
- `sprints/**`. The checklists live in the phase plans; a checkpoint closes
  when its PR shrinks a parity file and the owner merges it.
- `xtask/src/evidence.rs`, `evidence_tests.rs`, and in `main.rs` the `run`,
  `check`, `check-metrics`, `status --record`, `status --check-committed`
  commands, the sprint, experiment, history and dashboard-chart code.
- `scripts/phase{1,2,3,4}_producers.py`, `scripts/s07_producers.py`, the
  producer subcommands of `s04.py`, `s05.py`, `s06.py`, `s08_e2.py`,
  `s08_checkerbench.py`, `s08_relater.py`, `s10.py`, `s11.py`,
  `scripts/tracking-bootstrap.py`, `scripts/checks.py`.
- `scripts/phase{2,3,4}_{compare,blockers,audit}.py`, `phase2_claims.py`,
  `phase2_dispositions.py`, `phase2_residuals.py`, `phase2_informational.py`,
  `phase2_divergences.py`, `phase2_report.py`, `phase2_inventory.py`,
  `phase2_native*.py`, `phase2_corpus.py`, `phase3_{corpus,inventory,native,receipts,residuals,report}.py`,
  `phase4_{acceptance,corpus,native}.py`, `phase4_unit_tests.py` (its
  roster count moves into `xtask status`), and their tests under
  `scripts/tests/`.
- `data/phase2/**` except nothing (every file there is a frozen snapshot or
  a register), `data/phase3/**` except `printer/` if the reprint witness
  reads it, `data/phase4/{first-comparison,blockers,x-audit,approved-differences,unit-tests}.json`,
  `data/phase1/**` minus the locale tables the generator reads,
  `data/divergences.toml` after its entries are carried into the parity files.
- `.github/workflows/status.yml`.
- `docs/TRACKING.md` (replaced by this document and the new ADR), the
  `## Producers` and tracking sections of `sprints/README.md` with the
  directory.

Phase 0 and Phase 1 (owner decision 4, my call on the detail): both closed
by the owner's own PRs and ADR 0020; the machine never agreed because its
metrics were stale or unknown. Their oracle comparisons (scanner, parser and
encoder bytes, binder graphs, program loading, config families, test host,
relater, E2) are retired as gates. Everything they tested is exercised end
to end by the `compiler`, `tsc` and later `fourslash`/`lsp` suites and by the
ported unit tests; the `api` suite covers the encoder when added. Their
scripts and oracles are deleted under the `git grep` rule, in their own PR,
so the deletion is reviewable on its own.

## 9. Migration, one PR each

1. **`compiler` suite in CI.** `tsr_testrunner`; `scripts/parity.py`;
   `status/parity/compiler.json`, `compiler-concurrent.json` and
   `transpile.json` accepted from a full run; `ci.yml` with `quality`,
   `msrv`, `parity` and `parity-check`; `status.yml` deleted. CLAUDE.md
   gains the two commands a developer needs (`parity.py run --id` and
   `check`). Nothing else is deleted yet, so the PR is reviewable as an
   addition.
2. **`tsc` suite.** `tools/phase4/tsctests` gains the result-line contract;
   `status/parity/tsc.json`; the `native` job. Delete the Phase 4
   producer, compare, blockers, audit, acceptance and corpus scripts and
   the Phase 4 registers; carry `approved-differences.json` into the parity
   file.
3. **Ledger, status and the old machine.** Drop `verify` from
   `PORTS.toml`; rewrite `cargo xtask status` as the renderer of section 7;
   `pages` job; delete `status/evidence`, `runs.toml`, `experiments.toml`,
   `history.jsonl`, the committed views, `sprints/`, the xtask evidence
   code, `TRACKING.md`; new ADR and ADR 0004 amendment. Delete the Phase 2
   and Phase 3 Python layers and data (their Rust harness code moved in
   step 1).
4. **Perf.** `perf.yml` with the `runs-on` input; the harnesses write the
   `status/perf` format; first dispatch on `ubuntu-latest` to learn what
   fits; the owner's recent E5/E6/checkerbench numbers carried over as the
   first committed runs with their host labels.
5. **Phase 0 and Phase 1 retirement.** Delete their producer entry points,
   oracle harnesses and frozen data under the `git grep` rule; keep
   `s09_format.py compare`.
6. **Phase 5 plan amendment** ([PR #82](https://github.com/ferrotype/tsr/pull/82)):
   section 5 names `fourslash` and `lsp` parity files instead of the `lsp`
   producer and its metrics; L-checkpoints close by PR.

Steps 1 and 2 are the work; 3 to 5 are deletions guided by the rule.

## 10. Decisions

| # | Question | Decision |
|---|---|---|
| 1 | Expectation files with CI computing parity on every PR | yes (owner) |
| 2 | Where the suite runs | GitHub-hosted runners now; `runs-on` kept swappable for a self-hosted runner (owner) |
| 3 | Committed render or published only | published only, nothing rendered is committed (mine, section 7) |
| 4 | Phase 0 and Phase 1 | already closed by the owner's PRs and ADR 0020; retire their oracle gates, delete under the rule (mine, section 8) |
| 5 | Perf cadence | dispatch only; the first dispatch establishes what GitHub's runners can host, the rest runs on the owner's host or a self-hosted runner (owner) |

## 11. Effect on Phase 5

[PHASE5-plan.md](PHASE5-plan.md) section 5 (acceptance) is written against
the old model: an `lsp` producer, `run.lsp.*` metrics, a P5B exit. Under
this plan the Phase 5 acceptance is two parity files, `fourslash.json` and
`lsp.json`, produced by the carried harness patch driving
`tsrust --lsp --test-host`; the 99.5 % gate is "at most 22 entries, each
with a reason"; checkpoints L0 to L8 close by merged PRs. Step 6 records
that amendment. Phase 5 L0 starts on this plan's model, not the old one.
