# Phase 2 C1: symbol, type and relation foundations

Checkpoint C1 of the [Phase 2 plan](PHASE2-plan.md), written after the
[C0 record](PHASE2-C0.md) (PR #58, merged into `main` at `df83531`). Upstream
is Corsa `1f70213d4922b434345f639b441681e470c7cfc1`. Unlike C0, C1 changes
checker semantics: it is production work in `crates/tsr_checker`, paired with
its witnesses, and it is the first checkpoint whose exit is measured by the C0
contract rather than by preparation.

## 1. Objective and exit

Audit and complete the checker's foundations: symbol resolution and merging,
declarations, member and signature construction, arrays, tuples, enums and
literals, recursive type resolution and its limits, the five relation modes,
variance state and comparison diagnostics. Retain everything S08 implements.

C1 exits when all of the following hold on one full run at the C1 head,
recorded by the owner through `cargo xtask run checker`:

| Exit condition | Measured by |
| --- | --- |
| Existing cases preserved: the S08 regression subset still matches completely | `run.checker.regression_parity == 1` (9,367 of 9,367) |
| Every other case C0 already matched is preserved too: no previously matching domain of any executed row becomes a non-match | `run.checker.c1_regressions == 0` against the authenticated C1-start row report `data/phase2/c1-baseline.json.gz` |
| Every C1-attributed row of section 3 matches in every domain | `run.checker.c1_open == 0` over `data/phase2/c1-claims.json` |
| No production panic, stack overflow or unnamed error in a foundation module | `run.checker.c1_failures == 0` |
| The foundation function groups of section 4 are audited: each pinned function is mapped, disposed as equivalent, or handed to a named later checkpoint; none is an unexplained gap | `run.checker.c1_audit_complete == true` over `data/phase2/c1-audit.json` |
| The direct contracts of C1.8 (recursive, cold/repeated and failure paths) pass in debug and release | `run.checker.c1_contracts == true`, from the recorded test receipt |
| Variance measurement that needs C2's instantiation is handed over by name, not silently deferred | the C2 handoff list in the C1 record |

`run.checker.c1_complete` is the conjunction. It binds `P2B-C1` in
`sprints/P2B.toml`. No ratio threshold is introduced: C1 either closes its
attributed rows or names what stays open and why.

## 2. Starting point

Everything below exists and is consumed as is. C1 extends it; it re-derives
nothing.

| Asset | Where | What C1 takes from it |
| --- | --- | --- |
| The S08 checker | `crates/tsr_checker/src`, 237 modules; relations in `relater*.rs`, `variance.rs`, `compare.rs`; merges in `merge.rs`; members and signatures in `members.rs`, `signatures.rs`, `construct.rs`; tuples and arrays in `arrays.rs`, `relater_tuples.rs`, `infer_tuples.rs`; enums and literals in `enums.rs`, `enum_eval.rs`; the lazy-resolution guard in `resolution.rs` (ADR 0008) | the production code to retain and complete |
| C0 contract and gap map | `data/phase2/inventory.json` (13,432 executed), `data/phase2/first-comparison.json`, `data/phase2/blockers.json`, `scripts/phase2_{inventory,native,corpus,compare,blockers,producers}.py` | the denominator, the buckets, the run and report commands, the producer to extend |
| C0 result | 12,308 of 13,432 variants match in every domain; regression 9,367 of 9,367; open rows: C2 342, C3 15, C4 767; 6 production failures | the rows C1 attributes to itself (section 3) |
| Ledger and function inventory | `PORTS.toml`, `data/go-functions.tsv`, `status/unmapped-functions.json` | the per-file mapped/unmapped state of section 4 |
| Relation contract | `data/s08/relater-fixtures.json`: 21 fixtures, five modes, first, repeated, reversed and self comparisons with native ternaries and before/after state; the production relater against the isolated prototype at 105 of 105 | the relation-mode witnesses C1 keeps at parity |
| Ownership schedules | `scripts/s08_ownership.py` over `p2_checker`: single checker, repeated forward/reverse queries, separate checkers, concurrent first construction behind a barrier; `run.e3.independent_checker_merges` | the cold/repeated and merge-isolation paths C1.8 reuses |
| E2 recursion checks | `[e2]` in `status/runs.toml`: native 512 KiB deep fixtures, debug/release recursion and reentry tests | the recursion witnesses C1.5 extends |
| Direct checker tests | 44 tests in `tsr_checker` (`tests.rs`, `flag_tests.rs`, `node_builder_cache_tests.rs`, `families_tests.rs`); 9 sub-test tests in `tsr_compiler/tests/phase2_subtests.rs` | the test homes C1.8 adds to |

The C0 Rust capture is historical (the build fingerprint changed after it was
taken; `source_stable: false`). Its observations are still the gap map, so the
attribution of section 3 starts from it and is confirmed by C1.0's fresh run.

## 3. What C1 owns

The inventory's checkpoint rule assigns every executed variant to `regression`,
C2, C3 or C4 by family; it assigns none to C1. C1 is therefore a completion
owner of *causes*, not of rows: it claims the open rows whose observed cause is
a foundation defect, whichever checkpoint owns the row. Row ownership in the
inventory does not change; `data/phase2/c1-claims.json` records the claim, the
bucket, the C0 evidence and, after C1.1, the reproduction that confirmed it.

Candidate claims from C0, all in C2-owned rows (`first-comparison.json`
buckets):

| Bucket | Variants | Representative | Why it is a foundation cause |
| --- | ---: | --- | --- |
| `diagnostics: TS2322` | 73 | `typeParameterHasSelfAsConstraint` | assignability outcome or elaboration; the representative is a self-referential constraint |
| `different: types` | 24 | `constraintErrors1` | declared or resolved types differ, first seen on constraint errors |
| `failed: checker_error` | 13 | `interfaceMergeWithNonGenericTypeArguments` | the checker stops with `missing failed signatures` while merging an interface with non-generic type arguments, so its diagnostics never complete |
| `diagnostics: TS2416`, `TS2430` | 9, 9 | `mismatchedGenericArguments1`, `mappedTypesAndObjects` | member and base compatibility: relation over resolved members |
| `diagnostics: TS2365` | 5 | `comparisonOperatorWithNumberOperand` | comparability relation |
| `panic: tsr_checker::relater_tuples` | 3 | `lambdaParameterWithTupleArgsHasCorrectAssignability` | `relater_tuples.rs:130`, target position past the element list |
| `diagnostics: TS2313`, `TS2314` | 2, 2 | `circularBaseTypes`, `typeAliasDeclarationEmit` | circular base resolution; type-argument count of a declared alias |
| `diagnostics: TS2636` | 2 | `varianceAnnotationValidation` | variance annotation check |
| `diagnostics: TS2741` | 1 | `classExtendingAbstractClassWithMemberCalledTheSameAsItsOwnTypeParam` | missing-property relation error |
| `different: display` | 1 | `spuriousCircularityOnTypeImport` | circularity in resolution shows in display |
| `failed: stack overflow` | 1 | `infiniteConstraints2` | unbounded constraint recursion (ADR 0011) |
| `failed: walker_error` | 1 | `packageDeduplicationDuplicateGlobals` | the walk fails with `lazy roots must belong to the current transaction` over duplicated globals: lazy-storage ownership in symbol merging across deduplicated packages |
| `panic: tsr_checker::infer_tuples` | 1 | `sliceTupleTypeOutOfBounds` | `infer_tuples.rs:65`, subtracting the target's fixed prefix and suffix from a shorter source's length underflows; inference is C2's, tuple arity handling is C1's |

That is 147 rows as an upper bound. Three more buckets stay with C2 unless a
reproduction shows a foundation cause: `TS2344` (10, type-argument constraint
satisfaction), `TS2339` (9, generic indexed member access) and `TS2345` (9,
call arguments). The `panic: tsr_ast::factory` row
(`declarationEmitAugmentationUsesCorrectSourceFile`) is a node-builder retention
defect and stays with its row owner for C5. The blockers B01–B18 are C2's,
C4's, C5's and Phase 5's; C1 introduces none and removes none.

A claim is confirmed only by reproduction in C1.1: the row's smallest program,
run through `phase2_corpus.py run --case`, with the differing observation traced
to a foundation function. A bucket whose reproduction lands in inference,
instantiation or a type-level operator goes back to C2 with the trace attached.

## 4. Work items

Each item names what exists, what to build, its artifact and its executable
exit check. Foundation function groups are named by their pinned Go names
(`tsc/internal/checker`), so `port:` markers and the ledger stay the mapping
authority; the counts are the current `status/unmapped-functions.json` state.

### C1.0 Refresh the gap map at the C1 head

- Exists: the C0 scripts and the historical capture.
- Build: `phase2_compare.py baseline`, which writes
  `data/phase2/c1-baseline.json.gz`: every executed row's outcome and digest
  per domain, bound to the native observation digest, the inventory digest and
  the Rust capture identity it came from. It is the authority for
  `c1_regressions` (C1.10); a row report edited by hand no longer matches the
  capture it names. Run the full contract once at the branch point:
  `phase2_native.py verify` (unchanged native contract, 52 s),
  `phase2_corpus.py run` (about 5 minutes), `phase2_compare.py report
  --previous target/phase2/rust/comparison.json` (the C0 row report; the
  committed `first-comparison.json` is a summary without rows), then
  `baseline`. The owner records `cargo xtask run checker` on the designated
  host; that recording is also what C0 still owes.
- Done on 2026-09-25 at the plan's branch point: 13,432 rows, 0 harness
  errors, 12,308 matching in every domain, regression 9,367 of 9,367, the same
  74 buckets with the same counts as C0 and 0 changed observations; the
  capture is `target/phase2/c1-base`, source-stable at that head.
- Exit: `harness_valid`, 0 harness errors; `changed_observations` against the
  C0 comparison reviewed and every change explained by a commit since
  `bf2013e`; the baseline file committed.

### C1.1 Foundation audit and attribution

- Exists: 1,005 of 1,505 `checker.go` functions mapped, 109 of 189 in
  `relater.go`, 8 of 111 in `types.go`, 55 of 150 in `utilities.go`; the
  unmapped `relater.go` list includes `Relater.isRelatedTo`, `isRelatedToWorker`,
  `isRelatedToSimple`, the recursion-identity helpers, the error-state and chain
  helpers, `getUnmatchedProperties` and `getVarianceStackIndex`, although the
  Rust relater implements relations end to end. The ledger's `rust` lists for
  `relater.go`, `mapper.go`, `exports.go` and `inference.go` are empty
  (`status = planned`) while modules exist. Both are mapping gaps to separate
  from real gaps, as section 3 of the plan requires.
- Build: `scripts/phase2_audit.py` producing `data/phase2/c1-audit.json`: for
  each function in the C1 groups (section 4 lists them under C1.2–C1.7), one
  disposition with evidence: `mapped` (marker present), `equivalent` (a Rust
  field, accessor or shared helper implements it; the Rust site named; the
  disposition that Phase 1 accepted for accessors, proposed here for the
  `types.go` accessor methods), `missing_mapping` (implemented, marker to add
  in this item), `gap` (not implemented; the C1 item that closes it), or
  `later` (named checkpoint, with the reason). `check` requires every C1-group
  function to carry one disposition and no `gap` at exit. Markers are added to
  the Rust sites so `STATUS.md`'s function metrics move with the audit, and
  the ledger `rust` lists and statuses are regenerated from markers, not typed.
  Also in this item: `data/phase2/c1-claims.json` from the table of section 3,
  each row with its reproduction command and the function the trace named.
- Exit: `phase2_audit.py check` passes on the C1 groups; every candidate row
  of section 3 is either claimed with a confirmed cause or returned to C2 with
  the trace.

### C1.2 Symbol resolution, merging and declarations

- Exists: `name_resolution.rs`, `name_scopes.rs`, `merge.rs` (checker-owned
  transient merge records over immutable source symbols, ADR 0007),
  `module_aliases.rs`, `external_aliases.rs`, `declaration_checks.rs`; the four
  merge schedules of `s08_ownership.py` pass.
- Build: the `checker_error` bucket (`missing failed signatures` on an
  interface merge with non-generic type arguments) and the `walker_error` row
  (`lazy roots must belong to the current transaction` over duplicated globals
  of deduplicated packages); the unmapped merge and lookup functions
  `combineSymbolTables`, `recordMergedSymbol`, `addInheritedMembers`,
  `resolveClassOrInterfaceMembers`, `getGlobalSymbol`,
  `getClassOrInterfaceDeclarationsOfSymbol`, `getSymbolOfNode`,
  `getResolvedSymbolOrNil`, `getDeclaredTypeOfAlias`,
  `tryGetDeclaredTypeOfSymbol`, `getTypeOfFuncClassEnumModule`,
  `resolveAliasWithDeprecationCheck`, `resolveIndirectionAlias` and the
  export/import target lookups (`getTargetOfExportAssignment`,
  `getTargetOfExportSpecifier`, `getTargetOfImportSpecifier`), each disposed
  by C1.1; the four `getSymbolFlags: alias resolution` refusal sites in
  `name_resolution.rs`, which the corpus does not reach and a direct native
  case must.
- Exit: the two claimed buckets match; the merge schedules still pass; the
  direct alias-resolution case has a native observation.

### C1.3 Members, signatures and index infos

- Exists: `members.rs`, `signatures.rs` (arena-numbered signatures, index infos,
  predicates), `compound_signatures.rs`, `source_signatures.rs`,
  `signature_parameters.rs`, `indexes.rs`, `late_members.rs`.
- Build: the `TS2416` and `TS2430` buckets (member and base compatibility over
  resolved members); the unmapped construction functions
  `resolveTypeReferenceMembers`, `createInstantiatedSymbolTable`,
  `instantiateSymbolTable`, `instantiateSymbols`, `instantiateSignatures`,
  `instantiateIndexInfos`, `getSignaturesOfType`,
  `getSignaturesOfStructuredType`, `createUnionSignature`,
  `getIntersectedSignatures`, `createCanonicalSignature`,
  `getCanonicalSignature`, `getRestTypeOfSignature`,
  `getIndexInfosOfStructuredType`, `getApplicableIndexInfo` and its `ForName`
  and plural forms, `findIndexInfo`, `getIndexTypeOfType`, `isSimpleTupleType`
  and `expandSignatureParametersWithTupleMembers`. Instantiation of a signature
  table is a C1 construction primitive even though its callers are C2's:
  the audit records that split per function.
- Exit: the two buckets match; `hasSignatures`, `typeHasCallOrConstructSignatures`
  and the index-info lookups have direct cases where the corpus leaves them
  unobserved.

### C1.4 Arrays, tuples, enums and literals

- Exists: `arrays.rs` (global array identities, Go's absent-global sentinel),
  `relater_tuples.rs`, `infer_tuples.rs`, `enums.rs` and `enum_eval.rs` (an
  explicit failed state for value computation), literal interning in
  `construct.rs` and `types.rs`.
- Build: the two tuple panics (`relater_tuples.rs:130` indexes
  `target_infos[target_position]` past a two-element target;
  `infer_tuples.rs:65` underflows when a source tuple is shorter than the
  target's fixed prefix plus suffix); the unmapped tuple functions `getNormalizedTupleType`,
  `getRestTypeOfTupleType`, `getRestArrayTypeOfTupleType`,
  `getArrayOrTupleTargetType`, `isGenericTupleType`, `isMutableTupleType`,
  `isSingleElementGenericTupleType`, `isVariadicTupleElement`,
  `isValidDeclarationForTupleLabel` and `getUniqAssociatedNamesFromTupleType`;
  enum and literal functions `getTypeFromLiteralTypeNode`,
  `parseBigIntLiteralType`, `getWidenedLiteralTypeForInitializer`,
  `getUniqueLiteralTypeForTypeParameter`, `isConstEnumSymbol` and
  `getBigIntLiteralValue`; and the unreached refusals `getWidenedLiteralType:
  enum`, `getTupleElementLabel: binding pattern`, `typeToTypeNode: computed
  enum` and `AnyToString: computed enum value`, each either implemented or
  reached by a direct native case that shows the refusal is outside the pinned
  corpus. `fresh non-string literal type` is not a production gap: its only
  site is `storage_pilot.rs:109`, the deliberately restricted S08 storage
  probe, and the audit classifies it as a harness boundary.
- Exit: the two panic buckets match; no refusal in these modules is unreached
  by a case.

### C1.5 Recursive type resolution and limits

- Exists: `resolution.rs` (the `pushTypeResolution` guard keyed on entity and
  property, cycle marking to the cycle start), reserved stacks and growth
  guards (ADR 0011), `constraints.rs`, E2's deep fixtures.
- Build: the `infiniteConstraints2` stack overflow, `TS2313`, `TS2314`, the
  `TS2322` bucket's self-constraint representative, `different: types` on
  constraint errors and the `spuriousCircularityOnTypeImport` display row; the
  unmapped `hasNonCircularBaseConstraint`, `getBaseConstraintOrType`,
  `getConstraintOfIndexedAccess`, `getConstraintFromIndexedAccess`,
  `mayResolveTypeAlias`, `isResolvedByTypeAlias` and
  `getTypeParametersForTypeAndSymbol`. Three different mechanisms are kept
  apart, each with its own witness in C1.8:
  1. *Stack growth* (ADR 0011, `stacker::maybe_grow`): a successful growth
     continues checking and returns the ordinary result; it is not a failure
     and produces no diagnostic; the checker stays reusable afterwards.
  2. *Semantic limits*, each with the pin's exact result: the relater's
     100-level backstop returns `TernaryMaybe` with no diagnostic
     (`relater.go:3133`); `isDeeplyNestedType` likewise yields `Maybe`; the
     instantiation limit (`instantiationDepth == 100` or `instantiationCount
     >= 5_000_000`, `checker.go:22452`) reports
     `Type_instantiation_is_excessively_deep_and_possibly_infinite` at the
     current node and returns the error type; `Excessive_complexity_comparing_
     types_0_and_1` is reported where `relater.go:381` and `:3108` report it.
     The union-size limit (`Expression_produces_a_union_type_that_is_too_
     complex_to_represent`, blocker B10) stays with C2.
  3. *Panic retirement* (ADR 0012): only an actually caught panic retires the
     generation; deep but valid checking never does.
  A Rust limit never reports a diagnostic the pin does not, and never turns
  valid deep checking into a failure.
- Exit: the claimed rows match; the deep-recursion cases run on the E2 small
  stacks in debug and release with the pin's results; a constructed unbounded
  constraint ends exactly as the pin ends it.

### C1.6 Relations in all five modes and comparison diagnostics

- Exists: `relater.rs` and its `_compound`, `_conditional`, `_excess`,
  `_mapped`, `_properties`, `_signatures`, `_structure`, `_tuples` and
  `_variance` modules; `relation_errors.rs`, `relation_error_target.rs`,
  `relation_helpers.rs`; per-mode caches on the checker, assumptions committed
  on success and dependent assumptions dropped on failure; the ported
  comparators of ADR 0010; the 21-fixture relation contract.
- Build: the `TS2365` and `TS2741` buckets and the remainder of `TS2322` after
  C1.5; the 80 unmapped `relater.go` functions, most of them mapping work:
  `checkTypeRelatedTo`, `checkTypeAssignableToEx` and `AndOptionallyElaborate`,
  `checkTypeComparableTo`, the `isType{Assignable,Comparable,Identical,
  Subtype,StrictSubtype}To` entry points, `compareTypes{Identical,
  SubtypeOf,AssignableSimple,AssignableWorker}`, `Relater.isRelatedTo`,
  `isRelatedToWorker`, `isRelatedToSimple`, `typeRelatedToEachType`,
  `typeRelatedToIndexInfo`, `signaturesIdenticalTo`,
  `indexSignaturesIdenticalTo`, `isPropertySymbolTypeRelated`, the recursion
  identity helpers (`getRecursionIdentityTarget`, `hasMatchingRecursionIdentity`,
  `asRecursionId`), `resetMaybeStack`, the error state and chain
  (`getErrorState`, `restoreErrorState`, `getChainMessage`, `chainArgsMatch`,
  `chainDepth`, `createDiagnosticChainFromErrorChain`),
  `getUnmatchedProperties`, `shouldReportUnmatchedPropertyError`,
  `tryElaborateArrayLikeErrors`, `elaborateDidYouMeanToCallOrConstruct`,
  `findBestTypeForObjectLiteral`, `findBestTypeForInvokable`,
  `findMatchingTypeReferenceOrTypeAliasReference`,
  `templateLiteralTypesDefinitelyUnrelated` and
  `traceUnionsOrIntersectionsTooLarge`. A recursive encounter is a `maybe`
  result committed only by the outer success, and the modes never share one
  undifferentiated cache (S08 worklist item 4). Diagnostic elaboration keeps
  the pin's chain order and arguments.
- Exit: the claimed buckets match; the relation contract stays at 105 of 105
  against the prototype (its record may go stale under the phase-end rule, but
  the parity itself is rerun here); every unmatched-property and elaboration
  path has a native case.

### C1.7 Variance state

- Exists: `variance.rs` (marker instantiation, restart at the smallest source
  symbol), `relater_variance.rs`.
- Build: the `TS2636` bucket (variance annotation validation); the unmapped
  `getVarianceStackIndex`, `isMarkerType`, `hasCovariantVoidArgument` and
  `getKeyPropertyCandidateName`. The plan fixes the split: variance is
  measured by instantiating with marker types, so its completion over generic
  declarations belongs to C2. C1 delivers the state machine, the annotation
  check and the relater's use of recorded variances, and hands C2 a named list
  of the instantiation-dependent measurements (each with its pinned function
  and the corpus rows that need it).
- Exit: `TS2636` matches; the handoff list is in the C1 record and in
  `c1-audit.json` as `later: C2`.

### C1.8 Direct contracts: recursive, cold/repeated and failure paths

- Exists: the assets of section 2 (relation fixtures, ownership schedules, E2
  recursion tests, 44 direct tests).
- Build: `crates/tsr_compiler/tests/c1_contracts.rs`, one test per contract,
  each over production entry points with a pinned Go counterpart named in its
  doc comment:
  1. lazy resolution enters a cycle: the guard marks the cycle, the entity gets
     the pin's error type and diagnostic, and the checker is reusable after;
  2. a query repeated in the same checker returns the identical type id and
     performs no new type, signature or instantiation work (the relation
     fixtures' work classes);
  3. a failed relation leaves no committed assumption behind, and the same
     relation in another mode is not answered from the first mode's cache;
  4. merges across the four schedules give distinct checker-local symbols and
     unchanged source graphs (delegates to `s08_ownership.py`'s program);
  5. a relation or resolution deep enough to grow the stack completes with
     the ordinary result on the E2 small stacks, and the same checker answers
     a later query;
  6. each semantic limit of C1.5 produces the pin's exact result and
     diagnostic, compared with a native observation of the same program;
  7. an injected panic inside a relation or resolution unwinds, retires the
     generation as ADR 0012 requires, and a fresh checker on the same program
     succeeds;
  8. the five relation modes over the 21 fixtures, first, repeated and
     reversed, in debug and release.
- Exit: `cargo test -p tsr_compiler --features relation-probe --test c1_contracts` in debug and release;
  the receipt is recorded by the producer (C1.10).

### C1.9 Inherited Phase 1 items

Section 3 of the plan asks the first detailed plan to record two items; C0 did
not, so C1 does:

- The six checker-facing project-reference `Program` accessors and the
  `isSourceFromProjectReference` helper (Phase 2 rows in the
  [destination audit](PHASE1-F5b-destinations.md)). C1 ports them in C1.2:
  they feed symbol resolution across referenced projects, and one corpus case
  file carries a tsconfig `references` list. Their loader-side counterparts
  stay with Phase 1.
- Generic qualified-name serialization's fourth scoped
  `typeParameterSymbolList` (`CopyOnWriteSet<SymbolId, ...>`, both
  repeated-symbol suppression sites and nested success/error scope restoration,
  as the [Phase 1 plan](PHASE1-implementation-plan.md#ordered-storage-and-json-integration-decision--2026-09-21)
  specifies). It is node-builder work and goes to C5, recorded in
  `c1-audit.json` as `later: C5`.

### C1.10 Producer wiring

- Exists: `scripts/phase2_producers.py` emitting the C0 metrics and the
  parity ratios; `sprints/P2B.toml` with `P2B-C1` waiting on
  `run.checker.c1_complete`.
- Build: the producer reads `data/phase2/c1-claims.json`,
  `data/phase2/c1-audit.json`, `data/phase2/c1-baseline.json.gz` and the
  contracts receipt alongside the recorded capture and emits `c1_open`
  (claimed rows not matching in every domain), `c1_regressions` (rows with a
  domain that matched in the baseline and does not match now, over the full
  denominator; the baseline must name the same native observation and
  inventory digests as the run), `c1_failures` (failed rows whose panic module
  or error site is a foundation module), `c1_audit_complete`, `c1_contracts`
  and `c1_complete`. `phase2_compare.py report` also gains a `regressions`
  list next to `changed_observations`, which today records only rows that
  stayed different with a changed digest; both are kept. The receipt comes
  from `phase2_producers.py observe --witness c1-contracts`, in the Phase 1
  pattern: it records the command, exit code, output digests and the source
  inputs of the test crate, so an edit to a contract test after the recording
  stales it. All four new authorities are added to `[checker]` `inputs` in
  `status/runs.toml`, which lists files by name, so editing any of them
  invalidates the recorded result; the producer keeps one capture identity:
  the exit run writes `target/phase2/rust` (the C0 capture moves to
  `target/phase2/rust-c0`), and the producer, the ledger command and the
  recording all read that path. A sample run cannot feed these metrics: they
  come only from a recorded full capture, as C0's run policy says.
  `P2B-C1.done_when` stays `run.checker.c1_complete == true`.
- Exit: `cargo xtask validate`; `phase2_producers.py checker` emits the six
  metrics; tests in `scripts/tests/test_phase2_producers.py` show that a
  claimed row that differs keeps `c1_complete` false, that an unclaimed
  non-S08 row regressing from match to different makes `c1_regressions`
  nonzero, and that changing each of the four new inputs invalidates the
  recorded result.

## 5. Dependencies and owners

| Dependency | Owner | State for C1 |
| --- | --- | --- |
| Program loading, options, module resolution, binder over every executed variant | Phase 1 | no open item (C0 record); the four pending Phase 1 entries (Realpath capture, fresh mutation evidence for three operations) are not C1 inputs |
| C0's recording of `checker` on the designated host | owner | owed; C1.0's run is recorded in its place if C0's never was |
| Instantiation-dependent variance measurement | C2 | handoff list from C1.7 |
| Inference, mappers, conditional, mapped, indexed-access and template-literal operators behind `TS2344`, `TS2339`, `TS2345` and the returned buckets | C2 | not C1 inputs; C2 starts on ordinary generics after C1.3 and C1.6 |
| Node-builder retention (`tsr_ast::factory` panic), `typeParameterSymbolList` | C5 | recorded, not C1 inputs |
| The isolated relater prototype (ADR 0008) | S08 history | kept at parity; not extended |
| Owner decisions of section 9 | owner | before C1.1 |

## 6. Delivery order

1. C1.0 the fresh gap map, then C1.1 the audit and the claims file, reviewed
   before production changes: the audit decides how much of section 4 is
   mapping work and how much is implementation.
2. Crashes first, because a failed row withholds every domain: C1.4's two
   tuple panics and C1.5's stack overflow, each with its direct case.
3. C1.2 and C1.3, since members and merged symbols feed every relation.
4. C1.6, the largest item, in mode order (identity, assignable, comparable,
   subtype, strict subtype), rerunning the relation contract after each.
5. C1.7 and the C2 handoff; C1.9 accessors inside C1.2.
6. C1.8 grows alongside 2–5, one contract per item; C1.10 last, then the exit
   full run and the record.

Intermediate runs use the recorded 300-variant sample plus the claimed rows
(`--sample --case ...`, about 10 s of Rust time). Full runs are C1.0, the exit,
and whenever a relation-cache or resolution-guard change is broad enough that a
sampled regression cannot bound it. Changes inside already-differing rows are
inspected through `--previous`, and match-to-non-match transitions through the
baseline; a row is never trusted because its category did not move.

## 7. Executable exit checks

```sh
export PATH="$(mise where go)/bin:$PATH"
python3 scripts/phase2_inventory.py check
python3 scripts/phase2_native.py verify --capture target/phase2/native
mv target/phase2/rust target/phase2/rust-c0                                  # once; the producer reads target/phase2/rust
python3 scripts/phase2_corpus.py run --native target/phase2/native --output target/phase2/rust
python3 scripts/phase2_compare.py report --native target/phase2/native --rust target/phase2/rust --previous target/phase2/rust-c0/comparison.json --record
python3 scripts/phase2_blockers.py build --native target/phase2/native --rust target/phase2/rust --record
python3 scripts/phase2_audit.py check --groups c1
cargo test -p tsr_compiler --features relation-probe --test c1_contracts && cargo test -p tsr_compiler --features relation-probe --test c1_contracts --release
python3 scripts/phase2_producers.py observe --witness c1-contracts           # the receipt, with source binding
python3 scripts/phase2_producers.py checker            # c1_open 0, c1_regressions 0, c1_failures 0, c1_audit_complete, c1_contracts, c1_complete
python3 scripts/s08_relater.py build  --output target/s08/relater-c1
python3 scripts/s08_relater.py parity --output target/s08/relater-c1         # 105/105, all_cases_match true, both implementations
python3 -m pytest scripts/tests/test_phase2_*.py -q
python3 scripts/checks.py fmt && python3 scripts/checks.py clippy
cargo xtask validate && cargo xtask check P2B                                 # reports P2B-C1 done; the sprint stays open until C7
```

`cargo xtask run checker` and `cargo xtask status --record` are the owner's.
`check P2B` cannot pass before C7 by construction; the C1 assertion is the
`P2B-C1` line of its report and the six metrics above.

## 8. Evidence reuse rules

- The C0 native capture is reused as long as `verify` accepts it; C1 changes
  no oracle source. A Rust capture is reused only when `replay` accepts it
  against the captured executable; every production commit stales it, which is
  why intermediate work runs the sample.
- The C1-start baseline is reused only while it names the current native
  observation and inventory digests; a new native contract needs a new
  baseline, taken before any production change.
- Claims are per row and per cause. A row C1 fixes stays owned by its
  checkpoint in the inventory; the claims file is the only record of C1's
  share, and the producer counts from it.
- A difference is accepted only through the divergence ledger (ADR 0004), per
  variant and metric, with hashes. C1 expects none: its rows are foundation
  defects, not approved divergences.
- The relation contract, the ownership schedules and E2 may go stale under the
  phase-end rule when C1 touches fingerprinted sources; their parity is rerun
  in section 7, and their records are refreshed at the phase-end green-up.
- Mapping progress is reported by markers and the ledger, never as behavioral
  coverage; the corpus and the C1.8 contracts are the behavioral evidence.

## 9. Owner decisions before C1 starts

1. **Claim rule.** C1 claims causes inside C2-owned rows without changing row
   ownership, through `data/phase2/c1-claims.json` confirmed by reproduction
   (section 3). The alternative, re-owning those rows to C1 in the inventory,
   would make the checkpoint rule depend on execution outcomes, which the C0
   rule forbids.
2. **Accessor dispositions.** The 103 unmapped `types.go` methods are mostly
   accessors over a payload layout Rust deliberately does not share
   (`types.rs`). Proposed: `equivalent` dispositions naming the Rust field or
   accessor, the treatment Phase 1 accepted for accessors, rather than one
   Rust function per Go method.
3. **Variance split.** C1 delivers variance state, annotation validation and
   the relater's use of recorded variances; instantiation-dependent measurement
   is C2's, by a named handoff (C1.7). Confirm, or move all of variance to C2.
4. **Project-reference accessors.** Proposed owner C1 (C1.9), since they feed
   symbol resolution across references. The alternative is C3 with modules and
   aliases.
5. **Exit run.** `c1_complete` is computed only from a recorded full capture
   (C1.10). Confirm that the C1 exit recording is the owner's, like C0's, and
   whether C1.0's recording may stand in for the C0 recording still owed.
