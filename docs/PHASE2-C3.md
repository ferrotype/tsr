# C3 implementation record

## Starting point

C3 starts from the recorded C2 exit capture at `target/phase2/rust`: 13,432
executed variants, no harness error, 12,647 full-domain matches, all 9,367 S08
regression rows matching, and `P2B-C2` recorded complete. The native capture
is reused unchanged. `data/phase2/c3-baseline.json.gz` is that row report
frozen for the `c3_regressions` comparison (`cdacf17`).

The C3-start worksheet `data/phase2/c3-claims.json` holds every C3-owned row
that was open in that capture: the fifteen content-mapper rows, each `blocked`
with a Phase 5 handoff. The handoff names the Go operation
(`tsc/internal/contentmapper/transform.go:TransformAndParse`), the Rust refusal
site (`crates/tsr_compiler/src/loader.rs`, traced per row in
`data/phase2/c3-content-mapper/trace.json`), the withheld domains, the exact
reproduction command and the request, raw-observation and capture digests.
No C2 row was handed to C3, so the worksheet has no `incoming` entry at the
start. The worksheet claims neither the audit nor C3 completion.

## Producer wiring

`2a3448f` wires the C3 authorities before any semantic work, so that every
later run is measured the same way:

- `scripts/phase2_producers.py` registers the C3 claims, audit and baseline
  under `CHECKPOINT_AUTHORITIES["C3"]` (no measurement authority: C3 sets no
  performance condition, and `checkpoint_metrics` only requires a
  `_measured` metric for checkpoints that declare one). The checker producer
  reports `c3_open`, `c3_regressions`, `c3_failures`, `c3_blockers_open`,
  `c3_handoffs`, `c3_audit_complete`, `c3_contracts` and `c3_complete`, and
  builds the blocker register once from the C2 and C3 handoffs merged.
- `scripts/phase2_blockers.py` learns the C3 claims and audit and merges the
  handoffs of every checkpoint (`load_all_handoffs`), refusing a variant that
  two checkpoints hand off.
- `scripts/phase2_audit.py` binds the C3 scope: seven reviewed groups with
  their function counts and digests, `flow.go` as the file whose inventory
  must be complete, and no required handoff.
- `scripts/phase2_claims.py rebind` re-binds the capture, request and
  raw-observation digests of `handed`, `blocked` and `incoming` entries to a
  later capture when the unmet domains are still covered by the handoff and
  the trace digest is unchanged, reporting any stale entry.
- `status/runs.toml` lists the five new inputs of the `checker` run, so that
  changing any of them invalidates the recorded result.
- `scripts/tests/test_phase2_c3.py` covers the exit metrics (a `blocked` row
  is excluded from `c3_open` only with a registered blocker, an `incoming` row
  counts until it matches, the prerequisites and authorities are required, no
  measurement is required), the rebind, and the wiring of the committed files.

## Function audit

The audit disposes the seven C3 groups of the plan over the pinned inventory
(`data/go-functions.tsv`), one commit per group (`132ffa0`, `4a6b44d`,
`40739da`, `006a1e9`, `e2447c9`, `d759e63`, `4e32ec3`). A function is
`mapped` when a `// port:` marker names it anywhere under `crates/`,
`equivalent` when the Rust site that carries its behavior is named with the
reason and the reviewed source, and `later` when a named later checkpoint owns
it. No group has a `gap`.

| Group | Functions | Mapped | Equivalent | Later |
| --- | ---: | ---: | ---: | ---: |
| C3.2 narrowing and flow (`flow.go`, complete) | 130 | 127 | 3 | 0 |
| C3.3 iteration, async and generators | 11 | 10 | 1 | 0 |
| C3.4 classes, `this` and members | 51 | 5 | 42 | 4 |
| C3.5 namespaces, modules and aliases | 33 | 5 | 25 | 3 |
| C3.6 JavaScript and JSDoc (`jsdoc.go`) | 4 | 2 | 2 | 0 |
| C3.7 statements, expressions and grammar (`grammarchecks.go` share) | 136 | 58 | 76 | 2 |
| C3.8 utilities (`utilities.go` share) | 143 | 51 | 84 | 8 |
| Total | 508 | 258 | 233 | 17 |

