# F5b coverage audit

The original 44-operation compiler placement review was resolved against the existing Phase 1 plan.
It does not waive any behavior or change a comparison denominator. Of the 44
previously ambiguous operations, 43 have exact later destinations (two in
Phase 3, 24 in Phase 4 and 17 in Phase 5); one Program wrapper is unused at the
pin. Every original operation ID remains in `coverage-review.json` and the
operation inventory. No new owner decision is required for these existing
emit, watch/CLI and editor boundaries.

The detailed machine record carries the Go file hash and per-operation source
and consumer evidence. The destination is the first applicable planned
consumer, rather than a guess based on a method name. In particular, the
loader's free `getModeForTypeReferenceDirectiveInFile` is not the same
operation as the Program method with that name. `sourcemap`'s `LineCount`
receiver is not Program. Initial loading of JSX runtime and helper imports is
also distinct from the helpers that reject a program-reuse shortcut.

The compiler review below does not certify every later-phase assignment in the
full inventory. The follow-up review identified template evidence outside this
43-operation table; the follow-up project-reference audit now retains 23 ordinary loader/configuration operations as pending Phase 1 work. See [the destination audit](PHASE1-F5b-destinations.md). The
existing plan supplies phase boundaries, not automatic approval of an
unexamined per-operation classification. See [the review disposition](PHASE1-F5b-review.md).

## Compiler destinations

All symbols below are in `compiler/program.go`, except the explicitly named
host and include-processor rows. Full IDs and source hashes are in the JSON.

| Operation | Phase | Reason and consumer |
| --- | ---: | --- |
| `host.go: NewCachedFSCompilerHost` | 4 | The cached host constructor is used by CLI execution and project-reference build orchestration; the underlying cachedvfs contract remains required in Phase 1. |
| `includeprocessor.go: updateFileIncludeProcessor` | 4 | This clones include-diagnostic state only while reusing a program during a watch edit. |
| `GetDiagnosticsOfAnyProgram` | 3 | This combined semantic/declaration/emit-gating workflow first becomes required by emission; the separate parse/bind/syntactic APIs remain Phase 1. |
| `Program.CommandLine` | 3 | This config accessor serves the emit reference lookup before the later CLI/project/API callers; config parsing itself remains Phase 1. |
| `Program.DeepImportPackageNames` | 5 | Deep-import package enumeration belongs to editor auto-import discovery. |
| `Program.DuplicateSourceFiles` | 5 | Duplicate-source ownership reporting feeds project snapshots and their reference-counted caches. |
| `Program.ExplainFiles` | 4 | The --explainFiles report is CLI execution, not option parsing or syntax diagnostics. |
| `Program.FilesByPath` | 4 | Exposing the entire program path map is first consumed by native watch execution. |
| `Program.GetDefaultLibFile` | 4 | This LibFile metadata accessor is consumed by incremental build-info serialization. |
| `Program.GetIncludeReasons` | 4 | This report accessor exists for CLI/watch test instrumentation. |
| `Program.GetLibFileFromReference` | 5 | Resolving an existing lib-reference directive for navigation belongs to language services. |
| `Program.GetResolvedTypeReferenceDirectiveFromTypeReferenceDirective` | 5 | This source-level directive lookup is an editor navigation/import-tracking service. |
| `Program.GetResolvedTypeReferenceDirectives` | 4 | Exporting the complete per-file type-resolution cache serves incremental snapshots. |
| `Program.GetUnresolvedImports` | 5 | The lazy unresolved-import set drives automatic type acquisition in the project system. |
| `Program.HasSameFileNames` | 5 | Cross-program filename equality is a project update decision. |
| `Program.HasTSFile` | 5 | The memoized implementation-TS-file presence query serves workspace symbol behavior. |
| `Program.IdentifierCount` | 4 | Program-wide identifier totals feed CLI diagnostic statistics. |
| `Program.InstantiationCount` | 4 | Aggregating checker instantiation totals serves CLI statistics; checker instantiation semantics remain Phase 2. |
| `Program.IsGlobalTypingsFile` | 5 | Classifying files inside the automatic-typings cache is an auto-import discovery query. |
| `Program.IsLibFile` | 5 | The lib-map membership accessor is used for workspace symbol selection. |
| `Program.IsMissingPath` | 4 | This missing-path report query is used by the CLI/watch test system. |
| `Program.LineCount` | 4 | The complete-program line total is a CLI statistic. |
| `Program.PackageJsonCacheEntries` | 4 | Iterating all program package cache entries serves incremental dependency snapshots. |
| `Program.ResolvedPackageNames` | 5 | Resolved package-name enumeration belongs to auto-import discovery. |
| `Program.ReuseProgram` | 4 | Reusing the loaded program after a single-file change belongs to watch execution. |
| `Program.SymbolCount` | 4 | The total of source and checker symbol counts is a CLI statistic. |
| `Program.Tracing` | 4 | The tracing-state accessor serves the incremental CLI. |
| `Program.TypeCount` | 4 | Checker type-count aggregation is a CLI statistic. |
| `Program.UnresolvedPackageNames` | 5 | Unresolved package-name enumeration belongs to auto-import discovery. |
| `Program.UpdateProgram` | 5 | This reuse-or-rebuild orchestration entry point is a project-system update operation. |
| `Program.UsesUriStyleNodeCoreModules` | 5 | The program-wide node: style preference is consumed by language-service import helpers. |
| `Program.canReplaceFileInProgram` | 4 | The replacement admissibility predicate exists only for watch/program reuse. |
| `Program.collectPackageNames` | 5 | Collecting resolved/unresolved/deep-import package sets serves the three editor auto-import accessors. |
| `Program.extractUnresolvedImports` | 5 | This lazy ATA set builder is a project-system helper. |
| `Program.extractUnresolvedImportsFromSourceFile` | 5 | Filtering per-file unresolved/non-TS external resolutions feeds ATA. |
| `Program.getModeForTypeReferenceDirectiveInFile` | 5 | This completed-program directive-mode accessor supports editor lookup. |
| `Program.jsxRuntimeImportSpecifier` | 4 | This helper rejects the single-file reuse shortcut when JSX synthetic imports would need recomputation. |
| `Program.needsImportHelpersImportSpecifier` | 4 | This helper rejects the single-file reuse shortcut when helper-import bookkeeping would change. |
| `equalCheckJSDirectives` | 4 | Comparing old/new @ts-check state belongs to the watch reuse admissibility predicate. |
| `equalFileReferences` | 4 | Reference-directive equality serves the watch reuse admissibility predicate. |
| `equalModuleAugmentationNames` | 4 | Old/new augmentation-name equality serves the watch reuse admissibility predicate. |
| `equalModuleSpecifiers` | 4 | Old/new import-node equality serves the watch reuse admissibility predicate. |
| `lazyValue.tryReuse` | 4 | Copying an initialized lazy cache from a previous Program is a watch reuse operation. |

