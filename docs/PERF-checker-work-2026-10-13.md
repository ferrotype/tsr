# Matched checker work, 2026-10-13

The matched-work investigation found a missing index-signature short circuit,
now restored to the pinned Go behavior. It did **not** find a large excess of
overload or inference work that would justify rewriting those algorithms.
This is a diagnostic and a focused development screen, not Phase 7 acceptance.

## What was compared

Temporary source instrumentation counts 44 executed checker operations on
the self-compiler, VS Code and XState projects. Both compilers use one checker,
the same frozen file order, source bytes and options. Native diagnostics match
the frozen work on all three projects. Type-creation counters also equal each
compiler's own reported type count. Go and Rust type counts need not equal one
another; their small existing differences are recorded below.

The Go authority is pin `1f70213d4922b434345f639b441681e470c7cfc1`, built
with Go 1.27.1. Rust uses 1.97.1. The control Rust source is `9b854a5b`
(its production Rust is unchanged from `9169eec0`). The candidate adds only the
guard and its tests to production sources. Everything runs on this macOS
arm64 host. Loading can use workers; the diagnostic checker count is one.

`tools/phase7/bench/work_counts.py` builds Go through an overlay without editing
the submodule. It temporarily instruments Rust, preserving original and injected
bytes before changing any file. Restoration verifies hashes and refuses to
overwrite intervening edits. Both saved executables and raw counts live outside
`target/`. **Instrumented executions supply no performance numbers.** The
counts observe semantic entries made visible by instrumentation; ordinary
optimization can inline or eliminate parts of those operations. Optional
relation-origin capture aggregates static call sites instead of writing a large
event trace. In a second capture every one of the 44 counts exactly reproduced
the first run, for both runtimes and all three workloads.

Archive: `../ts-rust-benchmarks/2026-10-13/checker-work-counts/`.

## Findings

Each table cell is **Go / Rust before the guard**. These are operation counts,
not equal-cost units; nested counters must not be added.

| Operation | Self-compiler | VS Code | XState |
| --- | ---: | ---: | ---: |
| Overload selection | 53,232 / 53,232 | 928,284 / 928,291 | 24,542 / 24,542 |
| Overload candidates examined | 56,606 / 56,606 | 1,064,036 / 1,064,043 | 28,078 / 28,071 |
| Contextual overload retries | 1,088 / 1,088 | 28,568 / 28,568 | 2,245 / 2,245 |
| Applicability arguments | 96,087 / 96,087 | 1,385,219 / 1,385,232 | 28,085 / 28,085 |
| Call type-argument inference | 10,307 / 10,307 | 222,154 / 222,161 | 17,213 / 17,213 |
| Recursive inference worker | 319,766 / 319,766 | 1,407,544 / 1,407,600 | 326,830 / 326,790 |
| Normal expression-cache misses | 31,557 / 31,557 | 379,503 / 379,504 | 8,713 / 8,713 |
| Type-instantiation worker | 168,789 / 168,732 | 2,919,430 / 2,917,445 | 1,467,144 / 1,428,522 |
| Front relation requests | 1,633,068 / 1,680,670 | 5,149,357 / 5,959,754 | 424,134 / 509,333 |
| Relation worker | 690,780 / 691,485 | 4,101,965 / 4,103,579 | 377,112 / 378,682 |
| Property relations | 43,549 / 43,549 | 905,816 / 905,918 | 113,886 / 113,978 |
| Types created | 108,942 / 109,084 | 2,232,481 / 2,231,749 | 279,263 / 278,541 |

Overload selection, retries, argument checks and recursive inference perform
essentially the same number of operations. The measured normal expression-cache
misses and type-instantiation workers show no broad excess in these projects.
These counts do not prove universal
algorithmic equivalence or explain the cost of each operation.

The relation-request excess was actionable. `applicable_index_info` checked
whether a key could use a string index even when the object had no string index.
Pinned `findApplicableIndexInfo` tests `stringIndexInfo != nil` first. The Rust
fallback now makes the same short circuit. It still resolves members, examines
other index candidates in order, and checks applicability when a string index
actually exists. It does not substitute a cached or approximated relation.

| Work removed by the guard | Self-compiler | VS Code | XState |
| --- | ---: | ---: | ---: |
| Front relation requests | 37,111 | 807,905 | 94,185 |
| Relation-worker entries | 0 | 22 | 7 |
| Recursive relation entries | 0 | 9 | 7 |
| Types created | 0 | 0 | 0 |
| Remaining Rust front requests | 1,643,559 | 5,151,849 | 415,148 |

