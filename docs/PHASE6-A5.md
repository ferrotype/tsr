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
- **An A3 correction found on the way**: `getExportSymbolOfSymbol` answered
  null for a symbol without an export link; the pin answers the symbol
  itself.
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

## Routes still owed to the owner

`docs/PHASE6-tests.md` keeps the nineteen `decoder_test.go` cases routed to
work with an equivalent-coverage note for the owner to review: the Rust
decoder reads the format the Phase 0 corpus parity froze over 16,120
files; it is exercised by `printNode` and `formatNodeForInsertion` over
client-encoded trees (the `tsr_api` printing and formatting tests, the
jsapi `SnapshotInternalAPI` and `printNode` cases), and every server
encoding is decoded by the client suites (`test/encoder.test.ts`,
`test/sync/ast.test.ts`). A port of the nineteen cases is the alternative
if the owner prefers one.

## Known differences

- `createProgram` with an `oldProgram` loads the program afresh; the pin
  reuses or clones it (`TestCreateProgramReusesProgram`'s update kinds).
  The responses are the same; `build_program` must mark file changes when
  reuse lands (docs/PHASE6-A2.md).
- The standalone session formats insertions and runs the import adder with
  default settings and preferences; an LSP-hosted session's are the
  server's.
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
