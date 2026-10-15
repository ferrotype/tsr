# Checker operation experiments

These development experiments compare routed reads, owner-scoped reads, typed
payload access and explicit expression continuations. They measure complete
operations with their semantic callbacks. They do not replace
whole-compiler parity or throughput measurements.

The feature is `operation-experiments`, propagated from `tsr_compiler` to
`tsr_checker`. The example is
[`checker_operations`](../../crates/tsr_compiler/examples/checker_operations.rs);
the orchestration is
[`tools/phase7/bench/operations.py`](../../tools/phase7/bench/operations.py).
The feature is not enabled in normal builds.

## Why these two operations

The archived profile analysis is at
[`checker-operations/analysis.txt`](../../../ts-rust-benchmarks/2026-10-15/checker-operations/analysis.txt),
with explicit groups in
[`operation_analysis.py`](../../../ts-rust-benchmarks/2026-10-15/checker-operations/operation_analysis.py)
and derived values in
[`operation-analysis.json`](../../../ts-rust-benchmarks/2026-10-15/checker-operations/operation-analysis.json).
These paths refer to the external evidence archive beside the checkout, not
files committed with this document.

The archive compares saved PGO Rust, ordinary Rust, and Go executables on the
same VS Code project. Its manifest records identical input and diagnostic-output
hashes, 9,628 files, and 347 diagnostics. Rust work counts agree between builds;
Go's type, symbol, and instantiation counts differ slightly. Those samples
predate these experimental changes.

All values below are sampled CPU attributed to stacks containing the semantic
phase boundary. They sum CPU across threads; they are not wall time or a speedup
measurement. Go background GC is outside that boundary, while assists on
checker stacks remain inside it.

| Attribution | PGO Rust | Ordinary Rust | Go |
| --- | ---: | ---: | ---: |
| Semantic-stack CPU | 27.331 s | 32.687 s | 11.406 s |
| Either flow type or reachability, inclusive union | 20.75% | 22.97% | 19.47% |
| Visible node-access self CPU | 7.35% | 20.69% | No equivalent separate group |
| Visible symbol-access self CPU | 0.20% | 3.87% | No equivalent separate group |
| Visible flow-storage self CPU | 0.02% | 0.67% | No equivalent separate group |

Ordinary contextual/applicability stacks contain 2.907 s of visible node-access
self CPU and 0.594 s of symbol-access self CPU. Ordinary flow stacks contain
1.517 s of node-access self CPU and 0.216 s of flow-storage self CPU. These
operation envelopes overlap and must not be added.

This ranks a contextual node-reader experiment ahead of a flow-reader
experiment. It does not establish their recoverable costs. PGO absorbs many
accessors and contextual wrappers into other symbols: ordinary
`check_function_expression_context` has 5.277 s inclusive attribution and no
separate PGO symbol, while `collect_signature_applicability_errors` changes from
10.934 s ordinary to approximately 1 ms visible PGO. Such differences cannot be
interpreted as eliminated operations. Go pointer loads are similarly often
inside algorithm-named symbols.

## Contextual argument checking: live state

`argument` observes or times contextual argument operations during a fresh,
single-checker semantic pass. Each backend checks the same complete program
prefix, so inference contexts, contextual stacks, type allocation, and checker
caches arise naturally. A request recorded midway through checking is not a
standalone replay snapshot.

The driver uses the project's effective configuration with `noEmit: true`,
`incremental: false`, and `composite: false`, and a single checker. These are
cold semantic-check experiments, not incremental/project-build measurements.
The effective options are hashed alongside the retained configuration sources.

In `observe` mode the report records ordered entry/return evidence, kind counts,
and a bounded sample of source locations and operation state, including the
contextual type, inference context, expression mode, result or error, type
counts, and state restoration. The trace checksum covers the full trace; the
samples aid inspection. It is a diagnostic checksum rather than an independent
semantic oracle. Location and cleanup errors invalidate a run.

In `time` mode only outermost operation intervals contribute to `elapsed_ns`.
Their nested calls belong to those intervals and are not timed again. Trace
hashing and sample collection are omitted; calls, outermost calls, operation
errors, and cleanup checks still provide coverage. Therefore every timed pair
can compare counts, diagnostics, and work, but cannot independently compare the
full per-request trace. That comparison comes from the preceding observed runs.

