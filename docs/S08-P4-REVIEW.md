# S08 P4 takeover review — 2026-09-13

Reviewed the uncommitted increment at `f952263` and the eight commits ending at
that revision, against the pinned Go tree
`1f70213d4922b434345f639b441681e470c7cfc1`. Changes remain on
`codex/s08-p4`, PR #22 (now based on `main`). This review does not certify P4
completion or E2 parity.

## Review scope

The uncommitted increment included the initializer-resolution fix, updated
checker/census tests, and lint corrections. Its changes were retained and
reviewed together. The committed range was:

| Commit | Main change reviewed |
| --- | --- |
| `8500112` | Import-attribute contextual types and const contexts |
| `c9155ab` | Entity-name chains and destructured property accessibility |
| `de1fdc4` | Specifier ranking, lib/package suggestions and alias merging |
| `0ffb55c` | Remaining small checker failure buckets |
| `2dd1581` | Spread-argument arity diagnostic spans |
| `b8e2906` | Long-tail boundaries and production suggestion diagnostics |
| `1b3a5da` | Split value/type exports, CommonJS containers and implements errors |
| `f952263` | Reentrant effects signatures |

Review concentrated on control flow, recursive state, diagnostic arguments and
ordering, symbol identity and the newly exposed production paths. It was not a
fresh review of the entire checker or a full native baseline comparison.

## Corrections

1. **Initializer resolution cleanup.** The new native initializer guard correctly
   stops the self-reference OOM. A fallible `type_facts` call could nevertheless
   leave its resolution entry on the stack. Both initializer checking and type
   facts now finish before the entry is popped, and errors propagate after the
   pop. The circular-default fixture preserves all eight native diagnostics.
2. **Missing reachability shortcut.** Rust had the shared-flow cache but lacked
   Go's `lastFlowNode`/`lastFlowNodeReachable` shortcut. It now records the last
   reachability query and clears the shortcut when reduce-label antecedents
   change. The old native sample attributed 3,675 of 3,727 samples to the
   unreachable-statement reachability path. This improves the large-flow case,
   but does not close its performance follow-up below.
3. **Reentrant alias diagnostics.** The circularity-report target was stored in
   shared checker state; a nested alias query could overwrite it. It is now local
   to each invocation, as `exportSymbol` is in Go. The nested three-module cycle
   fixture checks the complete native diagnostic output and repeated queries.
4. **Exported symbol spelling.** `getNameOfSymbolAsWritten` used the node's raw
   `name()` accessor instead of `GetNameOfDeclaration`. Consequently `export =
   third` displayed the internal `export=` name. The existing AST helper now
   supplies export-expression and assigned declaration names; the cycle fixture
   caught and verifies this separate display defect.
5. **Combined declaration lists.** `combineValueAndTypeSymbols` removed every
   duplicate declaration, whereas Go compacts adjacent duplicates after
   concatenation. Rust now uses the corresponding adjacent compaction. This was
   established by source comparison, not a new observed corpus failure.
6. **Pinned lib suggestion table.** The final `Date`/`esnext`/
   `toTemporalInstant` row was missing. The table now matches all 423 ordered
   native string entries. A regression checks TS2550 and its three arguments.
7. **Failed late source pass.** A failed unused/suggestion check consumed its
   pending queue but left the source marked complete, allowing a retry to appear
   successful. It now records terminal source failure before propagating the
   error. This correction came from error-path inspection.
8. **TSX is not itself an unsupported operation.** The blanket file-kind guard
   rejected ordinary declarations/imports in `.tsx` files, including the two
   eligible fixtures stored under `conformance/jsx`. TSX/JSX files now reach the
   ordinary checker. Actual JSX expressions still fail explicitly at the
   expression operation, and the failure-repeat test now contains a real JSX
   expression. No eligibility predicate or frozen option changed.

The stale test updates and the census family correction agree with the current
implementation. The lint gate stays `-D warnings`: straightforward lint issues
were fixed, while native branch structure, distinct optional cache states and
operation signatures use narrowly scoped allowances with reasons. No dependency
was added for separator counting, and no workspace-wide lint was disabled.

