# Phase 6 A5 — residuals, the panic witness and closure

A5 closes the client suite, ports the direct Go tests that were still
routed to work, adds the real-client panic witness, runs the pin's bench
files against the Rust server once, and gives the Phase 6 files their
ledger homes.

## What landed

- **The last by-name methods**: `getConstantValue` (the checker's constant
  value of an enum member or a constant reference, as JSON),
  `getJsDocTags` and `getDocumentationComment` (the pin's
  `GetSymbolJSDocTags` and `GetSymbolDocumentationComment` through
  `tsr_ls::api_server`: the comment of each distinct declaration, the tags
  as name/text pairs rendered as plain text, the `@typedef`/`@callback`
  rule), `getSignatureUsages` (the referencing names of a signature
  declaration paired with the calls they are callees of, the declarations'
  own names left out), and `getImportAdderEdits` (each exported symbol's
  best fix by the auto-import ranking over the project's registry,
  coalesced by the import adder, placed against the file and converted to
  UTF-16 offsets of the original text by `toAPITextEdits`; the action-kind
  and missing-symbol refusals carry the pin's texts).
- **`getExportSymbolOfSymbol`** reads the symbol's raw export link, as the
  pin's handler does (`sym.ExportSymbol`), and answers null without one; the
  clients answer the symbol itself in that case without asking. An A5 change
  that answered the checker's merged export symbol was reverted by the
  review fixes.
- **The real-client panic witness** (plan item 2): `phase5_testserver
  --api` (`tsr_testhost::api_witness`, with `tsr_api`'s `fault-injection`
  feature) serves the production session with one test-only control that
  arms a panic in the next checker operation of a named snapshot.
  `tools/phase6/wire/panic-witness.mts` drives the untouched asynchronous
  client: two snapshots on one project's pool and a project on another; the
  faulted request fails with the connection's `panic:` error, both
  snapshots' old handles are rejected with `the checker generation has
  retired`, the other pool keeps answering, a file change rebuilds the
  project on a fresh pool whose snapshot answers, a released snapshot's
  handle reports `snapshot N not found`, and a reconnect answers. Against
  the Go binary without the fault (`--native`) the same script passes and
  shows the ordinary error forms.
- **Bench smoke** (plan item 3): `test/sync/api.bench.ts
  --singleIteration`, `test/async/api.bench.ts --singleIteration` and
  `test/nodelist.bench.ts` complete without error against the Rust server
  (`upstream/built/local/tsc` pointed at it). Timings are not recorded, as
  the plan says; the owner's profiling pass is the place for them.
- **Direct tests and routes** (plan item 4): `TestJSONValueToAny` (through
  `parseJsonConfigFileContent`: key order, null elements and empty arrays
  kept), `TestToAPITextEditsUsesOriginalCoordinates`, the four
  `encoder_test.go` cases (the two baselines reproduced byte for byte from
  the pinned baseline files, now at `crates/tsr_encoder/testdata/api/`; the
  content-mapper metadata offsets and directive bytes; the node index table
  against the encoding), and the nine session tests still routed to A3:
  the empty root set, root order, the observable half of program reuse
  (changes seen, options applied; the Rust session loads programs afresh,
  so the pin's `ProgramUpdateKind` has no counterpart), project references
  echoed and resolved, a configured project's program dropping the other
  projects, temporary snapshots adding unopened files and deriving from the
  client's base, and the unloaded ancestor project left out of
  `updateSnapshot`.
- **Ledger** (plan item 5): every Phase 6 file has its Rust homes and
  `ported` status in `PORTS.toml`; `cmd/tsc/api.go`, `ls/api.go`,
  `project/api.go` and `project/snapshothost.go` move to Phase 6
  (`scripts/ledger-init.py`'s `API_SERVER_FILES`), and the manifest is
  regenerated.

## The suite against A5

Native: 12 files, 706 cases, no skips, no failures.

| Measure | A4 | A5 |
| --- | ---: | ---: |
| Cases passing | 685 | 706 |
| Cases failing | 21 | 0 |
| Files failing as a whole | 0 | 0 |

`status/parity/jsapi.json` has no failing entries and no approvals.

## Routes

The `decoder_test.go` cases are ported (`crates/tsr_encoder/src/decoder_tests.rs`,
the review fixes after A5): each parses its text, encodes it with the Rust
encoder, decodes it again and reads the decoded tree as the Go test reads
its own. `docs/PHASE6-tests.md` routes every `api` test.

## Known differences

- `createProgram` with an `oldProgram` loads the program afresh; the pin
  reuses or clones it (`TestCreateProgramReusesProgram`'s update kinds).
  The responses are the same; `build_program` must mark file changes when
  reuse lands (docs/PHASE6-A2.md).
- An LSP-hosted session's snapshots carry the hosting session's user
  preferences as they stand when each snapshot is stored (the pin's are
  those of the project snapshot); a standalone session has the defaults.
- The language service builds the auto-import registry on demand from the
  project's cache; the pin clones the snapshot with auto-imports and
  retries (`clone_with_auto_imports`, no Rust counterpart).
- The differences recorded in A3 (tuple metadata, template-literal
  `getTypes`) stand; `getRestTypeOfSignature` was corrected by the review
  fixes.

## Checks

- `cargo test -p tsr_api -p tsr_ls -p tsr_encoder -p tsr_testhost`, the
  clippy gate on the changed crates and `cargo xtask validate` pass.
- `python3 scripts/parity.py check jsapi` passes against the accepted
  expectation file.
- The panic witness passes against `phase5_testserver --api` and, without
  the fault, against the pinned binary.

## Review fixes after A5

- The witness verdict checks what this record claims: the faulted request's
  `panic:` error, both old handles rejected with `the checker generation has
  retired`, the other pool answering, a fresh snapshot with a new id, the
  released snapshot's `not found`, and the reconnect; against the pin, the
  old handles answering and the same closing checks. CI runs it in the jsapi
  job against the test server (`test-server` among the prepared binaries)
  and, without the fault, against the pin.
- `phase5_testserver --api` is `StdioServer` itself with the fault control
  through its session hook (`StdioServerOptions::session_hook`), so the
  witness serves on the path `tsrust --api` takes; only the command's mapper
  spawner is absent.
- The copied encoder baselines are checked against the pin's files when the
  submodule is present.
- `getJsDocTags`, `getSignatureUsages` and `getImportAdderEdits`' failure
  case answer `[]`, the pin's nil slice.
- The API's language service converts positions with the session's
  encoding, as the pin's snapshot converters do: the standalone server's
  is UTF-8, so the LSP character `toAPITextEdits` adds to a line's byte
  start is a byte count, and an import extended past a non-ASCII
  identifier is reported at the UTF-16 offset after it, not inside it.
  Completions build their LSP position through the same converters. A
  hosted session follows the LSP's negotiated encoding, where the pin's
  addition mixes units on a non-ASCII line in the same way.
- `getImportAdderEdits` takes the snapshot's user preferences and format
  settings, and `formatNodeForInsertion` the snapshot's format settings
  with its new-line preference, as the pin's handlers read them; a hosted
  session answers them from the LSP session's configuration.
