# Phase 3 T0 record: the emit acceptance contract

T0 of the [Phase 3 plan](PHASE3-plan.md) (section 4), implemented on the
Phase 3 branches from `main` `1a455cd2`; this record is written on
`phase3-b13-harness`. Upstream remains Corsa
`1f70213d4922b434345f639b441681e470c7cfc1`. T0 prepares; it claims no Rust
emit parity. Its one production-facing addition is harness code: the reprint
witness and the `unsupported` emit domains of `phase3_emit`.

## Exit

| Exit condition | State |
| --- | --- |
| `run.emit.inventory_frozen` | true: `phase3_inventory.py check` rebuilds `data/phase3/inventory.json` byte for byte |
| `run.emit.native_verified` | true: single-mode capture, 13,432 rows, reproduced from a second sharding, every composed baseline equals its reference, review equals `data/phase3/native-provenance-single.json` |
| `run.emit.native_verified_concurrent` | true: the same for the concurrent mode and `native-provenance-concurrent.json` |
| `run.emit.transpile_native_verified` | true: `data/phase3/transpile-native.json`, 28 configurations composing the 41 reference baselines |
| `run.emit.harness_valid` | true on `target/phase3/rust-single` for the sources of this branch (see "Staleness" below) |
| `run.emit.harness_valid_concurrent` | true on `target/phase3/rust-concurrent` |
| `run.emit.result_recorded` | true: the comparison's acceptance summary equals `data/phase3/first-comparison.json` |
| `run.emit.blockers_named` | true: `data/phase3/blockers.json` equals the register rebuilt from the evidence |
| Every row in exactly one category per domain | yes: 13,432 rows × 5 domains, no blank and no harness error |
| `run.emit.mode_parity` | true: 0 outcome differences between the two modes' runs |
| `P3A-T0` passes on a recorded `emit` run | **not done**: recording (`cargo xtask run emit`) is the owner's |
| The ledger move of `compiler/emitter.go` and `compiler/emitHost.go` (decision 1) | **deferred** to the T8 green-up (deviation 1) |
| Cost measured and run policy set | below |

Run directly, `python3 scripts/phase3_producers.py emit` reports
`inventory_frozen`, `native_verified`, `native_verified_concurrent`,
`transpile_native_verified`, `harness_valid`, `harness_valid_concurrent`,
`result_recorded`, `blockers_named` and `mode_parity` true, `reprint_parity`
1.0 and `unsupported_required` 13,432.

**Staleness.** `harness_valid` binds the Rust capture to the sources it was
built from (`crates/**` without the test-only suites, `tools/phase3/harness`,
the S08 executor, the build configuration). This branch's run was built from
this branch's sources. Every Phase 3 unit that changes `crates/**` stales it,
so the integrated branch needs one fresh full run, comparison and register
(the reproduction commands below) before the owner records `emit`. Until then
the producer reports `harness_valid` false and withholds the acceptance
metrics; that is the expected mid-phase state (plan decision 12).

## Deviations

1. **The ledger move is deferred to the T8 green-up.** Moving
   `compiler/emitter.go` and `compiler/emitHost.go` to Phase 3 in
   `PORTS.toml` changes `data/upstream.json`, which every recorded Phase 2
   capture binds (and which the Phase 3 native captures bind through
   `phase3_native.SCRIPT_INPUTS`). Doing it at T0 would stale the recorded
   Phase 2 captures without anything to show for it; T8 re-records `checker`
   and `emit` together anyway (decision 12), so the move lands there, with the
   S07 and trace re-freezes it requires.
2. **Native rows keep per-file output digests; texts only with `--texts`.**
   The plan asked for every emitted output "by name and text". The native
   rows record each emitted file's name, digest and size (`outputs.js`,
   `dts`, `maps`) and keep the three rendered baselines' texts, which are the
   authority (decision 5). `phase3_native.py capture --texts` keeps the
   per-file texts and the reprint texts for debugging; the recorded captures
   were taken without it.

