# Phase 6: JS API server

Status: **detailed plan, proposed** (2026-10-07). The owner accepted the
decisions of section 8 on 2026-10-07; this revision adds the work items,
witnesses and exit checks of each checkpoint and incorporates Astra's
read-only review of the same day (handle identity aligned with the symbols
design note, the A3 gap list corrected, the A2 factoring and the IPC binary
branch named). The detailed plan for
[PLAN Phase 6](../PLAN.md#phase-6-js-api-server).

Planning reference: `main` at `d8b1d37a` (Phase 5 L7 merged; L8 closure
pending and running in parallel). Upstream remains Corsa
`1f70213d4922b434345f639b441681e470c7cfc1`.

## 1. Outcome

`tsrust --api` is the pinned Go API server: the untouched
`packages/typescript` client, built from the pin, spawns it, talks to it and
passes its own test suites against it. In the pin that means:

- **Two protocols over three transports.** The synchronous client speaks a
  three-element msgpack tuple `[type, method, payload]` over stdio (or a
  named pipe on Windows, out of scope) and handles requests one at a time
  inline; the asynchronous client speaks JSON-RPC with `Content-Length`
  framing over stdio or a Unix-domain socket. The LSP server also hosts API
  sessions: `custom/initializeAPISession` opens a socket, returns its path and
  session id, and serves the asynchronous protocol on it against the editor's
  own project session (`lsp/server.go:2283`). PLAN's Phase 5 scope deferred
  that integration to Phase 6.
- **142 protocol methods** (`proto.go`'s `Method` constants) plus `echo`,
  `ping` and the two connection-level timing requests: snapshot lifecycle
  (`initialize`, `updateSnapshot`, `updateTemporarySnapshot`, `createProgram`,
  `release`, `getDefaultProjectForFile`), configuration and transpile, source
  files (binary encoder output, names, metadata), about a hundred checker,
  symbol, type and signature queries, diagnostics, completions, references,
  signature usages, import-adder edits, printing, insertion formatting, emit,
  `batchRequests` with byte-bounded pagination and continuation tokens, and
  the three profiling methods.
- **Handles with identity semantics**: `SnapshotID` (u64), `ProjectID` (the
  project path), `SymbolID` (u64, snapshot-wide: the same symbol seen from two
  projects is one handle), `TypeID` (u32) and `SignatureID` (u64) per project
  registry, and `NodeHandle` (`index.kind.path` over the encoder's node index
  table). Snapshots are reference counted; registries live and die with them.
- **The callback file system**: `readFile` (content, not found, or fall
  through to the real file system), `fileExists`, `directoryExists`,
  `getAccessibleEntries`, `realpath` and `writeFile`, chosen per connection by
  `--callbacks`, served by the client while a request is in flight.
- **Binary responses** on the synchronous protocol (encoded source files and
  synthesized nodes as raw bytes; base64 on JSON-RPC), `--timing` with the
  client's timing snapshot, `--runExternalCode` for content mappers, `--cwd`.

PLAN's gate, as amended by the owner on 2026-10-07: the untouched pinned
`packages/typescript` sync and async suites pass against Rust; API baselines
and codec wire fixtures match; through the real client, a panic injected into
one of two retained snapshots sharing a checker pool retires exactly the
affected registries and cannot be aliased by refresh or reconnect.
Performance measurement, optimization and budget acceptance are Phase 7.

## 2. Starting point

### What exists

| Asset | Where | State |
| --- | --- | --- |
| The exported protocol contract | `data/s03/schema/api.json` (142 methods, 128 types, 105 DTO declarations, 14 special mappings), produced by the carried `tools/s03/patches/gen-proto-json.patch` through `scripts/s03.py`'s disposable tooling worktree | S03. ADR 0015's `api.json`; regenerated only on a pin bump |
| Pinned wire observations and the codec inventory | `data/s03/schema/api-wire-fixtures.json` (36 marshal/unmarshal fixtures with structured error snapshots), `data/s03/api-special-codecs.json` (14 adapters: raw JSON, batch responses, document identifiers, tristate, numeric ids, enum values, ordered maps, package.json values, literal unions) | S03; "Rust API codec implementations and complete wire parity belong to the API sprint" |
| The client-file identity check | `data/s03/schema/client-files.json`: 69 files (59 generated, 10 handwritten enum inputs) compared with the untouched client after the pinned formatter; `tools/s03/proto/render_typescript.py` reconstructs `proto.generated.ts` from the export | S03; ADR 0015's equality check |
| The protocol-8 encoder and decoder, node index tables, string table | `tsr_encoder` (ported in Phase 0/1 from `api/encoder`) | Source files `ported`; the pin's 25 encoder/decoder Go tests are routed in A0 |
| Snapshot-local API roots and response commitment | `tsr_api` (877 lines): a type registry with generation checks, `request` with its unwind boundary and retirement, prepared and committed responses, `print_node`, `format_node_for_insertion`, `format_decoded_for_insertion` | ADR 0012's design for the API. Its header says transport framing and the wire endpoints "belong to the later API server": that server is this phase |
| Connections | `tsr_ipc`: `Handler`, `Conn`, `Protocol`, JSON-RPC protocol, `AsyncConn`, timing collector and the two timing requests, an in-memory `Stream`; `tsr_jsonrpc` framing | Ported for the content-mapper child process (Phase 4). No synchronous connection, no msgpack protocol, no stdio or socket transport |
| Project session API hooks | `tsr_project`: `Session::api_update` (`project/api.go`), `ApiSnapshotRequest`, reference-counted API opens, `ProjectTreeRequest`, `flush_resources` | L6 |
| Direct service queries with retained handles | `tsr_ls::api` (`ls/api.go`); `RetainedType`, `RetainedSymbol`, `RetainedSignature` and `RetainedNode` in `tsr_checker::handles`, each keeping its checker owner and exposing the checker's numeric id; most of the checker operations the queries call (see A3) | L3 to L6 |
| Everything else the handlers call | emit, transpile, config parsing, diagnostics formatting, completions, references, import adder, formatter, printer, node builder | Phases 2 to 5 |
| The CLI | `tsrust --api` is recognized and refused with `NotImplemented`; `tsrust --lsp` answers `custom/initializeAPISession` with `-32601` | Phase 4 decision 12; Phase 5 L2 |
| Reusable harness pieces | `scripts/phase5_replay_ci.py prepare` builds the pinned Go `cmd/tsc` and the release `tsrust` once (`native-lsp`, `rust-lsp`), and CI already uploads them; `parity.py`'s batch bridge and native-first denominator from L7; CI pins Node from the pin's `volta` entry | L7 |

### What is missing

| Pinned source | Lines | Ledger |
| --- | ---: | --- |
| `api/session.go`: the session, registries, 142 handlers, batches | 4,303 | Phase 6, planned |
| `api/proto.go`: DTOs, handle types, custom codecs, response constructors | 1,682 | Phase 6, planned |
| `api/protocol_msgpack.go`, `api/server.go`, `api/callbackfs.go` | 654 | Phase 6, planned |
| `ipc/conn_sync.go`, `ipc/transport.go`, `ipc/transport_unix.go` | 328 | Phase 6, planned (`transport_windows.go` out of scope) |
| `cmd/tsc/api.go`: flags and the server entry | 82 | Phase 4, planned; moves to Phase 6 with `snapshothost.go` (decision 7) |
| `project/snapshothost.go` and the `Snapshot.cloneForProgram`, `cloneWithTemporaryFile` and auto-import clones it calls | 184 + about 300 in `project/snapshot.go` | Phase 5, planned, no Rust home |
| `lsp/server.go:handleInitializeAPISession` and the socket lifetime | about 90 | inside a Phase 5 file |
| A Rust emitter for `api.json` | — | none; `xtask gen` lists `api`, `api-wire-fixtures` and `client-files` as checked artifacts without a Rust emitter |
| `api/enum_values_generated.go` | 1,008 | out of scope: a `go:build ignore` program that validates the client's generated enums |

About 7,600 lines of Go to port, against 22,849 in Phase 4 and 88,466 in
Phase 5. The weight of this phase is not volume but the contract: the client
is 17,476 lines of TypeScript that already exists and is not changed.

### The acceptance corpus

The pinned client's own tests, run by Node's test runner over the TypeScript
sources (`npm run test` is `node --conditions @typescript/source --test
'./test/**/*.test.ts'`), spawning whatever `getExePath()` resolves:

| File | Cases | Exercises |
| --- | ---: | --- |
| `test/sync/api.test.ts` | 267 | the synchronous client: 76 `describe` blocks over the whole method surface |
| `test/async/api.test.ts` | 273 | the same families through the asynchronous client |
| `test/sync/api-generators.test.ts` | 10 | generator-based batching |
| `test/sync/ast.test.ts`, `test/{sync,async}/astnav.test.ts` | 71 | the client-side AST over decoded source files |
| `test/encoder.test.ts`, `test/spanMap.test.ts`, `test/wtf8.test.ts` | 43 | encoder round trips, span maps, WTF-8 decoding |
| `test/diagnosticFormatter.test.ts`, `test/scanner.test.ts`, `test/version.test.ts` | 5 | client-only |
| Total | 669 | 12 files; none is skipped in the source |

The `describe` blocks weigh the work: about fifty are `Checker -`, `Symbol -`
and `Type -` queries, then `LanguageService -` (imports, completions,
references, signature usages), `Snapshot`, `Multiple snapshots`,
`Source file caching`, `Snapshot disposal`, `runWithTemporaryFileUpdate`,
`Program - diagnostics/emit`, `Emitter - printNode`,
`SnapshotInternalAPI - formatNodeForInsertion`, `readFile callback
semantics`, `Timing` and `getDefaultProjectForFile`.

Two facts make the harness small. The pin ignores `/built`, and
`getExePath()` resolves `upstream/built/local/tsc` when the package runs from
the repository; `ast.test.ts:573` even spells that path out. Placing a binary
there needs no patch to the pinned client, and placing the Go binary built
from the pin gives the native run. The package self-references through its
`exports`, `vscode-jsonrpc` is vendored, and the pin's Volta Node (24.20.0,
which CI already installs for the LSP generator) runs the TypeScript sources
directly; but the two `api.test.ts` files import the bench module, which
imports the workspace's `tinybench` and `typescript`, so the pin's lockfile
is installed first (`npm ci --ignore-scripts` in `upstream/`, into the
ignored `node_modules`; A0 found this on its first run). Tests use the repository root
as `cwd` and read `tsc/testdata/fixtures` from the real file system under
their virtual one. The placement is one shared path, so native and Rust runs
of a file are serialized, and any later sharding on one machine isolates its
checkout or serializes too.

Also in the corpus: 31 Go tests in `internal/api` (batches and pagination,
`DocumentIdentifier` decoding, API-state reference counting, temporary
snapshots, `createProgram` reuse and root order, completions with
auto-imports, text-edit coordinates), 25 in `api/encoder`, and the two
`api` baselines (`encodeSourceFile*.txt`). These are direct tests of Go
objects and become Rust tests, routed in `docs/PHASE6-tests.md`.

The four benchmark files (`tinybench`) contain no assertions. PLAN's
"correctness assertions exercised by benchmark cases" therefore means: each
bench case runs to completion against Rust in single-iteration mode; its
timing is Phase 7's. They use the same installed `node_modules` as the suites.

## 3. Architecture decisions

1. **One server, the pin's shapes.** `tsr_api` grows the session, the
   dispatch, the handlers, the msgpack protocol, the callback file system and
   `StdioServer`; `tsr_ipc` grows `SyncConn` and the stdio and Unix-socket
   transports; `tsrust --api` ports `cmd/tsc/api.go`; `tsr_lsp` ports the
   API-session handshake; `tsr_project` gains the snapshot host. No new
   production crate: the generated DTOs are the module `tsr_api::proto`.
   `tsr_api` and `tsr_ipc` are already public crates in the release order.
2. **The msgpack framing is five byte markers, not a library.** The pin's
   protocol uses `0x93` (three-element array), `0xcc` or a positive fixint for
   the type, and `0xc4`/`0xc5`/`0xc6` binaries for method and payload; nothing
   else. It is hand-ported with byte goldens captured from the Go binary.
   ADR 0017 anticipated an msgpack crate; this phase does not need one.
3. **Generated protocol from the committed export (ADR 0015).** `cargo xtask
   gen api [--check]` emits `tsr_api::proto::generated` from
   `data/s03/schema/api.json`: method constants and ids, DTO structs with
   `tsr_json` codecs that honor each field's JSON name, `omitempty`/`omitzero`,
   non-nil, deprecated and internal markers, the scalar handle aliases, the
   literal unions and the declaration order. The 14 special mappings keep
   handwritten adapters in `tsr_api::proto::codecs`, each named in
   `api-special-codecs.json` and covered by the 36 pinned fixtures plus the
   `proto_test.go` ports. Pin bumps regenerate `api.json` through the existing
   `scripts/s03.py` tooling worktree, where the unpatched extractor's
   `proto.generated.ts` must still equal the untouched client's; the emitter
   never reads Go. `uint64` ids are `u64`, never `f64`.
4. **Handles follow ADR 0012 and the symbols design note**
   ([`docs/design/symbols.md`](design/symbols.md), section 2.5). Generation
   tokens stay server-side; the wire carries the pin's numeric and string
   handles. `SymbolID` is a registry-assigned dense counter, as the pin's
   lazily assigned `GetSymbolId` is, mapped in the snapshot-wide symbol
   registry to the internal packed id and the retained owner; the packed id
   never goes on the wire. The registry entry's key distinguishes the two
   kinds of symbol the checker has: a file-owned symbol is keyed by its bound
   file or bundle root and packed id, so two projects of one snapshot that
   share the bound file resolve to one entry and one id, as in Go; a
   checker-owned symbol (a merge or a transient) is keyed by its exact
   `CheckerId` and id, and is one id per checker, which is also what Go's
   per-checker merged symbols give. Importing a retained symbol into a lease
   validates owner, generation and file membership as the note requires; the
   registry never substitutes another checker. The first project to observe a
   symbol is recorded as canonical (`symbolCanonicalProjects`). `TypeID` keeps
   the checker's 32-bit type ids in a per-project registry bound to the exact
   `CheckerId`, as `tsr_api` already does; `SignatureID` likewise over
   `RetainedSignature`. `NodeHandle` uses `tsr_encoder`'s node index table;
   its kind is informational and its path must name a loaded file. A request
   through a retired generation or an unknown handle returns the pin's error
   form: `api: client error: …` text in an error tuple on the synchronous
   protocol, an `InternalError` response on JSON-RPC. Each request and each
   batch item is an unwind boundary; a caught checker panic retires the
   generation, and every registry of that generation rejects its handles.
5. **Concurrency as in the pin.** The synchronous connection handles one
   request at a time on its reader and serializes outgoing calls under one
   lock, because the protocol correlates by method name and reads each
   callback reply inline; handler threads that read files concurrently queue
   on that lock. The asynchronous connection reuses `tsr_ipc::AsyncConn`.
   Inside the session an `update` lock serializes snapshot updates while the
   snapshots map is held only for map-bounded sections, so queries against
   existing snapshots run while the next snapshot is being built
   (`session.go:381`). Transport readers never wait on a worker.
6. **Two session hosts, one builder.** A standalone session owns a
   compatibility snapshot and clones it linearly through a snapshot host; an
   LSP-hosted session updates through `Session::api_update` and adopts
   auto-import preparations through the existing
   `Session::try_adopt_snapshot_in_background`. Both drive the project
   builder that `Session::update_snapshot` drives today. That builder is
   bound to `&Session` (`session/build.rs:22`) and `update_snapshot`
   publishes session state, so the host is a factoring that first gives the
   builder a build context a session-less host can supply, then ports the
   three clone entry points Rust lacks; it is not a second project system.
7. **Acceptance follows ADR 0023.** A `jsapi` suite in `scripts/parity.py`
   and CI runs the untouched client tests through Node's test runner with a
   repository-owned NDJSON reporter: one variant per test file, one row per
   test case, `status/parity/jsapi.json` the only failing set, the native Go
   run first for the denominator and the native skip set, as the fourslash
   suite does. Direct Go tests become `cargo test` ports. Codec fixtures,
   wire goldens and the generated-code drift check run in the ordinary test
   and lint jobs. No benchmark runs in PR CI.
8. **Phase 7 boundaries stay explicit.** `startCPUProfile`, `stopCPUProfile`
   and `saveHeapProfile` answer with the same explicit unsupported error the
   LSP server uses for its profiling methods; the API benchmarks, retained
   memory and any optimization are Phase 7's, and nothing in the design
   forecloses them (binary responses stay zero-copy, pagination byte
   accounting matches the pin exactly).

## 4. Working rules

- **Who.** Claude builds A0 and reviews every checkpoint pull request; Astra
  builds A1 to A4 in that order, each on its own branch and PR against
  `main`; A5's residuals are split by crate after triage, Astra on
  `tsr_api`/`tsr_project`/`tsr_checker` causes and Claude on the harness,
  records and the panic witness tooling. Claude keeps this plan, the
  records and `docs/PHASE6-tests.md`.
- **Order and overlap.** A0 lands first; A1 may start on the agreed `proto`
  module shape before A0 merges but merges after it. A1 to A4 are sequential.
  Phase 5's L8 runs in parallel; A1 and later rebase on L8's `tsr_project` and
  `tsr_ls` changes when it merges.
- **Checks per commit.** The crate's unit tests, clippy on the touched
  crates with warnings denied, `cargo xtask validate` (every `// port:`
  marker names a pinned function; the ledger's `rust` homes follow the
  ports), `cargo xtask gen api --check` when the emitter or the schema
  changes, and `parity.py run jsapi --id jsapi/<file>` for the test files a
  change touches. CI runs the full `jsapi` suite on every PR; a local full run
  is for the initial acceptance and for integration doubt, not for every
  commit. PR CI installs the pin's locked dependencies for the `jsapi` suite
  and never runs the benchmarks.
- **Expectation files.** Each PR runs `parity.py accept jsapi` so
  `status/parity/jsapi.json` is exact for the commit; reasons are cause
  labels, `approved` stays empty until the owner words a retained entry.
- **Ledger.** `PORTS.toml` entries move to `in-progress`/`ported` with their
  Rust homes as the ports land; `snapshothost.go` and `cmd/tsc/api.go` move
  to Phase 6. Nothing is regenerated for a moved marker.
- **Records.** One short record per checkpoint, `docs/PHASE6-A<n>.md`, with
  the commands, the suite numbers at exit and what the next checkpoint
  inherits; no new evidence machinery.

## 5. Checkpoints

```text
A0 -> A1 -> A2 -> A3 -> A4 -> A5
```

### A0 — protocol generation and the acceptance path (Claude)

Pinned sources: `api/proto.go` (1,682 lines, the DTOs and codecs, not the
response constructors), `cmd/tsc/api.go` (82), the client test corpus.

1. **The emitter.** `xtask/src/gen/api.rs`, wired as `cargo xtask gen api
   [--check]` beside `gen lsproto`, reads `data/s03/schema/api.json` and
   writes `crates/tsr_api/src/proto/generated.rs`: the `Method` enum with the
   142 wire ids and Go constant names, the scalar aliases (`SnapshotID: u64`,
   `ProjectID: String`, `SymbolID: u64`, `TypeID: u32`, `SignatureID: u64`,
   `NodeHandle: String`), one struct per DTO declaration in the export's
   order with `tsr_json` encode/decode that honors `jsonName`,
   `omitempty`/`omitzero`, `nonnil`, `internal` and `clientVisible`, and the
   literal unions. The export is sufficient input: it carries field types,
   JSON names, optionality, non-nil, visibility and documentation, aliases
   and declaration order; no amendment of the carried patch is needed.
   Special mappings are emitted as references to handwritten
   types in `proto/codecs.rs`: `DocumentIdentifier` (`string | { uri }` with
   the pin's `ToFileName`, `ToURI`, `ToAbsoluteFileName`), `RawBinary`,
   `BatchRequestsResponse` (pre-encoded responses, conditional continuation
   token), raw JSON values, tristate booleans, the numeric enum values
   (`JsxEmit`, `ModuleKind`, …), the insertion-ordered maps, package.json
   value kinds and the literal method names. The manifest
   `data/s03/generated.json` records the output hash as it does for the other
   generated files.
2. **Codec fixtures.** The 36 pinned observations of `api-wire-fixtures.json`
   become `tsr_api` tests: successful marshal and unmarshal byte-for-byte,
   and for error fixtures the structured class, JSON pointer and offset; the
   pin's deliberately variable top-level wording is not compared. All 14
   mappings have fixture ids, and `scripts/s03.py` checks that coverage; the
   inventory itself defers two things to runtime tests, the private
   pre-encoded `BatchRequestsResponse` path (A4) and nested package.json
   value equality (A2). Port `proto_test.go` (3) and `jsonvalue_test.go` (1).
3. **Regeneration on a pin bump** stays `scripts/s03.py` in its disposable
   tooling worktree: patched extractor to `api.json`, unpatched extractor's
   TypeScript and the client-files comparison. Document the sequence in the
   emitter's header and `docs/PHASE6-tests.md`; CI's lint job gains
   `cargo xtask gen api --check` (pure Rust, no Go or Node).
4. **The `jsapi` suite.** `tools/phase6/jsapi/`: `reporter.mjs` (a Node
   test reporter emitting one NDJSON event per `test:start`, `test:pass`,
   `test:fail`, `test:skip`/`todo` with file, nesting path and error text),
   `runner.py` (`list`: the 12 pinned test files, validated against a glob of
   the pin; `run --id jsapi/<file>`: place the binary at
   `upstream/built/local/tsc`, run
   `node --conditions @typescript/source --test <file>` from
   `upstream/packages/typescript` with the reporter, map events to rows
   `jsapi/<file>/<describe path>/<test>` with the file as explicit parent,
   fail the file variant when the process ends without a terminal event for
   a started test; `--test-name-pattern` passes through for one case), and
   `scripts/phase6_parity.py` as the batch bridge: for each file the native
   binary runs first, then Rust, and the L7 comparison rules apply (a Rust
   skip the Go run lacks fails; an observation absent from the Go run fails;
   a Go failure marks the parent `native reference failed`). `parity.py`
   gains `jsapi` with `batch=True`. Binaries come from
   `scripts/phase5_replay_ci.py prepare` (`native-lsp` is the pin's `cmd/tsc`,
   `rust-lsp` is `tsrust`), renamed to a phase-neutral name in passing.
5. **CI.** A `jsapi` job in `ci.yml`: Node from the pin's `volta` entry,
   the prepared binaries from the existing `phase5-runner` artifact, the
   suite in one shard (measure before sharding) after `npm ci
   --ignore-scripts` in the pin's root, cached on its lockfile, results uploaded,
   `parity-check` extended to `jsapi`. The Node test runner's per-file process
   model is kept; `--test-concurrency` starts at 1 and is raised only after
   a measurement.
6. **`docs/PHASE6-tests.md`**, generated by `tools/phase6/jsapi/inventory.py`
   from the native run's events and the pin's Go test files: the 669 client
   cases by file and family, the 31 `api` and 25 `encoder` Go tests with a
   route each (a Rust test name, an explanation of equivalent coverage, or
   `WORK`), and the two `api` baselines. The 21 `decoder_test.go` cases are
   expected to be covered by the Phase 0 corpus encoding parity; the
   document says so per test or routes them as work.
7. **`tsrust --api`** parses the pin's flags (`--cwd`, `--pipe`,
   `--callbacks`, `--async`, `--timing`, `--runExternalCode`; Go's `flag`
   accepts one or two dashes), installs the signal scope, accepts one stdio
   connection in the protocol `--async` selects, and answers every request
   with the pin's error form naming the method as not implemented. This
   skeleton is what makes the first full run a complete failing set instead
   of 669 deadlines; A1 replaces it with the real connections.
8. **First full run.** Native and Rust over the 12 files; `accept` writes
   `status/parity/jsapi.json` with cause labels (`not implemented: <method>`
   at first); record build time and test time separately in
   `docs/PHASE6-A0.md`.

Witnesses: `gen api --check` fails on a one-byte edit of the generated file;
the fixtures reject a changed byte of a marshal expectation; a test confirms
the 36 fixture ids are all exercised; the reporter test fixture (a fake test
file with pass, fail, skip, nested describe and a crash) yields the expected
rows and parent mapping; the native run of the 12 files passes everything
(N = 669 unless the Go run shows otherwise).

Exit: the emitter, codecs and fixtures are green; the `jsapi` suite runs end
to end in CI with its first accepted failing set; the routing document
exists; the CLI flag contract is in place.

### A1 — transports and connections (Astra)

Pinned sources: `api/protocol_msgpack.go` (281), `ipc/conn_sync.go` (211),
`ipc/transport.go` and `transport_unix.go` (117), `api/callbackfs.go` (242),
`api/server.go` (131), `lsp/server.go:2283–2365`, the `echo`, `ping`,
`initialize` and timing paths of `session.go`.

1. **Msgpack protocol** in `tsr_api::protocol_msgpack` implementing
   `tsr_ipc::Protocol`: `readTuple` with the fixint/`0xcc` type byte and
   `bin8/16/32` sizes, request/call-response/call-error mapping with the
   method name as the id, `writeTuple` with the smallest `bin` marker that
   fits, `Flush` per message, the error texts of the pin (`expected fixed
   3-element array (0x93), received: 0x..`). `tsr_ipc`'s `HandlerResult` and
   `Protocol::write_response` gain a raw-binary branch (`RawBinary`) that this
   protocol writes verbatim and the JSON-RPC protocol never receives, because
   the session only produces it when `useBinaryResponses` is set; the Phase 4
   content-mapper connections keep their JSON path unchanged.
2. **`tsr_ipc::SyncConn`**: `run` reads one message and dispatches inline;
   the two timing requests are answered before dispatch and never recorded;
   a handler panic becomes an `InternalError` response whose message starts
   `panic: <message>` followed by the sanitized backtrace; `call` locks,
   writes the request, reads the reply inline and rejects any other message
   as `unexpected message while waiting for … response`; `notify`. The timing
   collector of `tsr_ipc::timing` is reused.
3. **Transports** in `tsr_ipc::transport`: `StdioTransport` (one connection;
   closing closes both ends), `PipeTransport` over a Unix-domain socket
   listener (remove a stale socket file first; `Accept`, `Close`, `Path`),
   `generate_pipe_path(name)` under the temporary directory. Windows named
   pipes remain unbuilt.
4. **Callback file system** in `tsr_api::callbackfs`: wraps the bundled OS
   file system, validates the enabled names against the six, is connected
   after accept, and delegates each enabled operation through `Conn::call`
   with the pin's payloads: `readFile` returns content, not-found or
   fall-through from `{content}`, `{content: null}` and an empty reply;
   `fileExists`, `directoryExists`, `realpath`, `getAccessibleEntries`
   (`{files, directories}`), `writeFile` (`{path, data}`). A callback waits
   without a timeout, as the pin's does; error mapping follows `callbackfs.go`.
5. **`tsr_api::StdioServer`** with the pin's options (in/out/err, cwd,
   default library path, pipe path, callbacks, async, timing, external code,
   mapper spawner): transport selection, callback wrapping, the project
   session options (UTF-8 positions, logging off), standalone session
   construction, protocol and connection by mode, `set_collect_timing`,
   `run` until the connection closes; the session is closed on return.
