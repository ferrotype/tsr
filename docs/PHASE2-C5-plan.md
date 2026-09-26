# Phase 2 C5: display, checker services and emit-resolver contracts

Checkpoint C5 of the [Phase 2 plan](PHASE2-plan.md), written after the
[C2 plan](PHASE2-C2-plan.md) (PR #62, branch `phase2-c2-plan`) and its review
amendments, over the recorded C2 exit capture (`target/phase2/rust`: 12,647 of 13,432 rows match in every domain, regression 9,367 of 9,367; `P2B-C2` recorded complete with three emit-order rows handed to C5). Upstream is Corsa
`1f70213d4922b434345f639b441681e470c7cfc1`. C5 is production work in
`crates/tsr_checker`, `crates/tsr_nodebuilder` and the checker-facing seams of
`crates/tsr_compiler`, paired with its witnesses, measured by the C0 contract
and by the direct contracts the corpus does not observe. The numbers below
describe that capture; C5.0 refreshes them at the C5 head.

## 1. Objective and exit

Cover the remaining public checker query surface (`exports.go`), name
accessibility (`symbolaccessibility.go`), type-node serialization and reuse
with its caches, recovery scopes and truncation (`nodebuilder*.go`,
`nodecopy.go`, `symboltracker.go`, `printer.go`, `pseudotypenodebuilder.go`),
the service-specific queries (`services.go`, which starts from zero and has no
compiler-corpus witness), and the checker-dependent declaration diagnostics
through the emit resolver (`emitresolver.go`). Add direct native cases where
baseline walks do not exercise a required entry point. The exit is that the
documented downstream contracts pass through real checker operations with
correct owner retention and source context, and that every required
emit-diagnostic dependency has an implemented path or remains an explicit
joint blocker. Retain everything S08 and C1 to C4 implement.

C5 exits when all of the following hold on one full run at the C5 head,
recorded by the owner through `cargo xtask run checker`:

| Exit condition | Measured by |
| --- | --- |
| Current, full, authenticated and recorded capture with no harness errors | `inventory_frozen`, `native_verified`, `harness_valid`, `result_recorded` and `blockers_named` are true; prerequisites of `c5_complete` |
| Existing cases preserved: the S08 regression subset still matches completely | `run.checker.regression_parity == 1` (9,367 of 9,367) |
| No previously matching domain of any executed row becomes a non-match | `run.checker.c5_regressions == 0` against the authenticated C5-start row report `data/phase2/c5-baseline.json.gz` |
| Every row handed to C5 with a validated trace matches, and every declaration-diagnostic difference of the declaration audit with a C5 cause is closed | `run.checker.c5_open == 0` over `data/phase2/c5-claims.json`; `run.checker.c5_handoffs` reported |
| The `display` domain matches on every executed row whose difference has a node-builder cause | `run.checker.display_parity` reported; no `display` difference remains attributed to C5 |
| No unexplained production failure in the node builder, the resolver or the declaration-diagnostics path, including the retained late-declaration panic | `run.checker.c5_failures == 0` |
| The emit-order blocker's C5 share is closed by an implementation, or re-owned to Phase 3 with the trace that names the emit cause | `run.checker.c5_blockers_open == 0` |
| The C5 function groups of section 4 are audited: each pinned function mapped, disposed as equivalent, or handed to a named later phase | `run.checker.c5_audit_complete == true` over `data/phase2/c5-audit.json` |
| The recorded service calls of the pinned fourslash suite replay against the Rust checker with equal results, per operation, with an owner-approved list of any operation left to Phase 5 | `run.checker.c5_services == true`, from `data/phase2/services-replay.json` and its replay record |
| The direct contracts of C5.8 pass in debug and release | `run.checker.c5_contracts == true`, from the recorded v2 receipt |

`run.checker.c5_complete` is the conjunction. It binds `P2B-C5` in
`sprints/P2B.toml`. No ratio threshold is introduced.

## 2. Starting point

Everything below exists and is consumed as is. C5 extends it; it re-derives
nothing.

| Asset | Where | What C5 takes from it |
| --- | --- | --- |
| The recorded C2 exit capture and the declaration audit | `target/phase2/rust` (12,647 matches); `data/phase2/blockers.json` `declaration_audit`: 1,754 native declaration requests, 1,740 working, 5 `different`, 3 not loaded (content mappers), 6 withheld by C4's JSX refusal, no panic (C2 fixed the late-declaration retention failure C1 had traced); `display` 12,196 match, 0 different, 679 native-disabled, 557 withheld by C4 refusals | the declaration and display differences C5.0 attributes; the rule that only a missing transform or resolver operation becomes a blocker |
| The handoffs already recorded | C2 (`c2-claims.json`: 3 rows `handed` to C5 with validated traces, the emit-order rows: `incorrectRecursiveMappedTypeConstraint` first resolves through `ConstEnumInliningTransformer.visit` and `EmitResolver.GetConstantValue`; `typeParameterWithInvalidConstraintType` and `recursiveMappedTypes` through `ImportElisionTransformer.visit` and `MarkLinkedReferencesRecursively`; the recorded stacks and reproductions are in `data/phase2/c2-emit-handoffs/`); C1's audit `later: C5`: `getNameFromIndexInfo` and the fourth scoped `typeParameterSymbolList`; every other row C1 or the C2 plan had named for C5 (the `TS40xx` accessibility rows, the elision-comment and reparsed-module refusals, the expando-parameter row, the late-declaration panic) was closed in C2 | the `incoming` rows of C5.0, rebound to the C5-start capture |
| The harness the review repaired | the baseline and regressions comparison, the v2 receipt, read-only replay, `phase2_audit.py check --audit PATH`, the per-checkpoint metric helper and the per-variant blocker ownership of C2.12, the diagnostic Go overlay pattern of C2.10 (`-overlay`, fingerprinted, never editing `upstream/`) | every C5 authority follows it; the overlay pattern is how C5.5 records service calls |
| The node builder | `node_builder.rs`, `node_builder_cache.rs`, `node_builder_class_emit.rs`, `node_builder_containers.rs`, `node_builder_emit.rs`, `node_builder_enum.rs`, `node_builder_extra.rs`, `node_builder_imports.rs`, `node_builder_names.rs`, `node_builder_pseudo.rs`, `node_builder_pseudo_output.rs`, `node_builder_reuse.rs`, `node_builder_scope.rs`, `node_builder_scopes.rs`, `node_builder_serialize.rs`, `handles_display.rs`, `type_display.rs`; `crates/tsr_nodebuilder` (the flag and symbol-tracker contract shared with the declarations transformer and the printer); the cache, retention, reuse and stack tests | the production builder behind `.types` display, `typeToString` and declaration serialization; `nodebuilderimpl.go` 67 of 113 marked, `nodebuilder.go` 3 of 29, `nodebuilderscopes.go` 2 of 4, `nodecopy.go` 4 of 28, `symboltracker.go` 0 of 14, `printer.go` 7 of 27, `pseudotypenodebuilder.go` 8 of 9, `nodebuilder_hover.go` 0 of 18 (tracker counts at the C2 exit) |
| The emit resolver | `emit_resolver.rs` (today the resolver borrows the caller's exclusive checker operation for its lifetime; the pin instead hands the resolver a checker without the pool lock and every resolver method takes the checker's lock for that call, which C5.6 and C6.3 adopt), `emit_visibility.rs`, `emit_reference.rs`, `emit_scopes.rs`, `emit_checks.rs`, `accessibility.rs`; `tsr_compiler/src/declaration_diagnostics.rs` (executes the declarations transform with the checker's resolver, no output written; the request phase `declaration` runs when the native row emits declarations) | `emitresolver.go` 22 of 64 marked; all 43 exported methods are consumed by the pin's printer, transformers, compiler and transpile packages; `symbolaccessibility.go` 19 of 37 |
| The public API | `exports.go` (89 functions, none marked: the `Checker` methods the pin's language service, API and printer call), `query.rs`, `query_location.rs`, `query_names.rs`, `handles*.rs`; `crates/tsr_api` (snapshot-local roots, handles as checker type ids, ADR 0012) and `crates/tsr_project` (slot lifetime and generation retirement) | the surface C5.2 completes; the handle and retention rules it must keep |
| The services surface | `services.go`: 66 functions, 46 exported, 43 called by `internal/ls`, `internal/lsp` and `internal/api` (`GetContextualType` at 29 sites, `GetExportSpecifierLocalTargetSymbol` 9, `GetApparentProperties` 8, `SkipAlias` 8, `GetConstantValue` 8, `GetSignatureFromDeclaration` 8, `GetCallSignatures` 6, `GetRootSymbols` 5, and the rest at four or fewer; `ForEachExportAndPropertyOfModule`, `GetConstructSignatures` and `IsSymbolReferencedInFile` are uncalled); `nodebuilder_hover.go` is called by `internal/ls/hover.go` | the operations C5.5 ports; the call sites that define their reference behavior |
| The pinned fourslash suite | `upstream/tsc/internal/fourslash`: 4,356 `_test.go` files over the 48-file `internal/ls` package (completions, hover, definition, references, rename, signature help, code actions, inlay hints, call hierarchy, code lens, folding, selection ranges, semantic tokens, linked editing, symbols, JSDoc, format, organize imports); ADR 0019 carries a transport-only harness patch for Phase 5 | the executable reference for `services.go` (section 4, C5.5); C5 records the checker calls it makes, Phase 5 runs the semantic assertions |
| Direct tests | `c1_contracts.rs`, `c1_review_regressions.rs`, `checker_display.rs`, `checker_baselines.rs`, `checker_error_tail.rs`, `checker_outer_expressions.rs`, `diagnostic_writer.rs`, `phase2_subtests.rs`; the node-builder cache, retention, reuse and stack tests in `tsr_checker` | the homes C5.8 adds to |

The C5-start capture is a fresh named output directory; no existing capture is
moved or overwritten.

## 3. What C5 owns

C5 owns no inventory rows. It owns causes and contracts:

| Source | Rows or units | State at the C5 start |
| --- | ---: | --- |
| Rows handed to C5 by C2 | 3 | `incoming`: the emit-order rows (register entry B05 at the C2 exit, B09 in the C1 capture; kind `emit_order`), rebound at C5.0 |
| Rows C1 had returned to C5 | 0 | closed in C2 (the expando-parameter `TS4025` row; the late-declaration panic, fixed) |
| Rows C4 hands to C5 | up to 10 | the declaration-emit rows of JSX components and decorated classes, after C4 lifts their refusals |
| Declaration-audit differences with a node-builder or resolver cause | up to 5 | attributed in C5.0; those with a type-level cause go back to C2 or C4 by trace |
| `display` differences with a node-builder cause | 0 at the C2 exit | C4's rows may add some; attributed in C5.0 |
| Blockers | the emit-order entry (B05 at the C2 exit), owner `C5 emit resolver, with Phase 3 emit (joint)` | C5 closes its share by executing the pin's emit schedule (C5.7) or re-owns it to Phase 3 with the trace |
| Contracts the corpus never observes | the public API, accessibility chains, hover expansion, the services operations, resolver queries, owner retention and source context | C5.5 and C5.8 |

`data/phase2/c5-claims.json` records, under `incoming`, every row another
checkpoint handed to C5 (the handing entry, its trace digest and the pinned
function it named), the declaration-audit and display rows C5.0 attributes to
C5 with their reproductions, and C5's own onward handoffs to Phase 3 or Phase
5 with the validator's fields of the C2 plan (target, pinned function id,
reproduction argv, capture and request identities, trace artifact path and
digest, full raw observation digest with fatal class and `panic_location`,
covered domains, cause). An incoming row counts in `c5_open` while it differs.
A C5 declaration handoff never disables the whole `errors` comparison of a
row: declaration diagnostics are part of `errors`, and the trace must cover
every currently nonmatching domain. Status labels never exempt a current
difference.

The seven domains are `errors`, `types`, `symbols`, `display`, `trace`,
`union_ordering` and `parent_pointers`. `display` is the public
`TypeToString` query set the walker records (`public_type_strings`), compared
by `compare_display`; it is the corpus-level witness of the node builder's
string output, and `.types` and `.symbols` are its structural witnesses.

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
authority.

### C5.0 Refresh the gap map at the C5 head

- Exists: the recorded C2 exit capture; the declaration audit; the C1, C2 and C4
  claims files; `phase2_compare.py baseline`.
- Build: check the native capture with its read-only validator; run one full
  Rust contract at the C5 head into a fresh directory; compare against the
  latest recorded report and explain every change; freeze the authenticated
  result into `data/phase2/c5-baseline.json.gz`; rebind the incoming rows
  with `phase2_claims.py rebind` (the rule of section 3); write
  `data/phase2/c5-claims.json` with the incoming rows and the attributed
  declaration-audit and display rows. Attribution distinguishes a
  serialization cause (the node builder produced a different node or string
  for the same type) from a type cause (the type differs) by comparing the
  `.types` observation of the same position: a type difference goes back to
  its owner by trace.
- Exit: 0 harness errors, 0 regressions, an explained per-row delta, the
  baseline committed; every incoming and attributed row named exactly once.

### C5.1 Audit

- Exists: `scripts/phase2_audit.py` with the C1 to C4 scope bindings; C1's
  `later: C5` dispositions.
- Build: `data/phase2/c5-audit.json`, with a separately reviewed C5 scope
  binding, over these complete files: `emitresolver.go` (64), `nodebuilder.go`
  (29), `nodebuilderimpl.go` (113), `nodebuilderscopes.go` (4),
  `nodebuilder_hover.go` (18), `nodecopy.go` (28), `symboltracker.go` (14),
  `printer.go` (27), `pseudotypenodebuilder.go` (9), `symbolaccessibility.go`
  (37), `services.go` (66), `exports.go` (89); plus the `checker.go` entries
  `GetEmitResolver`, `GetAliasedSymbol`, `isValidPropertyAccessForCompletions`,
  `getNameFromIndexInfo`, `TryGetThisTypeAtEx` and the four
  `getTypeArgumentsFromNode(s)`-adjacent accessors that only the API calls.
  `exports.go` is mostly accessors over existing Rust methods and takes the
  `equivalent` disposition with the Rust site; `printer.go`'s string entry
  points map to `type_display.rs`; the `Report*` methods of `symboltracker.go`
  and `nodecopy.go` map to one Rust tracker. The C4 wrappers
  (`GetJsxFactoryEntity`, `GetJsxFragmentFactoryEntity`,
  `GetTypeReferenceSerializationKind`, `GetJsxIntrinsicTagNamesAt`,
  `GetContextualTypeForJsxAttribute`) are C5's with a dependency on C4's
  functions. Uncalled services (`ForEachExportAndPropertyOfModule`,
  `GetConstructSignatures`, `IsSymbolReferencedInFile`) are ported and
  disposed `mapped`, with no replay witness expected.
- Exit: `phase2_audit.py check --audit data/phase2/c5-audit.json` passes with
  no `gap`.

### C5.2 The public checker query surface

- Exists: `query.rs`, `query_location.rs`, `query_names.rs` (`GetTypeAtLocation`,
  `GetSymbolAtLocation` and the location classification), `handles*.rs`,
  `tsr_api`'s handle model.
- Build: the `exports.go` surface as public methods of the checker operation:
  the intrinsic type accessors, `GetUnionType`, `GetPropertiesOfType`,
  `GetPropertyOfType`, `GetSignaturesOfType`, `GetResolvedSignature`,
  `GetContextualTypeForArgumentAtIndex`, `GetContextualTypeForObjectLiteralElement`,
  `GetIndexSignaturesAtLocation`, `ResolveName`, `GetBaseTypes`,
  `GetApparentType`, `GetBaseConstraintOfType`, `GetTypePredicateOfSignature`,
  `TypePredicateToString`, `GetExpandedParameters`, `GetMergedSymbol`,
  `GetImmediateAliasedSymbol`, `GetTargetSymbol`, `GetTypeOnlyAliasDeclaration`,
  `ResolveExternalModuleName`, `ResolveExternalModuleSymbol`, `TryFindAmbientModule`,
  `GetGlobalSymbol`, `GetSymbolFlags`, `GetDeclaredTypeOfSymbol`,
  `GetTypeOfSymbol`, `GetNonMissingTypeOfSymbol`, `GetConstraintOfTypeParameter`,
  `GetDefaultFromTypeParameter`, `GetTrueTypeOfConditionalType`,
  `GetFalseTypeOfConditionalType`, `IsArrayType`, `IsTupleType`,
  `IsArrayLikeType`, `IsReadonlySymbol`, `HasEffectiveRestParameter`,
  `GetLocalTypeParametersOfClassOrInterfaceOrTypeAlias`, `GetJsxNamespace`,
  `GetJsxFragmentFactory`, `WasCanceled` (the query exists; its semantics are
  C6's), and the rest of the file; each keeps the operation-borrow model
  (ADR 0008) and returns ids usable only through the checked view, so `tsr_api`
  can expose them without new retention rules.
- Exit: every `exports.go` function has a Rust entry point with a marker or an
  `equivalent` disposition; the API contract of C5.8 passes.

### C5.3 Name accessibility and symbol chains

- Exists: `accessibility.rs` (declaration accessibility sharing the
  name-chain and container algorithms of name serialization),
  `node_builder_names.rs`, `node_builder_containers.rs`, `class_accessibility.rs`.
- Build: the 18 unmarked `symbolaccessibility.go` functions
  (`getAccessibleSymbolChain`, `GetAccessibleSymbolChain`,
  `IsSymbolAccessibleByFlags`, `IsSymbolAccessible`, the `symbolTableID*`
  keys over locals, exports, resolved exports, members and globals,
  `hasExternalModuleSymbol`, `hasNonGlobalAugmentationExternalModuleSymbol`,
  `getExternalModuleContainer`, `getFileSymbolIfFileSymbolExportEqualsContainer`,
  `getQualifiedLeftMeaning`, `isUMDExportSymbol`, `isNamespaceReexportDeclaration`,
  `isPropertyOrMethodDeclarationSymbol`, `getClassExpressionNameTable`); the
  accessibility diagnostics of declaration emit (`TS4023`, `TS4025`, `TS4031`,
  `TS4052`, `TS4060`, `TS4076`, `TS4082`, `TS9010`) reported through the
  symbol tracker with the pin's messages, spans and related information for
  the incoming rows; the `symbolTableID*` keys are the cache identities of
  chain lookups and must key on the same tables the pin keys on.
- Exit: the incoming accessibility rows match; a direct case per chain shape
  (local, exported, re-exported through a namespace, UMD, `export =`
  container, class expression name table, global augmentation) against a
  native observation.

### C5.4 Type-node serialization, reuse, caches and truncation

- Exists: the node-builder modules and their tests; `type_display.rs` (the
  builder cache and emit context reused across diagnostic calls; completed
  entries retain their AST frames; uncached output is released); the
  `tsr_nodebuilder` flag and tracker contract; B03 and B08 under C2 until
  attributed.
- Build: the 56 unmarked `nodebuilderimpl.go` functions, in particular the
  expansion and truncation control (`checkTruncationLengthIfExpanding`,
  `isExpandableType`, `shouldExpandType`, `isActivelyExpanding`,
  `checkTypeExpandability`, `getAccessStack`, `createElidedInformationPlaceholder`,
  `ReportTruncationError`, `noErrorTruncation`), the reuse decisions
  (`typeNodeIsEquivalentToType`, `tryGetResolvedSymbolFromTypeNode`,
  `existingTypeNodeIsNotReferenceOrIsReferenceWithCompatibleTypeArgumentCount`,
  `serializeTypeName`, `isIdentifierTypeReference`, `typesAreSameReference`),
  chains and names (`lookupSymbolChain`, `sortByBestName`, `symbolToName`,
  `canUsePropertyAccess`, `getPropertyNameNodeForSymbolFromNameType`,
  `classifyPropertyName`, `trackComputedName`, `shouldWriteTypeParametersInQualifiedName`
  with the fourth scoped `typeParameterSymbolList`: both repeated-symbol
  suppression sites and nested success and error scope restoration, as the
  Phase 1 plan specifies), module specifiers (`canHaveModuleSpecifier`,
  `TryGetModuleSpecifierFromDeclaration` and its worker,
  `moduleSpecifierResultForSymbol`, `moduleSpecifierResolvesToSymbol`,
  `getModuleSpecifierOverride`, `rewriteModuleSpecifier`), signatures and
  parameters (`typePredicateToTypePredicateNode`, `symbolToTypeParameterDeclarations`,
  `getEffectiveParameterDeclaration`, `serializeInferredReturnTypeForSignature`,
  `tryGetThisParameterDeclaration`, `indexInfoToIndexSignatureDeclarationHelper`
  with `getNameFromIndexInfo`, `hasTypeAnnotation`), anonymous and alias
  types (`getTypeAliasForTypeLiteral`, `createAnonymousTypeNode`,
  `getParentSymbolOfTypeParameter`, `typeParameterShadowsOtherTypeParameterInScope`,
  `isMappedTypeHomomorphic`, `createExpressionWithTypeArguments`,
  `lookupInstantiatedTypeArgumentNodes`, `lookupExpressionChainTypeArgumentNodes`);
  `nodecopy.go`'s reuse and recovery scopes (`reuseNode`,
  `walkNodeForExpandability`, `markError`, `startRecoveryScope`,
  `endRecoveryScope`, `createRecoveryBoundary`, `finalizeBoundary`,
  `getExistingNodeTreeVisitor`, `getEnclosingDeclarationIgnoringFakeScope`,
  the error fallback node stack); the tracker's `Report*` methods; the synthetic property elision comments and
  the reparsed-module scope lookup that C2 closed (`node_builder.rs`,
  `node_builder_scope.rs`) are retained, not redone; `createRecoveryBoundary`'s
  `checkNotCanceled` (`nodecopy.go`): a canceled checker refuses the node
  builder too, so hover and services after a cancel panic as the pin does
  (C6.5 gives the semantics);
  the `nodebuilder.go` entry points (`SerializeTypeForDeclaration`,
  `SerializeTypeForExpression`, `SerializeReturnTypeForSignature`,
  `SerializeTypeParametersForSignature`, `SignatureToSignatureDeclaration`,
  `SymbolToParameterDeclaration`, `TypeParameterToDeclaration`,
  `IndexInfoToIndexSignatureDeclaration`, `SymbolToExpression`,
  `SymbolToEntityName`, `SymbolToNode`, `SymbolToTypeParameterDeclarations`,
  `TryJSTypeNodeToTypeNode`, the context push, pop and exit helpers) and the
  `printer.go` string entry points with their flag conversions. Where the
  C2.3 and C2.5 traces attribute B03 or B08 to a missing builder branch, C5
  takes them by the ordinary handoff.
- Exit: the incoming node-builder rows match; `display_parity` has no
  C5-attributed difference; a direct case per reuse decision and per
  truncation path against a native observation; the builder cache and
  retention tests still pass.

### C5.5 Services and hover: the recorded fourslash reference

- Exists: nothing for `services.go`; `nodebuilder_hover.go` unported; the
  pinned fourslash suite (4,356 tests) drives `internal/ls`, whose 48 files
  call the 43 used services; the diagnostic Go overlay pattern of C2.10.
- Build, the reference: `scripts/phase2_services.py record` builds a
  diagnostic overlay of the pin that wraps every exported `services.go` method
  and the hover expansion entry points with a recorder, runs the pinned
  fourslash suite under it, and writes `data/phase2/services-replay.json.xz`
  with a per-test record and `data/phase2/services-replay.json` with the
  manifest (pin, overlay fingerprint, the tests that actually ran, call counts
  per operation, digests). Fourslash drives the project service through
  edits, so a call's result depends on the snapshot and on that checker's
  earlier queries: each record carries the snapshot identity (file texts and
  options after each edit) and the per-checker call order, and replay
  reproduces both before comparing. Arguments and results are identified by
  recorder-local identities: a token assigned at first sight to each type,
  symbol and node, with its construction dependencies recorded (kind,
  declaration positions and flags, constituents and arguments by token), so
  that a synthetic or instantiated type is identified by how it was built and
  not by a display string; the recorder never calls the node builder, a
  comparator, a formatter or a lazy checker query, and never requests an id
  the pin assigns lazily. Neutrality is proved, not assumed: the suite runs
  with and without the recorder and the fourslash outcomes and every call
  result must be identical. The 385 tests the pin skips as known failing are
  reported as skipped. The record and replay contract is proved first on one
  bounded edited-program case before the whole suite is instrumented. The
  recorder is not the ADR 0019 transport patch. `verify` reproduces the
  manifest.
- Build, the port: the 66 `services.go` functions over the Rust checker
  operation (`GetSymbolsInScope`, `GetExportsOfModule`,
  `GetExportsAndPropertiesOfModule`, `IsValidPropertyAccess`,
  `IsValidPropertyAccessForCompletions`, `GetAllPossiblePropertiesOfTypes`,
  `GetNonOptionalType`, the index and element type accessors,
  `GetCallSignatures`, `GetConstructSignatures`, `GetApparentProperties`,
  `TryGetMemberInModuleExports` and its properties variant, `GetContextualType`
  with `ContextFlags`, `GetResolvedSignatureForSignatureHelp`, `SkipAlias`,
  `GetRootSymbols`, `GetMappedTypeSymbolOfProperty`, `GetExportSymbolOfSymbol`,
  `GetExportSpecifierLocalTargetSymbol`, `GetShorthandAssignmentValueSymbol`,
  `GetSymbolsOfParameterPropertyDeclaration`, `IsDeclarationUsed`,
  `IsSymbolReferencedInFile`, `GetReferencesToSymbolInFile`,
  `GetTypeArgumentConstraint`, `IsTypeInvalidDueToUnionDiscriminant`,
  `GetJsxIntrinsicTagNamesAt`, `GetContextualTypeForJsxAttribute`,
  `GetConstantValue`, `GetCandidateSignaturesForStringLiteralCompletions`,
  `GetTypeAtPosition`, `GetTypeParameterAtPosition`,
  `GetContextualTypeForArrayLiteralAtPosition`, `GetFirstTypeArgumentFromKnownType`,
  `GetPropertySymbolsFromContextualType`, `GetPropertySymbolOfDestructuringAssignment`,
  `GetSignatureFromDeclaration`, `IsLibSymbolForHoverVerbosity`,
  `IsLibTypeForHoverVerbosity` and their helpers); `nodebuilder_hover.go`
  (`ExpandSymbolForHover` and its class, interface, enum, module and type
  alias expansions with truncation); `scripts/phase2_services.py replay`
  drives the Rust checker through `tsr_api`-level entry points with the
  recorded requests and compares results per operation; an operation whose
  reference calls all match is `replayed`; an operation left to Phase 5 needs
  the owner's approval and is listed in the C5 record.
- Exit: `record` and `verify` reproduce the manifest; `replay` reports every
  operation `replayed` or approved; `c5_services` true.

### C5.6 The emit resolver

- Exists: `emit_resolver.rs`, `emit_visibility.rs`, `emit_reference.rs`,
  `emit_scopes.rs`, `emit_checks.rs`; the declaration-diagnostics path in
  `tsr_compiler`; C3.5's alias marking and C4.7's metadata marking.
- Build: the 39 unmarked `emitresolver.go` functions: visibility
  (`IsDeclarationVisible`, `IsEntityNameVisible`, `isSymbolAccessible`,
  `IsSymbolAccessible`, `IsNameResolvable`, `noopAddVisibleAlias`), alias
  queries (`IsReferencedAliasDeclaration`, `IsValueAliasDeclaration`,
  `isValueAliasDeclarationWorker`, `isAliasResolvedToValue`,
  `IsTopLevelValueImportEqualsWithEntityName`, `isConstEnumOrConstEnumOnlyModule`,
  `isCommonJSModuleExports`, `aliasMarkingVisitorWorker`,
  `getMeaningOfEntityNameReference`, `MarkLinkedReferencesRecursively`,
  `GetExternalModuleFileFromDeclaration`, `SetReferencedImportDeclaration`),
  parameters and members (`IsOptionalParameter`, `IsLateBound`,
  `RequiresAddingImplicitUndefined`, `RequiresAddingImplicitUndefinedUnsafe`,
  `requiresAddingImplicitUndefinedWorker`, `declaredParameterTypeContainsUndefined`,
  `isOptionalUninitializedParameterProperty`, `isRequiredInitializedParameter`,
  `IsExpandoFunctionDeclaration`, `GetEnumMemberValue`, `GetConstantValue`,
  `GetEffectiveDeclarationFlags`), serialization entry points
  (`CreateTypeOfDeclaration`, `CreateTypeOfExpression`,
  `CreateReturnTypeOfSignatureDeclaration`, `CreateTypeParametersOfSignatureDeclaration`,
  `TryJSTypeNodeToTypeNode`), the C4 wrappers (`GetJsxFactoryEntity`,
  `GetJsxFragmentFactoryEntity`, `GetTypeReferenceSerializationKind`) and
  `newEmitResolver`; the locking model: as the pin's `getCheckerForFileNonExclusive`
  hands the resolver a checker without the pool lock and every resolver
  method then takes the checker's lock, each Rust resolver method acquires
  the checker's operation for that call and releases it, never holding one
  across calls, and the declarations transformer may call it from any thread
  (C6.3 supplies the acquisition; today's resolver borrows the caller's
  operation and is migrated here); the late-declaration retention failure
  C1 traced (`WrongOwner` in `late_statements`) was fixed in C2, and C5 keeps
  the LLDB-confirmed path as a regression witness, touching the declarations
  transformer's retention only under Phase 3's review (decision 4).
- Exit: the resolver rows match, including the three C1 returns except the
  B09 row; every exported resolver method has a Rust entry point consumed by
  the declarations transformer or a direct case; the panic row completes.

### C5.7 The structured pre/post-emit comparison and the emit-order rows

- Exists: C0's structured pre/post comparison over every executed variant,
  which found the three emit-order rows; C2's traces
  (`data/phase2/c2-emit-handoffs/`): the native harness builds two programs,
  checks the first before emit and calls `Program.Emit` on the second before
  its semantic diagnostics; `incorrectRecursiveMappedTypeConstraint` is first
  resolved by the constant-enum inlining transformer asking the resolver's
  `GetConstantValue`, the other two by the import-elision transformer's
  `MarkLinkedReferencesRecursively`; the emitter enables import elision only
  when `verbatimModuleSyntax` is off and the file is not JavaScript, and
  constant-enum inlining only when `isolatedModules` is off, and both only
  for the files the program emits; `compare_errors` adds a `native_pre_post`
  difference whenever the pin's pre- and post-emit sets differ, so a Rust
  post-emit set that matches every native field still compares `different`
  while Rust's emit is unexecuted; the rule that only an observed missing
  transform or resolver operation becomes a blocker.
- Build: the two resolver paths of C5.6 (`GetConstantValue` and the linked-
  reference marking); a second fresh program per emitting row, as the harness
  builds one, on which the corpus driver runs the pin's transform schedule
  without writing output (the constant-enum inlining and import-elision
  visitors with the emitter's per-file option guards and the program's
  emitted-file selection) before collecting that program's diagnostics
  (decision 3); the comparator migration: the Rust observation records that
  emit executed, with the schedule identity (transforms run, files, guards)
  and its authentication, and `compare_errors` compares the native post-emit
  set with Rust's post-emit set when both executed emit, keeping the
  `native_pre_post` refusal whenever Rust's emit is unexecuted. This
  reproduces the pin's harness order and changes the three rows' diagnostics,
  which is the point; it does not change the pre-emit checker API. If decision
  3 is declined, the entry is re-owned to Phase 3 with the trace and stays an
  explicit joint blocker. Either way the structured comparison is rerun at the
  C5 exit and any new pre/post difference is attributed to its operation and
  owner.
- Exit: the three rows match through both transform paths, or the entry is
  re-owned with the trace; the pre/post comparison reports no unattributed
  difference; a row whose emit is unexecuted still compares `different`.

### C5.8 Direct contracts

- Exists: the C1 to C4 contracts; the node-builder retention, cache, reuse
  and stack tests; `tsr_api` and `tsr_project` tests.
- Build: `crates/tsr_compiler/tests/c5_contracts.rs`, one test per contract,
  each over production entry points with a pinned Go counterpart named in its
  doc comment:
  1. owner retention, in three lifetimes: operation-local temporaries
     (uncached builder output, the resolver's per-call state) are released
     with the operation; checker-owned caches (completed diagnostic-builder
     entries with their AST frames, the resolver's marks) live as long as the
     checker and are reused across calls; retained roots (leases, retained
     results, registries) keep retired storage alive until the last root
     drops, as ADR 0012 and `retirement_rejects_retained_access_while_storage_stays_live`
     already require; a retired generation's handles are refused immediately,
     and the counters return to baseline only after the last owner or root
     drops;
  2. source context: declaration diagnostics for a file are produced from
     that file's view, and the late-declaration case of the augmentation
     row completes with the pin's diagnostics;
  3. accessibility: the chain shapes of C5.3, with the pin's diagnostics and
     related information;
  4. serialization and reuse: an annotated declaration reuses its type node,
     an inferred one serializes, a recovery scope drops a failed subtree, and
     `noErrorTruncation` changes the pin's truncation exactly;
  5. hover expansion: a class, an interface, an enum, a namespace and a type
     alias expand as the recorded fourslash hovers show;
  6. services replay: the `phase2_services.py replay` contract over a fixed
     subset of recorded tests runs inside the test suite, and a changed result
     fails it;
  7. the public API: every `exports.go` entry point answers over a checked
     program, and answers again after an unrelated query;
  8. the resolver's alias and visibility queries over C3's marking state
     (joint with C3's contract 6) and C4's JSX and metadata state (joint with
     C4's contracts 1 and 7);
  9. two checkers over one program produce independent builders, caches and
     resolver marks, and a panic inside a builder retires only its checker.
- Exit: `cargo test -p tsr_compiler --test c5_contracts` in debug and
  release; the v2 receipt is recorded by the producer (C5.9).

### C5.9 Producer wiring

- Exists: the per-checkpoint metric helper and the per-variant blocker
  ownership of C2.12; `sprints/P2B.toml` with `P2B-C5` waiting on
  `run.checker.c5_complete`.
- Build: the producer reads `data/phase2/c5-claims.json`,
  `data/phase2/c5-audit.json`, `data/phase2/c5-baseline.json.gz`, the
  contracts receipt (`observe --witness c5-contracts`) and the services
  replay record, and emits `c5_open` (incoming and attributed rows with a
  currently unmet domain not covered by a valid exclusion), `c5_handoffs`,
  `c5_regressions`, `c5_failures` (every unattributed failure plus failures
  traced to C5, in any crate), `c5_blockers_open`, `c5_audit_complete`,
  `c5_services` (the manifest verifies, the replay record is current for the
  exit executable, every operation is `replayed` or approved),
  `c5_contracts` and `c5_complete`. All new authorities are added to
  `[checker]` `inputs`; the services manifest and replay record are inputs.
- Exit: `cargo xtask validate`; `phase2_producers.py checker` emits the nine
  metrics; `scripts/tests/test_phase2_c5.py` shows that a differing incoming
  row keeps `c5_complete` false, that a stale replay record keeps
  `c5_services` false, that a handoff without its trace is rejected, and that
  changing each new input invalidates the recorded result.

## 5. Dependencies and owners

| Dependency | Owner | State for C5 |
| --- | --- | --- |
| Type-level causes behind any new declaration or display difference | C2, C4 | C2 closed its display refusals and the TS40xx rows before its exit; C4's rows may hand new ones, attributed by trace; C5 takes only serialization causes |
| Alias marking semantics | C3 | C3 marks, C5 queries; contract 8 is joint |
| JSX entities, metadata marking and the services over them | C4 | C5.6 exposes the resolver entry points C4's contracts need, before C4 exits |
| Cancellation (`WasCanceled`, the node builder's refusal of a canceled checker), the checker pool and generation disposal | C6 | C5 keeps the query and the retention rules; C6 gives them their semantics and the per-call resolver acquisition |
| The declarations transformer, the JavaScript transform schedule behind the emit-order rows and `.d.ts` parity | Phase 3 | the transformer is Phase 3 code; C5 owns the resolver and the seam; the emit-order entry closes here only under decision 3 |
| Content mappers (3 declaration rows not loaded) | Phase 5 | withheld under B06 |
| The editor features over the services, and the fourslash semantic passes | Phase 5 | C5 supplies the operations and the replay; Phase 5 runs the suite through the ADR 0019 transport |
| The `checker` recording | owner | C5.9 |
| Owner decisions of section 9 | owner | before C5.1 |

C5 contract preparation starts with C0's assets and does not wait for C4:
C5.2, C5.3, C5.5's recording and C5.8's retention contracts depend only on
C1; C5.4's incoming rows arrive from C2; C5.6's JSX wrappers arrive from C4.

## 6. Delivery order

1. C5.0 the fresh gap map and claims file; C5.1 the audit, reviewed before
   production changes; C5.5's `record` and `verify` early, because the
   services reference is the long pole and does not depend on other
   checkpoints.
2. C5.6 (the resolver) first among the ports, because C3, C4 and Phase 3
   consume it; the late-declaration panic closes here.
3. C5.3 and C5.4 together (accessibility feeds serialization), consuming the
   incoming rows from C1 and C2 as they arrive.
4. C5.2 and C5.5's port, with the replay growing per operation.
5. C5.7 after decision 3; C5.8 grows alongside 2–4; C5.9 last, then the exit
   full run and the record.

Intermediate runs use the recorded 300-variant sample plus the incoming rows
(`--sample --case ...`) and the services replay over a fixed subset. Full runs
are C5.0 and the exit.

## 7. Executable exit checks

`target/phase2/rust-c5-start/comparison.json` below is the C5-start report
from C5.0; use its actual saved path. Exit output is a fresh directory.

```sh
export PATH="$(mise where go)/bin:$PATH"
python3 scripts/phase2_inventory.py check
python3 scripts/phase2_corpus.py run --native target/phase2/native --output target/phase2/rust-c5
python3 scripts/phase2_compare.py report --native target/phase2/native --rust target/phase2/rust-c5 --previous target/phase2/rust-c5-start/comparison.json --record
python3 scripts/phase2_blockers.py build --native target/phase2/native --rust target/phase2/rust-c5 --record
python3 scripts/phase2_audit.py check --audit data/phase2/c5-audit.json
python3 scripts/phase2_services.py verify                                     # the recorded fourslash reference reproduces its manifest
python3 scripts/phase2_services.py replay --output target/phase2/services-c5  # every operation replayed or approved
cargo test -p tsr_compiler --test c5_contracts --locked && cargo test -p tsr_compiler --test c5_contracts --locked --release
python3 scripts/phase2_producers.py observe --witness c5-contracts           # the v2 receipt, with the dependency and asset closure
python3 scripts/s08_relater.py build  --output target/s08/relater-c5
python3 scripts/s08_relater.py parity --output target/s08/relater-c5         # 105/105, all_cases_match true, both implementations
python3 scripts/phase2_producers.py checker --rust target/phase2/rust-c5    # c5_open 0, c5_regressions 0, c5_failures 0, c5_blockers_open 0, c5_audit_complete, c5_services, c5_contracts, c5_complete; c5_handoffs and display_parity reported
python3 -m pytest scripts/tests/test_phase2_*.py -q
python3 scripts/checks.py fmt && python3 scripts/checks.py clippy
cargo xtask validate
# Assert the C5 metrics explicitly; `check P2B` stays pending until C7.
```

`cargo xtask run checker` and `cargo xtask status --record` are the owner's.
The services recording runs the pinned Go suite once per pin under the
diagnostic overlay; it is a producer input, not a corpus capture.

## 8. Evidence reuse rules

- The C0 native capture is reused only when its read-only current-input
  validator succeeds; the services overlay is a separate diagnostic build and
  never touches the canonical capture or `upstream/`.
- A Rust capture supplies current acceptance only when replay succeeds against
  the captured executable with `source_stable: true`.
- The C5-start baseline is reused only while it names the current native
  observation and inventory digests.
- The services reference is reused while `verify` reproduces its manifest at
  the same pin and overlay fingerprint; the replay record is current only for
  the exit executable.
- Claims are per row and per cause; incoming rows are counted from the
  handing checkpoint's validated entries; the producer rejects a handoff or an
  incoming reference without its trace.
- A difference is accepted only through the divergence ledger (ADR 0004). C5
  expects none; the emit-time marking order of decision 3 changes when a
  marking runs, never what is reported.
- Emitted output is never C5 evidence; declaration diagnostics, display
  strings, serialized nodes observed through the resolver, and the replay
  results are.
- Mapping progress is reported by markers and the ledger, never as behavioral
  coverage; the corpus, the replay and the C5.8 contracts are the behavioral
  evidence.

## 9. Owner decisions before C5 starts

1. **The services reference.** Record the checker calls the pinned fourslash
   suite makes through a diagnostic Go overlay and replay them against Rust
   (C5.5), as the Phase 2 plan proposes. The alternative is hand-written
   direct cases per function, which would have no pinned reference for the
   call patterns the language service actually uses. Proposed: record; scope
   the recording to the whole suite, with per-operation reporting.
2. **Operations left to Phase 5.** Any `services.go` operation not `replayed`
   at the C5 exit needs an owner-approved entry in the C5 record; the default
   is that all 66 are ported. Confirm.
3. **The emit-order rows.** Let the corpus driver build the harness's second
   program and run the pin's transform schedule on it without writing output
   (constant-enum inlining and import elision, with the emitter's per-file
   option guards and emitted-file selection) before collecting its
   diagnostics, and migrate `compare_errors` to compare post-emit sets when
   Rust's emit is recorded as executed. This reproduces the pin's harness
   order and changes the three rows' diagnostics to the post-emit set the
   baseline holds. The alternative keeps the entry as an explicit joint
   blocker until Phase 3 emits.
4. **Retention at the transformer seam.** C2 fixed the late-declaration
   `WrongOwner` failure; C5 keeps the regression witness and touches the
   declarations transformer's retention (Phase 3 code) only under Phase 3's
   review. Confirm.
5. **Hover expansion.** `nodebuilder_hover.go` is C5's (the builder half);
   the hover feature over it is Phase 5's. Confirm.
6. **The public API now.** Expose the whole `exports.go` surface in C5.2
   rather than as Phase 5 needs it, so the API contract is complete inside
   Phase 2. Confirm.
7. **Exit run.** `c5_complete` is computed only from a recorded full capture;
   the recording is the owner's. Confirm.
