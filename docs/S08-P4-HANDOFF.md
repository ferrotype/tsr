# S08 P4 handoff

**Current continuation:** see [the 2026-09-13 takeover review](S08-P4-REVIEW.md)
for the reviewed dirty increment, eight-commit audit, corrections and remaining
work. The sections below retain the earlier handoff and Claude's implementation
history. Inventory-07 is now the latest full execution run; the review adds
focused reruns rather than claiming a new full capture.

The owner requested a pause after active work. **P4 is incomplete; P5–P8 have
not started.** This is an implementation checkpoint, not an acceptance result.
The original request was P4 on PR #19, then one stacked PR per later checkpoint.

## Starting point and scope

- Branch: `codex/s08-p4`; parent PR #19 is `codex/s08-p3-remaining` at
  `37097c4211c1fef3e0ef4e68bdfb7aac68ef8f1e`.
- Pinned Go tree: `1f70213d4922b434345f639b441681e470c7cfc1`.
- Read `docs/CODEX-RUST-GUIDELINES.md` before implementation and
  `docs/S08-implementation-plan.md` for the accepted checkpoint scope.
- Acceptance remains **9,369** variants, with **1,359** informational variants
  separate. No thresholds, eligibility predicates or divergence policy changed.
- Requested declaration diagnostics are required, including the declaration
  transformer and checker emit resolver. An unexecuted phase is a failure even
  when native diagnostics are empty. Focused fixtures cannot certify P4 or E2.
- Never rewrite pushed history. Preserve unrelated `.playwright-mcp/` and
  `four-robots.png`; neither belongs to this checkpoint.

## What is implemented

The checker now has production expression, call, contextual-type, statement,
flow, return/predicate inference, value declaration, class, enum, module and
JavaScript/JSDoc paths. These call the P3 type/instantiation/relation machinery.
Named unsupported branches remain and must be resolved from the full inventory.

`ts_pseudochecker` ports native syntactic inference separately from semantic
checking. `ts_transformers` implements the declaration transform used by
`Program::declaration_diagnostics`; `ts_printer::emit_resolver` defines its checker
callbacks. Output is constructed and graph-validated, rather than emulating
only a list of expected diagnostics. The Program phase retains successful
results; callbacks execute outside the result-cache lock.

Node-builder work includes symbol accessibility, referenced-value resolution,
class/function object serialization, annotation reuse with recovery boundaries,
pseudo-type equivalence, and lexical scopes for signature serialization. Emit
metadata retains original nodes, generated-name identity and comment ranges.
Generated placeholder text must not be printed as a real name; remaining source
printing/name-generation work belongs to P5.

Serialization scopes are checker-owned factory nodes and binding tables; source
parents are explicitly retained. Published bound files stay immutable. Scope
allocations are charged by the census but currently live until the checker
generation is dropped, unlike native builder-local scope links. P7 must measure
this retention. Avoid premature storage redesign.

The last active implementation slice is declaration module-specifier generation
and import-attribute closure. It preserves path bytes, import resolution modes,
package JSON identity and native path/extension ordering. The final verification
section below distinguishes its compilation from native parity coverage.

## Evidence already collected

The first full inventory is **an older immutable binary**, not the final source:
`target/s08/p4-inventory-01`. Its authenticated replay covers all 10,728 variants.
Capture manifest SHA-256:
`f251dd64c0651536ee57ecac85ef19375517f819410d0e1ea2682cd2668e653c`.
Acceptance: 6,185 executed semantic diagnostics, 3,023 failed there, and 161
panicked before the phase. Only 5,433 completed all requested diagnostic phases.
Informational: 932, 413, 14, and 827 respectively. These are execution counts,
**not parity counts**. Many of the named failures have since been implemented.
Do not quote these numbers as the current pass rate.

Focused results on recorded earlier combined binaries include:

- All **420** frozen P0 relation actions: results, calls, cache/state transitions,
  type creation counts and diagnostics. Latest pre-pause replay is
  `target/s08/p4-relations-comparison-reuse-01.json`. Remaining full relation
  protocol/display/performance obligations are P5/P7, not certified here.
- **850** complete native pseudo-type tree/error-location observations. A
  4,000-level const tuple is constructed and released on a 512 KiB stack.
