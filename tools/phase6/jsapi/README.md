# The `jsapi` suite

The pinned `packages/typescript` client's own tests, run unchanged against a
server binary. The client resolves the server it spawns through
`getExePath()`; in a repository checkout that is `upstream/built/local/tsc`,
a path the pin ignores, so the adapter places a symlink there and no client
file is patched. The pinned Go binary gives the native run that fixes the
denominator and the native skip set; `tsrust` gives ours. The two runs of a
file never overlap because they share that one path.

```sh
python3 scripts/phase5_replay_ci.py prepare --output target/phase6/binaries      # native-lsp and rust-lsp, once
python3 tools/phase6/jsapi/runner.py prepare --prepared target/phase6/parity/jsapi --binaries target/phase6/binaries
python3 scripts/parity.py run jsapi --runner target/phase6/parity/jsapi/prepared.json --output "$SCRATCH/jsapi"
python3 scripts/parity.py check jsapi --results "$SCRATCH/jsapi"
```

`parity.py run jsapi` without `--runner` prepares into `target/phase6/parity/jsapi`,
building both binaries. Node must be the version the pin's `volta` entry
names; the suites need no `npm install` (the package self-references through
its `exports` and vendors `vscode-jsonrpc`).

One variant is one test file (`jsapi/test/sync/api.test.ts`); one row is one
test case under its `describe` path (`jsapi/test/sync/api.test.ts/Snapshot >
updateSnapshot …`), with the file as the row's explicit `parent`. Node's
`--test` runs a file in its own process with `reporter.mjs` turning the test
events into JSON lines; a process that ends before a started test finishes
fails those cases and the file. Rows follow the L7 comparison rules: a Rust
skip the native run does not have fails, a case the native run did not
observe fails, and a native failure fails the file. `parity.py check jsapi`
prints the case counts; closure requires `status/parity/jsapi.json` to hold
only owner-approved entries.