6. **`tsrust --api`** completes `cmd/tsc/api.go`: exit 2 on a flag error,
   1 on a run error; `newSystem()`'s mapper spawner is the Phase 4 process
   spawner; `bundled.LibPath()` is `tsr_bundled`.
7. **LSP-hosted sessions** in `tsr_lsp`: the `Runtime` (or its `Server`)
   gains the API-session map and lifetime owner the pin keeps in
   `apiSessions`; `custom/initializeAPISession` leaves the unimplemented list
   (`runtime.rs:1355`), builds `tsr_api::Session::for_lsp` over
   `self.ready()?.session()`, takes the client's pipe path or generates
   `tsgo-api-<time hex>-<random hex>`, listens, and on a background thread
   accepts one connection, closes the listener and runs an `AsyncConn` under
   a cancellable context with panic isolation (log, cancel, close), removing
   the session from the map when the connection ends. The response is
   `{sessionId, pipe}`; ids are `api-session-<n>`. The pin removes a session
   only when its connection ends; shutdown behavior follows the pin.
8. **Direct tests**: `ipc/conn_sync_test.go` (2) and `ipc/timing_test.go`
   (4) ported; the Rust `AsyncConn` tests extended for the socket stream.

Witnesses:
- *Byte goldens.* `tools/phase6/wire/capture.py` drives the pin's binary with
  a fixed script on each protocol (`echo` with binary payload, `ping`,
  `initialize`, an unknown method, a `readFile` round trip in each of the
  three states, `getServerTiming`, a client-side error) and commits the
  frames; Rust must produce identical framing bytes and value-identical
  payloads, with `currentDirectory` as the only placeholder.