- **504** native declaration-accessibility selector observations.
- **90** downlevel/private-field/helper/options programs and queries.
- Assignment declarations **23/23**, isolated declarations **7/7**, and tagged
  calls **10/10** on `target/s08/p4-build-reuse-01/p2_checker`.
- Enum **15 programs/66 queries**, classes **11/27**, and contextual expressions
  **7/28** on `target/s08/p4-build-class-emit-01/p2_checker`.

Each copied build directory records a source fingerprint and executable hash.
Do not compare new source against an old binary and call it verification.
`target/s08/p4-inventory-01` retains its executable, source snapshot, request
rows and phase outcomes. Other local native captures and results also live in
`target/`; they are not silently promoted to committed evidence. Preserve them
when handing the checkout over. Request specs and oracle/replay code are tracked.

## Shared declaration failure and its fix

Several suites failed with `Checker(Arena(InvalidGraph))` when declarations
contained ordinary calls, including zero-argument calls. The failure was in the
transformer's **eager accessibility selector**, not an arena/storage invariant:
it read the special call context's second argument while installing the context,
even though native defers that read until a diagnostic actually uses it.
The fix defers selector results/errors and propagates an error only if that
selector is selected. Its regression is
`ordinary_call_context_defers_an_unused_accessibility_selector`.

Other recent native fixes include the CommonJS repeated-export assignment of
`undefined`, tagged-call diagnostic guards, contextual annotation reuse for
qualified generic constraints, and native class/private-member diagnostic
selection. Keep their focused request cases when refactoring.

## Resume order

1. Read the final verification and agent handoff notes below. Inspect the draft
   PR diff and current tree before changing anything. Do not repeat all native
   captures or broad lint suites merely to rediscover their recorded outcomes.
2. Address any compile or focused-parity failures explicitly retained below.
   Native module-specifier/attribute parity and remaining node-builder boundaries
   need particular attention; compilation is not proof of their behavior.
3. Build a **new immutable** `p4_inventory` executable and source snapshot, then
   run the full frozen inventory once. `python3 scripts/s08_p4.py --help` gives
   the capture/replay interface. Start from the largest *current* failure buckets,
   not the obsolete rankings in inventory-01. Preserve acceptance/informational
   partitions and distinguish harness failures, panics and named unsupported
   operations from native diagnostic differences.
4. Finish P4's required operations and direct lazy-library fixtures. Inspect
   `Error::Unsupported` paths in the checker and declaration transformer against
   actual requests; do not weaken eligibility to make them disappear.
5. Review and complete P4, then start P5 on its PR. P5 owns exact type/error
   baseline decoration, remaining printer/metadata/byte closure and nested stack
   tests; P6 owns exact E2 acceptance and ownership instrumentation; P7 owns the
   full relation/type-storage/performance measurements; P8 refreshes stale
   evidence, status and S07 captures, then runs the four native CI targets.

Typical focused replay (use the canonical request emitted by capture):

```sh
BINARY=/absolute/path/to/immutable/p2_checker
NATIVE=/absolute/path/to/native-capture
ACTUAL=/absolute/path/to/new-results.json
"$BINARY" "$NATIVE/requests.json" "$ACTUAL"
python3 scripts/s08_p2.py compare --native "$NATIVE" --actual "$ACTUAL" --output /absolute/path/to/comparison.json
```

Do not pass the pretty-printed `tools/s08/p4/*.json` specs directly to the Rust
adapter; it deliberately requires the canonical native request bytes. The
comparison command reports the first failed phase; the actual-results JSON
retains all program outcomes for diagnosis.

## Final verification and remaining work

The final immutable build is `target/s08/p4-pause-build-02`. It compiled
`p2_checker`, `p3_relations`, `p3_comparators` and `p4_inventory` with all compiler
features and `ts_checker/storage-pilot`, without warnings. Source fingerprint:
`651693857c9e281fbe7e21dafb8b6d359cfcf3eb6960adc60b0e902720840bc9`.
The `p2_checker` binary SHA-256 is
`e0404c3f821a1712d33698bf39df6c8e9bceaed41c9779cc1f163c39d2d36949`.
`build.json` includes every source hash; `source-snapshot/` retains those bytes.
`inventory-build.json` records that same snapshot. A subsequent bookkeeping-only
edit corrected 18 source-reference comments; a checked diff confirms non-comment
lines are unchanged (`p4-pause-checks/comment-only-delta.json`). The strict source
fingerprint consequently changed: rebuild before a new inventory capture, rather
than editing the recorded hashes.
The earlier pause-build-01 failed compilation and contains no copied binaries.