The diff adds 61 marker lines at the sites where the Rust port inlines a
pinned function (the `type_at_flow` arms, the narrowing, switch, equality,
reference and destructuring flow helpers, the iteration protocol, accessor
and private-member helpers, `getThisContainer` in the binder helpers, the
scope-change cache, the module specifier helpers, `checkBinaryExpression`,
`checkBindingElement`, `checkExpression`, `checkSourceElement` and the
kind-assignability helpers). The seventeen `later` dispositions are:

- C4: `getClassElementPropertyKeyType`, `getParentTypeOfClassElement`,
  `markEntityNameOrEntityExpressionAsReference`, `markTypeNodeAsReferenced`,
  `newParameter`, `newProperty`, `getEntityNameFromTypeNode`,
  `isJsxIntrinsicTagName` (the JSX and decorator half of alias marking and
  the class-element helpers those checks need);
- C5: `getDiagnostics`, `getMemberOverrideModifierStatus`,
  `IsExternalModuleSymbol`, `SkipAlias`, `introducesArgumentsExoticObject`,
  `isRightSideOfAccessExpression`, `symbolsToArray` (resolver and
  declaration-emit queries);
- C6: `checkNotCanceled`, `isCanceled` (cancellation).

`python3 scripts/phase2_audit.py check --audit data/phase2/c3-audit.json`
reports `valid` and `complete` with no problem.

## The raw-lookup call-path audit

The C1 record left open whether every raw `lookup_symbol` caller tolerates an
unresolved alias. The pin's `getSymbol` resolves an alias to its target's
flags before testing the meaning; the Rust `lookup_symbol` refuses that case
(`getSymbolFlags: alias resolution`) and only the resolve-name path and
`lookup_symbol_resolving` retried it. `201b514` switches the six remaining
raw sites that look up a name whose declaration may be an alias to the
resolving wrapper: the `Record` lookup of the narrowing code, the export
lookup of the JSDoc checks, the `Symbol` lookup of the iteration protocol, the
`NaN` lookup of the equality operators, the `Extract` lookup of the statement
checks and the `NonNullable` lookup of the flow facts. After the change the
only raw callers are the resolve-name worker and the resolving wrapper itself;
fourteen sites go through the wrapper. The fixture `lookup_alias_globals.ts`
(`import =` aliases of a namespace's `Record` type and `NaN` value, reached
through an `in` narrowing and a `=== NaN` comparison) records the native
diagnostics for the case.

## Project-reference callers

The C1 record named three callers of the project-reference accessors that C1
had not implemented. All three are ported against the pin and observed
natively (`d50f0e9`, `0bd784d`):

- `canHaveSyntheticDefault` asks the program for the referenced output's emit
  module format instead of refusing, and answers false for an ES-module
  target under an `esnext` import mode, as the pin does
  (`external_aliases.rs`).
- `resolveExternalModule` compares the relative path between the two projects'
  common source directories with the relative path between their output
  directories when a `.ts` import is rewritten across a reference, and reports
  `TS2878` when they differ (`external_resolution.rs`).
- A source that resolves to a referenced project whose declaration output is
  not in the program reports `TS6305`, naming the output and the source. The
  host accessor `get_project_reference_from_source` now returns the pin's
  source-with-output shape (`ProjectReferenceSource`), so the checker can name
  the output.