- *Serialized callbacks.* A handler that issues callbacks from eight threads
  while one request is in flight, against a fake client that validates
  framing: no interleaved frames, no deadlock, replies routed to their
  callers.
- *Disconnect.* The client closes mid-request; the server exits without an
  orphan process or a socket left bound.
- *The handshake.* A repository-owned Node script using the untouched
  asynchronous client with `ClientSocketOptions { pipe }` connects to the
  socket the Rust LSP server announced, initializes and runs one query; the
  Go server gives the same responses.

Exit: both pinned clients spawn `tsrust --api`, initialize and run `echo`,
`ping` and the timing requests in both protocols; the `API` and `Timing`
families pass as far as they need no snapshot; the handshake witness and
the direct tests pass; everything else still fails by named method.

### A2 — sessions, snapshots and handles (Astra)

Pinned sources: `project/snapshothost.go` (184) with `snapshot.go`'s
`cloneForProgram`, `cloneWithTemporaryFile` and the auto-import clone (about
300); `session.go` lines 1 to 660 (session, registries, handles, setup) and
the lifecycle, configuration, transpile and source-file handlers (about
1,100); `proto.go`'s response constructors for projects, config files and
source files.

The checkpoint has two halves, A2a and A2b, that may be two commits or two
PRs of one checkpoint.

