# Phase 2 C0: the full checker acceptance contract

Status: **proposed for owner review**, 2026-09-25. The first detailed
checkpoint plan under the amended [Phase 2 plan](PHASE2-plan.md) and its
[review](PHASE2-plan-review.md). C0 prepares; it implements no checker
semantics and requires no Rust parity at its exit.

Planning reference: `main` `14aa5c95f` plus PR #56; upstream remains Corsa
`1f70213d4922b434345f639b441681e470c7cfc1`.

## 1. Objective and exit

Establish the contract every later checkpoint is graded against: the exact
denominator, a native capture of every required observation, a Rust run
categorized row by row, the named blockers, and the producer and sprint wiring
that records all of it. Exit when:

- the inventory is frozen and reproduces from its inputs (C0.1);
- the native capture covers every executed variant and verifies byte for byte
  from two shardings (C0.2);
- one complete Rust run is recorded with every row in exactly one category, and
  the comparison report attributes every difference to a bucket and a
  checkpoint (C0.3, C0.4);
- harness validation passes, including protocol and request identity, with no
  unresolved adapter failures in the recorded run (C0.3);
- the blockers register names each cross-phase dependency with its variant
  count and owner (C0.5);
- the `checker` producer records the result and `P2A-C0` passes on it (C0.6);
- the three sub-tests Phase 2 owns have a Rust definition (C0.7);
- the cost of one full run is measured and the checkpoint-exit run policy is
  set from it (C0.8).

Passing parity is not required. Production panics, deadlines and named
unsupported operations are categorized gaps. Adapter failures, malformed
responses, broken request identity and other harness defects block C0; they
cannot satisfy preparation by being counted as compiler failures. An ambiguous
failure must be attributed before the harness can be declared valid.

## 2. Starting point

Everything C0 consumes exists; C0 joins it and extends it, never re-derives it.

| Asset | Where | What C0 takes from it |
| --- | --- | --- |
| S07 source inventory | `data/s07/subset.json` (15,206 effective variants with options, configured names, harness options, exclusion reasons, reference baselines by kind and git blob, loading-request digests, dependency closures) | the variant identity, its options and baseline set, its family tags |
| Phase 1 syntax schedule | `data/phase1/syntax-schedule.json` (one row per variant with the native selection outcome: `runs`, `option_guard_skip`, `filename_skip`; boundaries `options_rejected`, `content_mapper`) | the executed set and the informational rows |
| S08 acceptance partition | `data/s07/e2-acceptance.json` (9,369 acceptance, 1,359 informational) | the regression subset, kept as its own metric |
| Native loading requests | `target/s07-subset/review/loading-requests.candidate.json`, rebuilt by `scripts/s07.py observe-subset` and `s07_subset.prepare` from the pinned preprocessing | the exact program inputs both sides load |
| S08 native oracle | `tools/s08/oracle/*.go` overlays (access-only adapters over `harnessutil`, `tsbaseline.DoErrorBaseline`, `DoTypeAndSymbolBaseline`, the acceptance policy, the public `TypeToString` observer) driven by `scripts/s08_baselines.py` (`--walker-inputs --error-inputs --public-type-strings`, `--review-capture`) | the baseline writers and the query schedule, extended to the full domain |
| S08 Rust corpus runner | `scripts/s08_p5_corpus.py` building the `p5_inventory` example, resumable, replayable, 60 s per case | the Rust execution protocol |
| S08 E2 contract | `scripts/s08_e2.py` and `s08_e2_contract.py`: preflight of request digests, immutable captures, replay before recording, strict outcome vocabulary, divergence ledger (`data/divergences.toml`, ADR 0004) | the grading rules, reused as a library, not edited |
| Phase 1 producer pattern | `scripts/phase1_*.py`, `sprints/P1A.toml`, `status/runs.toml` | the namespace layout: new scripts over the existing manifests, metrics named per producer, sprint items with `done_when` |

