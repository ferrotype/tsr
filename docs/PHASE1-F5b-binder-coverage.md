# F5b binder witness audit

This pass adds 30 exact operation IDs in four witness records. It reuses
existing resolver observations, three named supplemental binding requests and
four source-derived classification tests. It adds no production implementation,
new native answer or corpus-wide internal-operation credit. The pin remains
`1f70213d4922b434345f639b441681e470c7cfc1`.

The records are in `data/phase1/cases.json`. Their gates remain the existing
binder producer and native workspace tests. The production annotations for
all 30 IDs were checked. The audit does not classify an unmatched Go method as
unused or equate an unverified implementation by name.

## Resolver helper paths: 15 operations

The exact native results are the 84 rows of
`data/s07/resolver-observations.tsv`; its manifest authenticates the pinned
sources, adapter and observation bytes. The native adapter is
`tools/s07/resolver/export_test.go`. The matching Rust tests are in
`crates/tsr_binder/src/resolver_tests.rs` and compare each requested label with
the complete frozen row. They are enumerated in `data/s07/helper-tests.json`
for the binder producer. The source audit below supplements the preexisting
entry-point witness; it does not invent a new resolver probe.

All IDs in these tables have prefix `tsc/internal/binder/`.

| Exact TSV rows | Newly linked operations | Discriminator and necessary path |
| --- | --- | --- |
| `scope/0/3` through `scope/8/9`, with targets 3, 4, 7 and 9 | `nameresolver.go:NameResolver.requiresScopeChangeWorker` | `requiresScopeChange` necessarily invokes the worker on the name/initializer. Optional/nullish expressions switch true to false at ES2020, object-rest at ES2017, static class fields at ES2022; nested function and type-node cases remain false. |
| `global/0` through `global/3` | `nameresolver.go:NameResolver.lookup` | The request starts with meaning zero and fails without a hook. Hooks record the exact requested meaning and whether the global table is nil; hook-returned nil fails, and hook-returned symbols succeed, including a nil table. The callback traces distinguish bypassing the helper from executing it. |
| `arguments` | `nameresolver.go:NameResolver.argumentsSymbol` | Two resolutions return the same symbol, with name `arguments` and flags 33554436. Rust additionally requires exactly one transient symbol allocation. |
| `static/type` | `nameresolver.go:NameResolver.getSymbolOfDeclaration`, `NameResolver.error` | The class's attached symbol supplies the `T` member; resolving its static reference returns nil and invokes the error hook with TS2302. These are the default declaration-symbol and present-error-hook branches. |
| `default` | `nameresolver.go:isExportDefaultSymbol` | The exported default function resolves to its distinct local symbol; the nil argument returns nil. `GetLocalSymbolForExportDefault` checks this helper before inspecting declarations. |
| `lexical/0`, `lexical/1` | `nameresolver.go:isSelfReferenceLocation` | The recursive function reference yields only `success:false:true`; a captured `x` yields `referenced,success:false:true`. The reference callback is suppressed only for the self-reference. |
| `reference/0` through `reference/3`, `reference/fallback` | `referenceresolver.go:referenceResolver.getResolvedSymbol`, `getReferencedValueSymbol`, `getMergedSymbol`, `getExportSymbolOfValueSymbolIfExported` | Existing resolution gives `resolved,merged`; the fallback gives `resolved,resolve,merged`; a nil fallback gives `resolved,resolve` and a nil declaration list. Non-nil results contain two value declarations and omit the type alias. With all hooks absent, the lazy name-resolver path still reaches the local variable. |
| `import/0` through `import/4`, `import/hook-nil` | `referenceresolver.go:referenceResolver.isTypeOnlyAliasDeclaration`, `getDeclarationOfAliasSymbol` | Ordinary imports and import-equals yield their original declarations; specifier-, clause- and import-equals-level type-only imports do not. A present type-only hook returning nil permits the declaration: it does not trigger the structural fallback. |
| `export/container/0` through `export/container/4`, `member/value/0`, `member/value/1` | `referenceresolver.go:referenceResolver.getParentOfSymbol`, `getSymbolOfDeclaration`, `getMergedSymbol`, `getExportSymbolOfValueSymbolIfExported` | Prefixing the local export returns the original namespace. Present hooks returning nil for parent, declaration symbol or merged symbol suppress that result. The member-value request follows the local export symbol to the function, both with and without the resolved-symbol hook. |

The Rust homes are `name_resolver.rs` and `reference_resolver.rs`. The
`export/container/5` and `member/value/2` panic rows are not used to justify
these new links: the nonpanic rows already discriminate the relevant paths.
The seven existing resolver tests do not have an all-row consumption assertion;
this audit only names rows actually looked up by those tests.

## Supplemental diagnostics: 12 operations

Frozen request `s07/smoke/diagnostics.ts` is stored in
`data/s07/binder-supplemental.json`, derived from
`tools/s07/binder/cases/diagnostics.ts`. The producer compares complete parsed,
bound and repeated graphs. The inspected
archived `target/s07-binder-reports-before-f4b-1790070946278610000/requests.ndjson`
retains the complete
raw diagnostic records on both sides; all six bound diagnostics match:

| Order | Code | Range | Arguments |
| --- | --- | --- | --- |
| 0 | TS1215 | 60..64 | `eval` |
| 1 | TS1215 | 66..75 | `arguments` |
| 2 | TS1214 | 83..88 | `yield` |
| 3 | TS1102 | 104..108 | none |
| 4 | TS2451 | 18..27 | `duplicate` |
| 5 | TS2451 | 33..42 | `duplicate` |

