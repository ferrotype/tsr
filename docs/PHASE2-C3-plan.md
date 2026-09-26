# Phase 2 C3: flow and ordinary program semantics

Checkpoint C3 of the [Phase 2 plan](PHASE2-plan.md), written after the
[C2 plan](PHASE2-C2-plan.md) (PR #62, branch `phase2-c2-plan`) and its review
amendments, over the recorded C2 exit capture (`target/phase2/rust`: 12,647 of 13,432 rows match in every domain, regression 9,367 of 9,367; `P2B-C2` recorded complete with three emit-order rows handed to C5). Upstream is Corsa
`1f70213d4922b434345f639b441681e470c7cfc1`. C3 is production work in
`crates/tsr_checker`, paired with its witnesses, measured by the C0 contract
exactly as C1 and C2 are. The numbers below describe that capture; C3.0
refreshes them at the C3 head, after C2's changes.

## 1. Objective and exit

Finish narrowing, assignment and definite-assignment, reachability, returns,
generators and async, classes and inheritance, late-bound and computed members,
namespaces, module and alias interactions, and the JavaScript, JSDoc and
expando behavior of the pin, including its documented differences from
TypeScript. Include malformed inputs and option combinations. C3's evidence is
diagnostic order, spans, arguments, chains and related information, since most
of its families are observed through `errors` only. Retain everything S08, C1
and C2 implement.

C3 exits when all of the following hold on one full run at the C3 head,
recorded by the owner through `cargo xtask run checker`:

| Exit condition | Measured by |
| --- | --- |
| Current, full, authenticated and recorded capture with no harness errors | `inventory_frozen`, `native_verified`, `harness_valid`, `result_recorded` and `blockers_named` are true; prerequisites of `c3_complete` |
| Existing cases preserved: the S08 regression subset still matches completely | `run.checker.regression_parity == 1` (9,367 of 9,367) |
| The module-resolution sub-test keeps its parity | `run.checker.trace_parity == 1` over the 154 traced variants, 148 `.trace.json` baseline files (owner decision of 2026-09-25: Phase 2 owns the gate; a difference is a Phase 1 resolver defect, routed there, never a C3 fix in the checker) |
| No previously matching domain of any executed row becomes a non-match | `run.checker.c3_regressions == 0` against the authenticated C3-start row report `data/phase2/c3-baseline.json.gz` |
| Every C3-owned row meets all seven applicable domains or is withheld by a registered blocker of another owner (the 15 content-mapper rows, `blocked` and excluded from `c3_open`), and every row another checkpoint handed to C3 with a validated trace matches, or its remaining difference has a current validated handoff (section 3) | `run.checker.c3_open == 0` over `data/phase2/c3-claims.json`; `run.checker.c3_handoffs` reported |
| No unexplained production failure anywhere in a C3 claim, regardless of crate or module | `run.checker.c3_failures == 0` |
| Every blocker C3 owns in `data/phase2/blockers.json` is closed by an implementation or re-owned with the trace that names the owner; B06 stays Phase 5's over C3 rows | `run.checker.c3_blockers_open == 0` |
| The C3 function groups of section 4 are audited: each pinned function mapped, disposed as equivalent, or handed to a named later checkpoint; the C1 record's call-path audit of raw lookups is closed | `run.checker.c3_audit_complete == true` over `data/phase2/c3-audit.json` |
| The direct contracts of C3.8 pass in debug and release | `run.checker.c3_contracts == true`, from the recorded v2 receipt |

`run.checker.c3_complete` is the conjunction. It binds `P2B-C3` in
`sprints/P2B.toml`. No ratio threshold is introduced.

## 2. Starting point

Everything below exists and is consumed as is. C3 extends it; it re-derives
nothing.

| Asset | Where | What C3 takes from it |
| --- | --- | --- |
| The recorded C2 exit | `target/phase2/rust` (12,647 matches; C3 open rows 15, all withheld by the content-mapper blocker; C4 open 767; C2 open 3, all `handed` to C5 in `data/phase2/c2-claims.json`, 188 `closed`); the C2 record (`PHASE2-C2.md`) with `P2B-C2` recorded complete | the row state C3.0 re-measures at the C3 head; no C2 handoff names C3 |
| The harness the review repaired | `phase2_compare.py baseline` and `regressions`; the v2 receipt (`observe --witness`, both exact Cargo commands, the full dependency, bundled-asset and generator closure); read-only replay; `phase2_audit.py check --audit PATH` with its reviewed scope binding; the per-checkpoint metric helper C2.12 generalizes from `c1_metrics` | every C3 authority follows it; C3.9 consumes the helper, or lands it if C3 starts first |
| The C3 modules | `flow.rs`, `flow_arrays.rs`, `flow_assignments.rs`, `flow_destructuring.rs`, `flow_discriminant.rs`, `flow_effects.rs`, `flow_equality.rs`, `flow_facts.rs`, `flow_initial.rs`, `flow_instanceof.rs`, `flow_narrow.rs`, `flow_predicates.rs`, `flow_reference.rs`, `flow_switch.rs`, `flow_symbol.rs`, `narrowable_references.rs`, `type_facts.rs`, `truthiness.rs`, `unreachable.rs`, `check_statements.rs`, `check_bodies.rs`, `generators.rs`, `iteration.rs`, `iteration_protocol.rs`, `await_expressions.rs`, `promises.rs`, `classes.rs`, `class_check.rs`, `class_members.rs`, `class_properties.rs`, `class_property_flow.rs`, `class_overrides.rs`, `class_accessibility.rs`, `class_context.rs`, `class_expressions.rs`, `class_grammar.rs`, `constructor_checks.rs`, `late_members.rs`, `late_indexes.rs`, `module_aliases.rs`, `module_alias_like.rs`, `module_augmentations.rs`, `module_exports.rs`, `module_specifiers*.rs`, `module_wrappers.rs`, `source_alias_checks.rs`, `source_imports.rs`, `source_module_aliases.rs`, `source_modules.rs`, `external_aliases.rs`, `external_resolution*.rs`, `name_resolution.rs`, `name_scopes.rs`, `name_qualified.rs`, `name_errors.rs`, `jsdoc_checks.rs`, `jsdoc_types.rs`, `signature_jsdoc.rs`, `assignment_declarations.rs`, `grammar_lists.rs`, `grammar_modifiers.rs`, `grammar_variables.rs`, `check_type_syntax.rs`, `type_only_uses.rs`, `unused_identifiers.rs` | the production code to retain and complete; `flow.go` is 97 of 130 marked, `checker.go` 1,042 of 1,505, `grammarchecks.go` 52 of 78, `utilities.go` 55 of 150, `jsdoc.go` 2 of 2 (tracker counts at the C2 exit) |
| Retained evidence | the regression subset by suite: compiler 4,707, es6 918, parser 777, classes 473, types 338, node 318, expressions 283, jsdoc 255, externalModules 214, salsa 185, statements 140, moduleResolution 71, async 68, internalModules 59, controlFlow 46, es7 44, interfaces 40, override 37, dynamicImport 31, importDefer 26, ambient 21, importAttributes 17; 817 rows with `allowJs`, 686 with `checkJs`; all matching | C3's families are already exercised at scale; a C3 change is measured first against this subset through the sample |
| The C1 review record's open items | `PHASE2-C1.md`: the raw `lookup_symbol` call-path audit (iteration, JSDoc, flow and ordinary expression sites; only the resolve-name and lookup wrappers retry the pending-alias sentinel); the project-reference callers (referenced-module format in `canHaveSyntheticDefault`, TS6305 for missing reference output, the cross-project rewrite checks in `resolveExternalModuleNameWorker`) | C3.1 and C3.5 obligations, named there as C3 work |
| Option handling | `crates/tsr_compiler/src/verify_options.rs`, `checker_config_diagnostics.rs`, `plain_js_errors.rs`; `tests/checker_config_diagnostics.rs`, `verify_options.rs`, `config_source_parity.rs` | the option-dependent checks C3.7 completes |
| Direct tests | `crates/tsr_compiler/tests/c1_contracts.rs` (feature `recursion-probe`), `c1_review_regressions.rs`, `c1_package_metadata.rs`, `checker_semantics.rs`, `checker_dynamic_imports.rs`, `checker_project_references.rs`; the `tsr_checker` unit tests | the homes C3.8 adds to |
| Pin tracing | the harness overlay build and one-row shard accept a diagnostic Go overlay (`-overlay`, fingerprinted under its build output); `upstream/` is never edited | the attribution method of C3.1 |

The C3-start capture is a fresh named output directory; no existing capture is
moved or overwritten. The producer reads an explicit capture path, or the
default `target/phase2/rust` only after a deliberate promotion.

## 3. What C3 owns

The inventory's checkpoint rule assigns C3 the executed rows with no
type-level, JSX or decorator family: 185 rows, 170 of them emitted-output-only
(175 run with `NoTypesAndSymbols`, so `types`, `symbols` and `display` are
native-disabled and `errors`, `union_ordering` and `parent_pointers` are
compared) and 15 content-mapper rows. By suite: compiler 67, statements 60,
expressions 25, classes 24, es6 4, externalModules 3, functions 1, generators
1. At the reviewed head all 170 emitted-output-only rows match, and the 15
content-mapper rows are withheld by B06 (`content-mapper execution`, owner
Phase 5, refused in `tsr_compiler/src/loader.rs`).

C3's row ownership is therefore thin; its substance is the causes other
checkpoints hand to it and the audit of the C3 areas. The C3 exit set is:

| Source | Rows | State at the C3 start |
| --- | ---: | --- |
| C3-owned rows withheld by B06 | 15 | `blocked`; Phase 5 owns the operation; C3 keeps them visible and never marks them closed |
| Rows C2 hands to C3 | 0 | C2 closed every row the C2 plan had listed as a C3 candidate (operand display, node16 `import type`, bigint property names, super-call placement, JS function typing, CommonJS `export=`) before its exit; its only handoffs are the three emit-order rows to C5 |
| C4 rows whose only remaining difference is a class, alias or JS cause after C4 lifts a refusal | 0 today | by the same rule in reverse |

`data/phase2/c3-claims.json` records every C3-owned row that is not matching
(`blocked` rows with the blocker's stable identity: kind, operation, variant,
domains) and, under `incoming`, every row another checkpoint handed to C3: the
handing checkpoint's claims entry, its trace digest and the pinned Go function
it named. An incoming row counts in `c3_open` while it differs, whatever its
inventory owner; when it matches, the handing checkpoint's producer sees its
`handed` row match and C3 records the closing commit. A C3 handoff onward uses
the checkpoint validator's fields of the C2 plan (target checkpoint and pinned
function id, reproduction argv, capture and request identities, trace artifact
path and digest, full raw observation digest with fatal class and
`panic_location`, covered domains, cause), and the trace must cover every
currently nonmatching domain. Status labels never exempt a current difference.

