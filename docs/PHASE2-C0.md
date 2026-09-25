# Phase 2 C0 record: the checker acceptance contract

C0 of the [Phase 2 plan](PHASE2-plan.md), implemented from the
[C0 plan](PHASE2-C0-plan.md) on branch `phase2-c0` (from `main` `7bed6d8`,
after PRs #56 and #57). Upstream remains Corsa
`1f70213d4922b434345f639b441681e470c7cfc1`. C0 prepares; it changes no checker
semantics. The one production addition is a read-only accessor,
`Operation::union_types` (the pinned public `Checker.UnionTypes`), which the
union-ordering sub-test needs.

## Exit

| Exit condition | State |
| --- | --- |
| Inventory frozen and reproducible (C0.1) | `phase2_inventory.py check` rebuilds it byte for byte |
| Native capture of every executed variant, verified from two shardings (C0.2) | 13,432 rows; 10 contiguous shards and 7 interleaved shards agree on every row |
| Native outputs equal the committed references (C0.2) | 0 disagreements over every `.errors.txt`, `.types`, `.symbols` and `.trace.json` outcome |
| One complete categorized Rust run; every difference bucketed (C0.3, C0.4) | 13,432 rows, one bucket and one owner per open row |
| Harness validation, no unresolved adapter failure (C0.3) | 0 harness errors; every fatal row attributed to production |
| Blockers named with counts and owners (C0.5) | 18 entries in `data/phase2/blockers.json` |
| Producer records and `P2A-C0` passes (C0.6) | producer and sprints validated; **recording is the owner's** (`cargo xtask run checker`) |
| Rust definitions of the three owned sub-tests (C0.7) | `tools/phase2/subtests.rs`, 9 direct tests |
| Cost of one full run measured, run policy set (C0.8) | below |

The tables and gap counts below describe the initial C0 capture. The review
fixes expanded the Rust build fingerprint to include `rust-toolchain.toml`,
`.cargo/` configuration and bundled library bytes. That capture did not record
those inputs, so it is now historical: replay preserves its observations but
reports `source_stable: false`, and the producer withholds acceptance metrics.
A new Rust capture, comparison and blocker record are required before recording
`checker`; the native contract is unchanged. No full corpus was rerun for these
harness fixes, and no new input hashes were attached to the old capture.

## Owner decisions (2026-09-25)

1. **Module-resolution sub-test.** Phase 2 owns it and its gate,
   `run.checker.trace_parity`: C0 captures and compares the 148 `.trace.json`
   baselines through the compiler runner and C7 requires them, subject to
   approved divergences. Phase 1 stays responsible for the resolver; a trace
   difference is a foundation defect to fix.
2. **Display.** S08's `TypeToString` protocol, unchanged: every type the native
   walker returns, the same flags, enclosing context and query order, exact
   strings, unsupported calls as failures. Hover and `typeToTypeNode` come in
   C5 as separate observations and metrics.
3. **Run budget.** Intermediate checkpoints run the recorded 300-variant sample
   plus targeted cases for the change and its known regressions; full runs are
   C0's gap map and C7's acceptance, and whenever a broad change or a sampled
   regression justifies one. Samples show progress; they never claim parity.

## C0.1 The denominator

`data/phase2/inventory.json` has one row per effective variant (15,206), joined
from the S07 subset, the Phase 1 syntax schedule and the S08 acceptance
partition. Each row carries the ordered effective options and root keys, roots,
loading-request digest, native selection and boundary, tier, harness flags,
reference blobs by kind, family tags, the owning checkpoint, the S08 tier and
the sample flag. The document records its three input digests; `check`
rebuilds it and requires byte identity.

| Rows | Variants |
| --- | ---: |
| Effective variants | 15,206 |
| **Executed** (the Phase 2 denominator) | **13,432** |
| of which the S08 acceptance subset | 9,367 |
| of which newly included | 4,050 |
| of which content-mapper | 15 |
| Informational: option guard / filename skip / not enumerated | 1,720 / 52 / 2 |

**Correction to the plans' 13,434.** The pinned runner enumerates only
`\.tsx?$` files (`compilerBaselineRegex`). Two physical case files are `.js`:
`compiler/jsxNestedIndentation.js` and
`conformance/parser/ecmascript5/Statements/ReturnStatements/parserReturnStatement4.js`.
The Phase 1 syntax schedule marked them `runs`, and S07 indexed their
`.types`/`.symbols` references by stem, although those files belong to the
`.tsx`/`.ts` tests of the same name. The C0.2 review exposed the mismatch, and
the inventory now classifies both as `not_enumerated` (informational). Both are
in S08's 9,369 acceptance set, so the regression subset Phase 2 executes is
9,367.

Other counts over the executed rows: 7,301 `.errors.txt`, 12,751 `.types` and
`.symbols` and 148 `.trace.json` references. 464 rows have none of the checked
three (413 emitted-output-only, where the absent `.errors.txt` asserts zero
diagnostics), and 679 disable the type/symbol walk. `GetEmitDeclarations`
(declaration or composite) holds for 1,754; the plan's 1,756 also counted
`emitDeclarationOnly` keys, and the native capture observes exactly 1,754
requests.

Checkpoint owners: regression 9,367, C2 2,954, C3 185 (emitted-output-only and
content-mapper rows with no type-level, JSX or decorator family), C4 926.

The 300-variant sample is a deterministic greedy cover. Every checkpoint gets
8 representatives, every family 3, and every value of each option and harness
feature 2. The sample is then filled to regression 100, C2 100, C4 70 and C3 30
in `sha256(id)` order. The rule and the selection are in the inventory.

## C0.2 The native contract

`scripts/phase2_native.py` compiles one oracle test binary from the S08
access-only overlay and runs it in upstream's default single-threaded mode. The
overlay keeps S08's harness stage hooks, original baseline writers, walker
pulls, post-walk `TypeToString` schedule and native error selection. Three
observations are generated mechanically from the pinned `compiler_runner.go`,
each assertion becoming a recorded verdict:

- `verifyUnionOrdering` over every checker of the program;
- `verifyParentPointers`, which still stops at its first failure;
- the module-resolution trace (`verifyModuleResolution`).

A fourth hook records the post-emit program's declaration diagnostics.

- **Capture:** 13,432 rows, all `executed`, in 42 s wall on 10 shards (oracle
  build cached).
- **Verify:** a second, interleaved 7-shard capture agrees on every row's
  contract digest. Six rows differ only in the walker's numeric `type_id`
  values, which vary with process-shared harness caches. S08 already excludes
  them from the contract (`s08_queries.action`), and every baseline byte is
  identical.
- **Review:** every native `.errors.txt`, `.types`, `.symbols` and
  `.trace.json` outcome equals the committed reference file, and every absent
  reference matches `no_content` or `disabled`. There are 0 input mismatches,
  1,754 declaration requests and 154 trace rows (148 with content, 6 empty).
  No union-ordering inconsistency and no parent-pointer failure occurred
  natively.
- **Emit order.** The structured pre-emit and post-emit diagnostic sets differ
  in 3 variants (`incorrectRecursiveMappedTypeConstraint`,
  `typeParameterWithInvalidConstraintType`, `recursiveMappedTypes`), even
  though their counts are equal, so the pin's mismatch diagnostic never fires.
  After emit, a circularity error gains related information or moves to
  another node. The baselines are the post-emit sets. Blocker B09 records this
  dependency.

`data/phase2/native-provenance.json` records the request, observation and
per-row digests, stage outcomes, oracle binary and overlay identities, and the
toolchain (go1.27.1 darwin/arm64) with its review.

## C0.3 The Rust run

`scripts/phase2_corpus.py` runs `phase2_checker`, which is the S08 P5 row plus
the three sub-tests, over the same requests, one process per variant.
Requests carry native inputs only. Loading requests are rebuilt from the native
preprocessing S07 froze, and each hashes to its inventory digest. Completion
records are immutable, bound to the request, capture and executable digests,
and replayed before any report.

Harness defects are separate from compiler outcomes:

- protocol violations are harness defects;
- panics are attributed by location: `crates/` is production, the adapter is
  the harness;
- deadlines and stack overflows are production gaps;
- a walker or renderer error counts as harness only when its text is an
  adapter literal;
- anything unattributed is a harness defect.

`scripts/tests/test_phase2_corpus.py` covers these rules:

- it rejects missing, extra, reordered and duplicate rows, forged request
  digests, stale capture metadata, changed raw outputs and unknown statuses;
- an injected adapter panic becomes a harness error, not a gap, and a real
  production panic stays a measured failure;
- resume requires identical inputs;
- five named S08 controls are compared again from committed raw observations;
- panic completions must equal their raw observation, and stdout/stderr plus
  every present observation must carry a hash;
- sub-test counts reject negative numbers, booleans and missing fields, and
  trace/failure records have state-specific schemas;
- the tests use seven small recorded fixtures, with no `target/` dependency or
  capture-dependent skip.

Result: 13,432 rows, 0 harness errors, 13,426 completed and 6 production
failures. A second run from identical sources reproduced every row
observation (`--previous` reports 0 changed observations):

- five panics: `relater_tuples.rs:130` ×3, `infer_tuples.rs:65` and
  `tsr_ast/src/factory.rs:122`;
- one stack overflow in `compiler/infiniteConstraints2.ts`.

## C0.4 The first comparison

`scripts/phase2_compare.py` puts every domain of every row in one category.
`data/phase2/first-comparison.json` holds the summary; `comparison.json` in the
capture holds the rows.

**12,308 of 13,432 executed variants (91.6%) match in every domain.** The S08
regression subset matches completely: 9,367 of 9,367. All 154 traced variants
match their `.trace.json` byte for byte.

| Domain | match | different | failed | unsupported | disabled |
| --- | ---: | ---: | ---: | ---: | ---: |
| errors | 12,372 | 223 | 19 | 818 | |
| types | 12,015 | 69 | 20 | 649 | 679 |
| symbols | 12,075 | 9 | 20 | 649 | 679 |
| display | 12,037 | 47 | 20 | 649 | 679 |
| trace | 154 | | | | 13,278 |
| union ordering | 13,411 | | 6 | 15 | |
| parent pointers | 13,411 | | 6 | 15 | |

By owner: C4 767 open of 926 (every JSX variant, 331 of 490 decorator
variants), C2 342 open of 2,954, C3 15 open of 185 (the content-mapper rows),
regression 0 open.

The largest buckets are listed below; the full list of 74 buckets, each with
its smallest reproduction, is in the record.

| Owner | Bucket | Variants |
| --- | --- | ---: |
| C4 | unsupported `checkExpressionWorker` (JSX expressions) | 439 |
| C4 | unsupported `checkClassLikeDeclaration: decorators` | 284 |
| C2 | diagnostics differ, first at TS2322 | 73 |
| C2 | unsupported `typeReferenceToTypeNode: applied outer arguments` | 36 |
| C2 | unsupported `extractRedundantTemplateLiterals` | 30 |
| C2 | diagnostics differ, first at TS4025 | 27 |
| C4 | unsupported `checkDecorators: parameter or binding` | 25 |
| C2 | `.types` differs | 24 |
| Phase 5 | unsupported content-mapper execution | 15 |

The report's "checker area" comes from execution evidence: the named refusal,
the panic's Rust module, or the first differing diagnostic code. The S07
obligation records map library syntax families, not variants to Go functions,
so they cannot attribute a row. Family and configuration counts are reported
as dimensions and never create an outcome. Given `--previous`, the report also
lists rows whose category is unchanged but whose observation changed.

## C0.5 Blockers

`data/phase2/blockers.json` has 18 entries, each with variants, withheld
domains, owner and evidence:

- 16 named production refusals owned by C2 or C4, led by
  `checkExpressionWorker` (440) and class decorators (284);
- content-mapper execution (Phase 5, 15);
- the emit-order dependency (C5 emit resolver with Phase 3 emit, 3).

The declaration audit covers the 1,754 native requests:

- 1,669 working paths give identical declaration diagnostics;
- 67 differ; these are counted as differences, not blockers;
- 15 are withheld by named checker refusals;
- 3 panic;
- 3 are content-mapper rows.

No declaration-transform operation is refused on its own, so there is no
Phase 3 transform blocker at C0.

Phase 1 contracts consumed by C0 have no open item: program loading with
project references (#55, #56), options and module resolution (#49, #50, #55),
and the binder (#51, #53, #55). Phase 1 still has four pending entries after
#56 (the Linux `Realpath` capture and fresh mutation evidence for
`WithFileNames`, `ThrottleGroup.Go` and `ThrottleGroup.Wait`); C0 consumes none
of them.

## C0.6 Namespace, producer and sprints

- `data/phase2/`: the inventory, native provenance, first comparison and
  blocker register.
- `scripts/phase2_{inventory,native,corpus,compare,blockers,producers}.py`.
- Tests: `scripts/tests/test_phase2_{inventory,corpus,compare}.py`.
- `tools/phase2/subtests.rs` and
  `crates/tsr_compiler/{examples/phase2_checker.rs,tests/phase2_subtests.rs}`.
- `status/runs.toml` `[checker]` replays the recorded captures and never runs
  a corpus. It emits `inventory_frozen`, `native_verified`, `harness_valid`,
  `result_recorded` and `blockers_named`. It also emits the ratios
  `errors_parity`, `types_parity`, `symbols_parity` and `display_parity` over
  the executed denominator, `trace_parity` over traced rows, `ordering`,
  `parent_pointers`, `unsupported_required` and `regression_parity`.
- `sprints/P2A.toml` (`P2A-C0`) and `sprints/P2B.toml`. P2B holds C1 to C7
  with metrics their plans will wire, so it stays open by construction; its
  exit is the Phase 2 gate. P2A has no blanket Phase 1 flag, following PLAN's
  dependency rule.

Run directly (without recording evidence), the producer reports
`inventory_frozen`, `native_verified`, `harness_valid`, `result_recorded` and
`blockers_named` true. The ratios are errors 0.921, types 0.945, symbols 0.950,
display 0.947, trace 1.0, ordering and parent pointers 0.998, regression 1.0,
with 859 variants that have an unsupported domain.

`cargo xtask validate` accepts both sprints and the producer. The E2 producer
is untouched. New files under its fingerprinted directories stale E2's record,
which stays history; `regression_parity` carries the current 9,367 result.
Recording `cargo xtask run checker` is the owner's step on the designated host;
until then `cargo xtask check P2A` reports the C0 items as pending.

## C0.7 The sub-tests on the Rust side

- **Union ordering.** Checks every union the checker interned
  (`union_types`), sorting its reversed list and ten shuffles with the
  production comparator. The shuffles use a port of Go's `math/rand/v2` PCG
  and `Shuffle` with the pinned seed. C0 runs it over the program's single
  checker; C6 runs it per checker. The verdict is compared, not the union
  count.
- **Parent pointers.** Walks each non-default-library file (`is_lib`, the port
  of `IsSourceFileDefaultLibrary`) through the generated child visitor, in
  Go's pre-order. It stops at the first missing or mismatched parent. The root
  is not checked.
- **Trace.** Localizes the loader's trace and sanitizes it through a port of
  `TracerForBaselining`, including its per-compile package.json cache.

The direct tests in `crates/tsr_compiler/tests/phase2_subtests.rs` cover:

- Go's PCG and shuffle vectors;
- detecting an inconsistent comparator without crashing;
- a checked program's unions;
- a user declaration file included in the walk, with libraries skipped;
- a reparsed JSDoc child reached by the walk;
- missing and wrong parents with stop-at-first-failure, on a constructed tree;
- trace rendering and each sanitizer rule.

## C0.8 Cost and run policy

Measured on the development host (18 cores, Apple M-series, go1.27.1). The
build is excluded unless stated.

| Step | Wall | Processes | Disk |
| --- | ---: | ---: | ---: |
| Native capture, 13,432 rows | 42 s (10 shards) | 10 test processes | 1.8 GB (864 MB observations, shards) |
| Native second sharding | 52 s (7 shards) | 7 | 1.7 GB |
| Rust run, 13,432 rows | 309 s (14 jobs); 353 s with build and replay | 13,432 | 2.1 GB |
| Comparison and register | about 50 s | 1 | the row report |
| `checker` producer (replays all of it) | 141 s | 1 | none |

A full contract takes about 8 minutes of wall time on this host. That is cheap
enough to run whenever a broad change warrants it. The owner's policy stands:
intermediate checkpoints run the recorded 300-variant sample plus targeted
cases (about 10 s of Rust time), and full runs are C0's gap map and C7's
acceptance.

## Intermediate runs

Use the frozen sample plus repeatable named targets; their union runs in
inventory order. `--case` alone selects just the named executed variants.
Duplicate or unknown targets, including informational variants, are rejected.

```sh
python3 scripts/phase2_corpus.py run --native target/phase2/native \
  --output target/phase2/c1-sample --sample \
  --case 'compiler/sliceTupleTypeOutOfBounds.ts#configuration=0'
python3 scripts/phase2_compare.py report --native target/phase2/native \
  --rust target/phase2/c1-sample
```

Selection is bound into capture metadata and completion hashes. `--resume`
requires that same selection, native capture, requests and deadline. Reports
include only selected native/Rust rows and say `partial: true`; they cannot be
recorded with `--record`, replace the full blocker register, or emit acceptance
metrics through the producer. `--limit N` remains a separate prefix smoke test
and cannot be combined with `--sample` or `--case`.

The Phase 2 exit now explicitly includes `run.checker.display_parity == 1`,
alongside errors, types and symbols.

## Reproduction

```sh
export PATH="$(mise where go)/bin:$PATH"
python3 scripts/phase2_inventory.py check
python3 scripts/phase2_native.py capture --output target/phase2/native --shards 10
python3 scripts/phase2_native.py verify --capture target/phase2/native --shards 7 --scheme interleaved
python3 scripts/phase2_native.py review --capture target/phase2/native --record
python3 scripts/phase2_corpus.py run --native target/phase2/native --output target/phase2/rust
python3 scripts/phase2_compare.py report --native target/phase2/native --rust target/phase2/rust --record
python3 scripts/phase2_blockers.py build --native target/phase2/native --rust target/phase2/rust --record
python3 scripts/phase2_producers.py checker
python3 -m pytest scripts/tests/test_phase2_*.py -q
cargo test -p tsr_compiler --test phase2_subtests
cargo xtask validate && cargo xtask check P2A
```