All other counters remain identical to the Rust control except the recursive
relation-cache hits corresponding to those 9 and 7 skipped entries. Files,
diagnostic output and compiler-reported types remain unchanged. Most removed
calls returned before structural relation work, so the request reduction cannot
be interpreted as the same percentage reduction in checking time.

## Counting limits and remaining differences

- The generators instrument equivalent semantic sites, including Rust sites
  that inline a Go helper. They reject absent or ambiguous source anchors.
  A numeric type or node identity is never joined across runtimes.
- Overload counts are relation passes and candidate visits, not unique source
  calls. The argument counter covers ordinary non-omitted positional arguments,
  not JSX or the separate rest-argument processing. Instantiation-request counts
  also see Go's nil-type inputs, which Rust's type-ID API cannot represent.
- Front relation requests are not every type comparison. Some Go reporting
  paths enter the relater directly where Rust enters through the front helper.
  For class heritage this contributes 3 / 15,643 / 6 additional Rust front
  requests. Those are an entry-boundary difference, not duplicate comparisons.
- `expression_cached_request` includes nonnormal-mode bypasses;
  `expression_cached_miss` counts normal unresolved-cache evaluations. Their
  difference is **not** a cache-hit count. The context-free expression cache is
  a separate cache.
- Flow-node visits count actual execution. VS Code has 22,825,171 Go visits
  versus 23,412,877 Rust visits. Reachability-node visits go the other way:
  5,761,599 versus 2,995,853. Rust's existing reachability cache avoids worker
  reentry and retains pass-through results differently. These counters cannot
  establish a common-cost total or a speedup by subtraction.
- Origin buckets require semantic grouping: inference constraint helpers,
  comparability helpers and assignment-reduction helpers have different Rust
  and Go function boundaries. The saved origin build also leaves Rust's
  `source_type_assignable` forwarding wrapper grouped at its own call site.
  The current generator propagates that origin further, but that refinement
  was not recaptured; the guard's direct before/after counts above establish
  its effect independently.
- There are small residual differences in type creation and relation/flow
  work. They remain visible in the raw reports. No counter was adjusted to
  force equality, and no semantic cache was added on the strength of a count.

## Ordinary-binary timing and retention

The saved ordinary release control has SHA-256
`3ec1aa6a822179cb92e25bbc1f5f5007be37f3af48e181e07beba8b718e0aa51`.
The guard candidate is `guard-release/tsrust`, SHA-256
`11d56b80da8a3d65f46c3f25c1f97fee3562f398f235eb4cca29e786c9d85dfc`.
Both use the workspace's ordinary optimized release profile: fat LTO, one
codegen unit and unwinding. Neither contains work counters or PGO. Cargo's
artifact message selected the candidate; it was copied before building the
test runner so test features cannot change the measured executable.

All 24 pairs started control and candidate concurrently at a barrier,
alternating submission order. No builds, tests, instrumentation or profiler
overlapped the screen. Each sample matched the frozen file/diagnostic output
and exactly equal paired type counts. Input bytes, bundled assets, options,
file order, sources and executable identities were checked before and after.
Maximum recorded launch skew was 0.235 ms.

Ratios are medians of **individual candidate/control pairs**, not ratios of the
median times in the adjacent column. Smaller is faster or uses less memory.

| Workload; checkers | Pairs | Median wall seconds, control / candidate | Wall ratio | Check ratio | User CPU ratio | RSS ratio |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| VS Code; 2 | 5 | 19.992 / 20.010 | 1.0002 | 0.9997 | 1.0019 | 1.0059 |
| VS Code; 8 | 5 | 10.114 / 10.098 | 0.9983 | 0.9985 | 1.0021 | 1.0009 |
| Self-compiler; 2 | 7 | 1.450 / 1.442 | 0.9810 | 0.9991 | 0.9994 | 0.9950 |
| XState; 2 | 7 | 1.478 / 1.477 | 0.9992 | 1.0000 | 0.9999 | 1.0005 |

**No meaningful checker-time gain is demonstrated.** All median check ratios
are within 0.15% of one; user CPU is also effectively unchanged. The
self-compiler's wall ratio does not come with a corresponding checking or CPU
gain and is not evidence of faster relation work. These bounded screens do
not establish an improvement outside timing noise or a memory reduction.

