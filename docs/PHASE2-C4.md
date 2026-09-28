# C4 implementation record

## Starting point

C4 starts from the C3 exit capture: 13,432 executed variants, no harness
error, 12,647 full-domain matches and all 9,367 S08 regression rows
matching. The native capture is reused unchanged. The C4-start capture
`target/phase2/rust-c4-start` (the crate sources of `main` at `1a455cd`) is
the same observation; `data/phase2/c4-baseline.json.gz` freezes its row
report for the `c4_regressions` comparison (`f1cf878`).

The C4-start worksheet `data/phase2/c4-claims.json` holds the 767 C4-owned
rows that were open in that capture, each `open` with its baseline outcomes
and exact reproduction:

| Bucket at the start | Rows |
| --- | --- |
| `unsupported: checkExpressionWorker` (the JSX expression kinds) | 439 |
| `unsupported: checkClassLikeDeclaration: decorators` | 284 |
| `unsupported: checkDecorators: parameter or binding` | 25 |
| `diagnostics: …` (decorator rows that passed the refusals) | 16 |
| `unsupported: getContextualType: decorator/JSX context` | 2 |
| `unsupported: checkSourceElementWorker: statement/type family` | 1 |

Those rows are the variants of five blocker-register entries: B01
(`checkExpressionWorker`, 439), B02 (class decorators, 284), B03 (decorated
parameters, 25), B06 (the decorator and JSX contextual types, 2) and B07 (the
static-block decorator, 1). The C2 and C3 handoffs were rebound to the
C4-start capture with every observation unchanged, and the blocker register
rebuilt (`130789c`). No row of an earlier checkpoint was handed to C4.

## Producer wiring

`f1cf878` wires the C4 authorities before any semantic work:

- `scripts/phase2_producers.py` registers the C4 claims, audit and baseline
  under `CHECKPOINT_AUTHORITIES["C4"]` (no measurement authority: C4 sets no
  performance condition), makes C4 the newest checkpoint, and adds the
  `c4-contracts` witness (`cargo test -p tsr_compiler --features
  recursion-probe --test c4_contracts`, debug and release, at least nine
  tests, sources including `tests/fixtures/c4`).
- `scripts/phase2_blockers.py` learns the C4 claims; `scripts/phase2_audit.py`
  binds the C4 scope: three reviewed groups with their function counts and
  digests, and `jsx.go` as the file whose inventory must be complete.
- `status/runs.toml` lists the C4 claims, audit, baseline and contract
  receipt as `checker` inputs.
- `scripts/tests/test_phase2_c4.py` covers the exit metrics (matching rows
  complete without a measurement, an open row keeps C4 incomplete whatever its
  label, a handed row needs its validated transfer, a C4-owned blocker left in
  the register keeps it open, every prerequisite and authority is required)
  and the wiring of the committed files.

## Decorators (C4.6 and C4.7)

`051e814` ports the decorator checks into `crates/tsr_checker/src/decorators.rs`:
`checkDecorators` and `checkDecorator` at every decorated position (class,
method, accessor, auto-accessor, property, parameter, and the function and
static-block positions the grammar rejects), `resolveDecorator` through the
ordinary call resolution with the synthetic arguments of
`getEffectiveDecoratorArguments`, the legacy and ES call signatures
(`getDecoratorCallSignature`), their argument counts and the TS1278/TS1279
arity messages under the resolution head message (TS1238 to TS1241), the ES
context types built from the lib's generic globals with the cached
`{ name, private, static }` override per member kind and name type, the
decorator contextual type, `checkGrammarDecorator`, and the metadata alias
marking under `emitDecoratorMetadata` with TS1272 under `isolatedModules`.
The refusals in `class_check.rs` and `binding_checks.rs` are gone. Decorated
functions check their decorators before their signatures, as the pin does;
the qualified-name climb of `isTypeReferenceIdentifier` fixed a TS2702/TS2663
choice on the way.

## JSX (C4.2 to C4.5)