## Flow reachability: a warm replay

`flow` first checks a fresh program and captures an ordered, bounded schedule of
production reachability requests. The current driver captures at most 100,000
requests with stride one. It then replays that schedule against the same checker
using the selected reader. The request digest retains source/owner/flow slot
identity while excluding process-global arena identity. The result digest
covers ordered results, including errors.

This is a warm replay after semantic checking. It holds the request schedule and
semantic state more nearly constant, but does not reproduce the original cache
state at every first-time query. It does not measure type-at-flow as a whole or
show that the flow algorithm visits too many nodes.

The elapsed interval includes traversal and semantic callbacks. Source hashing,
diagnostic serialization, and final request/result digest generation happen
outside that interval. Node/list reads and semantic callbacks must agree between
backends. `local_requests + routed_requests` counts kernel entries, including
semantic callback reentries, so it can exceed the captured root-request count.
The runner requires equal kernel-entry totals, at least one actual node read,
zero local entries for the routed backend, and nonzero local entries for the
scoped backend. Errors invalidate the measurement. A truncated capture remains
a bounded experiment, and its truncation flag must agree in every run.

Both experimental readers check completed root results at the same entry point,
before the traversal stack guard. A scoped hit therefore gets no extra shortcut
relative to its control. Only a miss selects the checked reader or imports the
root into the branded local reader. The default production build retains its
existing entry behavior. Request identity is streamed into SHA-256; no JSON
request tree remains allocated during replay.

## Running a saved executable

Build the feature-gated example separately and save its executable together with
its Cargo artifact messages, source identity, and effective compiler flags. For
example, the build target is:

```sh
cargo build --release -p tsr_compiler --example checker_operations \
  --features operation-experiments --message-format=json-render-diagnostics
```

The runner never builds. Its optional build manifest is archived as supplied
metadata, with a file hash; it does not verify or infer optimization, PGO, LTO,
or other compiler flags. Its source revision/status describe the checkout when
the runner starts, not the saved binary's build inputs. The binary hash is
checked again at the end.

The small shared fixture is
[`tools/phase7/operations/tsconfig.json`](../../tools/phase7/operations/tsconfig.json).
It includes generic and nested callbacks, contextual object methods, overloads,
destructuring, branch and switch narrowing, loops, and calls returning `never`.
It is checked, never executed. It provides coverage, not a representative timing
workload; use the provisioned real project for timing conclusions.

With `EXPERIMENT_BINARY` set to the saved executable's absolute path, the actual
driver interface is:

```sh
"$EXPERIMENT_BINARY" "$PWD/tools/phase7/operations/tsconfig.json" argument routed observe
"$EXPERIMENT_BINARY" "$PWD/tools/phase7/operations/tsconfig.json" argument scoped observe
"$EXPERIMENT_BINARY" "$PWD/tools/phase7/operations/tsconfig.json" flow routed observe
"$EXPERIMENT_BINARY" "$PWD/tools/phase7/operations/tsconfig.json" flow scoped observe
```

Each invocation loads a fresh program and checker. A successful child writes one
JSON report to stdout and exits zero. A driver failure is preserved by the runner
with its exit status and stderr.

For the paired screen, set `PROJECT_TSCONFIG` to an absolute project config and
`RESULT_ROOT` to an external evidence directory. Each output directory must be
new and outside the checkout, including outside its `target` directories.
`BUILD_MANIFEST` names the saved build-metadata JSON file; omit that option if no
manifest is available, and report that limitation.

```sh
python3 tools/phase7/bench/operations.py run \
  --binary "$EXPERIMENT_BINARY" --project "$PROJECT_TSCONFIG" \
  --experiment argument --pairs 5 --timeout 600 \
  --build-manifest "$BUILD_MANIFEST" --output "$RESULT_ROOT/argument"

python3 tools/phase7/bench/operations.py run \
  --binary "$EXPERIMENT_BINARY" --project "$PROJECT_TSCONFIG" \
  --experiment flow --pairs 5 --timeout 600 \
  --build-manifest "$BUILD_MANIFEST" --output "$RESULT_ROOT/flow"
```