Retain the guard because it restores a missing Go short circuit and avoids
relations/cache effects that should never run, with a regression test that
failed before the fix. Do not sell it as a checker speed improvement. The
measured larger gain from the separate [PGO experiment](PERF-checker-pgo-2026-10-13.md)
remains separate; the guard was not combined with PGO in this screen.

## Validation and conclusion

- 111 checker library tests with `relation-probe` passed. The new observer test
  proves absent indexes perform no fallback relation and separately proves
  that the same valid generic key would enter the relater. Two more tests cover
  real string indexes and overlapping template-pattern indexes.
- Checker clippy with all targets, `relation-probe` and warnings denied passed.
  Formatting, `git diff --check` and `cargo xtask validate` passed.
- 18 selected compiler variants, 162 subtests, passed with no failures, skips
  or parity expectation changes. They cover no-signature access, generic and
  numeric keys, overlapping template keys, JSX, inheritance and intersections.
- 62 focused Python tests passed across the counter, PGO and paired-run tools.
  The 16 counter-tool tests also pass under `python -O`; external source
  anchors and existing helper checks cannot disappear under optimization.

Logs and exact commands are in `validation-index-guard`, `validation-final`
and `validation-guard-parity`; selected IDs are in
`index-guard-selected-parity.json`. Complete parity suites remain CI's job.

The proposed large overload/inference rewrite has no supporting repeated-work
evidence here. The comparison found and removed a real unnecessary operation,
but timing shows it does not explain the speed gap. Further performance work
needs attribution of **cost within equivalent operations**, rather than an
assumption that Rust is repeating much more semantic work. These three projects
do not rule out an algorithmic discrepancy on other inputs. R1 stays open.

## Reproduction

Run one build at a time in a checkout nobody is editing. Choose fresh output
directories; the commands refuse to replace a previous capture. Provision the
ordinary Phase 7 workload cache first.

```sh
ARCHIVE=../ts-rust-benchmarks/NEW-checker-work-counts
CACHE=../ts-rust-wt/.ts-rust-workloads/phase7.noindex
python3 tools/phase7/bench/work_counts.py build \
  --go /path/to/go1.27.1/bin/go --output "$ARCHIVE/build"
python3 tools/phase7/bench/work_counts.py capture \
  --build "$ARCHIVE/build" --output "$ARCHIVE/counts" --cache "$CACHE" \
  --scenario self-compiler --scenario vscode --scenario xstate
```

`build --relation-origins` enables the optional caller attribution.
`build --allow-dirty` explicitly permits edits to tracked candidate inputs and
records their source hashes and patch. Untracked build inputs are rejected
because a Git patch would omit their contents; ordinary use requires clean
production inputs.
After an interrupted instrumented build, use
`work_counts.py restore <build-directory>/rust-overlay` before compiling again.
Do not use either instrumented executable for timing. The saved original runs
are `build-03` / `capture-01`, `origin-build` / `origin-capture`, and
`guard-count-build` / `guard-count-capture`.

Build an ordinary, uninstrumented CLI for timing with
`cargo build --release --locked -p tsrust --bin tsrust --message-format=json-render-diagnostics`
and save the executable selected from Cargo's artifact message. The archived
ordinary build is in `guard-release/`, including its source hashes and patch.
Reproduce the screen with the saved binaries and fresh output directories:

```sh
ARCHIVE=../ts-rust-benchmarks/2026-10-13/checker-work-counts
CONTROL=../ts-rust-benchmarks/2026-10-13/checker-context/control/tsrust
CACHE=../ts-rust-wt/.ts-rust-workloads/phase7.noindex
while read -r scenario checkers pairs; do
  python3 tools/phase7/bench/pairs.py --scenario "$scenario" --mode check \
    --checkers "$checkers" --pairs "$pairs" \
    --control "$CONTROL" --candidate "$ARCHIVE/guard-release/tsrust" \
    --cache "$CACHE" --output "$ARCHIVE/NEW-guard-$scenario-$checkers" || exit 1
done <<'SCREENS'
vscode 2 5
vscode 8 5
self-compiler 2 7
xstate 2 7
SCREENS
```

The recorded screen directories are `guard-{vscode-2,vscode-8,self-compiler-2,xstate-2}`.
Every raw pair remains there. `guard-preflight` and `guard-postflight` preserve
the file/input/option checks; `guard-screen-analysis.json` summarizes ratios.