The consequential deferred-selector regression passed. The existing pinned-Go
path-comparison observations passed after sharing the relative-path helper.
Tracking views were regenerated after correcting the source-reference comments,
and `cargo xtask validate` passed. Existing correctness/performance evidence is
stale where the changed inputs require it; no producer outcomes were fabricated.
No full corpus, fresh Go captures, broad lint rerun, performance capture or
four-target CI was performed for this pause. CI is deliberately deferred on the
draft checkpoint (`[skip ci]` commit), not reported as passing.

Latest full-program focused comparisons total **94/98 exact**:

| Suite | Exact | Comparison under `target/s08/` |
| --- | ---: | --- |
| Ordinary declarations | 13/13 | `p4-pause-checks/declaration-emit-comparison.json` |
| Generated declarations | 8/8 | `p4-pause-checks/declaration-generated-comparison.json` |
| Pseudo/semantic serialization | 12/12 | `p4-pause-checks/declaration-serialization-comparison.json` |
| Annotation reuse | 14/14 | `p4-annotation-reuse-comparison-pause-02.json` |
| JavaScript declaration classes | 5/5 | `p4-pause-js-classes-comparison-01.json` |
| CommonJS declarations | 13/14 | `p4-pause-common-js-comparison-01.json` |
| Import attributes | 16/18 | `p4-pause-attributes-comparison-01.json` |
| Class expansion | 13/14 | `p4-class-expansion-final-verified.json` |

Four known focused failures are ready for the next implementer:

1. **CommonJS destructured `require`:** `module_aliases.rs`'s
   `get_target_of_alias_declaration` handles `VariableDeclaration` but not
   `BindingElement`, producing the named Unsupported external/CommonJS alias
   result. Native capture: `target/s08-p4-declaration-common-js-native-01`.
2. **Two import-attribute diagnostic chains:** `nonstring-value` and
   `module-attribute-method` omit intermediate TS2530. In
   `relater_properties.rs::members_related_to_index`, a failed `self.related`
   returns false without reporting `Property_0_is_incompatible_with_index_signature`.
   Pinned `relater.go:4688–4693` adds that wrapper using `symbolToString(prop)`.
   Native capture: `target/s08/p4-import-attributes-native-01`.
3. **External unique symbol declaration:** `external-unique-value` emits extra
   declaration TS2527 for `outer` at `/case.ts:42..47`; native emits none. All
   semantic/query results match. Native capture:
   `target/s08/p4-declaration-class-expansion-native-03`.

The new `module_specifiers*.rs` generation/inversion and compiler host data APIs
compile, but dedicated native module-specifier parity is **unmeasured**. The host
retains the loader resolver's package cache and uses its existing package-directory
algorithm only for native runtime-dependency discovery. Existing import resolution
is not recomputed. Contextual export-equals selection, external-container ranking
and generic qualified-value paths still contain named node-builder boundaries.
These are follow-ups; the new path generator does not certify the entire name
serialization closure.

The current source still needs a new full execution inventory. The next
implementer can build a fresh authenticated snapshot and run:

```sh
python3 scripts/s08_p4.py build --output target/s08/p4-resume-build-01
python3 scripts/s08_p4.py run --tier all --build-record target/s08/p4-resume-build-01/build.json --output target/s08/p4-inventory-02
python3 scripts/s08_p4.py replay target/s08/p4-inventory-02
```

After a source edit, run `scripts/s08_p4.py build --output <new-build-directory>`
first; the runner correctly rejects stale binary/source pairs. The old full
inventory is useful for provenance and historical attribution, not the current
remaining-work count. All subagent tasks and local build/replay commands have
completed; no background continuation is scheduled.

## Resume increment (2026-09-12)

Two passes on the paused draft, committed together as one increment.