Blockers: C3 owns none at the start. B06 stays with Phase 5. A C3 refusal that
appears while lifting an incoming cause is registered under C3 by its stable
identity and closed before the exit.

The seven domains are `errors`, `types`, `symbols`, `display`, `trace`,
`union_ordering` and `parent_pointers`; native-disabled domains are met.
Declaration diagnostics are part of `errors`.

**Completion and handoffs across captures.** Every handoff and the C2
measurement are bound to one Rust capture: the blocker builder's
`validated_handoffs` drops a handoff whose `capture_sha256` differs from the
current comparison's, and `c2_measured` binds the checkerbench record to the
corpus capture it was verified against. Two rules follow, and every plan from
C3 on uses them. First, a checkpoint's completion is a recorded historical
fact: `P2B-Cn` closes on the `checker` run recorded at that checkpoint's exit,
and no later checkpoint recomputes `cN_complete` or `cN_measured` on its own
capture; C7.7 extends the tracker so that an item's `done_when` can name a
recorded run (`recorded.checker.cN_complete == true`, with the evidence
identity) instead of the current one. Second, at every later checkpoint's item
0, `phase2_claims.py rebind` re-validates each open handoff against the fresh
capture: it re-runs the row's reproduction, checks that the new raw observation
still differs only in the covered domains, and rewrites the capture, request,
observation and trace digests in both the handing checkpoint's claims file and
the receiving checkpoint's `incoming` entry; a handoff that no longer holds is
reported and its row counts as open for the receiving checkpoint. C2.12 is
complete without a rebind step; C3.9 lands it in the per-checkpoint helper.