**A2a — the snapshot host and the lifecycle.**

1. **`tsr_project::SnapshotHost`**. Rust has no session-less equivalent of
   the pin's `CloneSnapshot`, `CloneSnapshotWithTemporaryFile`,
   `CloneSnapshotForProgram` and `CloneSnapshotWithAutoImports` today;
   `ProjectBuilder` takes `&Session` and `update_snapshot` publishes session
   state. The first step therefore gives the builder a build context (file
   system, options, caches, counters, mapper host) that either a `Session` or
   a host supplies, with the L6 project tests guarding the session path. The
   host then owns the parse cache, the content-mapped parse cache, the
   extended-config cache, the program counter, the mapper host, the file
   system, the session options and the snapshot id counter, and ports the
   clone entry points: `new_standalone_root_snapshot`, `retain`,
   `clone_snapshot(base, file_changes, api_request)`,
   `clone_with_temporary_file(base, uri, text)` (`snapshot.go:238`),
   `clone_for_program(base, roots, options, references, config_diagnostics,
   old_project, file_changes)` (`snapshot.go:103`: one synthetic inferred
   project from explicit roots, seeded from the old project),
   `clone_with_auto_imports(base, uri)` (`snapshot.go:671`), `close`. The
   LSP-hosted path reuses the existing
   `Session::try_adopt_snapshot_in_background` (`session/api.rs:7`).