## Verification and retained artifacts

- The final workspace run passed all 660 tests, including 68 compiler semantic
  tests. The script suite ran 395 tests with one skipped. Fresh `clippy` and `fmt` producer
  evidence records clean results; tracker views are refreshed with this change.
- `tools/s08/p4/review-regressions.json`: six native programs and six queries,
  including a 10,000-assignment reachability control. Complete observations,
  diagnostic ranges/arguments/order, display results, four merge schedules and
  retained-owner checks compare equal, with no unsupported operation.
  Capture: `target/s08/p4-review-native-04`; comparison:
  `target/s08/p4-review-comparison-03.json`.
  Native observation SHA-256:
  `016940bb5f2e8775c46d518ac4cfeb621f1f4f5c649a1be8f061f0c29bd6fbe7`.
- The two original timeout requests completed their requested diagnostic phases
  in `target/s08/p4-review-inventory-01`. The initializer emits the eight expected
  diagnostics; the large-flow case emits TS2563 at 152–157, corresponding to the
  pinned reference baseline. This is an execution capture, not E2 evidence.
- All 25 requests previously rejected by the TSX guard completed their requested
  diagnostic phases in `target/s08/p4-review-inventory-02`: 24 acceptance and one
  informational. Capture SHA-256:
  `d528aa65196d787125ae6cb05b8038f83d7c25225147c28a3af770e0929ba095`.
  This uses the immutable `p4-review-build-02` executable/source snapshot and
  keeps both tiers separate.

Only the request spec and ordinary Rust regression tests are added to the
repository. Local native captures, copied executables and inventory artifacts
remain under `target/`; they are not promoted to committed acceptance evidence.

## Remaining after the initial review

This records the state before the follow-up corrections and full inventory below.

1. **Large-flow timeout margin.** Before this review the debug case was reported
   at 149 seconds. After the shortcut, one normal inventory invocation completed
   in about 54 seconds; a separate direct invocation took 62.81 seconds while
   other validation ran. The attempted native sampler could not attach, so there
   is no new self-cost ranking. These are diagnostics, not paired performance
   results. The case is still too close to the 60-second inventory deadline to
   call the timeout reliably fixed. Keep the deadline; inspect the remaining
   work before the full refresh.
2. **Full current-source inventory.** The latest full run remains
   `target/s08/p4-inventory-07`, which took about 113 minutes. It completed 9,343
   of 9,369 acceptance variants and 1,358 of 1,359 informational variants at its
   older revision. Its remaining rows were the two timeouts and the TSX guard
   failures just retested. Focused successful reruns cannot be added to that
   older run and called a new full capture. Run a fresh immutable build across
   all 10,728 variants after the timeout follow-up.
3. **Checkpoint acceptance.** Confirm P4's full-inventory and direct-library
   obligations on that revision, then review/finish PR #22. P5 owns the native
   type/symbol baseline walker and exact display/error-baseline work; later
   parity, storage and performance checkpoints remain separate. No S08 or E2
   gate is marked complete by this review.

## Follow-up corrections — 2026-09-13

The owner approved folding the subsequent review findings into this checkpoint,
with no CI changes or new remote CI runs. These changes are saved locally; do not
push merely to trigger validation.

- The range-loop lint cleanup in `6ed7264` introduced a panic when a rest
  parameter's index exceeded the argument count. Claude's pending correction
  restores the empty-loop behavior, and its two IIFE regressions now also assert
  that semantic diagnostics are empty. The affected frozen rows are
  06694/06695/06696, the default-parameter function-expression fixtures.
- DOM-name recognition now expands intersections as well as unions, matching
  `everyContainedType`. `missingDomElements.ts` now produces TS2812 for
  `EventTarget & HTMLInputElement`; its non-DOM and nonempty-object controls still
  produce TS2339.
- The unused-check result is cached separately from the completed type check.
  A late failure remains an error on repeated unused/suggestion requests without
  poisoning type-only requests or another source file. An injected unsupported
  registered node exercises the actual failure/queue-consumption path across
  operations, rather than merely seeding a cached error.
