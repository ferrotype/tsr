# Carried Phase 5 test client

`overlay.py` leaves upstream files untouched. The compiled test binary contains
all pinned assertions. With `TSR_LSP_SERVER` unset it calls Go's server; with it
set, the client uses the private Rust endpoint. Fourslash retains one Rust
process across sequential sessions and resets it before reattachment. Pinned
cases that keep multiple clients alive under the same `testing.T` receive
independent temporary servers for the additional clients; these close with the
client and preserve the primary session. Different test owners cannot attach
concurrently. A broken
reset retires the worker. The original mapper spawner still handles mapper
requests. Project-state rendering uses the original writer with the checked
projection decoder from `tools/phase5/project`.

The transport applies FIFO backpressure after each 32 client notifications
through `test/barrier`. This private action does not read a snapshot or change
session state. It keeps the existing 64-operation endpoint bound intact. The
client waits after releasing its write mutex so filesystem and mapper callback
replies can progress while earlier notifications drain.

Four pinned files receive anchored access patches: `lsptestutil/lspclient.go`
selects the transport, `fourslash/statebaseline.go` reads the state projection,
`baseline/baseline.go` reports comparison outcomes, and `repo/paths.go` accepts
`TSR_UPSTREAM_ROOT` for downloaded test binaries. The last patch replaces the
build host's absolute fixture root with the execution checkout; it requires an
absolute path containing `go.mod`. Without that variable, the original Go root
lookup remains intact. No test or reference baseline is overlaid.

CI builds both Go assertion binaries, `test2json` and one release Rust server in
`phase5-runner`, then uploads a bundle. `bundle.py relocate` verifies executable
hashes, restores permissions and writes prepared manifests for the destination
checkout. Shards need the binaries and submodule fixture files; the pinned Go
installation is confined to the build job. Four fourslash shards each use one
retained worker, and the LSP shard uses fresh processes per test. Their merged
results are checked against the existing suite expectation files.

Prepare once, outside test deadlines (the command reports Cargo's executable,
including a custom `CARGO_TARGET_DIR`):

```
python3 tools/phase5/harness/runner.py prepare fourslash --prepared target/phase5/harness
python3 scripts/parity.py run fourslash --runner target/phase5/harness/prepared.json --output target/phase5/fourslash --jobs 4
```

Use `--id fourslash/TestName` for a development check. An explicit prepared
manifest is a prebuilt artifact, like `parity.py --runner`; rebuild it after
source changes. Normal runs without `--runner` prepare the current source.
Preparation requires the pinned Go locally and never downloads a Go toolchain.
It compiles `cmd/test2json` explicitly because some Go distributions lack that
standalone tool. Baselines and tracking output go under the selected output,
never into the submodule. Builds honor the shared Go cache.

`lsp` uses the explicit client-test routes listed in `docs/PHASE5-tests.md`.
Each client test starts a fresh test process. Direct server-internal tests and
recorded replay cases retain their separate documented routes; they do not
receive corpus credit from a client-test run.

Every batch first runs Go, then Rust, with identical assertions. Native skips
remain visible; unexpected Rust skips, missing observations, crashes and test
deadlines fail. Every row has an explicit top-level `parent`. Test, subtest and
baseline rows cannot multiply the count of failed top-level tests. A package
failure after otherwise passing tests aborts the run instead of recording a
successful capture. Raw native/Rust events and differing baseline files remain
under `local`; result metadata is written only after the entire run finishes.

The ordinary expectation file is `status/parity/fourslash.json` (and `lsp.json`).
`accept` records failures; it does not approve them or assert phase closure.
The fourslash summary shows the measured native execution count and failing
parent count, including approved failures. Owner approvals are unchanged.

For transport diagnostics only, `TSR_FAULT=hover`, `completion` or `baseline`
corrupts a selected actual response/output. The normal adapter clears inherited
fault selectors. Invoke the test binary directly under `supervisor.run_batch`
for these mutation witnesses. The wrong answer is still checked by the pinned
assertion. No alternate expected output is substituted.