`003c31a` ports `jsx.go` into `crates/tsr_checker/src/jsx.rs` with its
checker-level state (`JsxState`: element flags and attributes types, the
namespace per location with the unknown symbol for a failed lookup, the
implicit import per file, the first tag per file, the per-file pragma
entities, and the checker's namespace and factory entity). An opening-like
element or fragment resolves as a call whose single argument is its attributes
object, so the seams are the call-resolution ones: `resolveSignature`,
`getEffectiveCallArguments`, `hasCorrectArity`, `isSignatureApplicable`,
`inferTypeArguments`, `resolveUntypedCall`, `getContextualTypeForArgumentAtIndex`
and the overload-failure report, plus `checkExpressionWorker`,
`checkDeferredNode`, `getContextualType`, `getApparentTypeOfContextualType`
(the attributes discrimination), `isContextSensitive`, `elaborateError`, the
JSX attribute's symbol type, `getSymbolAtLocation` for intrinsic tags, the
deprecated-signature node, and the relater's JSX excess-property message,
hyphenated-name rule and intrinsic-attributes intersection case. Factory and
pragma entities are parsed by the parser's `parseIsolatedEntityName` and
rebuilt in the checker's factory with the synthetic range, which also required
`resolveEntityName` to treat a synthesized name as present (`NodeIsMissing`
requires a non-negative position). The checker host gains
`get_jsx_runtime_import_specifier`, the loader's synthetic runtime import, which
resolves in the mode of the `tslib` import with its errors on the file's first
JSX tag.

Four shared fixes, each matching the pin, came out of the JSX rows:

- `checkExpressionWithContextualType` pushes the contextual type and inference
  context on `getContextNode(node)`: the element, for attributes with
  children, so that children see the inference context
  (`contextuallyTypedJsxChildren2`).
- An excess-property failure reports its relation error directly, as
  `isRelatedTo` does, instead of through `reportErrorResults`; the JSX case of
  that function had otherwise dropped the "not assignable" head.
- A distributive conditional type without an alias maps the distribution
  union (`mapTypeWithAlias`), which keeps an unchanged union and its origin;
  `Pick<Props, Extract<keyof Props, …>>` now prints `keyof Props`
  (`reactDefaultPropsInferenceSuccess`, and the same without JSX).
- `checkExpressionWorker` answers `errorType` for every other kind, which the
  three decorator-in-expression rows parse to (`MissingDeclaration`).

The deferred-node dispatch also routes tagged templates and decorators to
`resolveUntypedCall`, as the pin does.

## Function audit

`data/phase2/c4-audit.json` disposes the three reviewed groups over the
pinned inventory (`6332e5f`). A function is `mapped` when a `// port:` marker
names it under `crates/`; the three `jsx.go` functions the port had inlined
(`getJSXRuntimeImportSpecifier`, `getJsxLibraryManagedAttributes`,
`getJsxElementTypeSymbol`) and the relater's `isHyphenatedJsxName` and
`isIgnoredJsxProperty` got named helpers so that each carries its marker.

| Group | Functions | Mapped | Equivalent | Later |
| --- | ---: | ---: | ---: | ---: |
| C4.2-C4.5 JSX (`jsx.go`, complete) | 59 | 59 | 0 | 0 |
| C4 JSX seams (checker, grammar, relater, utilities; C5 wrappers) | 14 | 8 | 0 | 6 |
| C4.6-C4.7 decorators and metadata | 47 | 42 | 4 | 1 |

The equivalent entries are `reportObviousDecoratorErrors` and
`findFirstIllegalDecorator`, inlined at their only caller, the port of
`checkGrammarModifiers`; and `markTypeNodeAsReferenced` and
`getEntityNameFromTypeNode`, which have no caller at the pin and whose
composed operation is the port of `markEntityNameOrEntityExpressionAsReference`.
The seven `later: C5` entries are the emit resolver's and the services'
surfaces over C4's functions (`GetJsxFactoryEntity`,
`GetJsxFragmentFactoryEntity`, `GetJsxFragmentFactory`, `GetJsxNamespace`,
`GetTypeReferenceSerializationKind`, `GetContextualTypeForJsxAttribute`,
`GetJsxIntrinsicTagNamesAt`). `phase2_audit.py check` passes with no `gap`
and no open issue. None of the sixteen `different` rows at the start needed a
separate attribution: each is a decorator row whose difference was a missing
decorator check, and each matches after C4.6.

## Direct contracts

