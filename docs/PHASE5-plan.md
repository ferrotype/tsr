# Phase 5: language service, project system, LSP server

Status: **amended after PR #82 review** (2026-10-03).
The detailed implementation plan for [PLAN Phase 5](../PLAN.md#phase-5-language-service-project-system-lsp-server).
Its checkpoints carry their own work items, witnesses and exit checks;
production implementation starts at L0.

Planning reference: `main` at `a89193d2` (Phase 4, expectation files,
Phase 0/1 harness retirement and 0.2.0 release preparation merged).
The CLI package is now `tsrust` under `crates/tsrust`; package `tsr` under
`crates/tsr_facade` is the public library facade. Scope counts below are the
planning census at `e3d7d435`; L0 checks test discovery against the pin.
Upstream remains Corsa `1f70213d4922b434345f639b441681e470c7cfc1`.

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
| Programs, checking, emit, incremental, watch | Phases 2 to 4 | `Program::load`, `CheckedProgram`, `CompilerCheckerPool`, `CancellationToken`, `CheckedProgram::emit`, `tsr_incremental`, `tsr_fswatch`, the watch manager (`tsr_tsc::watchmanager`), the mapper child process (`crates/tsrust/src/process.rs`), `--lsp` and `--api` recognized by `tsrust` and refused with `NotImplemented` (Phase 4 decision 12) |
| Source maps | `tsr_sourcemap` (T2) | The decoder and the document position mapper the service's `sourcedefinition.go` reads |
| Content mappers | `tsr_contentmapper`, `tsr_ipc`, `tsr_jsonrpc`, `tsr_contentmappertest` | The host, the in-process fakes, and since Phase 4 the child-process spawner |
| The pinned harnesses | `internal/fourslash` (5 files, 8,019 lines), `testutil/lsptestutil` (the in-process LSP client, 15 functions), `testutil/projecttestutil` (48 functions, with generated client and npm mocks) | Go; drive the Go server in process |

### The acceptance corpus

| Suite | Count | Form |
| --- | --- | --- |
| fourslash | 4,355 test files, 4,548 test functions under `internal/fourslash/tests` (PLAN's 4,356 counts the files); 1,749 committed baselines under `testdata/baselines/reference/fourslash` | Go test bodies that open files, move to markers, edit, and verify a result (a list of completions, a quick-info text, a baseline file) against the in-process server |
| project | 85 top-level tests in `internal/project`, plus tests in its subpackages | Direct Go-object assertions. Port these tests to Rust with their client, npm and clock mocks; a client transport patch cannot redirect them |
| lsp | 30 top-level tests in `internal/lsp`, plus `lsproto` and `lspwatcher` tests | Mixed: client-driven server tests stay in Go over the transport patch; direct server, queue, progress, codec, sanitizer and watcher tests become Rust tests |
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

## 2. Implementation decisions

### 2.1 What runs against Rust

There are two test paths, chosen by what the assertion reads:

| Test shape | Implementation | Result home |
| --- | --- | --- |
| Fourslash test bodies and LSP tests that send client requests | Keep the pinned Go bodies and assertions. A carried transport patch connects their client to the private test-host entry point running the production `tsr_lsp` server | `status/parity/fourslash.json` and the client-driven part of `lsp.json` |
| Tests that construct or inspect Go project/server objects | Port the bodies, mocks and assertions to Rust against the real Rust components | The owning crate's `cargo test` suite, with `// source:` references |
| Replay sessions | A client adapter sends the same recorded actions to both servers and compares responses; it also checks the committed expected responses during ordinary CI | Replay variants in `lsp.json` |

This split is necessary. `project/checkerpool_test.go` calls `NewSession`
and checks `pool.checkers[0]`; `snapshot_test.go` inspects cached files and
object identity. `lsp/dynamic_queue_test.go` constructs `newDynamicQueue`
directly. Patching `lsptestutil` or `projecttestutil` would leave those
assertions testing Go. They cannot count as Rust passes that way.

L0 lists each pinned test file and its route in `docs/PHASE5-tests.md`, with
named exceptions within a mixed file and the checkpoint that implements it.
Include the subpackages (`ata`, `dirty`, `background`, `logging`, `lsproto`,
`lspwatcher`, `lsutil`, `lsconv`, `change`, `autoimport`, `format`); the
planning table is not an exhaustive test roster. Preserve subtest assertions,
not just top-level names. Existing Rust ports are reused by name. Unported
unit tests remain explicit work in that document; they are not placeholder
passing tests or permanent `#[ignore]` tests. L8 requires every routed unit
test implemented or an owner-reviewed explanation of its equivalent coverage.

ADR 0019's carried Go fourslash harness remains the semantic specification.
Using Go as the client/assertion runner is the Phase 5 qualification to
ADR 0023's no-Go statement for suites that already have Rust runners. It does
not restore Go oracle captures, producers or source-freshness checks.

### 2.2 The harness patch and the actual seams

Keep the patch under `tools/phase5/harness/`, applied to a disposable copy or
through `go test -overlay`; never edit the pinned submodule. The adapter has
`list`, single-case `run`, and batch execution, and implements the existing
`parity.py` result format. Build its Go test executables and the Rust server
once before running cases; compilation is not part of a case's deadline.

The production entry point is `tsrust --lsp`. The private harness owns a
`phase5_testserver` entry point that instantiates that same `tsr_lsp` server
with `tsr_testhost` adapters and the injected filesystem/cache/spawner. LSP
and `test/` methods share its connection, as ADR 0019 requires. Keep service
dispatch, project construction and requests in the production libraries;
only injection, reset and state inspection live in the test adapter. This
keeps the private `tsr_testhost` dependency out of the published `tsrust`
closure. Also run ordinary-client smoke tests against `tsrust --lsp` itself;
passing through the private entry point cannot substitute for a working CLI.

Port the seams used by the tests, not a shadow language service:

- Replace `lsptestutil.NewLSPClient`'s in-process `lsp.NewServer` with the Rust
  process connection. The Go side serves the injected filesystem and fake
  mapper spawner through S11's callback and byte-stream interfaces.
- Carry inferred-project options through `test/initialize`/`test/setOptions`.
  Replace direct `Server.InitComplete()` waits with the corresponding
  initialization notification, after the real `workspace/configuration`
  exchange. A request's response remains the options-completion barrier.
- Keep filesystem case sensitivity, symlinks, bundled libraries, cwd,
  preferences and `runExternalCode` exactly as each test specifies. Preserve
  the pin's fixture setup, including `@tsc` prebuilds; that setup is not a Go
  language server answering requests on behalf of Rust.
- Handle fourslash's direct state inspection too:
  `statebaseline.go:printStateDiff` reads `Server.Session().Snapshot()`.
  Add a test-only, read-only `test/projectState` projection of the real Rust
  snapshot. It supplies the project/file/config data and stable identities
  that the pinned state-diff writer reads. Adapt the writer's data access to
  this projection; retain its sorting, diff rules and baseline bytes. Program
  and source-file identity changes must not be replaced by content equality.
  A Go-side projection of an actual Go snapshot must reproduce the original
  writer's output before this path is used to judge Rust. Never construct a
  second Go program or project system to supply Rust's state.
- Audit remaining direct `client.Server` access. A lifecycle/state getter gets
  an explicit seam; a test of internal Go behavior gets a Rust unit-test port.
  There must be no accidental Go-server fallback in the Rust execution mode.

All semantic test bodies and their expectations stay unchanged. The patch
changes transport and access to server-owned state. Baselines are compared by
the pin's baseline code; the adapter reports the result and preserves `local/`
output. A changed reference byte, a deliberately failed assertion, and a
missing test result must each make the common parity check fail.

### 2.3 Batches, isolation and cache lifetime

Choose **one Rust server per batch worker, one active test per worker**.
Several workers/shards may run independently. This preserves a cache without
sharing mutable sessions between tests.

The existing `parity.py` launches one process per variant. L0 adds an optional
batch capability there; existing compiler/tsc execution remains unchanged.
For fourslash, each worker launches one compiled Go test binary for its
selected test names with `-test.parallel=1`. The Go tests' `t.Parallel()`
calls remain in place; the flag serializes active parallel tests, while
concurrent requests and background work *inside* each server/test still run
normally. Acquire the session only after the test resumes from `t.Parallel`;
L0 must check that ordering at the harness entry points. Any case that starts
server work before pausing runs in its own process, rather than holding a
shared-session lease while paused. Nested subtests must finish before their
session lease is released.

The worker owns the Rust process and connection for the batch. Per-test Go
clients attach to it through the adapter; closing one client releases its
session instead of terminating the process. The adapter runs a reset barrier
before the next test can initialize:

1. Stop admitting work for the old session, cancel outstanding work and join
   its tasks; drain or retire pending callbacks and mapper streams.
2. Drop the session, projects, snapshots, overlays, watchers, checker pools,
   options, capabilities, position encoding and per-client routing state.
   Late replies cannot resolve a request of the next session.
3. Keep only the explicitly injected parse cache. Its keys and invalidation
   follow `project/parsecache.go`, including parse options, content and mapper
   identity. The pin's test cache disables deletion; that policy is test-only.
4. Acknowledge `test/reset`, then admit the next initialization. If cleanup
   cannot finish within the deadline, kill the worker and start clean. Do not
   continue with a partly reset process.

Use the same server/session code for production requests. Reset and state
inspection are available only through the private test entry point. Version
and test the extension of S11's protocol; do not silently change its
version-2 contract.
Small LSP client tests may use one fresh process per case, especially when a
case verifies shutdown or exit. They need no shared-cache optimization.

Batch supervision is deliberately bounded. Read Go test events to identify
an active top-level case (including `pause`/`cont`, not just `run`), enforce
its deadline, and publish only completed case results. On a crash or timeout,
mark that active case failed, preserve completed results and restart with
only the unstarted cases. Kill/reap the Go process, Rust server and their
children together. A startup failure with no identifiable case is a harness
failure, not thousands of invented test outcomes; repeated startup failure
stops the batch. Missing/duplicate/foreign result IDs still fail `parity.py`.

L0 proves reset of the state it implements, timeout continuation and
single-case/batch outcome equivalence. L1 adds real project/cache witnesses:
two tests reuse a library parse; the second cannot observe the first's files,
options, encoding or callbacks; changed text/options/mapper identity cannot
reuse an incompatible parse. Establish these before enabling cache reuse
across semantic tests. Measure this small working path before claiming any
full-suite duration. There is no assumed ten-minute bound.

### 2.4 Protocol generation and positions

ADR 0015's pinned resolver is authoritative. The raw `metaModel.json` is not
the complete Corsa protocol: `_generate/generate.mts` adds custom structures,
requests, notifications, aliases and transformations, including
`InitializationOptions` and content-mapper configuration.

L0's pipeline is:

1. Obtain the model version selected by the pin's `fetchModel.mts` and
   package lock. Keep the exact external schema bytes and their source in the
   generator's inputs so normal generation is offline and repeatable. This
   is a code-generation input, not a recorded test run.
2. Add a narrow JSON-export hook to a scratch copy of the pinned generator,
   after its model augmentation and resolution. Export the normalized types,
   inheritance, union/discriminator rules, methods and wire field behavior
   needed by Rust. Do not reimplement that resolver in Rust.
3. The Rust emitter `xtask/src/gen/lsproto.rs` consumes that export and
   writes `tsr_lsproto`'s generated types/codecs. Wire it into the existing
   generation command as `cargo xtask gen lsproto [--check]`; keep handwritten
   special codecs explicit. The access adapter and metamodel inputs live under
   `tools/phase5/lsproto`.
4. Check type/method/field coverage against the pin and port `lsp_test.go`,
   `lsp_json_test.go` and `baseproto_test.go`. Include custom initialization
   and mapper fields, absent/null/empty values, union alternatives, unknown
   fields and field ordering; matching type names alone is insufficient.

Port `baseproto`, `jsonrpc`, `structcodec` and `util`, reusing the existing
transport/JSON implementation where it satisfies the same contract. Keep
S11's retained Rust transport tests when replacing its framing implementation;
do not restore the retired S11 Python producers. Its explicit
strict-Unicode filesystem limitation remains visible (ADR 0019).

Positions are **negotiated per initialized session** (ADR 0013). Match
`server.go:handleInitialize`: default UTF-16, choose UTF-8 when the client
offers it. The pinned fourslash harness offers UTF-8. Source offsets remain
bytes internally; document sync, every request position and every returned
range use that session's converter. Test UTF-8 and default UTF-16 on BMP and
non-BMP text, combining characters, CRLF and edits before a marker. Reset
must also reset this choice. Do not rewrite the Go client's advertised
encoding to conceal a conversion bug.

### 2.5 Ownership and remaining boundaries

- Port the synchronous filesystem bridge first in L1: a worker waits on a
  reply slot while the router continues reading. Cancellation, disconnect,
  reset and normal completion each settle the slot once; progress can flow
  while a worker is parked. No router lock may be held across the wait.
- Port the project/session ownership graph, snapshot immutability, lazy
  storage, mapper bundle lifetime and canceled checker disposal through the
  existing public compiler contracts. Use production lifecycle tests, not
  detached counter-only fakes.
- Automatic type acquisition uses a mockable npm executor. Rust unit tests
  retain the pinned mocks and fake time. Network access and real npm installs
  are excluded from ordinary CI; an explicit manual integration test covers
  the real executor. Replay fixtures supply their dependencies locally.
- The Go fourslash parser and baseline writers remain the test harness;
  Phase 5 does not create a Rust `tsr_fourslash` runner. Phase 7 may revisit
  that choice for cut-over. This deferral does not cover the state projection
  required above or any production language-service operation.

## 3. Scope, prerequisites and coordination

| Area | Phase 5 work | Existing dependency |
| --- | --- | --- |
| Protocol and server | `tsr_lsproto`, `tsr_lsp`: generation, framing/codecs, initialization, request queue, cancellation, progress, logging, stack sanitizing, capabilities and watcher registration | `tsr_json`, S11 session/transport, Phase 4 native watcher/process/signal layer |
| Projects | Extend `tsr_project`: sessions, snapshots, overlay FS, configured/inferred projects, config registry and builders, project collection, references, parse/ref-count caches, owner/program counters, checker pool, file changes, watch timeouts, background work, logging, ATA and API-facing methods | `Program::load`, `CheckedProgram`, compiler checker pool, generation retirement, cancellation, config parsing and resolver |
| Language service | `tsr_ls`, `tsr_autoimport`: all feature files, `lsutil`, `lsconv`, `change`, auto-imports and API-facing methods | Checker/service operations, formatter, request-scoped printing, source maps and emit |
| Compiler hand-overs | `projectreferencedtsfakinghost.go`, remaining project-facing `program.go` operations and mapper hand-overs from Phase 4 | Existing compiler/program implementation; no build-info dependency in the server |
| Native entry point | Replace `--lsp` refusal in `tsrust`; port `runLSP`, parent-process watchdog and `isProcessAlive` | Phase 4 command dispatch and process layer |
| Tests | Carried Go client harness; Rust ports of direct tests; existing parity/CI/performance tools | ADRs 0013, 0015, 0019 and 0023 |

There is no historical-capture prerequisite to L0 and no phase-end evidence
refresh. Work from main, run focused tests for changed behavior, and let CI
run the full applicable suites. Amend expectation files only from actual
results, preserving reasons and owner approvals. Do not regenerate the
ledger or any retired inventory because a marker moved. Update existing
ledger status/Rust homes as ports land, within `cargo xtask validate`'s rules;
`later` is a planning disposition in this document, not a new ledger status.

The retired Phase 0/1 harness crates and scripts are not dependencies to
resurrect. The current parity, generation and perf commands are the tooling
base. Respect the 0.2.0 packaging split: production dependencies belong to
the public closure and test adapters remain private. In particular, keep the
compiler's reverse development dependency on `tsr_project` path-only while
the project system gains its normal compiler dependency; do not reintroduce
the release-order cycle removed by #88.

A missing dependency is part of the implementation task when it can be fixed
faithfully through the existing API. Make the change and its regression test
in a separate commit; seek owner input only for a real behavior, dependency
or scope decision. Phase ownership is not a reason to stop at a routine
cross-crate fix. Phase 6 may begin after the session/service API it consumes
is stable; implementing `project/api.go` and `ls/api.go` belongs here.

## 4. Delivery order and completion rules

L0 establishes the working protocol and test path; it includes production
skeleton code, but claims no language-service parity. L1 builds project
ownership and the bridge; L2 connects the server; L3 to L5 implement features;
L6 completes cross-project/ATA/mapper behavior; L7 closes remaining failures
and measures latency; L8 documents completion.

```text
L0 -> L1 -> L2 -> L3 / L4 / L5 -> L6 -> L7 -> L8
```

This is dependency order, not a ban on implementing a shared helper or a
small later-checkpoint dependency early. Feature blocks share code and need
integration review; their names do not imply fully disjoint implementations.

Before an implementation exists, the end-to-end case is a **failure with a
named missing-operation reason** in the ordinary expectation file. The wire
format remains `pass|fail|skip`; there is no new `unsupported` success or
untracked denominator. A Go-native skip keeps its reason and does not count
as a Rust semantic pass. L0 identifies native skips separately; unexpected
skips are harness failures, not a way to reduce the phase's failure count.

Each checkpoint's assigned work must be implemented and its unit tests pass.
A green CI run with thousands of expected failures does not itself complete
a checkpoint. Unresolved failures stay named; move a task to a later
checkpoint only with a concrete dependency, not to make an exit look green.
At L7, only the owner-approved, bounded final exceptions of section 5 may
remain. Routine commits need focused checks, not a local full-suite rerun.

### L0 — working protocol and acceptance path

1. Implement the generation pipeline in section 2.4 and its wire tests. Add
   `tsr_lsproto`, the `tsr_lsp` skeleton and the harness/emitter tooling.
   Register production crates in the publication policy and dependency-first
   release order at the workspace's lockstep version; harness/emitter crates
   stay private. This changes package metadata, not registry publication.
   The binary remains `tsrust`; do not add CLI code to the `tsr` facade.
2. Support `initialize`, `initialized`, `shutdown`, `exit`, S11 test-host
   setup/options/streams and the lifecycle reset. Other requests fail by name
   through a typed error, not a panic. A not-yet-implemented state projection
   is also an explicit failure until L1 connects real snapshots.
3. Implement the carried client patch, initialization barrier and common
   result adapter in `tools/phase5/harness/`. Add the optional batch path to
   `scripts/parity.py`, plus tests for single/batch equivalence, deadlines,
   restart, incomplete output and session isolation. Keep the ordinary
   single-case path for `--id` debugging.
4. Separate direct Go-object tests from transport-driven ones in
   `docs/PHASE5-tests.md`. Derive executable case names from the pinned test
   binaries; route the direct tests to their Rust crate/checkpoint. Include
   all relevant subpackages. The list is implementation work allocation,
   not a generated coverage/evidence register.
5. Keep baseline comparison in the pinned writer. Prototype the read-only
   state projection and the Go writer's adapted input against a native
   session: initial state, file edit, changed program identity, project
   removal and config changes must reproduce the existing text exactly.
6. Demonstrate a real Rust endpoint with initialization, option completion,
   filesystem callbacks, case sensitivity, symlinks and fake mapper bytes.
   Check that Rust mode cannot instantiate the Go language server. Prove
   reset of the implemented state with two small tests before a batch of the
   corpus. Actual project/cache reuse waits for L1's witnesses.
7. Add `fourslash` and transport `lsp` suites to the existing CI/parity tool;
   Go is installed for this client harness. Rust unit tests remain in the
   workspace test job. The adapter returns assertion/baseline failures even
   though Go's test binary exits nonzero on an ordinary failing test; process
   crashes and missing outcomes remain distinct harness failures.
8. Run the complete initial suite once, accept its exact failing set and
   explain missing features by checkpoint. Keep logs/local baselines as CI
   artifacts. Note preparation/build time and test time separately. Do not
   run a second copy merely to authenticate a recording.

**Exit:** the real endpoint and adapter work; transport and batch regression
checks pass; CI exercises both new expectation files. A skeletal server must
not yield semantic passes merely because initialization succeeded.

### L1 — project ownership, snapshots and the synchronous bridge

1. Port the core `Session`, snapshot/overlay filesystem, config registry and
   builders, configured/inferred project selection, program construction,
   parse/ref-count caches, owner/program counters, checker pool, file changes,
   project watches/timeouts, dirty maps, background queue and logging. Bring
   in compiler hand-overs as callers need them.
2. Port direct core project tests to Rust with the same observations: old
   snapshots stay unchanged, identity is reused only where Go reuses it,
   caches release on the same lifecycle transitions, configuration changes
   select the same projects, checker request affinity and idle cleanup hold.
   Use controllable clocks/notification barriers in timeout tests. Tests
   needing ATA, cross-project or mapper support are explicitly assigned L6.
3. Connect synchronous FS calls to the router's reply slots. Test delayed
   reads, several blocked workers, progress, cancellation, disconnect, reset
   and late replies. The router continues to drain both directions; canceled
   requests do not leak slots or strand workers.
4. Wire options application and `test/projectState` to the actual session.
   Match the pinned state-baseline writer through the data projection, with
   identity tokens that preserve its equality tests. Keep snapshot reads
   consistent with the preceding acknowledged client actions.
5. Repeat ownership/disposal and panic-retirement tests through real sessions:
   retained snapshots and bundles remain usable, their final drop releases
   storage, and one retired generation does not invalidate another project.
   Include the test-cache lifetime as a separate deliberate retention case.
6. Prove the real cross-test parse reuse and invalidation cases in section
   2.3, then enable the retained-cache batch path for semantic tests.

**Exit:** assigned core project/bridge Rust tests pass and the state projection
is faithful. Do not claim all 85 project tests pass while L6 work remains.
Protocol-level diagnostics checks are added when L2's publishing path exists.

**Implementation progress:** the L1 branch starts with worker callback reply
slots (including cancellation, retirement and disconnect), immutable overlays,
disk snapshots and dependency tracking, copy-on-write maps, the background
queue and reference-counted parse caches. The compiler now accepts a project
cache: program loads and reuse own its leases, failed loads release them, and
escaped ASTs retain syntax independently of those leases. The explicit test
cache can retain parses across disposed programs. Hashing reuses the workspace's
`xxhash-rust` implementation of the pin's XXH3-128; the mapped key includes the
raw hash, transform identity and locale.

The next increment adds the actual session/configuration coordinator, immutable
project collections, owner-counted extended configs and program roots. Open and
queued edit/config events construct real programs through the shared parse cache;
unchanged projects retain identity, compatible edits use the compiler's reuse
path, and old compiler hosts release their construction builders when frozen.
Focused tests cover old-snapshot isolation, extended-config disposal, disk-cache
pruning, overlay language kinds and independent project panic retirement.

The session pool now schedules diagnostics, queries and persistent API checkers,
with request/file affinity, cancellation cleanup, accumulated global diagnostics
and staggered idle eviction. Discarded pools remain usable by retained snapshots.
Manual-clock and contention-barrier tests exercise these transitions.

L1 review fixes preserve file affinity when a released checker is reacquired
by request, expand watched directory deletions before resolving aliases, and
retain dirty-project state across snapshots until the project is requested.
The latter includes config changes and accumulation of multiple changed files;
old snapshots keep their programs and unaffected projects keep identity.
The unused key-only cache `retain` API was removed; program reuse already uses
`acquire` with the retained file as its fallback. Scheduler waits remain
cancellation-aware, unlike Go's semaphore wait; the project README and scheduler
record the resulting potential difference in subsequent slot/type allocation
order. This documents existing behavior, not a new parity claim.

Config and program watches now follow snapshot updates, with shared registration
counts, external-directory grouping, URI-relative patterns, per-call deadlines
and rollback/retry using the original watcher IDs. The client interface runs on
the background queue; real-session tests check publication precedes callbacks and
text-only edits keep watch identity. Native client registration is connected with
the server in L2.

Session update and idle-clean timers now use the shared deadline service with
controlled-clock tests; inferred-options changes publish before acknowledgement.
Project log sinks and forked collectors are implemented with an injected local
timestamp formatter. ATA log embedding remains with L6.

The version-3 private endpoint now drives the real `tsr_lsp::Server` and
`tsr_project::Session`. An ordered worker performs updates while the router
continues processing filesystem replies, progress and cancellation. Options
responses wait for the actual application hook; reset retires callbacks and
joins old session work before admitting the next test. The reset queue slot is
reserved even at the admission bound.

The state projection is read-only, with identity tokens for the native writer's
pointer comparisons. `tools/phase5/project/check.py` compares eight successive
states through the original Go writer, first validating the native projection
round trip. It also checks identical output in a fresh process and a two-test
retained-cache batch. This small endpoint sequence takes about 1.3 seconds on
the development host after builds; it is not a full-suite estimate.

Mapped cache leases retain canonical and supplemental outputs together and
reconstruct the key from the original input options. Failed construction
publishes nothing. Real-worker reset tests verify shared library parse reuse,
changed text/parse-option invalidation, old session/filesystem disposal, and
isolation of overlays, options, encodings and callbacks. The retained cache is
an explicit private-test policy; production caches release on the last lease.

The L1 core implementation and focused tests are complete. The test assignment
and reproduction commands are in `tools/phase5/project/README.md`; this does
not claim all 85 native project tests or their subcases. L2 owns diagnostics,
progress publication, capability negotiation and watcher registration; L6 owns
ATA, mapper execution and cross-project/API project construction. L0's full
fourslash transport patch remains a separate prerequisite for semantic corpus
execution: the retained-cache path is enabled and tested in the real private
endpoint, but the eight-state project adapter is not that complete harness.

### L2 — server behavior and negotiated conversions

1. Port dispatch, document sync, configuration exchange, diagnostics publishing,
   capabilities, dynamic queue, cancellation/ordering, progress and logging.
   Connect the project session rather than creating a parallel test model.
2. Port direct LSP internal tests to Rust: queue ordering and cancellation,
   progress state, server recovery/logger internals, stack sanitizing and
   content-mapper configuration parsing. Port `lsconv` tests and verify
   negotiated UTF-8/default UTF-16 on non-ASCII document changes and ranges.
3. Implement `lspwatcher` and dynamic client watcher registration, including
   the native fallback when the client lacks registration support. Port its
   fake-backend tests and keep real-backend checks on the applicable hosts.
4. Implement `runLSP`, shutdown/exit and the parent watchdog in the native
   `crates/tsrust` entry point. Explicitly test orderly cleanup and parent
   disappearance. Run an ordinary client against that binary through
   initialize, open/edit, diagnostics, one service request and shutdown;
   compare its responses with the private test entry point on the same files.
5. Run client-driven Go tests over the patched transport: initialization,
   options, progress, project info and document sync. Preserve feature failures
   until L3 to L6 implement them; direct unit tests never enter `lsp.json`.

**Exit:** assigned server/unit tests pass; the endpoint dispatches into the
real session, and remaining protocol failures name their missing feature.

#### L2 implementation record (2026-10-04)

The native `tsrust --lsp --stdio` entry and version-3 private endpoint now use
the same production runtime and real project session. Input/reverse replies
remain independent of request preparation; diagnostic workers retain snapshots
and honor cancellation. Initialization negotiates encoding/capabilities,
exchanges configuration, registers watches, and connects progress, logging,
recovery and diagnostic publication. Native watcher fallback uses the Phase 4
backend on hosts with fast recursive watching. Shutdown, exit and the parent
watchdog are connected.

`tsr_ls` owns coordinate/diagnostic conversion, including canonical and
supplemental projections, mapped diagnostic ranges, tags, localization and
style severities. Pull diagnostics collect all four compiler phases and
aggregate diagnostics in synthesized mapper code. Project-info retains config
spelling independently of case-insensitive cache keys.

[The L2 test record](../tools/phase5/lsp/README.md) maps the direct tests and
documents the two small cross-runtime checks. Four unchanged pinned Go client
tests and an additional options/document-sync contract pass against both
implementations. Full initialize, diagnostic, project-info, incremental edit,
configuration and shutdown responses match Go through both Rust entry points
in UTF-8 and UTF-16. These checks are not fourslash/`lsp.json` acceptance credit.
L3–L6 service handlers, mapper execution and ATA still refuse their named
features; the full L0 fourslash supervisor/transport patch remains outstanding.
Native pprof remains the recorded Phase 7 boundary. Telemetry uses the pinned
sanitizer's unknown-frame rule, so Rust backtraces are redacted on the wire.

### L3 — read-only features

Implement hover/quick info and symbol display (`hover.go`, `hovericon.go`,
`displaypartswriter.go`, `symbol_display.go`); references/highlights
(`findallreferences.go`, `documenthighlights.go`, `importTracker.go`);
definition/implementation/source definition and source maps; document and
workspace symbols; signature help; inlay hints; semantic tokens; call
hierarchy; selection ranges; folding; code lens; linked editing; diagnostics
and suggestion diagnostics. Port shared `lsutil`/`lsconv` helpers as needed.

Use the primary-verifier families from section 1 to select focused cases,
including edits between requests, cross-file results, cancellation and both
position encodings. Baseline writers stay in Go; Rust returns the real
service results. Port relevant direct `ls` and helper tests alongside code.

**Exit:** assigned features and unit tests work; end-to-end failures are fixed
or explicitly dependent on the named L4 to L6 work. A reason alone does not
turn an unimplemented L3 operation into completed work.

### L4 — completions and auto-imports

Implement `completions.go`, `string_completions.go`, JSDoc completions/snippets,
`autoinsert.go`, constants, import fixes and the complete `autoimport`
package (`index`, `registry`, `export`, `extract`, `view`, `aliasresolver`,
`specifiers`, `fix`, `import_adder`, `util`). Implement completion resolution
and applying its code action, not just the initial list.

Port the pinned comparators and `sortText` rules (ADR 0010). Start with the
completion-ordering cases and a bounded sample, then expand to the 1,111
completion, 231 import-fix, 27 JSDoc and apply-action families. Those planning
family counts overlap; they are not added to create a denominator. Include
the direct auto-import tests and invalidation after edits/config changes.

**Exit:** the assigned completion/import features and unit tests pass; pending
multi-project/ATA behavior has an explicit L6 dependency.

### L5 — edits

Implement rename and file rename; code actions/fixes; organize imports and
its pinned Unicode normalization comparer; formatting over `tsr_format`;
formatting options; and the change tracker (`tracker`, `trackerimpl`,
`delete`). Apply changes and verify resulting contents, not just edit counts.

Use the rename, code-fix, organize-imports, formatting and edit/verify
fourslash families, plus direct `change`/`format` tests. Compare exact text
and ranges in the **negotiated** encoding. Include edit ordering, overlapping
changes where the pin rejects them, non-BMP identifiers, CRLF and cancellation.

**Exit:** assigned edit behavior and unit tests pass; retained exceptions
require the same explicit disposition as other feature work.

### L6 — cross-project, ATA and mapper completion

Finish `crossproject.go`, project-reference source redirection and
`projectreferencedtsfakinghost.go`, config changes/custom names, untitled and
dynamic files, project lifetime, bulk cache and watch timeouts. Complete ATA
(discovery, types map, package validation and mockable npm execution), mapper
projects and lifecycle, and `project/api.go`/`ls/api.go` for Phase 6.

Complete the deferred direct project/ATA tests with Rust mocks. Run the
client-driven project-reference and mapper tests, fourslash `@tsc` and
multi-project cases, state baselines and mapper cancellation/disposal tests.
Use the actual mapper host over S11's stream tunnel. A client/server failure
must not leave an external child or leased bundle behind.

**Exit:** every project/internal LSP unit-test port assigned to this phase
passes; transport LSP cases pass. Replay variants still being prepared in L7
remain explicit pending cases, not claimed complete here.

### L7 — residuals, replay and latency

1. Fix remaining fourslash failures by shared cause and then individual case.
   Read the existing expectation entries and CI diffs; do not build a second
   blockers/register system. Retained differences need the owner's approval
   in the relevant entries, bounded by section 5's distinct-test count.
2. Add repository-owned replay fixtures for TypeScript project references,
   JavaScript with `checkJs`, and a monorepo. Include the project files and
   local dependencies the actions read, not only request logs against a
   mutable directory. Owner-supplied editor sessions may extend these fixtures;
   collecting private projects is not a prerequisite to L0.
3. Adapt `lsp/replay_test.go`'s input handling. Its existing test is a replay
   driver, not a cross-server semantic comparator. Compare each request's
   result/error and the relevant ordered server notifications from Go and
   Rust; retain the expected responses for CI. Correlate request IDs and
   project-root placeholders explicitly, compare arrays/order and text/ranges
   exactly, and do not discard fields merely because they differ. No replay
   makes a network/npm request in CI. A deliberate changed response must fail.
4. Add the `lsp` workload to `scripts/perf.py`, `status/perf/thresholds.toml`
   and the existing dispatch-only `.github/workflows/perf.yml`. Do not add
   another workflow or require the old checker/parse-bind capture setup for
   this workload. Measure the actual `tsrust --lsp` and pinned Go binary with
   the same local filesystem fixture, without test-host callbacks. Measure
   open-to-first-diagnostics, completion, hover, references and rename on a
   representative checked-in fixture, with an owner project optional. Use
   twenty paired repetitions in alternating runtime order. Define the exact
   document/version/marker and completion event for every scenario. First
   diagnostics uses a fresh process/cache; other scenarios use identical
   initialization and warmup actions on each runtime. Retain raw samples,
   host, revision and per-scenario ratios in `status/perf/lsp/`.
5. Apply PLAN's no-request-latency-regression requirement: compare each
   scenario's Rust/Go median to 1.0, report variability, and resolve an
   inconclusive/noisy result before claiming closure. A measured regression
   needs a fix or a separate owner decision; do not turn it into a report-only
   requirement. This run is not required after each commit or in PR CI.

**Exit:** section 5's correctness count holds, replay comparisons pass, and
request latency meets PLAN. There is no additional Phase 5 memory budget;
retained memory is reported for Phase 7.

### L8 — closure

Check the merged implementation against section 5. Finish missing unit ports,
feature work, docs and ledger homes; describe the Phase 6/7 API hand-over.
Use the actual CI results and existing performance run. Update the Phase 5
record and let `cargo xtask status` render the current files. No historical
captures are refreshed to manufacture a green dashboard.

## 5. Acceptance and counting

All correctness runs use the existing common parity tool or `cargo test`.
`status/parity/fourslash.json` and `lsp.json` are the only end-to-end failing
sets; their `approved` fields are the only retained-divergence approvals.

| Required result | How it is checked |
| --- | --- |
| At least 99.5% semantic fourslash passes | The pinned 4,548 top-level test functions, discovered and checked in L0, run against Rust. Count distinct tests with any nonpassing required outcome, not failing subtest entries; at this pin the maximum is 22 |
| Fourslash assertions and baseline bytes | Keep every assertion outcome and baseline subtest in the expectation file. The 1,749 references are checked by the pin's writers; none is dropped to improve the percentage |
| Project and internal LSP/related unit tests | Rust ports against production components pass under `cargo test`; the L0 routing document accounts for every pinned test including subpackages |
| Client-driven LSP and replay | `lsp.json` is empty at closure; the client/assertions execute against Rust, and replay responses match |
| Synchronous bridge | Integration tests cover blocked workers, callbacks, cancellation, progress, disconnect and cleanup |
| Ownership | Production session/snapshot/project/bundle lifetime and panic-retirement tests pass; test-cache retention is distinguished from leaks |
| Request latency | The existing perf workflow/tool records the paired L7 scenarios; no scenario regresses against Go without a separate owner decision |
| Port completion and build quality | All Phase 5 production work is ported or explicitly accounted for; ordinary CI and `cargo xtask validate` pass |

Let `N` be the pinned top-level fourslash test count, and `F` the set of those
test IDs with any failed required assertion/baseline, crash, timeout, missing
implementation or unresolved skip. Closure requires
`(N - len(F)) / N >= 0.995`; with `N = 4548`, `len(F) <= 22`.
One test with three failed baseline subtests contributes **one** to `F`, but
all three failing entries remain visible. Approval does not remove the test
from `F`. A skipped test is not a semantic pass: L0 identifies pinned skips,
and any unresolved skipped test remains in `F` unless the owner separately
changes the scope. The harness must also fail on unexpected/new skips;
`parity.py`'s generic acceptance of a `skip` row is not sufficient here.

The adapter supplies the exact parent-test mapping for baseline/subtest IDs;
the report must not infer parents by an arbitrary number of slashes. Add
focused counting tests: several failures in one test; 22 versus 23 distinct
failing tests; an approved failure; a missing result; and a skipped test.
Use runner output/common summaries for this count, not another committed
metric file. Do not add fabricated failures to the expectation file for an
unobserved behavior difference: first add a witness that exposes it, or
state the untested limitation without claiming it passed.

Each final retained failing entry names the native behavior, Rust behavior
and reason and carries the owner's approval. Green intermediate CI only
means the observed failures equal those entries; it does not assert that the
99.5% phase exit already holds.

## 6. Day-to-day work and cost control

- Begin with a small working protocol/fixture, then port production behavior
  with the focused tests that exercise it. Source/API fidelity comes before
  inventing new infrastructure.
- Use `parity.py run ... --id ...` or a small feature batch during development.
  CI runs the applicable full suites. Run a full local suite only for a
  specific integration need or the initial acceptance setup, not to commit.
- Keep test inputs, patches, generated source and readable implementation
  notes. Put runtime logs and local baseline output in disposable output/CI
  artifacts. Add no `phase5_compare`, `phase5_blockers`, `phase5_audit`,
  producer, fingerprint, frozen-observation or approval-register system.
- Checkpoints use new commits and normal pushes. Update the expectation
  files from actual results when behavior changes; preserve owner approvals.
  Do not change thresholds or classify a real failure away.
- The ledger is maintained as ports land; code generation owns only its
  schema inputs/output. Neither activity triggers historical corpus or
  benchmark re-recording. Render status only when useful; never commit it.
- Make a bounded corrective experiment when performance or reliability
  warrants one. Measure the working batch/cache path before optimizing it;
  do not design a multi-session server solely to accelerate the harness.
- Stop for owner input only on an unresolvable semantic/scope choice, new
  dependency outside the choices below, or a demonstrated blocker requiring
  external data/access. Do independent implementation work meanwhile.

## 7. Main risks and required witnesses

| Risk | Required check |
| --- | --- |
| A Go test passes without touching Rust | Classify direct tests; poison the in-process Go server in Rust-harness mode; demonstrate an injected Rust response defect fails its Go assertion |
| Custom protocol behavior is lost | Run the pinned resolver, export its normalized model, and check wire fixtures including custom fields/methods |
| One test corrupts another's session | Serialized worker lifetimes, a quiescent reset barrier, late-callback/changed-option/changed-encoding tests |
| Cache reuse hides changed inputs or leaks owners | Pinned cache keys, changed text/options/mapper identity tests, explicit test retention policy and production disposal tests |
| A parked worker deadlocks the router | Concurrent blocked reads plus progress/cancellation/disconnect tests with deadlines |
| Wrong encoding corrupts edits | Both negotiated UTF-8 and default UTF-16 on non-ASCII and non-BMP edits/ranges |
| A Go process crash loses a batch | Preserve completed results, fail the active test, restart only unstarted cases, stop on startup failure |
| Long-tail failures are hidden by bookkeeping | One expectation file per suite, distinct-test counting, no passing credit for skips or approvals |
| Replay or latency uses different work | Fixed local fixtures and action sequences, response comparison, explicit warm/cold setup and raw timing samples |

## 8. Decisions for execution

This revision resolves the review's design questions as follows. L0 can start
without waiting for historical runs or owner-recorded editor sessions.

1. **Names and tools:** L0 to L8; `fourslash` and `lsp` parity files; existing
   parity, generation, CI and perf commands. No new evidence framework.
2. **Test routing:** retain Go fourslash and client-driven LSP assertions;
   port direct project/server/helper tests to Rust. A Go internal test is
   never counted as a Rust pass.
3. **Isolation:** one Rust server per serialized fourslash batch worker,
   per-test reset retaining only the explicitly injected cache; independent
   workers provide concurrency. Small LSP lifecycle tests may use fresh
   processes. Production request concurrency remains covered inside tests.
4. **Crates:** `tsr_lsproto`, `tsr_lsp`, `tsr_ls`, `tsr_autoimport`, and the
   extended `tsr_project`, consuming existing compiler/format/API crates.
   Public dependency closure, lockstep versions and release order are updated
   at creation. The private test entry point and emitter remain repository-only.
5. **Generation:** pinned LSP resolver plus normalized export and Rust emitter,
   including Corsa extensions. No second interpreter of the raw metamodel.
6. **Threshold:** retain PLAN's 99.5%, measured in distinct semantic tests;
   final differences need owner approval. Subtest details remain visible.
7. **Replay:** repository-owned TS-reference, JS/checkJs and monorepo fixtures
   and sessions by default. Owner-supplied sessions are optional additions.
8. **ATA:** a mockable npm executor; deterministic local tests and replay,
   explicit manual coverage of real external execution.
9. **Harness ownership:** the Go fourslash parser/renderer stays in use; a
   Rust runner is deferred to Phase 7. Required state/lifecycle transport
   adapters and every production service operation are implemented here.
10. **Latency:** the five scenarios in L7, twenty paired repetitions,
    dispatch/local measurement once the service works. PLAN's no-regression
    exit remains; there is no additional Phase 5 memory threshold.
11. **Dependencies:** the normalization dependency anticipated by ADR 0017;
    otherwise reuse current dependencies. JSON uses `tsr_json`; msgpack is
    Phase 6. Bring any additional dependency choice to the owner with reasons.
12. **Closure:** actual CI parity and unit tests, the L7 performance result,
    code/docs review and merged PRs. No freeze, freshness or green-up cycle.
