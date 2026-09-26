# Phase 2 C4: JSX and decorators

Checkpoint C4 of the [Phase 2 plan](PHASE2-plan.md), written after the
[C2 plan](PHASE2-C2-plan.md) (PR #62, branch `phase2-c2-plan`) and its review
amendments, over the reviewed-source C1 capture (`target/phase2/rust`, the
promoted `c1-reviewed` capture: 12,459 of 13,432 rows match in every domain,
regression 9,367 of 9,367). Upstream is Corsa
`1f70213d4922b434345f639b441681e470c7cfc1`. C4 is a port from scratch in
`crates/tsr_checker`, paired with its witnesses, measured by the C0 contract
exactly as C1 to C3 are. The numbers below describe that capture; C4.0
refreshes them at the C4 head, after C2 and C3.

## 1. Objective and exit

Port `jsx.go`, the JSX paths of `checker.go` (attribute contextual typing,
element type resolution, runtime and namespace import resolution per `jsx`
mode) and the decorator checks (legacy and ES decorators, their call
signatures, context types, metadata marking and grammar), with their
contextual-typing and inference interactions, runtime and helper resolution,
and the diagnostics of every `jsx` mode the pin supports (`preserve`,
`react-native`, `react`, `react-jsx`, `react-jsxdev`). Keep checking and
transformation outputs distinct: Phase 3 owns the JSX, decorator and metadata
transforms; C4 owns what the checker reports and what it records for the
resolver. Retain everything S08 and C1 to C3 implement.

C4 exits when all of the following hold on one full run at the C4 head,
recorded by the owner through `cargo xtask run checker`:

| Exit condition | Measured by |
| --- | --- |
| Current, full, authenticated and recorded capture with no harness errors | `inventory_frozen`, `native_verified`, `harness_valid`, `result_recorded` and `blockers_named` are true; prerequisites of `c4_complete` |
| Existing cases preserved: the S08 regression subset still matches completely | `run.checker.regression_parity == 1` (9,367 of 9,367) |
| No previously matching domain of any executed row becomes a non-match | `run.checker.c4_regressions == 0` against the authenticated C4-start row report `data/phase2/c4-baseline.json.gz` |
| Every C4-owned row meets all seven applicable domains, or its remaining difference has a current validated handoff (section 3) | `run.checker.c4_open == 0` over `data/phase2/c4-claims.json`; `run.checker.c4_handoffs` reported |
| No unexplained production failure anywhere in a C4 claim, regardless of crate or module | `run.checker.c4_failures == 0` |
| Every blocker C4 owns in `data/phase2/blockers.json` (B01's C4 share, B02, B05, B12, B17) is closed by an implementation | `run.checker.c4_blockers_open == 0` |
| `jsx.go` and the decorator functions are audited: each pinned function mapped, disposed as equivalent, or handed to a named later checkpoint; no unexplained gap | `run.checker.c4_audit_complete == true` over `data/phase2/c4-audit.json` |
| The direct contracts of C4.8, including one per `jsx` mode and one per decorator position, pass in debug and release | `run.checker.c4_contracts == true`, from the recorded v2 receipt |

`run.checker.c4_complete` is the conjunction. It binds `P2B-C4` in
`sprints/P2B.toml`. The Phase 2 plan's rule applies: a few isolated diagnostic
patches do not close these families; the exit is the port.

## 2. Starting point

Everything below exists and is consumed as is. C4 extends it; it re-derives
nothing.

| Asset | Where | What C4 takes from it |
| --- | --- | --- |
| The reviewed C1 capture | `target/phase2/rust`: C4 rows 926, matching 159, open 767 (errors 766, types 555, symbols 547, display 547; 276 rows run with `NoTypesAndSymbols`) | the rows C4 owns (section 3) |
| The harness the review repaired | the baseline and regressions comparison, the v2 receipt, read-only replay, `phase2_audit.py check --audit PATH` with scope bindings, the per-checkpoint metric helper of C2.12, the blocker builder's per-variant ownership of C2.12 | every C4 authority follows it |
| Parsing and binding of JSX and decorators | `tsr_parser/src/jsx.rs` and `pragmas.rs` (JSX syntax, `@jsx` and `@jsxFrag` pragmas), the binder's JSX and decorator handling (`dispatch.rs`, `container_classification.rs`, `declarations.rs`, `bindings.rs`); all 379 `.tsx` rows parse and bind identically to Go (Phase 1 gate) | the trees and symbols C4 checks; nothing to port there |
| Checker guards and refusals | `query.rs` (`checkExpressionWorker` refuses every JSX expression kind), `class_check.rs` (`checkClassLikeDeclaration: decorators`), `binding_checks.rs` (`checkDecorators: parameter or binding`), `expression_context.rs` (`getContextualType: decorator/JSX context`), `check.rs` (`checkSourceElementWorker: statement/type family`, reached by a decorator on a class static block), `grammar_modifiers.rs` `has_grammar_decorator`, `name_errors.rs` `has_decorators`; JSX-aware branches already in `relater_excess.rs`, `object_discriminants.rs`, `flags.rs`, `host.rs`, `call_errors.rs`, `calls.rs`, `type_only_uses.rs`, `value_references.rs`, `module_specifiers_paths.rs` | the exact sites the port replaces; the guards that already read `jsx` options |
| Option handling | `tsr_compiler/src/verify_options.rs`, `checker_diagnostics.rs`, `output_paths.rs`, `metadata.rs`, `plain_js_errors.rs` (the `jsx`, `jsxFactory`, `jsxFragmentFactory`, `jsxImportSource`, `experimentalDecorators`, `emitDecoratorMetadata` options and their program-level diagnostics) | the option state C4 reads; program-level option errors stay where they are |
| Mapping state | `jsx.go` 0 of 59 marked; the 30 decorator functions of `checker.go`, 4 of `grammarchecks.go` and 1 of `utilities.go` unmarked; `isHyphenatedJsxName`, `isIgnoredJsxProperty` (`relater.go`) and `isArrayOrTupleLikeType` (`checker.go`) disposed `later: C4` by C1's audit | the audit input of C4.1; no port marker names JSX today (plan review finding 2) |
| The pin's JSX entry points in `checker.go` | the deferred element checks in `checkDeferredNode`; the five JSX cases of `checkExpressionWorker`; `resolveJsxOpeningLikeElement` in `resolveSignature`; `inferJsxTypeArguments` in `inferTypeArguments`; `checkJsxAttribute` in the attribute property's declared type; `markJsxAliasReferenced` at two sites; the JSX runtime import, namespace and factory checks of the source-file check; the three JSX cases of `getContextualType`; `discriminateContextualTypeByJSXAttributes` in object-literal contextual typing | the seams C4 wires, each named by its pinned function |
| C2 and C3 results | contextual typing, inference and overload resolution (C2.4, C2.9); classes and alias marking (C3.4, C3.5) | JSX props are contextual types and generic components are inferred calls; decorated classes are classes |
| Lib | `tsr_bundled`: `lib.decorators.d.ts`, `lib.decorators.legacy.d.ts`, `lib.es2015.symbol.wellknown.d.ts`, `lib.esnext.decorators.d.ts` and the `JSX` namespace declarations the test files supply (no React typings in the corpus) | the global types decorator contexts and JSX resolution look up |
| Direct tests | the C1 to C3 contracts, `checker_semantics.rs`, `checker_display.rs`; the `tsr_checker` unit tests | the homes C4.8 adds to |
| Pin tracing | the diagnostic Go overlay through the harness (`-overlay`, fingerprinted, never editing `upstream/`) | the attribution method of C4.1 |

The C4-start capture is a fresh named output directory; no existing capture is
moved or overwritten.

## 3. What C4 owns

The inventory assigns C4 every executed row that uses JSX or decorators,
whatever else it uses: 926 rows (decorators 490, JSX 436; 379 `.tsx`, 547
`.ts`; suites: compiler 303, jsx 225, esDecorators 161, statements 97,
decorators 88, es6 16, classes 14, externalModules 12; 233 emitted-output-only
rows compared on `errors`, `union_ordering` and `parent_pointers`; 10 emit
declarations). 162 carry non-empty type arguments, 159 explicit type
parameters, 15 conditional, 12 indexed-access, 7 `infer`, 7 mapped, 3
template-literal and 2 import types: the rows the Phase 2 plan assigns to C4
because they combine C2 features with JSX or decorators. Options over the JSX
rows: `jsx` preserve 209, react 158, react-jsx 38, react-jsxdev 19,
react-native 2, unset 10. Over the decorator rows: `experimentalDecorators`
true 249, false 52, unset 189 (ES decorators); `emitDecoratorMetadata` 77.

159 rows match in every domain at the reviewed head (files whose JSX or
decorators sit where the refusals are not reached, and rows compared only on
the two syntactic domains). The 767 open rows are C4's exit set:

| Bucket | Variants | Representative | Cause |
| --- | ---: | --- | --- |
| `unsupported: checkExpressionWorker` (B01, the C4 share) | 439 | `conflictMarkerTrivia3.tsx` | every JSX expression kind is refused (`query.rs`); the C2 share (`importWithTypeArguments`) is C2's |
| `unsupported: checkClassLikeDeclaration: decorators` (B02) | 284 | `classExpressionWithDecorator1` | a decorated class refuses before any member is checked (`class_check.rs`) |
| `unsupported: checkDecorators: parameter or binding` (B05) | 25 | `decoratorOnFunctionParameter` | decorated parameters and binding variables refuse (`binding_checks.rs`) |
| `diagnostics: TS2304` | 5 | `anonymousClassDecoratorEs2022` | names the ES-decorator paths resolve (`Symbol.metadata`, context types) are not looked up |
| `diagnostics: TS1240`, `TS1241`, `TS1329` | 2, 2, 2 | `decoratorOnClassProperty6`, `decoratorOnClassMethod8`, `decoratorOnClassProperty11` | decorator signature applicability and argument-count errors |
| `unsupported: getContextualType: decorator/JSX context` (B12) | 2 | `decoratorChecksFunctionBodies` | the contextual type of a decorator expression and of JSX children |
| `diagnostics: TS1272`, `TS2318`, `TS2331`, `TS2593`, `TS2660` | 1 each | `emitDecoratorMetadata_isolatedModules`, `missingDecoratorType`, `decoratorOnClassMethod11`, `metadataImportType`, `decoratorOnClassMethod12` | metadata type references under `isolatedModules`, missing global decorator types, `this` in decorator targets, metadata import types, `this` parameters under decorators |
| `unsupported: checkSourceElementWorker: statement/type family` (B17) | 1 | `classStaticBlock19` | a `Decorator` node on a class static block reaches the statement switch |

Blockers C4 owns: B01's C4 share (439 variants; the builder of C2.12 keeps
the C2 and C4 shares of one operation separate by variant), B02, B05, B12 and
B17. C4 closes each by implementing the refused operation; the register is
rebuilt at the exit and never edited by hand. A B-number is a display label;
the stable identity is kind, operation, variant and domains.

Ownership rule. Row ownership does not change. `data/phase2/c4-claims.json`
records every open C4 row with its bucket, its smallest reproduction, the
pinned function the trace named and a status: `open`, `closed` (matches at the
current head, with the commit), `blocked` (a registered blocker of another
owner withholds it) or `handed` (a validated trace attributes the remaining
difference to another checkpoint's cause: a C2 operator once the JSX or
decorator refusal is lifted, a C3 class or alias cause, a C5 declaration-emit
or node-builder cause on the 10 declaration rows). The validator's fields and
the rule that the trace must cover every currently nonmatching domain are the
C2 plan's. Both `open` and regressed `closed` rows count in `c4_open`;
`handed` rows are counted in `c4_handoffs` and listed in the C4 record.

The seven domains are `errors`, `types`, `symbols`, `display`, `trace`,
`union_ordering` and `parent_pointers`; native-disabled domains are met.
Declaration diagnostics are part of `errors`.

## 4. Work items

Each item names what exists, what to build, its artifact and its executable
exit check. Functions are named by their pinned Go names
(`tsc/internal/checker`); `port:` markers and the ledger stay the mapping
authority.

### C4.0 Refresh the gap map at the C4 head

- Exists: the reviewed C1 capture; `phase2_compare.py baseline`.
- Build: check the native capture with its read-only validator; run one full
  Rust contract at the C4 head into a fresh directory; compare against the
  latest recorded report and explain every change; freeze the authenticated
  result into `data/phase2/c4-baseline.json.gz`; write
  `data/phase2/c4-claims.json` from the table of section 3, every open C4 row
  `open`, and the C2-family rows tagged for the reverse handoff rule.
- Exit: 0 harness errors, 0 regressions, an explained per-row delta, the
  baseline committed; the claims file names every open C4 row exactly once.

### C4.1 Audit and attribution

- Exists: `scripts/phase2_audit.py` with the C1 to C3 scope bindings; the
  C1 `later: C4` dispositions.
- Build: `data/phase2/c4-audit.json`, with a separately reviewed C4 scope
  binding, over these groups:
  - `jsx.go`, the complete file (59 functions): element and attribute
    checking (`checkJsxElement`, `checkJsxElementDeferred`, `checkJsxExpression`,
    `checkJsxSelfClosingElement`, `checkJsxSelfClosingElementDeferred`,
    `checkJsxFragment`, `checkJsxAttributes`,
    `checkJsxOpeningLikeElementOrOpeningFragment`, `checkJsxPreconditions`,
    `checkJsxReturnAssignableToAppropriateBound`, `checkJsxAttribute`,
    `checkJsxChildren`, `createJsxAttributesTypeFromAttributesProperty`,
    `getNameFromJsxElementAttributesContainer`); contextual typing and
    inference (`inferJsxTypeArguments`, `getContextualTypeForJsxExpression`,
    `getContextualTypeForJsxAttribute`, `getContextualJsxElementAttributesType`,
    `getContextualTypeForChildJsxExpression`,
    `discriminateContextualTypeByJSXAttributes`, `getJsxManagedAttributesFromLocatedAttributes`,
    `instantiateAliasOrInterfaceWithDefaults`, `getJsxLibraryManagedAttributes`);
    elaboration (`elaborateJsxComponents`, `generateJsxChildren`,
    `getElaborationElementForJsxChild`, `elaborateIterableOrArrayLikeTargetElementwise`,
    `getSuggestedSymbolForNonexistentJSXAttribute`); resolution
    (`resolveJsxOpeningLikeElement`, `checkApplicableSignatureForJsxCallLikeElement`,
    `getUninstantiatedJsxSignaturesOfType`, `getEffectiveFirstArgumentForJsxSignature`,
    `getJsxPropsTypeFromCallSignature`, `getJsxPropsTypeFromClassType`,
    `getJsxPropsTypeForSignatureFromMember`, `getStaticTypeOfReferencedJsxConstructor`,
    `getJsxReferenceKind`, `createSignatureForJSXIntrinsic`,
    `getIntrinsicAttributesTypeFromStringLiteralType`,
    `getIntrinsicAttributesTypeFromJsxOpeningLikeElement`, `getIntrinsicTagSymbol`);
    the `JSX` namespace and element types (`getJSXFragmentType`,
    `getJsxElementTypeSymbol`, `getJsxElementPropertiesName`,
    `getJsxElementChildrenPropertyName`, `getJsxStatelessElementTypeAt`,
    `getJsxElementClassTypeAt`, `getJsxElementTypeAt`, `getJsxElementTypeTypeAt`,
    `getJsxType`, `getJsxNamespaceAt`, `getJsxNamespace`, `getLocalJsxNamespace`);
    runtime and factory resolution (`getJsxFactoryEntity`,
    `getJsxFragmentFactoryEntity`, `parseIsolatedEntityName`, `markAsSynthetic`,
    `getJsxNamespaceContainerForImplicitImport`, `getJSXRuntimeImportSpecifier`);
  - `checker.go` JSX seams: the eleven call sites of section 2 and
    `markJsxAliasReferenced`; `relater.go` `isHyphenatedJsxName`,
    `isIgnoredJsxProperty`; `checker.go` `isArrayOrTupleLikeType`;
    `grammarchecks.go` `checkGrammarJsxElement`, `checkGrammarJsxName`,
    `checkGrammarJsxExpression`;
  - decorators (30 `checker.go` functions: `checkDecorators`, `checkDecorator`,
    `resolveDecorator`, `isPotentiallyUncalledDecorator`,
    `getDiagnosticHeadMessageForDecoratorResolution`, `getDecoratorArgumentCount`,
    `getLegacyDecoratorArgumentCount`, `getEffectiveDecoratorArguments`,
    `getDecoratorCallSignature`, `getLegacyDecoratorCallSignature`,
    `getESDecoratorCallSignature`, `newESDecoratorCallSignature`,
    `getContextualTypeForDecorator`, `newClassDecoratorContextType`,
    `newClassMethodDecoratorContextType`, `newClassGetterDecoratorContextType`,
    `newClassSetterDecoratorContextType`, `newClassAccessorDecoratorContextType`,
    `newClassFieldDecoratorContextType`, `getClassMemberDecoratorContextOverrideType`,
    `newClassMemberDecoratorContextTypeForNode`, `newClassAccessorDecoratorTargetType`,
    `newClassAccessorDecoratorResultType`, `newClassFieldDecoratorInitializerMutatorType`,
    `newTypedPropertyDescriptorType`, `markDecoratorAliasReferenced`,
    `getParameterTypeNodeForDecoratorCheck`, `markDecoratorMedataDataTypeNodeAsReferenced`,
    `getEntityNameForDecoratorMetadata`, `getEntityNameForDecoratorMetadataFromTypeList`;
    `grammarchecks.go` `checkGrammarDecorator`, `reportObviousDecoratorErrors`,
    `findFirstIllegalDecorator` and the fourth decorator grammar entry;
    `utilities.go`'s decorator predicate).
  The emit resolver's `GetJsxFactoryEntity`, `GetJsxFragmentFactoryEntity` and
  `GetTypeReferenceSerializationKind` and the services' `GetJsxIntrinsicTagNamesAt`
  and `GetContextualTypeForJsxAttribute` are C5's surface over C4's functions;
  C4 lists them as `later: C5` with the C4 function each wraps.
- Build, second half: the attribution of the 25 `different` rows (the
  decorator rows that pass the refusals) by reproduction and, where the Rust
  observation does not name the function, a pin trace through the diagnostic
  overlay; the C2-family rows are attributed after their JSX or decorator
  refusal is lifted, not before.
- Exit: `phase2_audit.py check --audit data/phase2/c4-audit.json` passes with
  no `gap`; every `different` row has a named cause.

### C4.2 JSX grammar, pragmas and preconditions

- Exists: the parser's JSX syntax and pragma collection; `verify_options.rs`
  for the `jsx` family; `has_grammar_decorator`.
- Build: `checkGrammarJsxElement`, `checkGrammarJsxName`,
  `checkGrammarJsxExpression`; `checkJsxPreconditions` (the `jsx` option
  requirement, `TS17004`) and `checkJsxOpeningLikeElementOrOpeningFragment`;
  `getJsxFactoryEntity` and `getJsxFragmentFactoryEntity` with
  `parseIsolatedEntityName` and `markAsSynthetic` over the file's pragmas and
  the `jsxFactory`/`jsxFragmentFactory` options, including the pragma and
  option conflict diagnostics; the entity names are recorded for the
  resolver's queries (C5) and never emitted here.
- Exit: the grammar rows match; a direct case per pragma and option
  combination against a native observation.

### C4.3 JSX elements, attributes, children and the `JSX` namespace

- Exists: the refusal at `query.rs`; the excess-property and discriminant
  helpers that already read JSX flags; `object_literals.rs` and
  `object_spread.rs` for attribute objects.
- Build: the five `checkExpressionWorker` cases (`checkJsxExpression`,
  `checkJsxElement`, `checkJsxSelfClosingElement`, `checkJsxFragment`,
  `checkJsxAttributes`) and the deferred element checks; attribute types from
  the attributes property (`createJsxAttributesTypeFromAttributesProperty`,
  `checkJsxAttribute`, `checkJsxChildren`, spread attributes and
  `getNameFromJsxElementAttributesContainer`); the `JSX` namespace lookup at a
  location (`getJsxNamespaceAt`, `getLocalJsxNamespace`, `getJsxType`) and the
  element, element-class, element-attributes-property, element-children and
  fragment types it defines; intrinsic elements (`getIntrinsicTagSymbol`,
  `IntrinsicElements`, `IntrinsicAttributes`, `IntrinsicClassAttributes`,
  `createSignatureForJSXIntrinsic`, the string-literal intrinsic case);
  `checkJsxReturnAssignableToAppropriateBound` and the `LibraryManagedAttributes`
  instantiation with defaults; `isHyphenatedJsxName` and `isIgnoredJsxProperty`
  in the relater's property enumeration; `isArrayOrTupleLikeType` through
  `elaborateJsxComponents`, `generateJsxChildren` and the child elaboration
  helpers; `getSuggestedSymbolForNonexistentJSXAttribute`. The JSX flags on
  types and symbols (`flags.rs`) are already in place and are verified, not
  redesigned.
- Exit: B01's C4 share is closed; the JSX rows without generic components
  match; a direct case per element kind (intrinsic, function component, class
  component, fragment, member-expression tag, namespaced tag) and per
  children shape.

### C4.4 JSX resolution, contextual typing and inference

- Exists: `calls.rs` (`resolveSignature` refuses the JSX branch),
  `call_arguments.rs` (`inferTypeArguments`), `expression_context.rs`
  (`getContextualType` refuses the JSX cases), `object_discriminants.rs`;
  C2.4 and C2.9.
- Build: `resolveJsxOpeningLikeElement` in `resolveSignature`, with
  `getUninstantiatedJsxSignaturesOfType`, `getEffectiveFirstArgumentForJsxSignature`,
  `checkApplicableSignatureForJsxCallLikeElement`, the props types from call
  and construct signatures and class types, `getJsxReferenceKind` and
  `getStaticTypeOfReferencedJsxConstructor`; `inferJsxTypeArguments` in
  `inferTypeArguments`; the three JSX cases of `getContextualType` and
  `getContextualTypeForChildJsxExpression`; `discriminateContextualTypeByJSXAttributes`
  in object-literal contextual typing; B12's JSX half. The C2-family rows
  (162 with type arguments, 159 with explicit type parameters) are the
  generic-component evidence; a row whose only remaining difference after
  this item is a C2 operator goes back to C2 by the reverse rule.
- Exit: the generic-component rows match or are handed with a trace; a direct
  case per resolution kind (overloaded components, generic components with
  inferred and explicit type arguments, contextual attribute functions,
  discriminated attribute unions) against a native observation.

### C4.5 JSX runtime and namespace imports per `jsx` mode

- Exists: the option state (`host.rs`, `verify_options.rs`), module
  resolution through the program adapter (Phase 1), `type_only_uses.rs` and
  `value_references.rs` (alias references), C3.5's alias marking.
- Build: `getJsxNamespaceContainerForImplicitImport` and
  `getJSXRuntimeImportSpecifier` (the `react-jsx` and `react-jsxdev` runtime
  module `jsx-runtime`/`jsx-dev-runtime` under `jsxImportSource` and the
  `@jsxImportSource` pragma, resolved through the program's retained
  resolutions, with the pin's diagnostics when the module or its exports are
  missing); the source-file check that marks the factory and fragment entity
  names and the implicit runtime import as referenced (`markJsxAliasReferenced`
  at both sites, the `React` namespace lookup under `preserve`, `react` and
  `react-native`, the `TS2874`, `TS2875`, `TS17016` and `TS17017` family);
  per-mode behavior differences the corpus configurations exercise (209
  preserve, 158 react, 38 react-jsx, 19 react-jsxdev, 2 react-native). The
  resolver's `GetJsxFactoryEntity` and `GetJsxFragmentFactoryEntity` read
  this state (C5); Phase 3 emits the calls.
- Exit: the runtime-import rows match; a direct case per `jsx` mode with a
  present and an absent runtime module, observing the diagnostics and, through
  the resolver, the recorded entities.

### C4.6 Decorators: grammar, resolution, call signatures and context types

- Exists: the refusals at `class_check.rs`, `binding_checks.rs`,
  `expression_context.rs` and `check.rs`; `has_grammar_decorator`,
  `has_decorators`; `grammar_modifiers.rs`; the bundled decorator libs;
  C3.4's class checks.
- Build: `checkGrammarDecorator`, `reportObviousDecoratorErrors`,
  `findFirstIllegalDecorator` and the decorator-placement grammar
  (`TS1206`, `TS1207`, `TS1219`, `TS1240`, `TS1241`, `TS1329`, the static-block
  case behind B17); `checkDecorators` and `checkDecorator` for classes,
  methods, accessors, properties, parameters (legacy only) and their
  positions; `resolveDecorator`, `isPotentiallyUncalledDecorator`,
  `getDiagnosticHeadMessageForDecoratorResolution`, the argument counts and
  `getEffectiveDecoratorArguments`; the legacy call signature
  (`getLegacyDecoratorCallSignature`, `newTypedPropertyDescriptorType`) and the
  ES call signature (`getESDecoratorCallSignature`, `newESDecoratorCallSignature`,
  the six context types, `getClassMemberDecoratorContextOverrideType`,
  `newClassMemberDecoratorContextTypeForNode`, the accessor target and result
  types, the field initializer mutator type) with their lib lookups
  (`ClassDecoratorContext` and the others, `Symbol.metadata`; `TS2318` and
  `TS2304` when absent); `getContextualTypeForDecorator` (B12's decorator
  half); the `experimentalDecorators` split (249 true, 52 false, 189 unset)
  decides the signature kind per row exactly as the pin does.
- Exit: B02, B05, B12 and B17 are closed; the decorator rows match; a direct
  case per position and per signature kind against a native observation.

### C4.7 Decorator metadata and alias marking

- Exists: C3.5's marking state; `type_only_uses.rs`; the `TS1272` row
  (`emitDecoratorMetadata_isolatedModules`).
- Build: `markDecoratorAliasReferenced`, `getParameterTypeNodeForDecoratorCheck`,
  `markDecoratorMedataDataTypeNodeAsReferenced`, `getEntityNameForDecoratorMetadata`
  and `getEntityNameForDecoratorMetadataFromTypeList` under
  `emitDecoratorMetadata` (77 rows): the type references the metadata emit
  will serialize are marked referenced and the `isolatedModules` type-only
  diagnostics report; the serialization kind itself
  (`GetTypeReferenceSerializationKind`) is the resolver's (C5) and the
  `__metadata` emit is Phase 3's. Checking and transformation stay distinct:
  C4 asserts the marking through the resolver's alias queries, never through
  emitted output.
- Exit: the metadata rows match; a direct case with a type-only import, a
  value import and an ambient type under `emitDecoratorMetadata`, observing the
  marking through the resolver.

### C4.8 Direct contracts

- Exists: the C1 to C3 contracts and helpers.
- Build: `crates/tsr_compiler/tests/c4_contracts.rs`, one test per contract,
  each over production entry points with a pinned Go counterpart named in its
  doc comment:
  1. the `jsx` mode matrix: one program checked under each of the five modes
     with and without factory pragmas; diagnostics, resolved entities and the
     implicit runtime import equal the native observations;
  2. element kinds: intrinsic, function, class, fragment, member and
     namespaced tags, with `LibraryManagedAttributes` and children typing;
  3. generic components: inferred and explicit type arguments, overloads and
     contextual attribute functions, over a bundled-lib-only program;
  4. JSX in JavaScript: a `.jsx` file under `checkJs` reports the pin's
     diagnostics;
  5. decorator positions: class, method, getter, setter, auto-accessor,
     field, parameter, static block, each under legacy and ES decorators with
     the pin's signature selection and diagnostics;
  6. context types: the ES decorator context types have the pin's identities
     and members, and `Symbol.metadata` resolves through the lib;
  7. metadata marking: the referenced type names under `emitDecoratorMetadata`
     are observed through the resolver, and `isolatedModules` reports
     `TS1272`;
  8. lifecycle: a JSX error and a decorator error leave the checker reusable,
     and two checkers over one program resolve the runtime import
     independently;
  9. no transform: the checker produces no emitted syntax for JSX or
     decorators; the resolver's recorded entities are the only output.
- Exit: `cargo test -p tsr_compiler --test c4_contracts` in debug and
  release; the v2 receipt is recorded by the producer (C4.9).

### C4.9 Producer wiring

- Exists: the per-checkpoint metric helper and the blocker builder's
  per-variant ownership (C2.12); `sprints/P2B.toml` with `P2B-C4` waiting on
  `run.checker.c4_complete`.
- Build: the producer reads `data/phase2/c4-claims.json`,
  `data/phase2/c4-audit.json`, `data/phase2/c4-baseline.json.gz` and the
  contracts receipt (`observe --witness c4-contracts`), and emits `c4_open`,
  `c4_handoffs`, `c4_regressions`, `c4_failures` (every unattributed failure
  plus failures traced to C4, in any crate), `c4_blockers_open`,
  `c4_audit_complete`, `c4_contracts` and `c4_complete`. B01's C2 and C4
  shares are counted per variant. All new authorities are added to `[checker]`
  `inputs`.
- Exit: `cargo xtask validate`; `phase2_producers.py checker` emits the eight
  metrics; `scripts/tests/test_phase2_c4.py` shows that an open row keeps
  `c4_complete` false, that a handoff without its trace is rejected, that a
  C4-owned blocker left in the register keeps `c4_blockers_open` nonzero, and
  that changing each new input invalidates the recorded result.

## 5. Dependencies and owners

| Dependency | Owner | State for C4 |
| --- | --- | --- |
| Contextual typing, inference, overloads and the generic operators the combined rows use | C2 | C4.4 starts after C2.4 and C2.9; rows with a remaining C2 cause go back by the reverse rule |
| Classes, alias marking, option-dependent checks | C3 | C4.6 consumes C3.4; C4.5 and C4.7 consume C3.5's marking state |
| The resolver's JSX factory, fragment, metadata serialization and services queries over C4's functions | C5 | `later: C5` in the C4 audit; contracts 1, 5 and 7 observe C4's state through the resolver, so the resolver entry points must exist before C4 exits (C5.6 of the C5 plan) |
| The JSX, decorator and metadata transforms and their `.js` output | Phase 3 | not C4's; C4 records state, Phase 3 emits |
| Declaration emit of JSX components and decorated classes (10 rows) | C5 with Phase 3 | handoff targets by trace |
| The bundled decorator and symbol libs | `tsr_bundled` | present; C4 verifies the lookups |
| The `checker` recording | owner | C4.9 |
| Owner decisions of section 9 | owner | before C4.1 |

## 6. Delivery order

1. C4.0 the fresh gap map and claims file; C4.1 the audit and the attribution
   of the 25 `different` rows, reviewed before production changes.
2. C4.6 and C4.7 (decorators), because their refusals withhold 310 rows and
   depend only on C3.4 and C3.5; B02, B05, B12's decorator half and B17 close
   here.
3. C4.2 and C4.3 (JSX grammar, elements and the namespace), closing B01's C4
   share for the non-generic rows.
4. C4.4 (resolution and inference) and C4.5 (runtime imports), then the
   reverse handoffs to C2 for any remaining operator cause.
5. C4.8 grows alongside 2–4, one contract per item; C4.9 last, then the exit
   full run and the record.

Intermediate runs use the recorded 300-variant sample (70 C4 rows, 100
regression rows) plus the claimed rows (`--sample --case ...`). Full runs are
C4.0, after step 3 (the first time JSX expressions are checked at scale) and
the exit.

## 7. Executable exit checks

`target/phase2/rust-c4-start/comparison.json` below is the C4-start report
from C4.0; use its actual saved path. Exit output is a fresh directory.

```sh
export PATH="$(mise where go)/bin:$PATH"
python3 scripts/phase2_inventory.py check
python3 scripts/phase2_corpus.py run --native target/phase2/native --output target/phase2/rust-c4
python3 scripts/phase2_compare.py report --native target/phase2/native --rust target/phase2/rust-c4 --previous target/phase2/rust-c4-start/comparison.json --record
python3 scripts/phase2_blockers.py build --native target/phase2/native --rust target/phase2/rust-c4 --record
python3 scripts/phase2_audit.py check --audit data/phase2/c4-audit.json
cargo test -p tsr_compiler --test c4_contracts --locked && cargo test -p tsr_compiler --test c4_contracts --locked --release
python3 scripts/phase2_producers.py observe --witness c4-contracts           # the v2 receipt, with the dependency and asset closure
python3 scripts/s08_relater.py build  --output target/s08/relater-c4
python3 scripts/s08_relater.py parity --output target/s08/relater-c4         # 105/105, all_cases_match true, both implementations
python3 scripts/phase2_producers.py checker --rust target/phase2/rust-c4    # c4_open 0, c4_regressions 0, c4_failures 0, c4_blockers_open 0, c4_audit_complete, c4_contracts, c4_complete; c4_handoffs reported
python3 -m pytest scripts/tests/test_phase2_*.py -q
python3 scripts/checks.py fmt && python3 scripts/checks.py clippy
cargo xtask validate
# Assert the C4 metrics explicitly; `check P2B` stays pending until C7.
```

`cargo xtask run checker` and `cargo xtask status --record` are the owner's.

## 8. Evidence reuse rules

- The C0 native capture is reused only when its read-only current-input
  validator succeeds; a changed canonical input requires a fresh, verified
  capture. JSX and decorator observations are already in it: the native
  harness never skipped these rows.
- A Rust capture supplies current acceptance only when replay succeeds against
  the captured executable with `source_stable: true`.
- The C4-start baseline is reused only while it names the current native
  observation and inventory digests.
- Claims are per row and per cause; the producer rejects a handoff without
  its trace; a C2-family row is attributed only after its refusal is lifted.
- A difference is accepted only through the divergence ledger (ADR 0004). C4
  expects none: the pin supports every `jsx` mode and both decorator kinds.
- Emitted output is never evidence for C4; the resolver's recorded state is.
- Mapping progress is reported by markers and the ledger, never as behavioral
  coverage; the corpus and the C4.8 contracts are the behavioral evidence.

## 9. Owner decisions before C4 starts

1. **Scope binding.** The C4 audit binds the complete `jsx.go` inventory and
   the named decorator functions of `checker.go`, `grammarchecks.go` and
   `utilities.go`; the resolver and services wrappers are `later: C5`.
   Confirm.
2. **B01's split.** The blocker builder counts B01's C2 and C4 shares per
   variant (C2.12); C4's exit requires only its share. Confirm.
3. **Resolver entry points.** C4's contracts observe JSX entities and
   metadata marking through the resolver's queries, so C5.6 must land those
   entry points before C4 exits, or C4 exposes them provisionally under C5's
   review. Proposed: C5.6 first.
4. **Start order.** C4.6 and C4.7 (decorators) start after C3.4 and C3.5,
   before C2 exits; C4.3 to C4.5 start after C2.4 and C2.9. Confirm, or hold
   all of C4 until C2 and C3 exit.
5. **Emitted-output-only rows.** The 233 C4 rows compared only on `errors`,
   `union_ordering` and `parent_pointers` count as ordinary rows; no separate
   metric. Confirm.
6. **Exit run.** `c4_complete` is computed only from a recorded full capture;
   the recording is the owner's. Confirm.