The parsed graph has no binder diagnostics, and repeated binding retains the
same six ordered records. The file identity, source/key bytes, arguments,
empty related-info/message-chain arrays and flags are also compared.

| Operations in `binder.go` | Why this observation witnesses them |
| --- | --- |
| `Binder.checkStrictModeEvalOrArguments`, `isEvalOrArgumentsIdentifier`, `Binder.getStrictModeEvalOrArgumentsMessage` | The two forbidden formal-parameter names produce the module-specific TS1215, with their distinct text arguments and identifier spans. Other parameter/function names in the fixture produce no such diagnostics. |
| `Binder.checkContextualIdentifier`, `Binder.getStrictModeIdentifierMessage` | The `yield` identifier produces module-specific TS1214 with `yield`, rather than the plain strict-mode or class-specific message. |
| `Binder.checkStrictModeDeleteExpression` | `delete eval` produces TS1102 over the identifier operand, not the whole delete expression. |
| `Binder.errorOnNode`, `Binder.createDiagnosticForNode`, `Binder.addDiagnostic` | The native strict checks use this chain to attach file identity, compute exact identifier ranges and append diagnostics in the observed order. Rust has the corresponding production chain in `diagnostics.rs`. |
| `Binder.declareSymbol`, `Binder.declareSymbolEx`, `Binder.getDisplayName` | The second `let duplicate` reaches the existing symbol through the module's local table. The conflict reports both declarations, previous first, using the displayed name rather than an internal/missing-name spelling. `declarations.rs` ports the same path. |

The fixture starts with `"use strict"`, but also contains exports. A module is
already strict without that directive. Therefore it does **not** prove
`FindUseStrictPrologue` or `isUseStrictPrologueDirective`; neither is credited.
It also does not justify unrelated strict-mode catch, assignment, unary-update,
label, with-statement or invalid function-name branches.

## Symbol names: two operations

Two exact supplemental requests exercise generated names:

- `s07/smoke/private_classes.ts` observes private symbols and table keys with
  prefix bytes `fe23`, owning class-symbol graph reference 2, 4 or 7, and suffix
  `@#x`, `@#y` or `@#value`. The same `#x` in separate classes has distinct owner
  identity. These names necessarily pass through
  `binder.go:Binder.getDeclarationName` and
  `binder.go:GetSymbolNameForPrivateIdentifier`, ported in `declarations.rs`.
- `s07/smoke/pattern_modules.ts` observes two attributed `*.style` ambient
  module symbols with prefix bytes `fe222a2e7374796c65227061747465726e40` and
  distinct attribute-node references 4 and 5. This is the attributed-pattern
  branch of `Binder.getDeclarationName`.

The graph comparison first validates each runtime's literal numeric component
against its own actual node/symbol ID, then compares graph identity, prefix and
suffix. It does not pretend the process-wide numeric bytes are equal. Both
symbol names and table keys are observed, as is stability after repeat binding.
The retained `qualified_name_observations` in the same report provide the exact
field, sequence and stage for these values. No other symbol-creation or graph
helper receives credit merely because the containing graph matched.

## Container classification: one source-derived contract

Four existing tests in `container_classification_tests.rs` use literal flag
words checked against `binder.go:GetContainerFlags` and its flag constants:

- `fixed_container_rules_do_not_inspect_payload_or_parent`: fixed kinds,
  token-shaped incompatible payloads and out-of-domain raw kinds;
- `method_rules_read_only_the_selected_parent_kind`: get/set/method kinds
  across object literal, class expression, class declaration and other parents;
- `block_rules_include_signature_and_static_block_parents`: function/signature/
  static-block parents return zero; ordinary block containers return 34;
- `property_rules_inspect_initializer_without_requiring_a_parent`: a present
  initializer returns 260, an absent initializer zero.

Both the public checked adapter and the production local binder reader are
asserted. The expected values do not call either implementation. This is
explicitly a source-derived Rust contract witness, **not** a captured native
execution or a claim that invalid Go memory accesses equal Rust errors.

## What remains pending

The full graph has flow edges and counters, but the retained successful report
mostly records equality/counts, not a per-helper observable trace. There is no
blanket extension from `BindSourceFile` to every flow constructor, branch label,
loop, assignment, CommonJS or expando helper here. Rust-vs-Rust exclusive/local
backend comparisons alone would not provide independent native truth.

Native pool management (`getBinder`/`putBinder`), interface-shaped accessors and
other name-inferred implementation questions are left unchanged. No new unused
or later-phase decision was made. The next useful tranche would retain and
index exact small supplemental flow/symbol graphs, then audit branch-specific
edges and counters; it does not require another full corpus run.

## Validation

All exact TSV row labels cited by the new resolver witness exist in the frozen
84-row observation and are reached by the named Rust test loops. The native
adapter invokes the original resolver on deliberately constructed symbol
graphs. The existing binder report records all seven resolver tests passing and
all 18 supplemental requests matching. Its six diagnostic records and generated
name observations were inspected directly as described above.

The new witness records pass the repository's child-free witness validation,
and all 30 operation IDs have explicit production port annotations. This audit
ran no new compiler build, full producer, broad test suite or benchmark. Existing
report success is historical evidence; currentness remains the normal producer
fingerprint decision and is not granted by this document.
