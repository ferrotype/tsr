# Phase 4: programs, command line, build and watch

Status: **proposed for owner review** (2026-10-01).
The detailed implementation plan for [PLAN Phase 4](../PLAN.md#phase-4-programs-command-line-build-and-watch).
Its checkpoints carry their own work items, witnesses and exit checks;
production implementation starts at X0.

Planning reference: branch `phase2-c7` at `8a456baf`, with the accepted
[Phase 3 plan](PHASE3-plan.md) beside it. Upstream remains Corsa
`1f70213d4922b434345f639b441681e470c7cfc1`.

## 1. Outcome and starting point

Deliver the pinned command line as the native `tsrust` binary over a project or a
file list, `--incremental`, `--watch`, and `tsc -b` with clean, dry, force and
build-watch, with the pin's console output (plain and pretty, colors,
locales), exit statuses, written files and `.tsbuildinfo`. This is PLAN's
Phase 4 gate: "All 517 tsc, watch, build and build-watch baselines; the smoke
test compiles upstream's fixture project single- and multi-threaded; a
ThreadSanitizer build mirrors upstream's race-mode job; the
`--singleThreaded` and concurrent-test-programs configurations both pass."

The starting point is the recorded Phase 2 exit and the accepted Phase 3
plan. Program loading, checking, the checker pool, cancellation and the
tracing seam exist. Emit is Phase 3's, and 420 of the 517 baselines contain
emitted files, so Phase 4 closes after Phase 3 does (section 3).

### What exists

