# Phase 4 X0 record: the command-line acceptance contract

X0 of the [Phase 4 plan](PHASE4-plan.md) (section 4), implemented on branch
`phase4-x0` from `main` `99654b22`. Upstream remains Corsa
`1f70213d4922b434345f639b441681e470c7cfc1`. X0 prepares; it claims no Rust
command-line parity. Its Rust code is repository-only harness code
(`tools/phase4/tsctests`); no `crates/**` file changed.

## Exit

| Exit condition | State |
| --- | --- |
| `run.tsc.inventory_frozen` | true: `phase4_scenarios.py check` passes on `data/phase4/scenarios.json.gz` (516 scenarios, 1,255 edits, 654 recorded file operations), current with the pin, the recorder's patch digests and the ledger hashes of the patched files |
| `run.tsc.inventory_verified` | true: the producer re-runs the three checks; reproduce 516/516 with the orphan as recorded, a second recording identical in its JSON and gzip bytes, replay 516/516 through the pin's unpatched runner |
| `run.tsc.harness_valid` | true on `target/phase4/rust` for the sources of this branch (see "Staleness" below) |
| `run.tsc.result_recorded` | true: the comparison's acceptance summary equals `data/phase4/first-comparison.json` |
| `run.tsc.blockers_named` | true: `data/phase4/blockers.json` equals the register rebuilt from the comparison |
| Every row in exactly one category | yes: 516 rows, 516 `unsupported`, no blank row and no harness error |
| `P4A-X0` passes on a recorded `tsc` run | **not done**: recording (`cargo xtask run tsc`) is the owner's |
| The ledger moves of decision 4 | **deferred** (see "What is not done") |
| Cost measured and run policy set | below |

Run directly, `python3 scripts/phase4_producers.py tsc` prints:

```text
{"metrics": {"audit": true, "baseline_parity": 0.0, "blockers_named": true, "harness_valid": true, "incremental_correctness": 0.0, "inventory_frozen": true, "inventory_identical": true, "inventory_replayed": true, "inventory_reproduced": true, "inventory_verified": true, "result_recorded": true, "unit_tests": true, "unsupported_required": 516}}
```

**Staleness.** `harness_valid` binds the Rust capture to the sources it was
built from (`crates/**` without the test-only suites, `tools/phase4/tsctests`,
the Phase 3 patience diff the harness includes, the build configuration) and
to the inventory's digest. Any change under `crates/**` stales it, and the
recorded comparison binds the capture's digest, so a branch that changes the
crates needs one fresh run, comparison and register (the reproduction commands
below) before the owner records `tsc`. Until then the producer reports
`harness_valid` false and withholds the acceptance metrics, the expected
mid-phase state (decision 14).

**One edit to a Phase 2 script.** Declaring `[tsc]` in `status/runs.toml`
trips Phase 2's C7 evidence check: `scripts/phase2_producers.py` lists the
later phases' producers in `LATER_PHASE_RUNS`, and `c7_evidence_current`
requires every other declared run to be a C7 prerequisite. X0 adds `"tsc"`
to that list, as Phase 3 added `"emit"` (`bbaa0b23`). The script is an input
of the recorded `checker` run, which reads stale until the green-up.

## Corrections to the plan