## The denominator

`data/phase3/inventory.json` (`scripts/phase3_inventory.py`) has one row per
executed variant of the Phase 2 denominator, joined from
`data/phase2/inventory.json` (bound by the digest of its executed rows),
`data/s07/subset.json` and the pinned runner's `skippedEmitTests`, never from a
native or Rust run.

| Rows | Variants |
| --- | ---: |
| **Executed** (the Phase 3 denominator) | **13,432** |
| `output` runs and owes a `.js` reference | 12,174 |
| `output` runs and owes `<no content>` | 1,189 (1,155 `noEmit`, 34 other) |
| `output` disabled with the pin's reason | 69 |
| of which no input file other than declaration files | 61 |
| of which `skippedEmitTests` (four reasons, eight tests) | 8 |
| `.js.map` references | 150 |
| `.sourcemap.txt` references | 157 |
| Rows without a `.js` reference (graded `<no content>` or disabled) | 1,258 |
| The 300-variant sample (Phase 2's, inherited) | 300 |

Each row carries the options that select transforms (`emit_options`), the
reference blobs by kind, `has_non_dts_files` as the runner computes it, and
what each sub-test owes. The plan's "42 others" without a `.js` reference are
the inventory's 34 `other` plus the 8 skipped tests, which the inventory
classes as `disabled`.

The transpile section lists the runner's 25 test files and 41 reference
baselines; the runner's vary-by expands them into 28 configurations.

## The native contract

`scripts/phase3_native.py` builds one oracle test binary from an access-only
overlay of the pinned runner (`tools/phase3/oracle/emit_test.go` as
`testrunner/phase3_emit_test.go`, the baseline observer, and S08's harness
hooks). The overlay replaces no pinned algorithm: `baseline.Run` hands the
observer the text it would compare.

Per row it records the baseline writers' inputs (the header and the runner's
three file groups, the only native values a Rust request carries), the emit
result (`EmittedFiles`, `EmitSkipped`, the emit diagnostics), each emitted
file's name, digest and size, the rendered `output`, `sourcemap` and
`sourcemap record` baselines, the pre- and post-emit diagnostic counts, and
the reprint witness: the pinned `printer.EmitSourceFile` over every
non-library source file, with and without comments, with the file's
`ScriptKind` and `LanguageVariant`.

Facts from `data/phase3/native-provenance-single.json` and
`native-provenance-concurrent.json`:

- Both modes executed all 13,432 rows with no native failure and produced the
  same observation digest (`e99a8dcc…`); the single mode ran with
  `single_threaded: true`, the concurrent one with
  `TS_TEST_PROGRAM_SINGLE_THREADED=false`.
- Each capture ran on 8 contiguous shards and was reproduced by a 5-shard
  interleaved capture, identical on every row digest.
- Review: every composed baseline equals its committed reference: `output`
  12,174 content and 1,189 `<no content>`, 69 disabled; `sourcemap` 150
  content and 1 `<no content>` (13,281 not baselined); `sourcemap_record` 157
  content and 13,275 `<no content>`. Zero reference disagreements, zero
  inventory disagreements, zero failed sub-tests.
- 100 rows report `EmitSkipped`. One row's pre- and post-emit diagnostic
  counts differ
  (`conformance/dynamicImport/importCallExpressionNoModuleKindSpecified.ts`);
  the plan expected three from Phase 2's structured comparison, where the
  sets differ but the counts do not. T8's `emit_diagnostics` domain compares
  the post-emit program's diagnostics and this count.
- Reprint: 18,517 non-library files; the pin prints 18,488 in both comment
  modes and panics on 29 (`unhandled statement: KindJSImportDeclaration`),
  which is the contract for those files. Script kinds over the files: TS
  16,387, JS 1,515, TSX 535, JSON 67, JSX 13; every file but the TS ones has
  the JSX language variant.
- Toolchain go1.27.1 darwin/arm64 (`macOS-26.6.1-arm64`), oracle binary
  `bb90f300…`.

