# Phase 3: emit

Status: **accepted by the owner, 2026-10-01** (the decisions of section 8).
The detailed implementation plan for [PLAN Phase 3](../PLAN.md#phase-3-emit).
Its checkpoints carry their own work items, witnesses and exit checks;
production implementation starts at T0.

Planning reference: branch `phase2-c7` at `8a456baf` (the C7 exit is being
recorded on it; [PHASE2-C7.md](PHASE2-C7.md) is its record). Upstream remains
Corsa `1f70213d4922b434345f639b441681e470c7cfc1`.

## 1. Outcome and starting point

Deliver the pinned emit: JavaScript, declaration files, source maps and the
source-map record for every executed compiler and conformance variant, and
the transpile API, all byte-identical to the pinned oracle. This is PLAN's
Phase 3 gate: "100 percent of `.js`, `.map`, `.d.ts` and source-map-record
baselines and the 41 transpile baselines."

The starting point is the recorded Phase 2 exit plus the emit slices earlier
sprints built for their own needs. Nothing here is a new architecture; the
data layer (transform arenas, the emit context's side tables, retained
results) is the one [ADR 0006](adr/0006-node-ownership--arenas--lazy-file-storage--bundles-and-check.md)
fixed and S08/S09 exercised.

### What exists

| Asset | Where | State |
| --- | --- | --- |
| The printer | `crates/tsr_printer` (10,397 lines): `printer.rs`, `printer_statements.rs`, `printer_expressions.rs`, `literal_text.rs`, the four writers, `emit_context.rs`, `emit_flags.rs`, `list_format.rs`, `type_precedence.rs` | `printer.go` 282 of 378 functions marked. Every type-display path and the statement and expression emitters exist; the 96 unmarked functions are comment emission (37), source-map positions (7), name generation and generated identifiers (11), 25 node emitters (`EmitSourceFile`, `emitSourceFile`, the shebang, directives and helpers, accessors, binding elements, specifiers, clauses, enum members, JSDoc) and 16 others (writer kinds, literal and property writers, snippets and tab stops, indirect calls, ASI parenthesization). `Printer::write` refuses comment emission and `PreserveSourceNewlines` with a source file; it produces no source maps. `emitcontext.go` 21 of 87 marked (the original-node table, emit flags, comment ranges and synthetic comments exist; the lexical and variable environments, hoisting, prologue splitting, `ParseNode`, source-map ranges and `IsFileLevelUniqueName` do not). `factory.go` 3 of 91, `namegenerator.go` 0 of 25, `helpers.go` 0 of 1 function and none of its 27 helper texts |
| Native printer observations | `data/s08/printer-cases.json`, `data/s08/printer-observations.json`, `tools/s08/oracle/printer_test.go`; E4's `run.e4.helper_semantics` | The pinned printer's bytes for synthetic type-display trees and the printer constants; the escaping helpers (`printer/utilities.go`, 51 functions) are mapped through `tsr_jsstring::escape` and `literal_text.rs` |
| The emit resolver | `tsr_printer::emit_resolver` (`DeclarationEmitResolver`, `DeclarationSymbolTracker`, `SymbolAccessibilityResult`, `TypeReferenceSerializationKind`), `tsr_checker::emit_resolver_js` and the resolver's public methods (C5.6) | Both halves of `emitresolver.go` are Phase 2's and complete: the declaration half since S08, the JavaScript half (alias elision queries, `MarkLinkedReferencesRecursively`, `GetConstantValue`, the JSX factory entities, `GetTypeReferenceSerializationKind`, the reference queries) since C5 |
| The declaration transform | `crates/tsr_transformers/src/declarations` (5,007 lines, 64 of 180 `declarations/*.go` functions marked) and `tsr_compiler::declaration_diagnostics` | `transform_declarations` builds the transformed `.d.ts` root in a caller-owned `AstBuilder` with its `EmitContext`, and its diagnostics feed `Program.GetDeclarationDiagnostics` (1,754 native declaration requests match). The root is never printed; the tracker's visibility and accessibility reports are the pin's |
| The pseudochecker | `crates/tsr_pseudochecker` (1,442 lines) | 22 of 72 functions marked; `lookup.go` half ported, `type.go`'s pseudo types (35 functions) not |
| Output paths | `tsr_tsoptions::output_paths`, `tsr_compiler::output_paths` | 9 of 22 `outputpaths` functions marked, driven by option verification: the common source directory, the declaration extension, the build-info name, `Program::javascript_emit_files` (C5.7) and the blocked output paths. The per-file `OutputPaths` (`JsFilePath`, `SourceMapFilePath`, `DeclarationFilePath`, `DeclarationMapPath`), `GetOutputPathsFor` and `ForEachEmittedFile` are not ported |
| The emit schedule | `tools/s08/p5/emit.rs`, `scripts/phase2_corpus.py` (`EMIT_TRANSFORMS`, `validate_emit`, `pre_emit_view`) | C5.7's model of the post-emit program: the resolver calls of import elision, the runtime-syntax enum values and constant-enum inlining, in pre-order, with the class-fields downleveler's private-name replacement modeled (C5). It transforms and writes nothing and says so (`emit: not_executed`). Phase 3 replaces the model with the emit itself |
| Program driving | `tsr_compiler::CheckedProgram` with `CheckerRequest` (C6, C7), `CompilerCheckerPool`, `tsr_core::workgroup::WorkGroup`, the `TraceSink` seam, `tsr_core::CancellationToken` | The pin's per-file checker hand-out for the emit host (`newEmitHost` takes `GetTypeCheckerForFile`), the parallel or single-threaded group emit runs in, and the trace phases emit pushes. The bounded work group of the #73 review is a dependency (section 3) |
| Content mappers | `tsr_contentmapper`, `tsr_ast::span_map`, `Program::load_with_content_mapper_project` (C7.8) | Mapped files load and check; their declaration-map positions (`MapSourcePosition` over the span map) and the supplemental-references transformer are Phase 3's (C7's handoff) |
| Node visitor and factory | `tsr_ast::NodeVisitor` and `NodeVisitorHooks` (the pin's `ast/visitor.go`), `tsr_ast::AstBuilder` with the generated constructors, `tsr_ast::precedence` (the pin's `ast/precedence.go` parenthesizer rules) | The transform framework's mechanics; used by the declaration transform today |
| The binder's reference resolver | `tsr_binder::reference_resolver` | The pin's `binder.ReferenceResolver`, which the script transforms take when the checker's resolver is not needed |
| The harness | `tools/s08/p4/executor.rs` (`observe_pooled`), `tools/s08/p5`, `scripts/phase2_corpus.py`, `scripts/phase2_compare.py`, `scripts/phase2_native.py`, `tools/s08/oracle/*.go` | The S08 P5 protocol: one process per variant, immutable captures, replay, the categorized comparison over seven domains, the native oracle as an access-only overlay over the pinned runner, both test-program modes |
| The pinned unit tests | `internal/printer/printer_test.go` (2,624 lines: `TestEmit` with 564 rows and 65 parenthesization tests), `namegenerator_test.go` (36 tests), `utilities_test.go` (4), `internal/sourcemap/generator_test.go` (32 tests), `internal/transformers/tstransforms/importelision_test.go` and `typeeraser_test.go` (91 rows), `internal/outputpaths` (2), `internal/transpile` (1) | Direct witnesses to port with their crates. The ES, module, JSX, inliner and declaration transforms have no Go unit tests; the corpus is their evidence |
| The API print scratch | `tsr_api::printing` (S09-3) | A request-scoped printer use over decoded syntax; not an emit path |