## 4. Work items

Each item names what exists, what to build, its artifact and its executable
exit check. Functions are named by their pinned Go names
(`tsc/internal/checker`); `port:` markers and the ledger stay the mapping
authority. Names that C1's audit or the C2 plan's C2.1 already carry stay with
those checkpoints and are not re-listed.

### C3.0 Refresh the gap map at the C3 head

- Exists: the recorded C2 exit capture; `phase2_compare.py baseline`; the C2
  claims file.
- Build: check the native capture with its read-only freshness validator; run
  one full Rust contract at the C3 head into a fresh directory; compare
  against the latest recorded report and explain every change; freeze the
  authenticated result into `data/phase2/c3-baseline.json.gz`; write
  `data/phase2/c3-claims.json` with the 15 `blocked` rows and the `incoming`
  rows any checkpoint has recorded as `handed` to C3 by then, rebound to
  this capture.
- Exit: 0 harness errors, 0 regressions, an explained per-row delta, the
  baseline committed; the claims file names every non-matching C3 row and
  every incoming row exactly once.

### C3.1 Audit and attribution

- Exists: `scripts/phase2_audit.py` with the reviewed C1 scope binding and the
  C2 scope binding C2.1 adds; the C1 record's open call-path audit;
  `status/unmapped-functions.json`.