**First pass.** The three retained focused failures were reproduced against
their native captures with the handoff's immutable binary, root-caused in the
pinned Go and fixed: the CommonJS destructured `require` alias (`BindingElement`
arm of `getTargetOfAliasDeclaration` plus the `getExternalModuleRequireArgument`
port in `module_aliases.rs`/`external_aliases.rs`), the two missing TS2530 chains
(`Property_0_is_incompatible_with_index_signature` and `indexInfoRelatedTo` in
`relater_properties.rs`), and the extra declaration TS2527 (upstream widens a
unique symbol in `checkExpressionForMutableLocation`). The declaration suites went
from 94/98 to 98/98 exact; the 14 P3 suites replay 108/108; nine specs under
`tools/s08/p4/` that had never been captured now match pinned Go, 82/82 programs.
Two full inventories were run, `target/s08/p4-inventory-02` from the pristine
checkpoint and `-03` from the fixed source: acceptance variants completing all
requested phases 8,457 → 8,471, semantic failures 848 → 833, declaration failures
61 → 58, the external/CommonJS alias bucket 21 → 0, no new failure reason.

**Second pass.** The fixes above were reviewed against pinned Go
(`checker.go:16036`, `utilities.go:233`, `ast/utilities.go:2885`,
`relater.go:4688–4715`, `checker.go:25855` and `25865`) and match. The three
largest tractable inventory-03 buckets were then closed:

- `checkExpressionWorker` (153 acceptance variants): regular expression literals
  with the scanner re-scan grammar check, `delete` with its optional/read-only
  rules, and `new.target`/`import.meta` with their grammar and module checks
  (`regular_expressions.rs`, `delete_expressions.rs`, `meta_properties.rs`).
  JSX expression kinds remain the named boundary.
- `checkSourceElementWorker: statement/type family` (65): `debugger` under the
  ambient-context grammar check, missing declarations, and the two kinds
  upstream's switch has no case for (`export as namespace`, `;` class members).
- `missing type alias declaration` (53): `getDeclaredTypeOfTypeAlias` accepted
  only TS aliases where upstream's `IsTypeOrJSTypeAliasDeclaration` also takes
  JSDoc `@typedef`/`@callback` aliases.

Focused regressions cover each (`checker_semantics.rs`). An 18-variant probe
drawn from those buckets on `target/s08/p4-claude-build-01` runs 17 of 18
through every requested phase; the 18th now reaches the excess-property
reporting boundary below. Clippy, formatting, ledger validation and tracking
views are current. **The full 10,728-variant inventory was not re-run after
this pass**; build a new immutable snapshot and run it before quoting counts.

**Third pass.** The three largest remaining named boundaries were closed while
the owner's full inventory re-run was in flight (its binary and snapshot are
immutable, so these edits do not affect it):

- `checkSourceFile: unused declarations pass` (125): the checker already
  recorded reference kinds (`query.references`, fed by the resolver's
  `symbolReferenced` hook and `markPropertyAsReferenced`), so the pass is the
  `checkUnusedIdentifiers` family in `unused_identifiers.rs` plus
  `registerForUnusedIdentifiersCheck` at all twelve upstream sites (signature
  declarations, `infer`, blocks and loops with locals, case blocks, classes and
  class expressions, namespaces, type aliases, interfaces, external-module source
  files). `checkUnusedRenamedBindingElements` runs from `checkSourceFile`; the
  pass itself runs once per file after the type check, as an error under
  `noUnusedLocals`/`noUnusedParameters` and as a suggestion otherwise
  (`recorded_suggestions` now requests it, as `GetSuggestionDiagnostics` does).
- `report excess properties: source object expression` (43): `hasExcessProperties`
  now reports. `RelationErrors` carries the relater's error node so the property
  name inside the literal can replace it; `getSuggestionForNonexistentProperty`
  and `getSpellingSuggestionForName` are ported over the target's property
  symbols; the JSX attribute branch is the remaining named boundary.
- `resolveExternalModule: CommonJS/ESM mismatch details` (41): the Node16/Node18
  branch with `createModeMismatchDetails` as the message chain and the
  `import =`/type-only/`import type` message selection. Go's repopulate marker
  (tsbuildinfo only) has no Rust counterpart.

