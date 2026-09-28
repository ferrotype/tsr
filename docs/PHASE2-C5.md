# C5 implementation record

## Starting point

C5 starts from the recorded C4 exit (`target/phase2/rust-c4`: 13,414 of 13,432
variants match every enabled domain, all 9,367 S08 regression variants
match). The C5-start capture `target/phase2/rust-c5-start` is the same
observation on every row, and `data/phase2/c5-baseline.json.gz` freezes its
row report for the `c5_regressions` comparison (`d8127ab`). C5 owns no rows:
`data/phase2/c5-claims.json` holds the three emit-order rows C2 handed to C5
as incoming entries, each naming the resolver function its first transform
reaches (`GetConstantValue` for `incorrectRecursiveMappedTypeConstraint`,
`MarkLinkedReferencesRecursively` for `typeParameterWithInvalidConstraintType`
and `recursiveMappedTypes`).

## Producer wiring (C5.0, C5.1, C5.9)

`d8127ab` binds the C5 audit scope (thirteen groups over the twelve complete
C5 files and the unmarked `checker.go` API entries, 377 open gaps at the
start), registers the C5 claims, audit, baseline and the services reference
(`data/phase2/services-replay.json`) as C5 authorities, adds the
`c5-contracts` witness and the `c5_services` completion input, and lists the
C5 inputs of the checker run in `status/runs.toml`, including the record
`data/phase2/services-replay.json.xz`, the owner's approvals
`data/phase2/services-approvals.json` and the replay receipt
`data/phase2/receipts/c5-services.json`. `scripts/tests/test_phase2_c5.py`
covers the exit metrics and the wiring; `scripts/tests/test_phase2_services.py`
covers the services record and replay classification.

## The emit resolver and the post-emit schedule (C5.6, C5.7)

`5526221` gives the resolver the JavaScript-emit half of `emitresolver.go`
(the alias elision queries, `MarkLinkedReferencesRecursively` over the pin's
`markLinkedReferences` dispatcher, `GetConstantValue`, the reference queries,
the JSX factory entities and `GetTypeReferenceSerializationKind`) as public
operation methods, and ports `Program.GetDeclarationDiagnostics` (sorted and
deduplicated), which closes the five declaration-audit differences.
`8f825b5` implements owner decision 3: the corpus driver runs the harness's
second program with the resolver calls of the script transforms that reach
the checker first (import elision, the enum member values, constant-enum
inlining in pre-order) before collecting its diagnostics, and
`compare_errors` compares post-emit sets once Rust's emit is recorded as
executed. The three emit-order rows match; the mutation check reopened them
as the commit describes. The exit run found that the schedule asked
`GetConstantValue` about private-name accesses that the pin's class-fields
downleveler, which runs between the TypeScript transforms and constant-enum
inlining, has already replaced with helper calls below ES2022. The check that
query forces reported the same error a second time, and the program's
`compactAndMergeRelatedInfos` then re-sorted its related information (six
nested-class private-name rows), or reported the missing-helper error at the
private access before the decorator (`importHelpersES6`). The schedule now
models that replacement and asks only about the receiver.

## The public query surface, accessibility and the node builder (C5.2 to C5.4)

`9701529` makes every `exports.go` function a public method of the checker
operation over owner-bound handles (with `IndexInfoRef` and
`TypePredicateRef`), and ports the `checker.go` API entries in scope.
`2b50c1c` adds the accessibility entry points and the exact chain search.
`708061f`, `604a2f1` and `4daedab` complete the builder: recovery scopes that
keep deferred symbols, the hover expansion control (`VerbosityContext`,
expansion depth, the type stack and the two output signals), the
`nodebuilder.go` entry points with the pin's `exitContext`, the printer's Ex
forms, the three property-name functions and comment ranges.

## Services and hover (C5.5)

### The ports

`ce1d65e` ports all 66 `services.go` functions and `nodebuilder_hover.go`
with the `ExpandSymbolForHover` entry points (`services.rs`,
`node_builder_hover.rs`), including the language service's inference
blocking (`skipDirectInferenceNodes`) and the resolution without the
resolved-signature cache that signature help and string-literal completions
use.

