# Phase 6 A4 — diagnostics, emit, batches, completions and references

A4 adds the program-level methods of the pin's `session.go`: the nine
diagnostics methods, the four emit methods, insertion formatting, batched
requests with pagination, the profiling refusals, and the language-service
queries for completions, referenced symbols and a symbol's references in a
file.

## What landed

- **Diagnostics** (`session/diagnostics.rs`): `getSyntacticDiagnostics`,
  `getBindDiagnostics`, `getSemanticDiagnostics`, `getSuggestionDiagnostics`
  and `getDeclarationDiagnostics` over one `getDiagnostics` with the pin's
  file-list rule (an omitted `files` means every file, `[]` names none);
  `getConfigFileParsingDiagnostics`, `getProgramDiagnostics` and
  `getGlobalDiagnostics` (semantic diagnostics of every file first, then the
  checker's file-less diagnostics). Semantic, suggestion and declaration
  diagnostics acquire the project's diagnostics checker through its
  scheduler. A config-file diagnostic names the config source file, which
  the program holds outside its file list, so it reports `fileName` and
  positions as the pin does. A diagnostic the client sends with
  `createProgram` (the pin's `ToDiagnostic`, an ad hoc message) localizes to
  its own text; `tsr_compiler::diagnostic_writer::localized` learned that
  case.
- **Emit** (`session/emit.rs`): `emit` writes through the session's file
  system, which is the client's callback file system when `writeFile` is
  enabled; `emitToString` collects the outputs sorted by name;
  `getJavaScriptEmit` and `getDeclarationEmit` emit the selected files with
  `forceEmit`, refuse an omitted list and accept an empty one; `emitOnly`
  values outside 0–2 are the client error `invalid emitOnly value: N`.
- **Insertion formatting**: `formatNodeForInsertion` decodes the client's
  base64 AST and formats it through `tsr_api::format_node_for_insertion`
  at the UTF-8 position of the UTF-16 offset. The standalone session uses
  the default format settings; an LSP-hosted session's settings are the
  server's.
- **Batches** (`session/batch.rs`): `batchRequests` decodes items leniently
  (an unknown method is answered inside its item), dispatches each item
  with panic recovery, refuses a nested batch, and pages the encoded
  responses with the pin's arithmetic: the `{"responses":[]}` and
  continuation-token lengths count against `maxResponseBytesPerPage`, a
  single oversized response is sent whole, tokens are `<sessionId>-<n>` and
  are consumed once, and the default page limit is 300,000,000 bytes.
- **Profiling**: `startCPUProfile`, `stopCPUProfile` and `saveHeapProfile`
  answer `method not implemented: <method>` (decision 8).
- **Completions** (`session/service.rs`, `tsr_ls::api_server`):
  `getCompletionsAtPosition` runs the language service's completion worker
  at the UTF-8 position with label details, and with `includeSymbol` the
  symbol behind each symbol-backed entry, registered on the project's API
  checker so its type resolves. Module-export completions read the
  project's auto-import registry, which the service builds on demand; the
  pin's retry through a snapshot cloned with auto-imports has no Rust
  counterpart because the registry lives on the project.
- **References**: `getReferencesToSymbolInFile` through the checker;
  `getReferencedSymbolsForNode` through the reference search with the pin's
  definition-node rule (the group's node, else the symbol's first
  declaration) and node handles for every entry that has a node.
- **Panics**: a failure the pin reaches by panicking, a type-kind mismatch
  such as `getTypeArguments` of an intrinsic type, is the Rust accessor's
  `UnexpectedType` refusal; the session reports it as `panic: …`, which is
  what a batch item carries in the pin. A batch item carries the panic's
  first line only, where the pin's `panic: %v\n` is followed by the Go
  stack.

## Witnesses

`crates/tsr_api/src/session/service_tests.rs` drives the handlers through
the wire dispatch over a memory file system with the bundled libraries:
batch items answered individually with the nested-batch refusal; pages cut
at the byte limit, the oversized single response, the request-scoped limit
and the invalid continuation token; a panicking item alone; semantic,
syntactic and config diagnostics with `emitToString`, `getJavaScriptEmit`,
the `emitOnly` refusal and the profiling refusal; the omitted-versus-empty
file list for diagnostics and selected emit; the config file named by its
diagnostics; the client's `createProgram` diagnostic returned with its
text; completions whose symbols resolve (the `session_completion_test.go`
cases: a configured project, an inferred project, module exports); and
references as node handles. `session/batch.rs` encodes an empty result as
`[]`; `session/responses_tests.rs` ports the two diagnostic-response cases
of `proto_test.go`.

Direct Go tests routed in `tools/phase6/jsapi/inventory.py`: the eight of
`session_batch_test.go`, the three of `session_completion_test.go` and the
two diagnostic-response tests of `proto_test.go`; `session_textedit_test.go`
stays with the import adder.

## The suite against A4

Native: 12 files, 706 cases, no skips, no failures.

| Measure | A3 | A4 |
| --- | ---: | ---: |
| Cases passing | 591 | 685 |
| Cases failing | 115 | 21 |
| Files failing as a whole | 0 | 0 |

Cause labels of the failing cases:

| Label | Cases |
| --- | ---: |
| `server: not implemented: getImportAdderEdits` | 11 |
| `server: not implemented: getConstantValue` | 4 |
| `server: not implemented: getSignatureUsages` | 2 |
| `server: not implemented: getDocumentationComment` | 2 |
| `server: not implemented: getJsDocTags` | 2 |

Every remaining failure is a method that still fails by name: the import
adder (`getImportAdderEdits`, which also carries `getImportEditsForSymbols`
and the generator roster case), `getSignatureUsages`, and the three A3
follow-up methods. The `LanguageService -` completion and reference
families, every `Program -` family, `SnapshotInternalAPI -`, the timing
cases and the generator batching cases pass in both clients.

## Known differences left for later checkpoints

- `getImportAdderEdits` and `getSignatureUsages` fail by name. The import
  adder needs the auto-import view and adder over the registry with
  `toAPITextEdits` in original coordinates; there is no Rust
  signature-usages search yet.
- `getConstantValue`, `getJSDocTags` and `getDocumentationComment` still
  fail by name (the A3 follow-up).
- The standalone session formats insertions with the default settings; the
  pin reads the snapshot's format settings, which only an LSP-hosted
  session has.
- The differences recorded in A3 (`getRestTypeOfSignature`, tuple metadata,
  template-literal `getTypes`) stand.

## Checks

- `cargo test -p tsr_api -p tsr_ls -p tsr_compiler --all-features`, the
  clippy gate and `cargo xtask validate` pass.
- `python3 scripts/parity.py check jsapi` passes against the accepted
  expectation file.

## Review fixes after A5

The A4 review's findings, fixed in the review-fixes branch after A5:

- Diagnostics are filtered, sorted and deduplicated as the pin's program
  getters return them (`filterAndSortDiagnostics`): one file's result on its
  own, the whole program's as one list when `files` is omitted; the explicit
  list concatenates per-file results without a global sort, as the pin does.
- `getGlobalDiagnostics` is the pin's `GetProjectDiagnostics` filtered to
  file-less entries: the config-file parsing diagnostics, the program's own
  and the pool's accumulated globals after a semantic pass over every file,
  sorted and deduplicated.
- Emit runs through the project's checker pool
  (`CheckedProgram::with_pool`) with the request's cancellation, so an emit
  after a check reuses the checked files instead of rechecking on a private
  pool; the selected-files emit looks each file up once.
- `includeSymbol` attaches the symbol per completion item, as the pin's
  `CompletionItem.Symbol` is set per item; an auto-import that repeats a
  global's label carries no symbol.
- A nil slice goes out as `[]` with the pinned JSON module (its
  `FormatNilSliceAsNull` is off): the empty results of the list methods
  (`getReferencedSymbolsForNode`, the type and symbol array properties,
  `getMembersOfSymbol`, `getExportsOfSymbol`, `getPropertiesOfType`,
  `getIndexInfosOfType`, `getExportsOfModule`, `getConfigFileNames`, and
  A5's `getJsDocTags`, `getSignatureUsages` and `toAPITextEdits`' failure)
  are `[]`, not `null`.
- `parity.py accept` refreshes an unapproved reason from the run only in the
  `jsapi` suite, whose labels are mechanical by design (the suite table's
  `refresh_reasons`); every other suite keeps its reasons as worded
  (docs/EVIDENCE-plan.md).
- The insertion-formatting comment says what the code does: the default
  settings for every session.