- Circularity reporting tolerates an alias with no declaration, returning `any`
  without a diagnostic as Go does. Callers that require a declaration retain
  their existing failure contract.
- Five duplicated non-local-alias predicates now use
  `ts_ast::is_non_local_alias`. This preserves Go's assignment-backed JavaScript
  alias case even when another symbol meaning is present. Tests distinguish that
  case from a local merged alias and exercise CommonJS export/re-export paths.

Local verification: 48 checker unit tests and 70 compiler semantic tests pass;
clippy for the affected crates/all targets/all features passes with warnings
denied. `tools/s08/p4/review-followups.json` matches pinned Go for all four
programs/queries and the four shared-source merge schedules, with no unsupported
operation. The capture uses explicit strict options; the separate IIFE unit
test also checks the original non-strict fixture behavior.

Native capture: `target/s08/p4-followups-native-02`; comparison:
`target/s08/p4-followups-comparison-01.json`. Native observation SHA-256:
`a982496b90b0b5005983899505e04d4b7d588dd721ccee659bec76ad6c8bd67a`.
The code and schema are unchanged in the existing oracle and inventory drivers.

`p4-inventory-08` was stopped before the known rest-argument failure. The later
`p4-inventory-10` has now been interrupted at 1,548 completed rows because its
immutable executable predates these four corrections. Its partial records and
source snapshot are retained. They must not be combined with a new binary and
called a complete current-source capture. The next full capture uses a fresh
build/source snapshot; the 60-second timeout and both acceptance tiers are
unchanged. The large-flow performance follow-up remains open.

## Full inventory and CI repair — 2026-09-13

The owner subsequently authorized fixing the producer failures and updating
PR #22. `target/s08/p4-inventory-12` captures the checker source at `b48bc1d`:
all 10,728 completion records replay successfully, and the captured source
fingerprint still matches the workspace. Both acceptance (9,369/9,369) and
informational (1,359/1,359) variants complete every requested diagnostic phase.
There are no timeouts in this run. The remaining named boundary is P5's
unimplemented type/symbol baseline walker: 9,171 acceptance and 1,248
informational requests. Diagnostic execution is not native diagnostic parity
or completion of E2; this run also does not establish a new performance margin.

The failures in CI run
[34756147448](https://github.com/iantocristian/ts-rust/actions/runs/34756147448)
have three concrete corrections:

- Add the existing synthetic-factory retention test to the exact S07 ownership
  roster, raising the binding-publication suite from 17 to 18 tests and the
  complete S07 ownership inventory from 83 to 84. Keep rejection of missing,
  failed and unexpected tests; extend the missing/failed-case checks to this
  retention test. Revalidating the archived macOS arm64 and both Linux outputs
  against the corrected roster accepts all 40 suite/mode outputs per runner,
  including Miri and ASan. This checks the correction against already-executed
  instrumentation; it is not a new current-source E3 capture.
- Regenerate `data/s07/operations.json`. The reviewed diff changes only 45 Rust
  mapping records, including added port markers and moved line locations.
  Source functions, call edges, operation boundaries and generator inputs are
  identical. Record the new digest as a mapping-only amendment to the existing
  subset review. The selected subset and checker-obligation files are unchanged;
  the complete freeze validator succeeds using the authenticated existing Go
  syntax and loader observations for all 10,728 variants.
- Run the memory-accounting test's heavy/light pair from a fresh interpreter.
  Linux's pre-exec RSS accounting can give both children the large unittest
  process's memory floor, invalidating the old fixed 80 MiB/40 MiB assumption.
  A Linux Python 3.12 container reproduces the old failure with a 160 MiB parent:
  both children report 187,682,816 bytes. The revised test passes under the same
  condition and rejects a deliberately substituted cumulative-child RSS result.
  The production benchmark collector and its measurement semantics are unchanged.

Local validation: all 395 script tests pass (one platform skip), the two focused
child-accounting tests pass on Linux, and the complete frozen-subset validator
returns `frozen_subset: true`. No Rust production source or workflow configuration
changes are needed for these repairs. The refreshed remote CI run must establish
the current four-target result before merge.
