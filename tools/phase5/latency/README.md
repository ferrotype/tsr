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
The capture output must be empty and outside the fixture; evidence is never overwritten.

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
It must advertise exactly the scenario encoding, which the server must select.
The edit, warm-up and measured requests target the opened document. Warm-up hover
uses a distinct position even when other request options differ.

The five metrics, in nanoseconds, are:

- `first_diagnostics`, default `{"mode":"push"}`: didOpen write to the first complete pushed publication
  received after didOpen, whose URI equals the opened document, whose integer `version` equals the open
  version, and whose `diagnostics` is an array (empty is a valid complete report).
  Other document/version publications never satisfy the predicate. Missing or
  unversioned target publications fail the 20-second deadline; there is no
  substitution of a config diagnostic or a pull request.
- Explicit `"first_diagnostics": {"mode":"pull", "params": {"textDocument":
  {"uri":"@PROJECT_ROOT_URI@/src/ast/utils.ts"}}}` instead measures didOpen write
  to the complete `textDocument/diagnostic` response to an immediately following
  pull. It requires `kind: "full"` and an `items` array, including an empty array;
  unchanged, error and incomplete reports fail. No previous-result id or progress
  tokens are allowed. This measures project loading plus checking plus the pull
  exchange; it does not measure source push. Both the executed pull and its exact
  response are retained and compared. The mode is explicit in capture metadata.
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
Use `--pairs 40 --extend /absolute/original-capture --output /absolute/new-output`
to preserve all first-twenty samples and artifacts and add twenty pairs. Extension
requires the same host (including hostname), revision, fixture/scenario bytes and commands/binary
identities, with the original complete matched raw evidence present. Original
frames/messages and transcript agreement are revalidated before copying;
validation/copy failures write a rejected capture report without starting new
servers. A fresh
`--pairs 40` capture is separate evidence; it does not extend an earlier run.

Focused verification uses fake framed servers only: fresh alternating processes,
correctness rejection, URI/version diagnostic selection, and smoke/partial or
inconsistent record rejection. These checks provide no latency-fixture credit.

The supplied tool supports both diagnostic protocols; accepting pull as L7's
metric remains an owner decision. At the pin, the only pushed diagnostic construction found in the
pinned project/LSP implementation is `session.go:1935`, publishing a config URI
without a version. This cannot satisfy the required opened-source URI/version
predicate. No production latency capture has been run.

## Proposed pinned fixture, ready for review

`proposals/typescript-pull.json` fixes UTF-16 positions in the pin's
`upstream/packages/typescript/src/ast/utils.ts`: warm-up on the `SyntaxKind`
import, one space inserted at line 0, enum-member completion after `SyntaxKind.`,
and hover/references/rename on the imported `Node` type. Source inspection
verifies these positions, not response parity or meaningful measured results.
The package has 108 source files / 35,292 source lines and already carries its
vscode-jsonrpc declarations. Its only added declaration dependencies are
`@types/node` 22.20.1 and its `undici-types` 6.21.0 dependency, fixed by
`upstream/package-lock.json`. Preparation supplies their declarations inside the fixture.

`prepare.py` exports the package and lock from the exact Git pin, ignoring local
edits/build outputs, and verifies both caller-supplied npm archives against the
lock's SHA-512 integrity before extraction. It adds only
`compilerOptions.customConditions: ["@typescript/source"]` to `tsconfig.json` so
the package's `#enums/*` imports resolve to the pinned sources. Provenance records
the pin, source archive/lock hashes, dependency versions/integrities/archive
hashes and config overlay. Preparation does not download, build or approve the
fixture. Obtain the two locked archives separately before this offline step:

```
python3 tools/phase5/latency/prepare.py \
  --node-archive /absolute/node-22.20.1.tgz \
  --undici-archive /absolute/undici-types-6.21.0.tgz \
  --output /absolute/typescript-latency-fixture
```

After fixture and pull-metric approval, use that fixture with the proposed
scenario in the capture command above. Build the ordinary binaries once, outside
the measured clocks, with
`python3 scripts/phase5_replay_ci.py prepare --output target/phase5/latency/binaries`.
Set `GOGC=100` for captures on the authorized host. The binaries are
`native-lsp` and `rust-lsp` in that directory. Run three smoke pairs to establish
response parity, then the owner records twenty pairs on a quiet host. Record with
`python3 scripts/perf.py record lsp --capture /absolute/capture --label 'owner host'`
and compare with `python3 scripts/perf.py check lsp`. The five `[lsp]` thresholds
are 1.0, as required by L7.6.3's no-regression rule. Fixture and pull-metric
approval remain separate from that registered limit.

The dispatch-only `perf.yml` offers `lsp`, requiring explicit prepared fixture
and scenario paths. It builds the two ordinary binaries, records twenty or forty
pairs, supports an optional original twenty-pair extension path, and uploads raw
evidence even after capture failure. It never installs fixture dependencies or
selects the proposed scenario automatically. Dispatch integration is not a run
of record; the owner still chooses the quiet runner and reviews the result.
