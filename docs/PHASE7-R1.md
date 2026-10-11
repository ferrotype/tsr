# Phase 7 R1: full-project performance

Status: in progress, starting from main `2009f6ec` after R0. The owner removed
model-based checkpoint assignments on 2026-10-11; the order and budgets remain
those in [the Phase 7 plan](PHASE7-plan.md#r1--full-checking).

## First attribution: VS Code, two checkers

Fresh profiles use the ordinary R0 release executables, whose production
sources are unchanged between `5468cf02` and this starting revision. The inputs,
options and observable-work checks come from `tools/phase7/scenarios/vscode.json`
and `tools/phase7/bench/common.py`. All four profiled runs match the expected
9,628 files and 347 diagnostics; both emit runs also match all 8,616 emitted
files by their bytes. These are diagnostic runs with a sampler, not acceptance
measurements or candidate/control timing pairs.

| Profiled phase elapsed | Rust | Go |
| --- | ---: | ---: |
| Check, without emit | 20.117 s | 5.627 s |
| Check, with emit | 18.240 s | 6.027 s |
| Emit | 18.434 s | 2.609 s |

`samply record --save-only --unstable-presymbolicate --rate 1000` samples each
runtime and mode separately. Function weights use the captured thread CPU
deltas, not wall duration; inclusive weights overlap and must not be added.
The samples establish where to experiment, not an expected speedup:

- In the check-only Rust run, semantic checking contains 34.61 seconds of
  sampled thread CPU. `CheckerState::node` and `AstView::node` alone have
  1.59 and 1.40 seconds of self weight. Access to payloads, symbols and
  source owners adds costs elsewhere. Go's semantic-check subtree contains
  10.29 seconds, with collection also running on separate runtime workers.
- In the Rust emit run, transforms contain 16.88 seconds. Of that,
  `AstBuilder::factory_view` contains 4.75 seconds: every imported-node read
  resolves the owner, root, file metadata and completed binding again.
  This is the first bounded candidate. It does not address the independent
  checking gap.
- `resolve_object_type_members` contains 2.24 seconds in checking, but that
  includes recursively resolving and instantiating members. This does not
  establish that copying its vectors accounts for that time.

Raw profiles, symbol tables, complete compiler output, work comparisons,
executable identities and the profile reader are in `target/phase7/r1/`.
The provisioned scenario cache is the existing R0 cache beside the original
R0 checkout; no new workload install or checkout is needed.

## Candidate 1: reuse a completed factory binding

A transform builder remembers the first completed exclusive input binding it
reads. Later reads of that core arena still select the retained owner and
validate the node slot, but reuse the completed binding. It is a single
`OnceLock<Arc<BindResult>>`, not a per-node cache. The first successful fill
clones one existing Arc; hits allocate nothing and take no mutex. The cache
lives only as long as the builder and does not alter the published AST layout.

Unbound inputs do not populate it: a later binding remains observable. Other
owners, lazy JSDoc nodes and files bound through overlays keep the existing
selection path. The existing multi-source test and the new mixed-owner/late-bind
regression cover those distinctions; dropping the factory returns tracked
owners and allocations to baseline.

**Retained.** Development screens start control and candidate together, alternate
launch order, and compare each pair's ratio. Builds, tests and sampling finished
before timing started. Each run matched the R0 work and emitted-byte checks;
candidate/control type counts were equal too. Across 24 pairs (48 compiler
processes), launch skew was at most 0.201 ms.

| Scenario, mode and checkers | Pairs | Median control | Median candidate | Median paired wall ratio | Median paired RSS ratio (delta) |
| --- | ---: | ---: | ---: | ---: | ---: |
| VS Code: check, 2 | 3 | 22.525 s | 22.436 s | 0.9968 | 1.0039 (+22.2 MiB) |
| VS Code: check, 8 | 3 | 12.728 s | 12.751 s | 1.0013 | 0.9998 (-1.6 MiB) |
| VS Code: check + emit, 2 | 6 | 40.915 s | 38.375 s | 0.9247 | 0.9922 (-43.7 MiB) |
| VS Code: check + emit, 8 | 3 | 19.808 s | 18.749 s | 0.9465 | 0.9766 (-162.2 MiB) |
| Self-compiler: check + emit, 2 | 9 | 1.917 s | 1.753 s | 0.9166 | 1.0100 (+3.4 MiB) |

The wall ratio is the median of individual pair ratios, not the ratio of the
two median times. VS Code check-plus-emit saves about 3.1 seconds at two
checkers and 1.1 seconds at eight. Checking alone is neutral. The self-compiler
saves about 0.16 seconds overall; its emit phase drops from roughly 0.47 to
0.31 seconds. Its first three pairs had a 2.2% median RSS increase, so six more
pairs checked that cost: all nine give +1.0% (+3.4 MiB), with individual ratios
from 0.979 to 1.032. This is not a claim of a memory improvement there.

The control executable SHA-256 is
`5abc9fb5b89a3276335c49cffecd7a1413540462ea0442faeb8c85175f8ad247`;
the candidate is
`59e8dd14848a880d8a0b1b55409fab947d9668abd3c80272f9f504cb405dbaf6`.
The release build's `compiler-artifact.executable` identified the candidate;
the saved control was not overwritten. The executable grows by 1,200 bytes.
Raw outputs, arguments, work hashes, RSS and timings are retained in
`target/phase7/r1/{check-2-screen,check-8-screen,emit-2-screen,emit-2-confirm,emit-8-screen,self-emit-2-screen,self-emit-2-confirm}/`.

The concurrent runner is committed at `tools/phase7/bench/pairs.py`. It takes
explicit paths to the saved control and candidate release executables, a
provisioned scenario cache, and a new result directory. For this workspace's
saved R1 binaries and cache, run from the repository root:

```sh
BENCHMARK_RUNS=../ts-rust-benchmarks
BENCHMARK_ARCHIVE="$BENCHMARK_RUNS/2026-10-11/target"
python3 tools/phase7/bench/pairs.py --mode emit --checkers 2 --pairs 3 \
  --control "$BENCHMARK_ARCHIVE"/phase7/r1/bin/control --candidate "$BENCHMARK_ARCHIVE"/phase7/r1/bin/candidate \
  --cache ../ts-rust-wt/.ts-rust-workloads/phase7.noindex \
  --output "$BENCHMARK_RUNS"/phase7/r1/NEW_DIRECTORY
python3 tools/phase7/bench/pairs.py --scenario self-compiler --mode emit --checkers 2 --pairs 6 \
  --control "$BENCHMARK_ARCHIVE"/phase7/r1/bin/control --candidate "$BENCHMARK_ARCHIVE"/phase7/r1/bin/candidate \
  --cache ../ts-rust-wt/.ts-rust-workloads/phase7.noindex \
  --output "$BENCHMARK_RUNS"/phase7/r1/ANOTHER_NEW_DIRECTORY
```

All paths can be changed; `--cache` defaults to
`../.ts-rust-workloads/phase7.noindex`. On a fresh checkout, provision scenarios
with `python3 scripts/phase7_scenarios.py provision vscode self-compiler`.
Build and save the control release executable before making candidate edits,
then build and save the candidate with the same release flags. Supply those
two files to the runner; it never rebuilds or overwrites them. Historical
`target/` paths in this record now have verified copies under
`../ts-rust-benchmarks/2026-10-11/target/`. The commands above use those copies
and write new runs outside the build directory. Captures and saved comparison
binaries must survive `cargo clean`.

This development script calls the existing R0 `common.arguments`, `common.run`,
`common.observe` and `common.problems`. Two threads wait on a barrier before
launching their compiler; submission order alternates and the OS determines
actual launch order. Each writes to its own fresh emit directory. Both
children finish before any output comparison or hashing starts. The script
rejects work mismatches and compares paired type counts, records both raw
results even when validation fails, and checks the provisioned inputs before
and after the batch. A failed screen stays incomplete with no median result;
a successful screen reports the median of each pair's candidate/control ratio.
It is not a new producer or a substitute for `perf.py` acceptance runs.
`scripts/tests/test_phase7_bench.py` tests concurrent execution, observation
ordering, separate emit outputs, failure handling and paired medians with
small synthetic runs. Promoting the workspace script into the repository did
not rerun or replace the measurements above.

Validation: all 190 AST unit tests and 24 doctests, focused compiler parity (8 variants,
68 subtests), AST clippy with warnings denied, formatting and ledger validation.
The focused variants cover decorators, JSDoc, destructuring and source maps;
all pass against pinned baselines and require no expectation changes. CI runs
the full suites. The five-scenario acceptance capture was not repeated for this
bounded candidate.

## Remaining R1 work

This first candidate does not close R1. Next: checker node/payload access from
the fresh check profile, then memory and scaling on the real scenarios. Retain
or reject each candidate on measured concurrent pairs. The five-scenario,
two-mode, 2/4/8-checker acceptance matrix belongs at the checkpoint exit;
no thresholds or R0 results change here.