The runner first observes routed and scoped backends and requires exact workload
and operation evidence. It then launches fresh processes in simultaneous pairs
from a thread barrier, alternating submission order. The OS determines actual
launch order; no process-level barrier synchronizes the later operation start.
The runner performs no output parsing, validation, or hashing until both children
finish. The children still perform their own setup and result preparation, which
can overlap the other child's measured interval. Shared-resource contention is
part of this development screen's noise.

Every timed sample must have positive elapsed time and nonzero operation
coverage. Workload identity includes the ordered source-file list and byte
hashes, the effective compiler-options digest for the program and each source
file, and the ordered config/dependency paths and byte hashes. All must match
the observation, alongside the canonical diagnostic digest, diagnostic count,
type/signature/instantiation counts, and stable operation counts/results. A
config or option change invalidates the run even if both timed backends agree
with each other. Observe-only fields are
compared only between observed runs. Backend routing counts are compared by total
kernel entries rather than by their local/routed split. Additional stable driver
fields are compared exactly instead of silently ignored.

`summary.json` records the requested pairs, completed pairs, binary identity,
metadata, per-pair `scoped / routed` operation-time ratio, and median paired
ratio. Ratios below one favor scoped access for the measured interval. Raw
stdout, stderr, process metadata, parsed reports, and validation errors remain
in the output directory. A failed run stays `complete: false`; it contributes no
accepted result and its directory cannot be reused.

## Status and interpretation

The first screen completed on 15 October 2026 using an ordinary release build
(fat LTO, one codegen unit, no PGO). The exact Cargo command, compiler version,
source hashes before/after building, and saved executable are in the external
archive's `operation-build-v2/`. The executable SHA-256 is
`1e43eb869650d8510fe8009ea38f79132d4662e95ea949c016af119ec3540203`.
All reports and raw process outputs are beside it under
`self-compiler-{argument,flow}` and `vscode-{argument,flow}`; `results.json`
summarizes the paired samples. The initial fixture-only executable is preserved
separately as `operation-build/`.

Each row has five simultaneous fresh-process pairs. Percentages are the median
paired operation-time ratio; saved milliseconds are the median paired absolute
difference, not a subtraction of independently selected medians.

| Operation | Project | Scoped / routed | Median saved time |
| --- | --- | ---: | ---: |
| Contextual argument | TypeScript compiler | 0.9561 | 27.45 ms |
| Contextual argument | VS Code | 0.9725 | 331.02 ms |
| Warm reachability replay | TypeScript compiler | 0.9412 | 0.22 ms |
| Warm reachability replay | VS Code | 0.9466 | 0.36 ms |

The contextual observations cover 112,672 calls on the compiler project and
1,630,385 calls on VS Code, including 16,579 and 245,116 inferential calls.
All ordered request/result checksums, cleanup checks, diagnostics, source and
option hashes, and type/signature/instantiation counts agree. The programs load
249 and 9,628 files and create 108,911 and 2,231,749 types respectively.

Both flow replays cover the first 100,000 captured requests: the full captures
observed 131,469 and 2,067,431 requests, so both are truncated prefixes. They
perform 56,848/60,526 node visits, 25,760/24,348 list visits and 4,368/6,252
semantic callbacks, equal across readers. No extra types are created in the
warm replay. The VS Code flow pair ratios range from 0.9276 to 1.0343: this tiny
interval is noisy, and its fractional-millisecond difference does not justify
further investment as a major CPU optimization. These prefixes cannot estimate
the cost of the complete production flow algorithm or its cold semantic work.

The contextual candidate still uses full `NodeId`/`NodeRead` and normal payload
decoding; it changes owner selection, not the entire access representation.
Its consistent but modest gains bound that particular mechanism. The next
structural alternative should address typed payload access and the execution
state within the contextual expression operation. Adding another result cache
is not the working hypothesis. Flow's expensive semantic callbacks need their
own operation experiment before drawing conclusions from its cheap warm graph
walk.

