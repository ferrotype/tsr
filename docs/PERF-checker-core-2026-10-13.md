# Checker core experiments, 2026-10-13

Status: this experimental push is complete on `codex/checker-core-speed`,
starting from `feaf91c45442e7ba10bfc006cb03b4e65f5049eb`. Retain the measured
A + D + E + F combination below. B, C and G were removed after screening.
This does not close the broader R1 performance work.

## Where the time goes

The existing R1 VS Code profile attributes 34.61 seconds of sampled CPU to
semantic checking. Inclusive samples overlap: overload selection has 16.91 s,
signature applicability 12.39 s, argument inference 6.36 s, flow-reference
typing 5.33 s, and reachability 4.45 s. They are not additive savings estimates.
`CheckerState::node` and `AstView::node` alone have about 3 s of self samples.
These are historical R1 samples, not a new profile of any candidate below.
The work below tests production mechanisms within those paths.

`NodeId` remains the full owner/slot identity. No experiment narrows its domain,
removes owner or slot checks, or stores a mutable factory node's kind in its ID.
The old kind-bit design's read frequencies predate the retained reference-shape
optimization and are not evidence of its present payoff.

## Method and artifacts

Build the control and each candidate with the same ordinary release command:
`cargo build --release --locked -p tsrust --message-format=json-render-diagnostics`.
Save the Cargo-selected executable before another build. Each archive includes
the binary, its SHA-256, source copies/patch, build log, arguments, raw compiler
outputs, and paired results. All are outside `target/` at
`../ts-rust-benchmarks/2026-10-13/checker-core-speed/` (the archive below).

Use `tools/phase7/bench/pairs.py`: control and candidate start concurrently at a
barrier; submission order alternates; both processes finish before observation
and hashing. No builds, tests or profiler run alongside a screen. Each process
must match the frozen files, diagnostics and options. The default additionally
requires equal paired type counts. Ratios below are medians of individual
candidate/control pair ratios, including the check-phase ratio; they are not
ratios of two median times. These development screens do not replace Go/Rust
acceptance captures or change any threshold, gate, or status expectation.

| Saved executable | SHA-256 |
| --- | --- |
| `control/tsrust` | `19c8be1225728a33b6582e0c8c798e10282f99e99b06891e5ce2c2be51d5761f` |
| `candidate-a/tsrust` | `48e450ee6543a086f4d2e0968f7bff81ea5e74da3454155819f16feafe0bb284` |
| `candidate-b/tsrust` | `2a4299130178a2ec3b62ce31821f340cd1ae49926ddd4c131f4b0110c615f337` |
| `candidate-c/tsrust` | `7832b9fe82350128849f827a23972bb07eb55ce8b0f07c477d3d9deec0dca0fd` |
| `candidate-ef/tsrust` | `1c2efaf7afad9e05917cced9bbdef4e2e4cac27983ed0cd04057454cafe698f2` |
| `candidate-def/tsrust` | `f1c2204b3cfd0f1635605a51ab1e29439bb38d432dc86e3f67da6b4907f7dfa1` |
| `candidate-g/tsrust` | `9ab91ae284a290a250533faedf0c7f6ccf23e050334e6147d99b7e3015a2efbd` |

## Screened candidates

These screens are VS Code checking, two checkers, three pairs each. EF
means A + E + F with D removed for isolation; DEF means A + D + E + F. B and C
are separate additions to A. Their measured ratios must not be added.