### The recorded reference

`scripts/phase2_services.py record` builds a diagnostic overlay of the pin
(never an edit of `upstream/`): `tools/phase2/services/instrument` rewrites
copies of the checker package so that each of the 226 exported entry points
of `Checker`, `NodeBuilder` and `EmitResolver` (and the four package
functions that take a checker) reports its arguments and results to the
recorder in `tools/phase2/services/overlay`, and a fourslash hook names the
running test and its virtual file system. Only a call that enters a checker
from outside (depth zero) is recorded. The recorder reads fields only: a
type, symbol or signature is a recorder-local token with the fields that
build it at first sight (flags, the Go data kind, symbol, alias,
constituents, arguments, declarations by file and position), nodes are
`{file, pos, end, kind}` or the structure of synthesized syntax, and
diagnostics carry their code, key, arguments, range, chain and related
information. Each program is recorded as a loading request with the snapshot
text of every source file it checks, its roots, options, configuration file
and project references.

Three facts of the pin make a naive record irreproducible, and the recorder
handles each: map-ordered results the language service walks
(`GetSymbolsInScope`, `GetExportsOfModule`, `GetExportsAndPropertiesOfModule`,
`GetAllPossiblePropertiesOfTypes`, `GetAmbientModules`) are returned in one
fixed order the pin can produce, in the recording build only; process-wide
symbol ids inside internal names (private names, unique symbols, pattern
ambient modules) are written without the id; and the remaining timing of the
language server (diagnostic file order, canceled requests, edits landing
between requests, map iteration inside the language service) is measured,
not assumed away: the suite runs once plain and three times recorded, the
fourslash outcome of every test must agree across all four runs (neutrality),
and a test whose recorded runs differ is excluded and named in the manifest.

The committed record (`data/phase2/services-replay.json` with the calls in
`services-replay.json.xz`, 12.6 MB) comes from the pin's fourslash suite of
4,590 tests (4,173 pass, 417 skipped by the pin itself, none failing), neutral
across the plain run and every recorded run. 3,589 tests enter a checker from
the language service. The record keeps the 3,496 that five recorded runs
record identically, 8,237,001 calls over 147 of the 226 entry points, and
names the other 93 as timing-dependent exclusions. Three of the runs are the
record's own; the other two are the runs of an earlier `verify` of the same
overlay, pooled with `record --reuse --pool`, because the language server's
timing can settle on one result for a whole session: the first exit
verification found `TestCompletionBeforeSemanticDiagnosticsInArrowFunction1`
recording, in both of its runs and in five retries, the other of the two
results the pin produces (a completion request that runs before, or is
overtaken by, the semantic diagnostics), and pooling excluded it with 16
tests that one of the five runs recorded differently. A test that starts
several servers records one segment per server, and its digest covers every
segment in order.

`verify` re-records twice and requires every committed test to reproduce its
digest in one of the runs, or to record differently in the two, or to
reproduce it when recorded alone within five attempts (timing, as the record's
own exclusions show); a test both runs and every retry record identically but
differently from the record has changed and fails verification, as do a
changed pin, overlay fingerprint, outcome or skipped set. The exit
verification of the committed record reproduces 3,495 of its 3,496
tests; `TestCompletionListInImportClause04` recorded differently in the two
runs, and nothing changed.

### The replay

`scripts/phase2_services.py replay` builds
`crates/tsr_compiler/examples/phase2_services.rs`, which runs the replay in
`tools/phase2/services/replay` over the record in parallel shards. For each
recorded program it builds the Rust program from the test's files and the
snapshot texts (through the configuration file when the program has project
references) and checks that it has the recorded files in the recorded order;
each recorded checker gets its own Rust checker, whose calls replay in the
recorded order through the public operation methods, the emit resolver
trait and fresh type node builders. Arguments resolve through the tokens
earlier results bound, or, for a value the language service reached without
a call (a declaration's symbol, a member of a table, a declared type
parameter), through its recorded declaration; nodes resolve by file, range
and kind, including JSDoc and the tokens the language service creates on
demand. Every result is compared by the fields that build it the first time
its token appears and by identity afterwards, so the comparison checks the
pin's object identity as well as its shape. The read-only type and signature
views behind the new `services-replay` feature read stored fields only, as
the recorder does.