### Gaps confirmed by source inspection

- No script transform exists: `transformers/*.go` (59 functions), `tstransforms`
  (141), `moduletransforms` (124), `estransforms` (407), `jsxtransforms` (48)
  and `inliners` (3) have no Rust function.
- No source-map code exists: `sourcemap/*.go` (50 functions: the generator,
  the decoder, the document position mapper, line info).
- No emit orchestration exists: `compiler/emitter.go` (20 operations),
  `compiler/emitHost.go` (28) and the `Program.Emit` family in `program.go`
  (`Emit`, `HandleNoEmitOptions`, `CombineEmitResults`, `IsEmitBlocked`,
  `GetSourceFileFromReference`, `CommandLine`, `getSourceFilesToEmit`).
  The Phase 1 destination audit assigned these 57 operations to Phase 3
  ([PHASE1-F5b-destinations.md](PHASE1-F5b-destinations.md)); C7's audit
  handed six more (`emitDeclarationFile`, `getDeclarationTransformers`,
  `GetSourceFileFromReference` and the three `SupplementalReferencesTransformer`
  functions).
- The compiler host has no write seam. `tsr_vfs::FileSystem::write_file` exists
  on the file systems; nothing emits through it, and the harness has no output
  recorder (`harnessutil.OutputRecorderFS`).
- The printer cannot print a source file with comments, cannot map positions,
  generates no names and has no helper table.
- `transpile` has no Rust function; the barebones lib text and `transpileFS`
  are not ported.

By the numbers: the Phase 3 ledger files hold 71 files (61 `planned`, 10
`in-progress`), 38,906 lines and 1,882 functions, 518 of them marked (28%);
the two compiler files that move here (section 2) add 719 lines and 50
functions, 3 of them marked (`getSourceFilesToEmit`, `sourceFileMayBeEmitted`
and `getDeclarationDiagnostics`); the destination audit counted 48 of them
as Phase 3 operations. The pin's printer and TypeScript-transform tests are
3,797 lines; the source-map generator has 32 more tests.

## 2. Scope and phase boundaries

| Area | Phase 3 responsibility | Boundary |
| --- | --- | --- |
| Printer | `printer.go` complete: comments, source-map positions, name generation, helpers, directives, `EmitSourceFile` and `Write`; `emitcontext.go`, `factory.go`, `namegenerator.go`, `helpers.go`, `emithost.go`, `emitresolver.go`, `generatedidentifierflags.go`, `syntheticfile.go` | Type display keeps its Phase 2 witnesses; the LS's change tracker and API printing use the same printer (Phases 5 and 6 consume it) |
| Source maps | `sourcemap/*.go`: the generator, the decoder, the document position mapper, line info | The LS's mapped-position queries over `.d.ts.map` are Phase 5's callers of the same mapper |
| Transforms | `transformers/*.go`, `tstransforms`, `moduletransforms`, `estransforms`, `jsxtransforms`, `inliners`, with their transform arenas and original-node side tables | Checker behavior enters only through the emit resolver and the reference resolver; a missing resolver query is a named joint blocker owned by Phase 2 maintenance, never a Phase 3 reimplementation |
| Declaration emit | `transformers/declarations/*.go` complete, `pseudochecker` complete, `isolatedDeclarations`, `stripInternal`, `declarationMap`, the supplemental-references transformer, content-mapped declaration positions | The diagnostics half stays as Phase 2 recorded it and must not change its results |
| Output paths and emit orchestration | `outputpaths/*.go`; `compiler/emitter.go` and `compiler/emitHost.go`, which move to Phase 3 in the ledger (decision 1); the `Program.Emit` family of `program.go`, ported as marked operations while `program.go` stays Phase 4's file | The CLI's emit gating (`execute/tsc/emit.go`), build-info emit, `EmitOnlyBuilderSignature`'s consumer (`incremental`) and `--listEmittedFiles` are Phase 4's; Phase 3 implements the `EmitOnly` and `ForceEmit` modes because the transpile API and the harness use them |
| Transpile | `transpile/*.go`: `TranspileModule`, `TranspileDeclaration`, the barebones lib, `transpileFS` | The API method that exposes them is Phase 6's |
| Acceptance | The runner's `output`, `sourcemap` and `sourcemap record` sub-tests, the JS baseline's declaration re-compilation and `noCheck` repeat, and the transpile runner | Phase 2's seven domains keep their recorded gate; Phase 3's changes to shared crates re-record it at the exit (section 5) |
| Entry points | `emit` on `tsr_embed::Session` and the WebAssembly checker entry with an in-memory write callback (decision 9) | Full WebAssembly and Rust-consumer emit acceptance is Phase 7's |

Three boundaries need explicit settling:

**The emitter files.** `compiler/emitter.go` and `compiler/emitHost.go` are
Phase 4 files in `PORTS.toml`, but every one of their operations was assigned
to Phase 3 by the destination audit the owner approved on 2026-09-25, and the
Phase 3 gate cannot close without them. As C6.1 moved `checkerpool.go`, T0
moves both to Phase 3 (decision 1). `data/upstream.json` changes with the
ledger, which stales every recorded run that binds it; C7.6's green-up
absorbs that cost only if the move lands with C7's, so T0 coordinates with
the C7 exit (section 3).

