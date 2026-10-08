# Phase 6 A3 — checker, symbol, type and signature queries

A3 adds the handle registries of a snapshot and the checker-backed queries
of the pin's `session.go`: symbols by position, location and file; types of
symbols, at locations and positions; signatures; the property fetchers of
symbols, types and signatures; the intrinsic and well-known handles; and the
type-to-syntax conversions.

## What landed

- **Registries** (`session/handles.rs`): symbols snapshot-wide with a dense
  id per symbol identity (the packed arena and slot of the AST symbol, so a
  symbol seen from two projects has one handle) and the project it was first
  observed in; types and signatures per project, each registry bound to the
  project's API checker (`CheckerSlot::Api`) for the snapshot's lifetime.
  Registry reads go through the operation that performs them, which
  validates the owner and the pool generation (docs/design/symbols.md
  section 2.5); a symbol minted by one project's checker is read through
  another project's checker as the same source symbol, and a transient
  symbol is rejected there. Errors carry the pin's texts (`empty type
  handle`, `type handle N not found in project registry`, `symbol handle N
  not found in snapshot registry`).
- **Node handles**: `index.kind.path` over the encoder's node index table of
  the node's file; resolution parses the handle, finds the file in the
  project's program and indexes the table, with the pin's error texts for
  malformed and stale handles.
- **Checker accessors** (`tsr_checker::api_accessors`): production readers
  for the type alias and its arguments, the literal value, the reference,
  index and string-mapping target, the interface type parameters in the
  pin's layout, the tuple shape, and the parts of indexed-access,
  conditional, substitution and template-literal types, plus `ast_view` for
  the node of a query. `LiteralValue` is exported for the response
  constructor.
- **Responses** (`session/checker_responses.rs`): `SymbolResponse` (escaped
  name, flags, check flags, declaration and value-declaration handles,
  parent and export symbol handles), `TypeResponse` (the pin's per-kind
  fields), `SignatureResponse`, `IndexInfoResponse`,
  `TypePredicateResponse`, `literalValueToJSON`. Symbols named by a response
  (parent, export symbol, alias symbol, parameters) are registered as they
  are named, so the client can follow them: the pin relies on its global
  symbol ids for that, the Rust registry has to mint them.
- **Handlers** (`session/checker.rs`): symbols at position(s), location(s),
  of source file(s); types of symbol(s), declared and non-missing;
  `resolveName`, `getSymbolsInScope`; signatures of type, resolved
  signature; types at location(s) and position(s); the symbol, type and
  signature property fetchers; contextual type, base type of literal type,
  non-nullable, type from type node, widened, parameter type, type parameter
  at position; `isArrayLikeType`, `isArrayType`, `isTypeAssignableTo`,
  `isContextSensitive`, `isReadonlySymbol`; shorthand assignment value
  symbol, type of symbol at location; return, rest and predicate of
  signature; base types, properties, apparent properties, apparent and
  reduced types, index infos, constraint and default of type parameter, base
  constraint, property of type, signature from declaration, export specifier
  local target, aliased, immediate aliased and target symbols, fully
  qualified name, exports of module, member in module exports, type
  arguments, true and false types of conditional types; the twelve intrinsic
  getters, well-known symbols and signatures; `typeToTypeNode`,
  `signatureToSignatureDeclaration` (encoded with `tsr_encoder::encode_node`
  from the builder's view, raw or base64), `typeToString`, `printNode`.
- **Left for the A3 follow-up**: `getConstantValue` (the confirmed checker
  gap), `getJSDocTags` and `getDocumentationComment` (language-service
  helpers); the three still fail by name.

## Witnesses

`crates/tsr_api/src/session/checker_tests.rs` drives the handlers through
the wire dispatch over a memory file system: a symbol found by position, its
type and the printed type, the declaration handle resolving to the same type
handle, signatures with their parameters and return type, symbols in scope;
intrinsic and well-known handles stable within a snapshot; a type handle of
one project rejected by another project's registry and the pin's texts for
empty, unknown and malformed handles.

## The suite against A3

Native: 12 files, 706 cases, no skips, no failures.

| Measure | A2 | A3 |
| --- | ---: | ---: |
| Cases passing | 287 | 591 |
| Cases failing | 419 | 115 |
| Files failing as a whole | 1 | 0 |

Cause labels of the failing cases:

| Label | Cases |
| --- | ---: |
| `server: not implemented: getSemanticDiagnostics` | 16 |
| `server: not implemented: batchRequests` | 13 |
| `server: not implemented: emit` | 13 |
| `server: not implemented: getImportAdderEdits` | 10 |
| `server: not implemented: getCompletionsAtPosition` | 8 |
| `server: not implemented: getSyntacticDiagnostics` | 8 |
| `server: not implemented: getJavaScriptEmit` | 6 |
| `server: not implemented: formatNodeForInsertion` | 6 |
| `server: not implemented: getConfigFileParsingDiagnostics` | 4 |
| `server: not implemented: getConstantValue` | 4 |
| `server: not implemented: getBindDiagnostics` | 4 |
| `server: not implemented: getGlobalDiagnostics` | 4 |
| `server: not implemented: getReferencedSymbolsForNode` | 2 |
| `server: not implemented: getSignatureUsages` | 2 |
| `server: not implemented: getDocumentationComment` | 2 |
| `server: not implemented: getJsDocTags` | 2 |
| `server: not implemented: getDeclarationEmit` | 2 |
| `server: not implemented: getSuggestionDiagnostics` | 2 |
| `server: not implemented: getDeclarationDiagnostics` | 2 |
| `server: not implemented: getProgramDiagnostics` | 2 |
| `server: not implemented: emitToString` | 2 |
| `server: unknown method: unknown` | 1 |

Every remaining failure is an A4 method (diagnostics, emit, batches,
completions, references, the import adder, insertion formatting) or one of
the three A3 follow-up methods. `test/async/api.test.ts` now completes
within the per-file deadline: the pending promise recorded in A1 and A2 was
the client waiting on a session method that failed, and the checker queries
answer it.

## Exit families deferred to A4

`API > createProgram…` (12 cases) and `runWithTemporaryFileUpdate` (2) are
A2 exit families that still failed at A3: they need A4's diagnostics and
pass with them. The 115 failing entries of `status/parity/jsapi.json` at
the A3 head carried the first run's labels (`not implemented: initialize`);
A4 made `accept` refresh mechanical labels from the run.

## Review fixes after A4

- Checker handlers publish under the retirement-serialized protocol of
  docs/design/ownership.md section 2.7: registry insertion and lookup take
  the project pool's generation gate, and a response is serialized outside
  the gate, then committed to the connection only after the gate revalidates
  the checker and generation. A generation retired by a sibling between a
  query's computation and its commitment yields the error form, not handles
  of a retired checker.
- `register_type` and `register_signature` refuse an operation of another
  checker (`duplicate type`, `duplicate signature`, the pin's panics).
- `base64_decode` follows Go's `StdEncoding`: CR and LF are skipped,
  padding may only end the input, and the error names the offending byte
  (`illegal base64 data at input byte N`).
- A malformed node handle index reports `strconv.ParseUint`'s text and
  bounds the index to 32 bits.
- `getRestTypeOfSignature` slices a tuple rest parameter first (see the
  note below).
- Identity witnesses: one symbol id across two projects with the first
  project canonical; a released snapshot's handles rejected with the pin's
  text; a retired generation's handles rejected and a fresh snapshot
  answering.

## Known differences left for later checkpoints

- `getRestTypeOfSignature` answers the element type, as the pin does
  (`tryGetRestTypeOfSignature`): a tuple rest parameter is first sliced to
  its rest element and the numeric index type of that element is read, so
  `(...args: [string, number])` and `(...args: [string, ...number[]])`
  both answer `any`. (The A3 head read the numeric index type of the whole
  tuple; the review fixes corrected it.)
- Tuple metadata (`elementFlags`, `fixedLength`, `readonly`, labeled
  declarations) is reported for tuple targets only, as the pin's
  `IsTupleTypeTarget` requires; instantiations report `isTupleType` alone.
- `getTypes` of a template literal type returns its type parts (the pin's
  `Type.Types`); the checker's union/intersection `constituents` did not
  cover template literals.
- `getConstantValue`, `getJSDocTags` and `getDocumentationComment` still
  fail by name (the A3 follow-up).

## Checks

- `cargo test -p tsr_api -p tsr_checker -p tsr_project -p tsr_lsp -p tsrust`,
  the clippy gate and `cargo xtask validate` pass.
- `python3 scripts/parity.py check jsapi` passes against the accepted
  expectation file.
