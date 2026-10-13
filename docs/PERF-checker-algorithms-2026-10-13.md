# Checker overload and inference experiments, 2026-10-13

Status: retain H + I + J + K on `codex/checker-algorithms`, stacked on
`codex/checker-core-speed` at `ba15b0d36103fed8c238964299598cae29dbbc60`.
L and M were removed after a direct isolation screen. The final real-project
screens show a small, consistent gain, not the large checker improvement
sought by this investigation. R1 remains open.

## Fresh profile and scope

A fresh profile of the retained #127 control ran VS Code with two checkers.
The unmodified release executable was sampled at 1,000 Hz with samply. It
matched the frozen work: 9,628 files, 347 diagnostics, and 2,509,704 types.
The compiler took 17.456 seconds wall time, including 15.990 seconds checking.
Sampled CPU totals 35.146 seconds, of which 29.136 seconds is semantic checking.
Profiling is separate from every timing screen below.

| Path | Inclusive sampled CPU | Share of semantic checking |
| --- | ---: | ---: |
| Overload selection (`choose_typed_call`) | 13.928 s | 47.8% |
| Signature applicability | 10.015 s | 34.4% |
| Call type-argument inference | 5.305 s | 18.2% |
| Flow-reference typing | 4.568 s | 15.7% |
| Type relations (`related_worker`) | 4.204 s | 14.4% |
| Flow reachability | 3.479 s | 11.9% |

These paths nest; their inclusive samples cannot be added. The remaining
node-access self costs include `CheckerState::node` at 1.458 seconds and
`AstView::node` at 1.080 seconds. Union matching contributes only 0.129 seconds
inclusive in this profile, so improving it cannot explain a large overall win.

Within applicability's argument work, the innermost expression categories
are objects 23.2%, functions 18.2%, identifiers 14.4%, property accesses 14.0%,
calls 12.3%, and arrays 9.7%. This is distributed contextual checking, rather
than a single expensive lookup. The pinned Go algorithm also repeats arguments
for inference, applicability, and context-sensitive retries. Existing Rust
caches already cover context-free expressions, resolved call signatures,
function context status and ordinary return expressions. A blanket cache keyed
only by expression would discard inference/flow context and is not justified
by this profile. No such cache or NodeId representation change was made.

Profile and attribution files are in `control-vscode-2-profile/` under the
archive described below, including `argument-subwork.json` and raw samples.

## Retained changes

- **H — overload short circuits and borrowed inference state.** Match Go's
  placement of the second rest/arity check: only a generic instantiation can
  change the candidate there. Skip callee/super lookup until a non-void `this`
  parameter needs it. Borrow inference information for the candidate/default
  scan instead of cloning candidate buffers. Avoid allocating an empty shared
  type-parameter list for nongeneric signatures.
- **I — union matching bookkeeping.** Preserve matcher calls, inference order,
  and depth/tie-break sorting. Matched types retain an ordered vector; above
  16 distinct matches a membership set replaces repeated linear membership
  scans. Hash iteration never determines inference order. The pair traversal
  itself remains quadratic; dense-match bookkeeping no longer adds another
  linear membership factor. No-match remainders borrow their original slices.
- **J — completed symbol-type cache.** Consult the shared completed-value cache
  using the symbol already resolved, before dispatching a cache hit through
  another symbol lookup. Preserve native lazy ID observation before the read
  and keep the original dispatcher order on misses. Non-value symbols retain
  their previous lookup without acquiring a native comparison ID.
- **K — deprecation checks.** Borrow declaration lists, copy them only when a
  suggestion is actually emitted, and use the pin's short-circuit `any`/`all`
  rules for merged declarations. This removes allocations from ordinary
  identifier checking without suppressing required deprecation work.

Unit tests exercise success and error order, absent/void `this`, lazy ID
assignment, borrowed no-match results, dense and duplicate union matches,
and deprecation prefixes that must or must not be evaluated. Union inference
is compared with the previous algorithm, including resulting inference state.

## Method and archive

All artifacts are outside `target/`, under
`../ts-rust-benchmarks/2026-10-13/checker-algorithms/`. Saved binaries, source
copies and patches, SHA-256 manifests, build logs, profiler output, validation
logs and every raw timing result remain there. The control is the retained
A + D + E + F executable from the preceding checker-core experiment; its
source identity was checked against the base commit above.

| Executable | SHA-256 |
| --- | --- |
| `control/tsrust` | `f1c2204b3cfd0f1635605a51ab1e29439bb38d432dc86e3f67da6b4907f7dfa1` |
| Retained `candidate-hijk/tsrust` | `3ec1aa6a822179cb92e25bbc1f5f5007be37f3af48e181e07beba8b718e0aa51` |
| Rejected `candidate-hijklm/tsrust` | `d3a205b066bb192acd4a2d4c5fa7248215de2a2f2d2141160e5a73ec81190fc0` |