Focused regressions cover each (`checker_semantics.rs`: four unused-pass tests,
one excess-property test with the spelling suggestion and the discriminated
union, one Node16 test over a two-file fixture). Clippy, formatting, ledger
validation and tracking views are current (3,577 functions mapped). **The full
inventory was not re-run after this pass either.**

**Fourth pass.** Three more buckets, sized against the code rather than their
names:

- `resolveStructuredTypeMembers: type family` (45 + 1 declaration): not a
  missing family. Rust dispatches every Go family; the failing variants extend an
  `any` base, which upstream records as the base type and reads through
  `getSignaturesOfType`/`getIndexInfosOfType` with the `anyBaseTypeIndexInfo`
  special case. `resolveObjectTypeMembers` now does the same instead of resolving
  structured members on the base directly.
- `checkExternalEmitHelpers` (five boundaries, 83): `checkExternalEmitHelpers`
  was already fully ported in `emit_checks.rs`; the import/export declaration,
  object-rest binding, `yield*`, `for await`, destructuring-rest and `using`
  call sites now use it, and `checkSignatureDeclaration` requests the
  `__awaiter`/`__asyncGenerator` helpers as upstream does.
- `checkGrammarVariableDeclarationList: using` (two boundaries, 38): the three
  `using` grammar errors, `checkGrammarAwaitOrAwaitUsing` generalized over
  `await` expressions and `await using` lists, and the Disposable/AsyncDisposable
  initializer check with memoized global type lookups.

Declaration `unclassified` (58) is the driver's label for a declaration-phase
failure whose reason sits in a per-file result. Aggregated over inventory-03:
22 `getSymbolChain: external module specifier ranking`, 16
`checkExpressionWorker`, 12 CommonJS/ESM mismatch, 8 JS type-alias lookup, 6
`isInlineImportAttributes`, 3 `getContainersOfSymbol: class-expression CommonJS
assignment`, 3 singletons. The middle 36 are closed by the passes above; the
specifier ranking is node-builder work.

Focused regressions cover each (`checker_semantics.rs`; the tslib test asserts a
single TS2354 per file at the binding name, the `using` tests use a global
fixture file). **The full inventory was not re-run after this pass.**

**Inventory-04 reading.** The owner's `target/s08/p4-inventory-04` run captured
the source at bef37fe (before the third and fourth passes): acceptance variants
completing all phases 8,471 → 8,731, semantic failures 833 → 572, declaration
failures 58 → 42. Its one new reason, `getContextualType: ... import context`
(4 import-attribute variants), and the small rises in `report excess properties`
(+3), `elaborateNeverIntersection` (+1) are not regressions: every one of those
variants failed earlier in inventory-03 (`checkExpressionWorker` or the JS
type-alias lookup) and now reaches the next boundary. The import-attribute
boundary itself is closed by `getContextualImportAttributeType` and
`isInlineImportAttributes` (`expression_context.rs`), tested against the pinned
`importAttributes6(module=nodenext)` error baseline. Note for fixtures: under
`module` node16+ upstream forces every non-declaration file to be a module, so
global helper declarations for such tests must live in a `.d.ts` file.

**Fifth pass: the two panic signatures.** Inventory-05 (source at 62051a9)
reported 39 + 10 `Node.Text: *ast.PropertyAccessExpression` panics and one
`Node.Expression: KindShorthandPropertyAssignment` panic; the first bucket had
gained `declarationsForFileShadowingGlobalNoError` because the `any`-base fix
let that program reach declaration emit. Both were Rust caller bugs:

- The node builder built type references over `symbolToExpression`, so a
  qualified type name came out as a `PropertyAccessExpression` inside a
  `TypeReferenceNode`, and the length accounting then asked `Node.Text` for it.
  `createAccessFromSymbolChain` is now ported (`node_builder_names.rs`); type
  references and `typeof` queries build `QualifiedName` chains with upstream's
  per-component length charges, export-alias naming, and indexed-access members.
  The computed-entity-name member and instantiation-expression access branches
  are named boundaries.