1. **516 scenarios, not 517.** The pinned suite renders 516 baselines: `tsc`
   217 (the plan's 218), `tsbuild` 192, `tscWatch` 42, `tsbuildWatch` 65. The
   517th committed reference, `tsc/commandLine/adds-color-when-FORCE_COLOR-is-set.js`,
   is an orphan: upstream commit `31ff97358f` ("Handle FORCE_COLOR values like
   Node") removed its scenario and left the file. The inventory lists it in
   `orphan_references`; the comparison reports it apart and never counts it.
   The `commandLine` folder has 48 scenarios (the plan's X1 list says 49; the
   14 color cases are all there). The edit steps (1,255) and the scenarios with
   edits (292) are the plan's. `P4A` and `P4B` say 516 and name the orphan.
2. **150 unit tests, not 146.** The plan's per-package counts omit
   `internal/execute/tsc`'s four tests: `emit_test.go`'s
   `TestContentMapperLoggerEnvironmentVariable` and
   `TestIncrementalDeclarationEmitTimeIsExcludedFromCheckTime`, and
   `extendedconfigcache_test.go`'s `TestExtendedConfigCacheExtendsCircularity`
   and `TestExtendedConfigCacheNullExtendsDoesNotPanic`. All four are X1's.
3. **24 direct `tsctests` functions, not 14.** `watcher_race_test.go` holds 12
   test functions that assert directly (the plan counted 2 race tests); the
   12 content-mapper lifecycle tests are as planned. The package's 76 test
   functions are 51 that produce baselines (the plan's 61), 24 direct ones and
   `TestMain`. The roster (`data/phase4/unit-tests.json`) has 174 entries: 5
   ported (the incremental package's five, already in `tsr_incremental`), 164
   pending, 5 not applicable (the four Windows `nativepath` tests and the Go
   FFI trampoline test).
4. **The audit's numbers.** The plan counted before Phase 3: 70 files (65
   source, 5 harness), 22,479 lines, 1,055 functions, 196 marked (19%): 167 in
   the compiler files, 25 in the diagnostic writer, 4 in `nativepath`. After
   decision 4's three moves (`fswatch/kqueue.go` out of scope,
   `projectreferencedtsfakinghost.go` to Phase 5, `pprof/pprof.go` to Phase 7)
   the scope is 67 files (62 source, 5 harness), 21,294 lines and 1,007
   functions: 477 mapped, 52 equivalent, 27 later (Phase 5 25, Phase 6 2), 451
   pending, 0 gap. The compiler files that stay have 181 of 252 functions
   marked (`program.go` 96 of 144, with 12 equivalent, 19 later and 17
   pending; `fileloader.go` 38 of 41); `execute/incremental` 175 of 175; the
   harness group 92 mapped and 4 equivalent. Pending by owning checkpoint: X1
   64, X2 3, X3 90, X4 198, X5 60, X6 36.
5. **`tsr_incremental` already exists.** Phase 3 ported `execute/incremental`
   whole, so X2's remaining audit work is three functions:
   `Program.GetIncludeReasons` and `Program.IsMissingPath` (the testing
   accessors the harness's program baseline reads) and
   `execute/tsc.go:performIncrementalCompilation`; and the two accessors the
   harness needs made public (below).
6. **Sixteen unmarked ports.** The audit records these as `equivalent` with
   `marker_to_add` (a one-to-one port that should carry the marker); all are
   in X1's group: `program.go`'s `Program.GetDefaultResolutionModeForFile`,
   `Program.GetGlobalTypingsCacheLocation`, `Program.GetResolvedModules` and
   `Program.Tracing`; `diagnosticwriter.go`'s `FlattenDiagnosticMessage`,
   `WriteFlattenedASTDiagnosticMessage`, `diagnosticPrefix`,
   `getCategoryFormat`, `writeWithStyleAndReset` and the `ECMALineMap`,
   `FileName` and `Text` of `originalTextFile` and `renamedFile`; and
   `extendedconfigcache.go`'s `ExtendedConfigCache.GetExtendedConfig`.
7. **The harness's four decoders have no caller.** The readable build info's
   `UnmarshalJSON` methods (`readableBuildInfoDiagnosticsOfFile`,
   `readableBuildInfoSemanticDiagnostic`, `readableBuildInfoFilePendingEmit`,
   `readableBuildInfoResolvedRoot`) were X0's four pending functions. At the
   pin the readable form is only marshalled (`toReadableBuildInfo`'s
   `json.MarshalIndent`, `readablebuildinfo.go:242`); the unexported types
   appear only in `readablebuildinfo.go`, and the package's `json.Unmarshal`
   calls (`fs.go:40`, `62`) decode `incremental.BuildInfo`. They are recorded
   `equivalent` at the harness's encode-only Rust types, as the Phase 3 audit
   records a function with no caller; X0 has no pending function.

## The scenario inventory and its verification

`scripts/phase4_scenarios.py record` runs the pinned `tsctests` package in a
scratch export of the pin (`target/phase4/scenarios-tree`) with patched copies
of `runner.go`, `sys.go` and `fs.go` laid over it (`go test -overlay`; each
patch is a strict unified diff headed by the pinned file's SHA-256, checked
against the ledger's `source_hash`) and `recorder.go` added. The patches only
add hook calls. The recording, `data/phase4/scenarios.json.gz` (105,772
bytes, gzip of canonical JSON), holds per scenario its arguments, working
directory, environment, TTY flag, case sensitivity, Windows-style root, the
initial file map with the library files the fake system adds, the clock
readings before the first command, and per edit its caption, argument
override, expected-difference text and the primitive file operations the
closure performed (601 writes, 52 removes, 1 `Chtimes`) with their clock
readings; 3 edits record different operations on the clean-build shadow; one
scenario starts from `GetFileMapWithBuild`. Provenance: the pin, the patch
digests, go1.27.1 darwin/arm64, `macOS-26.6.1-arm64-arm-64bit`, and the 516
scenario digests.

`verify` holds the plan's three checks: a fresh patched run reproduces every
committed reference it renders (516 of 516) and the rendered set plus the
orphan is exactly the four families' 517 files; a second recording is
byte-identical; and `replay_test.go`, added to the unpatched package, rebuilds
each scenario from the inventory with the recorded operations in place of the
closures, runs it through the pin's own `tscInput.run`, and passes 516 of 516
with no local baseline written. `check` validates the committed file without
Go.

## The Rust harness

`tools/phase4/tsctests` (package `phase4_tsctests`, `publish = false`) ports
the five harness files and the `testutil` pieces as harness code with
markers: the fake system and its clock, the 11 testing hooks and the output
sanitizer (`sys.go`), the test file system with the build-info version swap
and the readable build info (`fs.go`, `readablebuildinfo.go`), the mock watch
backend, the runner with the clean-build shadow and `getDiffForIncremental`
(`runner.go`), the file-system differ (`fsbaselineutil`) and the baselining
tracer (`harnessutil`). Its binary
`phase4_tsctests --output DIR [--scenarios FILE] [--jobs N] all|FAMILY|ID...`
writes one row per scenario (`rows.jsonl`), each transcript as far as it got
(`baselines/<id>`) and `summary.json`.

Its witnesses (`cargo test -p phase4_tsctests`, 24 tests): every scenario's
transcript up to and including its first command line equals the committed
reference (516 of 516); the readable build info renders every committed
rendering byte for byte from the build-info text it was written for (1,257 of
1,257, over 1,271 build-info texts); and the sanitizer, the tracer, the mock
watch backend, the clock, the fake system's steps and the incremental
differences match a native probe of the pin (`testdata/probe.json`).

**The seam X1 must supply.** `src/execute.rs` holds the command-line entry and
the pinned interfaces it is called through, shaped as the pin's so that
`tsr_execute` (X1), `tsr_fswatch` (X4) and the watch manager (X5) replace the
module without a harness change: `execute.CommandLine`, `tsc.System` (the
pin's eleven methods, `Spawn` through `tsr_contentmapper::Spawner`), the six
exit statuses, `CommandLineResult` with its `Watcher`, `CommandLineTesting`
with its eleven hooks, and the watch backend types. Until X1 the entry refuses
with the named operation `execute.CommandLine is Phase 4 X1`.

**Two private items.** `src/program_view.rs` reads what `TestSys.OnProgram`
needs; two of them are crate-private and refuse with a named operation until
X1/X2 make them public: `tsr_incremental::Snapshot::semantic_diagnostics_per_file`
(the pin's `TestingData.SemanticDiagnosticsPerFile.Load`) and
`tsr_compiler::Program::include_reasons` (`Program.GetIncludeReasons`).

## The Rust run

`scripts/phase4_corpus.py run` builds the binary (`cargo build --locked -p
phase4_tsctests --bin phase4_tsctests`), copies it into the capture, runs it
once over the inventory and binds the capture: `capture.json` names the
inventory (path, digest, pin, scenario count), the selection and the digest of
the selected ids, and the digests of the executable, `build.json` (the cargo
command, the binary's digest, the source fingerprint), `rows.jsonl`,
`summary.json` and the binary's standard streams. Loading or replaying a
capture checks every artifact and every row: one state with exactly its
fields, the scenario's id, family and recorded digest, the progress against
the scenario's edits, each transcript's digest and size, the ids against the
selection in inventory order, the summary against the rows. A `harness`
failure or a panic located outside `crates/` is a harness error. A selection
other than `all` is partial: informational, never recorded.

The first run, `target/phase4/rust` (capture `81c2ac39…`, executable
`66180f1a…`): 516 rows, all `unsupported` with `execute.CommandLine is Phase 4
X1`, each stopped at its first command; 0 harness errors, 0 production
failures.

## The comparison

`scripts/phase4_compare.py report` compares each scenario's whole transcript
with its committed reference, read from the pinned commit with `git cat-file`
(never the working tree), and puts every scenario in one category: `match`,
`different`, `failed`, `unsupported` or `unexecuted` (not selected by a
partial run). The orphan reference is listed apart. `--record` writes the
acceptance summary (the report without its rows) to
`data/phase4/first-comparison.json` and refuses a partial, harness-invalid or
source-stale run.

**Attribution.** A `different` row is split into the runner's steps
(`initial`, `edit N`) and each step into the sections `runner.go`, `sys.go`
and `fsbaselineutil` write: `input` (the current directory, case sensitivity,
`Input::` and the initial files), `edit` (the `Edit [N]:: caption` line and the
files the edit changed), `command` (`tsgo <arguments>` and `ExitStatus::`;
absent for a watch cycle), `output`, `files` and `buildinfo` (the file-system
difference after the command by path and label, the `.tsbuildinfo` and
`.readable.baseline.txt` entries apart), `watch` (`Watch Registrations::`),
`program` (each program's `<config>::`, `SemanticDiagnostics::` and
`Signatures::`, and an include-reason failure block) and `incremental` (the
`Diff::` block). The split is driven by the recorded scenario (captions,
command lines, family) and is lossless. Over the 516 references it finds all
1,771 steps, 1,479 commands (the 292 watch edit steps have none), 397 watch
states, 1,336 program blocks and the plan's 7 explained incremental
differences, with every section boundary where the writers put it. A row names
its first differing step and section, the first differing path with both
labels or the first differing line, and every section that differs; an edit
step agrees incrementally when its `incremental` section equals the
reference's. A content difference inside an emitted file is flagged for
Phase 3.

**The first comparison** (full run):

| Family | match | different | failed | unsupported | unexecuted |
| --- | ---: | ---: | ---: | ---: | ---: |
| `tsc` | 0 | 0 | 0 | 217 | 0 |
| `tsbuild` | 0 | 0 | 0 | 192 | 0 |
| `tscWatch` | 0 | 0 | 0 | 42 | 0 |
| `tsbuildWatch` | 0 | 0 | 0 | 65 | 0 |
| Total | 0 | 0 | 0 | 516 | 0 |

All 516 stopped transcripts are a prefix of their reference (the harness's
header witness, seen by the comparison). Edit steps agreeing incrementally: 0
of 1,255. The orphan reference: `tsc/commandLine/adds-color-when-FORCE_COLOR-is-set.js`.

**The mutation check.** `phase4_compare.py mutation` writes synthetic
captures of completed rows built from the committed references to
`target/phase4/mutation` and reports each through the same loading and
comparison code. It fails unless every untouched row matches and every mutated
row reads `different`:

| Variant | Mutated rows | Read `different` | Attributed to the mutated step and section | Unmutated rows matched |
| --- | ---: | ---: | ---: | ---: |
| untouched references | 0 | | | 516 of 516 |
| one byte changed (a letter made uppercase, in a part chosen per scenario) | 516 | 516 | 516 (buildinfo 58, command 128, edit 27, files 34, incremental 1, input 70, output 114, program 63, watch 21) | |
| the last edit step dropped | 292 | 292 | 292 (the step missing) | 224 of 224 |
| the outputs of two steps swapped | 264 | 264 | 264 (output) | 252 of 252 |
| two scenarios' transcripts swapped | 514 | 514 | (first sections: input 442, command 56, output 10, program 6) | 2 of 2 |

`scripts/tests/test_phase4_compare.py` runs the same check on five scenarios,
one of each family and the explained-difference one.

## Blockers

`scripts/phase4_blockers.py build|check` derives `data/phase4/blockers.json`
from the full comparison: one entry per bucket of scenarios that cannot pass
(category and cause: the named refusal, the failure reason, or the first
differing section and the kind of step), with its scenario count, the digest
of its sorted ids, the count per family, five examples with their capture
transcript, and the owner: a cause that names its checkpoint is that
checkpoint's; a content difference inside an emitted file is Phase 3's
residual, a cross-phase entry (plan section 7); any other cause is its
families' (`tsc` X1, or X2 when the first difference is in the build
information, the program data or the incremental difference; `tsbuild` X3;
the watch families X5). `check` rebuilds it and requires equality.

| Id | Kind | Cause | Owner | Scenarios |
| --- | --- | --- | --- | ---: |
| B01 | unsupported | `execute.CommandLine is Phase 4 X1` | X1 | 516 (`tsc` 217, `tsbuild` 192, `tscWatch` 42, `tsbuildWatch` 65) |

The plan's cross-phase entry, from the evidence:

- **Phase 3's emit** (420 of the 517 baselines contain emitted files, plan
  section 2): `landed`. `crates/tsr_compiler/src/program_emit.rs` carries the
  markers of `Program.Emit`, `CombineEmitResults` and `HandleNoEmitOptions`,
  and Phase 3's recorded comparison (`data/phase3/first-comparison.json`)
  matches 13,432 of 13,432 variants with no unsupported row. No entry.

## Producer and sprints

`scripts/phase4_producers.py tsc` (`status/runs.toml` `[tsc]`) never runs the
harness:

- `inventory_frozen`: `phase4_scenarios.py check` passes;
- `inventory_reproduced`, `inventory_identical`, `inventory_replayed` and
  `inventory_verified`: the producer runs `phase4_scenarios.py verify` itself
  (the Go toolchain of `data/s04/toolchains.toml` must be on PATH) and reports
  the three checks and their conjunction;
- `harness_valid`: the capture at `target/phase4/rust` is complete (not
  partial, one row per scenario), taken over the current inventory, built from
  the current sources, every artifact bound, no harness error in its replay or
  its comparison;
- `result_recorded`: the comparison's acceptance summary equals
  `data/phase4/first-comparison.json`;
- `blockers_named`: the committed register equals the rebuilt one and names
  every scenario that cannot pass with an owner;
- `unsupported_required` (scenarios with a named refusal), `baseline_parity`
  (scenarios that match whole over the 516) and `incremental_correctness`
  (edit steps agreeing incrementally over the 1,255), emitted only over a
  harness-valid capture;
- `unit_tests` and `audit`: `phase4_unit_tests.py check` and
  `phase4_audit.py check` pass (current and valid; the audit is not complete).

`status/runs.toml` `[tsc]` lists as inputs the inventory, the roster, the
audit, the recorded comparison, the register, Phase 3's recorded comparison
(the register's emit evidence), `data/upstream.json`, `.gitmodules`,
`data/s04/toolchains.toml`, `PORTS.toml` and `data/go-functions.tsv`; as
sources the Phase 4 scripts and the libraries they import, `tools/phase4/**`,
`upstream`, the capture's closure, and every Rust file under `crates/` and
`tools/` (the audit and the roster scan them for markers and ported tests).

`sprints/P4A.toml` (`P4A-X0`) exits on the five X0 metrics. `sprints/P4B.toml`
carries the plan's section 5 exit (516 where the plan says 517, with a comment
naming the orphan), with the seven Phase 2 parity metrics and every `run.emit`
condition of `P3B`'s exit, and one item per checkpoint, `P4B-X1` to `P4B-X7`,
each on `run.tsc.xN_complete`, which no producer emits yet. The later metrics
(`buildinfo_codec`, `buildinfo_interop`, `unit_rosters`, `watcher_tests`,
`live_watch_parity`, `smoke`, `thread_sanitizer`, `determinism`, `residuals`,
`dispositions`, `evidence_current`, `report`, `xN_complete`) come with their
checkpoints.

`scripts/tests/test_phase4_{corpus,compare,producers}.py` cover the row
contract, the capture binding (a tampered artifact or transcript, reordered,
missing or extra rows, another inventory or selection, a summary or standard
output that disagrees, harness errors), the split over every reference, the
attribution, the recorded result, the mutation check, the producer's validity
and metrics (a partial, tampered, stale or missing capture withholds them; a
failed verification check is reported; the recorded result and register must
be this run's) and the register (a new bucket makes it incomplete; the owner
rules; the emit evidence). They need no `target/` capture.
`test_phase4_audit.py` gains the X0 group's closure. `python3 -m pytest
scripts/tests -q -k phase4`: 73 passed.

## What is not done

- `cargo xtask run tsc` and `cargo xtask check P4A` are the owner's; `STATUS.md`,
  `status.json` and `docs/status.html` are not regenerated.
- The ledger moves of decision 4 are deferred: they regenerate `PORTS.toml`'s
  generated fields and `data/upstream.json`, which stales every recorded
  capture that binds them, so they wait for the owner. The audit's scope
  already excludes the three moved files and is the same before and after the
  regeneration.
- Decision 7 (dependencies) is open. X0 added no third-party dependency:
  `Cargo.lock` gained only the `phase4_tsctests` package.
- The per-scenario feature tags and first-checkpoint assignment that X1's
  witness list attributes to X0's inventory are not built; the register
  assigns owners by cause and family.

## Cost and run policy

Measured on the development host (Apple M-series, go1.27.1).

| Step | Wall |
| --- | ---: |
| `phase4_scenarios.py record` (one patched run of the pinned suite) | about 6 s |
| `phase4_scenarios.py verify` (reproduce and identical 4.3 s, replay 2.4 s) | about 7 s |
| The pinned suite's own run (`phase4_scenarios.py pinned`) | about 4 s |
| `phase4_corpus.py run`, 516 scenarios (cached build 0.3 s, the binary 0.8 s with 8 jobs, replay 0.1 s) | 1.6 s |
| `phase4_corpus.py replay` | 0.3 s |
| `phase4_compare.py report --record` | 0.4 s |
| `phase4_compare.py mutation` (five captures of 516 rows) | 2.4 s |
| `phase4_blockers.py build` or `check` | 0.3 s |
| `tsc` producer (7 s of it the verification) | 8 s |
| `cargo test -p phase4_tsctests` (cached build) | 4 s |
| `pytest scripts/tests -k phase4` | 18 s |

Policy: every run is the full suite. No sample is needed while a full run
takes seconds; the plan's estimate (517 + 2 × 1,255 command executions per run
once the command line exists) will raise it from X1 on, and a family or a
scenario id can be selected during work (`phase4_corpus.py run tscWatch`,
partial and never recorded). The producer re-verifies the inventory on every
recording; it reruns the pinned suite twice in seconds.

## Reproduction

```sh
export PATH="/Users/cristian/.local/share/mise/installs/go/1.27.1/bin:$PATH"
python3 scripts/phase4_scenarios.py check
python3 scripts/phase4_scenarios.py verify
python3 scripts/phase4_unit_tests.py check
python3 scripts/phase4_audit.py check
cargo test --locked -p phase4_tsctests
python3 scripts/phase4_corpus.py run --output target/phase4/rust --replace
python3 scripts/phase4_compare.py report --rust target/phase4/rust --record
python3 scripts/phase4_compare.py mutation --output target/phase4/mutation
python3 scripts/phase4_blockers.py build --record
python3 scripts/phase4_blockers.py check
python3 scripts/phase4_producers.py tsc
python3 -m pytest scripts/tests -q -k phase4
```

Intermediate runs:

```sh
python3 scripts/phase4_corpus.py run --output target/phase4/rust-tscWatch --replace tscWatch
python3 scripts/phase4_compare.py report --rust target/phase4/rust-tscWatch
```

A partial report says `partial: true` and lists the unselected scenarios as
`unexecuted`; it cannot be recorded, cannot replace the register and does not
make the producer's metrics.