| Candidate | Control | Wall ratio | Check ratio | RSS ratio | Decision; result directory |
| --- | --- | ---: | ---: | ---: | --- |
| A: inline keys, instantiation scratch/list reuse, bounded ancestry caches, retained flow graph | Original head | 0.9806 | 0.9794 | 0.9841 | Retained in combined candidate; `a-vscode-2` |
| B: checked scalar kind accessor at 73 hot call sites | A | 0.9965 | 0.9985 | 1.0067 | Removed; negligible gain for API/call-site churn; `b-vs-a-vscode-2` |
| C: inference traversal buffer reuse and fewer inference-info clones | A | 1.0033 | 0.9996 | 0.9987 | Removed; neutral here; `c-vs-a-vscode-2` |
| EF: retained flow-target reads and scoped name-resolution core view | Original head | 0.9645 | 0.9452 | 0.9800 | Retained for combined screen; `ef-vscode-2` |
| G: reuse ordinary-call arity syntax across overload candidates | DEF | 1.0022 | 1.0004 | 1.0040 | Removed; neutral despite eliminating repeated syntax scans; `g-vs-def-vscode-2` |

A keeps exact key bytes, including long keys; common keys up to 22 bytes fit
inside a 24-byte key on 64-bit hosts. Census accounting charges the larger
map buckets and only separately allocated key backing. Active instantiation
maps retain capacity after clearing; inactive maps are not semantic roots.
Unchanged instantiated lists retain their original shared backing. Ancestry
caches are bounded to 40 KiB per checker and apply only to immutable program
core nodes. A retained flow binding serves checked graph reads; antecedent
lists are validated before streaming so a malformed tail cannot be hidden by
an early branch result.

B preserved overlays, imported owners, lazy nodes, factory replacement,
access-statistics accounting and the complete open `NodeKind` domain. Its
screen did not justify keeping it. C reproduced Go's inference-state free-list
discipline, but that alone did not make it beneficial here. Their code,
tests and measurements remain in the corresponding candidate/result archives.
EF borrows a retained immutable file view for flow targets and name-resolution
reads; factory, imported and lazy owners keep the existing checked fallback.

G cached only immutable call/list facts across candidate signatures and kept
parameter, rest, void and inference work live. It added about 40 stack bytes
to each `TypedCall`. Its 103 unit tests and clippy passed; all three timed
pairs kept identical outputs and type counts. Neither checking time nor CPU
improved materially, so the implementation and tests remain only in the
external `candidate-g/` archive. The production source was restored to DEF
and checked against the saved build's source hashes.

## D: pinned short-circuit and exact type-count expectations

Pinned Go's `checkExpressionWithContextualType` tests whether an argument type
is literal before instantiating its contextual type. Rust previously
instantiated the context unconditionally, even when the result was unused.
D restores that short-circuit; literal freshness still uses the instantiated
context. This avoids some type construction without changing files or
diagnostic bytes.

The default runner correctly rejected the first DEF pairs for unequal type
counts. Those incomplete captures remain in `def-vscode-2`, `def-vscode-8`,
`def-self-compiler-2-discovery`, `def-xstate-2-discovery`, and
`def-mui-docs-2-discovery`; none publishes a median result. The EF isolation
screen preserved **2,509,903** types on both sides at two checkers. Adding D
produced **2,509,704**, 199 fewer. This isolates that VS Code count change from
A/E/F; the other configurations have their own observed exact counts below.

The runner now accepts optional `--control-types` and `--candidate-types`
only together, as positive integers, with a nonempty `--type-count-reason`.
It records the expected counts and reason in `summary.json` and rejects every
sample that differs from either declared count. The default continues to
reject any paired count mismatch. Frozen output checks, the oracle type-count
floor, positive timing requirements and input-identity checks still apply.
There is no skip-count option or tolerance for count drift.

## Combined DEF screens

All five captures below completed with the same saved DEF and original control
executables. Each sample matched its declared per-side type count and frozen
files/diagnostic bytes. Times are the two separate median wall durations, for
scale; the ratio is calculated from the pairs themselves.