**Program-level emit.** `Program.Emit`, `HandleNoEmitOptions`,
`CombineEmitResults`, `IsEmitBlocked`, `getSourceFilesToEmit`,
`GetSourceFileFromReference` and `CommandLine` live in `program.go`, a Phase
4 file that C5.7 and C6 already port operations from (`javascript_emit_files`,
`CheckedProgram`). Phase 3 ports them as marked operations of `tsr_compiler`
(a `CheckedProgram::emit`), and `program.go` stays Phase 4's ledger entry.

**The runner's own limits.** The pinned `output` sub-test skips eight tests by
name for nondeterministic parallel output (`skippedEmitTests`) and does not
run for the 61 variants whose input files are all declaration files (counted
from the inventory's root files; T0 confirms the count from the capture).
Those rows are
`disabled` for `output`, with the pin's reason recorded, and stay
informational (decision 6); the `sourcemap` sub-tests still run on them.

## 3. Prerequisites and coordination

Phase 3 consumes these Phase 1 and 2 contracts as they are, through their
public entry points. A missing operation among them is a named joint blocker
with an owner, not a Phase 3 patch:

| Contract | Owner | Phase 3 use |
| --- | --- | --- |
| The emit resolver's two halves, and the resolver's public operations | Phase 2 (C5) | Every resolver call of the script and declaration transforms |
| `CheckedProgram` with `CheckerRequest`, the compiler pool's per-file checker, `ForEachCheckerParallel` | Phase 2 (C6, C7) | `newEmitHost` takes the file's checker; global diagnostics for `noEmitOnError` |
| `WorkGroup` | Phase 2 (C6), amended by the #73 review: a parallel group runs on bounded workers, not one thread per task | `Program.Emit` queues one task per emitted file; a whole-program emit on a large program must not start thousands of threads. **Dependency:** the bounded group lands before T8's parallel emit is measured |
| `TraceSink` | Phase 2 (C6) | The emit phase's `Push` sites (`emit`, `emitJsFileOrBundle`, `emitDeclarationFileOrBundle`, `transformNodes`) |
| `CancellationToken` | Phase 2 (C6) | `Program.Emit` returns early when the context is done after `HandleNoEmitOptions`; the transpile API returns `nil` when canceled |
| The content-mapper host, span maps and `Program::load_with_content_mapper_project` | Phase 2 (C7.8) | Content-mapped files' `MapSourcePosition`, `OriginalText`/`OriginalFileName`, `ContentMapperExtensions`, and which mapped files may emit |
| `javascript_emit_files`, the blocked output paths, option verification's common source directory | Phase 2 (C5.7), Phase 1 | `getSourceFilesToEmit` and `IsEmitBlocked` |
| The declaration diagnostics path | Phase 2 (C5) | Unchanged results; T7 prints the transformed root the same transform produces |
| `NodeVisitor`, `AstBuilder`, the generated factory, `ast/precedence.go`'s parenthesizer rules, `SetTextRange`/original-node metadata | Phase 1 | The transform framework |
| `tsr_binder::reference_resolver` | Phase 1 | The script transforms' resolver when the checker's is not required |
| `tsr_jsstring` escapes, `tsr_jsnum` number printing, `tsr_tspath`, `tsr_vfs::FileSystem::write_file` | Phase 1 | Literal text, source-map paths and URLs, `WriteFile` |
| The S08 P5 protocol, the phase2 comparison and native-capture libraries | Phase 2 (C0) | Reused as libraries by the Phase 3 scripts, never edited (section 5) |

Coordination:

- **The shared checkout is C7's.** Phase 3 develops on its own branch in its
  own worktree until C7 is recorded and merged. T0's ledger move is
  coordinated with the C7 exit: either it lands in C7.6's green-up, or T0
  re-records what it stales.
- **Staleness is expected mid-phase.** Every Phase 3 change under `crates/**`
  stales the recorded `checker` run, and every change to a recorded input
  stales the captures that bind it. Phase 3 does not re-record the Phase 2
  gate per fix; T8's green-up re-records `checker` and `emit` together
  (decision 12). CI red for that reason is listed, not ranked.
- **Recorded inputs are not edited.** `scripts/phase2_native.py`,
  `scripts/phase2_inventory.py` and `scripts/phase2_corpus.py` are inputs of
  the recorded Phase 2 captures; C7 learned that an edit stales both native
  captures. Phase 3 adds `scripts/phase3_*.py` beside them and imports their
  modules.
- **The dependency rule holds** ([PLAN section 9](../PLAN.md#9-sequence-and-gates)):
  no blanket Phase 2 flag; each checkpoint names the contracts it consumes.

## 4. Delivery order

Checkpoints are **T0 to T8** (E is taken by the experiments E1 to E8). Each
pairs its witnesses with its production work and verifies the combined
result, as Phase 2's did; T0 is preparation only. The order follows the
dependency graph, not the line count: the printer and the source-map generator
sit under every transform's witness, so they go first.

```text
T0 ─ T1 ─┬─ T2 ─────────────────┐
         │                      ├─ T7 ─┐
         └─ T3 ─ T4 ─ T5 ─ T6 ──┤      ├─ T8
                                └──────┘
```

T7 (declaration emit) needs T1 and T2 and can run beside T3 to T6. T4 to T6
each need T3's framework; their order follows the transform chain the
emitter runs (TypeScript, JSX, downlevel, module, inliner), so each
checkpoint's witness rows are the variants whose emit needs no later
transform.

### T0 — the emit acceptance contract

Preparation only; no Rust emit parity is claimed. It joins the existing
inventory and harness and extends them; it re-derives nothing.

- **Exists:** `data/phase2/inventory.json` (13,432 executed variants with
  their reference baselines by kind); `scripts/phase2_native.py`'s generated
  driver and the S08 overlays; the S08 P5 protocol and executor; the phase2
  comparison, blockers and producer libraries; the `[checker]` producer's
  wiring pattern.
- **Build:**
  - *The denominator.* `data/phase3/inventory.json`: the 13,432 executed
    variants with their emit obligations: `.js` reference (12,174 rows),
    `.js.map` (150), `.sourcemap.txt` (157), whether the `output` sub-test
    runs (no for the 61 declaration-only-root variants and the 8
    `skippedEmitTests` variants, each with the pin's reason), the options
    that select transforms (`target`, `module`, `jsx`, `declaration`,
    `emitDeclarationOnly`, `isolatedDeclarations`, `sourceMap`,
    `inlineSourceMap`, `declarationMap`, `noEmit`, `noEmitOnError`,
    `importHelpers`, `noEmitHelpers`, `removeComments`, `newLine`, `emitBOM`,
    `stripInternal`, `outDir`, `declarationDir`, `sourceRoot`, `mapRoot`,
    `inlineSources`). The 1,258 executed rows without a `.js` reference are
    graded too: 1,155 `noEmit`, 61 declaration-only roots and 42 others whose
    baseline is `<no content>`; a Rust emit that writes a file there is a
    difference. Plus the transpile inventory: the 25 test files under
    `testdata/tests/cases/transpile`, expanded by the runner's vary-by
    (`sourcemap`, `inlinesourcemap`, `declarationmap`) into the configurations
    behind the 41 baselines, each with its `TranspileModule` and
    `TranspileDeclaration` runs.
  - *The native capture.* `scripts/phase3_native.py capture|verify|review`
    with its own generated driver `testrunner/phase3_emit_test.go`, over the
    same requests and sharding as C0, in both test-program modes. Per
    variant it records: every emitted output by name and text (`result.JS`,
    `DTS`, `Maps`, in the harness's input order), `EmittedFiles`,
    `EmitSkipped`, the emit diagnostics, the source-map record
    (`GetSourceMapRecord`), the rendered `.js`, `.js.map` and `.sourcemap.txt`
    baselines exactly as `DoJSEmitBaseline`, `DoSourcemapBaseline` and
    `DoSourcemapRecordBaseline` compose them (headers, `<no content>`, the
    preview link, the `noCheck` repeat's diff, the `DtsFileErrors` of the
    declaration re-compilation), and the pre/post-emit diagnostic-count
    mismatch diagnostic. Per transpile configuration: the output text, the
    source-map text and the reported diagnostics of both runs. `review`
    checks the rendered baselines against the committed reference files;
    `verify` reproduces the row digests from a second sharding.
  - *The Rust harness.* Requests gain `emit: true` beside Phase 2's
    `emit_schedule`; a request with `emit` runs `CheckedProgram::emit` on the
    post-emit program with an in-memory write recorder (the pin's
    `OutputRecorderFS`), then renders the three baselines with ported
    writers in harness code (as S08 P5 ported the error writer) and records
    the per-file outputs and digests. Until T8 the emit operation reports
    `unsupported`; the row is categorized, never blank.
  - *The comparison.* `scripts/phase3_compare.py report|modes` adds the
    domains `output`, `sourcemap`, `sourcemap_record`, `emit_diagnostics`
    (the post-emit program's emit diagnostics and the count-mismatch
    diagnostic) and `transpile`, over the Phase 2 outcome vocabulary
    (`match`, `different`, `failed`, `unsupported`, `disabled`,
    `unexecuted`). `disabled` carries the pin's reason. Buckets attribute a
    difference to a transform, the printer, the source-map generator or the
    declaration transform from the first differing output file and the
    options that select transforms.
  - *The first run and the blockers.* One complete Rust run in the single
    mode (every row `unsupported` for emit today), `data/phase3/blockers.json`
    with the cross-phase entries (the bounded work group; any resolver query
    the transforms need that the resolver lacks), the register rebuilt from
    evidence.
  - *Wiring.* Sprints `P3A` and `P3B` (section 5), the `emit` producer in
    `status/runs.toml`, `data/phase3/` for the authorities,
    `scripts/tests/test_phase3*.py`, the T0 record.
  - *The ledger move.* `compiler/emitter.go` and `compiler/emitHost.go` to
    Phase 3 (decision 1), with the S07 re-freeze and the trace re-freeze the
    provenance change requires, coordinated with C7.6.
  - *Cost.* Measure one native capture and one Rust run and set the run
    policy: the 300-variant sample of `data/phase2/inventory.json` for
    intermediate work, full runs at checkpoint exits.
- **Exit:** `run.emit.inventory_frozen`, `native_verified`,
  `native_verified_concurrent`, `harness_valid`, `result_recorded` and
  `blockers_named` true on a recorded `emit` run; every row in exactly one
  category; `P3A-T0` passes.

### T1 — the printer

- **Exists:** the type-display and statement/expression emitters, the
  writers, the emit context's original-node table and flags, the escaping
  helpers, `tsr_ast::precedence`.
- **Build:**
  - Comment emission: `emitCommentsBeforeNode`/`AfterNode`,
    `emitCommentsBeforeToken`/`AfterToken`, leading, trailing, detached and
    nested comments, `shouldEmitComments` and `shouldWriteComment`,
    `writeCommentRange`, synthetic comments, `hasCommentsAtPosition`,
    JSDoc-style emission (`OnlyPrintJSDocStyle`), `emitShebangIfNeeded`,
    triple-slash directives (`emitTripleSlashDirectives`, `emitDirective`),
    `writeLines`, `writeLineSeparatorsAndIndentBefore`/`After`,
    `getEffectiveLines`, `PreserveSourceNewlines` with a source file.
  - `EmitSourceFile`, `setSourceFile`, `Write` with a writer and an optional
    source-map generator, `emitHelpers` with the helper table of `helpers.go`
    (27 helpers, their texts byte for byte, `compareEmitHelpers` order,
    `getUniqueHelperName`), `emitSnippetNode` and `emitTabStop` (the LS's
    snippets; ported so the printer is complete, witnessed by the pin's
    tests), the accessor and binding-element node emitters and the other 25
    unmarked emitters.
  - The name generator (`namegenerator.go`, 25 functions: scopes, temp and
    loop variables, unique and generated names for nodes, private names,
    reserved names, `MakeFileLevelOptimisticUniqueName`), the generated
    identifier flags, `PrintHandlers.HasGlobalName`, and the factory's name
    constructors (`NewTempVariable`, `NewLoopVariable`, `NewUniqueName`,
    `NewGeneratedNameForNode`, the private-name variants).
  - Source-map positions in the printer (`emitSourcePos`, token source maps,
    `OmitBraceSourceMapPositions`, `setSourceMapSource`, JSON sources
    skipped), landing with T2's generator; T1 leaves the calls in place
    against the T2 interface.
  - The remaining parenthesizer paths (`parenthesizeExpressionForNoAsi`,
    `shouldEmitIndirectCall`) and `emitcontext.go`'s `ParseNode`,
    `IsFileLevelUniqueName`, source-map and token ranges, `AssignedName`,
    `TextSource`, `ClassThis`.
- **Witnesses:**
  - The pinned printer tests ported: `TestEmit`'s 564 rows, the 65
    parenthesization tests, `namegenerator_test.go`'s 36 tests,
    `utilities_test.go`'s 4.
  - **The reprint witness** (decision 7): the pinned `printer.EmitSourceFile`
    over every source file of every executed variant, with comments and with
    `RemoveComments`, captured natively once (`data/phase3/reprint.json`,
    small: digests per file, texts in the capture directory) and compared
    with the Rust printer over the same parsed files. It exercises every
    emitter over the corpus's syntax with no transform between, and it is
    the printer's regression witness for every later checkpoint.
  - A deep-input stress fixture through the printer alone: a 100,000-term
    binary chain, deeply nested parentheses, deeply nested JSX
    ([ADR 0011](adr/0011-deep-recursion--reserved-stacks-and-growth-guards.md)).
- **Exit:** `run.emit.reprint_parity == 1`; the ported printer tests pass;
  the stress fixture completes on the harness's stack; `printer.go`,
  `namegenerator.go`, `helpers.go`, `emitcontext.go` (its printer half) and
  `factory.go`'s name constructors read `mapped` in the T1 audit group.

### T2 — source maps

- **Exists:** nothing in Rust; the printer's emitters from T1.
- **Build:** `tsr_sourcemap` (decision 2): the generator (`NewGenerator`,
  `AddSource`, `SetSourceContent`, `AddName`, the three `Add*Mapping`
  functions with their backtracking and validation rules, the pending
  mapping commit, base64 VLQ, `RawSourceMap`, `String`, `Base64DataURL`);
  the decoder (`DecodeMappings` and the mapping iterator the record baseline
  walks); `lineinfo.go` and `ECMALineInfo`; the document position mapper
  (`createDocumentPositionMapper`, used by Phase 5 but part of the package's
  parity and cheap now); `source.go` and `util.go`. In the emitter:
  `getSourceRoot`, `getSourceMapDirectory`, `getSourceMappingURL`,
  `shouldEmitSourceMaps`, the `sourceMappingURL` comment, the `.map` write,
  `inlineSourceMap`, `inlineSources`, `sourceRoot` and `mapRoot`. Positions
  in mappings are UTF-16 columns; the writers' line and column tracking
  follows the pin's `EmitTextWriter`.
- **Witnesses:** `generator_test.go`'s 32 tests ported; the 150 `.js.map` and
  157 `.sourcemap.txt` baselines through the harness once T8 emits (T2's
  exit is the direct tests plus a printer-level fixture set: the same source
  file printed with a generator on both sides, over the reprint capture's
  files where `sourceMap` would apply); the preview link and the record
  writer in harness code compared with the native rendering.
- **Exit:** the ported tests pass; the printer-with-generator fixture set
  matches; the `sourcemap` files read `mapped`.

### T3 — the transform framework and the TypeScript transforms

- **Exists:** `NodeVisitor` and hooks, `AstBuilder`, the declaration
  transform's use of both, `tsr_binder::reference_resolver`, the checker's
  resolver operations, the emit context's original-node table.
- **Build:**
  - The framework: `Transformer`, `Chain`, `TransformOptions` (context,
    options, reference resolver, emit resolver, `GetEmitModuleFormatOfFile`),
    the emit context's lexical and variable environments (`Start`/`End`,
    hoisted declarations and functions, `MergeEnvironment`, custom prologue
    detection), `factory.go`'s helpers (`InlineExpressions`,
    `CreateExpressionFromEntityName`, `RestoreEnclosingLabel`,
    `CreateForOfBindingStatement`, `NewTypeCheck`, the method-call helpers,
    outer-expression restoration, `EnsureUseStrict`, the prologue splitters,
    `NewStringLiteralFromNode`, the boolean, `void 0`, comma, assignment and
    logical constructors), `transformers/utilities.go`, `destructuring.go`
    (`FlattenDestructuring*`), `modifiervisitor.go`.
  - The transform arena: one `AstBuilder` per emitted file for the script
    chain, released after printing; the original-node side table in the
    emit context; source nodes referenced, never copied, until a transform
    replaces them (ADR 0006). The same contract the declaration transform
    already follows.
  - `tstransforms`: `typeeraser.go`, `importelision.go` (over the resolver's
    `IsReferencedAliasDeclaration`/`IsValueAliasDeclaration`),
    `runtimesyntax.go` (enums, namespaces, parameter properties),
    `legacydecorators.go`, `metadata.go` with `typeserializer.go` (over
    `GetTypeReferenceSerializationKind`), `utilities.go`; the resolver choice
    of `getScriptTransformers` (the emit resolver when import elision, JSX,
    non-isolated modules or decorator metadata need it, else the binder's).
- **Witnesses:** `importelision_test.go` and `typeeraser_test.go` (91 rows)
  ported; the corpus rows whose emit runs only the TypeScript transforms, the
  use-strict transform and the implied-module transform (target `esnext`
  with an ES module kind) through T8's driver as they come online; a
  contract that a transform's arena and side tables are released with the
  file's emit and that the source arena is unchanged afterwards (S09-2's
  `builder_cache_retention` shape).
- **Exit:** the ported tests pass; the framework and `tstransforms` files read
  `mapped`; the retention contract passes.

### T4 — module and inliner transforms

- **Build:** `moduletransforms`: `externalmoduleinfo.go` (the export
  bindings, the `import`/`export` collection), `commonjsmodule.go` (2,162
  lines: `require` calls, `exports` assignments, the `__importDefault`,
  `__importStar`, `__createBinding` and `__exportStar` helpers,
  `esModuleInterop`, dynamic `import()` lowering, the top-level
  `this`/`exports` handling, `rewriteRelativeImportExtensions`),
  `esmodule.go` (`--module
  preserve` and the ESM shapes), `impliedmodule.go` (per-file module format
  through `GetEmitModuleFormatOfFile`), `utilities.go`; `getModuleTransformer`;
  `inliners/constenum.go` over the resolver's `GetConstantValue`.
- **Witnesses:** the 2,673 variants with an explicit `module` (commonjs
  1,654, esnext 346, nodenext 175, node20 104, node16 103, node18 101,
  es2015 75, preserve 62, es2020 29, es2022 24) and the 10,759 with the
  default, whose module kind follows the target; the 329 target-unset rows
  take the CommonJS transformer. A direct fixture set for the CommonJS
  shapes the corpus reaches rarely (`export =`, `import =`, re-exports with
  `esModuleInterop` on and off, dynamic import under each module kind).
- **Exit:** the module and inliner files read `mapped`; the T3 and T4 witness
  rows match in `output` once T8's driver emits them.

### T5 — the ES downlevel transforms

- **Build:** `estransforms`: `classfields.go` (3,618 lines: fields, private
  names and the `__classPrivateField*` helpers, static blocks,
  auto-accessors, `classthis.go`), `esdecorator.go` (2,751 lines, with the
  `__esDecorate`, `__runInitializers`, `__propKey` and `__setFunctionName`
  helpers), `async.go` (the `__awaiter` and the two `async-super` helpers)
  and `forawait.go` (`__await`, `__asyncGenerator`, `__asyncDelegator`,
  `__asyncValues`), `using.go` (`__addDisposableResource`,
  `__disposeResources`),
  `objectrestspread.go` (`__rest`, `__assign`), `optionalchain.go`,
  `nullishcoalescing.go`, `logicalassignment.go`, `exponentiation.go`,
  `optionalcatch.go`, `taggedtemplate.go` (`__makeTemplateObject`),
  `namedevaluation.go`, `usestrict.go`, `utilities.go`; `GetESTransformer`'s
  chains by target, with `definitions.go`'s order (ESNext: decorators and
  class fields; ES2021 to ES2025: plus `using`; ES2020: plus logical
  assignment; and so on down to the ES2016 chain, which is also the default
  for every older target). The emit helpers' texts (T1) and `importHelpers`
  (`tslib` imports through the module transformer), `noEmitHelpers`.
- **Witnesses:** the target distribution of the executed rows (es2015 11,446,
  esnext 760, es2022 573, unset 329, es2017 124, es2020 102, es2018 42,
  es2021 22, es2019 16, es2016 9, es2023 4, es2024 3, es2025 2) and the 490
  decorator rows; direct fixture sets for each helper (every helper's text
  emitted once, with `importHelpers`, with `noEmitHelpers`) and for private
  names under each target.
- **Exit:** the `estransforms` files read `mapped`; the witness rows match in
  `output`.

### T6 — the JSX transform

- **Build:** `jsxtransforms/jsx.go` (1,209 lines): the classic factory over
  the resolver's `GetJsxFactoryEntity` and `GetJsxFragmentFactoryEntity`, the
  automatic runtime over the program's `GetJSXRuntimeImportSpecifier` and
  `GetJSXImplicitImportBase`, `react-jsxdev`'s `jsxDEV` with the source
  location argument, children and spread handling, entity and whitespace
  text rules, the `key` argument, `preserve` and `react-native` (no
  transform: the printer prints JSX syntax).
- **Witnesses:** the 448 JSX rows (preserve 226, react 163, react-jsx 38,
  react-jsxdev 19, react-native 2); the C4 fixtures' JSX cases emitted under
  every mode as a direct set (the C4 native records hold their diagnostics;
  T6 records their emit natively too).
- **Exit:** `jsx.go` reads `mapped`; the JSX rows match in `output`.

### T7 — declaration emit

- **Exists:** `transform_declarations` with its tracker and diagnostics; the
  declaration half of the resolver; `Program.GetDeclarationDiagnostics`.
- **Build:** the 116 unmarked `declarations/*.go` functions: the parts of
  `transform.go` that build output the diagnostics path skips, `util.go`,
  `tracker.go`'s remaining reports, `supplementalreferences.go` (the
  content-mapper references), `diagnostics.go`'s message selection for every
  declaration diagnostic; `isolatedDeclarations` (27 rows) with the
  pseudochecker completed (`type.go`'s pseudo types and `lookup.go`'s 15
  unmarked functions); `stripInternal`; `emitDeclarationFile` with its gates
  (`declBlocked`, `noEmit`, `IsEmitBlocked`, `EmitOnlyBuilderSignature`,
  `forceDtsEmit`); `declarationMap` (15 rows) with the printer's
  `MapSourcePosition` handler over the span map for content-mapped files and
  `newDeclarationMapSource`; `Program.GetSourceFileFromReference`; the
  declaration printer options (`OnlyPrintJSDocStyle`, `NoEmitHelpers`,
  `OmitBraceSourceMapPositions`).
- **Witnesses:** the 2,097 `.js` baselines with `.d.ts` sections, the 23 with
  `DtsFileErrors` (the emitted declarations compiled again as a second
  program and checked, as `compileDeclarationFiles` does, through
  `CheckedProgram`), the 15 content-mapper rows' declarations, the transpile
  declaration baselines (T8); the declaration diagnostics of the 1,754 native
  requests unchanged (the Phase 2 receipt re-verified).
- **Exit:** the `declarations` and `pseudochecker` files read `mapped`; the
  declaration rows match in `output`; `run.checker.*` unchanged on the same
  sources.

### T8 — emit orchestration, transpile, both modes and closure

- **Build:**
  - `CheckedProgram::emit` (`Program.Emit`: `HandleNoEmitOptions`, the
    emitted-file selection with `sourceFileMayBeEmitted` and the JSON rules,
    one `emitter` per file in a `WorkGroup` under the program's
    single-threaded setting, the writer pool, `OutputPaths` per file,
    `CombineEmitResults` in input order), the `emitter` (`emit`,
    `emitJSFile`, `emitDeclarationFile`, `printSourceFile`, `writeText`,
    `EmitOnly` and `ForceEmit`, `EmitBOM`, `newLine`, `EmitSkipped`,
    `EmittedFiles`, `SourceMaps`, the `Could_not_write_file` diagnostic), the
    emit host (`emitHost.go`'s 28 operations over `CheckedProgram` and the
    file's checker's resolver, `WriteFile` through the program host's file
    system or a caller's `WriteFile` callback with `WriteFileData`),
    `outputpaths`' remaining 13 functions, the trace pushes.
  - `tsr_transpile`: `TranspileModule` and `TranspileDeclaration` with the
    option overrides, the default file name, the barebones lib text, the
    `transpileFS`, `SkipModuleResolution`, `ReportDiagnostics`.
  - The corpus driver's emit path completes (the `unsupported` of T0 gives
    way to the emit), the pre/post-emit count mismatch is observed (three
    native rows differ), the `noCheck` repeat and the declaration
    re-compilation run inside the harness.
  - The two modes: the concurrent capture compared against the concurrent
    native capture; `phase3_compare.py modes` with zero outcome differences
    between the modes' Rust runs.
  - `emit` on `tsr_embed::Session` and the WebAssembly checker entry
    (decision 9), with the in-memory write callback, without a Phase 7 claim.
  - Closure: the residual list, the divergence ledger check, the function
    audit complete over every Phase 3 file, the ledger `ported` with `verify`
    checks that derive `verified`, the S07 anchor refresh the ports move,
    the green-up (both runners green, `checker` and `emit` re-recorded in
    both modes), the Phase 3 record with the per-area dashboard and the
    consumption report for Phases 4 to 7, one bounded emit timing capture
    without a threshold (decision 10).
- **Exit:** the P3B exit (section 5) on recorded runs; `P3B-T8` passes.

## 5. Acceptance and evidence design

Namespace: sprints `P3A` (stage A, T0) and `P3B` (stage B, T1 to T8); data
under `data/phase3/`; scripts `scripts/phase3_*.py` over the phase2 libraries;
one producer, `emit`, with its inputs and sources declared in
`status/runs.toml`; contract witnesses `t1-contracts` to `t8-contracts` with
receipts as Phase 2's.

| Required claim | Evidence and denominator | Reuse |
| --- | --- | --- |
| The `.js` baseline of every executed variant matches | 13,432 rows: 12,174 with a `.js` reference, 1,258 graded `<no content>`, 69 `disabled` for `output` with the pin's reason; the rendered baseline text is compared whole, and the per-file outputs, `EmittedFiles`, `EmitSkipped` and the emit diagnostics are compared for attribution | The C0 harness, the S08 error-writer port pattern |
| Declarations match | The 2,097 baselines with `.d.ts` sections, the 23 `DtsFileErrors`, the `noCheck` repeat (0 baselines differ, but the repeat runs), the 15 mapped rows | `CheckedProgram` for the re-compilation |
| Source maps match | 150 `.js.map` and 157 `.sourcemap.txt` baselines; `<no content>` graded on the rest | T2's decoder for the record |
| Transpile matches | The transpile runner's configurations behind the 41 baselines, both `TranspileModule` and `TranspileDeclaration` | The native capture of the transpile runner |
| Emit diagnostics match | The post-emit program's emit diagnostics and the count-mismatch diagnostic on every row | Phase 2's pre/post comparison, now over an executed emit |
| Both modes agree | The single-threaded and concurrent runs, each against its mode's native capture, with zero outcome differences between them | C6's `modes` comparison |
| The printer is complete | The reprint witness over every corpus file (with and without comments) and the ported printer tests | New native capture (decision 7) |
| Phase 2's gate is preserved | `run.checker.errors_parity`, `types_parity`, `symbols_parity`, `display_parity`, `trace_parity`, `ordering`, `parent_pointers` at 1 and `unsupported_required == 0` on the final sources, in both modes | The `checker` producer, re-recorded at T8 |
| Ownership, recursion, cancellation, determinism | Contracts: a transform arena and emit side tables released per file; emitted text owns no arena; a panic during one file's emit retires the generation and fails the group; cancellation before emit; two runs byte-identical; the parallel and single-threaded emits identical; the deep-input fixtures | S09's ownership harness shapes, C6's retirement contracts |
| Function disposition | Every function of the 73 Phase 3 files `mapped`, `equivalent` with a site or `later` with an owner; no `gap` at T8 | `scripts/phase2_audit.py`'s rules, a Phase 3 scope |

Producer metrics (`run.emit.*`): `inventory_frozen`, `native_verified`,
`native_verified_concurrent`, `harness_valid`, `harness_valid_concurrent`,
`result_recorded`, `blockers_named`, `unsupported_required`, `output_parity`,
`declaration_parity`, `sourcemap_parity`, `sourcemap_record_parity`,
`emit_diagnostics_parity`, `transpile_parity`, `reprint_parity`,
`mode_parity`, `residuals`, `dispositions`, `evidence_current`, `report`, and
per checkpoint `tN_complete` bound to `P3B-TN` as recorded facts (C7.7's
tracker extension). The P3B exit is:

```text
sprint.P3A.done == 1
run.emit.harness_valid == true
run.emit.harness_valid_concurrent == true
run.emit.output_parity == 1
run.emit.declaration_parity == 1
run.emit.sourcemap_parity == 1
run.emit.sourcemap_record_parity == 1
run.emit.emit_diagnostics_parity == 1
run.emit.transpile_parity == 1
run.emit.reprint_parity == 1
run.emit.mode_parity == true
run.emit.unsupported_required == 0
run.emit.residuals == 0
run.checker.errors_parity == 1 (and the six other Phase 2 metrics)
```

No threshold is introduced. A retained difference passes only with an
owner-approved entry in `data/divergences.toml` (ADR 0004), which needs an
exact observation witness per variant and metric; the Phase 2 rule that
failures, missing operations and native-unavailable rows cannot be waived
holds.

Every recorded fact keeps the Phase 2 discipline: immutable captures bound to
the pin, the request digests, the executable and the source closure;
`replay` before recording; the receipts binding the contract sources; the
native capture verified from two shardings and reviewed against the committed
references; the residual list and the register rebuilt from evidence, never
edited. `cargo xtask run emit` and `status --record` remain the owner's.

## 6. Cost control and performance risk

- **Runs.** One native capture per mode (the C0 capture cost plus the emit;
  the emit sub-tests also run the `noCheck` repeat and, for about 2,100 rows,
  the declaration re-compilation). One Rust full run per mode at each exit;
  the recorded Phase 2 run takes about five minutes, and emit adds the
  transforms, the printing, the repeat and the re-compilation: measured at
  T0, expected under three times that. Sample runs (the 300-variant sample)
  during checkpoints; the reprint witness is cheap and runs whenever the
  printer changes.
- **Threads.** Emit queues one task per emitted file. With the bounded work
  group this is the pin's shape at the pin's cost; without it, a
  whole-program emit on a large program starts one 256 MiB-stack thread per
  file. The dependency is named in section 3 and blocks T8's concurrent
  measurement, not the corpus (its programs are small).
- **Allocation.** A transform arena per file and per chain, released after
  printing; helper texts as static bytes; `JsString` sharing for literal
  text copied from source. Keep the phase timers; record one bounded emit
  timing capture at T8 with no threshold. Phase 7 owns the budgets and the
  benchmark scenarios ([S12-acceptance.md](S12-acceptance.md)).
- **Recursion.** The printer recurses on the left operand of binary
  expressions and the transforms recurse with the visitor. The checker's
  guard (`stacker::maybe_grow` at the deepest paths, on the reserved stacks)
  applies; T1's stress fixtures decide whether any site needs a trampoline
  (PLAN section 13, item 11). No trampoline is added without a failing
  fixture (decision 8).
- **Correctness first.** No speed or memory gate; unfavorable timing is
  reported beside Go's without conversion.

## 7. Risks

| Risk | Where it shows | Mitigation |
| --- | --- | --- |
| Comment fidelity: trivia scanning, detached and nested comments, JSDoc-only emission, line preservation | Whole-corpus `output` differences of a few bytes | T1's reprint witness over every file, with and without comments, before any transform lands |
| Name generation order: `generatedNames` is process-wide in the pin ("to match Strada"), temp flags per scope, reserved names | Renamed temporaries in downleveled output | Port `namegenerator.go` as written, including its Strada-compatibility note; its 39 tests |
| Helper texts and order | `importHelpers`/`noEmitHelpers` rows, every downlevel row | The 27 texts as static bytes checked against the pin's; `compareEmitHelpers` |
| Source-map columns are UTF-16 code units | `.js.map` rows with non-ASCII text | The writers' column tracking ported with the position rules of ADR 0013; T2's fixtures include surrogate pairs and malformed input |
| The `noCheck` repeat and the declaration re-compilation double the harness work and depend on the checker under `noCheck` | The `output` baseline of declaration rows | Both are observed natively and reproduced in the harness; the checker's `noCheck` path is Phase 2's and already recorded |
| Parallel emit order | The 8 skipped tests; any output-name collision | The harness sorts outputs by input order as the pin's does; the skipped tests stay informational |
| Content-mapped declaration maps | The 15 mapper rows with `declarationMap` | `MapSourcePosition` over the span map; the mapper rows' declarations are a T7 witness |
| A resolver query the transforms need that Phase 2 did not port | Any transform row | A named joint blocker owned by Phase 2 maintenance, with the pinned caller; not a Phase 3 reimplementation |
| Staleness | Every recorded run reads `stale` while Phase 3 changes shared crates | Accepted mid-phase (decision 12); T8's green-up |

## 8. Owner decisions (2026-10-01)

The owner reviewed the twelve proposals on 2026-10-01: 1, 3, 4, 5 and 6
confirmed as written, 2 left to the implementer ("your call", recorded below
as proposed), and the rest confirmed with the plan ("lgtm"). Each entry keeps
its proposal and records the outcome.

1. **The ledger move.** `compiler/emitter.go` and `compiler/emitHost.go` move
   from Phase 4 to Phase 3 in `PORTS.toml`; `program.go` stays Phase 4's
   while Phase 3 ports its `Emit` family as marked operations. The move
   changes `data/upstream.json` and is coordinated with C7.6's green-up.
   **Confirmed.**
2. **Crates.** New `tsr_sourcemap` and `tsr_transpile`; the declaration
   transform stays in `tsr_transformers` (PLAN's crate map names a separate
   `tsr_declarations`); output paths stay split between
   `tsr_tsoptions::output_paths` and `tsr_compiler::output_paths` with no
   `tsr_outputpaths` crate. The record notes both deviations from the crate
   map. New crates register in `tools/packaging/packages.json`.
   **The owner left this to the implementer; it stands as proposed.**
3. **Names.** Checkpoints T0 to T8, sprints `P3A` and `P3B`, producer `emit`,
   data under `data/phase3/`. **Confirmed.**
4. **Native captures.** Emit captured natively in both test-program modes on
   this host (as C6's were), plus the transpile runner; `phase3_native.py`
   with its own driver beside `phase2_native.py`, which is not edited.
   **Confirmed for this host.** No Linux capture was asked for; if one is
   wanted later, it joins as a second provenance file, as C7.6 planned for
   the assignments.
5. **Baseline authority.** The rendered `.js`, `.js.map` and `.sourcemap.txt`
   texts, composed as the pin's writers compose them (including the
   declaration re-compilation's `DtsFileErrors` and the `noCheck` repeat),
   compared whole, with per-file outputs beside them for attribution; the
   writers are ported as harness code, as S08 ported the error writer.
   **Confirmed** ("yes emit": the emitted texts, composed as the pin's
   writers compose them, are the authority).
6. **The runner's limits.** The 8 `skippedEmitTests` variants and the 61
   declaration-only-root variants are `disabled` for `output` with the pin's
   reasons and stay informational; the `sourcemap` sub-tests still grade
   them. **Confirmed.**
7. **The reprint witness.** A new native capture of `EmitSourceFile` over
   every corpus file, with and without comments, as a standing `emit` input
   and a `reprint_parity` metric. **Confirmed.**
8. **Recursion.** The checker's growth guard on the reserved stacks for the
   printer and the transforms; trampolines only where T1's stress fixtures
   fail. **Confirmed.**
9. **Entry points.** T8 exposes `emit` on `tsr_embed::Session` and the
   WebAssembly checker entry with an in-memory write callback, without an
   acceptance claim; Phase 7 grades them. **Confirmed as proposed:** T8
   exposes them.
10. **Performance.** One bounded emit timing capture at T8, recorded beside
    Go's, no threshold. **Confirmed.**
11. **Tracing.** The emit phase's trace pushes go through C6's `TraceSink`
    now; the file writer stays Phase 4's. **Confirmed.**
12. **Evidence.** Phase 3 changes stale the recorded `checker` run and the
    captures that bind the edited inputs; no re-recording per fix; T8's
    green-up re-records `checker` and `emit` in both modes, and the
    recordings are the owner's. **Confirmed.**

T0 starts on this plan; the T0 record (`docs/PHASE3-T0.md`) carries the
measured cost, the frozen inventory and the first categorized run, and each
later checkpoint keeps its record beside it, as Phase 2's did.