The ordinary CLI release build is
`cargo build --release --locked -p tsrust --message-format=json-render-diagnostics`.
The executable comes from Cargo's artifact message. The test runner is built
separately so its features do not alter the measured CLI. After removing L/M,
all seven changed Rust files were checked against the saved H/I/J/K source
hashes; the final measurements use that exact saved executable.

`tools/phase7/bench/pairs.py` starts control and candidate concurrently at a
barrier and alternates submission order. No builds, tests or profiler run
during measurement. Output validation and hashing start after both children
finish. Every sample matches the frozen project file/diagnostic checks and
has exactly equal paired type counts. No explicit count exception is used.
Ratios below are medians of individual pair ratios, not ratios of median times.
These are development screens, not new Go/Rust acceptance results.

## Final combined screens

| Scenario; checkers | Pairs | Median wall seconds, control / candidate | Wall ratio | Check ratio | CPU user ratio | RSS ratio | Types on each side |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| VS Code; 2 | 5 | 19.684 / 19.504 | 0.9921 | 0.9903 | 0.9925 | 0.9988 | 2,509,704 |
| VS Code; 8 | 5 | 9.777 / 9.752 | 0.9960 | 0.9937 | 0.9913 | 1.0013 | 3,225,692 |
| Self-compiler; 2 | 7 | 1.396 / 1.396 | 0.9873 | 0.9889 | 0.9899 | 1.0048 | 142,282 |
| XState; 2 | 7 | 1.491 / 1.482 | 0.9935 | 0.9888 | 0.9914 | 1.0123 | 308,516 |
| MUI docs; 2 | 5 | 16.541 / 16.465 | 0.9954 | 0.9945 | 0.9951 | 1.0024 | 1,273,342 |

All 29 pairs completed; maximum recorded launch skew was 0.303 ms. These
results support retaining the small combined improvement. They do not isolate
the contribution of H, I, J or K, and do not establish a memory improvement:
XState's median RSS increases about 1.2%. Small-sample timing precision limits
apply; no confidence interval or threshold-crossing claim is made.

The directories are `final-{vscode-2,vscode-8,self-compiler-2,xstate-2,mui-docs-2}`.
Reproduce from the repository root into new directories:

```sh
ARCHIVE=../ts-rust-benchmarks/2026-10-13/checker-algorithms
CACHE=../ts-rust-wt/.ts-rust-workloads/phase7.noindex
while read -r scenario checkers pairs; do
  python3 tools/phase7/bench/pairs.py --scenario "$scenario" --mode check \
    --checkers "$checkers" --pairs "$pairs" \
    --control "$ARCHIVE/control/tsrust" --candidate "$ARCHIVE/candidate-hijk/tsrust" \
    --cache "$CACHE" --output "$ARCHIVE/NEW-$scenario-$checkers" || exit 1
done <<'SCREENS'
vscode 2 5
vscode 8 5
self-compiler 2 7
xstate 2 7
mui-docs 2 5
SCREENS
```

## Rejected additions and earlier screens

L added a direct paged directory for checker-local value-symbol links, keeping
foreign-owner routing and ownership/census tests. M borrowed signature
parameters in parameter lookup helpers instead of cloning the shared list.
Together they added code but no measurable speed over H/I/J/K, so both were
removed. Their patches, tests, binaries and measurements remain archived.

| Candidate / control, VS Code two checkers | Pairs | Wall ratio | Check ratio | CPU user ratio | RSS ratio | Directory |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| H/I/J/K / original control | 3 | 0.9859 | 0.9861 | 0.9889 | 0.9983 | `hijk-vscode-2` |
| H/I/J/K/L/M / original control | 5 | 0.9884 | 0.9883 | 0.9887 | 1.0034 | `hijklm-vscode-2` |
| H/I/J/K/L/M / H/I/J/K (direct isolation) | 3 | 0.9991 | 1.0004 | 1.0000 | 0.9988 | `hijklm-vs-hijk-vscode-2` |

Use the same runner, scenario and checker mode with the table's saved
executables and pair counts to reproduce these screens. Do not subtract
separate batches to estimate L/M: the direct isolation is the relevant result.
Prepared synthetic dense-union inputs in `dense-union-fixtures/` were not run;
they are not measurement evidence or a retention justification.

## Validation and next limit

Final H/I/J/K validation: 110 checker unit tests; checker clippy across all
targets/features with warnings denied; formatting; and `cargo xtask validate`
all passed. Two unchanged checker doctest passes were reused. A separately
built release runner passed 61 selected compiler variants (549 subtests),
with no failures, skips or expectation differences against the pin. Exact
commands and selected IDs are in `validation-final/results.json`; output is in
`parity-final/`. No parity expectations or status records needed an update.
The full corpus remains CI's responsibility.

The remaining larger opportunity is reducing repeated contextual argument
work while preserving candidate-specific inference, diagnostic order, fresh
object types and lazy symbol/type creation. This push did not establish a
safe cache or a way to skip those semantic passes. Accessor and local-directory
changes measured so far do not justify a broad NodeId rewrite. R1's complete
scenario, mode and checker-count acceptance matrix remains separate.
