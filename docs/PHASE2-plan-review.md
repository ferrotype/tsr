# Phase 2 plan review

Reviewed 2026-09-24 by Claude for the owner, against merged `main`
`02009ce1ec457ad6bd11c1e71afbe9243650680a` and pinned Go source
`1f70213d4922b434345f639b441681e470c7cfc1`. This reviews the proposed
[Phase 2 plan](PHASE2-plan.md) independently of its author. The plan is not
modified by this record; each finding names the section or decision it applies
to. No evidence, threshold, ledger entry, divergence or scope is changed here,
and nothing below claims that any Phase 2 checkpoint has started.

Amendment note (2026-09-25, Codex): the emit inference, parent-walk definition
and assignment-independence claim below are corrected following the C0 review.
The final section records all resulting plan amendments; it does not change
the accepted ADRs or record a new owner-approved divergence.

## Inputs checked

- [PLAN.md](../PLAN.md) sections 6, [9](../PLAN.md#9-sequence-and-gates), 10
  and 13; ADRs [0008](adr/0008-checker-mutation-model.md),
  [0009](adr/0009-concurrency-model-kept-from-corsa.md),
  [0010](adr/0010-order-sensitive-outputs-are-enumerated--comparators-are-port.md),
  [0014](adr/0014-host-seams-as-traits.md),
  [0020](adr/0020-phase-0-gate-decision.md) and
  [0022](adr/0022-checker-type-footprint-threshold.md); the
  [S12 acceptance matrix](S12-acceptance.md).
- The S08 [completion record](S08.md), [implementation plan](S08-implementation-plan.md),
  [acceptance amendment](S08-acceptance-amendment.md), [E2 protocol](S08-E2.md)
  and [P3 record](S08-P3.md); the [S09-1](S09-1.md), [S09-2](S09-2.md),
  [S09-4](S09-4.md) and [S09-5](S09-5.md) records.
- The Phase 1 [implementation plan](PHASE1-implementation-plan.md)
  ([section 2](PHASE1-implementation-plan.md#2-scope-and-boundaries), the
  [F3b resolver follow-up](PHASE1-implementation-plan.md#follow-up-before-concurrent-resolution-on-one-resolver)
  and the recorded Phase 2 follow-up), the [progress record](PHASE1-progress.md)
  ([F4a request accounting](PHASE1-progress.md#request-accounting-task-1) and
  [syntax schedule](PHASE1-progress.md#the-syntax-schedule-and-its-native-capture-tasks-2-and-3)),
  the [F5b review](PHASE1-F5b-review.md) and the
  [destination audit](PHASE1-F5b-destinations.md); [`sprints/README.md`](../sprints/README.md),
  `P1A.toml` and `P1B.toml`; the phase assignments in [`PORTS.toml`](../PORTS.toml).
- Pinned Go: `testrunner/compiler_runner.go` (sub-tests and skip list),
  `testutil/harnessutil/harnessutil.go` (compile path and option guard),
  `compiler/checkerpool.go`, `checker/tracer.go`, `checker/jsx.go` and the
  checker's module-resolution calls.
- Rust: the `tsr_checker` modules for JSX, decorators, conditional, mapped,
  indexed-access, template-literal, infer and import types, variance and
  instantiation; the checker host and the compiler's retained-resolution
  adapter; the compiler's diagnostic selection path.
- Committed baselines under `testdata/baselines/reference`: 7,301 `.errors.txt`,
  12,751 `.types` and 12,751 `.symbols` files.

Every link and anchor in the plan resolves.

## Confirmed

- Section 1's account of S08 is exact: 9,369 acceptance variants, exclusion of
  the nine syntax families from test sources only, generic semantics required
  through the 111 loaded libraries, and the relater comparison recorded in
  ADR 0008. S09-1, S09-2 and S09-4 are the retained-result, generation and pool
  mechanisms the plan names.
- The FENNEL heuristic and constants C6 cites are real: `checkerpool.go:50-105`,
  with the checker count clamped at line 313. ADR 0009 requires the same
  constants.
- Section 2's declaration-diagnostics path matches the harness compile path;
  the scope of missing emit work must be established by structured pre/post
  observations and missing operations, as corrected in finding 3. The
  destination audit leaves 23 ordinary reference-loading operations with
  Phase 1 and gives Phase 2 six checker-facing ones.
- Section 6 introduces no speed or memory threshold, consistent with PLAN's
  Phase 2 gate and ADR 0020's Phase 7 budgets. Letting S08's measurements go
  stale follows the ADR 0022 contract and the phase-end green-up practice.
- Sections 5 and 6 carry PLAN's Phase 2 gate unchanged: 100 percent of the
  three baselines with an approved allow-list.

## Findings

Ordered by effect on the C0 plan. "Amend" states the recommended change.

### 1. The denominator and selection authority are decidable now

Applies to section 4 (C0), section 5 and decision 1.

Phase 1 F4a already enumerated every compiler/conformance variant with its
native selection outcome
([syntax schedule](PHASE1-progress.md#the-syntax-schedule-and-its-native-capture-tasks-2-and-3),
[`data/phase1/syntax-schedule.json`](../data/phase1/syntax-schedule.json)).
C0's "inventory the complete pinned compiler/conformance configurations and
their native execution/selection rules" repeats that work.

| Inventory | Variants |
| --- | ---: |
| Full F4a schedule, one row per declared variant | 15,206 |
| Native selection: runs / option-guard skips / filename skips | 13,434 / 1,720 / 52 |
| S08 E2 inventory (9,369 acceptance, 1,359 informational) | 10,728 |
| Union of the nine excluded syntax families | 4,478 |

The option guard (`harnessutil.go:1236-1263`) skips UMD and System modules,
Node10 and Classic resolution, the ES5 target, a nonempty `baseUrl`, and
explicit `esModuleInterop`, `allowSyntheticDefaultImports` or `alwaysStrict`
false; the fatal AMD and `outFile` guards run first. These are options Corsa
does not implement at the pin, so their variants cannot be "complete pinned
checker behavior"; only Strada-era baselines exist for them. The S12 matrix
already says what to do with them: list native test-selection guards
explicitly.

Amend: state the default in the plan instead of deferring it to C0. Acceptance
rows are the variants the pinned runner executes. Option-guard, filename and
rejected-option skips are listed explicitly and stay informational, with Go
results collected past the guard where the harness can produce them, as the
S07-3 amendment allows. C0 then confirms the checker-level denominator (for
example the `NoTypesAndSymbols` cases) rather than deciding authority.
Decision 1 as written is a deferral, not a decision.

### 2. JSX has no starting asset and decorators almost none

Applies to section 1 and C4.

Section 1 says existing implementations of the excluded features are starting
assets. That holds for conditional, mapped, indexed-access, template-literal,
infer and import types: `conditional.rs`, `mapped.rs`, `indexes.rs`,
`template.rs`, the `infer_*.rs` modules and `import_types.rs` exist in
`tsr_checker`. It does not hold for the other two families.

| Feature | Pinned Go checker methods | Rust today |
| --- | ---: | --- |
| JSX (`checker/jsx.go`, 1,488 lines) | 58 | no function or port marker names JSX |
| Decorators (`*Decorator*` methods across the checker) | 32 | two grammar predicates, `has_grammar_decorator` and `has_decorators` |

Amend: correct the sentence, and size C4 as a from-scratch port of `jsx.go`,
the JSX paths in `checker.go` (attribute contextual typing, element type
resolution, runtime and namespace import resolution per `jsx` mode) and the
decorator checks, not as gap filling. The counts are mapping signals, not
parity claims.

### 3. Determine the emit joint blocker from structured observations

Applies to section 2 and decision 3.

In the pinned harness (`harnessutil.go:661-687`) the error baseline is the
post-emit diagnostic set: config-file, program, syntactic, semantic and global
diagnostics, plus `GetDeclarationDiagnostics` only when `GetEmitDeclarations()`
is true, plus suggestion diagnostics only when captured. `postProgram.Emit`
runs first, and a pre/post count difference appends an ad hoc diagnostic
(`harnessutil.go:698`). None of the 7,301 committed `.errors.txt` baselines
contains that diagnostic. This proves no recorded count mismatch, not equal
diagnostic sets: the pin compares only their lengths. The original conclusion
that JavaScript emit contributes nothing observable was too strong. S08's E2
already compares structured pre/post sets on its subset without executing
JavaScript emit in Rust and ports the declaration diagnostics that subset needs.

Amend: carry the structured pre/post comparison across every executed Phase 2
variant, retaining codes, spans, arguments, chains and related information.
Narrow a variant's emit dependency only where the observations establish
equality; otherwise attribute the difference to its operation and owner. S08
counted 1,459 native declaration-diagnostic requests among its 10,728 variants;
C0 counts the full set. Requests and declaration options do not themselves
establish missing work. Keep working paths and record a joint blocker only for
the affected domains with an observed missing transform or emit-resolver path.

### 4. Decision 4 moves work that PLAN and the ledger assign to Phase 4

Applies to section 2, C6 and decision 4.

`PORTS.toml` assigns `tsc/internal/compiler/checkerpool.go` to phase 4 in
`tsr_compiler`, and the
[Phase 4 scope](../PLAN.md#phase-4-programs-command-line-build-and-watch) lists
"checker pool" while the
[Phase 2 scope](../PLAN.md#phase-2-type-checker-the-critical-path) lists "the
checker pool, parallel checking, cancellation and `--generateTrace`". The plan
resolves the overlap toward Phase 2 without saying that PLAN and the ledger
disagree. Separately, `checker/tracer.go` is phase 2 but depends on
`internal/tracing`, which is phase 4 in `tsr_tracing`.

Amend: decision 4 should record that it resolves the PLAN overlap and moves the
`checkerpool.go` ledger entry to phase 2, so the generated phase tables stay
consistent with the plan. C6 should name a trace-sink seam (ADR 0014) so that
checker trace production does not pull the Phase 4 package forward.

### 5. Two PLAN Phase 2 tooling items are dropped silently

Applies to section 5 and C7.

PLAN's Phase 2 tooling names a per-area pass-rate dashboard and "the
creation-trace diff from decision 5 for the residual id-sensitive cases";
ADR 0010 says to keep a creation-trace mode in both binaries, scoped to those
cases. No creation-trace mode exists in `crates/`, `scripts/` or `xtask/`.
S08 P3 covered residual identity with direct comparator fixtures on the subset
(eight residual families), and the S08 plan treats creation traces as opt-in
diagnostics for a named mismatch rather than scheduled work.

Amend: either schedule the trace mode (C1 for the mechanism, C7 for the corpus
sweep) or record, with owner sign-off, that comparator fixtures plus the union
ordering sub-test replace it for the full corpus, and amend ADR 0010's wording
to match. Section 5's root-cause buckets by feature and configuration cover the
dashboard in substance; say so, so the PLAN item is visibly carried rather than
missing.

### 6. Enumerate and assign all nine runner sub-tests

Applies to the section 2 table, C0 and C7.

`compiler_runner.go:204-212` runs, per configuration: error, content mapper,
output, sourcemap, sourcemap record, types and symbols, module resolution,
union ordering, and source file parent pointers. The plan assigns error, types
and symbols (Phase 2) and output and sourcemaps (Phase 3), and names ordering
and parent pointers in C7. Content mapper is a Phase 5 boundary in the Phase 1
roster. Module resolution, the `traceResolution` baseline built from the
loader's trace, is assigned to no phase in the plan or in the Phase 1 records
checked here. The parent-pointer sub-test has no S08 E2 equivalent; with an
index-based AST and generated accessors, C7 must define what is verified
(start at each source file's children, excluding only default-library files;
check every reached descendant's parent id, including synthesized descendants,
without requiring a parent on the source-file root) rather than name the Go
sub-test. User declaration files remain included.

Amend: C0 lists all nine sub-tests with an owner each; C7 defines the Rust
parent-pointer check.

### Smaller items

- **Intermediate full runs (section 6).** C1 to C6 exits each require existing
  cases preserved, and the S08 E2 set does not cover the 4,478 newly included
  variants. Make a full run at each checkpoint exit the default, and let the
  cost measured at C0 argue it down.
- **Shared-runner staleness (C0, section 5).** `scripts/s08_e2.py` hashes
  `scripts/s08_e2*.py`, `tools/s08/oracle/**/*` and the S07 manifests as E2
  inputs, and inherits all Rust sources through `s08_p4.py`. Adding files under
  those globs changes the closure too. C0 keeps E2 as historical evidence under
  unchanged freshness rules and derives current regression results through the
  Phase 2 capture; it does not re-record E2 solely to refresh a fingerprint.
- **Inherited Phase 1 items (section 3).** The F5b review lists 1,365 pending
  entries. Two touch Phase 2 directly: the 23 ordinary project-reference loader
  operations (the Rust loader still rejects nonempty reference configurations,
  `tsr_compiler/src/checker_diagnostics.rs:25-26`; the audit gives Phase 2 six
  checker-facing reference operations; one corpus case file carries a tsconfig
  `references` list), and the recorded Phase 2 follow-up for generic
  qualified-name serialization's fourth scoped `typeParameterSymbolList`
  ([Phase 1 plan](PHASE1-implementation-plan.md#ordered-storage-and-json-integration-decision--2026-09-21)).
  Name both.
- **Resolver concurrency is not a C6 blocker (section 3).** The Rust checker
  reads retained resolutions through the program adapter
  (`tsr_checker/src/host.rs:69`, `tsr_checker/src/external_resolution.rs:234`,
  `tsr_compiler/src/checker_host.rs:204`), as Go's `Program.GetResolvedModule`
  does, and never calls the resolver during checking. Keep the guard, but say
  it applies only if a Phase 2 consumer resolves on demand.
- **C1/C2 boundary (section 4).** Variance is measured by instantiating with
  marker types, and `relater_variance.rs` and `instantiate.rs` already exist.
  The detailed C1 plan should not treat variance as finishable before
  instantiation.
- **Namespace (section 5).** Phase 1 used `P1A`/`P1B` and `data/phase1/`.
  Naming `P2A`/`P2B`, `data/phase2/` and the producer metric prefix in the plan
  saves C0 from inventing them.

## Disposition of the review decisions

| Decision | Disposition | Condition |
| --- | --- | --- |
| 1. Scope and authority | Approve with the default stated | Finding 1 |
| 2. Sequence | Approve as written | Note the C1/C2 variance boundary |
| 3. Emit boundary | Approve with observed dependencies | Require structured pre/post comparison and operation-level blockers per corrected finding 3 |
| 4. Execution boundary | Approve with the ledger consequence recorded | Finding 4 |
| 5. Acceptance and cost | Approve as written | Default to checkpoint-exit full runs; carry the PLAN tooling items per finding 5 |

## Review conclusion

The plan is consistent with PLAN.md, the accepted ADRs, the S08 and S09 records,
the S12 matrix and the Phase 1 handoffs. Proceed to the detailed C0 plan after
the amendments above; findings 1, 3, 5 and 6 and the namespace item are its
direct inputs. This review does not satisfy C0's reviewed denominator, does not
change the E2 record, and is not evidence that any Phase 2 checkpoint has
begun.

## Second round (2026-09-25)

A second review pass (Opus) and a check of it (Codex) added findings that this
review confirmed against the pin and the repository data. The plan was amended
the same day; the findings above stand.

Confirmed and folded into the plan: the runner-executed denominator (13,434 of
15,206; the 1,772 skips have no baseline of any kind) and the two exclusion
rules the section 1 list missed (`emitted_output_only` 413, with a zero-
diagnostic obligation, and the 15 content-mapper variants); content mappers as
a second cross-phase dependency (Phase 5; 2, 10 and 10 baselines); upstream's
two execution modes as C6's exit (the single-threaded default and the
concurrent-test-programs job) and their move from PLAN's Phase 4 gate; the arm64
fusion of the FENNEL scoring (`FMSUBD` on arm64, `MULSD`/`SUBSD` on amd64,
verified by compiling the pinned package for both targets), which affects
scoring and can change assignments; this does not establish that baselines are
independent of assignment.

Also confirmed: the pin's cancellation contract (per-statement and
per-deferred-node polling, a poisoned checker whose reuse panics, disposal by
the pool) as distinct from generation retirement; `services.go` (66 functions,
no Rust port) needing its own reference source; the 2,932 functions of the
phase-2 ledger files as an inventory rather than an effort measure; and a
bounded owner capture at the C2 exit, with elapsed time, retained bytes and
peak RSS kept as different measures.

Corrected: the variants needing both C2 type features and JSX or decorators are
189, 251 or 184 depending on the definition (the assignment rule stands, the
number does not); both residual id-ordered cases of ADR 0010 remain, since
`compareSymbolsWorker` still falls back to symbol ids for declaration-less
same-named symbols; the 148 `.trace.json` baselines of the module-resolution
sub-test get an owner in C0.

## C0 review amendments (2026-09-25)

Codex reviewed the three uncommitted plans against the pinned runner and the
existing evidence scripts. The owner requested these amendments; C0–C7 keep
their order. They are plan changes, not implementation or new capture results.

1. C0 now requires `harness_valid`: malformed responses, wrong request identity
   and unresolved adapter failures block preparation. Production failures and
   named unsupported operations remain measured gaps. Targeted controls and
   negative protocol tests must distinguish the two.
2. Family tags assign completion owners, not unsupported outcomes. Blockers
   name an observed missing operation and its affected domains; working JSX,
   decorator and declaration-diagnostic paths retain their results.
3. E2 remains historical under its existing fingerprints. Adding oracle or Rust
   files can stale it. Phase 2's capture supplies current regression evidence.
4. The count-only native pre/post check cannot prove equal diagnostic sets.
   Finding 3 and both plans now require structured comparison before narrowing
   any emit dependency.
5. C6 compares assignment witnesses with Go on the same GOOS/GOARCH and records
   toolchain/arithmetic behavior. Output parity in both execution modes is a
   separate check. Normalizing assignments across architectures would require
   an owner-approved ADR amendment.
6. C2 retains ADR 0010's scoped creation-trace mode in both binaries and tests
   both residual cases. Replacing it requires an explicit owner-approved ADR
   amendment; comparator fixtures alone do not discharge it.
7. The parent walk starts at source-file children and excludes only default
   libraries, preserving user declaration files and reached synthesized nodes.
8. The 39 rejected-option rows are labeled as a subset of the 52 filename skips,
   not an additional denominator category.

The accepted ADRs, recorded evidence and thresholds are unchanged. No corpus,
benchmark or producer run was needed for these documentation amendments.