`crates/tsr_compiler/tests/checker_project_references.rs` holds one case per
caller, each asserting the diagnostic code, one-based position and message the
pinned `tsgo -p tsconfig.json --noEmit --pretty false` printed on the same
workspace: `TS1192` at `main.mts(1,8)` with an ES-module reference and no
diagnostic with a CommonJS reference; `TS2878` at `main.ts(1,22)` with the
reference emitting to `ref/out` and no diagnostic when both projects keep the
same layout under `dist`; `TS6305` at `main.mts(1,17)` when `ref/out/a.d.ts`
is missing. The test now loads the bundled lib so the checker's global
initialization matches the native run; the earlier display cases keep `noLib`
in their configs and are unchanged.

## Direct contracts

`crates/tsr_compiler/tests/c3_contracts.rs` (feature `recursion-probe`,
`67bdf87`, `2e33afe`) holds 34 tests: four written directly and thirty over
fixtures under `crates/tsr_compiler/tests/fixtures/c3/`.

Each fixture's native diagnostics are recorded by `fixtures/c3/regenerate.py`
with the pinned `tsgo` build (`--noEmit --target esnext --ignoreConfig
--pretty false` plus the fixture's own flags from `fixtures.json`) into a
`.native.json` that binds the pin, the source digest, the executable digest,
the command and the output digest. `support/c3_native_diagnostics.rs` builds
the same program over the bundled lib, renders the syntactic then semantic
diagnostics the way the non-pretty output prints them (message chains joined
by newlines, no related information, which that output never prints) and
compares. The thirty fixture cases are:

- nineteen `changes_*` cases, one per JavaScript item of the pin's
  `CHANGES.md` (conflicting declarations, template code points, unknown
  arguments in non-strict JavaScript, JSDoc values as types, `arguments`
  without rest, variadic JSDoc types, postfix `=`, `asserts` on the declaring
  variable, async non-promise returns, `@typedef` in a class body, `@class`,
  `@param` scope, type assertions and narrowing, `@overload` on arrows,
  constructor functions, `void 0` expandos, `this` property annotations and
  mixed `module.exports` assignments), which also carry contract 7;
- six `options_*` cases (`exactOptionalPropertyTypes`,
  `noUncheckedIndexedAccess`, `useUnknownInCatchVariables`,
  `verbatimModuleSyntax`, `isolatedModules`, `noFallthroughCasesInSwitch`);
- contracts 2 to 5: definite assignment across loops, labels, `try`/`finally`
  and class property initializers; evolving arrays and discriminated unions
  through optional chains and `switch`; iteration types over the bundled lib's
  iterators, async iterators, maps and generators; override, abstract,
  accessor, private-name, static-block and `this`-type class checks;
- the global-alias lookup case of the call-path audit.

The four direct tests are contract 1 (two checkers over one program narrow the
same reference independently, and retiring one generation leaves the other
answering), contract 6 (a value alias is marked referenced, a type-only import
and a type-only re-export are marked type-only, observed through the
`alias_link_state` probe; the resolver's queries over that state are C5.6's),
contract 8 (a file with parse errors: the syntactic diagnostics equal the
native set, which is all the native CLI prints for such a file, the semantic
pass completes with diagnostics, and the same checker answers a builtin-type
query afterwards with the same result twice) and contract 9
(`binderBinaryExpressionStress` checks on a 256 KiB stack with the recursion
probe, and its diagnostics equal the main-thread run).

## Intermediate sample

The recorded 300-variant sample (`target/phase2/c3-s1`, run at `7b2205e`
before the project-reference work) has 237 full-domain matches, no regression
and no changed observation against the recorded C2 exit: the 100 regression
and 100 C2 rows match, 25 of the 30 C3 rows match and the other five are the
content-mapper rows, and the C4 rows are unchanged (12 match, 58 open). No
full run was made during the implementation; the exit run below is the first.

## Producer follow-ups at the exit

Two producer rules were settled by the exit run itself (`24458e2`):

- The rebind first rewrote the claims file's own `rust_capture_sha256`. The
  producer checks that field against the checkpoint's baseline (the start
  capture), so the rewrite made the C3 claims authority invalid. The rebind now
  leaves a claims file's start bindings alone and rewrites only the handoffs;
  the test asserts it.
