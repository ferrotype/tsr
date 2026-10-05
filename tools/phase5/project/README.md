# L1 projects and synchronous filesystem bridge

The private `phase5_testserver --stdio` executable wraps the production
`tsr_lsp::Server` and `tsr_project::Session`. It keeps one ordered project worker
and a separate router that continues receiving host replies while that worker
is blocked. `tsrust` does not depend on this test-only executable. Normal LSP
initialization, diagnostics publication and client watcher registration are
implemented by [L2](../lsp/README.md).

## Run the focused checks

```sh
cargo test -p tsr_project -p tsr_testhost -p tsr_lsp --lib
PATH=/Users/cristian/.local/share/mise/installs/go/1.27.1/bin:$PATH python3 tools/phase5/project/check.py
```

For another host, put the Go version in `data/s04/toolchains.toml` on PATH.
The second command compiles a Go test overlay and the private Rust executable,
then exercises `cases.json`: initial state, opening a configured project,
repeated reads, an edit, inferred projects/options, closing and cleanup. It
compares **the original pinned Go state writer's bytes**, including identity
comparisons, library filtering and config-retention reporting. The native
projection must first reproduce direct snapshot access. Rust supplies data,
never a reconstructed Go project. The temporary overlay does not edit upstream.
The same sequence then runs twice in one Rust process across reset and must
match the fresh-process output. No capture archives or status regeneration are
needed for these tests.

## Private protocol, version 3

S11's version-2 transport remains unchanged. This executable accepts version 3
of `test/initialize`: the existing filesystem/callback/plugin/options fields
plus `project` with `currentDirectory`, `defaultLibraryPath`,
`positionEncoding` (`utf-8` or `utf-16`), optional `runExternalCode` (false), and
optional `progressDelayNanos` (0, matching Go's test server default).
Compiler options use the native compiler-struct JSON shape, not tsconfig text.
Its response and `testhost/initialized` notification carry version 3.

An ordinary LSP `initialize` after this barrier starts the production runtime;
its negotiated position encoding replaces the private descriptor's initial
encoding. `initialized` completes client configuration/watch registration and
emits `testhost/lspInitialized` for the carried Go client's `InitComplete` wait.
The ordinary `shutdown` response disposes the LSP session; `exit` closes this
connection. Filesystem callbacks continue to run on the independent reader.
`test/setOptions` between `initialize` and `initialized` stores options for the
first session, matching `SetCompilerOptionsForInferredProjects` at the pin.

- `textDocument/didOpen`, `didChange`, `didClose`, `didSave` and
  `workspace/didChangeWatchedFiles` use the generated protocol codecs and the
  actual project session. They are notifications, processed in input order.
- `test/projectState` (empty parameters) waits for preceding project actions
  and returns a version-1 read-only projection. Program, source-file and config
  identity tokens remain stable within a test; they are reset between tests.
- `test/setOptions` uses S11's internal completion hook. Its response waits for
  the real project update; a failed application does not leave its options
  queued for the next request. An update is refused while prior project work
  is pending, and new project work is refused while options are being applied.
- `test/reset` cancels and joins old work, retires callback reply slots, drops
  the session/projection/streams and acknowledges `{ "reset": true }`.
  `test/shutdown` performs the same barrier and then closes the connection.
  Only injected raw and mapped parse caches survive reset. Deletion is disabled
  only in this test worker. Callback IDs never restart, so late replies cannot
  satisfy the next test's calls.
- S11 filesystem callbacks and plugin streams share the connection. The plugin
  stream transport feeds the production mapper host; transforms can wait while
  the independent reader pumps stream data, credits and callback responses.
  `test/state` retains its existing version-2 **host descriptor** schema and is
  not the project snapshot. It can be answered while the project worker waits.
- `$/cancelRequest` cancels that operation's filesystem scope. Host progress is
  forwarded as `testhost/progress`; an oversized progress result fails the
  callback without killing the connection. A failed document notification
  emits `testhost/failure` and requires reset. Panics retire the same test.

The project queue admits 64 in-flight operations, with a reserved reset slot.
Malformed frames, duplicate live IDs and overflow terminate the test worker;
a supervising harness must mark the active test failed and restart. Frames and
JSON retain S11's bounds and strict Unicode limitation. This endpoint does not
claim the full fourslash subprocess supervisor or transport patch is complete.

## Direct-test scope

The Rust tests port the core observations, often combining related Go tests
into one state transition test; their count is not a count of native tests.

| Pinned tests/families | Rust home and scope |
| --- | --- |
| Overlay processing, snapshot filesystem, bulk invalidation, realpath aliases/watch filtering, source filesystem | `tsr_project` overlay/snapshot_fs/source_fs tests: old values, misses, dirtiness, alias lifecycle and frozen reads |
| Ref-count caches and extended config ownership | parse_cache/ref_count_cache/owner_cache/config tests: concurrent construction, failed publication, program leases, final disposal, mapped bundles and reconstruction |
| Project/session snapshots, program update kind, project lifetime/default selection and config changes | session/config tests plus the native writer sequence: real programs, old snapshots, configured/inferred transitions, custom filenames and checker retirement |
| Checker pool routing, request/file affinity, contention, cancellation, idle cleanup, discard, stable API/diagnostics ownership and global diagnostics | scheduler and ownership tests with barriers and a manual clock; no shortened wall-time assumptions |
| Watch path grouping, cloning and timeout rollback; background queue, dirty maps and log trees | watch/background/dirty/logging tests; actual client registration remains L2 |
| Untitled inferred projects and display names | session tests: actual text/program membership and project names; reference/definition results remain L3, ATA imports remain L6 |
| Private bridge/reset | testhost bridge/project_host tests: several blocked callbacks, out-of-order replies, progress, cancellation, disconnect, late responses, full-queue reset and real session/cache isolation |

L6 adds project-reference faking, API snapshot construction, mapper execution
and ATA tests. Their exact scope and native checks are in
[the L6 record](../../../docs/PHASE5-L6.md). The core tests do not certify all
subcases of the native project suite or L7's full fourslash replay.