- `checkPropertyAccessibilityAtLocation` tested "this property" through the
  generic receiver accessor, which upstream guards by node kind;
  `isThisProperty`, `isThisInitializedObjectBindingExpression` and the
  binding-pattern form are ported. Binding elements now run
  `checkPropertyAccessibility` (with `markPropertyAsReferenced`) instead of the
  `destructuring private/protected property` boundary.

Rerunning the 49 previously panicking variants with the rebuilt example binary:
43 complete every phase, 5 stop at `getSymbolChain: external module specifier
ranking`, 1 was the shorthand panic and now completes. Regressions:
`inferred_qualified_type_names_emit_as_entity_names_in_declarations` (runs the
declaration phase) and `abstract_properties_destructured_from_this_in_constructors_are_reported`
(pinned `abstractPropertyInConstructor` class `C1`, plus a private destructuring).

**Sixth pass.** The four largest remaining named buckets after the panics:

- `getSymbolChain: external module specifier ranking` (30 declaration + 5
  semantic): `sortByBestName` is ported. External containers are ranked by the
  specifier `getSpecifierForModuleSymbol` produces for them (fewer path
  components first, non-relative before relative, symbol order as the tie
  break) instead of refusing; `PathIsRelative` and `CountPathComponents` are
  local helpers in `node_builder_names.rs`.
- `getSuggestedLibForNonExistentProperty` (20 + 4): the `getFeatureMap` table is
  `lib_features.rs`; a missing property that a later `lib` provides reports
  TS2550 with the lib name, ahead of the spelling suggestion as upstream does.
