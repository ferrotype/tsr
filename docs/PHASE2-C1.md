# C1 review record

The review of PR #61 found production defects and gaps in the claimed exit
evidence. These corrections are carried on `phase2-c2-plan`, above C1. The
historical C1 capture is preserved; a new capture, not a reinterpretation of
the old result, establishes the C2 starting point.

## Production corrections

- A diagnostic for an imported, circular type-parameter constraint now walks
  each ancestor through that node's owning file. The old second walk used the
  constraint file's view for a caller node and returned an ownership error.
- Excess-property target filtering now recursively tests constituent types
  for `NonPrimitive`, matching `maybeTypeOfKind`. A nested intersection can
  contain `object` without its immediate flag being `NonPrimitive`.
- The generic-mapped-target comparison distinguishes a skipped branch from a
  failed attempted comparison. It resets the diagnostic chain only at the
  pin's non-generic-source branch. Targets with `-?` and generic mapped
  sources retain the earlier chain.
- Package deduplication publishes metadata only for retained source files.
  Previously metadata for a discarded package dependency reached
  `GetSymlinkCache`, which requires a program file for every metadata key.
  This caused `TypeToTypeNode(useBar)` to fail in
  `packageDeduplicationDuplicateGlobals`. Redirect spellings still resolve to
  the retained source and its metadata.

`c1_review_regressions` compares the imported-constraint and nested-excess
cases with native behavior, and checks exact diagnostic chains through the
mapped-target branches. The mapped cases cover the routes but are not claimed
as mutation witnesses for every previous chain-reset error.
`c1_package_metadata` exercises module-specifier generation, duplicate package
dependencies, realpath aliases and retained metadata through public APIs.

## Executed recursion contracts

The former deep fixture compared a type with itself and returned before
structural recursion. The replacement has two distinct, finite 140-level
types, with different leaves beyond the native depth-100 backstop. Pinned Go
accepts the assignment without diagnostics. A test-only `recursion-probe`
feature observes the actual recursive relation: both debug and release must
reach depth 100 and execute on a grown stack segment from a 256 KiB thread.
The observer is separate from `relation-probe` and is absent from benchmarks.

The panic contract now injects a panic inside that recursive relation, then
checks retirement and a successful fresh checker. The excessive-instantiation
fixture still checks TS2589 and subsequent checker use. These are named
witnesses, not a claim that every complexity-limit branch is covered; C2.10
and C2.11 retain the instantiation-count, conditional-loop and cross-product
limit obligations.

Receipt v2 requires both exact Cargo commands, all seven named tests with no
ignored or filtered cases, full passing output and the production dependency,
bundled-asset and generator closure. A successful empty or filtered command
cannot certify the contract.

## Audit boundaries and handoffs

The audit binds the exact reviewed function groups and pin, including complete
`relater.go` and `types.go` inventories. Removing functions or supplying an
empty audit cannot make it complete. Port markers were added for the five
implemented project-reference accessor equivalents. This does not implement
their remaining callers: referenced-module format in
`canHaveSyntheticDefault`, TS6305 for missing reference output, and
cross-project rewrite checks in `resolveExternalModuleNameWorker` remain C3
module-resolution work.

The four occurrences of the alias-refusal strings are two producers and two
catches. `resolve_name_mode` resolves the pending alias and retries;
`lookup_symbol_resolving` does the same for its lookup. That establishes the
wrapper behavior, not universal non-escape from every raw `lookup_symbol`
caller. Raw lookup sites in iteration, JSDoc, flow and ordinary expression
checking remain part of C3's call-path audit.

Some reported literal/enum/tuple refusals guard invalid internal shapes:
`widen_literal_type` handles `ENUM_LIKE` before ordinary freshable literals;
the sole computed-enum constructor sets `ENUM`, so enum display dispatch
precedes the apparent unsupported arm; template conversion admits only its
literal flags. Tuple labels pass `parameter_label_at`'s identifier-only
selection before the binding-pattern guard, as in the pin's
`isValidDeclarationForTupleLabel`. These constructor/dispatch facts, rather
than absence from the corpus, delimit those guards. The storage-pilot-only
fresh-literal helper is not a production missing operation.

`declarationEmitAugmentationUsesCorrectSourceFile` remains a C5 failure. The
captured panic hook suppresses Rust backtraces, so classification by its
`tsr_ast::factory` label was insufficient. A single replay under LLDB stopped
at `rust_begin_unwind` and confirmed `declaration_diagnostics_with_checker`
through `module_declaration` to `late_statements` at `statements.rs:113`, then
the factory's `WrongOwner` panic. Visibility processing can late-mark a
declaration from the augmentation target's `.d.ts`, while the transformer
initially retains only its emitted `.ts` source. C5 must retain the appropriate
source before processing those late declarations. The selected stack is
saved in `data/phase2/c1-review/augmentation-stack.txt`; the claim records the
actual declaration-transform path rather than assuming a node-builder defect.
Reproduce with the captured executable, its `cases/01553/request.json`, and
a separate output path under LLDB (`breakpoint set -r rust_begin_unwind`,
`run`, `bt 30`, `continue`).

Variance algorithms are mapped, but C1 output parity alone does not witness
their measurement state. `c1-audit.json` therefore records the separate
`c2-variance-measurement` obligation: marker types, variance cycles/restarts,
and `Unmeasurable`/`Unreliable` propagation over seven named cases. C2 must
observe those states; a mapped function is not evidence that this obligation
has passed.

## Evidence integrity

- The acceptance comparison excludes optional previous-run history when
  deciding whether the result is recorded. The history remains in the saved
  report, and producer replay is read-only.
- Baseline creation authenticates requests, binaries and raw observations,
  recomputes every domain, and requires the complete ordered unique row
  inventory. Historical source staleness is allowed for a baseline, never for
  current acceptance.
- Candidate claims stay open. A failed row needs a traced return to another
  checkpoint or a covering registered blocker; an unfamiliar panic module
  does not establish ownership.
- `thislessFunctionsNotContextSensitive1` is claimed as fixed, rather than
  returned to C3 with the obsolete package-metadata attribution. Its iteration
  fast path accepts the first three arguments of the interface, as Go does.

No thresholds or divergence approvals change. C1/C2 completion is reported
only by their current validators; later-checkpoint obligations remain visible.

## Reviewed-source validation

`target/phase2/c1-reviewed` observed all 13,432 variants with stable sources
and no harness errors. All 9,367 S08 regression rows match in every enabled
domain. The package-deduplication row now matches; full-domain matches rise
from 12,458 to 12,459. No previously matching domain regresses, and no row
that remains different changes its recorded observation. The one production
panic is the traced C5 late-declaration failure above.

The seven recursion/ownership contracts pass in debug and release and have
a current v2 receipt. The three review regressions and package-metadata test
pass. Targeted compiler/checker clippy with warnings denied passes; 39 C1
evidence tests, 10 comparison tests and 14 selection tests pass. The exact
C1 function audit passes. This validation does not claim a benchmark result
or completion of later C2–C7 work.
