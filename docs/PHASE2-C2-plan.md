# Phase 2 C2: inference and advanced type interactions

Checkpoint C2 of the [Phase 2 plan](PHASE2-plan.md), written after the
[C1 plan](PHASE2-C1-plan.md) (PR #60) and its implementation (PR #61, branch
`phase2-c1`, head `c583254` with the exit evidence recorded at `0ab79c6`).
Upstream is Corsa `1f70213d4922b434345f639b441681e470c7cfc1`. C2 is production
work in `crates/tsr_checker`, paired with its witnesses, measured by the C0
contract exactly as C1 was.

## 1. Objective and exit

Complete inference and the advanced type operators: generic declarations and
calls, constraints and defaults, overload selection, contextual typing, mappers
and substitution, conditional and `infer` types, mapped, indexed-access,
`keyof`, template-literal, string-mapping and import types, instantiation
expressions, and the depth and complexity limits. Test their combinations and
their use over the loaded libraries, not isolated syntax. Implement the scoped
creation-trace diagnostic mode of [ADR 0010](adr/0010-order-sensitive-outputs-are-enumerated--comparators-are-port.md)
in both binaries and witness the known residuals plus the actual tie branches
identified in C2.10. Retain everything S08 and C1 implement.

The numbers below describe the pre-review C1 capture, not the amended sources.
C2.0 refreshes them after the C1 review fixes; C1 completion is not assumed.

C2 exits when all of the following hold on one full run at the C2 head,
recorded by the owner through `cargo xtask run checker`:

| Exit condition | Measured by |
| --- | --- |
| Current, full, authenticated and recorded capture with no harness errors | `inventory_frozen`, `native_verified`, `harness_valid`, `result_recorded` and `blockers_named` are true; these are prerequisites of `c2_complete` |
| Existing cases preserved: the S08 regression subset still matches completely | `run.checker.regression_parity == 1` (9,367 of 9,367) |
| No previously matching domain of any executed row becomes a non-match | `run.checker.c2_regressions == 0` against the authenticated C2-start row report `data/phase2/c2-baseline.json.gz` |
| Every C2-owned row meets all seven applicable comparison domains, or every remaining difference has a current validated handoff (section 3) | `run.checker.c2_open == 0` over `data/phase2/c2-claims.json`; `run.checker.c2_handoffs` reported |
| No unexplained production failure anywhere in a C2 claim, regardless of crate/module; exclusions require a current site-specific handoff | `run.checker.c2_failures == 0` |
| Every C2-owned blocker of `data/phase2/blockers.json` is closed by an implementation or re-owned with the trace that names the owner | `run.checker.c2_blockers_open == 0` |
| The C2 function groups of section 4 are audited: each pinned function mapped, disposed as equivalent, or handed to a named later checkpoint; no unexplained gap | `run.checker.c2_audit_complete == true` over `data/phase2/c2-audit.json` |
| The direct contracts of C2.11, including the creation-trace witnesses of both ADR 0010 residual cases, pass in debug and release | `run.checker.c2_contracts == true`, from the recorded test receipt |
| One comparable owner measurement capture exists at the C2 head, without a threshold (plan section 6) | `run.checker.c2_measured == true`, from the recorded checkerbench capture identity |

`run.checker.c2_complete` is the conjunction. It binds `P2B-C2` in
`sprints/P2B.toml`. No ratio threshold is introduced: C2 either closes its rows
or names, per row, the checkpoint whose cause keeps them open.

## 2. Starting point

Everything below exists and is consumed as is. C2 extends it; it re-derives
nothing.