Neither alternative is promoted to the normal compiler path. No end-to-end or
PGO-enabled speedup is claimed. The extracted production algorithms passed 196
AST unit tests, 125 checker unit tests plus the added common-entry regression,
the two flow-scope compile-fail doctests, and 7 focused compiler variants (63
sub-tests). The runner has 14 focused tests. Crate/example clippy and tracker
validation pass. Full compiler and PGO timing remain required before promotion.

## Typed fields and explicit contextual execution

The next experiment separates two structural changes, both over the same live
contextual operation. None adds a semantic cache or suppresses checker work.

- `typed` keeps the retained-owner scope and opts checked node reads into direct
  payload-shape selection for declaration names and the shape-based
  `modifiers`, `body` and `function_fields` getters. These reads select a typed row without constructing `NodeDataRead`.
  Expression, initializer and type-child access already used typed selectors;
  this experiment does not claim to introduce those. Factory and lazy payloads
  keep the existing path. Owner, slot and escaped-edge checks remain in place.
- `state` keeps the retained-owner scope and executes binary and parenthesized
  expressions through explicit continuations, using the same machine as the
  wasm checker. One checker-owned frame vector holds suspended binary/parenthesized parents;
  the active expression stays local. Recursive semantic callbacks operate above
  their own base index. Expression
  modes, current-node state, instantiation resets, postchecks and trace scope
  order remain per expression. Native stack growth remains at machine-entry
  boundaries because semantic callbacks can still recurse.
- `combined` runs both changes together. Components are measured for attribution;
  their separately measured savings are never added to predict this result.

The saved control is the preceding experiment executable, with backend `scoped`.
That isolates the new work from the previously measured owner-routing change.
The candidate executable also runs with `scoped` against that saved control:
compiling experimental fields can affect layout and optimization even when their
selection is disabled. Observation equality is required before every batch.
Timed processes start together, and each pair finishes before its output is
parsed or written. Source edits and builds stop during timed batches.

The runner accepts independent executable paths and backend names:

```sh
python3 tools/phase7/bench/operations.py run \
  --binary "$CANDIDATE_BINARY" --build-manifest "$CANDIDATE_BUILD" \
  --baseline-binary "$BASELINE_BINARY" --baseline-build-manifest "$BASELINE_BUILD" \
  --baseline-backend scoped --candidate-backend combined \
  --project "$PROJECT_CONFIG" --experiment argument --pairs 5 \
  --output "$EXTERNAL_ARCHIVE/combined"
```

`setup_ns` and `semantic_ns` remain separate from the outermost contextual
operation timer. The semantic interval contains the operation's shared timer
bookkeeping; it is useful pipeline attribution, not an uninstrumented compiler
acceptance result. A promising candidate still needs normal-build integration,
full parity and a production timing comparison before promotion.

### First implementation and representation correction

The first implementation preserved the observed work on both projects, but its
three concurrent pairs per row were slower than the saved scoped control:

| Backend in new executable | TypeScript ratio | VS Code ratio |
| --- | ---: | ---: |
| Scoped, candidates disabled | 1.0374 | 1.0298 |
| Typed payloads | 1.0408 | 1.0322 |
| Explicit state | 1.0465 | 1.0404 |
| Combined | 1.0461 | 1.0367 |

These are operation-time ratios. The combined differences were **28.44 ms** and
**433.86 ms** slower respectively. The disabled control regression prevents
attributing that whole difference to the candidate mechanisms. The original
executable, source hashes and raw results remain under the external
`2026-10-15/contextual-typed-state/` archive, in `candidate-build-v2/` and
`{typescript,vscode}-{scoped,typed,state,combined}/`.
`first-batch-results.json` also records whole-semantic times and pair ranges.

A layout probe found that the experimental bool had enlarged `NodeRead` from
40 to 48 bytes on this host, even when disabled. Moving it inside the existing
core-record variants restores 40 bytes; `layout.json` records the probe and
source identities. This proves the size change, not its CPU contribution.
The first continuation machine also stored every active scalar/worker frame in
the vector. The revision stores only suspended parents and retains local state
for the active expression; scalar and nested `typeof`-only calls now require no
continuation capacity. Both revisions preserve the checked ownership path.