- `errorOnImplicitAnyModule: package install diagnostic chain` (14):
  `CreateModuleNotFoundChain` is ported over a new host query,
  `package_bundles_types`, the per-package view of `GetPackagesMap`
  (`checker_host.rs` computes it from the program's resolutions). The
  repopulate marker has no Rust counterpart.
- `resolveAlias during mergeSymbol` (13): `mergeSymbol` resolves a non-local
  alias target through `resolveAlias` as `resolveSymbol` does, then clones the
  resolved target.

Rerunning the 93 inventory-05 variants from those buckets and the declaration
phase with the rebuilt example binary: 86 complete every phase, 7 stop at other
named boundaries (`getContainersOfSymbol: class-expression CommonJS assignment`
4, `unresolved type synthetic comment` 2, `isTypeParameterPossiblyReferenced:
type query scope` 1), none panic, no new reason. Regressions:
`missing_properties_from_later_libs_suggest_the_lib`,
`declaration_emit_names_types_from_other_modules_through_ranked_specifiers`,
`untyped_packages_report_the_types_install_chain` (a `node_modules` fixture),
`global_augmentations_merging_into_aliases_resolve_the_alias` (pinned
`checkMergedGlobalUMDSymbol`).

**Seventh pass.** The nine buckets of two to eleven variants:

- `getPropertyTypeForIndexType: unique symbol` (11): the `[Symbol]` spelling
  through `getFullyQualifiedName`.
- `resolveExternalModule: rewriteRelativeImportExtensions` (9): the three
  safety diagnostics with `ShouldRewriteModuleSpecifier`, `PathIsRelative`,
  `HasTSFileExtension`, `GetAnyExtensionFromPath`, `IsLiteralImportTypeNode` and
  `IsPartOfTypeOnlyImportOrExportDeclaration` as local helpers; the
  project-reference redirect comparison stays a boundary because loading rejects
  project references.
- `checkDeprecatedProperty` (8): `isDeprecatedSymbol` and
  `addDeprecatedSuggestion` are ported. Finding the tag needed lazy JSDoc: the
  checker now asks the host (`CheckerHost::jsdoc`, `ParserJsDocProvider` in
  `checker_host.rs`) when the eager roots are absent, as `Node.JSDoc` does.
  Follow-up: a same-line `/** @deprecated */` before a property signature yields
  no roots from the lazy provider; compare `get_jsdoc_comment_ranges` with Go.
- `removeSubtypes: nominal class derivation` (8): two classes reduce only when
  `isTypeDerivedFrom` holds, as upstream's condition reads.
- `elaborateNeverIntersection` (6): ported for both the property report and the
  base-type check, with `containerSeemsToBeEmptyDomElement` selecting the DOM
  lib message.
- `getExportsOfModule: import attributes type` (6): the boundary was redundant;
  the specifier resolver already carries the attributes type.
- `isTypeParameterPossiblyReferenced: type query scope` (7): the `typeof` scope
  walk over the first identifier's declarations. The `this.x` case exposed
  `isSelfTypeAccess` taking the receiver's resolved symbol unconditionally, which
  reported TS2304 for `this`; it now follows upstream (parent symbol first, then
  `getFirstIdentifier` over entity-name expressions).
- `unresolved type synthetic comment` (6): the `/*unresolved*/ any` keyword with
  its synthetic leading comment; the comment-free display printer shows `any`.
- `getContainersOfSymbol: class-expression CommonJS assignment` (4): reached
  only through the paths above; all four programs complete.

Rerunning the 64 inventory-05 variants of those buckets: all complete every
phase, no new reason, no panic. Regressions cover each port (`checker_semantics.rs`,
the deprecation test uses a multi-line tag, the nominal-class test a `.d.ts`
Array declaration).

**Eighth pass.** `the scope does not retain this arena` (3, class
`checker_error`) was a Rust bug in `getArgumentArityError`: the too-many-arguments
span read the extra arguments through the call expression's view, but arguments
expanded from a spread are synthetic expressions owned by the checker factory.
They now read their positions through their own views; the three programs
complete. Regression: `too_many_arguments_through_a_spread_report_the_extra_argument_span`.

**Ninth pass: the long tail.** Every remaining named boundary the inventory
reached, outside JSX:

- `isNeverReducedProperty` (private declarations) and the never-intersection
  elaboration share `isConflictingPrivateProperty`; `getTypeOfAlias` reports
  circularity through `reportCircularityError`; `mergeSymbol`-style alias
  resolution now also covers export assignments in `getDeclarationSpaces`.
- isolatedModules: an import that conflicts with a global value used in the file
  reports TS1275 unless the import is type-only.
- Unchecked JavaScript: `isUncheckedJSSuggestion` is ported and drives the
  `Could_not_find_name_0_Did_you_mean_1` / `Property_0_may_not_exist_on_type_1`
  suggestion messages through `addErrorOrSuggestion`; the deferred
  missing-property report carries the flag. `IsPlainJSFile` backs the
  "private field must be declared in an enclosing class" grammar error.
- `readonly` type operators outside arrays and tuples report TS1354;
  `maybeMappedType` produces the `K in ...` hint; the primitive alias
  suggestions (`string` for a misspelled `String`) are six transient symbols
  created on first use outside `SymbolCount`, as upstream's heap symbols are.
- `constructorVisibilitiesAreCompatible` reports the two visibilities;
  the export-assignment type-only re-export adds its related information;
  `mapToTypeNodes` regenerates colliding single-identifier references with
  `UseFullyQualifiedType` instead of refusing.
- `removeSubtypes` reports the too-complex union and `getUnionType` returns
  the error type; `reportFlowControlError` reports at the block's first token
  and flow analysis continues disabled, as upstream does.
- `getSymbolAtLocation` handles `this`, `this` types, `super`, the
  `constructor`/`default`/`function`/`class`/`=>` keywords, `export`,
  `import`/`new` meta-properties, `instanceof` and literal import types, with
  `IsInExpressionContext`/`IsExpressionNode` ported as helpers.
- Modifier keywords visited through `forEachChild` are no-ops in
  `checkSourceElementWorker`, as in upstream's switch; the node builder tolerates
  a binding-pattern parent for a function expression's variable, as upstream's
  nil symbol does.
- The inventory example now runs `GetSuggestionDiagnostics` per file instead of
  recording the suggestion phase as absent.

Rerunning the 246 inventory-05 failure variants outside P5 and JSX: 238
complete every phase. The rest: 4 reach `getContainersOfSymbol:
class-expression CommonJS assignment` in declaration emit (newly reachable
through the fully-qualified regeneration), 1 `getEffectsSignature: reentrant
effects resolution`, 1 `combineValueAndTypeSymbols: split external export`, and
2 run past 60 seconds in the debug build: `circularOptionalityRemoval` (already
the inventory's timeout) and `largeControlFlowGraph`, which previously stopped
at the flow-depth boundary and now checks the whole file (139 s debug).
Unit tests cover each port; `families_tests` guards the symbol census, which is
why the primitive alias symbols bypass `SymbolCount`.

Remaining inventory-05 acceptance buckets, largest first:

| Bucket | Variants | Needs |
| --- | ---: | --- |
| `checkSourceFile: JSX or non-script input` | 24 | out of P4 scope |
| `getContainersOfSymbol: class-expression CommonJS assignment` | 4 (declaration) | closed in the tenth pass |
| `largeControlFlowGraph` / `circularOptionalityRemoval` over 60 s | 2 | flow-analysis performance (P7) |
| `getEffectsSignature: reentrant effects resolution` | 1 | closed in the eleventh pass |
| `combineValueAndTypeSymbols: split external export` | 1 | closed in the tenth pass |
| `getDeclarationSpaces: export assignment alias` | 2 | alias declaration spaces |
| `onSuccessfullyResolvedSymbol: isolated imported-type/global-value conflict` | 2 | the isolatedModules conflict report |
| `getPrimitiveTypeAliasSuggestions` | 2 | checker-owned primitive suggestion identity |
| `getTypeOfAlias: reportCircularityError` | 2 | alias circularity |
| `isNeverReducedProperty: private declarations` | 2 | private never-reduction |
| `maybeMappedType: misspelled mapped type diagnostic` | 2 | the mapped-type spelling hint |
| singletons | 5 | one each |
| the single 60-second timeout | 1 | profiling |

Tenth pass (2026-09-13, while inventory-06 ran): the last two named
boundaries outside JSX and the slow runs are closed, plus one relater fix found
while checking their baselines.

- `combineValueAndTypeSymbols` builds the combined symbol for a split
  value/type external export (flags union, deduplicated declarations, the
  value's parent and value declaration, members from the type symbol, exports
  from the value symbol); `mergedDeclarations7` now reports Go's single TS2322.
- `getContainersOfSymbol` follows CommonJS class-expression assignments
  (`module.exports = class {}` / `exports.X = class {}` to the source-file
  symbol, `NS.K = class {}` to the resolved target of the access), which
  completes the declaration phase for the four affected JS variants.
- `reportRelationError` no longer suppresses the head message over a
  missing-property chain when the head is a conversion or interface
  implementation message (`isConversionOrInterfaceImplementationMessage`), so
  `class B3 implements A {}` reports TS2720/TS2420 with the missing property
  chained, as in `jsdocImplements_class.errors.txt`.

Rerunning the same 244 inventory-05 failure variants: 243 complete every phase;
the one left was `getEffectsSignature: reentrant effects resolution`. The two
slow cases are not in that set; timed separately with the same debug binary,
`largeControlFlowGraph` completes in 129 s and `circularOptionalityRemoval` is
killed (signal 9) after 166 s, both past the inventory's 60-second limit.

Eleventh pass (2026-09-13): `getEffectsSignature` no longer treats re-entry as
a boundary. Upstream leaves the signature link nil while resolving and simply
recomputes on re-entry; the recursion ends in `getExplicitTypeOfSymbol`'s
resolving set, which the port already had. `controlFlowFunctionLikeCircular_8`
(an assertion call whose type predicate narrows through `typeof arg`) now
reports Go's TS2448/TS2454/TS2554/TS2456, and all 244 rechecked variants
complete every phase.

Eight pre-existing tests in `checker_semantics.rs` still assert `Unsupported`
for operations that are now implemented and need re-pointing with per-case
evidence: `failed_intersection_reduction_clears_its_computed_flag_on_every_retry`,
`failed_query_caches_and_resolution_stack_cannot_convert_failure_to_success`,
`compound_constituents_are_checked_even_after_reduction_and_on_retry`,
`lazy_jsdoc_type_names_do_not_resolve_as_ordinary_wrapper_interfaces`,
`source_check_failure_after_a_diagnostic_stays_failed_across_operations`,
`unsupported_variable_widening_does_not_rebuild_a_successful_cached_initializer`,
`source_check_rejects_unported_grammar_relations_and_options`,
`union_property_failure_does_not_publish_a_partial_property_list`.