`crates/tsr_compiler/tests/c4_contracts.rs` (feature `recursion-probe`)
holds the nine C4.8 contracts over 21 native cases recorded by
`tests/fixtures/c4/regenerate.py` with the pinned `tsgo`. The recorder runs
each case's root file with `--noEmit --target esnext --ignoreConfig --pretty
false --skipLibCheck` plus the case's flags from `fixtures.json`, binds the
digests of the root and every file the case lists (imported modules, and a
minimal `react` package under `node_modules` with a classic namespace and an
automatic runtime but no development runtime), and refuses unclean runs.
`support/c4_native.rs` checks the same files over the bundled lib and renders
the diagnostics the way the command line composes them across files.

1. The `jsx` mode matrix: `jsx_modes.tsx` and `jsx_pragmas.tsx` under each of
   `preserve`, `react-native`, `react`, `react-jsx` and `react-jsxdev`; the
   diagnostics equal the native ones (only `react-jsx` finds its runtime and
   its `JSX` namespace; `react-jsxdev` reports TS2875 and falls back to the
   classic namespace), and the factory, fragment factory, namespace and
   implicit import equal the pin's, also when a fresh checker is asked before
   anything is checked.
2. Element kinds: intrinsic, function, class, fragment, member and namespaced
   tags, `LibraryManagedAttributes` defaulting a class prop, children typing
   and the JSX excess-property message.
3. Generic components: inferred and explicit type arguments, an overloaded
   component and contextually typed attribute and child functions, over the
   bundled lib only.
4. JSX in JavaScript: a `.jsx` file under `checkJs` with a JSDoc-typed
   component.
5. Decorator positions: class, method, getter, setter, auto-accessor, field
   and parameter decorators under ES and legacy decorators, and the
   static-block decorator under both.
6. Context types: each ES context type and its member override, shown by the
   argument each decorator receives, and `Symbol.metadata` resolving through
   the lib.
7. Metadata marking: under `emitDecoratorMetadata` the imported class a
   decorated constructor parameter names is marked referenced and the
   interface is not (observed through `alias_link_state`); without the option
   nothing is marked; under `isolatedModules` the interface imported as a
   value reports TS1272.
8. Lifecycle: after a JSX error and a decorator error a second query repeats
   the diagnostics, a second checker resolves the runtime import on its own,
   and retiring the first generation leaves the second answering.
9. No transform: checking leaves the source tree unchanged, and the checker's
   own factory holds no JSX, decorator or factory-call syntax; the recorded
   factory name is the one JSX entity it builds.

The JSX entities are observed through a `jsx_link_state` probe and the
factory's contents through `synthetic_syntax_kinds` (both behind
`relation-probe`) until C5.6 gives the resolver its entry points, the plan's
decision 3. Eight source mutations (no implicit runtime import, no JSX
excess message, no children property, attributes context on the attributes,
an inverted `static` override, no metadata marking, a wrong legacy class
argument count, no fragment-factory pragma) are each caught by at least one
contract; the last one survived the first version of contract 1, which then
gained its unchecked-checker query (`6709e21`).

## The earlier audits

C4's port markers name twelve functions that the C1 to C3 audits had
disposed otherwise, and the audit rule requires a marked function to read
`mapped`. Ten were `later: C4` entries that C4 fulfils (`isArrayOrTupleLikeType`,
`isHyphenatedJsxName` and `isIgnoredJsxProperty` in C1;
`getContextualTypeForDecorator` in C2; `getClassElementPropertyKeyType`,
`getParentTypeOfClassElement`, `markEntityNameOrEntityExpressionAsReference`,
`newParameter`, `newProperty` and `isJsxIntrinsicTagName` in C3). Two were C3
`equivalent` entries whose equivalence the JSX rows disproved:
`getContextNode` (the Rust site pushed the argument context on the
attributes, not the element) and `mapTypeWithAlias` (the conditional
instantiation rebuilt every distribution union). All twelve now read `mapped`
with the reason. The same edits shifted 69 `equivalent` anchors of those
audits in files C4 changed; each was re-mapped from its line at the branch
point (`1a455cd`) to the same line now. All four audits check.

## S07 operation anchors

The port markers and helpers moved the Rust line anchors of seven functions
in `data/s07/operations.json` (all in `rust_mappings`; everything else is
identical after stripping the mappings). The inventory was regenerated and
the subset review re-frozen under the owner's standing mapping-only approval,
finding `PHASE2-C4-2026-09-27-mapping-refresh` (`ca2178b`): `subset.json` and
`checker-obligations.json` keep their reviewed digests and only the rule's
operation matrix digest changes.

## Intermediate samples

The recorded 300-variant sample had 237 full-domain matches at the C4 start,
259 after the decorator port (`target/phase2/c4-s1`) and 295 after the JSX
port (`target/phase2/c4-s2`), with no regression and no changed observation
against the C4-start baseline at either point; the five open rows are the
content-mapper rows. During the implementation the claims were run by bucket
against the native capture: the decorator buckets matched after C4.6 and
C4.7, the 441 JSX claims after C4.2 to C4.5, and all 767 claims together
before the exit run (`target/phase2/c4-all-1`). No full run was
made during the implementation; the exit run below is the first.

## Exit run

The fresh `target/phase2/rust-c4` capture (sources at `d78acbe`; the later
commits change data and documents only) completed all 13,432 variants with
no timeout, execution failure or harness error, and replays source-stable.
13,414 match every enabled domain, 767 more than at the C4 start; all 9,367
S08 regression variants match; no domain of any row regressed and no
observation changed against the C4-start report, and the 154 traced variants
keep trace parity at 1. By checkpoint: all 926 C4-owned rows match; C3 keeps
170 of 185 with the other 15 blocked on content-mapper execution (Phase 5);
C2 keeps 2,951 of 2,954 with its three emit-order rows handed to C5. The
comparison is recorded in `data/phase2/first-comparison.json`.

The 15 C3 claims and the three C2 handoffs were rebound to this capture, none
stale and none changed. The rebuilt blocker register has two entries, the
content-mapper refusal (Phase 5, 15 variants) and the post-emit diagnostic
order (C5 with Phase 3, 3 variants); B01, B02, B03, B06 and B07 of the start
are closed by the implementation. The register's declaration audit has 1,746
working declaration requests (1,740 at the start: six had stopped at the JSX
refusal), five `different` and three not loaded, unchanged otherwise.

All nine C4 contracts pass in debug and release; the receipt is
`data/phase2/receipts/c4-contracts.json`. The C1, C2 and C3 receipts were
refreshed on the same sources. Relater parity is 105 of 105 groups for both
implementations with `all_cases_match` true (`target/s08/relater-c4`).
Workspace formatting and clippy with warnings denied pass, `cargo xtask
validate` passes, and the Python suite (1,393 tests, 2,094 subtests) passes
except the same seven subtests of the two Phase 1 ledger-closure tests that
the C3 record describes: the fixture, oracle and observation files under the
crates' `tests/fixtures` directories are inside the Phase 1 source closure but
outside the ledger's source globs (311 files at this head; the C4 fixtures
join that set). The fix remains a Phase 1 ledger amendment for the
end-of-phase green-up.

The checker producer reports `c4_open = 0`, `c4_regressions = 0`,
`c4_failures = 0`, `c4_blockers_open = 0`, `c4_handoffs = 0`,
`c4_audit_complete = true`, `c4_contracts = true` and `c4_complete = true`,
with `regression_parity = 1` and `trace_parity = 1`, and every prerequisite
true. C1 stays complete. For C2 and C3 it reports the accounting (both open 0,
regressions 0, failures 0, open blockers 0, audits complete, contracts true;
`c2_handoffs = 3`) and no completion metric, since only the newest checkpoint
computes one. Until C7.7 lets a tracker item name a recorded run, recording
this checker run makes `P2B-C2` and `P2B-C3` read as pending in
`sprints/P2B.toml` while `P2B-C1` and `P2B-C4` read as done; that is the
plan's known tracker gap, not a regression. The recording, `cargo xtask run
checker`, is the owner's.

## Review follow-up

The two reviews of #68 were addressed on `phase2-c5`, after C5 had been built
on the C4 branch.

- **Deep factory pragma.** `parseIsolatedEntityName` flattened the parsed
  qualified name recursively, so a `@jsx` pragma of 30,000 components
  overflowed the stack where the pin completes. The left spine is now walked
  in a loop. The regression is the native case `jsx_deep_pragma` (30,000
  components under `react`, TS7026 only), checked on a 512 KiB thread; with
  the recursive code it aborts.
- **Census.** The JSX links (seven maps and the retained namespace names), the
  decorator signatures and the decorator context overrides were outside the
  census, and so were three caches C5 added (the candidate list, the
  skip-direct-inference set and the JSX import references). Each is charged
  to its family, and the types and signatures they hold are reachability
  roots. A unit test populates the caches and fails when either the charge or
  the roots are removed.
- **External consumer lockfile.** `tools/s10/rust-consumer/Cargo.lock` lists
  `tsr_parser` (C4) and `tsr_astnav` (C5) under `tsr_checker`. The consumer's
  `lifetime` test passes with `--locked`.
- **`--noEmit`.** The C3 and C4 native loaders set `no_emit` from the recorded
  command instead of dropping it.
- **Checker state from the pin.** `fixtures/c4/state/regenerate.py` records the
  pinned checker's state through a Go overlay: two test files added to the
  pinned checker package, with `upstream/` untouched. It records the four JSX
  entities at the root's first JSX tag (`getJsxFactoryEntity`,
  `getJsxFragmentFactoryEntity`, `getJsxNamespaceAt`,
  `getJsxNamespaceContainerForImplicitImport`) after the loader's checks and
  on a fresh checker, plus `aliasSymbolLinks.referenced` of the root's import
  specifiers. It covers all 40 JSX and metadata cases, and each row binds its
  case's flags and source digests. Contracts 1, 2 and 7 to 9 compare with
  these records, so none of them states checker state by hand. The
  metadata-disabled command is its own native case, `metadata_plain`. Making
  the probe report no fragment factory fails three contracts.
- **Direct cases the C4.2 and C4.5 exits name.** For C4.2 there are ten
  cases:
  - the `jsxFactory` option alone (TS17016 at a fragment) and with
    `jsxFragmentFactory`;
  - `@jsx`/`@jsxFrag` pragmas over both options;
  - `@jsx` without `@jsxFrag` (TS17017);
  - no `jsx` option (TS17004);
  - `react` without `React` in scope (TS2874);
  - `@jsxRuntime classic` under `react-jsx` and `react-jsxdev`;
  - `@jsxRuntime automatic` under `react` and `preserve`.

  For C4.5, each of the five modes has an `@jsxImportSource` package with
  both runtime modules and one with neither (TS2875 where the mode needs
  one). The `jsxImportSource` option is covered under both automatic modes.
  All 22 cases match natively in Rust, and `every_recorded_case_matches_native`
  loads every case of the manifest.
- **The shared corrections outside JSX.** `conditional_distribution` witnesses
  `mapTypeWithAlias` without JSX. `Pick<P, Extract<keyof P, keyof D>>`
  instantiated without an alias keeps the origin of an unchanged `keyof Props`
  (`Pick<Props, keyof Props>`); rebuilding every distribution union, the
  pre-C4 code, fails it. `relation_excess` guards the excess-property path in
  declarations, nested literals, union, intersection and array targets, and
  an argument. The pre-C4 path (through `reportErrorResults`) passes it too,
  and it cannot fail: outside JSX `reportErrorResults` ends in the same
  relation error for a fresh object literal. Only its JSX-attributes branch
  differs, so the correction does not change behaviour outside JSX.
- **The C3 audit's inlined equivalents.** A sample of 14 of the 157 "inlined at
  the port of its pinned caller" entries all had their logic at a Rust site
  that matches the pin:
  - five anchors name the exact line;
  - six name the enclosing function or a call to the helper that implements
    the function;
  - three named unrelated code and now name the inline site
    (`isPropertyAbstractOrInterface`, `getVerbatimModuleSyntaxErrorMessage`
    and `getSuggestedSymbolForNonexistentProperty`).

  The rest of the 157 have not been re-anchored.

Not changed here:

- The claims file still reads `open` for every row, pending the owner.
- The tracker gap (review 1, finding 1) stays the plans' C7.7 item.
- These fixes change production sources, so the recorded `rust-c5` capture and
  the C1 to C5 contract receipts need their end-of-phase refresh.