The follow-up includes same-executable scoped controls, as well as comparison
with the immutable previous executable. No separately measured deltas are
subtracted to infer a mechanism's benefit.

### Revised screen and decision

The revised release executable is archived in `candidate-build-v3/`, SHA-256
`5cba602f6cfee1efb4ac2527db739016b85995943baa6a2a6c16bc1a4a23031d`.
It uses the same compiler, release profile and no-PGO policy as the saved
control. Source hashes agree before and after building. Results are in
`v3-{typescript,vscode}-{control,typed,state,combined,combined-saved}/`;
`final-results.json` summarizes them. All paths in this section are within
`../ts-rust-benchmarks/2026-10-15/contextual-typed-state/`, relative to the
repository root. No capture depends on `target/`.

Each table cell below is the median of three concurrent fresh-process pairs.
The **same executable's scoped backend** is the baseline for the first table:

| Candidate | TypeScript operation ratio | Paired time saved | VS Code operation ratio | Paired time saved |
| --- | ---: | ---: | ---: | ---: |
| Typed fields | 1.0003 | −0.16 ms | 1.0000 | −0.08 ms |
| Execution state | 1.0047 | −2.78 ms | 1.0065 | −71.11 ms |
| Combined | 1.0040 | −2.41 ms | 1.0069 | −75.68 ms |

Whole-semantic ratios for these same pairs are 0.9994 / 1.0002 for typed
fields, 1.0010 / 1.0027 for execution state, and 1.0001 / 1.0027 combined
(TypeScript / VS Code). These are instrumented semantic intervals, not normal
compiler throughput. Typed selection is effectively neutral. Explicit state
and the combination show no CPU improvement. The revised frame vector retains
16 frames / 1,920 bytes on each workload; this is experiment-owned capacity,
not a whole-program memory result.

Separate concurrent comparisons retain the **immutable previous executable**
as the baseline:

| Revised backend | TypeScript operation ratio | Paired time saved | VS Code operation ratio | Paired time saved |
| --- | ---: | ---: | ---: | ---: |
| Scoped, candidates disabled | 1.0328 | −19.01 ms | 1.0239 | −259.85 ms |
| Combined | 1.0388 | −22.42 ms | 1.0302 | −331.00 ms |

The combined whole-semantic ratios against that saved control are 1.0323 and
1.0252, with paired losses of 58.39 and 731.42 ms. Packing the bool restores
the read size but does not remove the common build/runtime-selection overhead.
No timing claim attributes that overhead to one cause. We do not subtract the
control row from another run or add component deltas.

Every observed pair matches the ordered operation trace, input/options hashes,
diagnostics, type/signature/instantiation counts and state-cleanup checks. Every
timed sample matches the observed work. Calls remain 112,672 / 1,630,385, with
87,345 / 1,021,623 outermost intervals. Tiny fixture screens also agree but are
not performance evidence. Binaries are hashed before and after each batch,
launch order alternates, and both timed children finish before report work. No
builds, source edits or other test runs overlap a timed batch.

**Decision: keep these as opt-in experiments; promote neither to native
production checking.** This typed candidate removes an intermediate payload
enum from four getter families; it still constructs `NodeRead`, resolves
payload pages and checks encoded child references. This execution candidate
flattens binary and parenthesized continuations; other semantic callbacks retain
recursive worker entry and stack guards. The results reject these specific
implementations as speedups, not every possible typed reader or execution
model. Further work should change the remaining representation/operation cost,
not retune these neutral selectors.

The checker-owned frame storage remains feature-only. The existing wasm path
shares the refactored continuation machine with its local frame storage; its
error restoration, trace order and deep callbacks are exercised by native
recursion-probe tests, and the wasm checker target compiles. Focused compiler
parity passes 7 variants / 63 sub-tests. All 199 AST and 136 checker unit tests
pass with both experiment and recursion-probe features. The 22 runner tests,
clippy, formatting, generator and tracker validation also pass. Full-corpus
wasm execution, production timings and PGO timings were not rerun for these
non-promoted experiments.