| Asset | Where | State |
| --- | --- | --- |
| Program loading and checking | `tsr_compiler`: `Program::load`, `load_with_content_mapper_project`, `CheckedProgram` with `CheckerRequest`, `CompilerCheckerPool`, option verification, include reasons (`explain_file_include`), program diagnostics, `CompilerConfigHost` | 167 of the 272 functions of the 11 compiler files that stay in Phase 4 are marked: `program.go` 83 of 144, `fileloader.go` 37 of 41, `projectreferencefilemapper.go` 11 of 14, `fileInclude.go` 9 of 13, `filesparser.go` 8 of 12, `includeprocessor.go` 7 of 8, `projectreferenceparser.go` 6 of 6, `host.go` 4 of 10, `processingDiagnostic.go` 2 of 4, `projectreferencedtsfakinghost.go` 0 of 20 |
| The diagnostic writer | `tsr_compiler::diagnostic_writer` (`mod.rs`, `pretty.rs`, `resolved.rs`) | 25 of 40 `diagnosticwriter.go` functions marked: the pretty form with color and context, the code snippet, the error summary and its tabular display, the status-and-time forms, `TryClearScreen`, locations, the comparer. The 15 unmarked are the flatten helpers and accessors, which have unmarked Rust counterparts (`flattened`, `prefix`, `category`, `styled`) |
| Command-line and config parsing | `tsr_tsoptions`: `ParseCommandLine`, `ParseBuildCommandLine`, response files, `GetParsedCommandLineOfConfigFile`, extended configs, wildcard directories, `ConvertToTSConfig` (`show_config.rs`) | Phase 1's, with the 309 config baselines. `OptionDeclaration` carries no description, category, default-value description or simplified-help flag: `--help`, `--all` and `--init` cannot be printed from it |
| File systems | `tsr_vfs`: `OsFs` (`osvfs` 23 of 23), `CachedFs` (`cachedvfs` 16 of 16, with `enable`, `clear_cache`, `disable_and_clear_cache`), `vfstest::TestFs` with a `Clock`, `chtimes`, `get_mod_time` and `entries`; `native::realpath` (`nativepath` 4 of 4); `tsr_bundled` (the embedded libraries and their wrapping file system) | Phase 1's. `bundled.LibPath` and `WrapFS` have Rust counterparts without markers |
| Locales | `tsr_locale::Locale::parse`, `tsr_diagnostics::locales_generated` | `locale.go` 5 of 5 |
| Concurrency and lifecycle | `tsr_core::workgroup::WorkGroup` (bounded: at most `worker_bound()` threads on reserved 256 MiB stacks), `ThrottleGroup`, `tsr_core::CancellationToken`, the poisoned-checker contract, `tsr_core::version` | C6 and C7; the bounded group the #73 review asked for is in |
| The tracing seam | `tsr_checker::trace`: `TraceSink`, `Tracer`, `TraceSpan`, `MemoryTraceSink`, `JsonLinesTraceSink`, `write_type_records` | The checker's phases and type records go through the seam. `tracing.go` (20 functions) has no marker: the trace-file writer, the thread ids and the legend are Phase 4's (C7's record; Phase 3 decision 11) |
| Content mappers | `tsr_contentmapper` (host, `Spawner`), `tsr_ipc`, `tsr_jsonrpc`, `tsr_contentmappertest::new_spawner` with the failing, lisp, supplemental and transforming fakes | In-process mappers load and check (C7.8). The verbatim, dynamic-verbatim and manifest fakes the command-line tests name, and `NewSpawnerWithProjectLifecycle`, are not ported; nothing starts a child process |
| Emit | Phase 3: `CheckedProgram::emit`, `HandleNoEmitOptions`, `CombineEmitResults`, the `EmitOnly` and `ForceEmit` modes, the write seam (T8) | Planned and accepted; not built |
| Hashing | `xxhash-rust` with `xxh3`, already a dependency of `tsr_contentmapper` and `tsr_embed` | The pin's `ComputeHash` is the 128-bit XXH3 of the text, in hex |
| Instrumented builds | The pinned nightly of `data/s04/toolchains.toml`; E3's Miri and AddressSanitizer runs with `-Zbuild-std` (`scripts/s04_ownership.py`) | The pattern a ThreadSanitizer run follows |
| The Go oracle | The `[oracle]` producer builds the pinned binary; `scripts/phase2_native.py` runs overlay tests inside pinned packages | Both reused: the binary for the live witnesses, the overlay pattern for the scenario recorder |

### The acceptance corpus

The 517 baselines are the committed references under
`testdata/baselines/reference/{tsc,tsbuild,tscWatch,tsbuildWatch}`. Each is the
transcript of one scenario of `internal/execute/tsctests`: the input files,
then for the first command and for every edit the command line, the exit
status, the sanitized console output and the file-system difference, and for
incremental runs the program's testing data.

| Family | Baselines | With edits | Edit steps | Mention `.tsbuildinfo` | No emitted output |
| --- | ---: | ---: | ---: | ---: | ---: |
| `tsc` (22 scenario folders) | 218 | 69 | 410 | 84 | 83 |
| `tsbuild` (32) | 192 | 125 | 553 | 182 | 11 |
| `tscWatch` (3) | 42 | 36 | 61 | 1 | 3 |
| `tsbuildWatch` (13) | 65 | 62 | 231 | 65 | 0 |
| Total | 517 | 292 | 1,255 | 332 | 97 |

The baselines hold 1,271 `.tsbuildinfo` texts and 1,257 readable renderings
of them; 7 carry an explained difference between the incremental and the
clean build; 9 run content mappers; 2 write trace files. The test sources
declare 380 scenarios literally and generate the rest (the 14 color cases,
for one) to reach 517. Of the 76 test functions of the package, 61 produce
baselines, 12 are content-mapper lifecycle tests that assert directly, 2 are
race tests and one is `TestMain`. The other Phase 4 packages hold 146 unit
tests: `fswatch` 124, `watchmanager` 7, `incremental` 5, `nativepath` 5 (4 of
them Windows), `build` 2, `tracing` 2 and `cmd/tsc` 1.

### Gaps confirmed by source inspection

- No driver exists: `execute/tsc.go` (12 functions), `execute/tsc/*.go` (49)
  and `cmd/tsc` (25) have no Rust function, and the workspace's only binary
  target is `tsr_bench`'s.
- No incremental state exists: `execute/incremental` (175 functions, 3,535
  lines) and no `.tsbuildinfo` reader or writer; the Rust side knows only the
  build-info file name.
- No build orchestrator exists: `execute/build` (87 functions).
- No watcher exists: `execute/watcher.go` (20), `execute/watchmanager` (29)
  and `fswatch` (219 functions in its 14 in-scope files).
- No program reuse exists: `Program.ReuseProgram`, its admissibility helpers,
  `lazyValue.tryReuse` and `updateFileIncludeProcessor`.
- The driver's program-level reports are missing:
  `GetDiagnosticsOfAnyProgram`, `Program.ExplainFiles`, the five statistics
  counters, `NewCachedFSCompilerHost` and `ContentMapperProjectDiagnostic`.
- The option table has no help metadata (above).
- No trace-file writer and no profiler exist.
- No part of the `tsctests` harness exists: the fake system and clock, the
  file-system differ (`fsbaselineutil`), the baselining tracer, the readable
  build info and the mock watch backend.

By the numbers: `PORTS.toml` assigns 82 files to Phase 4, 10 of them out of
scope (Windows and other-platform files of `cmd/tsc`, `fswatch` and
`nativepath`). After the two emitter files move to Phase 3 (its decision 1),
70 in-scope files remain, 65 source and 5 harness, with 22,479 lines and
1,055 functions, 196 of them marked (19%): 167 in `compiler`, 25 in
`diagnosticwriter` and 4 in `nativepath`. The harness and its tests are
13,359 lines of Go.

## 2. Scope and phase boundaries

| Area | Phase 4 responsibility | Boundary |
| --- | --- | --- |
| The driver | `execute/tsc.go` and `execute/tsc/*.go`: `CommandLine`, the compile paths, emit gating and reporting, diagnostic and status reporters, help, `--init`, `--showConfig`, `--version`, statistics, the extended-config cache | Emit itself is Phase 3's; option and config parsing are Phase 1's and are consumed as they are |
| The compiler remainder | `host.go` (both host constructors), `GetDiagnosticsOfAnyProgram`, `ExplainFiles`, the statistics counters, the accessors `incremental` reads, program reuse for watch, `ContentMapperProjectDiagnostic` | `program.go`'s `Emit` family is Phase 3's and its project-system operations are Phase 5's (below); the checker pool moved to Phase 2 at C6 |
| The diagnostic writer | The 15 unmarked `diagnosticwriter.go` functions get markers or recorded equivalents | The module stays where it is (decision 5) |
| Incremental | `execute/incremental` complete: the build-info codec, snapshots, affected files, pending emit, signatures, the incremental program | The forced declaration emit that computes a signature is Phase 3's `EmitOnly` mode; Phase 4 calls it |
| Build | `execute/build` complete: the graph, up-to-date status, tasks, hosts, clean and dry runs, the parallel builders | None |
| Watch | `execute/watcher.go`, `execute/watchmanager`, the orchestrator's watch cycle, program reuse | The language server's watcher registration (`lsp/lspwatcher`) is Phase 5's |
| The native watcher | `fswatch`: the core, events, debounce, the directory walker, and the FSEvents, inotify and fanotify backends, ported and not replaced by a generic crate (PLAN) | kqueue and Windows are out of scope ([ADR 0002](adr/0002-native-targets-and-the-glibc-floor.md)); `kqueue.go` is still a source file in the ledger (decision 4) |
| Tracing, statistics, profiling | `tracing.go` as a `TraceSink` that writes the pin's files; `statistics.go`; `pprof.go` by decision 6 | The trace events themselves are emitted by Phase 2's and 3's code through the seam |
| The binary and the system | `cmd/tsc`: `main`, the OS system (terminal, environment, clock, child processes), signals; the `lib/tsc` layout | `--lsp` and `--api` dispatch to Phases 5 and 6 (decision 12). Four-target builds, the size budget and cut-over are Phase 7's |
| Acceptance | The ported `tsctests` harness, the scenario inventory, the unit-test rosters, the live witnesses, the ThreadSanitizer run and the smoke test | Phase 2's and 3's gates keep their recorded form and are re-recorded at the exit (section 5) |

Six boundaries need explicit settling:

**The emit dependency.** A scenario whose baseline contains an emitted file
cannot match before Phase 3's emit exists: 420 of 517. Incremental state
depends on emit even where no file content is compared, because a file's
signature is normally the hash of its declaration output and a project's
up-to-date status follows its outputs' timestamps. The 97 scenarios without
emitted output can match first. X0, X4 and X6 can close without emit; X1, X2,
X3 and X5 cannot close before Phase 3's T8, and the P4B exit requires P3B
(decision 3).

**`program.go`.** 61 of its 144 functions are unmarked. The Phase 1
destination audit and C7's audit already place most of them: 21 are Phase
4's (the reuse family, the statistics counters, the reports and the accessors
`incremental` reads; section 4 names them), 6 are Phase 3's emit family, 19
are Phase 5's (the project system, auto-import and type acquisition),
`NewProgram` is C7's recorded equivalent, and 11 carry a Phase 2 destination
with neither a marker nor an audit entry. Three need settling.
`GetDiagnosticsOfAnyProgram` was sent to Phase 3, but its only caller is the
driver's `EmitFilesAndReportErrors` and the Phase 3 plan does not claim it:
X1 ports it. `ResolveModuleName` and `GetResolvedProjectReferenceFor` have no
destination; the first is called by the language service and type
acquisition, the second by nothing. X0's audit records every one of the 61.
The 11 either get the site of their Rust counterpart or become named work;
three of them are X2's if they are missing, because `incremental` reads them
(`GetSemanticDiagnosticsWithoutNoEmitFiltering`,
`FilterNoEmitSemanticDiagnostics`, `GetIncludeProcessorDiagnostics`).

**The faking host.** `projectreferencedtsfakinghost.go` (20 functions) runs
only when `UseSourceOfProjectReference` is set, and the pin sets it in one
place, `project/project.go`. No Phase 4 baseline or test reaches it. It moves
to Phase 5 (decision 4).

**The test-program modes.** The gate's last clause names upstream's
"concurrent test programs" CI job. Its switch,
`TS_TEST_PROGRAM_SINGLE_THREADED`, is read in one place, the compiler-test
harness (`harnessutil.go:972`); the
`tsctests` scenarios never consult it. They run the real command line, which
is concurrent unless a scenario passes `--singleThreaded`. Phase 4's
obligations under that clause are therefore: the smoke test in both thread
configurations; the Phase 2 and Phase 3 gates still holding in both
test-program modes on the final sources; and the 517 scenarios giving the
same bytes on every run of the default concurrent program (section 5).

**The mapper child process.** C7's record assigned "the project system's
child-process spawner" to Phase 5, but the code is `cmd/tsc/sys.go`
(`osSys.Spawn`, `spawnProcess`, `childProcess`), a Phase 4 file, and
`tsc --runExternalCode` needs it. Phase 4 ports it with its pinned test;
Phase 5 reuses it from the server entry (decision 15).

**Profiling.** `pprof.go` drives Go's runtime profiler, which has no Rust
counterpart in the dependency set. Decision 6 settles what `--pprofDir` does.

