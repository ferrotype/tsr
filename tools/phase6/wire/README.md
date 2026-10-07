# Phase 6 wire witnesses

Two scripts observe the API server from outside, with nothing of the Rust
protocol code in the loop, and compare the Rust server with the pin's.

## Byte goldens (`capture.py`)

`record` drives a server binary through a fixed script on each protocol and
writes every frame sent and received, plus the exit code and stderr, to
`golden/<protocol>.json`; `compare` replays the script against another binary
and reports each step whose frames differ.

```sh
python3 tools/phase6/wire/capture.py record --binary target/phase6/binaries/native-lsp --output tools/phase6/wire/golden
python3 tools/phase6/wire/capture.py compare --binary target/release/tsrust --golden tools/phase6/wire/golden
```

The committed goldens come from the pin's binary (`native-lsp`, built by
`scripts/phase5_replay_ci.py prepare`). A comparison requires identical
framing bytes (the array and type markers, the binary-framed method, the
payload marker kind; the Content-Length header for JSON-RPC) and
value-identical payloads. Two placeholders apply: the server's current
directory, which `initialize` reports, and the measured timings
(`processingTimeMs`, `totalProcessingTimeMs`, `timestamp`). The exit code and
stderr after the client-side framing error are compared as text with the
current directory replaced.

The script: `initialize`, `ping`, `echo` (512 arbitrary bytes on the
synchronous protocol, a JSON value on the asynchronous one), an unknown
method, `getServerTiming`, `resetServerTiming`, `getServerTiming` again, and a
malformed frame that ends the connection. The `readFile` callback round trips
join the script with A2, when a method that reads files exists; `run_script`
already passes `--callbacks` when the step list asks for it.

## The LSP-hosted session handshake (`handshake.mts`)

Starts `<binary> --lsp --stdio`, initializes it, requests
`custom/initializeAPISession`, connects the untouched asynchronous client to
the announced socket with `API.fromLSPConnection({ pipe })`, runs
`getTimingInfo` and an unknown method, closes, opens a second session with a
client-chosen pipe path, and shuts the server down. Every observation is one
JSON line, so the output for the pin's binary and for `tsrust` is diffed:

```sh
node --conditions @typescript/source tools/phase6/wire/handshake.mts target/phase6/binaries/native-lsp > "$SCRATCH/native.jsonl"
node --conditions @typescript/source tools/phase6/wire/handshake.mts target/release/tsrust > "$SCRATCH/rust.jsonl"
diff "$SCRATCH/native.jsonl" "$SCRATCH/rust.jsonl"
```

Node is the pin's (`upstream/package.json`, `volta.node`); the pinned client
is imported by path, so no install is needed for this script.

## The real-client panic witness (`panic-witness.mts`)

`phase5_testserver --api` (crate `tsr_testhost`, built with `tsr_api`'s
`fault-injection` feature) serves the production API session behind the pin's
`api` flags with one test-only control, `testhost/faultNextCheckerOperation`
`{"snapshot": n}`, which makes the next checker operation of that snapshot
panic inside its operation. The script drives the untouched asynchronous
client against it: two snapshots sharing a project's checker pool and a
second project on its own pool; the fault on one snapshot; then the
panicking request fails with the connection's `panic:` error, both
snapshots' old handles are rejected (`the checker generation has retired`),
the other pool keeps answering, a file change rebuilds the project on a
fresh pool whose snapshot answers, a released snapshot's handle reports
`snapshot N not found`, and a reconnect answers. With `--native` the same
script runs against the pinned binary without the fault and shows the
ordinary error forms.

```sh
cargo build --release -p tsr_testhost --bin phase5_testserver
node --conditions @typescript/source tools/phase6/wire/panic-witness.mts target/release/phase5_testserver
node --conditions @typescript/source tools/phase6/wire/panic-witness.mts target/phase6/binaries/native-lsp --native
```

Each run prints one JSON line per step and a `verdict`; the exit code is the
verdict.