`Program.ResolveModuleName` is the one unused operation. Its definition at
`compiler/program.go:2274` delegates to the resolver, but no call or interface
requirement refers to this Program wrapper at the pin. Other occurrences use
`module.Resolver`, a separate `emitHost`, or test mocks. Both `checker.Program`
and `modulespecifiers.ModuleSpecifierGenerationHost` were checked. This is an
exact unused-at-pin classification, not a claim that module resolution is
unused or complete: the underlying resolver retains its Phase 1 cases.

## What the pending operation count actually means

Removing the 44 placement questions leaves the F5a report's 2,805 attribution
entries: 2,490 missing exact witnesses and 315 name-inferred implementation
questions. Those entries do not mean that 2,805 production functions are
missing. They also cannot be closed by marking the full corpus as their
witness without proving which operation ran and what observation checks it.
The following counts describe the F5a starting point; the regenerated report
is authoritative after new witnesses land.

The generated AST file accounts for 1,338 of those entries:

| Generated operation family | Pending IDs | Required observation |
| --- | ---: | --- |
| Kind predicates | 232 | Each predicate over every declared kind, plus invalid raw kinds. |
| `As*` payload accessors | 192 | Correct payload identity and incompatible payload behavior, independently of kind where the pin does that. |
| `Clone` | 191 | New identity, shallow edge identity, flags/range and hook order. |
| `ForEachChild` | 167 | Ordered node/list visits, nil handling and early termination. |
| `VisitEachChild` | 166 | Unchanged identity and one-child replacement; role/list/raw-slice contracts. |
| `Update*` | 164 | Same-input identity and a changed input, including slice backing and empty/nil rules. |
| `New*` | 147 | Kind, supplied fields, masked flags, counters and create-hook order. |
| `Name` | 41 | The actual name field, including absent names. |
| `computeSubtreeFacts` | 38 | Child-specific fact propagation and exclusion rules. |

## Executable witness approach