- Build: `data/phase2/c3-audit.json`, with a separately reviewed C3 scope
  binding (extend the validator, never weaken the C1 or C2 checks), over these
  groups. Group members are taken from the tracker's unmapped list at the C3
  start (`phase2_audit.py worksheet --unmapped`), filtered to the areas below;
  the names listed are the C2-exit inventory of those areas, and a name that
  is mapped by then is not in the group:
  - `flow.go`, the complete file (130 functions, 33 in the tracker's unmapped list: `getFlowState`, `putFlowState`, `getFlowNodeOfNode`,
    `getFlowTypeOfReference`, `getBranchLabelAntecedents`,
    `getTypeAtFlowAssignment`, `getTypeAtFlowCall`, `getTypeAtFlowCondition`,
    `getTypeAtFlowBranchLabel`, `narrowTypeByOptionality`,
    `narrowTypeByBinaryExpression`, `narrowTypeByTypeof`,
    `narrowTypeByTypeName`, `narrowTypeByBooleanComparison`,
    `narrowTypeBySwitchOptionalChainContainment`,
    `narrowTypeBySwitchOnDiscriminantProperty`,
    `getElementTypeOfEvolvingArrayType`, `isEvolvingArrayTypeList`,
    `tryGetNameFromType`, `getLiteralPropertyNameText`,
    `isOrContainsMatchingReference`, `isCoercibleUnderDoubleEquals`,
    `eachTypeContainedIn`, `isDeclarationWithExplicitTypeAnnotation`,
    `isExpandoPropertyFunctionWithReturnTypeAnnotation`,
    `getAssignedTypeOfBinaryExpression`, `getAssignedTypeOfArrayLiteralElement`,
    `getAssignedTypeOfSpreadExpression`, `getAssignedTypeOfPropertyAssignment`,
    `getAssignedTypeOfShorthandPropertyAssignment`,
    `getAssignmentReducedTypeWorker`, `typeMaybeAssignableTo`, `isNil`);
  - `checker.go` classes, `this` and members: `checkClassStaticBlockDeclaration`,
    `isInstancePropertyWithInitializerOrPrivateIdentifierProperty`,
    `superCallIsRootLevelInConstructor`, `checkClassForStaticPropertyNameConflicts`,
    `getClassOrInterfaceDeclarationsOfSymbol`, `checkBaseTypeAccessibility`,
    `isPropertyAbstractOrInterface`, `checkMemberForOverrideModifier`,
    `getMemberOverrideModifierStatus`, `isPropertyWithoutInitializer`,
    `checkClassNameCollisionWithObject`, `isPropertyDeclaredInAncestorClass`,
    `isPropertyInClassDerivedFrom`, `resolveClassOrInterfaceMembers`, `resolveTypeReferenceMembers`,
    `getParentTypeOfClassElement`, `getClassElementPropertyKeyType`,
    `isPrototypeProperty`, `checkThisType`,
    `getThisContainer`, `getThisParameterFromNodeContext`, `tryGetThisTypeAt`,
    `TryGetThisTypeAtEx`, `isThisless`, `isThislessFunctionLikeDeclaration`,
    `combineUnionOrIntersectionThisParam`, `isThisPropertyAndThisTyped`,
    `getAnnotatedAccessorThisParameter`, `getEffectiveSetAccessorTypeAnnotationNode`,
    `getReturnTypeOfFullSignature`, `isUnwrappedReturnTypeUndefinedVoidOrAny`, `checkPropertyAccessChain`,
    `checkPropertyAccessibility`, `checkPropertyAccessibilityEx`,
    `getPrivateIdentifierPropertyOfType`, `getSuggestedSymbolForNonexistentProperty`,
    `symbolHasNonMethodDeclaration`, `isUncalledFunctionReference`,
    `isOptionalPropertyDeclaration`, `getThisArgumentOfCall`,
    `getThisArgumentType`, `hasNumericPropertyNames`, `getConstituentProperty`,
    `isSpreadIntoCallOrNew`, `checkMetaPropertyKeyword`, `checkBindingElement`,
    `getPropertyNameFromBindingElement`, `getPropertyOfVariable`,
    `isImmediatelyUsedInInitializerOfBlockScopedVariable`,
    `isES2015OrLaterConstructorName`, `getTypeOfVariableOrParameterOrProperty`, its worker,
    `getTypeOfFuncClassEnumModule`, `isGlobalSymbolConstructor`, `newProperty`,
    `addDuplicateDeclarationErrorsForSymbols`, `getFirstDeclaration`,
    `errorSkippedOnNoEmit`, `IsDeprecatedDeclaration`,
    `getReferencedValueOrAliasSymbol`, `removeDefinitelyFalsyTypes`,
    `getDefinitelyFalsyPartOfType`, `allTypesAssignableToKindEx`,
    `isTypeAssignableToKind`, `isConstEnumSymbol`,
    `getPropertyNameFromIndex`,
    `getDeclarationNodeFlagsFromSymbol`;
  - `checker.go` iteration, async and generators: `getIterationTypeOfIterable`,
    `isReferenceToType`, `isReferenceToSomeType`, `getBuiltinIteratorReturnType`,
    `combineIterationTypes`, `getIterationTypeUnion`,
    `getIterationTypesOfIterator`, `getIterationTypesOfIteratorSlow`,
    `isYieldIteratorResult`, `isReturnIteratorResult`;
  - `checker.go` namespaces, modules and aliases:
    `checkAndReportErrorForUsingTypeAsNamespace`,
    `checkAndReportErrorForExportingPrimitiveType`,
    `checkAndReportErrorForUsingNamespaceAsTypeOrValue`,
    `getFirstNonAmbientClassOrFunctionDeclaration`, `getIsolatedModulesLikeFlagName`,
    `getVerbatimModuleSyntaxErrorMessage`, `hasShadowedNamespace`,
    `getTypeOnlyDeclarationOfEntityName`,
    `getEmitSyntaxForModuleSpecifierExpression`, `reportNonExportedMember`,
    `getTargetOfExportSpecifier`, `getTargetOfExportAssignment`,
    `getModuleSpecifierFromNode`, `markSymbolOfAliasDeclarationIfTypeOnly`,
    `getCannotResolveModuleNameErrorForSpecificModule`, `errorOnImplicitAnyModule`,
    `GetAmbientModules`, `ResolveAlias`, `resolveIndirectionAlias`,
    `resolveAliasWithDeprecationCheck`, `isExportOrExportExpression`,
    `shouldMarkIdentifierAliasReferenced`, `markIdentifierAliasReferenced`,
    `markPropertyAliasReferenced`, `markExportAssignmentAliasReferenced`,
    `markExportSpecifierAliasReferenced`, `markLinkedReferences`,
    `markEntityNameOrEntityExpressionAsReference`, `getEntityNameFromTypeNode`,
    `markTypeNodeAsReferenced`, `checkCollisionWithRequireExportsInGeneratedCode`
    (the JSX and decorator marking entries are C4's; the import-equals entries
    are C2's);
  - `checker.go` JS and JSDoc: `checkJSDocComment`, `checkJSDocTypeIsInJsFile`
    and the JS assigned-type entries of `flow.go` above (`jsdoc.go` is fully
    marked);
  - `grammarchecks.go`, the C3 share (`getIdentifierFromEntityNameExpression`,
    `checkGrammarExportDeclaration`, `reportObviousModifierErrors`,
    `findFirstModifierExcept`, `checkGrammarClassLikeDeclaration`,
    `checkGrammarArrowFunction`, `checkGrammarIndexSignature`,
    `checkGrammarForAtLeastOneTypeArgument`,
    `checkGrammarExpressionWithTypeArguments`, `checkGrammarForInvalidQuestionMark`,
    `checkGrammarForInvalidExclamationToken`, `doesAccessorHaveCorrectParameterCount`,
    `checkGrammarYieldExpression`, `checkGrammarConstructorTypeParameters`,
    `checkGrammarConstructorTypeAnnotation`,
    `isInitializerStringOrNumberLiteralExpression`,
    `isInitializerBigIntLiteralExpression`,
    `checkGrammarTopLevelElementForRequiredDeclareModifier`,
    `checkGrammarTypeOnlyNamedImportsOrExports`; the decorator and JSX
    grammar entries are C4's, `checkGrammarMappedType` is C2's);
  - `utilities.go`, the complete file (150 functions, 55 marked, 95 in the
    tracker's unmapped list): mostly syntactic predicates, operator
    classifiers and modifier helpers that Rust implements in `tsr_ast` or
    inline; disposed as `equivalent` with the Rust site. The ADR 0010
    comparators are S08 and C2 work and already carry their markers except
    `getObjectTypeName`, which C3 disposes with them.
- Build, second half: the C1 record's call-path audit of every raw
  `lookup_symbol` site in iteration, JSDoc, flow and ordinary expression
  checking: each site either cannot observe the pending-alias sentinel (shown
  by the caller's resolution path) or retries as the two wrappers do, with a
  direct case per changed site; the attribution of every incoming row by
  reproduction and, where needed, a pin trace through the diagnostic overlay.
- Exit: `phase2_audit.py check --audit data/phase2/c3-audit.json` passes with
  no `gap`; every incoming row has a named cause; the lookup audit is recorded
  in the C3 record with its per-site disposition.

### C3.2 Narrowing, assignment and definite assignment

- Exists: the fifteen `flow_*.rs` modules, `narrowable_references.rs`,
  `type_facts.rs`, `truthiness.rs`, `class_property_flow.rs`; the C1 fix that
  narrows a for-in expression only when nullable.
- Build: the 34 unmarked `flow.go` functions, each disposed or ported;
  evolving array types and their element types; discriminant narrowing through
  optional chains and `switch`; `typeof` and type-name narrowing; boolean
  comparison narrowing; coercion under `==`; `typeMaybeAssignableTo` in the
  assignment-reduced type; the JS assigned types of binary expressions, array
  elements, spreads and property assignments; explicit annotation rules for
  declarations and expando functions; definite assignment across loops, labels,
  `try`/`finally` and class property initialization order; flow calls to
  never-returning functions (`getTypeAtFlowCall`) and their reachability
  effect; the `TS2365` operand display incoming from C2 (literal against base
  type). The pin's flow-state pooling (`getFlowState`/`putFlowState`) is an
  allocation strategy; Rust's checker-local flow state is its `equivalent` and
  a contract shows independence across checkers.
- Exit: the incoming rows match; the regression subset's controlFlow, salsa
  and expressions suites still match on the sample; a direct case per
  narrowing kind that the corpus observes only through `errors`.

### C3.3 Statements, reachability, returns, generators and async

- Exists: `check_statements.rs`, `check_bodies.rs`, `unreachable.rs`,
  `generators.rs`, `iteration.rs`, `iteration_protocol.rs`,
  `await_expressions.rs`, `promises.rs`; the C1 fix that takes iteration type
  arguments from the first three parameters.
- Build: the iteration group of C3.1 (builtin iterator return types, iterator
  result classification, slow-path iteration types, the union of iteration
  types); unreachable-code and implicit-return diagnostics with the pin's
  spans; `yield` and `await` in every container the corpus exercises,
  including static blocks and default parameters (`asyncArrowStaticFieldThis`,
  `asyncSuperDefaultParameters` are C3 rows); generator return and `next`
  types; the C3 rows in the statements, expressions and generators suites are
  the retained evidence and get no new claims unless C3.0 shows a difference.
- Exit: the claimed rows match; a direct case per iteration protocol path
  (sync, async, `for await`, spread, destructuring, `yield*`) against a native
  observation.

### C3.4 Classes, inheritance and late-bound members

- Exists: `classes.rs`, `class_check.rs`, `class_members.rs`,
  `class_properties.rs`, `class_overrides.rs`, `class_accessibility.rs`,
  `class_context.rs`, `class_expressions.rs`, `class_grammar.rs`,
  `constructor_checks.rs`, `late_members.rs`, `late_indexes.rs`,
  `private_access.rs`; the C1 fix for inherited members replacing type-only
  entries.
- Build: the classes, `this` and members group of C3.1; super-call placement
  (`TS2337`, incoming from C2); static blocks (the static-block branch of
  `check.rs` already visits children); override and abstract checks; accessor
  pairs; private names across nested classes; base-type accessibility;
  property initialization and `useDefineForClassFields`; late-bound and
  computed members with `unique symbol` keys and their index signatures;
  `this` types in members, accessors and static contexts. `super` calls with
  type arguments (`TS2554`, `TS2558`) stay C2's unless C2's trace lands here.
- Exit: the incoming rows match; the regression subset's classes, override and
  interfaces suites still match on the sample; a direct case per class check
  the corpus observes only through `errors`.

### C3.5 Namespaces, modules, aliases and project references

- Exists: the `module_*.rs`, `source_*.rs`, `external_*.rs` and `name_*.rs`
  modules, `type_only_uses.rs`, `unused_identifiers.rs`; the five
  project-reference accessor equivalents C1 marked; the C1 review's package
  metadata fix; the module-resolution sub-test at parity.
- Build: the namespaces, modules and aliases group of C3.1; the incoming
  `TS7060` (node16 `import type` syntax checking by module option), `TS1539`
  (bigint property names), `TS2315` (CommonJS `export=` of a generic type)
  rows; alias marking as checker semantics (which declarations are marked
  referenced, type-only or value aliases, `verbatimModuleSyntax` and
  `isolatedModules` errors), with the resolver's queries over that state
  staying C5's; the remaining project-reference callers the C1 record names
  (referenced-module format in `canHaveSyntheticDefault`, `TS6305` for a
  missing reference output, the cross-project rewrite checks in
  `resolveExternalModuleNameWorker`); `GetAmbientModules` and the implicit-any
  module error. A trace difference in the 154 traced variants (148 `.trace.json` baseline files) is a Phase 1
  resolver defect and is reported to Phase 1 with the row, never patched in
  the checker.
- Exit: the incoming rows match; `trace_parity` stays 1; a direct case per
  alias kind (`import =`, `export =`, namespace re-export, type-only, default
  interop under each `module` and `esModuleInterop` setting the corpus uses)
  against a native observation; the project-reference callers have cases in
  `checker_project_references.rs`.

### C3.6 JavaScript, JSDoc and expando behavior

- Exists: `jsdoc_checks.rs`, `jsdoc_types.rs`, `signature_jsdoc.rs`,
  `assignment_declarations.rs`, `flow_effects.rs` (expando members),
  `object_members.rs`; 817 `allowJs` and 686 `checkJs` regression rows
  matching; `plain_js_errors.rs` in `tsr_compiler`.
- Build: `checkJSDocComment` and `checkJSDocTypeIsInJsFile`; the JS
  assigned-type and expando entries of `flow.go`; the incoming `TS2349` and
  `TS2355` rows (JS function typing); the pin's documented JavaScript
  differences from TypeScript (upstream `CHANGES.md`): each documented item
  gets a direct case that asserts the Corsa behavior, not the TypeScript one;
  JSDoc `@type`, `@param`, `@typedef`, `@import`, `@satisfies` and tag
  resolution across files; CommonJS `module.exports` and `require` shapes the
  salsa suite exercises. The import-type JSDoc rows (B11) are C2's.
- Exit: the incoming rows match; the salsa and jsdoc suites still match on the
  sample; every `CHANGES.md` JavaScript item has a native case.

### C3.7 Grammar, malformed inputs and option combinations

- Exists: `grammar_lists.rs`, `grammar_modifiers.rs`, `grammar_variables.rs`,
  `check_type_syntax.rs`, `object_grammar.rs`, `regular_expressions.rs`;
  `verify_options.rs` and `checker_config_diagnostics.rs` in `tsr_compiler`;
  the parser's recovery already produces the corpus's malformed trees.
- Build: the C3 share of `grammarchecks.go` in C3.1; checking over files with
  syntax errors produces the pin's semantic diagnostics in the pin's order and
  the checker answers later queries (malformed inputs are a lifecycle
  contract, not only a diagnostic one); option-dependent checks the corpus
  configurations exercise: the `strict` family, `exactOptionalPropertyTypes`,
  `noUncheckedIndexedAccess`, `useUnknownInCatchVariables`, `noImplicit*`,
  `noPropertyAccessFromIndexSignature`, `module`/`target`/`moduleResolution`
  combinations, `isolatedModules`, `verbatimModuleSyntax`, `allowJs`/`checkJs`,
  `noEmit` interactions with `errorSkippedOnNoEmit`. The 1,720 option-guard
  skips of the inventory use options the pin does not implement (`baseUrl` 38
  rows, `moduleSuffixes` 15 rows among the guard-only keys) and stay
  informational; C3 adds no rows for them.
- Exit: a direct case per option the pin checks and the corpus observes only
  through `errors`; the malformed-input lifecycle contract of C3.8 passes.

### C3.8 Direct contracts

- Exists: the C1 and C2 contracts, the `recursion-probe` feature, the C1 test
  helpers.
- Build: `crates/tsr_compiler/tests/c3_contracts.rs`, one test per contract,
  each over production entry points with a pinned Go counterpart named in its
  doc comment:
  1. flow state independence: two checkers over one program narrow the same
     reference independently, and a checker's flow caches do not survive
     retirement;
  2. definite assignment across a loop, a label, `try`/`finally` and a class
     property initializer, with the pin's diagnostics and spans;
  3. evolving arrays and discriminated unions through optional chains and
     `switch`, compared with a native observation;
  4. iteration types over the bundled lib's `Iterator`, `AsyncIterator`,
     `Map` and generator objects, for sync and async `for`, spread and
     `yield*`;
  5. class checks: override, abstract, accessor pairs, private names, static
     blocks and `this` types, each with the pin's diagnostic;
  6. alias state: a type-only import, a value alias and a re-export are
     marked as the pin marks them, observed through the checker's alias links
     (the referenced, type-only and value-alias state the resolver will read);
     the resolver's `IsReferencedAliasDeclaration` and `IsValueAliasDeclaration`
     have no Rust entry point yet, so the resolver half of this witness is
     C5.6's and not a C3 exit condition;
  7. JavaScript: expando members, `module.exports` shapes and one
     `CHANGES.md` difference, each against a native observation;
  8. malformed input: a file with parse errors is checked to completion, its
     semantic diagnostics equal the native set, and the same checker answers
     a later query;
  9. deep flow graphs: `binderBinaryExpressionStress` (a C3 row) checks on
     the E2 small stack with the recursion observer showing growth, in debug
     and release.
- Exit: `cargo test -p tsr_compiler --features recursion-probe --test
  c3_contracts` in debug and release; the v2 receipt is recorded by the
  producer (C3.9).

### C3.9 Producer wiring

- Exists: the per-checkpoint metric helper of C2.12 (or `c1_metrics` if C2.12
  has not landed); `sprints/P2B.toml` with `P2B-C3` waiting on
  `run.checker.c3_complete`; `[checker]` `inputs` in `status/runs.toml`.
- Build: the producer reads `data/phase2/c3-claims.json` (rows and
  `incoming`), `data/phase2/c3-audit.json`, `data/phase2/c3-baseline.json.gz`
  and the contracts receipt (`observe --witness c3-contracts`), and emits
  `c3_open` (C3-owned rows with a currently unmet domain not covered by a
  valid exclusion, plus incoming rows that still differ), `c3_handoffs`,
  `c3_regressions`, `c3_failures` (every unattributed failure plus failures
  traced to C3, in any crate), `c3_blockers_open`, `c3_audit_complete`,
  `c3_contracts` and `c3_complete` (which also requires `regression_parity ==
  1` and `trace_parity == 1`). If C3 starts before C2.12 lands, C3.9 lands the
  per-checkpoint helper and C2.12 consumes it. All new authorities are added
  to `[checker]` `inputs`.
- Exit: `cargo xtask validate`; `phase2_producers.py checker` emits the eight
  metrics; `scripts/tests/test_phase2_c3.py` shows that a differing incoming
  row keeps `c3_complete` false, that a `blocked` row needs a registered
  blocker of another owner, that a handoff without its trace is rejected, and
  that changing each new input invalidates the recorded result.

## 5. Dependencies and owners

| Dependency | Owner | State for C3 |
| --- | --- | --- |
| Symbol, member and relation foundations | C1 | delivered and reviewed; unresolved C1 items stay visible |
| Contextual typing, inference and calls behind the generic-dependent cases (super calls with type arguments, generic class members) | C2 | C3's ordinary cases start now; generic-dependent cases after C2.4 and C2.9 |
| Rows handed to C3 | C2, C4 | none at the C3 start (C2 closed its C3 candidates); a later handoff arrives through `incoming` after the rebind of section 3 |
| Class components and decorated classes | C4 | C4 consumes C3.4; a C4 row with only a C3 cause left comes to C3 by the reverse rule |
| The resolver's alias and visibility queries over C3's marking state | C5 | C3 marks, C5 queries; C5.6's entry points are not a C3 prerequisite: contract 6 observes checker state, and C5.6 re-observes it through the resolver |
| Content-mapper execution (B06, 15 C3 rows) | Phase 5 | withheld; C3 keeps the rows `blocked` |
| Module-resolution trace differences | Phase 1 | none today; any difference is routed there |
| The `checker` recording | owner | C3.9 |
| Owner decisions of section 9 | owner | before C3.1 |

## 6. Delivery order

1. C3.0 the fresh gap map and claims file; C3.1 the audit, the lookup
   call-path audit and the attribution of the incoming rows, reviewed before
   production changes.
2. C3.5, because the incoming rows cluster on modules, aliases and options,
   and because alias marking is C5's input.
3. C3.2 and C3.3 together (flow feeds reachability and iteration), then C3.4.
4. C3.6 and C3.7, with the `CHANGES.md` and option cases.
5. C3.8 grows alongside 2–4, one contract per item; C3.9 last, then the exit
   full run and the record.

Intermediate runs use the recorded 300-variant sample (30 C3 rows, 100
regression rows) plus the incoming rows (`--sample --case ...`). Full runs are
C3.0 and the exit. Changes inside already-differing rows are inspected through
`--previous`, and match-to-non-match transitions through the baseline.

## 7. Executable exit checks

`target/phase2/rust-c3-start/comparison.json` below is the C3-start report
from C3.0; use its actual saved path. Exit output is a fresh directory (or an
explicitly resumed capture with identical inputs). Nothing is moved over an
existing capture.

```sh
export PATH="$(mise where go)/bin:$PATH"
python3 scripts/phase2_inventory.py check
python3 scripts/phase2_corpus.py run --native target/phase2/native --output target/phase2/rust-c3
python3 scripts/phase2_compare.py report --native target/phase2/native --rust target/phase2/rust-c3 --previous target/phase2/rust-c3-start/comparison.json --record
python3 scripts/phase2_blockers.py build --native target/phase2/native --rust target/phase2/rust-c3 --record
python3 scripts/phase2_audit.py check --audit data/phase2/c3-audit.json
cargo test -p tsr_compiler --features recursion-probe --test c3_contracts --locked && cargo test -p tsr_compiler --features recursion-probe --test c3_contracts --locked --release
python3 scripts/phase2_producers.py observe --witness c3-contracts           # the v2 receipt, with the dependency and asset closure
python3 scripts/s08_relater.py build  --output target/s08/relater-c3
python3 scripts/s08_relater.py parity --output target/s08/relater-c3         # 105/105, all_cases_match true, both implementations
python3 scripts/phase2_producers.py checker --rust target/phase2/rust-c3    # c3_open 0, c3_regressions 0, c3_failures 0, c3_blockers_open 0, c3_audit_complete, c3_contracts, c3_complete; trace_parity 1; c3_handoffs reported
python3 -m pytest scripts/tests/test_phase2_*.py -q
python3 scripts/checks.py fmt && python3 scripts/checks.py clippy
cargo xtask validate
# Assert the C3 metrics explicitly; `check P2B` stays pending until C7.
```

`cargo xtask run checker` and `cargo xtask status --record` are the owner's.
The producer's `--rust` path names the exit capture explicitly if the default
is not promoted.

## 8. Evidence reuse rules

- The C0 native capture is reused only when its read-only current-input
  validator succeeds and its verification record is present; a changed
  canonical input requires a fresh, verified capture.
- A Rust capture supplies current acceptance only when replay succeeds against
  the captured executable with `source_stable: true`; production edits stale
  it, which is why intermediate work runs the sample.
- The C3-start baseline is reused only while it names the current native
  observation and inventory digests.
- Claims are per row and per cause; `incoming` rows are counted from the
  handing checkpoint's validated entries; the producer rejects a handoff or an
  incoming reference without its trace.
- A difference is accepted only through the divergence ledger (ADR 0004), per
  variant and metric, with hashes. C3 expects none; the pin's documented
  JavaScript differences are pin behavior, not divergences.
- The relation contract, the ownership schedules and E2 may go stale under the
  phase-end rule when C3 touches fingerprinted sources; their parity is rerun
  in section 7 and their records refreshed at the phase-end green-up.
- Mapping progress is reported by markers and the ledger, never as behavioral
  coverage; the corpus, the regression subset and the C3.8 contracts are the
  behavioral evidence.

## 9. Owner decisions before C3 starts

1. **Incoming rows.** A row another checkpoint hands to C3 with a validated
   trace counts in `c3_open` while it differs, whatever its inventory owner.
   The alternative is to count only C3-owned rows, which would make C3's exit
   independent of the causes C2 attributes to it. Proposed: count incoming
   rows.
2. **The module-resolution gate.** C3 asserts `trace_parity == 1` at its
   exit (it is 1 today) and routes any difference to Phase 1, rather than
   leaving the assertion to C7. Confirm.
3. **Content mappers.** The 15 C3 rows stay `blocked` under B06 (Phase 5);
   `c3_complete` does not wait for them. Confirm.
4. **Project-reference callers.** The three remaining callers the C1 record
   names are C3.5 work. Confirm, or return them to Phase 1 with the loader
   side.
5. **Alias marking.** The checker-side marking functions (`markLinkedReferences`
   and the `mark*AliasReferenced` family, minus the JSX and decorator entries)
   are C3's; the resolver's queries over that state are C5's and not a C3
   exit prerequisite. Confirm.
6. **Documented differences.** Every JavaScript item of the pin's `CHANGES.md`
   gets a direct case asserting the Corsa behavior. Confirm the list is taken
   from the pinned file as is.
7. **Exit run.** `c3_complete` is computed only from a recorded full capture;
   the recording is the owner's. Confirm.