| Asset | Where | What C2 takes from it |
| --- | --- | --- |
| The C1 result | full corpus at `c583254`: 12,458 of 13,432 rows match in every domain; regression 9,367 of 9,367; open rows: C2 192 of 2,954, C3 15, C4 767; one production failure left (`declarationEmitAugmentationUsesCorrectSourceFile`, a node-builder retention panic, C5's) | the rows C2 owns (section 3) and the harness C1 left |
| The C1 harness | `data/phase2/c1-claims.json`, `c1-audit.json`, `c1-baseline.json.gz`, `receipts/c1-contracts.json`; `scripts/phase2_audit.py`, `phase2_compare.py baseline` and `regressions`, the six `c1_*` metrics and the `observe --witness` receipt in `scripts/phase2_producers.py`; `scripts/tests/test_phase2_c1.py` | the pattern every C2 authority follows; the audit script is reused with `--audit data/phase2/c2-audit.json` |
| The C2 modules | `inference.rs` and eleven `infer_*.rs` modules, `higher_order_inference.rs`, `return_inference.rs`, `call_arguments.rs`, `calls.rs`, `call_errors.rs`, `call_failure.rs`, `call_spread.rs`, `call_tagged.rs`, `check_generics.rs`, `instantiate.rs`, `instantiation_expressions.rs`, `mapper.rs`, `substitution.rs`, `conditional.rs`, `mapped.rs`, `template.rs`, `template_relation.rs`, `string_mapping.rs`, `import_types.rs`, `expression_context.rs`, `relater_conditional.rs`, `relater_mapped.rs`, `variance.rs`, `relater_variance.rs` | the production code to retain and complete; `getConditionalType`, `getConditionalTypeInstantiation`, `instantiateMappedType`, `getTemplateLiteralType`, `getStringMappingType`, `getTypeFromImportTypeNode`, `resolveCall`, `chooseOverload`, `inferTypeArguments`, `checkTypeArguments`, `getContextualType` and `instantiateTypeWithAlias` already carry `port:` markers |
| Mapping state | `inference.go` 39 of 77 marked, `mapper.go` 9 of 36, `checker.go` 1,025 of 1,504 with 118 unmarked functions whose names fall in C2's areas; the ledger `rust` lists of `inference.go` and `mapper.go` are empty (`status = planned`) while the modules exist | the per-file audit input of C2.1; mapping gaps to separate from real gaps |
| The C1 fixes that already touched C2 territory | distributive conditional constraints use `someType`; base constraints are computed over the simplified type; nested conditional constraints stop at 100 levels; a permissive-wildcard inference has no default; the canonical target signature; conditional-target and mapped-target relations never report | done; C2 does not redo them |
| Limits already in place | `instantiate.rs`: depth 100 or 5,000,000 instantiations report `Type_instantiation_is_excessively_deep_and_possibly_infinite` at the current node; `constraints.rs`: the 100-level conditional constraint bound; `union_reduction.rs`: the union-size error at the pin's cross-product estimate | witnesses to add, not code to write |
| ADR 0010 assets | the ported comparators (`CompareTypes`, `compareSymbols`, `compareNodes`, `CompareDiagnostics`); the S08 P3 comparator replay (`scripts/s08_p3_comparators.py`, run by the E2 obligations: 8 residual families, 7 union matrices, 77 exact permutations); the union-ordering sub-test over every interned union (C0.7) | the fixtures C2 retains; the sub-test the trace mode complements |
| Measurement producers | `[checkerbench]` (`scripts/s08_checkerbench.py`: elapsed time and the type footprint over the S08 workload on the quiet host), `[e5]`/`[e6]` (peak RSS) | the capture C2's exit records once, without a threshold |
| Direct tests | `crates/tsr_compiler/tests/c1_contracts.rs` (7 contracts, feature `recursion-probe`), `checker_semantics.rs`, `checker_display.rs`, `checker_dynamic_imports.rs`; 44 direct tests in `tsr_checker` | the homes C2.11 adds to |
| Pin tracing | the harness overlay build (`phase2_native.build_oracle`) and one-row shard (`run_shard`) support instrumented copies through Go overlays; the pinned `upstream/` checkout remains untouched; the `tsgo` CLI skips semantic diagnostics when global diagnostics exist, so noLib rows are traced through the harness | the attribution method of C2.1 (an access-only observation hook in an overlay, one row, `go.stderr`; never edit the gitlink checkout) |

The historical C1 capture is `target/phase2/rust`. Preserve it and its
row report without overwriting an existing directory. C2.0 captures the reviewed
C1 sources into a fresh named output, then freezes the authenticated C2 baseline.
The final producer uses an explicit capture path (or the default
`target/phase2/rust` after a deliberate promotion); never move or overwrite a
capture as an incidental exit-check step.

## 3. What C2 owns

Unlike C1, C2 owns rows: the inventory assigns 2,954 executed variants to C2
by family (explicit type parameters 2,383, non-empty type arguments 1,966,
indexed access 377, mapped 304, conditional 264, import types 117, `infer`
115, template literals 62; a variant may carry several). 2,762 match in every
domain at the C1 head. The 192 open rows are C2's exit set, together with the
rows another checkpoint returns to C2 with a trace.

Open C2 rows by domain: errors 144, types 124, display 111, symbols 96, plus
the one node-builder panic row (every domain). By family: explicit type
parameters 176, type arguments 126, indexed access 73, mapped 66, conditional
51, `infer` 28, template literals 16, import types 10. 96 of the 192 emit
declarations, and 47 differ first at a declaration-emit diagnostic
(TS4023/4025/4031/4052/4060/4076/4082/9010).

The buckets at the C1 head (`target/phase2/rust/comparison.json`, owner C2):

| Bucket | Variants | Representative | Proposed cause |
| --- | ---: | --- | --- |
| `unsupported: typeReferenceToTypeNode: applied outer arguments` (B03) | 36 | `typeArgumentInferenceWithClassExpression` | missing reference-display branch for applied outer type arguments; existing parameter and argument state must be traced before attributing a type-model gap (C2.3) |
| `diagnostics: TS4025` | 31 | `declarationEmitTupleRestSignatureLeadingVariadic` | declaration-emit accessibility over C2 types: handoff candidate to C5 unless the trace lands in instantiation |
| `unsupported: extractRedundantTemplateLiterals` (B04) | 30 | `contextualPropertyOfGenericMappedType` | intersection reduction over template literals (C2.7) |
| `diagnostics: TS4060` | 10 | `declarationEmitNestedGenerics` | declaration-emit accessibility: handoff candidate to C5 |
| `different: types` | 9 | `circularInstantiationExpression` | instantiation-expression and conditional display (C2.3, C2.5) |
| `unsupported: addIntraExpressionInferenceSite: array element` (B07) | 7 | `inferringAnyFunctionType2` | intra-expression inference sites (C2.4) |
| `unsupported: conditionalTypeToTypeNode: shadowed distribution parameter` (B08) | 5 | `recursiveConditionalCrash1` | conditional display must preserve distribution when the instantiated check type is no longer a type parameter (C2.5) |
| `diagnostics: TS2345`, `TS2554`, `TS2558` | 4, 1, 1 | `superWithTypeArgument3` | `super` calls with type arguments: argument and arity checking against the instantiated base (C2.9) |
| `diagnostics: TS7060` | 4 | `nodeModulesForbidenSyntax` | option-dependent checking of `import type` in node16 modules: handoff candidate to C3 |
| `diagnostics: TS2322`, `TS2339`, `TS2344`, `TS2313` | 3, 3, 3, 1 | `recursiveIndexedAccessSimplification`, `defaultPropsEmptyCurlyBecomesAnyForJs`, `distributiveConditionalBaseConstraint`, `inferTypesWithExtends1` | indexed-access simplification, generic member access, type-argument constraint satisfaction, `infer` constraints (C2.6, C2.2, C2.5) |
| `diagnostics: TS2365` | 3 | `comparisonOperatorWithNumberOperand` | operand display, literal against base type: C1 traced these to C3's operand typing; handoff candidate |
| `unsupported: Expression_produces_a_union_type_that_is_too_complex_to_represent` (B10) | 3 | `templateLiteralTypeTooComplex` | the union-size limit in template-literal cross products (C2.7, C2.10) |
| `unsupported: getTypeFromImportTypeNode: CommonJS typedef export lookup` (B11) | 3 | `jsdocImportTypeReferenceToStringLiteral` | import types over JSDoc typedef exports (C2.8) |
| `diagnostics: TS2352` | 2 | `aliasInstantiationExpressionGenericIntersectionNoCrash1` | returned by C1: the comparable relation over instantiation-expression aliases (C2.3) |
| `diagnostics: TS2795` | 2 | `intrinsicKeyword` | invalid `intrinsic` alias names or arities (C2.7) |
| `unsupported: node builder synthetic elision comments` (B13, B14) | 2, 2 | `nestedSpreadsAndWidening`, `hugeDeclarationOutputGetsTruncatedWithError` | node-builder output: re-own to C5 with the trace (decision 2) |
| `unsupported: someSymbolTableInScope: reparsed module` (B15) | 2 | `jsDeclarationsImportAliasExposedWithinNamespace` | node-builder scope lookup over reparsed JS modules: re-own to C5 (decision 2) |
| `diagnostics: TS18048`, `TS2536`, `TS7053`, `TS7006`, `TS1477` | 1 each | `specialIntersectionsInMappedTypes`, `unknownControlFlow`, `indexSignatures1`, `contextualTypeCaching`, `instantiationExpressionErrors` | mapped intersections, indexed access under narrowing, index-signature access, contextual type caching, instantiation-expression syntax (C2.6, C2.9, C2.3) |
| `diagnostics: TS1539`, `TS2315`, `TS2337`, `TS2349`, `TS2355` | 1 each | `bigintPropertyName`, `cjsExportGenericTypes`, `errorSuperCalls`, `jsDeclarationsFunctions`, `errorOnFunctionReturnType` | C1's attribution named C3 for these (bigint property names, JS `export=`, super-call placement, JS function typing): handoff candidates |
| `diagnostics: TS4023`, `TS4031`, `TS4052`, `TS4076`, `TS4082`, `TS9010` | 1 each | `mappedTypeGenericInstantiationPreservesHomomorphism` and others | declaration-emit diagnostics: handoff candidates to C5 |
| `unsupported: checkIfTypePredicateVariableIsDeclaredInBindingPattern` (B16) | 1 | `typeGuardFunctionErrors` | predicate parameters in binding patterns (C2.9) |
| `unsupported: reportOperatorError: awaited operand suggestions` (B18) | 1 | `operationsAvailableOnPromisedType` | the awaited-operand suggestion in operator errors (C2.9) |
| `unsupported: checkExpressionWorker` (B01, the C2 share) | 1 | `importWithTypeArguments` | `import()` expressions with type arguments (C2.8) |
| `different: display` | 1 | `spuriousCircularityOnTypeImport` | circular import-type display (C2.8) |
| `failed: walker_error` | 1 historically | `packageDeduplicationDuplicateGlobals` | fixed in the C1 review: discarded package-dependency metadata poisoned module-specifier generation; matches in the reviewed-source C2 baseline |
| `panic: tsr_ast::factory` | 1 | `declarationEmitAugmentationUsesCorrectSourceFile` | C5 declaration-transform late visibility retention, confirmed by the debugger stack recorded in C1 |

Blockers C2 owns in the register: B03, B04, B07, B08, B10, B11, B13, B14, B15,
B16, B18 (92 variants) and the C2 share of B01. C2 closes a blocker by
implementing the refused operation, or re-owns it with the trace that names the
owner; the register is rebuilt at the exit and never edited by hand.

Ownership rule. Row ownership does not change. `data/phase2/c2-claims.json`
records every open C2 row with its bucket, its smallest reproduction, the
pinned function the trace named and a status: `open` (C2's), `closed` (matches
at the current head, with the commit), `blocked` (a registered blocker of
another owner withholds it, for example B09), or `handed` (a trace attributes
the remaining difference to a named checkpoint's cause). The producer validates the complete inventory and all statuses. A `closed` row
that no longer matches becomes open again; a candidate without a demonstrated
cause stays open. Both `open` and regressed `closed` rows count
in `c2_open`; `handed` rows are counted separately in `c2_handoffs` and listed
in the C2 record, so the exit shows exactly what C2 leaves to C3 and C5.
The status label alone never exempts a current difference.
A handoff needs the same evidence a C1 claim needed: the reproduction, the
pinned function and the observation that places the cause outside C2's groups.
Rows C1 returned to C2 (the two `TS2352` rows) start as `open`.

The seven domains are `errors`, `types`, `symbols`, `display`, `trace`,
`union_ordering` and `parent_pointers`; native-disabled domains are met.
Declaration diagnostics are already part of `errors`, not an eighth domain.
A C5 declaration handoff never silently disables the whole errors comparison
or an unrelated type/display difference.

The checkpoint validator requires these fields for a handoff:

- target checkpoint and pinned Go function ID, checked against the frozen
  function inventory;
- reproduction argv naming the exact case, and the originating authenticated
  capture/request identities;
- trace artifact path and digest, plus the canonical **full raw observation**
  digest, including fatal class and `panic_location` (the existing fatal
  domain digests alone omit that location);
- covered comparison domains, the traced cause, and, for a blocker handoff,
  its kind and missing operation. B-numbers are display labels regenerated by
  group ordering, not durable identities.

The trace must cover every currently nonmatching domain before `handed` or
`blocked` can exempt a row. A new domain difference, a changed raw observation,
request, failure site or trace artifact invalidates the exemption and leaves
that row open. Different buckets or free-text owner strings are not proof.

The C4-owned rows that also carry C2 families are not C2's exit set; their
open outcomes are JSX and decorator refusals. C2's operators are still
exercised through them, and a C4 row whose only remaining difference is a C2
operator after C4's refusal is lifted comes back to C2 by the same handoff rule
in reverse.

## 4. Work items

Each item names what exists, what to build, its artifact and its executable
exit check. Function groups are named by their pinned Go names
(`tsc/internal/checker`); `port:` markers and the ledger stay the mapping
authority, and the counts are the marker state at the C1 head.

### C2.0 Refresh the gap map at the C2 head

- Exists: the C1 exit capture and comparison at `c583254`; `phase2_compare.py
  baseline`.
- Build: preserve the historical C1 report, check the existing native capture
  with its read-only freshness validator, and run one full Rust contract on
  the reviewed C1 sources in a fresh output directory. Compare against the
  historical report and explain every change from the C1 review fixes. Freeze
  the authenticated result with `baseline` into
  `data/phase2/c2-baseline.json.gz`; `data/phase2/c2-claims.json` from the
  table of section 3, every open C2 row with status `open` and the C1 returns.
- Exit: 0 harness errors, 0 regressions and an explained per-row delta against
  the historical C1 comparison, the baseline committed; the claims file names every open C2 row
  exactly once.

### C2.1 Audit and attribution

- Exists: `scripts/phase2_audit.py` (dispositions `mapped`, `equivalent`,
  `missing_mapping`, `gap`, `later`; markers as the authority; `--audit`
  selects the document), the C1 audit's conventions for inlined equivalents.
  The current validator deliberately binds C1's exact reviewed groups; extend
  it with a separately reviewed C2 scope binding, not by weakening the C1 check.
- Build: `data/phase2/c2-audit.json` with these groups, each function listed
  by its pinned name:
  - `inference.go` (77 functions, 38 unmarked: `getInferenceState`,
    `putInferenceState`, `inferFromTypeArguments`, `inferWithPriority`, the
    three contravariant inference entries, `compareTypesAndDepth`,
    `getTypeListDepth`, `getSingleTypeVariableFromIntersectionTypes`,
    `inferToMultipleTypesWithPriority`, `inferFromProperties`,
    `inferReverseMappedTypeWorker`, `replaceIndexedAccess`,
    `tupleTypesDefinitelyUnrelated`, `newInferenceContextWorker`,
    `getInferredTypes`, `getMapperFromContext`, `getContravariantInference`,
    `unionObjectAndArrayLiteralCandidates`, `hasPrimitiveConstraint`,
    `isTypeParameterAtTopLevelInReturnType`, `getTypeFromInference`,
    `getInferenceInfoForType`, `getSingleCommonSupertype`, `findLeftmostType`,
    `getCommonSubtype`, `getCombinedTypeFlags`, `literalTypesWithSameBaseType`,
    `isFromInferenceBlockedSource`, `isSkipDirectInferenceNode`,
    `newInferenceInfo`, `cloneInferenceInfo`, `hasInferenceCandidates`,
    `hasInferenceCandidatesOrDefault`, `hasTypeParameterDefault`,
    `hasOverlappingInferences`, `mergeInferences`);
  - `mapper.go` (36 functions, 27 unmarked, mostly the `Map`, `Kind` and
    `MapsThisOnly` methods of the mapper kinds and their constructors; the
    Rust `Mapper` enum in `mapper.rs` implements them as variants, the
    `equivalent` disposition C1 used for accessors applies);
  - `checker.go` groups: instantiation and keys (`instantiateTypes`,
    `instantiateSymbols`, `instantiateSignatures`, `instantiateSymbolTable`,
    `createInstantiatedSymbolTable`, `getTypeAliasInstantiationKey`,
    `getTypeInstantiationKey`, `getIndexedAccessKey`, `getConditionalTypeKey`,
    `getSignatureInstantiationWithoutFillingInTypeArguments`,
    `isTypeReferenceWithGenericArguments`, `writeGenericTypeReferences`,
    `newSubstitutionType`, `newConditionalType`, `newIndexedAccessType`,
    `newStringMappingType`); type parameters, constraints and defaults
    (`GetTypeAliasTypeParameters`, `getTypeParametersForTypeReferenceOrImport`,
    `getTypeParametersForTypeAndSymbol`, `getConstraintOrUnknownFromTypeParameter`,
    `isUnconstrainedTypeParameter`, `isThislessTypeParameter`,
    `getDefaultTypeArgumentType`, `getDefaultFromTypeParameter`,
    `hasTypeParameterByName`, `getUniqueTypeParameterName`,
    `getOuterInferenceTypeParameters`, `getOuterTypeParametersOfClassOrInterface`,
    `canGetTypeParametersOfClassOrInterface`, `appendTypeParameters`,
    `hasCorrectTypeArgumentArity`, `getEffectiveTypeArguments`,
    `getEffectiveTypeArgumentAtIndex`, `getTypeArgumentsFromNode(s)`,
    `getTypeArgumentsForAliasSymbol`, `checkNoTypeArguments`,
    `checkTypeReferenceOrImport`, `checkConditionalType`,
    `checkIndexConstraintForIndexSignature`); conditional and `infer`
    (`getConstraintOfConditionalType`, `getTrueTypeFromConditionalType`,
    `getFalseTypeFromConditionalType`, `getInferredTrueTypeFromConditionalType`,
    `isGenericType`, `isGenericObjectType`, `isNonGenericObjectType`,
    `isGenericTypeWithUndefinedConstraint`); mapped, indexed access and
    `keyof` (`getIndexType`, `getIndexTypeOfType(Ex)`, `getIndexedAccessType`,
    `getSimplifiedIndexedAccessType`, `distributeObjectOverIndexType`,
    `getConstraintOfIndexedAccess`, `getConstraintFromIndexedAccess`,
    `getApplicableIndexInfo(s)`, `getApplicableIndexInfoForName`,
    `instantiateMappedArrayType`, `isMappedTypeWithKeyofConstraintDeclaration`,
    `isPartialMappedType`, `isCircularMappedProperty`,
    `hasArrayOrTypeTypeConstraint`, `isGenericTupleType`, `getWidenedProperty`);
    template literals and string mapping (`extractRedundantTemplateLiterals`,
    `isTypeMatchedByTemplateLiteralOrStringMapping`, `applyTemplateStringMapping`,
    `isTemplateLiteralContext`, `isTemplateLiteralContextualType`,
    `getUniqueLiteralTypeForTypeParameter`); import types and instantiation
    expressions (`getTargetOfImportSpecifier`,
    `getSymbolOfPartOfRightHandSideOfImportEquals`,
    `isInternalModuleImportEqualsDeclaration`, `isPartOfImportEqualsModuleReference`,
    `markImportEqualsAliasReferenced`, `reportInvalidImportEqualsExportMember`,
    `getSuggestedImportSource`, `isESMFormatImportImportingCommonjsFormatFile`,
    `getTypeOfModuleDeclarationImportAttributes`); contextual typing (the
    eleven unmarked `getContextualTypeFor*` entries,
    `getContextualSignatureForFunctionLikeDeclaration`,
    `appendContextualPropertyTypeConstituent`, `pushCachedContextualType`,
    `pushContextualType`, `popContextualType`, `findContextualNode`,
    `pushInferenceContext`, `popInferenceContext`,
    `getWidenedLiteralLikeTypeForContextualType`,
    `getWidenedLiteralLikeTypeForContextualReturnTypeIfNeeded`,
    `shouldReportErrorsFromWideningWithContextualSignature`,
    `getWidenedTypeForVariableLikeDeclaration`, `getWriteTypeOfInstantiatedSymbol`);
    calls and overloads (`isNotOverload`, `pickLongestCandidateSignature`,
    `getLongestCandidateIndex`, `inferSignatureInstantiationForOverloadFailure`,
    `createCombinedSymbolForOverloadFailure`, `isGenericFunctionReturningFunction`,
    `skippedGenericFunction`, `assignNonContextualParameterTypes`,
    `getReturnTypeOfSingleNonGenericSignature(OfCallChain)`,
    `checkCollisionWithGlobalPromiseInGeneratedCode`, `tryCreateAwaitedType`,
    `getAwaitedTypeOfPromiseEx`).
  The 118 unmarked names above are the inventory of the C1-head marker state,
  not an effort measure: many will be `equivalent` (inlined at a cited Rust
  site, as 50 of C1's 59 gaps were). C1's explicit `c2-variance-measurement` handoff is separate from its
  mapped function dispositions. Consume its named functions and seven cases
  in C2.2; output equality does not prove the measurement flags or cycle state.
- Build, second half: the attribution of every open row of section 3 to a
  cause, by reproduction (`phase2_corpus.py run --case`) and, where the Rust
  observation does not name the function, by tracing the pin through the
  instrumented harness (section 2). The claims file gets the function and the
  commit or handoff for each row.
- Exit: `phase2_audit.py check --audit data/phase2/c2-audit.json` passes with
  no `gap`; every open row of section 3 has a named cause, and every handoff
  candidate is confirmed or taken back.

### C2.2 Generic declarations, type parameters, constraints and defaults

- Exists: `check_generics.rs` (`checkTypeArgumentConstraints`), the
  constraint machinery C1 completed in `constraints.rs`, `type_parameters.rs`
  (defaults with the resolving sentinel), `variance.rs` and `relater_variance.rs`.
- Build: `TS2344` (constraint satisfaction over instantiated constraints, the
  distributive base constraint of `distributiveConditionalBaseConstraint`),
  `TS2313` in `inferTypesWithExtends1` (an `infer` type parameter's constraint
  cycle), the type-parameter and default group of C2.1; the variance
  re-verification: every generic alias, class and interface of the C2 families
  is measured through marker instantiation with the pin's `Unmeasurable` and
  `Unreliable` flags, checked by the union-ordering and display observations
  of the corpus and by a direct case per flag.
- Exit: the claimed rows match; a direct case per variance flag; the
  `Two_different_types_with_this_name` and `TS2313` paths have native cases.

### C2.3 Instantiation, mappers, substitution and instantiation expressions

- Exists: `instantiate.rs` (the object, mapped, conditional, index and
  substitution instantiation, the depth and count limits), `mapper.rs` (the
  `Mapper` enum: simple, array, function, composite, merged, deferred,
  inference, permissive, restrictive, unique-literal), `substitution.rs`,
  `instantiation_expressions.rs`, the alias instantiation cache in
  `references.rs`.
- Build: the `different: types` bucket on `circularInstantiationExpression`
  and the instantiation-expression display of `.types`; the two `TS2352` rows
  C1 returned (the comparable relation over `typeof Err<U>` intersections);
  `TS1477` (instantiation-expression syntax placement); B03: the node builder's
  `typeReferenceToTypeNode` refuses references with applied outer type
  arguments (36 rows) at `node_builder_extra.rs`. `InterfaceData` already
  retains the outer-parameter count and parameter identities, while
  `ReferenceData` retains the resolved arguments. The pin groups outer
  parameters by declaring container, emits references for groups whose
  arguments differ from those parameters, and appends the final reference.
  First compare those values and container identities at the refusing branch.
  Port missing node-builder behavior using existing identities when sufficient;
  add type storage only for a demonstrated absent semantic fact. C2 retains
  this dependency until a trace proves a C5 cause; the alias instantiation key: the pin keys
  `getTypeAliasInstantiation` by the *unfilled* type arguments while
  `type_alias_instantiation` keys by the filled list, which conflates the alias-cache entries for
  `Foo<string>` and `Foo<string, Default>`. Port the keying, but do not assume
  distinct resulting type identities: downstream interning may still unify
  them in Go. Observe cache work, returned identities and display independently
  before fixing expectations (decision 4); the unmarked instantiation,
  key and mapper functions of C2.1.
- Exit: the claimed rows match; `--previous` shows no changed observation
  outside the rows named; the alias-key contract of C2.11 passes.

### C2.4 Inference

- Exists: `inference.rs` (contexts, inference infos, fixing and non-fixing
  mappers, intra-expression sites), `infer_types.rs`, `infer_matching.rs`,
  `infer_objects.rs`, `infer_reverse.rs`, `infer_templates.rs`,
  `infer_tuples.rs`, `infer_signatures.rs`, `infer_candidates.rs`,
  `infer_constraints.rs`, `infer_helpers.rs`, `higher_order_inference.rs`,
  `return_inference.rs`, `call_arguments.rs` (`inferTypeArguments`).
- Build: B07, the `addIntraExpressionInferenceSite: array element` refusal
  (7 rows; the site in `array_literals.rs` and `inference.rs`); the 38
  unmarked `inference.go` functions of C2.1, each disposed or ported;
  reverse mapped types (`inferReverseMappedTypeWorker`, `replaceIndexedAccess`
  and the array-element variant); contravariant inference with priorities and
  `strictFunctionTypes`; `getInferredTypes` and `mergeInferences` on cloned
  contexts; the return-type inference of generic functions returning
  functions (`isGenericFunctionReturningFunction`, `skippedGenericFunction`);
  `TS2345` on `super` calls with type arguments. Combinations are tested over
  the loaded libraries: the `Array`, `Promise`, `Map` and `Iterator` method
  signatures of the bundled libs are the inference sources the corpus exercises
  most, and C2's direct cases call them rather than hand-written signatures.
- Exit: the claimed rows match; B07 is closed; the inference contracts of
  C2.11 pass.

### C2.5 Conditional and `infer` types

- Exists: `conditional.rs` (`getConditionalType`, instantiation, the default
  and distributive constraints, simplification), `relater_conditional.rs`,
  the C1 fixes listed in section 2.
- Build: B08 (`conditionalTypeToTypeNode: shadowed distribution parameter`,
  5 rows): with `GenerateNamesForShadowedTypeParams`, the pin wraps a
  distributive conditional whenever its instantiated check type is no longer
  a type parameter. It introduces an `infer T` parameter and two outer
  conditionals to preserve distribution; a textual name collision is not
  required. The root already retains the source check and extends types,
  syntax node and infer parameters; `ConditionalData` retains the mapper and
  instantiated check and extends types. Trace those identities and prepend the
  new parameter mapping as the pin does before proposing a root-layout change.
  Keep B08 with C2 until the cause is attributed; the unmarked conditional helpers of C2.1; the
  `unknownControlFlow` `TS2536` row (indexed access under a narrowed
  conditional); `isDeeplyNestedType` over conditional roots with the pin's
  `Maybe` result (a C1.5 mechanism, now measured over the conditional family);
  `checkConditionalType` (the `infer` constraint and distribution checks).
- Exit: the claimed rows match; B08 is closed; a direct case per conditional
  limit.

### C2.6 Mapped, indexed-access and `keyof` types

- Exists: `mapped.rs` (homomorphic instantiation, constituents, `as` clauses,
  the apparent type over array and tuple constraints), `relater_mapped.rs`,
  `indexes.rs`, the indexed-access target case of `relater_structure.rs`.
- Build: `recursiveIndexedAccessSimplification` (`TS2322` missing:
  `getSimplifiedIndexedAccessType` and `distributeObjectOverIndexType`),
  `specialIntersectionsInMappedTypes` (`TS18048`), `indexSignatures1`
  (`TS7053`), `defaultPropsEmptyCurlyBecomesAnyForJs` (`TS2339` over a generic
  member access), the unmarked mapped and indexed-access functions of C2.1
  (`instantiateMappedArrayType`, `isPartialMappedType`,
  `isCircularMappedProperty`, `getConstraintOfIndexedAccess`,
  `getConstraintFromIndexedAccess`, `getApplicableIndexInfoForName`);
  `mappedTypeGenericInstantiationPreservesHomomorphism` differs at `TS4023`,
  a declaration-emit diagnostic, and is a handoff candidate once its
  homomorphism is confirmed preserved through the `.types` domain.
- Exit: the claimed rows match; every mapped-type modifier combination
  (`+?`, `-?`, `+readonly`, `-readonly`, `as`) has a native case over a generic
  and a concrete source.

### C2.7 Template-literal and string-mapping types, and the union-size limit

- Exists: `template.rs`, `template_relation.rs` (the comparable and
  definitely-unrelated rules C1 completed), `string_mapping.rs`,
  `infer_templates.rs`, `union_reduction.rs` (the cross-product estimate that
  reports `Expression_produces_a_union_type_that_is_too_complex_to_represent`).
- Build: B04, `extractRedundantTemplateLiterals` (30 rows, the refusal in
  `intersection.rs`); B10 (3 rows): `template.rs` refuses the template
  cross product instead of reporting the pin's error in
  `checkCrossProductUnion` when the estimated product is at least 100,000
  (`checker.go:27004`). That helper returns false, and
  `getTemplateLiteralType` then returns the error type; `TS2795`
  (`intrinsicKeyword`: the `intrinsic` keyword
  outside the pinned intrinsic names/arity pairs); `isTypeMatchedByTemplateLiteralOrStringMapping`,
  `applyTemplateStringMapping`, `isTemplateLiteralContext(ualType)`.
- Exit: the claimed rows match; B04 and B10 are closed; both limit sites have
  a direct case with the pin's diagnostic.

### C2.8 Import types and `import()` with type arguments

- Exists: `import_types.rs` (`getTypeFromImportTypeNode`), `import_calls.rs`,
  `external_aliases.rs`.
- Build: B11 (`getTypeFromImportTypeNode: CommonJS typedef export lookup`,
  3 JSDoc rows); the C2 share of B01 (`importWithTypeArguments`: an
  `import()` expression with type arguments refused in
  `checkExpressionWorker`); `spuriousCircularityOnTypeImport` (display of a
  circular import type); the import-equals helpers of C2.1; `TS2315` on a
  CommonJS `export=` of a generic type (`cjsExportGenericTypes`) is a handoff
  candidate to C3's module semantics unless the trace lands in instantiation.
- Exit: the claimed rows match; B11 is closed and B01's C2 share is removed
  from the register with its evidence.

### C2.9 Contextual typing and overload resolution

- Exists: `expression_context.rs` (`getContextualType` and the cached
  contextual type stack), `calls.rs` (`resolveCall`, `chooseOverload`, the
  empty-candidate result C1 fixed), `call_errors.rs`, `call_failure.rs`,
  `call_spread.rs`, `call_tagged.rs`, `object_literals.rs`.
- Build: the eleven unmarked `getContextualTypeFor*` entries and the
  push/pop/find helpers of C2.1; `contextualTypeCaching` (`TS7006`: the cached
  contextual type must not survive across the inference passes);
  `superWithTypeArgument*` (`TS2554`, `TS2558`, the `TS2345` chain); overload
  failure reporting (`pickLongestCandidateSignature`,
  `inferSignatureInstantiationForOverloadFailure`,
  `createCombinedSymbolForOverloadFailure`); B16 (predicate parameters
  declared in binding patterns, `check_type_syntax.rs`); B18 (the awaited
  operand suggestion of `reportOperatorError`, `binary.rs`); the `TS2365`,
  `TS2349`, `TS2355`, `TS2337`, `TS1539`, `TS7060` rows are attributed by
  trace and handed to C3 where the cause is operand typing, JS function typing,
  super-call placement, property-name syntax or module-option checking.
- Exit: the claimed rows match; B16 and B18 are closed; the contextual-type
  contracts of C2.11 pass.

### C2.10 Limits and the ADR 0010 creation-trace mode

- Exists: the instantiation depth and count limits, the conditional constraint
  bound and the union-size estimate (section 2); the ported comparators; the
  S08 P3 comparator replay inside the E2 obligations; the union-ordering
  sub-test. No creation-trace mode exists in `crates/`, `scripts/` or
  `xtask/` (plan review finding 5, resolution 6).
- Build, limits: a direct case per limit with the pin's exact diagnostic,
  node and result type, compared with a native observation of the same
  program (instantiation depth, instantiation count, conditional constraint
  depth, `removeSubtypes` and `checkCrossProductUnion`, and
  `isDeeplyNestedType` over conditional
  and mapped roots); a Rust limit never reports a diagnostic the pin does not. Each
  row records the reporting function and its caller. For cross products the
  callers are `getSpreadType` (two sites), `createNormalizedTupleTypeEx`,
  `getIntersectionTypeEx` and `getTemplateLiteralType`; B10 is the last path,
  not a general `getUnionType` cardinality check.
- Build, trace mode: observe the **actual final fallback branches** of
  `CompareTypes` and `compareSymbolsWorker`, not a filter assuming only two
  type kinds can reach them. The type-ID fallback is general after the earlier
  structural keys tie. Equal-named symbols also fall back when their first
  declarations compare equal (including the same declaration), as well as when
  both declaration lists are empty. Preserve the ADR's known witnesses and
  enumerate any additional observed family.
- Native instrumentation lives in a separately fingerprinted **diagnostic Go
  overlay**, generated under its build output from the pin. It may replace
  virtual copies of checker/AST/binder source through `-overlay`; it never
  edits `upstream/`. This is instrumentation, not an access-only observer.
  The Rust side uses `creation-trace`. Both are off in corpus and benchmark
  builds, and the trace build binds the pin, all replacement bytes, driver,
  compiler flags and executable. Implement this separate driver in
  `scripts/phase2_order_trace.py`, with diagnostic overlay inputs under
  `tools/phase2/order-trace/` and a witness manifest/receipt under
  `data/phase2/c2-order-traces.json`. Canonical corpus builders do not depend
  on these diagnostic-only files.
- Keep three events distinct: object birth, semantic-ID assignment, and
  comparator fallback. Go type IDs are assigned in `newType`; symbol IDs are
  assigned lazily by `ast.GetSymbolId`, not by `newSymbol`. Symbol trace tokens
  must be independent of semantic IDs. Never call `GetSymbolId`, `GetNodeId`,
  a comparator, formatter or lazy checker query merely to enrich an event.
  Read an existing ID without assigning it; observe assignment only within
  the natural assignment path. The original comparison calls and argument
  order remain unchanged.
- Birth hooks record an actual creating operation/caller or explicit operation
  scope, not an origin inferred later. Observe `newSymbol` once (`newSymbolEx`
  delegates to it), and binder/direct construction routes reached by the
  witnesses. An unobserved origin makes the trace incomplete; do not invent
  one. Record final flags and declarations at comparison because some fields
  are filled after allocation. The enclosing production sort supplies its
  input/output context through a separate hook.
- Fallback events carry the branch, operand witness identities, IDs actually
  used, result sign and available source location. Run fresh single-threaded
  witness processes with identical action schedules. Compare semantic witness
  labels and resulting order, not raw numeric ID equality or the complete
  Go/Rust allocation sequence. In particular, lazy symbol-ID order must not be
  mislabeled allocation order.
- Keep the canonical native capture separate from the diagnostic overlay. If
  its inputs remain byte-identical, reuse it. If canonical inputs change, the
  old capture cannot pass `current()` by rerunning `verify`: take a fresh
  native capture, verify it and record provenance. Compare the complete
  contract `row_sha256` inventory with the previous capture; raw walker type
  IDs may differ as the existing native contract explicitly allows. Do not overwrite
  the C2 baseline to erase that change. If a new canonical capture is needed,
  the producer must authenticate both captures and their complete contract-row
  equality before associating the historical baseline with the new native
  identity; absent that explicit verified association, the join stays blocked.
- Retain the S08 P3 comparator fixtures and union-ordering sub-test. They do
  not replace the source-program trace witnesses required by ADR 0010. The
  diagnostic mode must itself preserve each witness's ordinary outputs with
  tracing on and off.

- Exit: the limit cases pass in debug and release; the required residual cases have
  source-program witnesses, native/Rust traces and equal observed ordering;
  include reverse-mapped types, declaration-less duplicate names, equal-first-
  declaration duplicate names and intrinsic ordering, and classify every other
  final fallback reached; the E2 obligations replay (`scripts/s08_e2.py
  obligations`, which imports the P3 comparator) reports 8 families, 7
  matrices and 77 permutations.

### C2.11 Direct contracts

- Exists: the C1 contracts, the `relation-probe` feature, `relation_state()`
  and `builtin_type()` probes, the C1 test helpers (`program`, `checker`,
  `declaration`, `codes`).
- Build: `crates/tsr_compiler/tests/c2_contracts.rs`, one test per contract,
  each over production entry points with a pinned Go counterpart named in its
  doc comment:
  1. an inference context's fixing: a fixed inference is not changed by a
     later candidate, the non-fixing mapper leaves it unfixed, and a cloned
     context does not fix the original;
  2. instantiation identity: the same type arguments give the same type id
     through the object, alias, conditional and indexed-access keys; the alias
     key distinguishes explicit and defaulted arguments as the pin's does;
  3. each limit of C2.10 produces the pin's exact result and diagnostic;
  4. overload failure: the pin's `No overload matches this call` chain, its
     related information and the combined symbol, over a library overload set;
  5. contextual typing: a contextual type pushed for an argument is popped
     after an error, and a cached contextual type is not reused across
     inference passes;
  6. higher-order inference: the hoisted type parameters of a generic result
     signature, its permissive instantiation and the wildcard's lack of a
     default (the C1 trace of `genericCallInferenceConditionalType1`);
  7. reverse mapped inference over a homomorphic mapped type and over an
     array element;
  8. the ADR 0010 residual cases and the additional tie branches enumerated
     in C2.10, compared with native traces of the same source programs; trace
     completeness and unchanged ordinary results are asserted;
  9. depth and reuse: distinct source and target types prevent the identity
     shortcut, and an observed nested conditional/mapped instantiation reaches
     the actual 100-level instantiation limit (not merely a 100-node AST).
     Compare the result, full diagnostic and location with a recorded native
     observation. Assert execution on a grown segment of the E2 small stack
     and a successful later query in both debug and release; reuse the separate
     recursion instrumentation without enabling it in timed builds.
- Exit: `cargo test -p tsr_compiler --features recursion-probe,creation-trace
  --test c2_contracts` in debug and release; the receipt is recorded by the
  producer (C2.12).

### C2.12 Producer wiring and the measurement capture

- Exists: `scripts/phase2_producers.py` with the C0 metrics, the ratios and
  the six `c1_*` metrics; `sprints/P2B.toml` with `P2B-C2` waiting on
  `run.checker.c2_complete`; `[checkerbench]` in `status/runs.toml`.
- Build: the producer reads `data/phase2/c2-claims.json`,
  `data/phase2/c2-audit.json`, `data/phase2/c2-baseline.json.gz`, the
  contracts receipt (`observe --witness c2-contracts`, exact debug/release
  commands and test inventory bound to the complete dependency/assets closure),
  reusing the strengthened C1 receipt and tracker closure: compiler dependency
  and embedded assets, all workspace manifests/configuration, Cargo.lock and
  the toolchain. `cargo metadata` may derive dependencies but does not enumerate
  embedded assets. With the checkerbench identity, the producer emits
  `c2_open` (rows with a currently unmet domain not covered by a valid exclusion,
  independent of their stored open/closed label),
  `c2_handoffs` (rows with status `handed`, reported, not asserted),
  `c2_regressions` (against the C2 baseline, over the full denominator),
  `c2_failures` (all unattributed failures, plus failures traced to C2; only
  a current site- and observation-bound handoff or covering registered blocker
  can exclude a failure; any uncovered failure in a C2 claim counts even in
  the AST, arena or binder),
  `c2_blockers_open` (register entries owned by C2 at the exit),
  `c2_audit_complete`, `c2_contracts`, `c2_measured` (an independently verified checkerbench capture of the same
  relevant production sources and pin as the exit run; the corpus driver and
  benchmark are different executables and profiles, so their binary digests
  must not be equal. Validate each binary against its own build record, then
  join their common production source/configuration identity explicitly) and `c2_complete`. The
  `c1_*` code is generalized to a per-checkpoint helper rather than copied, so
  C3–C7 reuse it. All new authorities are added to `[checker]` `inputs` in
  `status/runs.toml`; the exit run writes `target/phase2/rust`.
- Build, blocker ownership: `phase2_blockers.owner_of` currently derives the
  owner from inventory checkpoints. Extend the builder to consume the same
  validated handoffs as the producer. Preserve the inventory owner and derive
  effective ownership per variant/cause/domain. Mixed-owner groups keep their
  unresolved C2 shares; a handoff for one row cannot transfer the entire
  operation. Bind by kind, operation, variant and domains, not shifting B IDs.
  Migrate C1's existing blocker references through that same stable identity.
  Rebuilding must preserve supported C5 transfers without manually editing the
  register or adding an unconditional operation-prefix override.

- Build, measurement: the owner captures `scripts/s08_checkerbench.py
  capture` on the quiet host at the C2 head, once; the plan's section 6 asks
  for elapsed time, retained bytes and peak RSS as different measures, so the
  C2 record reports the checkerbench elapsed ratio and footprint next to the
  E5/E6 peak-RSS figures with their capture dates, without converting one into
  another and without a threshold.
- Exit: `cargo xtask validate`; `phase2_producers.py checker` emits the nine
  metrics; `scripts/tests/test_phase2_c2.py` shows that an `open` row that
  differs keeps `c2_complete` false, that a `handed` row without a trace is
  rejected, that a C2-owned blocker left in the register keeps
  `c2_blockers_open` nonzero, and that changing each new input invalidates the
  recorded result.

## 5. Dependencies and owners

| Dependency | Owner | State for C2 |
| --- | --- | --- |
| C1's foundations (symbols, members, tuples, relations, variance state, limits) | C1 | review fixes and contract validation precede the C2 baseline; unresolved C1 work stays visible |
| C1's audit handoffs to C2 | C1 | explicit variance measurement handoff in `c1-audit.json` and the C1 record; C2.2 observes its named functions and cases |
| Narrowing and flow-sensitive operand typing behind `TS2365`, `TS2536` and the JS rows | C3 | handoff targets by trace (section 3); C3 can start its ordinary cases now and its generic-dependent cases after C2.4 |
| JSX and decorator refusals in the C4 rows that carry C2 families | C4 | not C2's exit set; C2 operators verified through them once C4 lifts the refusal |
| Declaration-emit diagnostics (TS40xx, TS9010) and node-builder output (B13, B14, B15, the factory panic) | C5 | handoff targets; C2 keeps B03 and B08 until the missing node-builder branches and retained type state establish their cause |
| The emit-order dependency (B09) | C5 emit resolver with Phase 3 emit | withholds three inventory-C2 rows; the cause stays with C5 emit resolver jointly with Phase 3 |
| The `packageDeduplicationDuplicateGlobals` walker error | C1 | metadata for discarded package dependencies was retained; the C1 review fix filters it at publication and C2.0 confirms the corpus row |
| The checkerbench capture on the quiet host, and the recording of `checker` | owner | C2.12 |
| Execution choices of section 9 | implementer | authorized by the instruction to proceed; new semantic divergences still require owner approval |

## 6. Delivery order

1. C2.0 the fresh gap map and the claims file, then C2.1 the audit and the
   attribution worksheet. Review each production slice against its pinned
   functions and affected rows before changing it; keep the remaining rows open.
   The full audit decides how much of section 4 is mapping work and must be
   complete at exit, but does not block an independently attributed refusal fix.
2. The refusals first, because an `unsupported` domain withholds every
   observation of its row: B04 and B10 (C2.7), B07 (C2.4), B03 (C2.3), B08
   (C2.5), B11 and B01's share (C2.8), B16 and B18 (C2.9), each with a direct
   case.
3. C2.3 and C2.4 together, since instantiation keys and inference contexts
   feed every other item; then C2.2.
4. C2.5, C2.6 and C2.7 in that order, rerunning the relation contract after
   each because their relater cases changed in C1.
5. C2.8 and C2.9; the handoffs to C3 and C5 confirmed as their traces land.
6. C2.10 and C2.11 grow alongside 2–5, one contract per item; C2.12 last, then
   the exit full run, the owner's measurement capture and the record.

Intermediate runs use the recorded 300-variant sample plus the claimed rows
(`--sample --case ...`). Full runs are C2.0, the exit, and whenever an
instantiation-key or inference-context change is broad enough that a sampled
regression cannot bound it. Changes inside already-differing rows are inspected
through `--previous`, and match-to-non-match transitions through the baseline.

## 7. Executable exit checks

`target/phase2/rust-c1/comparison.json` below is the preserved reviewed C1
report from C2.0. Use its actual saved path if a differently named capture was
created; do not move over an existing capture. Exit output must be a fresh
directory (or an explicitly resumed capture with identical inputs). The repaired
producer compares acceptance independently of optional previous-run history, so
`--previous --record` is now valid and keeps that history intact.

```sh
export PATH="$(mise where go)/bin:$PATH"
python3 scripts/phase2_inventory.py check
# Reuse current verified native output. Changed canonical inputs require a
# fresh native capture and verification, not just verification of old output.
python3 scripts/phase2_corpus.py run --native target/phase2/native --output target/phase2/rust
python3 scripts/phase2_compare.py report --native target/phase2/native --rust target/phase2/rust --previous target/phase2/rust-c1/comparison.json --record
python3 scripts/phase2_blockers.py build --native target/phase2/native --rust target/phase2/rust --record
python3 scripts/phase2_audit.py check --audit data/phase2/c2-audit.json
cargo test -p tsr_compiler --features recursion-probe,creation-trace --test c2_contracts --locked && cargo test -p tsr_compiler --features recursion-probe,creation-trace --test c2_contracts --locked --release
python3 scripts/phase2_producers.py observe --witness c2-contracts           # the receipt, with source binding
python3 scripts/s08_e2.py obligations --output target/s08/e2-c2               # the P3 comparator replay: 8 residual families, 7 matrices, 77 permutations
python3 scripts/s08_relater.py build  --output target/s08/relater-c2
python3 scripts/s08_relater.py parity --output target/s08/relater-c2         # 105/105, all_cases_match true, both implementations
python3 scripts/phase2_producers.py checker            # c2_open 0, c2_regressions 0, c2_failures 0, c2_blockers_open 0, c2_audit_complete, c2_contracts, c2_measured, c2_complete; c2_handoffs reported
python3 -m pytest scripts/tests/test_phase2_*.py -q
python3 scripts/checks.py fmt && python3 scripts/checks.py clippy
cargo xtask validate
# Assert current C2 metrics explicitly. Whole-sprint check P2B remains pending
# until C3-C7 are complete; a nonzero result is not a C2 implementation failure.
```

`scripts/s08_checkerbench.py capture` on the quiet host, `cargo xtask run
checker` and `cargo xtask status --record` are the owner's. `check P2B` cannot
pass before C7 by construction; the C2 assertion is the `P2B-C2` line of its
report and the metrics above. The E2 obligations replay runs against a
scratch output; the recorded `e2` run stays the owner's.

## 8. Evidence reuse rules

- The C0 native capture is reused only when its read-only current-input
  validator succeeds and its authenticated verification record is present.
  Creation tracing uses the separate diagnostic builder of C2.10. If any
  canonical input changes, take and verify a fresh canonical capture;
  re-running verification cannot make the old capture current. Authenticate
  complete contract-row equality before reusing the historical C2 baseline.
- A Rust capture supplies current acceptance only when replay succeeds
  against the captured executable and reports `source_stable: true` for the
  current source inputs. Historical replay may succeed with stale sources; that
  preserves a baseline, not current acceptance. Production source edits stale
  captures, which is why intermediate work runs the sample.
- The C2-start baseline is reused only while it names the current native
  observation and inventory digests, or the explicit authenticated native
  contract-equivalence association in C2.10 exists.
- Claims are per row and per cause; the claims file is the only record of
  C2's handoffs, and the producer counts from it. A handoff without its trace
  is rejected by the producer.
- A difference is accepted only through the divergence ledger (ADR 0004), per
  variant and metric, with hashes. C2 expects none.
- The relation contract, the S08 P3 comparator replay, the ownership schedules
  and E2 may go stale under the phase-end rule when C2 touches fingerprinted
  sources; their parity is rerun in section 7 and their records refreshed at
  the phase-end green-up.
- Mapping progress is reported by markers and the ledger, never as behavioral
  coverage; the corpus and the C2.11 contracts are the behavioral evidence.

## 9. Execution decisions

Implementation is authorized on the shared `phase2-c2-plan` branch. The
choices below preserve the accepted Phase 2 scope and Go authority; they do not
introduce divergence approvals or authorize a quiet-host benchmark during coding.

1. **Handoff rule.** C2 owns rows; a row whose remaining difference is traced
   to another checkpoint's cause is recorded as `handed` with the trace, is
   excluded from `c2_open` and counted in `c2_handoffs`. The alternative is to
   keep every C2-owned row in `c2_open` until C3 and C5 close them, which
   would make `c2_complete` depend on later checkpoints. Use the handoff
   rule, with the handoff list reviewed at the exit and unresolved attribution
   counted as open rather than treated as a handoff.
2. **Node-builder blockers.** B03 and B08 stay with C2 pending attribution
   of the missing display branches; B13, B14 and B15 (elision comments, the reparsed-module scope
   lookup) are re-owned to C5 at C2.1 with the trace. The actual refusing branches and retained type state decide the handoff;
   the bucket label alone is not evidence of a missing type model.
3. **Creation-trace mode.** Implement C2.10 with a fingerprinted diagnostic
   overlay and the checker cargo feature, observing actual ID fallback sites,
   separate birth/assignment events and the required source witnesses. The alternative is
   an owner-approved amendment to ADR 0010 that lets the comparator fixtures
   and the union-ordering sub-test replace it; the review recorded that
   fixtures alone do not discharge the requirement.
4. **Alias instantiation keys.** Port the pin's keying by unfilled type
   arguments (C2.3), with native witnesses establishing which cache identities and displayed alias
   arguments actually change for references that rely on defaults; the alternative is a
   recorded divergence, which the plan does not expect.
5. **Measurement.** `c2_measured` requires one checkerbench capture on the
   quiet host at the C2 head, recorded without a threshold, as the plan's
   section 6 asks; this capture remains the owner's quiet-host step, while the E5/E6 peak-RSS figures are reported from their
   last capture rather than re-measured for C2.
6. **Exit run.** `c2_complete` is computed only from a recorded full capture,
   as C1's was. Record C1/C2 evidence only when their respective current validators pass;
   do not certify historical C1 captures against amended source or infer C1
   completion from C2 parity alone.