The replay found these differences, and C5 fixes them (`fedcec0`):

- `getSymbolAtLocation` lacked the pin's JSDoc `@param` name branch (3,009
  calls), with a finished-tree form of the parser's `GetNodeAtPosition` in
  `tsr_ast`;
- `signatureToSignatureDeclarationHelper` covered 7 of the pin's 13 kinds and
  not the type arguments of an instantiated signature (155 builder calls and
  41 signature strings), and `symbolToParameterDeclaration` used the value
  declaration instead of the effective parameter declaration and dropped
  constructor parameter modifiers;
- modifier lists were built without their cached flags, so the hover
  namespace expansion kept `export` on every member; the hover module header
  used the entity-name form instead of `symbolToNode`, and its printer could
  not read the declaration's file;
- the deprecation suggestions of identifiers, properties, element accesses
  and import specifiers (`resolveAliasWithDeprecationCheck`,
  `isUncalledFunctionReference`) were missing: the C1 audit had recorded them
  as unobservable because the corpus never captures suggestions;
- `reportImplicitAny` did not skip JavaScript files without checkJs and
  lacked its function and mapped-type kinds; four `addErrorOrSuggestion`
  sites kept the error category on suggestions;
- `checkMetaPropertyKeyword` returned the meta property's type instead of the
  pin's error type; the `const` of `x as const` resolved to `any`;
- the string the language service is editing was not the blocked string type
  and its properties still inferred (`isSkipDirectInferenceNode`), the
  obligation C2's audit had deferred to the services;
- `resolveUntypedCall` checked a decorator's synthetic arguments (a missing
  link for an invalid parameter decorator); anonymous class names did not set
  the builder's error; typeof instantiation expressions were refused; a
  function declaration's JSDoc `@type` was used as its contextual type, which
  the pin never does.

The last two differences were closed at the exit:

- `GetBaseTypes` in `TestHoverCircularInheritedDocumentation`: an interface
  that a module augmentation merged through `export *` is reached through an
  import of its unmerged declaration, and `getTypeReferenceType` took that
  declaration's own type where the pin's `getTypeFromClassOrInterfaceReference`
  takes the merged symbol's;
- `GetSymbolAtLocation` in `TestFindReferencesBindingPatternInJsdocNoCrash1`
  and `2`: a binding pattern in the parameter of a JSDoc function type in a
  declaration file has no symbol, and `getTypeForVariableLikeDeclaration`
  lacked the pin's early return for such a parameter, so Rust found a
  property the pin does not.

At the exit every replayed call matches. Of the 8,237,001 recorded calls,
7,951,761 match; 285,127 belong to the 92 programs the replay excludes (below),
and 113 are calls the replay cannot make, for the two reasons below. 3,966
programs rebuild with the recorded files in the recorded order. 143 of the 147
recorded operations replay completely; the other four have no mismatch and are
open only for their unsupported calls: `GetSymbolAtLocation` (8 of 61,098),
`IsSymbolAccessible` (12 of 27), `EmitResolver.CreateTypeOfDeclaration` (9 of
72) and `GetMemberOverrideModifierStatus`, whose 84 recorded calls all pass a
member a code fix synthesized. The replay takes about ten seconds; its report
is `target/phase2/services/replay/report.json` and its receipt
`data/phase2/receipts/c5-services.json`.

### Left to Phase 5, approved by the owner

`data/phase2/services-approvals.json` names what the replay leaves to Phase 5.
The owner approved every entry on 2026-09-28 (`e2f0876`), and the replay run
again over the approvals classifies complete with no open operation: 143
operations replayed and 4 approved.