`data/phase3/transpile-native.json`: the pinned transpile runner's 28
configurations, each unit's `TranspileModule` and `TranspileDeclaration`
result, and the composed baselines, which equal the 41 references; no
reference lacks a run and no run lacks a reference.

## The Rust harness

`crates/tsr_compiler/examples/phase3_emit.rs` is one corpus row: it loads the
variant's program as the pin's harness loads its post-emit program
(`executor::load_fresh_checked`, with the configuration's content mappers) in
the request's mode, records the reprint witness
(`tools/phase3/harness/reprint.rs`) and reports the four emit domains as the
named refusal `Program.Emit is Phase 3 T8` (`output` is `disabled` with the
request's reason where the pin disables it). Each reprinted file records its
name, source digest, `script_kind` and `language_variant` (the pin's
integers) and the two printings: `printed` (digest and size), `refused` (the
printer's error) or `failed` (a Rust panic with its location).

`scripts/phase3_corpus.py` runs it over the S08 P4/P5 protocol: one process
per variant, a deadline, immutable completion records bound to the request,
capture and executable digests, resume and replay. Requests carry native
inputs only: the frozen loading request (checked against the Phase 2
inventory digest), the native baseline inputs and what the inventory says the
`output` sub-test is. A capture binds the verified native capture of its mode
(report and observation digests), the request digest, the build's source
fingerprint and the executable. Harness defects (protocol violations, adapter
panics, adapter-literal errors) invalidate a run; production panics,
deadlines and named refusals are measured outcomes.

## The comparison

`scripts/phase3_compare.py report` puts every domain of every row in one
category (`match`, `different`, `failed`, `unsupported`, `disabled`,
`unexecuted`):

- `reprint` matches when both sides list the same files (name, source digest,
  order), each file has the pin's script kind and language variant, and each
  printing agrees: equal digest and size, or a Rust refusal whose reason is
  the pinned panic's message. A refusal where the pin printed is `different`
  (`rust_refused`), a refusal with another reason than the pin's panic is
  `different` (`refusal_reason`), a print where the pin panics is `different`
  (`native_panic`), a script kind or language variant other than the pin's is
  `different` (`file_kind`).
- `output`, `sourcemap` and `sourcemap_record` compare the rendered baseline
  (state, name and text) once Rust emits; `disabled` carries the pin's reason.
- `emit_diagnostics` compares the emit diagnostics and `EmitSkipped`.

`--record` writes the acceptance summary (the report without its rows) to
`data/phase3/first-comparison.json`; it refuses a partial, concurrent,
harness-invalid or source-stale run. `modes` reports each mode's run against
its own mode's native capture and counts per-row, per-domain outcome
differences and differing Rust rows (the mode field aside).

**The first comparison** (single mode, full denominator):

| Domain | match | different | failed | unsupported | disabled |
| --- | ---: | ---: | ---: | ---: | ---: |
| reprint | 13,432 | 0 | 0 | 0 | 0 |
| output | 0 | 0 | 0 | 13,363 | 69 |
| sourcemap | 0 | 0 | 0 | 13,432 | 0 |
| sourcemap_record | 0 | 0 | 0 | 13,432 | 0 |
| emit_diagnostics | 0 | 0 | 0 | 13,432 | 0 |

The reprint witness matches on every row: 18,488 files printed byte for byte
with and without comments, and the 29 refusals each carry the pin's panic
message. 0 harness errors, 0 production failures. `reprint_parity` is 1.0,
`unsupported_required` 13,432 (every row, until T8 emits).

**Both modes.** The concurrent run (`target/phase3/rust-concurrent`) against
the concurrent native capture: 13,432 rows completed,
0 harness errors, the same categories in every domain as the single run.
`phase3_compare.py modes` reports 0 outcome differences between the two runs
and 0 differing Rust rows (the mode field aside); both runs compare with the
same native observation (`e99a8dcc…`), since the two native captures are
identical.

## Blockers

`scripts/phase3_blockers.py build|check` derives `data/phase3/blockers.json`
from the single-mode comparison: one entry per bucket of rows that cannot pass
yet (category and cause), with its row count, the digest of its sorted row
identities, the withheld domains with counts, five examples with their capture
case, and the owning checkpoint: a refusal that names its checkpoint is that
checkpoint's, a checker unsupported-operation refusal is a cross-phase resolver
entry owned by Phase 2 maintenance, and any other cause is its domains'
(`reprint` T1, the source-map sub-tests T2, `output` and `emit_diagnostics`
T8). `check` rebuilds it and requires equality.

Today it has one entry:

| Id | Kind | Cause | Owner | Variants | Domains |
| --- | --- | --- | --- | ---: | --- |
| B01 | unsupported | `Program.Emit is Phase 3 T8` | T8 | 13,432 | emit diagnostics 13,432, output 13,363, sourcemap 13,432, sourcemap record 13,432 |

The plan's two cross-phase dependencies, from the evidence:

- **The bounded work group** (Phase 2 C6, amended by the #73 review): landed.
  `crates/tsr_core/src/workgroup.rs` carries `worker_bound` and the contract
  test `a_large_parallel_group_runs_on_bounded_workers`, so T8's parallel emit
  has its dependency; no entry.
- **Resolver queries the transforms need**: not observed. No row refuses with
  the checker's unsupported-operation text; the transforms do not run in the
  corpus until T8, so this stays a rule the register applies, not an entry.

## Producer and sprints

`scripts/phase3_producers.py emit` (`status/runs.toml` `[emit]`) replays the
recorded captures and never runs a corpus:

- `inventory_frozen`, `native_verified`, `native_verified_concurrent`,
  `transpile_native_verified` (unchanged from the native commit);
- `harness_valid`, `harness_valid_concurrent`: that mode's Rust capture is
  complete (not a sample, every executed variant observed), current with its
  sources and with the requests the inventory and the native capture give now,
  bound to that mode's verified native capture (report and observation
  digests), and without a harness error in its replay or comparison;
- `result_recorded`: the comparison's acceptance summary equals
  `data/phase3/first-comparison.json`;
- `reprint_parity` (matched reprint rows over the executed denominator) and
  `unsupported_required` (rows with an unsupported domain), emitted only over
  a harness-valid single-mode run;
- `mode_parity`: both native captures verified, both Rust captures
  harness-valid, zero outcome differences between the two runs, and the
  single run the one compared;
- `blockers_named`: the committed register equals the rebuilt one and names
  every row that cannot pass with an owner.

`status/runs.toml` `[emit]` now lists `data/phase3/first-comparison.json` and
`data/phase3/blockers.json` as inputs and the Rust capture's source closure
(`crates/**` without `crates/*/tests/**`, the S08 executor, the build
configuration) as sources. `sprints/P3A.toml` (`P3A-T0`) is unchanged: its six
metrics are the plan's T0 exit. The parity metrics `output_parity`,
`declaration_parity`, `sourcemap_parity`, `sourcemap_record_parity`,
`emit_diagnostics_parity`, `transpile_parity`, `residuals`, `dispositions`,
`evidence_current`, `report` and `tN_complete` are not emitted yet; they come
with T1 to T8.

`scripts/tests/test_phase3_{inventory,native,corpus,producers}.py` cover the
inventory, the native validation, the row contract, the comparison (including
the mutation check that a refusal with another reason than the pinned panic
does not match, and the file-kind comparison), whole-capture comparison and
recording, harness validity (a tampered, partial, incomplete or stale capture
is rejected or invalid; a capture against another native capture is
rejected), the two-mode comparison and the register (a new bucket makes the
committed register incomplete; a resolver refusal becomes a cross-phase
entry). They use the four real rows of `scripts/tests/fixtures/phase3/` and
need no `target/` capture.

## What is not done

- No Rust emit exists: every emit domain is `unsupported` until T8, so
  `output`, `sourcemap`, `sourcemap_record` and `emit_diagnostics` compare no
  text yet; the comparison's attribution of a difference to a transform, the
  printer, the generator or the declaration transform (plan T0, "Buckets")
  has nothing to attribute and is not built beyond the reprint buckets.
- There is no Rust transpile and no `transpile` domain in the comparison; the
  native transpile observation is recorded and verified only.
- The `noCheck` repeat, the declaration re-compilation and the baseline
  writers in harness code are T8's.
- The ledger move is deferred (deviation 1).
- The `emit` run is not recorded and `P3A-T0` not checked: both are the
  owner's (`cargo xtask run emit`, `cargo xtask check P3A`). `STATUS.md`,
  `status.json` and `docs/status.html` are not regenerated.

## Cost and run policy

Measured on the development host (Apple M-series, 18 cores, go1.27.1).

| Step | Wall | Processes | Disk |
| --- | ---: | ---: | ---: |
| Native capture, 13,432 rows, one mode | about 30 s (8 shards, 5 jobs, oracle reused) | 8 test processes | 507 MB with its verification |
| Native second sharding | about 35 s (5 shards) | 5 | (included) |
| Rust run, 13,432 rows, one mode | 180 s (16 jobs); 200 s with replay; concurrent mode 194 s and 218 s | 13,432 | 611 MB (concurrent 604 MB) |
| Harness build (`phase3_emit`, opt-level 1) | about 70 s cold, cached afterwards | 1 | the cargo target |
| Comparison (`report --record`) | 10 s | 1 | the row report |
| Register (`phase3_blockers.py build`) | 6 s | 1 | none |
| `emit` producer (replays both modes) | 51 s | 1 | none |

A full single-mode contract takes about four minutes on this host, well under
the plan's "under three times" Phase 2's five; the emit work T8 adds will
raise it. The policy stands: intermediate checkpoints run the recorded
300-variant sample (`--sample`) plus targeted `--case` rows; full runs are
T0's first run and each checkpoint's exit, in both modes from T8.

## Reproduction

```sh
export PATH="/Users/cristian/.local/share/mise/installs/go/1.27.1/bin:$PATH"
python3 scripts/phase3_inventory.py check
python3 scripts/phase3_native.py capture --output target/phase3/native-single --mode single
python3 scripts/phase3_native.py verify --capture target/phase3/native-single
python3 scripts/phase3_native.py review --capture target/phase3/native-single --record
python3 scripts/phase3_native.py capture --output target/phase3/native-concurrent --mode concurrent \
  --oracle-from target/phase3/native-single
python3 scripts/phase3_native.py verify --capture target/phase3/native-concurrent
python3 scripts/phase3_native.py review --capture target/phase3/native-concurrent --record
python3 scripts/phase3_native.py transpile --output target/phase3/transpile-native \
  --oracle-from target/phase3/native-single --record
python3 scripts/phase3_corpus.py run --native target/phase3/native-single --output target/phase3/rust-single
python3 scripts/phase3_compare.py report --native target/phase3/native-single \
  --rust target/phase3/rust-single --record
python3 scripts/phase3_blockers.py build --record
python3 scripts/phase3_corpus.py run --native target/phase3/native-concurrent \
  --output target/phase3/rust-concurrent --mode concurrent
python3 scripts/phase3_compare.py modes --single target/phase3/rust-single \
  --concurrent target/phase3/rust-concurrent
python3 scripts/phase3_blockers.py check
python3 scripts/phase3_producers.py emit
python3 -m pytest scripts/tests -q -k phase3
```

Intermediate runs:

```sh
python3 scripts/phase3_corpus.py run --native target/phase3/native-single \
  --output target/phase3/rust-sample --sample --case 'compiler/2dArrays.ts#configuration=0'
python3 scripts/phase3_compare.py report --native target/phase3/native-single --rust target/phase3/rust-sample
```

A sample report says `partial: true`; it cannot be recorded, cannot replace the
register and does not make the producer's metrics.