## 3. The denominator

From the syntax schedule and the S07 inventory, without a native run:

| Rows | Variants |
| --- | ---: |
| Effective variants at the pin | 15,206 |
| Executed by the pinned runner (`runs`) | 13,434 |
| of which S08 acceptance (regression subset) | 9,369 |
| of which newly included | 4,050 |
| of which content-mapper | 15 |
| Skipped by the option guard / by file name (informational) | 1,720 / 52 |
| of the 52 filename skips, also marked rejected options (informational) | 39 |

Baseline applicability among the executed 13,434, from the inventory's
reference set: 12,753 `.types` and 12,753 `.symbols` references, 7,301
`.errors.txt`; 464 variants carry none of the three (413 of them emitted-output
only), where a missing `.errors.txt` asserts zero diagnostics and the 679
`NoTypesAndSymbols` variants disable the type and symbol walk with an explicit
disabled outcome, as in S08. Executed variants with declaration-emitting options
number 1,756; the exact native declaration-diagnostic requests are counted by
the capture (C0.2), not inferred from option keys.

Checkpoint assignment of the 4,065 non-regression rows by the latest checkpoint
they need: 926 to C4 (JSX or decorators, whatever else they use), 2,954 to C2
(explicit type parameters, type arguments, conditional, mapped, indexed-access,
infer, template-literal or import types), 185 to C1 or C3 (emitted-output-only
or content-mapper rows with no such family; C0.1 tags them exactly).
These are completion owners, not predetermined execution outcomes. Working
domains remain matches even when other operations in their family are missing.

The informational rows are listed with their native reason and never enter a
ratio. The S12 matrix requires that listing; it does not require an
observation for options Corsa does not implement.

## 4. Work items

Each item names what exists, what to build, its artifact and its executable
exit check. Producers run only committed inventories; scratch output goes under
`target/phase2/`.

### C0.1 Freeze the inventory

- Exists: the three manifests of section 2 and their digests.
- Build: `scripts/phase2_inventory.py freeze --output data/phase2/inventory.json`
  and `check`. One row per effective variant: `id`, `configured_name`, primary
  file and suite, ordered effective options and root keys, duplicate-file
  precedence as the native harness resolves it, `loading_request_sha256`,
  native selection and boundary, `tier` (`executed`, `informational` with
  reason), harness options (`NoTypesAndSymbols`, pretty, suggestions), the
  reference baselines present by kind with git blobs, the emitted-only and
  content-mapper flags, family tags from the S07 rule, the checkpoint
  assignment, and the S08 tier. The document records the digests of its three
  inputs; `check` rebuilds it and requires byte identity.
- Exit: `check` passes; the counts of section 3 are the document's own counts,
  asserted by `scripts/tests/test_phase2_inventory.py`.

### C0.2 Capture the native contract

- Exists: the S08 oracle and `s08_baselines.py`, which capture errors (pre- and
  post-emit structured sets and the rendered bytes), types, symbols and the
  public `TypeToString` schedule for the frozen 10,728.