- three program kinds the replay excludes: the auto-import registry's
  alias-resolver programs (the registry is Phase 5's), content-mapped sources
  (withheld under B06), and the editor's project-reference source
  redirection, which `Program::load_with_source_of_project_reference` refuses
  when the referenced outputs exist;
- two reasons the replay cannot make a call: syntax a code fix synthesized
  passed as an argument (`GetMemberOverrideModifierStatus`,
  `IsSymbolAccessible`, `EmitResolver.CreateTypeOfDeclaration`), since Rust
  checker queries take program nodes, and the program's synthesized tslib and
  JSX runtime import specifiers (`GetSymbolAtLocation`), which the Rust
  program exposes as text.

## Function audit

The C5 audit has no gap: every function of the twelve complete files and the
`checker.go` API entries is mapped or recorded as equivalent at its inline
site with the reason. The markers C5 added name 34 functions that the C1 to
C4 audits had disposed otherwise (7 in C1, 8 in C2, 12 in C3, 7 in C4; 20
recorded as equivalent, 14 left to a later checkpoint); each now reads
`mapped` with the reason. Two of them corrected a reason rather than a
schedule: `resolveAliasWithDeprecationCheck` in C1 and
`isUncalledFunctionReference` in C3 were `equivalent` on the ground that the
corpus never captures suggestions, which the services replay disproved. The
edits also moved the line anchors of the equivalent entries, which the
audits now name again. All five audits check.

## Direct contracts (C5.8)

`crates/tsr_compiler/tests/c5_contracts.rs` holds the nine contracts, each
naming its pinned counterpart:

1. owner retention in three lifetimes: builder output released with the
   builder, the checker's diagnostic builder and resolver marks reused by the
   next operation, and a retained result keeping a retired checker's storage
   until the last root drops, with the retired owner refused at once;
2. source context: `globalThisDeclarationEmit`'s declaration diagnostics come
   from each file's view as the native capture reports them, and the
   late-declaration case of `declarationEmitAugmentationUsesCorrectSourceFile`
   completes with the pin's (empty) set;
3. accessibility chains, 4. declaration serialization, reuse and truncation,
   and 5. hover expansion of a class, an interface, an enum, a namespace and
   the type aliases it holds: each replays the pinned fourslash calls of its
   tests with every call matching (4 also checks that `noErrorTruncation`
   removes the truncation the same display otherwise applies);
6. the services replay over its contract subset (20 tests, written from the
   record by `phase2_services.py fixture`) with every call matching, and a
   changed recorded result failing it;
7. the public query surface answering over a checked program, and answering
   the same after an unrelated query;
8. the resolver's alias, JSX and metadata queries over the marking state, and
   the recorded resolver visibility queries;
9. two checkers over one program with independent state, and a panic while one
   checker's builder is live retiring only that checker.

The witness builds with `recursion-probe,services-replay`.

## Exit run

The exit checks found four things that the sample runs had not reached, and
C5 fixed them before the recorded capture:

- `constAssertionInLoop` hit the deadline: the `const` of an `as const`
  assertion now resolves, as the pin's `getTypeFromTypeReference` does, to the
  checked expression, and Rust's `checkTypeReferenceNode` also resolved that
  `const` while checking the assertion, which the pin skips. Inside a loop the
  expression's flow analysis re-entered the assertion without end.
- Seven post-emit rows (`importHelpersES6` and six nested-class private-name
  rows) regressed through the emit schedule, as C5.7 above describes.
- The blocker register kept the post-emit order entry for the three rows C5
  closed, because it listed every row whose native pre- and post-emit sets
  differ. Only an observed missing emit is a blocker: the entry now lists a
  row only while the comparison still reports the `native_pre_post` refusal,
  that is while Rust has not executed the post-emit schedule.
- The finished-tree `get_node_at_position` that C5.5 added to `tsr_ast` for
  the JSDoc `@param` branch carried the port markers of `GetNodeAtPosition`
  and `nodeContainsPosition`, which the parser ported in Phase 1
  (`tsr_parser::references`, witnessed by `mutation/e1`); a second marker
  broke that operation's witness link in the Phase 1 scope. The helpers now
  name the parser's port instead.

The recorded `target/phase2/rust-c5` capture completed all 13,432 variants
with no timeout, execution failure or harness error, and replays
source-stable. 13,417 match every enabled domain, three more than at the C5
start; all 9,367 S08 regression variants match; no domain of any row regressed
and no observation changed against the C5-start report, and the 154 traced
variants keep trace parity at 1. By checkpoint: C2 matches all 2,954 of its
rows, including the three emit-order rows it handed to C5; C3 keeps 170 of
185 with the other 15 blocked on content-mapper execution (Phase 5); C4
matches all 926. The comparison is recorded in
`data/phase2/first-comparison.json`.

The C5 driver asks every Phase 2 request for the post-emit schedule, so each
request changed by that one field (`emit_schedule`). The 15 C3 content-mapper
handoffs keep byte-identical observations and were rebound with the new
request digest; the three C2 handoffs and the three C5 incoming entries were
rebound to the rows' new, matching observations, and the C5 claims are closed
with `8f825b5`. The rebuilt register has one entry, the content-mapper
refusal (Phase 5, 15 variants); the post-emit order entry is closed. Its
declaration audit has 1,751 working declaration requests and three not loaded:
the five that were `different` at the C4 exit (1,746 working) work since
`5526221`.

The two C1 claims that stayed `blocked` on the post-emit order
(`incorrectRecursiveMappedTypeConstraint` and
`typeParameterWithInvalidConstraintType`, whose C1 cause `341f3a3` fixed) are
`claimed` now that they match, since the register no longer lists them.

All nine C5 contracts pass in debug and release; the receipt is
`data/phase2/receipts/c5-contracts.json`, and the C1 to C4 receipts were
refreshed on the same sources. The services receipt
`data/phase2/receipts/c5-services.json` is current (every call matches or
falls under an approved entry; `complete` true since the approvals). Relater
parity is 105 of 105 groups for both implementations with `all_cases_match`
true (`target/s08/relater-c5`). Workspace formatting and clippy with warnings
denied pass, `cargo xtask validate` passes, and the Python suite passes 1,426
tests and 2,106 subtests; its seven failures are the same seven subtests of
the two Phase 1 ledger-closure tests that the C3 and C4 records describe: the
fixture files under the crates' `tests/fixtures` directories are inside the
Phase 1 source closure but outside the ledger's source globs (312 files at
this head; the C5 services subset joins that set), a Phase 1 ledger
amendment for the end-of-phase green-up.