2. **`tsr_api::Session`**: id, snapshot host, optional project session,
   compatibility snapshot, `use_binary_responses`, batch pages and their
   counter, the snapshots map with `latest_snapshot`, the open-project and
   open-file sets, the `update` and `snapshots` locks in the pin's order.
   `SnapshotData`: the project snapshot, its reference count, the
   snapshot-wide symbol registry and canonical projects, per-project
   registries of types and signatures, and per-project API checker leases
   (`CheckerSlot::Api`). The existing single-project `tsr_api::Snapshot`
   becomes the per-project registry; `request`, `prepare`, `commit` and the
   unwind boundary keep their contracts.

**A2b — handles and the lifecycle, source-file and configuration handlers.**

3. **Handles**: `register_symbol`/`resolve_symbol_handle` over retained
   symbols with the dense registry ids and the two-kind key of decision 4
   and the duplicate checks the pin panics on; `register_type`/
   `resolve_type_handle` and the signature pair per project, each registry
   bound to its exact `CheckerId`; `node_handle_from` and
   `resolve_node_handle` over the node index table, with the pin's error
   texts for malformed, stale and unloaded handles. Registry reads take the
   owner's operation permit as the design note requires, even where the pin
   dereferences a stored pointer.
4. **Handlers**: `initialize`; `updateSnapshot` (dedupe opens and closes
   against the session's sets, `api_update`, commit the sets only on
   success, reuse an existing `SnapshotData` by id, `computeSnapshotChanges`
   against the previous latest snapshot: removed projects, and changed or
   deleted files per project whose program identity changed);
   `updateTemporarySnapshot` (retain the base while cloning, diff against
   the client's base); `createProgram` (`fileChanges` requires `oldProgram`;
   roots absolute to `cwd`; base from the old snapshot or a fresh
   `api_update`; the inferred project is the response); `release`
   (reference counting; zero or unknown handles are errors);
   `getDefaultProjectForFile`; `parseCommandLine`, `readConfigFile`,
   `parseJsonConfigFileContent`, `parseConfigFile` with
   `NewConfigFileResponse` and `toProtocolJSONValue`;
   `transpileModule`/`transpileDeclaration` and their `FromFile` variants;
   `getSourceFile` (binary or base64), `getSourceFileNames`,
   `getSourceFileMetadata`, `getConfigFileNames`, `getConfigSourceFile`
   (with the nested package.json value equality the codec inventory defers to
   runtime tests); `Close` (release open refs through an `api_update` of
   closes, dereference snapshots, close the standalone host, clear batch
   pages).