## 3. Prerequisites and coordination

Phase 4 consumes these contracts as they are, through their public entry
points. A missing operation among them is a named joint blocker with an
owner, not a Phase 4 patch:

| Contract | Owner | Phase 4 use |
| --- | --- | --- |
| `CheckedProgram::emit`, `HandleNoEmitOptions`, `CombineEmitResults`, `IsEmitBlocked`, the `EmitOnly` and `ForceEmit` modes, the write callback with `WriteFileData`, the emit diagnostics | Phase 3 (T8) | Every compile path; `incremental`'s per-file emit, signature computation and build-info emit; `--listEmittedFiles`; `noEmitOnError` |
| `CheckedProgram` with `CheckerRequest`, `CompilerCheckerPool` and its count rule (`--checkers`, `--singleThreaded`) | Phase 2 (C6, C7) | Diagnostics gathering for every program kind; the pool the watch reuse recreates |
| The bounded `WorkGroup` | Phase 2 (C6) | Config parsing in the build graph, the builders of `tsc -b`, the harness's clean-build shadow |
| `CancellationToken` and the poisoned-checker contract | Phase 2 (C6) | SIGINT and SIGTERM; the watch loop's exit; the end of a test's context |
| `TraceSink` | Phase 2 (C6), Phase 3 (the emit pushes) | The file writer is one more sink |
| The content-mapper host, the in-process spawner, `load_with_content_mapper_project` | Phase 2 (C7.8) | `NewContentMapperHost` under `--runExternalCode`, the mapper project per compilation, per build task and per watch session |
| `ParseCommandLine`, `ParseBuildCommandLine`, `GetParsedCommandLineOfConfigFile` with an extended-config cache, `ConvertToTSConfig`, wildcard directories, the option declarations | Phase 1 | The driver's dispatch, `--showConfig`, the watch directories. The declarations gain help metadata in X1 |
| Module-resolution tracing and the package.json cache | Phase 1 | `--traceResolution` output in the baselines; the package.json paths a build info stores |
| `OsFs`, `CachedFs`, `vfstest` with its clock, `realpath`, the bundled libraries | Phase 1 | The OS system, the cached host of `tsc` and `tsc -b`, the fake system |
| Locales and the diagnostic message tables | Phase 1 | `--locale`, help and status texts |
| The phase2 comparison, register and producer libraries | Phase 2 (C0) | Reused as libraries by the Phase 4 scripts, never edited |

Coordination:

