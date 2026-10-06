# L7 latency capture

This command supplies tooling, not a performance result. The owner still chooses
and approves the latency fixture. No dependencies are downloaded, no binaries are
built, and no fixture is silently substituted for the owner's selection.

```
python3 tools/phase5/latency/capture.py \
  --fixture /absolute/local/fixture --scenario /absolute/scenario.json \
  --go-command '["/absolute/go-lsp","--lsp","--stdio"]' \
  --rust-command '["/absolute/tsrust","--lsp","--stdio"]' \
  --pairs 3 --smoke --output /tmp/lsp-latency-smoke
```

A smoke capture permits 1 or 3 pairs and is never accepted as a performance
record. A run of record permits 20 or 40 pairs; the quiet-host run belongs to the
owner. Every pair uses fresh runtime processes and identical restored fixture
bytes at the same copied root; runtime order alternates Go/Rust then Rust/Go.
Fresh process means a cold server cache, not an assertion about OS page cache.
All fixture symlinks must be relative and resolve inside the supplied fixture;
absolute targets are rejected because copying would retain the original root.

The scenario JSON supplies `encoding` (`utf-8` or `utf-16`), `open` (`path`,
`languageId`, positive `version`, optional exact `text`), `warmup_hover` (full hover
params at a different position than the measured hover), `edit` (full didChange
params, one ranged one-character change, document version open+1), and `requests`
(exactly `completion`, `hover`, `references`, `rename`, each full request params).
Use `@PROJECT_ROOT_URI@` and `@PROJECT_ROOT@` placeholders in request params.
The positions/versions are fixed in the supplied JSON; no symbol search or result
normalization changes them. Optional `initialize` supplies full initialize params.
The default advertises the chosen position encoding and error-only logs, with
ATA disabled. A custom initialize must explicitly disable ATA through
initializationOptions.userPreferences, omit workspace configuration callbacks
(which could override that preference), and omit work-done progress support.

The five metrics, in nanoseconds, are:

- `first_diagnostics`: didOpen write to the first complete pushed publication
  received after didOpen, whose URI equals the opened document, whose integer `version` equals the open
  version, and whose `diagnostics` is an array (empty is a valid complete report).
  Other document/version publications never satisfy the predicate. Missing or
  unversioned target publications fail the 20-second deadline; there is no
  substitution of a config diagnostic or a pull request.
- `completion`: request write to the full response after the fixed one-character
  edit.
- `hover`, `references`, `rename`: request write to full response after the same
  first diagnostics and one hover elsewhere, following completion.

Write timestamps occur immediately before writing the framed bytes. Completion
timestamps occur in the reader thread immediately after reading the complete
body, before JSON decoding and consumer scheduling. The strict replay frame reader
still validates every message before it becomes usable evidence.

Every request, error, server notification and server request remains visible in
raw evidence. Server stderr is saved to each runtime’s `stderr.log` without a
pipe that can stall the server. Raw-artifact write errors still retire the process.
Normalized correctness comparison reuses replay's exact typed
comparison, root/id correlation and per-document diagnostic streams. It includes
initialize/warmup/shutdown responses and executed client messages, as well as
measured responses. Any mismatch retains both transcripts under `pair-NN/mismatch`,
fails the command and excludes both runtimes' samples from that pair. No failed
pair is replaced. Protocol failures/timeouts preserve raw evidence and fail the
capture. Samples from matched pairs remain visible even if another pair fails.

`OUTPUT/samples.json` contains pair order, correctness status, five sample arrays
for both runtimes, host/revision/time, fixture and scenario SHA-256 identities,
and the exact scenario/commands plus executable SHA-256 identities. Pair directories contain raw frames/messages
and normalized transcripts. `read_capture(Path)` accepts the output directory or its JSON file only for
complete, matched, nonsmoke 20/40-pair runs with exact positive integer metric
arrays matching their pair evidence. `scripts/perf.py` computes ratios and
confidence intervals from the sample arrays; this capture does not manufacture
a pass from a median. Intervals touching 1.0 are inconclusive: extend a 20-pair run
once to 40 pairs; a still-inconclusive result belongs to the owner.

Focused verification uses fake framed servers only: fresh alternating processes,
correctness rejection, URI/version diagnostic selection, and smoke/partial or
inconsistent record rejection. These checks provide no latency-fixture credit.

Current pin boundary: the only pushed diagnostic construction found in the
pinned project/LSP implementation is `session.go:1935`, publishing a config URI
without a version. This cannot satisfy the required opened-source URI/version
predicate. A production capture remains blocked pending an explicit decision on
the metric; the tool does not silently substitute pull diagnostics.