5. **Direct tests**: `session_apistate_test.go` (3), `session_createprogram_test.go`
   (8), `session_temporary_test.go` (4).

Witnesses: the same symbol from two projects yields one id and the first
project as canonical; type ids of two projects do not collide; a released
snapshot's handles are rejected with the pin's text; a temporary snapshot
reuses an existing id's data and bumps its count; the E3 case through
`tsr_api::Session`: two retained snapshots on one pool, a panic injected in a
checker operation of one, both reject old handles, a new snapshot on the
replacement generation answers, and no retired id is re-minted.

Exit: the `API`, `Snapshot`, `Multiple snapshots`, `Source file caching`,
`Source file cache keying across projects`, `Snapshot disposal`,
`runWithTemporaryFileUpdate`, `getDefaultProjectForFile`, `readFile callback
semantics` and `SourceFile` families pass in both clients; the direct tests
pass; the snapshot host's ledger entry has its Rust home.

### A3 — checker, symbol, type and signature queries (Astra)

Pinned sources: `session.go` lines 1700 to 3780 less the A4 handlers (about
1,700), `proto.go`'s `newTypeResponse`, symbol and signature constructors.

1. **Response constructors**: `SymbolResponse` (escaped name, flags, check
   flags, declaration handles, value declaration, parent, export symbol),
   `TypeResponse` (flags, object flags, tuple shape with element flags, fixed
   length, readonly and labeled declarations, literal value, target, type
   parameters in their three forms, indexed-access and conditional parts,
   substitution base and constraint, template texts, fresh and regular
   types, `isThisType`, intrinsic name, alias arguments and symbol, symbol),
   `SignatureResponse` (flags, declaration, type parameters, parameters,
   this parameter, target), `IndexInfoResponse`, `TypePredicateResponse`,
   `JSDocTagInfo`, the well-known responses.