Use the existing `data/s03/schema/ast.json` (192 shapes),
`data/s06/generated-ast-scope.json` and the pinned Go inventory to generate
**test dispatch**, not expected answers. Keep this in the Phase 1 syntax
harness so it uses the current capture, provenance and exact-operation rules.
The generator must check that every claimed operation belongs to the pinned
inventory and has an emitted dispatch arm. Unsupported schema members stop
generation; they do not silently omit a shape.

1. Generate requests with exact operation IDs and stable action names. For
   each shape, use distinct sentinel children and lists for its fields so
   swapped/missing edges are observable; include nil and allocated-empty
   lists, flags and nondefault locations. Text observations carry bytes.
2. Generate a Go overlay in package `ast` that calls the actual pinned
   constructors, typed accessors, clone/update/visitor methods and private
   facts methods. Package-local access is needed for `computeSubtreeFacts`;
   copying its algorithm into the probe would not be independent truth.
3. Generate Rust dispatch through `FactoryMethods`, `VisitorMethods`,
   `NodeRead` and typed payload methods. Reuse the hook and identity visitor
   structure in `crates/tsr_ast/tests/generated_runtime.rs` and the node
   ordinal convention from S06 factory traces. Do not call an alternate
   hand-coded algorithm in the driver.
4. Observe exact values and identity relationships, including both unchanged
   and changed update results. Field-distinct children make order visible.
   Exercise early termination separately; one complete traversal cannot prove
   the short-circuit behavior. Keep Go kind dispatch distinct from payload
   dispatch, as the existing mismatch fixture requires.
5. Capture native and Rust outputs, compare them through the shared adapter,
   and record only the operation/action pairs actually executed. A matching
   constructor row must not automatically credit clone, visitor or update.
   Retain currently missing IDs until their own dispatch exists and runs.

A traversal of a parsed fixture alone can cover only the kinds and operations
that fixture invokes. It cannot prove update identity, clone hooks or unused
synthetic/JSDoc shapes. Conversely, emitting all shapes is tractable because
member types, factory flags and child/visit roles are already in the schema;
it does not require 1,338 handwritten implementations. The first vertical
slice should use `QualifiedName`, a list-backed shape, and the dynamic
`JSDocParameterOrPropertyTag` visitor before generating the entire matrix.
The emitter's `handWritten` and `handWrittenVisitor` distinctions must remain
explicit. `SourceFile` and `SyntheticExpression` cannot be waved through a
universal generated constructor assumption.

## Handwritten helper attribution

The remaining parser, binder, scanner and handwritten AST rows need a separate
pass. The largest groups are 488 parser/JSDoc/reparser helper witnesses, 181
binder/helper entries and 142 scanner/regexp/utility witnesses. First map
actual port markers and mechanisms, then match existing precise observations.
For example, `Parser.checkJSDecoratorSyntax` has a real home in
`tsr_parser/src/js_syntax.rs`; the option/comment/decorator syntax cases can be
reviewed for that specific call. `AccessorDeclarationBase.IsAccessorDeclaration`
is an interface-marker method in Go, not evidence of a missing Rust parser
feature. Its equivalent shape contract needs a reviewed Rust mechanism and a
corresponding shape witness.

For a private helper whose result is only visible through a public parse/bind
operation, use a narrow fixture with an audited necessary call path and an
observable discriminator. When the call path is conditional or ambiguous,
record call hits from a scoped instrumented correctness run; attach the exact
request that hit it. Hits prove execution only, so record the relevant output
field or expected diagnostic alongside them. Shared memory layout, a method
name, a file-level parity metric or a passing corpus alone is insufficient.

This work does not justify new ports purely to satisfy name matching, nor a
new general trace/replay framework. Reuse existing explicit native probes and
small access-only overlays. New Rust-contract-only tests must be labeled as
such rather than being presented as Go differential observations.

## First executable generated-AST increment

`tools/phase1/syntax/ast-generated/` implements the three-shape vertical slice
and all 232 generated predicates. Its input generator derives the exact
production dispatch names and fails if a pinned predicate lacks a Rust call or
its signature becomes unsupported. It never generates expected results. Five
focused script regressions check dispatch completeness, request/action links,
source drift and failure rather than silent omission.

The 26 shape scenarios distinguish nil/empty/list entries, nonzero flags and
locations, identity-preserving updates, clones and visitor order. A distinct
AwaitExpression(ThisKeyword) sentinel discriminates subtree propagation from
the Identifier bit. The predicate rows call both real implementations on all
kinds and invalid raw-kind boundaries. Capture and comparison results, rather
than the existence of this fixture, decide which exact operation links become
current evidence. This increment does not claim the other generated shapes.