| Scenario; checkers | Pairs | Median wall seconds, control / DEF | Wall ratio | Check ratio | RSS ratio | Exact types, control / DEF |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| VS Code; 2 | 5 | 21.591 / 20.738 | 0.9597 | 0.9446 | 0.9863 | 2,509,903 / 2,509,704 |
| VS Code; 8 | 5 | 9.784 / 9.513 | 0.9710 | 0.9617 | 0.9894 | 3,225,966 / 3,225,692 |
| Self-compiler; 2 | 7 | 1.615 / 1.518 | 0.9255 | 0.9217 | 1.0004 | 142,452 / 142,282 |
| XState; 2 | 7 | 1.617 / 1.561 | 0.9620 | 0.9512 | 1.0109 | 308,559 / 308,516 |
| MUI docs; 2 | 5 | 17.963 / 17.769 | 0.9891 | 0.9865 | 0.9956 | 1,273,345 / 1,273,342 |

Sources are `def-{vscode-2,vscode-8,self-compiler-2,xstate-2,mui-docs-2}-explicit/summary.json`
in the archive. DEF remains the measured combined candidate for this push.
The observed wall reductions range from about 1.1% to 7.5%; MUI's result is
small and XState's RSS increases about 1.1%. These screens establish neither
a large checker speedup nor an improvement to a Go acceptance gate.

The existing provisioned cache is
`../ts-rust-wt/.ts-rust-workloads/phase7.noindex`. From the repository root,
reproduce the five configurations into new directories:

```sh
ARCHIVE=../ts-rust-benchmarks/2026-10-13/checker-core-speed
CACHE=../ts-rust-wt/.ts-rust-workloads/phase7.noindex
while read -r scenario checkers pairs control_types candidate_types; do
  python3 tools/phase7/bench/pairs.py --scenario "$scenario" --mode check \
    --checkers "$checkers" --pairs "$pairs" \
    --control "$ARCHIVE/control/tsrust" --candidate "$ARCHIVE/candidate-def/tsrust" \
    --cache "$CACHE" --output "$ARCHIVE/NEW-def-$scenario-$checkers" \
    --control-types "$control_types" --candidate-types "$candidate_types" \
    --type-count-reason 'D matches pinned Go: nonliteral arguments skip unused contextual instantiation.' || exit 1
done <<'SCREENS'
vscode 2 5 2509903 2509704
vscode 8 5 3225966 3225692
self-compiler 2 7 142452 142282
xstate 2 7 308559 308516
mui-docs 2 5 1273345 1273342
SCREENS
```

For the equal-count EF isolation, use the same runner with
`--scenario vscode --mode check --checkers 2 --pairs 3`, original control,
`--candidate "$ARCHIVE/candidate-ef/tsrust"`, and a fresh output directory;
omit all three explicit type-count arguments. A/B/C use their table's saved
control/candidate files with those same scenario/checker/pair settings.

## Validation and remaining bottlenecks

Before the DEF screens: 101 checker unit tests, two checker doctests, and
50 exact compiler variants (450 subtests) passed. The focused comparator
validated completeness and the pin, and found no unexpected failures or
expectation differences. `run --id` is a development selection and cannot
be passed to full-suite `parity.py check`; the archived
`candidate-def/compare-selected-parity.py` applies the existing completeness
and failure-ID comparator helpers to the selected variants without changing
metadata or expectations. All 34 focused Python harness tests passed;
checker clippy with warnings denied, formatting and ledger validation passed.
Logs are in `candidate-def/` and `type-count-runner-tests.log`. No parity
acceptance or status-ledger update was needed.

The historical R1 profile above remains the attribution source; no new
candidate profile was captured for these results. Overload applicability,
argument inference, recursive relations and flow analysis remain the larger
targets. A generic per-file reader would mostly remove an already-cached owner
lookup: sealed core nodes already use flat storage, and B's scalar bypass was
neutral. Packing kind bits into `NodeId` would add mutable-kind, foreign-owner
and large-file complications without a demonstrated payoff from these screens.
The retained scoped reads avoid repeated work while preserving full identities.

A larger next change needs evidence of removable work within those semantic
algorithms, rather than another accessor wrapper. These results do not support
claiming a twofold speedup or projecting savings by adding individual samples.