2. **Gaps.** Most of the checker calls the handlers make exist in
   `tsr_checker` already (`get_type_at_position` is the pin's
   `GetTypeAtPosition` behind `getParameterType`,
   `try_get_member_in_module_exports` exists, the well-known symbols and
   signatures are existing calls). The confirmed gap is `get_constant_value`;
   the symbol accessors for parent, members and exports,
   `get_base_type_of_type` and alias type arguments are to be audited against
   the Go handlers' calls in `session.go:1700–3780` before work starts, and
   each confirmed gap is a port with its `// port:` marker. JSDoc tags and
   documentation comments are language-service helpers in the pin
   (`ls.GetSymbolJSDocTags`, `ls.GetSymbolDocumentationComment`), ported or
   reused from `tsr_ls`, not checker operations. Separately, the response
   constructors read type internals the operations do not expose today:
   alias type arguments and symbol, substitution base and constraint,
   template literal texts, tuple element flags, fixed length, readonly and
   labeled declarations, `isThisType`, intrinsic names; A3 budgets those
   production accessors for `TypeResponse`, `SymbolResponse` and
   `SignatureResponse` as its first item, before the handlers.
3. **Handlers** over `setup_checker` (snapshot, program, the project's API
   checker lease, project id) and the typed resolvers of the pin
   (`resolveTypePropertyOfType` and its siblings): symbols at position,
   positions, location, locations, of source file(s); types of symbol(s),
   declared and non-missing; `resolveName`, `getSymbolsInScope`; signatures
   of type, resolved signature; type at location(s) and position(s); the
   symbol, type and signature property fetchers; contextual type, base type
   of literal type, non-nullable, type from type node, widened, parameter
   type, type parameter at position; `isArrayLikeType`, `isArrayType`,
   `isTypeAssignableTo`, `isContextSensitive`, `isReadonlySymbol`;
   shorthand assignment value symbol, type of symbol at location; return,
   rest and predicate of signature; base types, properties and apparent
   properties, apparent and reduced types, index infos, constraint and
   default of type parameter, base constraint, property of type, constant
   value, signature from declaration, export specifier local target,
   aliased, immediate aliased and target symbols, fully qualified name,
   exports of module, member in module exports, JSDoc tags and documentation,
   type arguments; the twelve intrinsic getters, well-known symbols and
   signatures; `typeToTypeNode` and `signatureToSignatureDeclaration`
   (synthesized nodes encoded with `tsr_encoder::encode_node`, binary or
   base64), `typeToString`, `printNode` (`tsr_api::print_node`).

Witnesses: a handle minted on one project's registry is rejected on
another's with the pin's text; `typeToTypeNode` output decodes in the client
(the `ast` tests exercise this); each intrinsic getter returns the same id on
repeated calls within a snapshot.

Exit: the `Checker -`, `Symbol -`, `Type -`, `TypeParameter -`,
`IntrinsicType -`, `FreshableType -`, `ast -`, `modifierFlags` and
`VariableDeclarationList` families pass in both clients.

### A4 — language service, diagnostics, emit and batches (Astra)

Pinned sources: `session.go`'s diagnostics (3964–4100), completions
(4172–4252), references and usages (4100–4172, 4252–4303), import adder
(2233–2363), insertion formatting (3045–3094), emit (2908–3045), batches
(965–1065), profiling (1066–1096); `proto.go`'s diagnostic, completion,
emit and text-edit constructors.

1. **Diagnostics**: the nine methods over one `getDiagnostics` (optional file
   list; syntactic, bind, semantic, suggestion, declaration, program, global,
   config-file parsing); `NewDiagnosticResponse` with clamped positions,
   UTF-16 `pos`/`end`, line and character, the source-line excerpts, the
   localized text, `reportsUnnecessary`/`reportsDeprecated`, message chains
   and related information; `ToDiagnostic` for `createProgram`'s config
   diagnostics.
2. **Completions**: `getCompletionsAtPosition` with `includeSymbol` on the
   API checker lifetime, the `ErrNeedsAutoImports` retry through
   `clone_with_auto_imports` and background adoption on the LSP-hosted path,
   and the entry fields (name, sort, insert and filter text, detail, kind,
   label details, symbol).
3. **References and usages**: `getReferencesToSymbolInFile` (checker),
   `getReferencedSymbolsForNode` and `getSignatureUsages` (service), with
   node handles in results.
4. **Import adder**: `getImportAdderEdits` with the registry-preparedness
   check and auto-import clone, `autoimport` view and adder, and
   `toAPITextEdits` in original coordinates (`session_textedit_test.go`).
5. **Insertion formatting**: `formatNodeForInsertion` through
   `tsr_api::format_decoded_for_insertion` with the snapshot's format
   settings.
6. **Emit**: `emit` (writes through the callback file system's `writeFile`),
   `emitToString` (collected outputs, sorted), `getJavaScriptEmit` and
   `getDeclarationEmit` (selected files, forced), `getEmitOptions` and
   `getEmitProgram`.
7. **Batches**: `batchRequests` with per-item dispatch and panic recovery,
   nested-batch rejection, `newBatchResponsePage`, `paginateBatchResponses`
   ported arithmetically (the `{"responses":[]}` and continuation-token
   lengths, the single-oversized-response allowance), continuation tokens
   `<sessionId>-<n>` consumed once, `DefaultMaxResponseBytesPerPage`.
8. **Profiling**: `startCPUProfile`, `stopCPUProfile`, `saveHeapProfile`
   answer the explicit unsupported error (decision 8).
9. **Direct tests**: `session_batch_test.go` (8), `session_completion_test.go`
   (3), `session_textedit_test.go` (1).

Witnesses: the page-cut fixtures of the batch tests reproduce the pin's byte
boundaries; a panicking batch item yields `panic: …` in that item only while
its siblings answer; emitted files reach the client's virtual file system
through `writeFile`.

Exit: the `LanguageService -`, `Program -`, `Emitter -`,
`SnapshotInternalAPI -`, `Timing`, `Symbol - getDocumentationComment` and
generator families pass in both clients; all 31 `api` direct tests are
ported.

### A5 — residuals, the panic witness and closure (both)

1. **Residuals** by cause, as in Phase 5 L7: harness causes first, then
   crashes and deadlines, then shared causes by the number of cases, then
   singles; each fix with its unit regression and an `accept` for the commit.
2. **The real-client panic witness.** A private entry point (the Phase 5
   private test server gains `--api`, or a sibling binary in `tsr_testhost`)
   serves the production `tsr_api` session with one test-only control that
   makes the next checker operation of a named snapshot panic. A
   repository-owned Node script uses the untouched asynchronous client with
   `tsserverPath` pointing at that binary: two snapshots sharing a pool, a
   query on each, the fault on one, then both snapshots' old handles are
   rejected with the pin's error form, the other pool keeps answering, and a
   fresh `updateSnapshot` and a reconnect mint no id that aliases a retired
   one. The same script against the Go binary (without the fault) shows the
   wire error forms the client already handles.
3. **Benchmarks**: `npm ci --ignore-scripts` in the pin's root (ignored
   `node_modules`) on the running machine, then each bench file in
   single-iteration mode against Rust; completion without error is the
   result, timings are not recorded. Local or dispatch, never PR CI.