## Expanded generated-AST comparison

The vertical slice was expanded through actual per-shape dispatch, rather
than granting other shapes its results. The fixture now contains 1,026 rows:
232 predicates, the original 26 dynamic traces, 760 traces for 190 generated
shapes, and eight explicit SourceFile/SyntheticExpression cases. They execute
1,380 distinct operation identities. The 760 shape traces produce 7,788 named
action results: constructors and typed payload reads, each update field
changed independently, clone hooks and identity, child enumeration and early
exit, visitor replacement/order, names and generated facts where defined.

A development native capture and Rust run agree on every observation
(1,026/1,026), with no panic catcher or error-as-success normalization. The
only adapter correction during development was replacing Rust's nil
`NodeSlice::empty()` with the existing allocated-empty constructor to match
Go's nonnil `[]*Node{}`. The original mismatching capture is retained under
`target/phase1-f5b-generated-ast-development-comparison.json`. This is a
harness correction, not a product change or an approved divergence.

SourceFile's special case compares filename, canonical path, external-module
parse options, loaded source text, statements and EOF identity through the
real production owner. SyntheticExpression's special case calls the real
payload cast and child visitor, using a nil native semantic Type. It does
**not** cover the four New/Update/Clone/Visit operations carrying Go's opaque
`Type any`; Rust's syntax record has no such field and the S06 scope already
records those four as deferred. Their disposition needs the separate owner
review, not an inferred exemption from this generator.

These generated cases were subsequently captured and recorded as part of the
complete 1,122-row syntax-utility tranche in `019ec1c`; the initial `not_run`
state is no longer the history of that tranche. The reviewed 2026-09-23 refresh
records them again in the complete **1,123/1,123** matching syntax-utility
capture, with request/result/claim bindings on the changed production and
harness sources. It does not inherit the historical capture's freshness.
Native answers and operation/action links remain separate. Generation checks fail for missing Rust dispatch,
unknown parameter categories, changed action membership or shape-inventory
drift. The six focused generator regressions and the driver's malformed
identity/action regression protect those boundaries. Existing accessor and
runtime witnesses retain their own precise links; these results do not grant
credit to unrelated handwritten AST, parser or binder helpers.

## Reviewed completion checkpoint: 2026-09-23

The final family captures at `target/phase1-f5b-review-20260923-03` have 229
matching leaves, 355 matching filesystem cases, 498 matching config cases and
1,123 matching syntax-utility cases. The five separately approved differences
remain visible as raw differences: one leaves case, three filesystem cases
and one config case. The Linux-only realpath observation remains
`native_unavailable` on this host. The historical pilot records two matches
and four not-implemented observations; it does not enter production metrics.

The full program-syntax capture at
`target/phase1-f5b-review-full-20260923/full` matches all **15,152** executable
native rows. The **908** matching smoke rows are a selection from that full
capture, not another independent corpus run. Full syntax parity is recorded
against its own validated inventory; it grants no automatic operation links.

The raw reviewed captures are preserved in
[`f5b-reviewed-families.tar.gz`](../data/phase1/captures/f5b-reviewed-families.tar.gz)
and
[`f5b-reviewed-syntax-full.tar.gz`](../data/phase1/captures/f5b-reviewed-syntax-full.tar.gz).
The script suite passed locally: **901 tests**, **1 skipped**, **1,425 subtests**.
Program-producer and integration-receipt refresh results are recorded
separately; this checkpoint does not claim those executions have finished or
that remote CI is green.

At `02009ce` the complete operation report contained **1,364 pending entries**
(the mutation-kill witnesses later reduced it to 582; see
[the mutation-witness record](PHASE1-mutation-witnesses.md)):

| Root cause | Pending entries | Meaning |
| --- | ---: | --- |
| Missing exact operation witness | 947 | No accepted request/output or executed Rust-contract link yet |
| Unverified implementation mapping | 302 | Name-based mapping is not proof that a production port is absent |
| Unresolved later-step transfer | 92 | Exemption from one preparation step did not remove Phase 1 ownership |
| Ordinary project-reference loader/configuration work | 23 | The accepted build-scheduling exclusion does not cover these operations |

The [destination audit](PHASE1-F5b-destinations.md) retains 205 later-phase
destinations under the accepted plan and brings the 23 ordinary loader
operations back into explicit pending work. P1A/P1B remain incomplete. These
counts preserve the coverage obligations; neither matching corpora nor a
passing grouped metric can discharge them without the missing evidence.
