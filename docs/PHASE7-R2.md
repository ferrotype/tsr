# Phase 7 R2: editor and API latency

Status: in progress, stacked on R1 at `3d90a682`. This checkpoint keeps the
budgets in [the Phase 7 plan](PHASE7-plan.md#r2--editor-and-api-latency).
Candidate/control screens below are concurrent development measurements;
they do not replace the serial Go/Rust acceptance records.

## Starting point and attribution

The control is R1's saved ordinary release executable, SHA-256
`59e8dd14848a880d8a0b1b55409fab947d9668abd3c80272f9f504cb405dbaf6`.
The pinned Go executable is
`57ab2bcc2c59862d56fe53a3c87240f22bcd692316e7d261a012f736ae5f4d87`.
The TypeScript fixture and `typescript-pull.json` request schedule are the
existing Phase 5 latency workload. Three fresh alternating Go/Rust smoke
pairs match all responses and traffic. Their median ratios are 1.28 for
first diagnostics, 1.99 for completion, 2.59 for hover, 1.61 for references
and 1.82 for rename. Three pairs are not an acceptance capture, particularly
for the submillisecond hover and rename clocks.

`tools/phase7/lsp/profile.py` repeats one operation after its first response,
using the same client and checking every repeated answer. Fresh completion
profiles execute 1,000 repeats against each server; their normalized
responses and traffic agree. Rust's profile contains both language-service
work and JSON response work: `client::raw`, raw-value validation and token
encoding. The 3,000-repeat hover profile largely exposes startup and waiting;
it does not attribute the cold hover clock. These sampled runs are diagnostic,
not candidate timings, and their overlapping inclusive weights are not added.

The existing historical API record shows batched symbol queries among the
larger server-dependent gaps. The first bounded candidate targets repeated
node-handle table resolution and symbol declaration-handle construction.
It leaves registration, generation gates and callback scheduling intact.

## Candidate contract

- A batched location request remembers only its most recently used immutable
  node-index table. Every handle still gets parsed and its slot checked;
  changing files resolves the next table normally. The cache borrows the
  request and program and dies at the end of that request.
- A symbol's value declaration reuses its already encoded declaration
  handle when that node occurs in the declaration list. Otherwise it uses
  the original encoding path.
- Navigation reuses a child node read for flags, range and kind during a
  binary-search probe. Its private traversal builds the final visit vector
  directly instead of first building a second vector of hook results.
  The public hook order, nullable slots, reparsed-node filtering and search
  probe order remain unchanged.

No persistent per-node cache or new synchronization is introduced. New API
tests compare mixed-file and duplicate batches with individual queries and
exercise invalid handles after cache hits. Navigation tests cover token
boundaries and eager/deferred JSDoc identity.

## Measurements and retention

**Retained as one combined candidate.** The ordinary release candidate is
SHA-256 `5d4807c25c768184aa5d442b30eb41bc3fac2bc7310fd4c8fcf668eed231c9b8`.
All builds, tests and sampling finished before these concurrent screens.
Each API pair starts fresh clients, warms the checker as the pin does, and
executes ten batches of the 10,160 identifiers in `program.ts`. Every ordered
symbol observation agrees; each client spawns and reaps exactly one server.
The fixed ten repetitions omit tinybench's per-task warmup; only checker
setup is warmed, so these are development screens rather than acceptance.
The maximum release skew in the final API batch is 0.034 ms.

| API task | Client | Pairs | Median control | Median candidate | Median paired ratio |
| --- | --- | ---: | ---: | ---: | ---: |
| Batched symbol at location | sync | 5 | 35.086 ms | 32.144 ms | 0.9250 |
| Batched symbol at location | async | 5 | 35.852 ms | 32.742 ms | 0.9059 |
| Batched symbol at position | sync | 5 | 66.902 ms | 64.939 ms | 0.9707 |
| Batched symbol at position | async | 5 | 67.837 ms | 65.675 ms | 0.9735 |

The ratio is the median of the five pair ratios, not the ratio of the two
median clocks. All twenty pairs favour the candidate. An earlier screen
also favoured it in every pair (location medians 0.9053/0.9131 and position
0.9650/0.9678); the final batch above uses the committed driver's exact
contents after removing an unused diagnostic mode. An identical-binary
API check gives 1.0029 over five pairs, ranging from 0.9925 to 1.0165.
Captures are `target/phase7/r2/api-{sync,async}-{location,position}-final`,
with the earlier `-screen` runs and `api-sync-location-aa` retained too.

The process-isolated LSP screen covers twenty concurrent pairs, all with
matching responses and traffic. It establishes no LSP speedup:

| LSP clock | Identical-binary median ratio | Candidate/control median ratio |
| --- | ---: | ---: |
| First diagnostics | 0.9998 | 1.0005 |
| Completion | 1.0096 | 1.0185 |
| Hover | 0.9929 | 0.9867 |
| References | 1.0088 | 1.0062 |
| Rename | 0.9768 | 0.9969 |

These differences are within the small runner effects seen with identical
binaries; the submillisecond hover and rename samples are particularly noisy.
Raw measurements are in `target/phase7/r2/lsp-process-{aa,screen}`. The
navigation change remains part of the measured API improvement; no separate
speedup is attributed to either production component. There is no new retained
cache across requests, but this slice does not claim a measured RSS saving.

Three additional concurrent Go/candidate pairs per API task match every
symbol observation. They give Rust/Go ratios of 1.601/1.605 for location
batches (sync/async), and 1.534/1.538 for position batches. These development
screens, `api-{sync,async}-{location,position}-go-screen`, still exceed the
1.25 target. They are not the complete pinned tinybench acceptance run and
are not combined arithmetically with historical ratios.

Validation: 89 API and 8 navigation unit tests; all 542 observations from the
pin's sync/async API test files; five focused fourslash variants (six
observations) and two LSP variants. The focused navigation cases cover JSDoc,
trivia, token boundaries and completion with concurrent file closure. All
pass, with no parity expectation changes. Formatting, changed-crate clippy
with warnings denied, ledger validation, the paired-runner/profile tests,
the existing latency tests and S07 operation tests pass. CI runs the full
client suites.

