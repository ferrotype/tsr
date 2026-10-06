# Bounded L7.5 replay

`python3 tools/phase5/replay/replay.py list` lists the three session ids.
`generate.py` recreates the committed fixtures and newline-delimited sessions.
All dependencies are local; automatic type acquisition is disabled. No command
builds a server, uses npm, or reads a dependency outside the copied fixture.
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
Two tiny Go captures per fixture compared equally. One references attempt exited
with `context canceled`; a second pair completed and compared equally. The checkjs
Go/Rust request results and non-diagnostic traffic matched in the bounded checkjs
capture, but pushed config diagnostics differed: Go published 32 diagnostics twice;
Rust published 22, then 32, then two empty lists. Rust’s first publication lacked
ten missing-global-type diagnostics (code 2318). An enabled-validation control
also differed (Go counts 32/32/32, Rust 22/32/22/32), separating accumulated-global
publication timing from validation clearing. Raw mismatches are retained and block
accepted expectations until reviewed. After strict reader/comparator changes, one
additional Go checkjs pair matched with executed-message comparison enabled.
After the dependency/Unicode fixture updates, repeated Go captures matched for
all three fixtures in both UTF-8 and UTF-16. This harness
is supporting evidence, not full corpus credit or a parity acceptance claim.
