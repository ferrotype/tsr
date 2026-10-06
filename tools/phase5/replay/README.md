# Bounded L7.5 replay

`python3 tools/phase5/replay/replay.py list` lists the three session ids.
`generate.py` recreates the committed fixtures and newline-delimited sessions.
All dependencies are local; automatic type acquisition is disabled. The replay
runner never builds a server, uses npm, or reads dependencies outside the copied fixture.
References has three composite projects with declaration maps enabled; all twenty
core dependency files are explicitly included and re-exported through core/index; checkjs
has JSDoc and a CommonJS dependency; monorepo has a relative package symlink,
conditional exports and local ambient types. Each fixture contains 20–80 files.

Run or record with a prebuilt executable (absolute path), with `--command` last:

```
python3 tools/phase5/replay/replay.py record --id checkjs --output /tmp/go-replay --command /absolute/go-lsp --lsp --stdio
python3 tools/phase5/replay/replay.py compare --id checkjs --expected /tmp/go-replay/transcript.json --output /tmp/rust-replay --command /absolute/tsrust --lsp --stdio
```

`run` and `record` have the same capture contract; neither updates acceptance
files. `compare` exits 1 for any mismatch and retains expected/actual JSON under
`mismatch/`. All captures retain unnormalized `raw.json`: original incoming
messages/ids, complete executed client messages (including resolved completion
params and server-request replies), and incoming frame headers/body hex. The
normalized transcript binds the executed messages as well as all server results.
Select `--encoding
utf-8` or `--encoding utf-16` (default). CI integration must run both separately.

CI builds the ordinary pinned Go `cmd/tsc` and Rust `tsrust` executables once
in the existing `phase5-runner` job, using its pinned Go setup and Rust release
cache. This adds one native CLI build and one Rust CLI link/build; the latter
reuses dependencies compiled for the private assertion server. No build timing
or performance claim is inferred. Only the LSP shard downloads these binaries
and invokes `scripts/phase5_replay_ci.py run`, covering all three sessions in
both encodings against fresh native and Rust processes. Any capture error or
comparison mismatch fails that job. The `phase5-replay-results` artifact retains
raw transcripts, stderr, comparison files and the six-case summary on failures;
it is separate from the functional parity expectation artifacts. The helper's
`prepare --output DIR` builds the executables; `run --binaries DIR --output DIR`
only executes the bounded matrix and restores downloaded executable permissions.

The first session line names the fixture and project placeholders. Subsequent
lines follow pinned `{kind, method, params}` format. One explicit scripted
extension, `{"$response":2}`, resolves the first completion item from the third
request response. An empty or invalid completion response fails capture; it is
never silently skipped. The header schedules real config and source-file disk
mutations immediately before watched-file notifications, alongside workspace
configuration changes.
Header positions supply exact UTF-8/UTF-16 expansions for `$position` markers.
Every fixture queries a symbol after `/*😀*/`, so rename/reference ranges differ
by encoding (UTF-8 start 9 versus UTF-16 start 7 on the queried line). These
response ranges remain exact; no encoding normalization is applied.
Every sent entry remains in pinned replay format after scripted expansion. Mutation paths must stay inside
the fixture, including after symlink resolution.

Named normalization is limited to scratch root paths/URIs (including the pin's
lowercase root on a verified case-insensitive filesystem), response correlation
by session position, server request ids by stream position, and pushed diagnostics
in order per document. Other notifications preserve stream order. Responses keep
full JSON including errors, array order, absent versus null, and nested ids.
Comparison distinguishes booleans from numbers; JSON numeric values `1` and
`1.0` are equivalent. Parsing rejects duplicate keys and nonfinite numbers,
including numeric overflow. The reader validates JSON-RPC message structure,
integer/string ids, and exactly one result/error in every response. Only clean
EOF between frames is allowed after exit; partial/malformed frames and unexpected
responses fail capture, including after the final response.

Unknown server requests fail capture instead of fabricating an answer. Supported
configuration replies match the request item count; registration and diagnostic
refresh requests receive null acknowledgments. Work-done progress is not advertised.

Sessions request supported initialization log verbosity 5 (errors only). Logs
remain compared exactly; informational performance logs are prevented at source.
The completion scenario asks for a single object property because the pin’s global
completion array order varied between repeated Go captures. Arrays are never sorted.
The header's `notification_waits` schedule waits for the configured project's
first publication after open, the accumulated-global publication after the first
pull, and validation clearing after the disabled pull. This is client scheduling,
not output normalization: all notifications, their contents and their order remain
compared. Waiting applies the same verified scratch-root normalization as replay;
it never case-folds document paths independently. A missing publication times out.

The initial unsynchronized check-JS captures differed: Go published counts 32/32,
while Rust published 22/32/0/0. A direct native probe waiting after open observes
22, then 32 after checking: the first-count difference was background scheduling,
not missing global diagnostics. Rust did also clear twice; validation is now
captured with each immutable project snapshot, matching the pin's publication
transition. The synchronized fixture observes 22/32/0 on both runtimes.

After these changes all three fixtures compare equally in UTF-8 and UTF-16 on
both native and Rust servers. Raw stderr is retained separately without a bounded
pipe that can block the server. Raw-output failures still retire the child. This
is bounded replay verification, not a quiet-host latency result or full corpus
closure.
