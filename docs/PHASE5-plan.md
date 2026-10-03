# Phase 5: language service, project system, LSP server

Status: **proposed for owner review** (2026-10-03).
The detailed implementation plan for [PLAN Phase 5](../PLAN.md#phase-5-language-service-project-system-lsp-server).
Its checkpoints carry their own work items, witnesses and exit checks;
production implementation starts at L0.

Planning reference: `main` at `e3d7d435` (Phase 4 merged, #81). Upstream
remains Corsa `1f70213d4922b434345f639b441681e470c7cfc1`.

## 1. Outcome and starting point

Deliver the pinned language server as the `--lsp` mode of the native binary:
the project system (snapshots, inferred and configured projects, project
references, automatic type acquisition, the overlay file system, watchers),
the language service in full (completions and auto-imports, hover,
definitions, references, rename, signature help, diagnostics, code actions
and fixes, organize imports, inlay hints, call hierarchy, code lens, folding,
selection ranges, semantic tokens, linked editing, symbols, JSDoc and
formatting), and the LSP 3.17 server with its request queue, progress,
cancellation, logging and watcher registration. This is PLAN's Phase 5 gate:
"At least 99.5 percent fourslash with a triaged allow-list; project and LSP
suites; synchronous filesystem bridge cancellation/progress with blocked
workers; replay corpus; no request-latency regression against Go on the
benchmarking editor scenarios."

The starting point is Phase 4's exit: programs load, check and emit; the
checker pool, cancellation and generation retirement exist; the command line,
incremental programs, the build orchestrator and the native watcher exist;
the Phase 1 test-host transport (S11) carries file-system callbacks, options
and mapper streams over JSON-RPC but runs no compiler behind them.

### What exists

| Asset | Where | State |
| --- | --- | --- |
| The formatter | `tsr_format`: the 10 `format` files | S09-3: 235 functions ported, 16,120 of 16,120 corpus inputs in parity on position, insert, format, entry, indent and navigation; its `_test.go` tests ported. The ledger still reads `in-progress` for its files |
| Request-scoped printing and insertion formatting | `tsr_api::printing`, `tsr_api::formatting` | S09-4/5: the scratch arenas and their disposal contracts (`api_scratch_disposal`) |
| The test-host transport | `tsr_testhost` (`--stdio`, `serve`, `Session`; framing, wire version 2, `callbackFS` semantics, options staging, mapper stream tunnel) | S11, ADR 0019: proves input continues while callbacks and streams wait; does **not** prove the synchronous file-system bridge from a blocked worker, parse-cache injection or real project options |
| Project retention | `tsr_project::retention` | The S09/E3 ownership contracts for retained snapshots and bundles; no project system |
| `lsutil` fragments | `tsr_ast`/`tsr_format` carry `asi.go` (7), `children.go` (5), `completednode.go` (4) and two `lsconv` functions | Ported for the formatter; markers in place |
| Programs, checking, emit, incremental, watch | Phases 2 to 4 | `Program::load`, `CheckedProgram`, `CompilerCheckerPool`, `CancellationToken`, `CheckedProgram::emit`, `tsr_incremental`, `tsr_fswatch`, the watch manager (`tsr_tsc::watchmanager`), the mapper child process (`tsr::process`), `--lsp` and `--api` recognized by `tsrust` and refused with `NotImplemented` (Phase 4 decision 12) |
| Source maps | `tsr_sourcemap` (T2) | The decoder and the document position mapper the service's `sourcedefinition.go` reads |
| Content mappers | `tsr_contentmapper`, `tsr_ipc`, `tsr_jsonrpc`, `tsr_contentmappertest` | The host, the in-process fakes, and since Phase 4 the child-process spawner |
| The pinned harnesses | `internal/fourslash` (5 files, 8,019 lines), `testutil/lsptestutil` (the in-process LSP client, 15 functions), `testutil/projecttestutil` (48 functions, with generated client and npm mocks) | Go; drive the Go server in process |

### The acceptance corpus

| Suite | Count | Form |
| --- | --- | --- |
| fourslash | 4,355 test files, 4,548 test functions under `internal/fourslash/tests` (PLAN's 4,356 counts the files); 1,749 committed baselines under `testdata/baselines/reference/fourslash` | Go test bodies that open files, move to markers, edit, and verify a result (a list of completions, a quick-info text, a baseline file) against the in-process server |
| project | 85 tests in `internal/project` | Direct assertions on sessions, snapshots, projects, watches, caches and the config registry, with mocked client and npm |
| lsp | 30 tests in `internal/lsp` | The server over the in-process client: completions, progress, project info, project-reference updates, semantic tokens, content mappers, the dynamic queue, the stack sanitizer, replay |
| ls and helpers | 8 `ls`, 12 `autoimport`, 12 `lsutil`, 1 `change`, 7 `format` | Unit tests |
| replay | `lsp/replay_test.go` with `-replay FILE` | A recorded editor session replayed against a server; no file is committed |

The fourslash test bodies weigh the features. By the primary verifier of each
test file: completions 1,111 (plus 231 import fixes and 27 JSDoc completions),
find-all-references 362, quick info and hover 533, code fixes 270, go to
definition 225 (plus 69 implementation and 38 source definition), formatting
164, rename 148, document highlights 145, document symbols 84, signature help
82, inlay hints 64, organize imports 62, semantic tokens 45, call hierarchy
39, selection ranges 37, suggestion diagnostics 37, diagnostics counts 65,
code lens 8, linked editing 3; 431 files verify through other operations or
through edits (`Insert`, `Backspace`, `FormatDocument` with
`VerifyCurrentFileContent`). 162 tests call `MarkTestAsStradaServer`, which
relaxes one definition assertion; the Rust server is held to Corsa's committed
baselines, not Strada's.

### By the numbers

`PORTS.toml` assigns 130 files to Phase 5: 122 source, 5 harness
(`fourslash`), 3 generated (`lsp_generated.go`, `project_stringer_generated.go`,
`export_stringer_generated.go`); 88,466 lines; 114 `planned`, 15
`in-progress` (the formatter and `lsutil`), 1 `ported`. By package:

| Package | Files | Lines | Functions | Rust today |
| --- | ---: | ---: | ---: | --- |
| `ls` | 39 | 31,063 | 1,035 | none |
| `lsp/lsproto` | 6 | 18,325 | 857 (generated) | none |
| `project` (+ `ata`, `dirty`, `logging`, `background`) | 39 | 12,361 | 555 | retention contracts |
| `fourslash` | 5 | 8,019 | 306 | none |
| `ls/autoimport` | 11 | 5,400 | 182 | none |
| `format` | 10 | 4,259 | 235 | ported (S09-3) |
| `lsp` (+ `lspwatcher`) | 6 | 3,734 | 174 | none |
| `ls/lsutil`, `ls/change`, `ls/lsconv` | 13 | 5,073 | 197 | 18 functions |
| `compiler/projectreferencedtsfakinghost.go` | 1 | 232 | 20 | none (Phase 4 decision 4) |

About 3,560 functions, about 250 of them ported. Against Phase 4's 22,849 lines
this is 3.9 times the Go volume, with one block, `ls`, larger than all of
Phase 4.

### Gaps confirmed by source inspection

- No project system: no snapshot, overlay FS, config registry, inferred
  project, project collection, parse cache, program counter, owner cache,
  dirty maps, background queue, logger or automatic type acquisition.
- No server: no `lsp.Server`, dynamic queue, progress, stack sanitizer,
  `lsproto` types, JSON-RPC over the LSP base protocol (S11's framing is a
  compatible prototype in `tsr_testhost`, not `lsproto`'s), watcher
  registration or converters.
- No language service beyond the formatter.
- The test-host bridge that lets a worker blocked in a synchronous file-system
  call issue a reverse request while the router keeps pumping (ADR 0019,
  decision 3) is not proven.
- Nothing drives the pinned fourslash suite against a Rust server.

## 2. Scope and phase boundaries

| Area | Phase 5 responsibility | Boundary |
| --- | --- | --- |
| `lsproto` | The LSP 3.17 types generated from the pinned meta model by a Rust generator (ADR 0015), `baseproto.go` framing, `jsonrpc.go`, `structcodec.go` | Phase 6's `api` protocol is separate; S11's framing is retired in favour of `lsproto`'s, keeping its contract tests |
| `project` | Complete: `Session`, `Snapshot`, `SnapshotFS`, the overlay FS, the config-file registry and its builder, the project collection and its builder, `Project`, program counter, owner cache, parse cache, ref-count cache, file changes, watches with timeouts, the project checker pool, `dirty`, `background`, `logging`, `ata` | The compiler's `CheckedProgram` and pool are Phase 2's; `project/api.go` is ported here and exercised by Phase 6 |
| `ls` | Complete: every feature file, `lsutil`, `lsconv`, `change`, `autoimport` | `format` is S09's and is consumed; `ls/api.go` is ported here and exercised by Phase 6 |
| `lsp` | `server.go`, `dynamic_queue.go`, `progress.go`, `logger.go`, `stack_sanitizer.go`, `lspwatcher` | `--lsp` dispatch in `tsrust` exists (Phase 4); `runLSP`, the parent-process watchdog and `isProcessAlive` are ported here (Phase 4 left them `later` with this owner) |
| The compiler remainder | `projectreferencedtsfakinghost.go` (`UseSourceOfProjectReference`, set only by `project/project.go`), the 19 `program.go` functions Phase 4's audit left to Phase 5 (project references, unresolved imports, package names, lib-file lookups) and the mapper's two | The rest of `program.go` is Phase 4's |
| The test hosts | The carried harness patch, `tsrust --lsp`'s test-host extension, the project-test mock surface | The pinned Go suites stay the executable specification; no Rust rewrite of their bodies (PLAN) |
| Acceptance | The fourslash run through the patch, the project and LSP suites, the replay corpus, the bridge contracts, the latency scenarios, the audit | The JS API suites are Phase 6's; the benchmark budgets are Phase 7's |

Six boundaries need explicit settling:

**How the fourslash suite drives Rust.** The pinned harness
(`fourslash.go`) builds `lsp.ServerOptions{FS, DefaultLibraryPath, ParseCache,
Spawn}` and hands them to `lsptestutil.NewLSPClient`, which runs
`lsp.NewServer` in process over an in-memory pipe. Its test bodies are 4,548 Go
functions with closures, so recording them as Phase 4 recorded `tsctests`
is not an option. The plan's seam is the one PLAN and ADR 0019 reserved: a
carried patch of `lsptestutil` (and the few `fourslash.go` lines that set the
server options) that connects the client to a `tsrust --lsp --test-host`
process over stdio instead of an in-process server, serving the test's file
system through S11's `callbackFS` callbacks, its options through
`test/initialize`, and its content-mapper fakes through the stream tunnel. The
Go harness keeps asserting; the Rust server answers (decision 2).

**The parse cache.** The Go harness shares one `project.ParseCache` across
every test in the package, so the bundled libraries parse once per process.
A Rust server started per test would parse them 4,548 times. The patch
therefore keeps one Rust server per Go test binary and ends each test with a
`test/reset` that discards the session (projects, snapshots, open files,
watches) and keeps the parse cache, as the Go variable does. `reset` is the
test-host protocol's one Phase 5 addition (decision 3).

**The synchronous bridge.** Go's project system calls its file system
synchronously from worker goroutines; under the harness those calls become
reverse requests to the Go client. S11 proved the router pumps while
asynchronous callbacks wait and left the blocked-worker form to Phase 5. L1
builds it: a worker blocked in `ReadFile` parks on a reply slot while the
single-threaded router keeps reading, cancellation reaches the parked worker,
and progress keeps flowing. Its contracts are the first L1 witness.

**Automatic type acquisition** runs npm as a child process. The pinned tests
mock it (`npmexecutormock_generated.go`); production uses the real executor
through Phase 4's process module. No test or producer touches the network
(decision 8).

**The `fourslash` package itself.** The ledger maps its five files to a crate
`tsr_fourslash`. With the pinned harness driving, no Rust fourslash runner is
needed for the gate. The test-data parser, baseline utilities and
state-baseline writer (306 functions) are recorded `later` with Phase 7 as the
owner, which decides whether a Rust-side runner is wanted for the dogfood
period (decision 9); the ledger's crate name stays reserved.

**Strada-only behaviour.** `MarkTestAsStradaServer` and the triage files
(`testdata/submoduleAccepted.txt`, 1,539 lines; `submoduleTriaged.txt`)
describe where Corsa differs from Strada. They do not change the Rust gate:
the committed references are Corsa's, and the Rust server matches Corsa. A
Corsa behaviour the owner does not want reproduced is a divergence entry,
never a silent allow-list item (decision 6).

## 3. Prerequisites and coordination

Phase 5 consumes these contracts as they are, through their public entry
points. A missing operation among them is a named joint blocker with an
owner, not a Phase 5 patch:

| Contract | Owner | Phase 5 use |
| --- | --- | --- |
| `Program::load`, `CheckedProgram`, `CheckerRequest`, `CompilerCheckerPool`, generation retirement, `CancellationToken` | Phase 2 | Every project's program and checker; the project pool is built over them; a request's cancellation token |
| `CheckedProgram::emit`, `EmitOnly`, the write callback | Phase 3 | `getEmitOutput`, the API's emit (Phase 6), source-definition's declaration maps |
| `tsr_sourcemap` decoder and document position mapper | Phase 3 | `sourcedefinition.go`, `source_map.go` |
| `tsr_incremental` | Phase 3/4 | Not consumed by the server (the pin's language server does not read build info); listed to say so |
| The watch manager and `tsr_fswatch`, the mapper child process, `System`, the signal scope, `--lsp` dispatch | Phase 4 | `lspwatcher`, `project/watch.go`, `runLSP`, content mappers in the server |
| `ParseCommandLine`, `GetParsedCommandLineOfConfigFile`, the extended-config cache, wildcard directories, `ConvertToTSConfig` | Phase 1/4 | The config registry, `--showConfig` through the API, project options |
| `tsr_format`, `tsr_api::printing`/`formatting` | S09 | Formatting requests, code-action text, the API's printing (Phase 6) |
| `tsr_testhost` wire version 2: framing, `callbackFS`, options staging, streams | S11 (ADR 0019) | The test-host extension of `tsrust --lsp`; its contract tests keep passing |
| `tsr_contentmapper` host and spawners | Phase 2 (C7.8), Phase 4 | Mapper projects in the server, the fakes the fourslash and LSP tests name |
| The phase2 comparison, register and producer libraries | Phase 2 (C0) | Reused by the Phase 5 scripts, never edited |

Coordination:

- **Phase 4's green-up first.** Every recorded run is stale after #81; the
  re-freezes and re-recordings (`docs/PHASE4-X0.md`, the X7 witnesses) are the
  owner's and precede L0's own recordings. L0 can start before they finish:
  it adds files and does not touch `crates/**` beyond the new crates.
- **Phase 6 overlaps the tail.** `ls/api.go` and `project/api.go` are Phase
  5's ports; Phase 6 exercises them. Phase 6 can start at L6 against L1's
  project system and L3's read-only features.
- **One ledger regeneration** at L0: `format` and the `lsutil` fragments read
  `ported`, `fourslash` reads `later` (decision 9), the Phase 4 `later`
  functions move to their L checkpoints. Later moves wait for L8.
- **Staleness is expected mid-phase.** Every change under `crates/**` stales
  the recorded `checker`, `emit` and `tsc` runs. Nothing is re-recorded per
  fix; L8's green-up re-records what is stale (decision 12).
- **Recorded inputs are not edited.** The phase1 to phase4 scripts and oracles
  stay as they are; Phase 5 adds `scripts/phase5_*.py` and
  `tools/phase5/**`.
- **The shared checkout.** Phase 5 develops on its own branch; units that
  edit the same crate work on disjoint files and the integrator commits.

## 4. Delivery order

Checkpoints are **L0 to L8** (L for the language service; C, E, F, P, S, T
and X are taken). Each pairs its witnesses with its production work and
verifies the combined result; L0 is preparation only. The order follows the
dependency graph:

```text
L0 ─ L1 ─ L2 ─┬─ L3 ─┐
              ├─ L4 ─┼─ L6 ─ L7 ─ L8
              └─ L5 ─┘
```

L3, L4 and L5 are independent feature blocks over the same server and can run
in parallel. L6 completes the project system (cross-project, type
acquisition, mappers) that only a minority of tests need; L7 is the long
tail; L8 closes.

A checkpoint is complete when every test assigned to it passes or is on the
triaged allow-list. Before a feature exists, its tests read `unsupported` under
the register entry for the checkpoint that ports it, never `failed`.

### L0 — the service acceptance contract

Preparation only; no Rust service parity is claimed.

- **Exists:** the pinned suites; `tsr_testhost` and ADR 0019; the overlay
  pattern of `scripts/phase4_scenarios.py`; the Phase 4 register, comparison
  and producer libraries; the wiring pattern of `[tsc]`.
- **Build:**
  - *`tsr_lsproto`.* The generator (`tools/phase5/lsproto-gen`, a Rust
    program reading the pinned `metaModel.json` the Go `_generate` reads) and
    its output `lsp_generated.rs`, bound to the model's digest; `baseproto`,
    `jsonrpc`, `structcodec`, `util` ported with markers. L0 needs the types
    to speak `initialize`.
  - *The server skeleton.* `tsr_lsp::Server` over `lsproto`: `initialize`,
    `initialized`, `shutdown`, `exit`, document sync accepted, every other
    request answered with a named refusal (`lsp.Server is Phase 5 L2`) as a
    typed error, never a panic; `tsrust --lsp` runs it; `--test-host` adds
    the S11 surface (`test/initialize`, `test/setOptions`, `callbackFS`, the
    mapper stream tunnel) and the new `test/reset`.
  - *The carried harness patch.* `tools/phase5/harness/`: unified diffs of
    `testutil/lsptestutil/lspclient.go` (connect to the Rust process instead
    of `lsp.NewServer`; serve the test's `vfs` through `callbackFS`; forward
    options; tunnel the spawner), of the `fourslash.go` lines that build the
    server options, and of `projecttestutil` where the project tests need the
    same redirection; each diff headed with the pinned file's hash, applied
    by `scripts/phase5_harness.py` into a scratch tree (`go test -overlay`),
    the Rust binary path and the test-host wire carried in the environment.
    One Rust process per Go test binary, `test/reset` between tests.
  - *The inventory.* `scripts/phase5_inventory.py record`: the 4,548 fourslash
    test functions with their file, the verifiers each calls (by the
    `f.Verify*`/edit tally above), the features that implies, the baselines
    each writes (1,749), the `MarkTestAsStradaServer` flag, the global options
    (`@Filename`, `@module`, ...) parsed by the pin's `test_parser.go` (an
    overlay logs them), and the 153 project, lsp, ls, autoimport, lsutil,
    change and format tests with their packages; each test gets its first
    checkpoint from its features. Recorded as `data/phase5/inventory.json.gz`
    with provenance; verified by a second identical recording.
  - *The first run.* The patched suites against the skeleton: every fourslash
    test reads `unsupported` at `lsp.Server is Phase 5 L2`; the Go harness's
    own assertions on connection, initialization and file-system callbacks
    pass, which is the first proof that the bridge carries a real client. The
    result rows are the Go test outcomes (`pass`, `fail` with the first
    assertion message, `unsupported` with the operation, `skip` with the
    reason), parsed from `go test -json`, one per test, never blank.
  - *The comparison and register.* `scripts/phase5_compare.py report`: per
    test the outcome, per baseline the whole-file match against the committed
    reference (the pin writes `local/` copies the harness diffs; the patch
    captures them), the summary by suite, feature and checkpoint; mutation
    checked (a forced assertion failure, a changed reference byte and a
    dropped baseline must each read `fail`/`different`).
    `scripts/phase5_blockers.py`: one entry per cause with owner by feature.
  - *The audit.* `scripts/phase5_audit.py` over the 130 files, seeded with
    `format` and the `lsutil` fragments as `mapped`, `fourslash` as `later`
    (decision 9), the generated files as `generated`, and the Phase 4
    hand-overs.
  - *The bridge contracts.* `tsr_testhost`'s S11 contracts re-run against
    `tsrust --lsp --test-host` so the production endpoint, not the prototype,
    holds them.
  - *Wiring.* Sprints `P5A` and `P5B`, the `lsp` producer in `status/runs.toml`,
    `data/phase5/`, `scripts/tests/test_phase5*.py`, the L0 record
    (`docs/PHASE5-L0.md`), the ledger regeneration.
  - *Cost.* The patched suite's time against the skeleton and the pinned
    suite's own time.
- **Exit:** `run.lsp.inventory_frozen`, `inventory_verified`,
  `harness_valid`, `result_recorded`, `blockers_named`, `bridge_contracts`
  true on a recorded `lsp` run; every test in exactly one category; `P5A-L0`
  passes.

### L1 — the project system

- **Exists:** `Program::load`, the checker pool, retention contracts, the
  config parsing, the watch manager, `tsr_project::retention`.
- **Build:** `tsr_project` complete: `Session` (the request entry: open,
  change, close, `GetLanguageService`, `WaitForBackgroundTasks`), `Snapshot`
  and `SnapshotFS`, the overlay FS over the Phase 1 file systems, the
  config-file registry and builder, the project collection and builder
  (inferred and configured projects, default project selection, project
  references), `Project` with its program, `programcounter`, `ownercache`,
  `parsecache`, `refcountcache`, `filechange`, `watch.go` with timeouts, the
  project `checkerpool` (its disposal of a canceled checker was C7's hand-over),
  `compilerhost`, `snapshothost`, `client.go` (the server callbacks the
  session needs: configuration, watchers, publishing), `dirty` (the
  copy-on-write maps), `background` (the queue), `logging` (collector, logger,
  tree), `extendedconfigcache` (shared with Phase 4's), and the compiler
  hand-overs that projects call. The synchronous bridge: a worker blocked in a
  file-system call parks on its reply slot while the router pumps;
  cancellation unparks it with the token's error; progress is reported while
  it waits.
- **Witnesses:** the 85 project tests through the patched `projecttestutil`
  (their mocked client and npm stay mocks); E3's ownership contracts repeated
  through sessions and snapshots (an opened file's storage is released when
  the snapshot that holds it is dropped; a project's program and checkers are
  released with the project; a mapper bundle is disposed with its project;
  generation retirement after a panic in one snapshot leaves other snapshots
  working) as `l1-contracts`; the bridge contracts (blocked worker, reverse
  request, cancellation, progress) as recorded facts; fourslash tests that
  only open files and verify diagnostics counts (`VerifyNoErrors`,
  `VerifyNumberOfErrorsInCurrentFile`) through the server's publish path.
- **Exit:** the 85 project tests pass; `l1-contracts` current; the bridge
  facts true; `P5B-L1` passes.

### L2 — the server

- **Exists:** L0's skeleton, L1's session, Phase 4's watch manager and
  mapper spawner.
- **Build:** `tsr_lsp` complete: `server.go` (the handler table, request
  dispatch to the session's language service, document sync, diagnostics
  publishing, `workspace/configuration`, watched-file notifications,
  `initialize` capabilities exactly as the pin's), `dynamic_queue.go` (the
  request queue with its cancellation and ordering rules), `progress.go`,
  `logger.go`, `stack_sanitizer.go`, `lspwatcher` over the Phase 4 watch
  manager, `lsconv` (converters, line maps, UTF-16 positions by ADR 0013),
  `runLSP` with the parent-process watchdog and `isProcessAlive` in `tsrust`.
  The service surface is `tsr_ls::LanguageService` with every method present
  and refusing by name until its checkpoint.
- **Witnesses:** the 30 lsp tests through the patch (the dynamic queue,
  progress, project info, project-reference updates, the stack sanitizer, the
  content-mapper server tests, `server_completion_test` once L4 lands); the
  replay runner against a first recorded session (decision 7); fourslash
  tests that only exercise sync and diagnostics.
- **Exit:** the lsp tests pass except those naming an L3 to L6 feature, which
  read `unsupported`; `P5B-L2` passes.

### L3 — read-only features

In fourslash weight order: hover and quick info (533 files; `hover.go`,
`hovericon.go`, `displaypartswriter.go`, `symbol_display.go`), find all
references and document highlights (507; `findallreferences.go`,
`documenthighlights.go`, `importTracker.go`), go to definition,
implementation and source definition (332; `definition.go`,
`sourcedefinition.go`, `source_map.go`), document and workspace symbols
(84; `symbols.go`), signature help (82), inlay hints (64), semantic tokens
(45), call hierarchy (39), selection ranges (37), folding, code lens, linked
editing, `diagnostics.go` with suggestion diagnostics (37) and the
`VerifyBaseline*` writers' exact text shapes.

- **Witnesses:** the fourslash tests whose primary verifier is one of these,
  and the 1,749 baselines they write, compared whole with the committed
  references.
- **Exit:** every L3 test passes or is allow-listed; `P5B-L3` passes.

### L4 — completions and auto-imports

`completions.go` (the largest file of `ls`), `string_completions.go`,
`jsdoc.go`, `jsdoc_snippet.go`, `autoinsert.go`, `constants.go`, and the
`autoimport` package complete: `index.go`, `registry.go`, `export.go`,
`extract.go`, `view.go`, `aliasresolver.go`, `specifiers.go`, `fix.go`,
`import_adder.go`, `util.go`; `codeactions_importfixes.go`;
`VerifyApplyCodeActionFromCompletion`.

- **Order sensitivity:** completion lists are sorted by the pin's comparators
  and `sortText`; the index is built from the program's exports in the pin's
  order. ADR 0010 applies: the comparators are ported, not re-derived, and
  the first L4 witness is the completion ordering over a sample of 200 tests
  before the bodies are chased.
- **Witnesses:** the 1,111 completion tests, the 231 import-fix tests, the
  27 JSDoc completion tests, the 75 apply-code-action tests, the 12
  `autoimport` unit tests.
- **Exit:** every L4 test passes or is allow-listed; `P5B-L4` passes.

### L5 — edits

Rename (148 files; `rename.go`, `file_rename.go`), code actions and fixes
(270 plus 97 not-available checks; `codeactions.go` and the four fixer
files), organize imports (62; `organizeimports.go` with its comparer, the
Unicode normalization dependency ADR 0017 anticipated), formatting requests
over `tsr_format` (164; `format.go`, `formatcodeoptions.go`), the change
tracker (`ls/change`: `tracker.go`, `trackerimpl.go`, `delete.go`), and the
edit-and-verify tests (`Insert`, `Backspace`, `VerifyCurrentFileContent`,
`VerifyRangeAfterCodeFix`).

- **Witnesses:** the fourslash tests named above, the `change` and `format`
  unit tests, and the text edits compared against the pin's exact
  `TextEdit` ranges and texts (positions in UTF-16 code units).
- **Exit:** every L5 test passes or is allow-listed; `P5B-L5` passes.

### L6 — the project system's long reach

`crossproject.go` and the project-reference program tests, `project/ata`
(discover typings, types map, package-name validation, the npm executor
behind a trait, mocked in tests), `projectreferencedtsfakinghost.go`,
content mappers in the server (`server_contentmapper_test`,
`contentmapper_test`, the `verbatim`, `dynamic-verbatim` and `manifest`
fakes Phase 4 left), config-file changes, custom config names, untitled and
dynamic files, project lifetime, bulk cache, watch timeouts, `project/api.go`
and `ls/api.go` for Phase 6.

- **Witnesses:** the remaining project and lsp tests; the fourslash tests
  with `@tsc` build directives and multi-project setups; the mapper
  lifecycle tests.
- **Exit:** the project and lsp suites pass whole; `P5B-L6` passes.

### L7 — the long tail

Everything the inventory still reads `fail` for: the per-feature residuals
chased in descending count, the allow-list triaged with the owner (each entry
names the test, the pinned behaviour, the Rust behaviour and the reason it is
retained), the replay corpus against both servers (decision 7), the latency
scenarios (decision 10).

- **Exit:** fourslash passes at or above 99.5 percent of 4,548 with every
  failure on the triaged allow-list; the replay corpus replays identically;
  the latency capture shows no regression; `P5B-L7` passes.

### L8 — closure

The residual list, the divergence ledger check, the audit complete over
every Phase 5 file (`mapped`, `equivalent` with a site, `later` with an
owner, no gap), the ledger `ported`, the S07 anchor refresh, the green-up
(both runners green, `checker`, `emit`, `tsc` and `lsp` re-recorded), the
Phase 5 record with the per-feature dashboard and the consumption report for
Phases 6 and 7.

- **Exit:** the P5B exit (section 5) on recorded runs; `P5B-L8` passes.

## 5. Acceptance and evidence design

Namespace: sprints `P5A` (stage A, L0) and `P5B` (stage B, L1 to L8); data
under `data/phase5/`; scripts `scripts/phase5_*.py` over the phase2 and
phase4 libraries; one producer, `lsp`, with its inputs and sources declared in
`status/runs.toml`; contract witnesses `l1-contracts` and `l2-contracts` with
receipts as Phase 3's and 4's.

| Required claim | Evidence and denominator | Reuse |
| --- | --- | --- |
| fourslash passes | 4,548 test functions through the carried patch against `tsrust --lsp --test-host`; each test's outcome from `go test -json`; 1,749 baselines compared whole; at least 99.5 percent pass (4,526) with every failure on the triaged allow-list (`data/phase5/allowlist.toml`, owner-approved, one entry per test with the reason) | ADR 0019's seams, `tsr_testhost` |
| The project suite | 85 tests through the patched `projecttestutil` | The pinned mocks |
| The LSP suite | 30 tests | The patched client |
| The unit suites | 8 + 12 + 12 + 1 + 7 | Rust ports of the pinned tests, named in the roster |
| The bridge | A worker blocked in a synchronous file-system call issues a reverse request while the router pumps; cancellation reaches it; progress flows | S11's contracts, promoted to the production endpoint |
| Ownership | `l1-contracts`: release per snapshot, project and bundle; retirement after a panic; two runs identical | E3, C6, T3/T7 shapes |
| Replay | The recorded editor sessions replay identically against the Go and Rust servers | `lsp/replay_test.go` |
| Latency | The editor scenarios (decision 10) on this host, Rust beside Go, no regression | The S07 measurement discipline |
| Function disposition | Every function of the 130 files `mapped`, `equivalent` with a site or `later` with an owner; no `gap` at L8 | `phase4_audit.py`'s rules |

Producer metrics (`run.lsp.*`): `inventory_frozen`, `inventory_verified`,
`harness_valid`, `result_recorded`, `blockers_named`, `bridge_contracts`,
`fourslash_pass` (fraction of 4,548), `fourslash_allowlisted` (count),
`fourslash_baselines` (fraction of 1,749), `project_suite`, `lsp_suite`,
`unit_suites`, `replay_parity`, `latency_regression`, `residuals`,
`dispositions`, `evidence_current`, `report`, and per checkpoint
`lN_complete`. The P5B exit is:

```text
sprint.P5A.done == 1
sprint.P4B.done == 1
run.lsp.harness_valid == true
run.lsp.fourslash_pass >= 0.995
run.lsp.fourslash_baselines == 1 (over the tests that pass)
run.lsp.project_suite == true
run.lsp.lsp_suite == true
run.lsp.unit_suites == true
run.lsp.bridge_contracts == true
run.lsp.replay_parity == true
run.lsp.latency_regression == false
run.lsp.residuals == 0
run.checker.errors_parity == 1 (and the six other Phase 2 metrics)
run.emit.output_parity == 1 (and the other Phase 3 metrics)
run.tsc.baseline_parity == 1 (and the other Phase 4 metrics)
```

The 99.5 percent threshold is PLAN's and the only one; it admits at most 22
failing tests, each on the allow-list with an owner-approved reason. A
retained behaviour difference that is not a test failure is a
`data/divergences.toml` entry (ADR 0004).

Every recorded fact keeps the Phase 2 discipline: the inventory recorded
from the pin and bound by digest; the harness patch bound to the pinned
files' hashes; the Rust capture bound to the executable and the source
closure; `replay` before recording; the register rebuilt from evidence.
`cargo xtask run lsp` and `status --record` remain the owner's.

## 6. Cost control and performance risk

- **Runs.** The pinned fourslash package runs in a few minutes in Go with
  its shared parse cache; against one long-lived Rust process with
  `test/reset` the suite is expected under ten minutes once features exist
  (measured at L0 against the skeleton, where every test fails fast). During
  a checkpoint, the inventory's feature tags select the subset that
  checkpoint owns; the full suite runs at each exit.
- **Threads.** The server's request handling, background queue and project
  pool follow the pin's goroutine shape over the bounded work group (ADR
  0009); a request's work runs on the reserved stacks the checker uses.
- **Memory.** The project system is the first long-lived process in the
  port: snapshots, overlays, parse-cache entries and checker generations are
  released by the E3 contracts, which `l1-contracts` repeat; the latency
  capture records retained memory beside time. Phase 7 owns the budgets.
- **Latency.** Decision 10 names the scenarios; one capture at L7 beside Go,
  no threshold other than "no regression".
- **Correctness first.** No speed or memory gate; unfavourable timing is
  reported beside Go's without conversion.

## 7. Risks

| Risk | Where it shows | Mitigation |
| --- | --- | --- |
| The long tail: 4,548 tests, many features with few tests each | L7 stalls at 98 percent with hundreds of one-off failures | Feature tags in the inventory; residuals chased by count; the allow-list capped by the threshold |
| Completion ordering and `sortText` | Whole lists read `fail` for an ordering difference | ADR 0010: comparators ported; the L4 ordering witness before the bodies |
| The synchronous bridge deadlocks or starves | Tests hang; cancellation does not reach a parked worker | L1 builds it first with its contracts; a per-test deadline in the patch turns a hang into a recorded failure |
| Nondeterminism in the queue and diagnostics publishing | Flaky outcomes | The replay corpus and the two-runs-identical contract; the pin's ordering rules ported exactly |
| UTF-16 positions and line maps | Off-by-one ranges in every edit | ADR 0013; `lsconv` ported with its tests before L3 |
| Automatic type acquisition needs npm | CI cannot run it | Mocked in tests as the pin mocks it; the real executor exercised only by a manual witness |
| `lsproto` generation drifts from the Go generator | Field names or optionality differ, breaking the harness's JSON | The generator is checked against `lsp_generated.go`'s type list by a test; the JSON fixtures of `lsp_json_test.go` are ported |
| Staleness | Every recorded run reads `stale` while Phase 5 changes shared crates | Accepted mid-phase (decision 12); L8's green-up |

## 8. Owner decisions

Twelve proposals. Each stands as written unless the owner changes it.

1. **Names.** Checkpoints L0 to L8, sprints `P5A` and `P5B`, producer `lsp`,
   data under `data/phase5/`.
2. **The pinned suites drive the Rust server** through a carried harness
   patch (`lsptestutil`, the server-options lines of `fourslash.go`,
   `projecttestutil`) that connects the in-process client to
   `tsrust --lsp --test-host` over stdio with S11's file-system, option and
   stream seams. No Rust rewrite of the 4,548 test bodies; the Go harness
   keeps asserting. The alternative, porting the fourslash runner and
   converting the tests, is more work and cannot be proven equivalent.
3. **One Rust server per Go test binary** with a `test/reset` that discards
   the session and keeps the parse cache, as the pin's shared `parseCache`
   does. The alternative, one process per test, parses the bundled libraries
   4,548 times.
4. **Crates.** `tsr_lsproto` (generated types plus the base protocol),
   `tsr_lsp`, `tsr_ls`, `tsr_autoimport`, `tsr_project` (extended), with
   `tsr_format` and `tsr_api` consumed as they are; the harness patch and the
   generator under `tools/phase5/`, repository-only. New crates register in
   `tools/packaging/packages.json` as unpublished until a release decision.
5. **`lsproto` is generated here** from the pinned meta model by a Rust
   generator (ADR 0015), not transcribed from `lsp_generated.go`; the
   generator's output is checked against the Go file's type and method lists.
6. **The allow-list** (`data/phase5/allowlist.toml`) holds at most 22 tests,
   each with the owner's reason; it is the only tolerated-failure mechanism,
   and a retained behaviour difference goes to `data/divergences.toml`.
   Corsa's own Strada deviations (`MarkTestAsStradaServer`, the triage files)
   are reproduced, not excused.
7. **The replay corpus.** The owner records editor sessions with the pinned
   server's replay facility on their own projects (at least one per family:
   a TypeScript project with references, a JavaScript project with
   `checkJs`, a monorepo) and commits them under `data/phase5/replay/`; the
   replay runner compares both servers' responses. No session, no
   `replay_parity`.
8. **Automatic type acquisition** is ported with npm as a child process
   behind a trait; the pinned mocks drive the tests; no producer touches the
   network.
9. **The `fourslash` package** (parser, baseline utilities, state baseline)
   is `later` with Phase 7 as the owner: a Rust-side runner is a dogfood and
   cut-over tool, not a Phase 5 gate. The ledger keeps `tsr_fourslash`.
10. **Latency scenarios.** Five editor scenarios on the smoke fixture and one
    of the owner's projects: open a project and first diagnostics, completion
    at a marker, hover, find references, rename; each measured as the
    request's wall time over twenty repetitions, Rust beside Go, recorded at
    L7, no threshold beyond "no regression".
11. **Dependencies.** The Unicode normalization crate ADR 0017 anticipated for
    the organize-imports comparer; nothing else new. The JSON codec is
    `tsr_json`; msgpack stays Phase 6's.
12. **Evidence.** Phase 5 changes stale the recorded `checker`, `emit` and
    `tsc` runs. Nothing is re-recorded per fix; L8's green-up re-records
    them, and the recordings are the owner's.

L0 starts on this plan once the decisions are settled; the L0 record
(`docs/PHASE5-L0.md`) carries the measured costs, the frozen inventory and the
first categorized run.