- Build: `scripts/phase2_native.py capture --output DIR`, selecting every
  executed variant of the inventory (not the informational rows), adding three
  observations the S08 protocol lacks: the union-ordering check over every
  checker of the program (the pinned `verifyUnionOrdering`), the parent-pointer
  walk (`verifyParentPointers`), and the module-resolution trace (`.trace.json`
  content for the variants with `traceResolution`). Each row records its stage
  outcomes (`native_parse` … `native_type_symbol`, plus `native_ordering`,
  `native_parents`, `native_trace`), the declaration-diagnostic request when
  `GetEmitDeclarations()` is true, and for the 15 content-mapper variants the
  native result as observed. The capture runs in upstream's default
  single-threaded mode; the concurrent mode is C6's capture, not C0's.
  `verify` repeats the capture from a second sharding and requires identical
  digests; `review` compares each native baseline output with the committed
  reference file (after the runner's `DiffFixupOld` normalization) and lists
  every disagreement, expected to be none.
  Compare the full structured native pre/post-emit diagnostic sets for every
  executed variant, including codes, spans, arguments, chains and related
  information. Equal counts are insufficient. Record differences separately
  from oracle failures and attribute any required emit-side operation in C0.5;
  the absence of the pin's count-mismatch diagnostic cannot waive this check.
- Artifact: `data/phase2/native-provenance.json` (request digests, stage
  outcome counts, per-row digests, oracle binary and source identities, host
  and toolchain); the raw outputs stay in the capture directory, referenced by
  digest, as S08 does.
- Exit: `verify` and `review` pass; the declaration-request count and the
  trace-row count are recorded.

### C0.3 Run the Rust corpus

- Exists: `p5_inventory` and its runner, which already execute the frozen
  requests and refuse unknown operations by name (`Error::Unsupported`).
- Build: `scripts/phase2_corpus.py run --native DIR --output DIR`, over the same
  requests, with the S08 protocol (independent processes, deadline, raw output,
  atomic completion, `--resume`), adding the three observations of C0.2 on the
  Rust side (C0.7 defines them). Every row lands in exactly one category per
  domain: `match`, `different`, `failed` (production panic or deadline),
  `unsupported` (a named production-operation refusal), `disabled` (native
  explicitly disables the domain), `unexecuted`. Preserve harness failures as
  separate execution errors with raw output; they invalidate preparation
  instead of becoming any of these compiler outcomes.
- Harness validation: require complete ordered request/response identity and
  authenticated inputs on run, resume and replay. Tests reject duplicate,
  missing, extra or reordered rows, unknown statuses, stale inputs and forged
  request digests. Exercise the reused adapter on a small named set of existing
  S08 matching controls and verify their outputs. Inject an adapter failure to
  prove it cannot become a production failure or a passing C0 metric; also
  verify that a genuine production failure remains a recorded gap.
- Exit: the run completes for all 13,434 rows; the per-row deadline count and
  the total wall time are recorded (C0.8), harness validation passes and no
  unresolved harness failure remains.

### C0.4 Compare and bucket

- Build: `scripts/phase2_compare.py report --native DIR --rust DIR`. Domains:
  errors (byte-exact rendering plus the structured pre/post sets), types,
  symbols, public display, the three sub-tests. Buckets: family tag,
  configuration (module, target, jsx mode, strictness), first differing
  diagnostic code, checker area (from the Go function the S08 obligation
  records attribute to the row's syntax), and checkpoint. Each bucket names a
  representative reproduction (the smallest row by source bytes). The report
  compares row observations with the previous capture when one exists, so an
  internal regression inside a still-different row is visible, as the Phase 2
  plan requires.
- Artifact: `data/phase2/first-comparison.json` (summary and bucket counts, no
  raw observations) and the C0 record, `docs/PHASE2-C0.md`, written at C0's
  exit with the attribution.
- Exit: every non-matching row belongs to one bucket and one checkpoint. Every
  unsupported observation identifies the missing production operation reported
  by that execution. Family tags assign an owner but never manufacture an
  unsupported result or replace an observed match or difference.

### C0.5 Name the blockers

- Build: `data/phase2/blockers.json`, one entry per dependency with the
  variant ids and domains it affects, its owner phase/checkpoint, the missing
  operation and the raw observation establishing the gap. Audit declaration
  diagnostics against the exact native requests and pre/post sets of C0.2:
  keep working paths, and record only missing transform or emit-resolver work
  as a blocker under its Phase 3 or Phase 2 owner. A declaration option or an
  equal diagnostic count is not sufficient evidence. Audit content-mapper
  execution (Phase 5, 15 variants: 2 errors, 10 types, 10 symbols baselines)
  the same way. The 926 JSX/decorator variants belong to C4, but their outcomes
  come from execution, including any already-matching domains. Record the
  module-resolution ownership decision separately (C0.7). Phase 1 contracts
  consumed by C0 (loader with project references, options, module resolution,
  binder) have no open item; the register says so with the PR that closed each.
- Exit: every observation withheld by a known missing operation has a matching
  blocker entry, and every entry links the execution evidence identifying that
  dependency. A blocker may explain a difference but never hides or reclassifies
  it. An unimplemented harness observation blocks C0 instead of being assigned
  to a production checkpoint.

### C0.6 Namespace, producers and sprints

- Build:
  - `data/phase2/` for inventories and evidence, `scripts/phase2_*.py`, tests
    under `scripts/tests/test_phase2_*.py`; `tools/s08/oracle` and `p5_inventory`
    may be extended in place. E2 fingerprints all files under the oracle and
    all Rust sources through its inherited closure, so adding files also
    changes its inputs. Preserve E2's record as historical evidence and derive
    current regression results from the Phase 2 capture; do not re-record E2
    merely to refresh its fingerprint or weaken its freshness checks.
  - `status/runs.toml` `[checker]`: `python3 scripts/phase2_producers.py checker`,
    inputs the three manifests and the divergence ledger, sources the oracle,
    the corpus crates and the Phase 2 scripts. Metrics: `run.checker.inventory_frozen`,
    `run.checker.native_verified`, `run.checker.harness_valid` (the C0.3 checks
    pass and the run has no unresolved harness errors),
    `run.checker.result_recorded` (a complete authenticated categorized run
    exists and replays), `run.checker.errors_parity`,
    `types_parity`, `symbols_parity`, `display_parity` (each a ratio over the
    executed denominator, failures and unsupported rows counted against),
    `run.checker.ordering`, `run.checker.parent_pointers`,
    `run.checker.trace_parity`, `run.checker.unsupported_required`,
    `run.checker.blockers_named`, and `run.checker.regression_parity` over the
    S08 9,369 subset. The E2 producer and its metrics are untouched.
  - `sprints/P2A.toml` with `P2A-C0` (`done_when`: `inventory_frozen`,
    `native_verified`, `harness_valid`, `result_recorded`, `blockers_named`);
    `sprints/P2B.toml` holding the checkpoint items C1 to C7 as required items
    whose `done_when` metrics their own plans wire, so the sprint stays open
    by construction.
  - No new experiment and no threshold; PLAN's Phase 2 gate binds the parity
    metrics at C7.
- Exit: `cargo xtask validate` accepts the new producer and sprints;
  `cargo xtask check P2A` reports `P2A-C0` on the recorded evidence.

### C0.7 Define the sub-tests on the Rust side

- Union ordering: extend the S08 comparator adapter from its frozen fixtures to
  every union of every checker of a program, reversing and shuffling with the
  pinned seed as `verifyUnionOrdering` does; in C0 over the single checker the
  program has, in C6 over each checker.
- Parent pointers: after checking, skip only files selected by the equivalent
  of Go's `Program.IsSourceFileDefaultLibrary`; user declaration files stay in
  the walk. Set the traversal parent to each source-file root, then visit its
  children through the generated child visitors as Go's `ForEachChild` does.
  Each visited descendant, including synthesized descendants reachable by that
  visitor, must have the recorded parent id of the traversal parent. The root
  itself is not checked for a parent, and detached display/builder trees are
  outside this walk. Test a valid parentless root, a user declaration file, a
  reachable synthesized child, and a missing or wrong descendant parent. This
  is a structural check on the index-based AST, not a pointer comparison.
- Module resolution trace: the `.trace.json` baselines (148) render the
  loader's resolution trace, which Phase 1 already ports and compares per
  request. Proposed owner: the Phase 1 program surface, accepted through C0's
  runner as a Phase 2 sub-test row because the compiler runner produces it.
  Owner decision (section 9).
- Exit: each definition has a direct test on a constructed program and runs in
  C0.3.

### C0.8 Measure the cost and set the run policy

- Record the wall time, process count and disk use of C0.2 and C0.3 on the
  designated host, per row and total, in `PHASE2-C0.md`. The Phase 2 plan
  defaults to a full run at every checkpoint exit; this measurement either
  confirms that or argues it down to named families plus the S08 regression
  subset, and the decision is written next to the numbers.

## 5. Dependencies and owners

| Dependency | Owner | State for C0 |
| --- | --- | --- |
| Loader, options, module resolution, binder over every executed variant, including project references | Phase 1 (PR #55 ported reference loading; PR #56 closes the last operations) | ready; the Phase 1 syntax schedule already loads all 15,152 loadable rows |
| S08 oracle, corpus runner, E2 contract | S08, kept as history | ready; extended in place |
| Declaration transform for declaration diagnostics | Phase 3 | working paths retained; missing operations identified per domain by C0.2–C0.5 |
| Content-mapper execution | Phase 5 | blocker, 15 variants |
| Concurrent execution capture and per-checker ordering | C6 | not a C0 input |
| Owner decisions of section 9 | owner | before C0.6 and C0.7 |

## 6. Delivery order

1. C0.1 (Python only, a day): the inventory and its tests, reviewed before any
   run.
2. C0.7 definitions and C0.2 oracle extension in parallel; then the native
   capture, verified from two shardings.
3. C0.3 Rust run, resumable; C0.8 timing comes out of it.
4. C0.4 report and C0.5 register, then the C0 record.
5. C0.6 wiring last, recorded through `cargo xtask run checker`, which is the
   owner's to run on the designated host.

Nothing in C0 changes checker semantics. A defect found while defining a
sub-test in production code is filed against its checkpoint, not fixed in C0.
An adapter or comparator defect is fixed in C0 before preparation can pass.

## 7. Executable exit checks

```sh
python3 scripts/phase2_inventory.py check                                   # C0.1: byte-identical rebuild, counts of section 3
python3 scripts/phase2_native.py verify --capture target/phase2/native       # C0.2: second sharding identical
python3 scripts/phase2_native.py review --capture target/phase2/native       # C0.2: native equals committed references
python3 scripts/phase2_corpus.py replay --output target/phase2/rust          # C0.3: categories recomputed from raw outputs
python3 scripts/phase2_compare.py report --native target/phase2/native --rust target/phase2/rust
python3 -m pytest scripts/tests/test_phase2_inventory.py scripts/tests/test_phase2_compare.py scripts/tests/test_phase2_corpus.py -q
cargo xtask validate && cargo xtask check P2A
```

## 8. Evidence reuse rules

- The S08 E2 record and its frozen inventory stay intact as history. Shared
  runner and production changes can stale E2, including newly added files in
  fingerprinted directories. Its existing freshness rules remain unchanged;
  `run.checker.regression_parity` re-derives the current 9,369 result from the
  Phase 2 capture instead of re-recording E2 for bookkeeping.
- A native capture is reused only when `verify` accepts it against the current
  oracle sources and request digests; a Rust capture only when `replay` accepts
  it against the captured executable identity. Source changes stale both, as
  in S08; a stale capture is a gap, never a failure of the harness.
- Informational rows never enter a ratio; unsupported required rows are
  failures even when other domains match.
- The divergence ledger (ADR 0004) is the only path to accepting a difference,
  per variant and per metric, with exact hashes.

## 9. Owner decisions before C0 starts

1. Module-resolution sub-test owner: Phase 1 program surface accepted through
   C0's runner (proposed), or a Phase 2 metric in its own right.
2. Public display protocol: keep S08's default `TypeToString` on every type the
   walker returns (proposed), or add the hover and `typeToTypeNode` queries in C5.
3. Row budget: keep the 60 s per-case deadline and one capture per checkpoint
   exit (proposed), or a smaller sample for intermediate runs, decided on C0.8's
   numbers.
