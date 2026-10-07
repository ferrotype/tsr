# Phase 6 A2 — sessions, snapshots and the lifecycle handlers

A2 replaces the connection skeleton with the API session of the pin's
`session.go`: snapshots by handle with reference counts, the open project
and file sets, and the lifecycle, configuration, transpile and source-file
handlers. The checker-backed handlers and the symbol, type and signature
registries follow in A3, where the first query needs them.

## What landed

- **`tsr_api::session::ApiSession`** (`session/mod.rs`): the snapshot map
  with reference counts and the latest-snapshot diff base, the open sets
  with the pin's one-ref-per-path rule, `updateSnapshot` (dedupe against the
  sets, the project session's `api_update`, commit the sets only on success,
  reuse an existing snapshot id, `computeSnapshotChanges` against the
  previous latest), `updateTemporarySnapshot` (retain the base while cloning,
  diff against the client's base, never the latest), `createProgram`
  (`fileChanges` requires `oldProgram`; roots absolute to `cwd`; the base is
  the old snapshot or a fresh update; the synthetic inferred project is the
  response), `release`, `getDefaultProjectForFile`, `initialize`, the pin's
  `echo` and `ping`, and `close` (release held refs, drop snapshots, close a
  standalone project session). Errors carry the pin's classes:
  `api: client error: …` and `api: invalid request: …`.
- **Snapshot host.** The pin split `SnapshotHost` out of its project session
  so API sessions could derive snapshots without session side effects. The
  Rust project session already shares its caches behind `Arc`s, so the host
  role stays with `tsr_project::session::Session`: `derive_snapshot` is the
  pin's `SnapshotHost.update`, factored out of `update_snapshot`, which now
  adopts what it derives; `clone_with_temporary_file` and `clone_for_program`
  derive without adopting. A standalone API session owns a private project
  session whose current snapshot is the pin's compatibility snapshot, so the
  linear `updateSnapshot` chain is the same; an LSP-hosted session shares the
  server's. `ProjectBuilder::build_program` builds the one synthetic inferred
  project of `createProgram` from explicit roots and options, seeded from the
  old program's project.
- **Configuration and transpilation** (`session/config.rs`):
  `parseCommandLine`, `readConfigFile` (`{}` and the `Cannot read file`
  diagnostic for a missing file), `parseJsonConfigFileContent` (exactly one
  of `configDirectory` and `configFileName`), `parseConfigFile`,
  `transpileModule`/`transpileDeclaration` and their `FromFile` variants.
- **Source files** (`session/sources.rs`): `getSourceFile` (the Phase 0
  encoder's bytes, raw on msgpack and base64 on JSON-RPC, null for an
  unknown file), `getSourceFileNames`, `getSourceFileMetadata`,
  `getConfigFileNames`, `getConfigSourceFile` (the root config or an
  extended one, parsed from the snapshot's file system).
- **Responses** (`session/responses.rs`): `NewProjectResponse`,
  `NewConfigFileResponse` (with `toProtocolJSONValue` for the raw object),
  `newDiagnosticResponse` (UTF-16 positions, line and character, the four
  source lines), `ToDiagnostic`, `computeSnapshotChanges`.

## Direct tests

`crates/tsr_api/src/session/tests.rs` ports the observations of
`session_apistate_test.go`, `session_temporary_test.go` and
`session_createprogram_test.go` that need no checker: the standalone
session's updates, idempotent opens and held-ref closes, source-file
encoding by protocol, the configuration handlers' texts, the temporary
snapshot (distinct handle, latest unchanged, base text intact, unsupported
extension), and `createProgram` (one synthetic project, root files and
options, the `fileChanges requires an oldProgram` text). Snapshot ids are
not compared: the Rust counter is process-wide where the pin's is per host.
The semantic-diagnostic halves of those tests join with A4;
`docs/PHASE6-tests.md` routes each.

## The suite against A2

Native: 12 files, 706 cases, no skips, no failures.

| Measure | A1 | A2 |
| --- | ---: | ---: |
| Cases passing | 89 | 287 |
| Cases failing | 617 | 419 |
| Files failing as a whole | 1 | 1 |

Cause labels of the failing cases:

| Label | Cases |
| --- | ---: |
| `server: not implemented: getSymbolAtPosition` | 175 |
| `server: not implemented: printNode` | 22 |
| `server: not implemented: getTypeAtLocation` | 18 |
| `server: not implemented: getSemanticDiagnostics` | 16 |
| `server: not implemented: getSymbolAtLocation` | 14 |
| `server: not implemented: getTypeAtPosition` | 14 |
| `server: not implemented: emit` | 13 |
| `server: not implemented: batchRequests` | 12 |
| `server: not implemented: resolveName` | 10 |
| `server: not implemented: getCompletionsAtPosition` | 8 |
| `server: not implemented: getStringType` | 8 |
| `server: not implemented: getSyntacticDiagnostics` | 8 |
| `server: not implemented: getSymbolsInScope` | 6 |
| `server: not implemented: getJavaScriptEmit` | 6 |
| `server: not implemented: formatNodeForInsertion` | 6 |
| `server: not implemented: getConfigFileParsingDiagnostics` | 4 |
| `server: not implemented: getSymbolOfSourceFile` | 4 |
| `server: not implemented: getContextualType` | 4 |
| `server: not implemented: getResolvedSignature` | 4 |
| `server: not implemented: getNumberType` | 4 |
| `server: not implemented: getConstantValue` | 4 |
| `server: not implemented: getBindDiagnostics` | 4 |
| `server: not implemented: getGlobalDiagnostics` | 4 |
| `server: not implemented: getSymbolsOfSourceFiles` | 2 |
| `server: not implemented: getReferencedSymbolsForNode` | 2 |
| `server: not implemented: getSignatureUsages` | 2 |
| `server: not implemented: getTypesAtPositions` | 2 |
| `server: not implemented: getTypeFromTypeNode` | 2 |
| `server: not implemented: getAnyType` | 2 |
| `server: not implemented: getBooleanType` | 2 |
| `server: not implemented: getVoidType` | 2 |
| `server: not implemented: getUndefinedType` | 2 |
| `server: not implemented: getNullType` | 2 |
| `server: not implemented: getNeverType` | 2 |
| `server: not implemented: getUnknownType` | 2 |
| `server: not implemented: getBigIntType` | 2 |
| `server: not implemented: getESSymbolType` | 2 |
| `server: not implemented: getNonPrimitiveType` | 2 |
| `server: not implemented: getShorthandAssignmentValueSymbol` | 2 |
| `server: not implemented: getSignatureFromDeclaration` | 2 |
| `server: not implemented: getExportSpecifierLocalTargetSymbol` | 2 |
| `server: not implemented: isContextSensitive` | 2 |
| `server: not implemented: getDeclarationEmit` | 2 |
| `assertion failed` | 2 |
| `server: not implemented: getSuggestionDiagnostics` | 2 |
| `server: not implemented: getDeclarationDiagnostics` | 2 |
| `server: not implemented: getProgramDiagnostics` | 2 |
| `server: not implemented: emitToString` | 2 |
| `deadline: 180 s` | 1 |
| `server: unknown method: unknown` | 1 |

The two `api.test.ts` files now reach the checker: their failures are the
A3 queries (`getSymbolAtPosition`, `getTypeAtLocation`, `resolveName`, …)
and the A4 features (`printNode`, `emit`, diagnostics, completions,
`batchRequests`). `test/async/api.test.ts` still ends at the per-file
deadline for the reason recorded in A1.

## Known differences left for later checkpoints

- `createProgram` with an `oldProgram` loads the program afresh rather than
  reusing the old program's structure (`TestCreateProgramReusesProgram`);
  the response is the same.
- `parseConfigFile` omits `typeAcquisition` when the configuration does not
  set it, as the pin's accessor does; a configuration that sets it to an
  empty object is also omitted here where the pin sends `{}`.

## Checks

- `cargo test -p tsr_api -p tsr_project -p tsr_lsp -p tsrust`, the clippy
  gate and `cargo xtask validate` pass.
- `python3 scripts/parity.py check jsapi` passes against the accepted
  expectation file.