The checker producer reports `c5_open = 0`, `c5_regressions = 0`,
`c5_failures = 0`, `c5_blockers_open = 0`, `c5_handoffs = 0`,
`c5_audit_complete = true` and `c5_contracts = true`, with
`regression_parity = 1` and `trace_parity = 1`, and every prerequisite true;
`c5_services` was false at the exit, so `c5_complete` was false, until the owner
approved the entries of `data/phase2/services-approvals.json`, and the
replay then ran again to bind the approvals. C1 stays complete. For C2, C3 and C4 the producer reports the accounting (open
0, regressions 0, failures 0, open blockers 0, audits complete, contracts
true; `c2_handoffs = 3`) and no completion metric. The recording, `cargo
xtask run checker`, is the owner's.

## After the exit

The owner approved the five services entries, and the replay receipt was
refreshed over them (`e2f0876`).

The #68 review follow-up (`c4f2dc5`, recorded in `docs/PHASE2-C4.md`) changed
production sources after the exit capture. So `target/phase2/rust-c5` was
captured again on the new head. The comparison, the handoffs and the register
followed it, and so did the C1 contract receipt; the C2 to C5 contract
receipts still need refreshing on these sources. The
capture completed all 13,432 variants with no harness error, and 13,417
match every enabled domain, as before. All 9,367 regression variants match,
and no outcome changed. Two raw observations changed, and only in type ids:
`isolatedDeclarationErrorsClasses` and `overloadsWithComputedNames` swap two
pairs of ids from run to run of the same binary. That traces to the random
iteration order of the std map from which the checker builds its symbol
tables; it is not an effect of the follow-up. All 21 handoffs rebound
without a change, and the register is unchanged (B01, content-mapper
execution, Phase 5).