- The producer computed `c2_complete` on the C3 capture and reported it false
  because the C2 measurement is bound to the C2 exit sources. The plan's
  completion rule says a checkpoint's completion is the checker run recorded
  at its exit and no later checkpoint recomputes `cN_measured` or
  `cN_complete` on its own capture. The producer now computes completion only
  for the newest checkpoint with authorities and reports an earlier
  checkpoint's accounting (open, regressions, failures, open blockers,
  handoffs, contracts) without those two metrics.

## S07 operation anchors

The port markers and the host edits moved the line anchors of 33 functions in
`data/s07/operations.json` (all in `rust_mappings`; Go functions, call edges,
boundaries, variants and obligations are identical after stripping the
mappings). The inventory was regenerated and the subset review re-frozen under
the owner's standing mapping-only approval, finding
`PHASE2-C3-2026-09-26-mapping-refresh` (`461dbf1`): `subset.json` and
`checker-obligations.json` are byte-identical and only the rule's operation
matrix digest changes.

## Exit run

The fresh `target/phase2/rust-c3` capture (sources at `0bd784d`; the later
commits change scripts and data only) completed all 13,432 variants with no
timeout, execution failure or harness error. 12,647 match every enabled
domain, the same count as the recorded C2 exit, since every open C3 row is a
content-mapper row; all 9,367 S08 regression variants match; no domain of any
row regressed and no observation changed against the recorded C2 exit. The 154
traced variants keep trace parity at 1. By checkpoint: 170 of the 185 C3-owned
rows match and the other 15 are the content-mapper rows, blocked under B04
(Phase 5); C2 keeps 2,951 matches with its three emit-order rows handed to C5;
C4 has 159 matches and 767 open rows. The comparison is recorded in
`data/phase2/first-comparison.json`.

The 15 C3 claims and the three C2 handoffs were rebound to this capture, none
stale. The rebuilt blocker register has seven entries, B01 to B07, and no
C3-owned one.

All 34 C3 contracts pass in debug and release; the receipt is
`data/phase2/receipts/c3-contracts.json`. The C1 and C2 receipts (7 and 16
contracts) were refreshed on the same sources so that they are current at this
head. Relater parity is 105 of 105 groups for both implementations with
`all_cases_match` true (`target/s08/relater-c3`). Workspace formatting and
clippy with warnings denied pass, `cargo xtask validate` passes, and the
Python suite (1,386 tests, 2,084 subtests) passes after the S07 re-freeze
except seven subtests of the two Phase 1 ledger-closure tests
(`test_phase1_tracker.py`, `test_phase1_integration.py`). Those fail at the
base commit `bb29600` in the same way: the C2 fixture, oracle and observation
files under the crates' `tests/fixtures` directories are inside the Phase 1
source closure but outside the ledger's source globs (265 files at this head;
the C3 fixtures join that set). The fix is a Phase 1 ledger amendment for the
end-of-phase green-up, not a C3 change.

The checker producer reports `c3_open = 0`, `c3_regressions = 0`,
`c3_failures = 0`, `c3_blockers_open = 0`, `c3_handoffs = 0`,
`c3_audit_complete = true`, `c3_contracts = true` and `c3_complete = true`,
with `regression_parity = 1` and `trace_parity = 1`; `c1_complete` is true
again with the refreshed receipt. For C2 it reports the accounting
(`c2_open = 0`, `c2_regressions = 0`, `c2_failures = 0`,
`c2_blockers_open = 0`, `c2_handoffs = 3`, `c2_contracts = true`) and no
`c2_complete`. Until C7.7 lets a tracker item name a recorded run, recording
this checker run makes `P2B-C2` read as pending in `sprints/P2B.toml` while
`P2B-C1` and `P2B-C3` read as done; that is the plan's known tracker gap, not
a C2 regression. The recording, `cargo xtask run checker`, is the owner's.