- **Phase 4 can run beside Phase 3** ([PLAN section 9](../PLAN.md#9-sequence-and-gates)).
  X0, X1, X4 and X6 make progress without emit; X2, X3 and X5 are built
  against T8's emit and start when it exists. With one implementer this is an
  ordering choice, not parallel work (decision 3).
- **The shared checkout is C7's, then Phase 3's.** Phase 4 develops on its own
  branch in its own worktree and rebases on the Phase 3 checkpoints it
  consumes.
- **One ledger regeneration.** Decision 4's moves and Phase 3's decision 1
  change generated ledger fields and `data/upstream.json`, which stales every
  recorded run that binds them. They land together if Phase 3's T0 has not
  yet run, and otherwise in X0 with the re-freezes that follow.
- **Staleness is expected mid-phase.** Every change under `crates/**` stales
  the recorded `checker` and `emit` runs. X1's change to
  `scripts/s07_option_declarations.py` also stales the producers that bind
  that generator. Nothing is re-recorded per fix; X7's green-up re-records
  what is stale (decision 14). CI red for that reason is listed, not ranked.
- **Recorded inputs are not edited.** The phase2 and phase3 scripts that
  recorded captures bind stay as they are; Phase 4 adds `scripts/phase4_*.py`.
- **The dependency rule holds:** no blanket Phase 2 or Phase 3 flag; each
  checkpoint names the contracts it consumes.

## 4. Delivery order

Checkpoints are **X0 to X7** (X for `execute`; C, T, E, F, P and S are taken).
Each pairs its witnesses with its production work and verifies the combined
result; X0 is preparation only. The order follows the dependency graph:

```text
X0 ─ X1 ─┬─ X2 ─ X3 ─ X5 ─┐
         ├─ X4 ───────────┤
         └─ X6 ───────────┴─ X7
```

X4, the native watcher, is a leaf: the watch baselines run on a mock backend,
so X5 does not wait for it, and only X7's live witness joins the two. X5
needs X2 (the watcher's program is the incremental program) and X3 (build
watch is the orchestrator's cycle). X2, X3 and X5 are built against Phase 3's
emit.

A checkpoint is complete when every scenario assigned to it matches whole.
Before Phase 3's emit exists, a scenario whose baseline contains emitted files
is `unsupported` under the register entry for Phase 3, the report shows how
the sections that emit does not feed compare, and the completion of X1, X2,
X3 and X5 waits (decision 3).

### X0 — the command-line acceptance contract

Preparation only; no Rust command-line parity is claimed.

- **Exists:** the 517 committed references; the overlay pattern of
  `scripts/phase2_native.py`; the `[oracle]` producer's binary; the phase2
  register and producer libraries; the wiring pattern of `[checker]` and
  `[emit]`.
- **Build:**
  - *The scenario inventory.* The scenarios are Go literals whose edits are
    closures, 11,765 lines of test declarations. They are recorded, not
    transcribed (decision 2). `scripts/phase4_scenarios.py record` runs the
    pinned `tsctests` package in a scratch copy of the pinned tree with
    patched copies of `runner.go`, `sys.go` and `fs.go` laid over it
    (`go test -overlay`; each patch is checked against the ledger's
    `source_hash`). The patch logs, per scenario: the baseline path, the
    arguments, the working directory, the environment, the TTY flag,
    `ignoreCase`, `windowsStyleRoot`, the initial file map (texts, symlinks,
    the library files the fake system adds, and for the one scenario that
    starts from `GetFileMapWithBuild` the outputs the pin's build wrote), and
    per edit the caption, the argument override, the expected-difference
    text and the primitive file operations the closure performs, in order,
    at the fake system (`writeFileNoError`, `removeNoError`, and the one
    `Chtimes`). `replaceFileText`, `appendFile`, `prependFile` and the rename
    helper reduce to those in the pin, so state-dependent edits are resolved
    by the pin and the clock ticks the same number of times. Result:
    `data/phase4/scenarios.json.gz` with its provenance (pin, patch digests,
    Go version, host, 517 scenario digests). It is a recorded input and is
    never edited.
  - *Verification.* `scripts/phase4_scenarios.py verify` holds three checks:
    the patched run reproduces all 517 committed references on this host; a
    second recording is byte-identical; and a replay test, which reads the
    recorded scenarios back and feeds them to the pin's unpatched runner with
    the recorded operations in place of the closures, reproduces the 517
    again. The third check is the one that matters: it proves the recording
    is sufficient input for a harness that is not the pin's.
  - *The unit-test rosters.* `data/phase4/unit-tests.json`: the 146 unit tests
    of the Phase 4 packages and the 14 `tsctests` functions that assert
    directly, each with its checkpoint, its host condition (any, macOS,
    Linux) and, where it does not apply, the reason (the Go FFI trampoline
    test, the four Windows tests). A ported test names the Rust test that
    carries it.
  - *The Rust harness.* `tools/phase4/tsctests` (repository-only) ports the
    five harness files as harness code, with markers: the fake system
    (`TestSys`: writers, working directory, environment, TTY, terminal width
    from `TS_TEST_TERMINAL_WIDTH`, the in-process mapper spawner, the 11
    testing hooks), `TestClock` (one second per reading), `testFs` (the
    written-files set, default-library tracking, the build-info version swap
    and the readable build info), the mock watch backend with `WatchState`,
    the runner (the first command, each edit, the clean-build shadow and
    `getDiffForIncremental`, the unexpected-difference failure), the output
    sanitizer (timestamps, durations, the list-files, statistics, trace,
    build-status and watch-status blocks), and from `testutil` the
    file-system differ (`fsbaselineutil`) and the baselining tracer. Its
    binary takes scenario ids or a family and writes the rendered baselines
    and one result row per scenario. Until X1 the command entry reports
    `unsupported`; the row is categorized, never blank.
  - *The comparison.* `scripts/phase4_compare.py report`: the whole baseline
    text against the committed reference is the result (`match`,
    `different`, `failed`, `unsupported`, `unexecuted`, as in Phase 2). For
    attribution the transcript is split into its sections, per step: command
    and exit status, output, file-system difference by path and label, build
    info, program data, watch state, incremental difference. The comparison
    is mutation-checked before it is believed: a changed byte in a reference
    copy, a dropped edit step and two swapped outputs must each read
    `different`.
  - *The first run and the register.* One complete Rust run (every row
    `unsupported` today) and `data/phase4/blockers.json` with the cross-phase
    entries: Phase 3's emit (420 scenarios) and whatever the first run names.
  - *The audit.* `data/phase4/x-audit.json` seeded with the dispositions
    section 2 settles: the 61 unmarked `program.go` functions, the 24 other
    unmarked functions of the compiler files that stay (the faking host's 20
    aside), the 15 of the diagnostic writer.
  - *Wiring.* Sprints `P4A` and `P4B`, the `tsc` producer in
    `status/runs.toml`, `data/phase4/`, `scripts/tests/test_phase4*.py`, the
    X0 record (`docs/PHASE4-X0.md`), and the ledger moves of decision 4.
  - *Cost.* Measure the recorder, one Rust run and the pinned suite's own
    time; set the run policy (section 6).
- **Exit:** `run.tsc.inventory_frozen`, `inventory_verified`, `harness_valid`,
  `result_recorded` and `blockers_named` true on a recorded `tsc` run; every
  row in exactly one category; `P4A-X0` passes.

### X1 — the driver: one compilation from the command line

- **Exists:** command-line and config parsing, program loading and checking,
  25 of 40 diagnostic-writer functions, `ConvertToTSConfig`, locales, the OS
  and cached file systems.
- **Build:**
  - *`tsr_execute`.* The `System` trait with the pin's 11 methods (ADR 0014:
    nothing below it reads the environment, the clock or the terminal
    directly); the six exit statuses; `CommandLineResult`; the testing hooks.
    `CommandLine` (a build flag is honored only as the first argument) and
    `tscCompilation` in the pin's order: command-line errors, profiling,
    `--init`, `--version`, `--help` and `--all`, the `watch` with
    `listFilesOnly` conflict, the `--project` rules, the config search up the
    ancestors (with the "tsconfig.json is present but will not be loaded"
    error and the help-with-failure path when there is nothing to compile),
    config parsing through one `ExtendedConfigCache` (an unrecoverable config
    error exits `DiagnosticsPresent_OutputsGenerated`), `--showConfig`, then
    the watch, incremental or plain path. `performCompilation`.
  - *`execute/tsc`.* `EmitFilesAndReportErrors`: the diagnostics of any
    program kind in the pin's order through `GetDiagnosticsOfAnyProgram`,
    with the bind and check timers; emit unless `listFilesOnly`; sorting and
    deduplication; one report per diagnostic; `listFiles`
    (`--listEmittedFiles`' `TSFILE:` lines, `--explainFiles`, `--listFiles`,
    `--listFilesOnly`); the error summary. `EmitAndReportStatistics`'s
    exit-status rule. The reporters: quiet, plain, pretty, the pretty default
    (`FORCE_COLOR`, `NO_COLOR`, `TERM=dumb`, then the TTY), the error-summary
    reporter, the color table. `help.go` (16 functions: the header box, the
    simplified and full listings, the build help, wrapping to the terminal
    width, value candidates and defaults). `init.go` (`WriteConfigFile`,
    `generateTSConfig`). `PrintVersion`. The trace callback
    (`GetTraceWithWriterFromSys`).
  - *Option help metadata.* `scripts/s07_option_declarations.py` also emits
    the fields `help.go` and `init.go` read (description, category,
    default-value description, the simplified-help flag); the generated
    tables in `tsr_tsoptions` are regenerated.
  - *The compiler remainder.* `NewCompilerHost` and `NewCachedFSCompilerHost`
    (C7's handoff), `GetDiagnosticsOfAnyProgram` (C7's handoff),
    `ContentMapperProjectDiagnostic` (C7's handoff), `Program.ExplainFiles`.
    The 15 unmarked diagnostic-writer functions get markers or recorded
    equivalents.
  - *The binary.* Crate `tsr` (`publish = false`), target `tsrust`: `main` and `runMain`, the
    OS system (standard output and error, `IsTerminal`, the terminal width,
    the environment, the normalized working directory, the clock, the
    bundled library path over the OS file system), SIGINT and SIGTERM into
    the `CancellationToken`, the main work on a reserved stack (ADR 0011;
    `ApplyDebugStackLimit` is Go's and gets an `equivalent` disposition),
    the exit code from the status, `--lsp` and `--api` by decision 12.
  - *The mapper child process.* `spawnProcess` and `childProcess` (standard
    output as the read side, standard input as the write side, `ExitCode`,
    `Close` that kills and reaps with the pin's one-second wait), behind
    `tsr_contentmapper::Spawner`; the verbatim and manifest fakes the
    scenarios name are added to `tsr_contentmappertest`.
- **Witnesses:** the `tsc` scenarios that keep no build state, build nothing
  and do not watch: X0's inventory tags each scenario with the features it
  uses and assigns it to the first checkpoint that implements all of them
  (134 of the 218 `tsc` baselines mention no `.tsbuildinfo`). By folder:
  `commandLine` 49 (44 of them without emitted output, among them the 14
  color cases, help, `--init` and the locale cases), `showConfig` 17,
  `ignoreConfig` 16, `extends` 8, `declarationEmit` 7,
  `forceConsistentCasingInFileNames` 5, `libraryResolution` 4,
  `moduleResolution` 3, `listFilesOnly` 2, `typeAcquisition` 1, the five
  content-mapper folders (7), and the members of `noEmit`, `noEmitOnError`,
  `noCheck` and `projectReferences` without build state. The pinned tests
  `TestChildProcessCloseDoesNotWaitForLauncherDescendants`,
  `TestRealpathHardlinkedFile` and the one content-mapper lifecycle test
  that runs plain `tsc`.
- **Contracts (`x1-contracts`):** the exit-status table; cancellation before
  and after the program exists; a checker panic is reported and exits
  nonzero without a hang (ADR 0012); a real child process is started, served
  and reaped, and a mapper that outlives its parent's close is killed; the
  binary runs on a real directory with the bundled libraries and matches the
  Go binary's output and status on a fixed three-file project.
- **Exit:** every X1 scenario without emitted output matches whole; the
  others match once emit exists; `x1-contracts` receipted; `P4B-X1`.

### X2 — incremental programs and `.tsbuildinfo`

- **Exists:** output paths and the build-info file name; `xxh3`; X1's driver.
- **Build:** crate `tsr_incremental`.
  - *The codec.* `BuildInfo` with its eight custom JSON shapes (a root as a
    number, a pair or a string; a file info as a string or an object, with or
    without a signature; reference-map entries; diagnostics of a file; a
    semantic-diagnostics entry as an id or a pair; pending emit as an id or a
    pair; emit signatures; resolved roots), the `omitzero` rules, the field
    order, compiler options as an ordered map with paths relative to the
    build-info directory, diagnostics with their message keys, arguments,
    chains and repopulation records.
  - *The snapshot.* File infos (`ComputeHash`: the XXH3-128 hex, with the
    text appended in the tests' mode), `FileEmitKind` and the pending-emit
    transitions, emit signatures, the reference map, the change set;
    `programToSnapshot` (reuse from the old program, changed and deleted
    files, global-scope changes, pending emit and check, referenced files
    from imports and symbols); `snapshotToBuildInfo` and
    `buildInfoToSnapshot`; `canUseIncrementalState`.
  - *Affected files and emit.* `affectedFilesHandler` (shape signatures by
    Phase 3's forced declaration emit, the files a change affects, the
    declaration-may-change propagation through references and global scope);
    `emitFilesHandler` (pending emit kinds against the emit options,
    skipping a composite project's unchanged declaration output, build-info
    emit).
  - *The incremental program.* `Program`: the diagnostics API with cached
    semantic diagnostics, `Emit`, `emitBuildInfo`, the has-errors and
    package.json state, nested emit time, the testing data;
    `ReadBuildInfoProgram`; diagnostic repopulation (the mode-mismatch and
    module-not-found chains); `performIncrementalCompilation`.
  - *`program.go`.* `PackageJsonCacheEntries`, `GetDefaultLibFile`,
    `GetResolvedTypeReferenceDirectives`, `GetIncludeReasons`,
    `IsMissingPath`, and the three diagnostic operations of section 2 if
    X0's audit found them missing.
- **Witnesses:**
  - *The codec, before emit exists.* Each of the 1,271 `.tsbuildinfo` texts
    in the baselines decodes and encodes back to the same bytes, and its
    readable rendering equals the baseline's (1,257 texts).
  - `TestExternalDiagnosticBuildInfoRoundTrip` and the four content-mapper
    identity tests.
  - The `tsc` scenarios with build state (84 baselines mention
    `.tsbuildinfo`: `incremental` 27, `composite` 7 and the incremental
    members of `projectReferences`, `noEmit`, `noEmitOnError` and `noCheck`),
    with their edit steps, their program data (`SemanticDiagnostics::`,
    `Signatures::`) and the harness's incremental-against-clean comparison.
- **Contracts (`x2-contracts`):** a build info with another version, a
  truncated one and one naming a missing file are each treated as the pin
  treats them (a clean build, never a panic); an old program's storage is
  released once its snapshot is taken; the same inputs give the same build
  info bytes on every run and for every checker count.
- **Exit:** the codec witnesses at 1,271 and 1,257; every X2 scenario matches
  whole; `x2-contracts` receipted; `P4B-X2`.

### X3 — the build orchestrator

- **Exists:** X2; `ParseBuildCommandLine`; project-reference parsing; the
  bounded work group.
- **Build:** crate `tsr_build`: `Orchestrator` (the graph, with configs
  parsed in a work group, circularity errors, `Order`, `Upstream`,
  `Downstream`); `BuildTask` (`getUpToDateStatus` and its status kinds, the
  verbose status messages, pseudo-builds that only update timestamps,
  `compileAndEmit`, `updateDownstream`, the build-info cache and conflicting
  build infos, `cleanProject`, `--dry`, `--force`, `--stopBuildOnErrors`);
  `rangeTask` (four builders, one under `--singleThreaded`, `--builders N`
  otherwise); a task waiting for its upstream tasks and releasing its
  downstream; each task's output buffered and written in build order after
  the previous task's report; the result's summary, deletions and
  aggregated statistics; the host with its modification-time cache, the
  parse cache, the per-task compiler host; `tscBuildCompilation`,
  `PrintBuildHelp`, the builder status reporter.
- **Witnesses:** `TestBuildOrderGenerator` and
  `TestIsContentMapperSupplementalBuildInfoPath`; the three build-side
  content-mapper lifecycle tests; the 192 `tsbuild` baselines with their 553
  edit steps (11 without emitted output: `commandLine` 5, `configFileErrors`
  3, and one each in `contentMapperOptionDiagnostics`, `demo` and
  `solution`).
- **Contracts (`x3-contracts`):** the `sample` scenarios give the same bytes
  on 20 runs with four builders; a task that fails or panics releases its
  downstream and the build ends with a status; cancellation between tasks;
  the number of live threads stays within builders times the work-group
  bound.
- **Exit:** every X3 scenario matches whole; `x3-contracts` receipted;
  `P4B-X3`.

### X4 — the native file watcher

- **Exists:** `nativepath.Realpath`; `rustix` in `tsr_vfs`.
- **Build:** crate `tsr_fswatch`: the public `Watcher`, `Watch`, `Event` and
  `EventKind`; the core of `watcher.go` (the directory-watch table by
  physical directory, consolidation under a covering recursive watch,
  batched `WatchDirectories`, `WatchFile` over the parent directory, ignore
  predicates, event paths rooted at the requested directory for a symlinked
  root, `ErrWatchTerminated`, `ErrUnavailable`); `event.go` (the sequenced
  event list and its create, update and remove coalescing); `debounce.go`
  (50 ms minimum, 500 ms maximum wait, one debouncer per backend); the
  directory walker; the backends: FSEvents through CoreServices,
  CoreFoundation and libdispatch declared `extern "C"` with framework
  linkage (the pin's cgo-free trampolines, 32 functions and three assembly
  files, get `equivalent` dispositions); inotify (a watch per directory,
  recursion by walking, overflow as a rescan); fanotify (kernel 5.13 or
  later without privileges, file-id reporting, the `FAN_RENAME` probe, and
  the per-file-system fallback to inotify); NFC canonicalization on macOS;
  `Default()` (fanotify then inotify on Linux, FSEvents on macOS, and
  `ErrUnavailable` where the pin would fall back to kqueue).
- **Witnesses:** the pinned tests, ported: `watcher_test.go` 83 (run for each
  available backend), `eventlist_test.go` 7, `walkdir_test.go` 8,
  `fallback_test.go` 5, `fanotify_linux_test.go` 8, the 12 FSEvents tests.
  macOS on this host; Linux in CI (decision 9).
- **Contracts (`x4-contracts`):** closing every watch returns the thread and
  descriptor counts to their starting values; a watch closed inside its own
  callback; a callback that panics does not take the backend down; dropping
  the watcher closes it.
- **Exit:** the roster's `fswatch` tests pass on both hosts, each skipped
  test with the pin's own skip condition; `x4-contracts` receipted; `P4B-X4`.

### X5 — watch mode

- **Exists:** X2, X3; `CachedFs`; wildcard directories.
- **Build:**
  - *`watchmanager`* (in `tsr_execute`): `WatchManager` (the lock, draining
    events, overflow, the cycle signal, terminated watches,
    `ResolveDesiredDirs` with the ancestor fallback, `ReconcileWatches`,
    `DirWatchSet`, `RunLoop`); the `WatchBackend` trait with the `fswatch`
    backend and the testing injection; `ShouldIgnoreWatchPath`,
    `CanWatchDirectory`, `PerceivedOsRootLengthForWatching`.
  - *`Watcher`* (`execute/watcher.go`): `start`, `DoCycle` (the config
    recheck, the relevant-change filter, eviction of changed source files,
    the single-file fast path, the full rebuild), `computeDesiredWatches`,
    the caching compiler host, the watch status reporter with screen
    clearing, the mapper project's replacement and manifest changes.
  - *Program reuse.* `Program.ReuseProgram` (C7's handoff, with its
    content-mapped branch), `canReplaceFileInProgram`,
    `needsImportHelpersImportSpecifier`, the four `equal*` helpers,
    `lazyValue.tryReuse`, `FilesByPath`, `updateFileIncludeProcessor`. The
    reused program shares the unchanged files' owners (ADR 0006) and gets a
    new checker pool; the old program is released when the cycle ends.
  - *Build watch.* `Orchestrator.Watch` and `DoCycle`,
    `checkTasksForEventChanges`, `computeDesiredWatches`, the package.json
    lookups, `updateWatch`, `resetCaches`, graph regeneration reusing old
    tasks.
  - The dynamic-verbatim fake and `NewSpawnerWithProjectLifecycle` (C7's
    handoff) in `tsr_contentmappertest`.
- **Witnesses:** the seven `DirWatchSet` tests; the 42 `tscWatch` and 65
  `tsbuildWatch` baselines with their 292 edit steps and `WatchState`
  sections; the eight watch-side content-mapper lifecycle tests; the two race
  tests as concurrent stress tests (X7 runs them under the sanitizer).
- **Contracts (`x5-contracts`):** after 50 cycles over one project the owner
  and allocation counters are where they were after the second (E3's
  shape); a fast-path cycle and a full rebuild of the same edit give the
  same diagnostics and outputs; an overflow forces a full rebuild; a cycle
  canceled mid-check leaves the next cycle correct.
- **Exit:** every X5 scenario matches whole; `x5-contracts` receipted;
  `P4B-X5`.

### X6 — tracing, statistics and profiling

- **Exists:** `TraceSink` with the checker's and the emitter's pushes; X1.
- **Build:** crate `tsr_tracing`: `StartTracing` (the header events, the
  process and thread names), instants and duration events with the pin's
  separate or complete forms, flushing, stable thread ids by key, a type
  tracer per checker (`RecordType`, `DumpTypes` with the pin's type
  descriptors), `StopTracing` (the legend), the deterministic mode, all
  written through the system's file system; `startTracingIfNeeded` and
  `stopTracing` with their warnings; `Program.Tracing`. `statistics.go`: the
  table, durations, the content-mapper rows, aggregation for build mode, and
  the five counters (`LineCount`, `IdentifierCount`, `SymbolCount`,
  `TypeCount`, `InstantiationCount`). Profiling by decision 6.
- **Witnesses:** the two `generateTrace` baselines (the trace, types and
  legend files under `--singleThreaded`); `tracing_test.go`'s two tests. The
  statistics block is never in a baseline (the harness drops it), so its
  format is witnessed by a native observation of `Statistics.Report` on
  fixed inputs, taken with an overlay test.
- **Exit:** the two scenarios match whole; the statistics observation
  matches; `P4B-X6`.

### X7 — live systems and closure

- **Determinism.** The 517 scenarios run five times; every run gives the
  same bytes (section 2, the test-program modes).
- **The ThreadSanitizer run.** The harness suite, the race tests, the
  `fswatch` tests and the unit tests of the Phase 4 crates, built with the
  pinned nightly, `-Zsanitizer=thread` and `-Zbuild-std`, as E3's
  AddressSanitizer run is (decision 8). Zero reports.
- **The smoke test.** The Rust binary on `testdata/fixtures/compiler`
  (upstream's copy of the TypeScript compiler sources, about 148,000 lines)
  with `--noEmit --singleThreaded` and with `--noEmit`,
  as upstream's CI job runs the Go binary; the exit status and the output
  equal the Go binary's on the same host.
- **The live-watch witness.** Scripted edit sequences in a real directory
  against both binaries under `--watch` and `-b --watch`: a modified file, a
  new file that resolves a failed import, a deleted file, a rename, a
  config edit, an extended-config edit, a package.json edit under
  `node_modules`, a directory that does not exist yet, a symlinked root, a
  burst of edits. After the output settles, the sanitized last status, the
  diagnostics and the written files are compared; the number of
  intermediate rebuilds is not.
- **The build-info interoperability witness.** For one scenario per
  build-state family, on a real directory: the Go binary runs the first
  steps and the Rust binary the rest, and the reverse; the final files and
  output equal the all-Go run's. This is PLAN's "both binaries can share
  build state".
- **The binary.** Build release target `tsrust`, staged as `lib/tsc` (ADR 0002); on Linux
  the ELF's versioned symbols checked against glibc 2.28 (decision 11).
- **Phase 2's ownership contracts** through the command-line paths (C7's
  "E3 contracts through the compiler pool"): a compile, an incremental
  compile, a build and a watch session each return the owner counters to
  their starting values.
- **Dispositions.** Every function of the Phase 4 files `mapped`,
  `equivalent` with a site or `later` with an owner; none `gap`.
- **The record.** `docs/PHASE4.md` from `data/phase4/report.json`: the counts
  by family, the witnesses, the dispositions, what Phases 5 to 7 can
  consume, and the measured risks Phase 7 owns.
- **The green-up.** `checker`, `emit` and `tsc` re-recorded on the final
  sources, with every other producer the phase staled.
- **Exit:** the P4B exit of section 5.

## 5. Acceptance and evidence design

Namespace: sprints `P4A` (stage A, X0) and `P4B` (stage B, X1 to X7); data
under `data/phase4/`; scripts `scripts/phase4_*.py` over the phase2
libraries; one producer, `tsc`, with its inputs and sources declared in
`status/runs.toml`; contract witnesses `x1-contracts` to `x5-contracts` with
receipts as Phase 2's.

The implemented X7 witness commands, external capture index and replay rules
are documented in [PHASE4-acceptance.md](PHASE4-acceptance.md). Wiring a runner
does not mark its metric passed; current authenticated executions are required.

| Required claim | Evidence and denominator | Reuse |
| --- | --- | --- |
| Every command-line baseline matches | The 517 rendered transcripts against the committed references, whole text: `tsc` 218, `tsbuild` 192, `tscWatch` 42, `tsbuildWatch` 65; sections compared for attribution | The ported harness over the recorded scenarios |
| The recording is faithful | X0's three checks: the references reproduced, a second recording identical, the replay through the pin's own runner | The overlay pattern |
| Incremental builds are correct | The harness's own comparison at each of the 1,255 edit steps, the incremental run against a clean build of the same state: no difference except in the 7 baselines that record an explanation, whose text must match | The pin's `getDiffForIncremental` |
| `.tsbuildinfo` is the pin's | The codec over the 1,271 texts and 1,257 readable renderings; the interoperability witness in both directions | The baselines themselves; the `[oracle]` binary |
| The pinned tests hold | The roster: 146 unit tests and 14 direct tests, each ported, host-conditioned or not applicable with its reason | None |
| The native watcher behaves as the pin's | The `fswatch` roster on macOS and Linux; the live-watch witness against the Go binary | The `[oracle]` binary |
| The smoke test passes | Both thread configurations on the pinned fixture, status and output equal to the Go binary's | Upstream's CI job |
| No data race | The ThreadSanitizer run, zero reports | E3's instrumented-build pattern |
| The results do not depend on scheduling | Five runs of the 517 give the same bytes | C6's modes comparison |
| Phase 2's and Phase 3's gates are preserved | The `run.checker.*` and `run.emit.*` exit metrics on the final sources, in both test-program modes | Their producers, re-recorded at X7 |
| Ownership, cancellation, panics | The contracts of X1 to X5 and X7's ownership repeat | The E3 and C6 contract shapes |
| Function disposition | Every function of the Phase 4 files left after decision 4: `mapped`, `equivalent` with a site or `later` with an owner; none `gap` at X7 | `scripts/phase2_audit.py`'s rules, a Phase 4 scope |

Producer metrics (`run.tsc.*`): `inventory_frozen`, `inventory_verified`,
`harness_valid`, `result_recorded`, `blockers_named`, `unsupported_required`,
`baseline_parity` (raw equality), `baseline_accepted` (raw matches plus exact
owner-approved pairs), `incremental_correctness`, `buildinfo_codec`,
`buildinfo_interop`, `unit_rosters`, `watcher_tests`, `live_watch_parity`,
`smoke`, `thread_sanitizer`, `determinism`, `residuals`, `dispositions`,
`evidence_current`, `report`, and per checkpoint `xN_complete` bound to
`P4B-XN` as recorded facts (C7.7's tracker extension). The P4B exit is:

```text
sprint.P4A.done == 1
sprint.P3B.done == 1
run.tsc.harness_valid == true
run.tsc.baseline_accepted == 1
run.tsc.incremental_correctness == 1
run.tsc.buildinfo_codec == 1
run.tsc.buildinfo_interop == true
run.tsc.unit_rosters == 1
run.tsc.watcher_tests == true
run.tsc.live_watch_parity == true
run.tsc.smoke == true
run.tsc.thread_sanitizer == true
run.tsc.determinism == true
run.tsc.unsupported_required == 0
run.tsc.residuals == 0
run.checker.errors_parity == 1 (and the six other Phase 2 metrics)
run.emit.output_parity == 1 (and the other Phase 3 exit metrics)
```

The watcher metrics are host facts. The tracker's gates read the current
host, so a recorded run on this host witnesses FSEvents only; the Linux
backends are witnessed by CI's Linux job, which runs the same producer and
enforces the same metrics with `check-metrics` (decision 9).

No threshold is introduced. A retained difference passes only with an
owner-approved entry in `data/divergences.toml` (ADR 0004), which needs an
exact observation witness per scenario; failures, missing operations and
unexecuted rows cannot be waived.

Every recorded fact keeps the Phase 2 discipline: the recorded scenarios
bound to the pin and to the patch digests; results bound to the scenario
digests, the executable and the source closure; `replay` before recording;
the receipts binding the contract sources; the residual list and the
register rebuilt from evidence, never edited. `cargo xtask run tsc` and
`status --record` remain the owner's.

## 6. Cost control and performance risk

- **Runs.** A scenario runs its first command and, for every edit, the
  command and a clean-build shadow: 517 + 2 × 1,255 = 3,027 command
  executions per full run, each over a few small files and the harness's
  twenty-line library. Expected well under the corpus run's five minutes;
  measured at X0. No sample is needed: one scenario or one family during
  work, the whole suite at every checkpoint exit.
- **The recorder.** One run of the pinned suite to record, and two more to
  verify. It reruns only if the patch changes.
- **Real-time tests.** The watcher's tests wait on real events and a debounce
  of 50 to 500 ms, 83 of them once per backend: minutes. They run at X4's
  exit and at X7, not on every change. The live witnesses are a dozen
  scripted sessions per binary.
- **The sanitizer.** A nightly build of the standard library and roughly an
  order of magnitude in run time: once at X7 and in a scheduled CI job
  (decision 8).
- **Threads.** `tsc -b` runs four builders, and each builder's program has its
  own bounded work groups and checker pool. The bound is per group, so the
  worst case is the builders times `worker_bound()` threads on reserved
  256 MiB stacks, virtual and committed lazily. X3 measures the live thread
  count on the `sample` scenarios and X7 on the smoke fixture; a global
  bound is added only if a measurement calls for it.
- **Watch memory.** A cycle holds the old and the new program until the old
  one is released; the fast path shares the unchanged files' owners. X5's
  contract measures the steady state.
- **Performance.** Correctness first; no speed or memory gate. One bounded
  capture at X7: the smoke fixture in both thread configurations, wall time
  and peak RSS beside the Go binary's on the same host (decision 13). It is
  the first full-program command-line measurement; the recorded Phase 2
  figures (E6 at 1.153 and 1.399 of Go's wall time at one and eight threads)
  were taken on partial checking. Phase 7 owns the budgets.

## 7. Risks

| Risk | Where it shows | Mitigation |
| --- | --- | --- |
| Phase 3 slips, or its emit differs on these programs | 420 scenarios stay `unsupported` or differ inside an emitted file | The register names Phase 3 as the owner; the sections emit does not feed are reported from X1 on; a difference inside an emitted file is routed to Phase 3 as its residual |
| Build-info bytes: field order, omitted zero values, number forms, option order, relative paths, the hash's input bytes ([ADR 0013](adr/0013-source-bytes--javascript-strings-and-wire-positions.md)) | Every scenario with build state | The codec witness over 1,271 real texts, before any program logic lands |
| The fake clock: every reading advances it, and modification-time order decides a project's status | `tsbuild` and `tsbuildWatch` status lines | The recorder logs primitive operations, so the clock ticks as the pin's; the driver reads the clock at the pin's call sites; a contract compares the reading count on three scenarios with a native observation |
| Parallel builders: output order, upstream waits, a failed task | A `tsbuild` transcript that changes between runs; a hang | Per-task buffers and ordered reports ported as written; X3's 20-run, failure and cancellation contracts |
| File-system event semantics differ by platform: FSEvents coalescing, inotify's per-directory watches, fanotify's availability and file-system support, Unicode normalization | X4's tests and X7's live witness | The backends are ported, not substituted; the pinned tests run for each backend on both hosts; the live witness compares the settled state, never the number of events or rebuilds |
| fanotify is unavailable on a runner | Linux tests skip | The pin's own probe and skip conditions; the roster records which tests ran on which host; inotify always runs |
| The sanitizer needs an instrumented standard library and a compatible allocator | A failing build or false reports | `-Zbuild-std`, as the AddressSanitizer run; the system allocator under the sanitizer |
| A harness defect hides or invents a difference | Any row | X0's replay check and mutation checks; a harness defect is a separate outcome from a product gap |
| Checker assignment reaches the output (C6's per-architecture partition) | A row that differs by host | X0 first reproduces all 517 references with the pin on this host; the determinism runs at X7 |
| Help and `--init` text depends on option metadata, locale tables and width rules | The help, `--init` and locale scenarios | The generator reads the pin's declarations; these are among the first rows X1 matches |
| A child process outlives the compiler | Leaked mapper processes | The pinned close test and X1's kill contract |
| Staleness | Every recorded run reads `stale` while Phase 4 changes shared crates and one generator | Accepted mid-phase (decision 14); X7's green-up |

## 8. Owner decisions

Fifteen proposals. On 2026-10-02 the owner decided fourteen of them; each
entry keeps its proposal and records the outcome. Decision 7 (dependencies) was resolved during the X0 review; all fifteen decisions are now recorded.

1. **Names.** Checkpoints X0 to X7, sprints `P4A` and `P4B`, producer `tsc`,
   data under `data/phase4/`.
   **Confirmed.**
2. **The scenarios are recorded from the pin**, by an overlay over the pinned
   test package, and verified by reproduction and replay (X0). The committed
   references are the authority, so baseline parity needs no native capture.
   The alternative, transcribing the 380 declarations and their closures by
   hand, is more work and cannot be proven faithful.
   **Confirmed.**
3. **Ordering against Phase 3.** X1, X2, X3 and X5 cannot close before
   Phase 3's T8, and the P4B exit requires P3B. Proposed for one implementer:
   Phase 3 first; X0, X4 and the emit-free part of X1 may be taken while a
   Phase 3 checkpoint waits on review; X2, X3 and X5 after T8. The
   alternative is to start Phase 4 only after P3B closes.
   **Settled by events:** Phase 3 is merged (#77, #78), so Phase 4 starts
   after Phase 3's green-up with no interleaving.
4. **Ledger moves.** `fswatch/kqueue.go` to out of scope (ADR 0002 already
   excludes it); `compiler/projectreferencedtsfakinghost.go` to Phase 5;
   `GetDiagnosticsOfAnyProgram`'s destination from Phase 3 to Phase 4;
   `pprof/pprof.go` by decision 6. One regeneration, together with Phase 3's
   decision 1 if its T0 has not run.
   **Confirmed, without the `GetDiagnosticsOfAnyProgram` move:** Phase 3
   ported and marked it, so it stays Phase 3's. `pprof.go` moves to Phase 7
   (decision 6).
5. **Crates.** New `tsr_execute` (with `execute/tsc` and `watchmanager`, as
   the ledger maps them), `tsr_incremental`, `tsr_build`, `tsr_fswatch`,
   `tsr_tracing` and binary crate `tsr` (target `tsrust`); the harness under
   `tools/phase4/tsctests`, repository-only. The diagnostic writer stays in
   `tsr_compiler::diagnostic_writer` although the crate map names a separate
   `tsr_diagnosticwriter`, and the record notes the deviation. New crates
   register in `tools/packaging/packages.json` as unpublished; whether they
   join the next lockstep release, and the name the command line is
   published under, are separate decisions.
   **Confirmed, with the names already reserved:** the crates keep the
   `tsr_` prefix (`tsr_incremental` exists since Phase 3 and is registered
   as published), and the command line is published under the reserved
   names, the `tsr` crate on crates.io and the `tsrust` organisation on npm,
   not as a crate called `tsc`. The installed command is `tsrust`: two
   unrelated npm packages already install a command called `tsr`. The
   workspace's binary target is named `tsrust` too: `cargo install` installs
   a binary under its target name, and a target called `tsc` would collide
   with TypeScript's own `tsc` on a user's path.
6. **Profiling.** `--pprofDir` is accepted and reports that profiling is not
   available in this build; `pprof.go` moves to Phase 7, beside the
   benchmarking work, and no `tsr_pprof` crate is created now. The
   statistics table's memory row reports the allocator's counter where Go
   reports its runtime's; the row is never in a baseline. The alternative is
   a sampling-profiler dependency now.
   **Confirmed:** profiling is deferred to Phase 7.
7. **Dependencies** ([ADR 0017](adr/0017-dependency-policy.md)).
   **Accepted with amendments, 2026-10-02:** use the existing `rustix` for
   inotify, directory iteration, polling, pipes, process operations and
   terminal queries (features `fs`, `event`, `pipe`, `process`, `termios`).
   Direct `libc` is limited to missing fanotify/file-handle operations and
   Unix signal registration, and `localtime_r` for the pinned local watch-clock
   formatting (Rust std has no local-time conversion). Directory iteration
   does not require libc.
   On macOS, use feature-limited `objc2-core-services`,
   `objc2-core-foundation` and `dispatch2` bindings, replacing the original
   handwritten framework-declaration proposal. These supply native APIs and
   ownership wrappers; the pinned watcher algorithm remains ours. Their
   selected dependency closure must satisfy ADR 0017. `xxhash-rust` is
   already used by `tsr_incremental`. No generic watcher crate (PLAN).
   The signal handler only notifies normal execution, which cancels the
   context. Restore prior handlers when the scope ends, matching Go's
   deferred `NotifyContext.stop`; do not restore after the first signal.

   A shared `tsr_tsc` crate owns Go's `execute/tsc` contracts and compilation
   helpers. Both `tsr_execute` and `tsr_build` consume it, avoiding a circular
   dependency between the driver and build orchestrator. It remains
   unpublished along with the other Phase 4 crates until a release decision.
8. **The ThreadSanitizer run.** The pinned nightly with `-Zbuild-std` and the
   system allocator, over the harness suite, the race tests and the Phase 4
   crates' tests; recorded at X7 on this host and run by a scheduled CI job
   on Linux, not on every pull request.
   **Confirmed.**
9. **Linux evidence.** inotify and fanotify can be witnessed only on Linux.
   The `fswatch` roster, the live-watch witness and the smoke test run in
   CI's Linux job, which enforces their metrics; the recorded run on this
   host covers FSEvents. If a recorded Linux run is wanted, it joins as a
   second provenance.
   **Confirmed, with a recorded Linux run:** the owner takes it in a Linux
   container on this host or in the cloud, and it joins as a second
   provenance. fanotify needs a privileged container.
10. **The live witnesses.** The smoke test, the live-watch witness and the
    interoperability witness compare the Rust binary with the pinned Go
    binary on the same host at X7. They are observations the `tsc` producer
    takes, not committed captures.
    **Confirmed.**
11. **The binary.** X7 stages the release binary as `lib/tsc` and, on Linux,
    checks the ELF's versioned symbols against glibc 2.28 in CI: the first
    native binary is the place for PLAN's item 15. The run on a glibc 2.28
    image, the second Linux architecture, the size budget and cut-over stay
    Phase 7's.
    **Confirmed, with the target renamed:** the release binary is built as
    `tsrust` (decision 5). X7 copies it to `lib/tsc` only as the staging name
    that mirrors upstream's package layout; what upstream's launcher expects
    there is confirmed at the Phase 7 cut-over.
12. **`--lsp` and `--api`.** The binary recognizes both; until Phases 5 and 6
    supply the servers it says the mode is not available and exits with
    `NotImplemented` (5). `runLSP`, `runAPI`, the parent-process watchdog and
    `isProcessAlive` are `later` with those owners.
    **Confirmed.**
13. **Performance.** One bounded timing capture at X7 on the smoke fixture,
    beside Go's, no threshold.
    **Confirmed.**
14. **Evidence.** Phase 4's changes stale the recorded `checker` and `emit`
    runs and the producers that bind the option-declaration generator.
    Nothing is re-recorded per fix; X7's green-up re-records them, and the
    recordings are the owner's.
15. **Parse/bind trace order. Accepted, 2026-10-02.** Retain S07's exclusive
    eager binding: Rust binds during source loading, while Go binds later.
    The two `generateTrace` scenarios retain their raw `different` results.
    `data/phase4/approved-differences.json` names the exact native/Rust
    transcript hashes accepted by the owner. Type dumps, legends, diagnostics,
    emitted output and subsequent check/emit events match byte for byte.
    No generic trace normalization or future byte difference is approved.
    `baseline_parity` remains the raw ratio; `baseline_accepted` is the exit
    criterion including only those reviewed pairs. See `PHASE4-X6.md`.
    **Confirmed.**
15. **The mapper child process** is ported here with `cmd/tsc/sys.go`,
    amending C7's record, which named Phase 5.
    **Confirmed.**

X0 starts on this plan once the decisions are settled; the X0 record
(`docs/PHASE4-X0.md`) carries the measured costs, the first run and the
register.