4. **Direct tests and routes**: every `WORK` row of `docs/PHASE6-tests.md`
   ported or given its owner-reviewed equivalent-coverage explanation; the
   two `api` baselines compared by a Rust encoder test.
5. **Ledger and records**: Rust homes for all Phase 6 files and the moved
   Phase 5 leftovers (`snapshothost.go`, `project/api.go`, `ls/api.go`,
   `cmd/tsc/api.go`); `docs/PHASE6-A5.md` with the final suite numbers, the
   retained entries and their approvals, and the Phase 7 hand-over.

Exit: section 6 holds.

## 6. Acceptance and counting

| Required result | How it is checked |
| --- | --- |
| The untouched sync and async suites pass | `status/parity/jsapi.json` holds only owner-approved entries; N and the native skip set come from the Go run of the same test command and binary placement |
| Direct Go tests | 31 `api` and 25 `encoder` tests ported or accounted for in `docs/PHASE6-tests.md` under `cargo test` |
| Codec and wire fixtures | `cargo xtask gen api --check`; the 36 pinned codec fixtures and the `proto_test.go` ports; byte goldens for both framings captured from the Go binary |
| API baselines | the two encoder baselines through a Rust comparison |
| Panic retirement through the real client | the A5 witness |
| Benchmark cases | run to completion once in single-iteration mode; no timing claim |
| Build quality | ordinary CI, `cargo xtask validate`, the publication policy for the extended public crates |

Let `N` be the number of client test cases the native run executes without
skipping (669 at this pin unless the Go run shows otherwise) and `F` the set
of cases that fail, crash, time out or are skipped only against Rust. A file
whose process dies contributes every unfinished case to `F`. Closure requires
`F` to contain only owner-approved entries, each naming the native behavior,
the Rust behavior and the reason. Approval does not remove a case from `F`.
Counting tests: several failures in one file; an approved failure; a missing
terminal event; a native skip; a Rust-only skip.

## 7. Main risks

| Risk | Answer |
| --- | --- |
| Symbol identity: the pin shares one `*ast.Symbol` across projects and ids it globally; Rust binds per file and merges per checker | The per-session mint from retained identity (decision 4) and the `symbol identity across projects` and `multi-project type ID uniqueness` families in A2's exit |
| The synchronous protocol correlates by method name and reads callback replies inline | Port `SyncConn`'s lock discipline exactly; byte goldens and the eight-thread callback witness (A1) |
| The snapshot host is a Phase 5 leftover whose cloning paths are tied to `Session`; the builder takes `&Session` | A2a gives the builder a build context first, then ports the three clone entry points; the L6 project tests guard the session path |
| Symbol identity across checkers: a retained symbol imports only into a lease of its exact owner, and merged symbols are checker-local | The two-kind registry key of decision 4; the `symbol identity across projects` family in A2's exit |
| Pagination byte accounting | Port the arithmetic literally; the client tests check page cuts and oversized single responses (A4) |
| Position and text conversions | UTF-16 request positions and byte ranges convert at the edge (ADR 0013); the `textedit` test and the client's position tests cover it |
| Node and the test runner | Node from the pin's `volta`, the pin's lockfile installed once per checkout, the reporter checked against a fake test file, concurrency raised only after measurement |
| A0's placeholder server hides a transport bug behind "not implemented" | A1 replaces it; the goldens and the `API` family distinguish transport from method failures |

## 8. Decisions

Accepted by the owner on 2026-10-07:

1. **No msgpack crate.** Hand-port the five-marker tuple codec with goldens;
   ADR 0017's anticipated dependency is not needed.
2. **Crate layout.** Extend `tsr_api` and `tsr_ipc`; generated DTOs as the
   module `tsr_api::proto`.
3. **Profiling methods** answer the LSP server's explicit unsupported error;
   pprof remains Phase 7.
4. **The `jsapi` suite and its denominator**: the pinned client's test cases,
   N from the native Go run of the same command, native skips visible.
5. **Benchmarks** run once in single-iteration mode as A5's correctness
   smoke, locally or by dispatch; PR CI never runs them. The `jsapi` suite
   itself needs the pin's `npm ci`, found in A0; that install is in PR CI.
6. **The API-over-LSP handshake** lands in A1.
7. **`snapshothost.go`** and `cmd/tsc/api.go` move to Phase 6 in the ledger.
8. **One pull request per checkpoint** against `main`.
9. **Sequencing**: A0 starts now, in parallel with Phase 5's L8.
10. **Allocation**: Astra builds A1 to A4; Claude builds A0, reviews every
    PR and keeps the records; A5 is split by crate.
11. **Plan review**: one read-only review round by Astra before this plan is
    final; done 2026-10-07 and incorporated.

## 9. What Phase 7 inherits

The sync and async API benchmarks and `nodelist.bench`, the three profiling
methods, retained-memory reporting for API sessions, and the owner's
dogfooding of the API through the extension.
