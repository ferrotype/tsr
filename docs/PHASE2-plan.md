# Phase 2: complete the type checker

Status: **proposed for owner review**, 2026-09-24; amended 2026-09-25 with the
accepted findings of the [plan review](PHASE2-plan-review.md), including the
C0 review amendments. This is the high-level plan. Detailed checkpoint plans
follow its review; production implementation is a subsequent step.

Planning reference: merged Phase 1 PR #53, main
`02009ce1ec457ad6bd11c1e71afbe9243650680a`. Phase 1 closure is proceeding
separately. Upstream remains Corsa
`1f70213d4922b434345f639b441681e470c7cfc1`.

## 1. Outcome and starting point

Deliver the complete pinned checker behavior needed by compiler/conformance
diagnostics, type and symbol baselines, production type display, the emit
resolver, and checker services. Complete the compiler's parallel-checking and
cancellation contracts. Preserve the ownership and byte-string contracts already
implemented. This is [PLAN Phase 2](../PLAN.md#phase-2-type-checker-the-critical-path).

The starting point is a substantial working checker, not a new implementation.
[S08](S08.md) delivered the selected checker algorithms, type display and
declaration diagnostics, with exact results over 9,369 accepted variants.
Its libraries already required generic instantiation, inference and advanced
types. S09 added retained-result, generation and pool-lifetime mechanisms.
Phase 1 extended the foundations those implementations consume.

That evidence has a defined scope. S08 excluded source-level generics, JSX,
decorators, conditional, mapped, indexed-access, infer, template-literal and
import types from its test-source selection, plus 413 variants whose only
baseline is emitted output (a missing `.errors.txt` still asserts zero
diagnostics, so they keep an errors obligation) and the 15 content-mapper
variants. Conditional, mapped, indexed-access, infer, template-literal and
import types have Rust modules that are starting assets; JSX has none (58 Go
checker methods, no Rust function), and decorators have two grammar
predicates. Their presence never establishes coverage. The Phase 2 denominator
is observed from the Phase 1 syntax schedule (section 5), not inferred by
adding counts from the old reports.

Keep the production ID-based, single-threaded `&mut self` checker. The
alternative relater was measured and did not justify replacing it
([ADR 0008](adr/0008-checker-mutation-model.md)). Its extension is not on the
critical path. Type-allocation counts are not general semantic acceptance
criteria; preserve the existing explicitly scoped relater contracts and test
observable ordering under [ADR 0010](adr/0010-order-sensitive-outputs-are-enumerated--comparators-are-port.md).

## 2. Scope and phase boundaries

| Area | Phase 2 responsibility | Boundary |
| --- | --- | --- |
| Type system | Symbols and merges, declared/inferred types, all five relation modes, variance, instantiation, inference, contextual typing, flow and narrowing | Extend the current checker and shared foundation ports |
| Language semantics | Classes/interfaces/enums/namespaces, modules and aliases, JS/JSDoc, JSX, decorators, grammar and option-dependent checking | Follow pinned Corsa, including its documented differences from TypeScript |
| Display and queries | Type/symbol queries, production type-to-node/string conversion, accessibility, diagnostic chains and related information | Formatting uses production code; baseline envelopes remain declared test code |
| Downstream checker contracts | Emit resolver and `services.go`/`symbolaccessibility.go` operations | Phase 3 consumes the resolver; Phase 5 implements editor features using the services |
| Execution | Compiler checker pool and partitioning, serial/parallel checking, cancellation and checker trace production | Phase 4 owns CLI/build/watch orchestration and native trace-file/process services; Phase 5 owns editor scheduling and transports |
| Lifecycle | Shared bound files, independent checker state, retained results, lazy JSDoc, retirement and cancellation behavior | Extend the existing owners; real server/client integration remains with its phase |

There are three boundaries to settle explicitly in the detailed preparation plan:

**Declaration diagnostics.** The native `.errors.txt` is the post-emit
diagnostic set: config, program, syntactic, semantic and global diagnostics,
plus declaration diagnostics when declarations are emitted. Full emit runs
first. The pin's mismatch diagnostic compares only pre/post diagnostic counts;
its absence does not prove equal codes, spans, arguments or related information.
S08 compares the structured sets on its subset without executing JavaScript
emit in Rust. C0 carries that comparison over every executed variant. An emit
dependency can be narrowed only where those observations establish equality;
any difference remains visible and is attributed to the operation that causes it.

Declaration requests (1,459 native requests in S08's inventory; C0 counts the
full set) can require the declaration transform and the emit resolver over the
newly included families. Phase 2 owns checker/emit-resolver completion; Phase 3
owns the transform. Existing working paths retain their results. Only an
observed missing operation creates a named joint blocker for the affected
domain and variants; declaration options alone do not. Complete `.js`, `.map`
and `.d.ts` parity stays in Phase 3.

**Content mappers.** 15 executed variants use a content mapper, with 2
`.errors.txt`, 10 `.types` and 10 `.symbols` baselines computed over
mapper-transformed text. Phase 1 assigned content-mapper execution to Phase 5.
They are the second cross-phase dependency inside the Phase 2 gate and get the
same named-blocker treatment.

**Runner sub-tests.** The pinned runner runs nine sub-tests per configuration:
error, content mapper, output, sourcemap, sourcemap record, types and symbols,
module resolution, union ordering and source file parent pointers. Phase 2 owns
error, types and symbols, union ordering and parent pointers; output, sourcemap
and sourcemap record are Phase 3; content mapper is Phase 5. The module
resolution sub-test (148 `.trace.json` baselines from the loader's trace) has no
owner yet and gets one in C0. The parent-pointer check has no S08 equivalent.
After checking, traverse each source file's children and compare their parent
ids with the traversal parent, including synthesized descendants reached by the
child visitor. Exclude only default-library files, as Go does; user declaration
files remain included. The source-file root supplies the initial parent and is
not itself required to have a parent.

**Programs and project references.** Ordinary loading/configuration and
redirection prerequisites remain with the Phase 1 closure work. Phase 2 owns
their checker-facing semantics and queries; build scheduling remains Phase 4.
Use the [destination audit](PHASE1-F5b-destinations.md) to assign each operation
once. Do not duplicate or silently absorb Claude's active Phase 1 work.

## 3. Prerequisites and coordination

Planning and native-oracle preparation can proceed while Phase 1 closes.
Implementation may start per checkpoint when its needed AST, binder, resolver,
options and program contracts have reviewed implementations and usable tests.
A blanket Phase 1 completion flag is neither a substitute for that check nor a
reason to block unrelated preparation
([dependency rule](../PLAN.md#9-sequence-and-gates)).

Before each detailed implementation plan, record:

- The Phase 1 contracts it consumes, their owner, and any unresolved dependency.
- Existing S08 production paths to retain or extend, with confirmed gaps
  separated from missing mappings and missing tests.
- Its downstream consumers and any Phase 3/4/5 dependency needed for its exit.

Before introducing concurrent calls on one module resolver, resolve the
[F3b per-call-state follow-up](PHASE1-implementation-plan.md#follow-up-before-concurrent-resolution-on-one-resolver).
The current options-swap guard is a serial contract. Shared-cache synchronization,
redirect/mode separation and failure isolation must be designed together if that
call pattern is needed. Parallel checking is not permission to add a broad lock
around the current resolver or silently duplicate observable cache behavior.
This is not a C6 prerequisite: the Rust checker reads retained resolutions
through the program adapter, as Go's `Program.GetResolvedModule` does, and
never resolves during checking; the guard applies only if a Phase 2 consumer
resolves on demand.

Inherited from Phase 1, to record in the first detailed plan: the recorded
follow-up for generic qualified-name serialization (the fourth scoped
`typeParameterSymbolList`), and the six checker-facing project-reference
accessors now that ordinary reference loading is ported.

## 4. Proposed delivery order

Use **C0–C7** for Phase 2 checkpoints, avoiding S08's historical P0–P7 names.
The numbered list below is the default order. These are not another global
test-preparation phase followed by an implementation phase: after C0, each
checkpoint prepares its witnesses, implements its production gaps and verifies
the combined result. More detailed substeps are written only after this plan
is reviewed.

1. **C0 — Establish the full checker acceptance contract.** Inventory the
   complete pinned compiler/conformance configurations and their native
   execution/selection rules. Extend the existing S08 runner to cover the full
   domain, with exact requests, query schedules, diagnostic stages and baseline
   rendering authority. Obtain a first categorized Rust result and a concrete
   gap map. Exit: a reviewed denominator, working comparison and named blockers;
   passing Rust parity is not required at preparation exit. Adapter, protocol
   and request-identity failures block preparation; production failures remain
   measured gaps.

2. **C1 — Complete symbol, type and relation foundations.** Audit and complete
   symbol resolution/merging, declarations and member/signature construction,
   arrays/tuples/enums/literals, recursive type resolution, all relation modes,
   variance and comparison diagnostics. Retain what S08 already implements.
   Variance is measured by instantiating with marker types, so it completes
   with C2's instantiation, not before it. Exit: the assigned native contracts
   pass, including recursive, cold/repeated and failure paths, with relevant
   existing cases preserved.

3. **C2 — Complete inference and advanced type interactions.** Cover generic
   declarations and calls, constraints/defaults, overload selection, contextual
   typing, mappers/substitution, conditional and infer types, mapped/indexed/
   template-literal/import types, and depth/complexity limits. Test combinations
   and loaded-library use rather than accepting isolated syntax examples. The
   two residual id-ordered cases of ADR 0010, reverse mapped types without
   symbol or mapper and declaration-less same-named symbols, first appear at
   scale here. C2 implements the scoped creation-trace diagnostic mode in both
   binaries required by that accepted ADR and retains the comparator fixtures.
   Replacing the trace requirement requires a separate owner-approved amendment
   to ADR 0010; comparator or union-ordering results alone do not discharge it.
   Exit: assigned source-level families and their interactions match in errors,
   types, symbols and display, and the scoped trace mode has direct witnesses
   for both residual cases unless the accepted ADR has been amended.

4. **C3 — Complete flow and ordinary program semantics.** Finish narrowing,
   assignment/definite-assignment and reachability behavior, returns/generators/
   async, classes and inheritance, late-bound/computed members, namespaces,
   module/alias interactions, and JS/JSDoc/expando behavior. Include malformed
   inputs and option combinations. Exit: assigned program families pass with
   diagnostic order, spans, arguments, chains and related information intact.

5. **C4 — Complete JSX and decorators.** A port from scratch: `jsx.go`, the
   JSX paths in `checker.go` (attribute contextual typing, element type
   resolution, runtime and namespace import resolution per `jsx` mode) and the
   decorator checks, with contextual/inference interactions, runtime/helper
   resolution and diagnostics for the modes supported at the pin. Keep checking
   and transformation outputs distinct. Exit: these formerly excluded source
   families pass all required checker observations; a few isolated diagnostic
   patches do not close them.

6. **C5 — Complete display, checker services and emit-resolver contracts.**
   Cover the remaining public checker query surface, name accessibility,
   type-node serialization/reuse and its caches, service-specific queries, and
   checker-dependent declaration diagnostics. `services.go` starts from zero
   (66 functions, no Rust port, never called by the compiler corpus), so its
   half needs its own Go reference source; recording the checker calls the
   pinned fourslash suite makes and replaying them is the candidate. Add direct
   native cases where baseline walks do not exercise a required entry point.
   Exit: documented
   downstream contracts pass through real checker operations with correct
   owner retention and source context; required emit-diagnostic dependencies
   have an implemented path or remain explicit joint blockers.

7. **C6 — Complete checker execution and cancellation.** Integrate the pinned
   compiler partitioning/pool policy, including its FENNEL heuristic and
   constants, with the existing ownership primitives; distinguish it from the
   editor/API lifetime pool. Both `checkerpool.go` and `tracer.go` start from
   scratch, and the tracer needs a trace-sink seam (ADR 0014) rather than the
   Phase 4 tracing package. Cancellation follows the pin: the checker polls per
   statement and per deferred node, a canceled checker stays poisoned, its
   diagnostics are nil and reuse panics, and the pool disposes that checker;
   panic retirement of a generation (ADR 0012) is a separate contract. Verify
   serial and multiple checker execution, deterministic outputs, context
   polling, cancellation, tracing and panic retirement through production
   entry points. Exit: the full comparison passes in both of upstream's modes,
   the single-threaded default and the concurrent-test-programs configuration
   that PLAN's Phase 4 gate names, which moves here with the `checkerpool.go`
   ledger entry; interrupted work preserves the pin's contract without
   publishing invalid state or reusing a retired generation. Go's file-to-
   checker assignment fuses the FENNEL scoring on arm64 (`FMSUBD`) and not on
   amd64; an ordinary Rust arithmetic expression does not reproduce that
   contraction. Direct assignment witnesses compare against pinned Go on the
   same GOOS/GOARCH and record the toolchain and arithmetic behavior, preserving
   ADR 0009's partitioning contract. A normalized cross-architecture assignment
   policy would require a separate owner-approved ADR amendment. Output parity
   in both execution modes is checked independently; baseline independence from
   assignment is not assumed.

8. **C7 — Close full correctness and report readiness.** Resolve the remaining
   categorized failures, run complete acceptance on final relevant inputs,
   validate ordering and parent-pointer subtests and lifecycle/recursion checks,
   and publish the operation and dependency disposition. Exit: every required
   Phase 2 gate passes; any retained difference has its exact owner-approved
   scope. Report what Phase 3/4/5 can now consume and the measured performance
   risks that Phase 7 still owns.

C3 can develop ordinary cases after C1, while generic-dependent cases require
C2. C5 contract preparation should start with C0; individual service/resolver
slices can land as their type-system dependencies become ready, so Phase 3 need
not wait for all JSX/decorator work. C6's design also starts early, but concurrent
execution is validated against a working serial implementation. C7 requires the
completed results of all applicable checkpoints and their foundation contracts.
A variant belongs to the latest checkpoint whose features it needs: about two
hundred of the newly included variants combine C2 type features with JSX or
decorators and are C4's, whatever their other families. This assigns completion
ownership, not a result: every domain retains its observed match, difference or
named failure, including working paths in families whose implementation is
incomplete.

## 5. Acceptance and progress reporting

Create a distinct Phase 2 inventory and evidence namespace: sprints `P2A` and
`P2B`, data under `data/phase2/`, and new scripts over the existing manifests
in the Phase 1 pattern. Preserve S08's frozen E2 inventory and recorded evidence
as history. Extensions to shared runners or production sources, including new
files under fingerprinted directories, can stale E2 under its existing rules.
Phase 2 supplies the current 9,369-variant regression result through its own
capture; C0 does not re-record E2 merely to refresh its fingerprint. Broadening
Phase 2 must not silently redefine the earlier metrics or their freshness rules.

The denominator is the Phase 1 syntax schedule's 13,434 variants the pinned
runner executes, out of 15,206: S08's 9,369 acceptance variants, 4,050 newly
included, and the 15 content-mapper variants. The 1,772 the runner skips (1,720
by its unsupported-option guard, 52 by file name) have no reference baseline of
any kind and use options Corsa does not implement at the pin; C0 lists them
explicitly, as the S12 matrix requires, and they stay informational. C0 starts
from `data/phase1/syntax-schedule.json` and the S07 loading requests rather
than re-inventorying.

The proposed authority is the pinned compiler runner and production Go APIs,
with access-only adapters and the original native baseline writers where
possible. The full inventory records configuration variants, actual file/library
inputs, native skips/refusals, requested diagnostic stages and disabled baseline
domains. Ordered configuration maps and duplicate-file precedence are part of
input identity. C0 reviews native guard/skip classifications for Phase 2: the
S08 informational exceptions are scoped decisions, not automatic exclusions
from full-checker acceptance.

Report separately:

- Exact `.errors.txt`, `.types` and `.symbols` results, plus public display and
  checker-service contracts that baseline files do not completely observe.
- Match, different, failed, unsupported and unexecuted outcomes for every
  required domain; explicit native-disabled domains keep their native reason.
- Root-cause buckets and representative reproductions, with counts by feature
  and configuration. A previously differing row can still regress internally:
  inspect changed observations, not just match-to-difference transitions.
- Confirmed implementation gaps, missing behavioral witnesses, mapping-only
  gaps, approved differences and later-phase dependencies as separate work.

One scenario may witness several operations when its execution and assertions
support those exact links. Port-marker counts, name searches and broad corpus
pass rates cannot establish that link by themselves. Conversely, a Rust field
or shared helper can implement several Go methods without needing duplicate
production functions or one artificial fixture per trivial accessor. For
checker internals the corpus and the runner sub-tests are the evidence; Phase
1's per-operation audit is not repeated over the 2,932 functions of the phase-2
ledger files. Use direct tests for important public, lazy, cache, failure and
ownership contracts that the native corpus leaves unobserved. Section 5's
root-cause buckets by feature and configuration are the per-area dashboard
PLAN's Phase 2 tooling names.

Reuse the existing strict-protocol, provenance, immutable-capture and replay
machinery. Verify request/response identity, complete schedules and matching
work before grading. Preparation, coverage and production parity remain
different claims. Unsupported required work remains a failure, even when other
outputs happen to match. Update dependent inventories, review receipts and
generated views together, and check their consistency before expensive runs.

## 6. Cost control and performance risk

The difficult work is semantic interaction and coverage, not creating new
crates. C0 establishes the remaining size; the old Go line count or unmapped
method count is not an implementation estimate.

Use focused native regressions and previously matching controls for each
change, then complete affected families at checkpoint boundaries. Run the full
corpus at C0 for attribution and, by default, at every checkpoint exit, since
each exit requires existing cases preserved and the S08 set does not cover the
newly included variants; the cost measured at C0 can argue that down. Validate
manifests and executable identities before long runs. Reuse captures only when
their ordinary validators accept them.

The accepted Phase 0 measurements already show checker CPU and retained-memory
risk ([ADR 0020](adr/0020-phase-0-gate-decision.md)). Preserve phase timers and
allocation/retention attribution. Use bounded representative measurements to
detect architectural regressions while extending the checker, and record one
comparable owner capture at the C2 exit, without a threshold. The S08 figures
are different measures from the Phase 7 gates: 2.125 is elapsed time and 1.413
retained bytes, while the native gate is peak RSS at 0.70; they are reported
side by side, never converted into each other.
Performance investigation must name the mechanism and the decision it informs;
avoid broad trace/replay infrastructure or a storage rewrite without that need.

Phase 2 is a correctness gate. This proposal introduces no speed or memory
threshold, and does not repeat S07/S08 benchmarks merely because a fingerprint
changes. Full native/portable/embedding budgets and four-week acceptance remain
with [Phase 7](S12-acceptance.md). Report unfavorable results without treating
the smaller S08 footprint ratio as whole-checker memory or performance proof.

## 7. Review decisions and next planning step

Please review these proposed choices before detailed plans:

1. **Scope and authority:** complete the checking the pinned runner executes
   (13,434 variants), retaining S08 as regression evidence; the runner's skips
   are listed explicitly and stay informational, and C0 confirms the
   checker-level applicability of each executed variant.
2. **Sequence:** C0 first, then C1–C7 in the order above, with preparation and
   production paired per checkpoint and early preparation of downstream APIs.
3. **Emit boundary:** checker/emit-resolver work belongs here; required additional
   diagnostic-producing transforms are explicit Phase 3 dependencies, and a
   missing dependency keeps the full error gate open.
4. **Execution boundary:** complete compiler checker partitioning, cancellation
   and trace production here, resolving PLAN's overlap between its Phase 2 and
   Phase 4 scopes: the `checkerpool.go` ledger entry and the two-mode
   configuration of the Phase 4 gate move to Phase 2; CLI/watch, editor and
   transport ownership stay in their phases.
5. **Acceptance and cost:** full semantic parity plus exact consequential
   operation/lifecycle witnesses; bounded measurements during development,
   without a new architecture experiment or release-performance gate.

After review, write the detailed **C0 plan first**. Its resulting inventory and
first full comparison will refine the C1–C6 work boundaries. Each subsequent
implementation plan must identify existing code versus actual gaps, pinned
operations and native counterexamples, ownership/failure and ordinary-path
costs, dependency owners, incremental delivery, executable exit checks and
evidence reuse rules. Write the exact producer commands and sprint/metric wiring
there, after their contracts are settled. Keep unresolved scope choices visible
rather than encoding them as silent harness behavior.