## Reproducing development screens

Build and save control and candidate release executables separately, using
the same Cargo flags and the reported `compiler-artifact.executable` path.
Finish builds, tests and profiling before timing. These scripts never build
or replace the supplied executables.

```sh
BENCHMARK_RUNS=../ts-rust-benchmarks
BENCHMARK_ARCHIVE="$BENCHMARK_RUNS/2026-10-11/target"
GOGC=100 python3 tools/phase7/lsp/pairs.py \
  --control "$BENCHMARK_ARCHIVE"/phase7/r1/bin/candidate \
  --candidate "$BENCHMARK_ARCHIVE"/phase7/r2/bin/candidate \
  --fixture "$BENCHMARK_ARCHIVE"/phase5/latency/typescript-proposal \
  --scenario tools/phase5/latency/proposals/typescript-pull.json \
  --pairs 20 --output "$BENCHMARK_RUNS"/phase7/r2/NEW_LSP_SCREEN
GOGC=100 python3 tools/phase7/api/pairs.py \
  --control "$BENCHMARK_ARCHIVE"/phase7/r1/bin/candidate \
  --candidate "$BENCHMARK_ARCHIVE"/phase7/r2/bin/candidate \
  --mode sync --task location --pairs 5 --iterations 10 \
  --output "$BENCHMARK_RUNS"/phase7/r2/NEW_API_SCREEN
```

Repeat the API command with `--mode async` and `--task position` to cover both
batch entry points and client modes. The client uses the pinned Node version
and installed upstream dependencies. Each server is selected by an explicit
`tsserverPath`; concurrent clients never race over the pin's shared executable
symlink. Both clients finish setup before their timed batches start. Hashing
answers and teardown wait until both timed batches finish. The observations
reuse the existing symbol canonicalization and compare every ordered result.
The LSP screen gives each process its own fixture copy and uses the existing
Phase 5 schedule, clocks and exact response/traffic comparator. Pair ratios
are computed before taking their median; mismatched work cannot yield a
valid result. Raw runs and executable identities are retained under the
chosen output directory.

Each LSP client must also have its own Python interpreter. An initial
threaded runner shared a GIL across both clients' JSON readers. A six-pair
identical-binary check reported a 0.83 completion ratio and rename ratios
from 0.10 to 11.0. Those timings, including the earlier candidate screen,
are rejected as measurement artifacts and retained under
`target/phase7/r2/lsp-threaded-aa` and `lsp-screen`. The committed runner
uses spawned processes and defers normalization and artifact writes until
both peers retire. The serial acceptance schedule and comparison rules do
not change.

Profile separately, for example:

```sh
BENCHMARK_RUNS=../ts-rust-benchmarks
BENCHMARK_ARCHIVE="$BENCHMARK_RUNS/2026-10-11/target"
python3 tools/phase7/lsp/profile.py \
  --binary "$BENCHMARK_ARCHIVE"/phase7/r1/bin/candidate \
  --fixture "$BENCHMARK_ARCHIVE"/phase5/latency/typescript-proposal \
  --scenario tools/phase5/latency/proposals/typescript-pull.json \
  --metric completion --repeats 1000 \
  --output "$BENCHMARK_RUNS"/phase7/r2/NEW_PROFILE
```

Open the saved `cpu.json.gz` with `samply load`. The profile helper and paired
runners are committed. Historical `target/` paths in this record now have
verified copies under `../ts-rust-benchmarks/2026-10-11/target/`. The commands
above use the archived binaries and fixture and write new runs outside the
build directory. Captures and saved comparison binaries must survive `cargo clean`.

## Remaining checkpoint work

Completion serialization, project load and API transfer remain separate
candidates; no savings from different screens are added. The next bounded
serialization experiment is to stop allocating number-token text when the
existing JSON validator discards it. Borrowing the input during validation
is another candidate, without changing public decoder ownership. Per-task
API profiling and the owner's project-load workloads remain outstanding.
Replacing raw JSON validation is not justified by an encoder-only
microbenchmark: public raw replies must still reject bad JSON, enforce
enclosing depth, and preserve panic recovery.

R2 closes only with the declared LSP and per-task API acceptance records,
the client suites green, and the approved budgets met (or changed by an owner
ADR). R1's full-project work remains independent. No threshold or historical
performance record changes in this slice.
