# Phase 6 A1 — transports and connections

A1 replaces the A0 skeleton's connection code with ports of the pin's
`internal/ipc` connections and transports, the msgpack protocol's binary
branch, the callback file system and the API server, and adds the LSP-hosted
API session handshake. The session itself is still the A0 skeleton: it
answers `initialize`, `ping` and `echo`, and every other method fails by
name until A2.

## What landed

- **`tsr_ipc::SyncConn`** (`conn_sync.rs`): the pin's synchronous connection.
  `run` reads one message and dispatches inline; `getServerTiming` and
  `resetServerTiming` are answered before dispatch and never recorded; a
  handler panic becomes an internal error whose message starts `panic:`
  followed by the sanitized backtrace; `call` holds the connection's lock,
  writes the request and reads the reply inline, rejecting any other message;
  `notify`. The two tests of `conn_sync_test.go` are ported, plus the
  eight-thread serialized-call test.
- **Binary responses**: `HandlerResult` is now `Option<Response>` with
  `Response::Json` and `Response::Binary`; `Protocol::write_binary_response`
  is implemented by the msgpack protocol and refused by JSON-RPC. The Phase 4
  content-mapper connections keep their JSON path (`Response::json`).
- **Transports** (`transport.rs`): `Transport`, `PipeTransport` over a
  Unix-domain socket listener (a stale socket file is removed first;
  `accept`, `close` unbinds the file, `path`), `StdioTransport` (one
  connection), `generate_pipe_path`, and `Stream::from_unix`. Windows named
  pipes remain unbuilt.
- **`tsr_api::callbackfs::CallbackFs`**: wraps the bundled file system,
  validates the six callback names, is connected after accept, and delegates
  each enabled operation through `Conn::call` with the pin's payloads
  (`readFile` with content, not-found and fall-through replies;
  `fileExists`, `directoryExists`, `realpath`, `getAccessibleEntries`,
  `writeFile`). A callback waits without a timeout and a failed call panics,
  as the pin's does.
- **`tsr_api::server`**: `StdioServerOptions` with the pin's fields,
  transport selection, callback wrapping, protocol and connection by mode,
  timing collection, `serve` for one accepted stream, and the `Session` trait
  the connections dispatch to (`Skeleton` implements it until A2; ids are
  `api-session-<n>` from one process-wide counter). An unknown method fails
  as the pin's payload decoder does:
  `api: invalid request: unknown API method "<name>"`.
- **`tsr_lsp::api_session`**: `custom/initializeAPISession` leaves the
  unimplemented list. It takes the client's pipe path or generates
  `tsgo-api-<time hex>-<random hex>` under the temporary directory, listens,
  answers `{sessionId, pipe}`, and on a background thread accepts one
  connection, closes the listener and runs an `AsyncConn` under a cancellable
  context with panic isolation (log, cancel, close); the session leaves the
  runtime's map when its connection ends.
- **Timing tests**: the four tests of `timing_test.go` are ported (the
  negative-duration clamp is a comment: `Duration` cannot be negative).

## Witnesses

- **Byte goldens** (`tools/phase6/wire/capture.py`, goldens in
  `tools/phase6/wire/golden/`): recorded from the pin's binary; the Rust
  server reproduces all 8 steps of each protocol with identical framing and
  value-identical payloads, including the exit code and stderr text after a
  client-side framing error. The `readFile` round trips join in A2.
- **Serialized callbacks** (`server.rs` test
  `callbacks_from_eight_threads_are_serialized_on_the_wire`): a request whose
  handler reads eight files from eight threads through the callback file
  system; an independent frame reader on the client side sees one complete
  `readFile` call at a time and every reply reaches its caller.
- **Disconnect** (`a_client_that_disconnects_mid_request_...`): the client
  closes while its request is being handled; the connection ends and the
  socket file is unbound.
- **The handshake** (`tools/phase6/wire/handshake.mts`): the untouched
  asynchronous client connects to the socket the LSP server announced,
  initializes, runs `getTimingInfo` and an unknown method, closes, opens a
  second session on a client-chosen pipe path, and shuts the server down. The
  pin's binary and `tsrust` print identical observations; no socket file is
  left behind.

The `jsapi` CI job runs the golden comparison and the handshake diff after
the suite.

## The suite against A1

Native: 12 files, 706 cases, no skips, no failures (unchanged from A0).

| Measure | A0 | A1 |
| --- | ---: | ---: |
| Cases passing | 86 | 89 |
| Cases failing | 620 | 617 |
| Files failing as a whole | 1 | 1 |

Cause labels of the failing cases (`runner.classify`):

| Label | Cases |
| --- | ---: |
| `server: not implemented: updateSnapshot` | 542 |
| `server: not implemented: createProgram` | 18 |
| `server: not implemented: parseJsonConfigFileContent` | 12 |
| `server: not implemented: parseConfigFile` | 12 |
| `server: not implemented: batchRequests` | 10 |
| `server: not implemented: parseCommandLine` | 9 |
| `cancelled by parent` | 8 |
| `server: not implemented: readConfigFile` | 4 |
| `server: not implemented: transpileModule` | 2 |
| `deadline: 180 s` | 1 |

`initialize` is answered, so every client now reaches the first session
method of its test and fails there; `updateSnapshot`, `createProgram` and
the configuration methods are A2's.

### The async worker deadline, revisited

`test/async/api.test.ts` still ends at the per-file deadline. Tracing the
JSON-RPC traffic through a logging wrapper shows every request answered
(540 of 540 in one run) and no server process surviving the tests; node
attributes the pending promise to the file's root test, not to a case. The
cause is therefore in the client's teardown after failed cases, and it is
expected to disappear as A2 answers the session methods. The per-file
deadline of the suite is lowered from 600 s to 180 s (the slowest native file
takes 1.6 s), which bounds the cost at one such file per run.

## Checks

- `cargo test -p tsr_ipc -p tsr_api -p tsr_contentmappertest -p tsrust -p tsr_lsp`
  pass; `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
  and `cargo xtask validate` pass.
- `python3 scripts/parity.py check jsapi` passes against the accepted
  expectation file.

## Review fixes after A5

The A1 review's findings and their fixes: the stray-response error, the
owned callback error texts and the host-specific wire expectations were
fixed in the first round (docs/PHASE6-A2.md, "Review fixes after A4"); the
second round adds:

- A malformed reply to a read callback (`readFile`, `fileExists`,
  `directoryExists`, `getAccessibleEntries`, `realpath`) panics, as the
  pin's read callbacks panic on their unmarshal errors; the write side keeps
  returning its errors.
- The wire probe reads each reply under its deadline: a silent server is
  recorded as a timed-out step and killed instead of blocking the capture.
