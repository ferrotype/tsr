# Phase 3 record: emit

T1 to T8 of the [Phase 3 plan](PHASE3-plan.md), implemented on branch
`phase3`. Upstream remains Corsa `1f70213d4922b434345f639b441681e470c7cfc1`.
[PHASE3-T0.md](PHASE3-T0.md) is the T0 record; this record covers the emit
itself. Nothing here is a recorded run: `cargo xtask run emit` and the
green-up are the owner's (decision 12).

## Result against Go

The Rust harness runs what the pin's compiler runner runs for its `output`,
`sourcemap` and `sourcemap record` sub-tests on every executed variant: the
post-emit program's `Program.Emit` through an in-memory recorder, the three
baseline writers, the declaration re-compilation with its `DtsFileErrors`, and
the `noCheck` repeat as a real second compile and emit.

| Domain | Rows | Match | Different | Disabled by the pin |
| --- | ---: | ---: | ---: | ---: |
| `output` (the composed `.js` baseline) | 13,432 | 13,363 | 0 | 69 |
| `declaration` (the `.d.ts` outputs) | 13,432 | 13,432 | 0 | 0 |
| `sourcemap` (the `.js.map` baseline) | 13,432 | 13,432 | 0 | 0 |
| `sourcemap_record` (the `.sourcemap.txt` baseline) | 13,432 | 13,432 | 0 | 0 |
| `emit_diagnostics` (the emit result and the counts) | 13,432 | 13,426 | 6 | 0 |
| `reprint` (the printer over every corpus file) | 13,432 | 13,432 | 0 | 0 |

No row failed, none was refused (`unsupported_required` is 0) and the harness
reported no error. The same table holds for the concurrent mode against the
concurrent native capture, and the two Rust runs have 0 outcome differences
and 0 differing observations.

Run directly, `python3 scripts/phase3_producers.py emit` reports
`output_parity`, `declaration_parity`, `sourcemap_parity`,
`sourcemap_record_parity` and `reprint_parity` at 1, `emit_diagnostics_parity`
at 0.99955 (13,426 of 13,432), `mode_parity`, `harness_valid` and
`harness_valid_concurrent` true, and `unsupported_required` 0.

Transpile: `python3 scripts/phase3_transpile.py compare` matches 41 of the 41
native baselines (28 configurations, `TranspileModule` and
`TranspileDeclaration`). Its metric is `transpile_parity` in the comparison's
`comparison.json`; the `emit` producer does not read it yet.

## The six rows that differ

`compiler/incrementalConcurrentSafeAliasFollowing`, `incrementalConfig`,
`incrementalTsBuildInfoFile`, `jsEmitIntersectionProperty`,
`noEmitAndIncremental` and `noEmitAndIncrementalListFilesOnly` (configuration
0 of each) set `incremental`. The pin's harness then emits through
`incremental.NewProgram`, whose `EmittedFiles` ends with the `.tsbuildinfo`
path. The Rust `Program.Emit` writes no build info, so that one entry is
missing; every baseline, output and diagnostic of the six rows matches.

The cause is the `execute/incremental` package, which the plan assigns to
Phase 4 ("build-info emit is Phase 4's"). The register
(`data/phase3/blockers.json`) names the bucket `B01` under T8, because it
attributes by domain; its owner is a decision for the Phase 3 exit, since
`emit_diagnostics_parity == 1` is not reachable without that package or an
approved entry in `data/divergences.toml`.

## How the corpus got here

The first full run matched 13,411 rows on every domain. The 21 others had
four causes:

| Cause | Rows | Fix |
| --- | ---: | --- |
| `export` missing on a JSDoc typedef or reparsed namespace of a CommonJS file | 13 | `IsImplicitlyExportedJSDocDeclaration` reads the file through the binding-aware read: the binder sets the CommonJS indicator |
| No `/// <reference>` to a content mapper's supplemental declaration | 1 | a mapped file names its supplemental files; the supplemental references transformer finds them through the host |
| An extra TS1066 in a declaration program (`fakeInfinity3`) | 1 | enum evaluation asks the full `isBlockScopedNameDeclaredBeforeUse`, with its deferred-usage rule |
| No `.tsbuildinfo` in `EmittedFiles` | 6 | open, above |

The comparison was mutation checked end to end: one extra space after the
first `var` of every written file makes 113 of the first 300 rows differ in
`output`. Its own comparisons and validations have 20 mutants, all killed by
`scripts/tests/test_phase3_*.py`.

## What was built

| Checkpoint | Production | Witness |
| --- | --- | --- |
| T1 printer | `tsr_printer`: the printer with comments, the name generator, the emit context and its 27 helpers, the text writers | the pinned printer's own tests, the names and helpers witness (137 cases), the reprint witness over 18,517 files |
| T2 source maps | `tsr_sourcemap`, the printer's source-map positions | the source-map witness (117 cases), 150 `.js.map` and 157 `.sourcemap.txt` corpus baselines |
| T3 framework and TypeScript transforms | the transformer framework, type eraser, import elision, runtime syntax, legacy decorators, metadata | native transform probes |
| T4 module and inliner transforms | CommonJS, ES module, implied module, const-enum inlining | native transform probes |
| T5 ES downlevel | class fields, ES decorators, using, object rest and spread, async, for-await, optional chain, nullish coalescing, logical assignment, exponentiation, optional catch, tagged templates, use strict | native transform probes |
| T6 JSX | the JSX transform | native transform probes |
| T7 declarations | the declaration transformer, the supplemental references transformer, the pseudochecker | native declaration probes with their diagnostics |
| T8 orchestration | the emitter, the emit host, `CheckedProgram::emit`, `tsr_transpile`, `emit` on `tsr_embed::Session` and on the WebAssembly checker session | 57 emitter rows in both modes, 41 transpile baselines, the corpus |

The probes (`scripts/phase3_probe.py`,
`crates/tsr_compiler/tests/phase3_transforms.rs` and
`phase3_declarations.rs`) run any chain of pinned transformers over a
compiler-test source natively and require the ported chain to print the same
bytes: 2,182 transformer chains and 262 declaration chains at the time of this
record.

Function audit (`python3 scripts/phase3_audit.py check --complete`): 1,942
functions in scope (the 73 Phase 3 files and the `Emit` family of
`program.go`), 1,928 `mapped`, 14 `equivalent` with a site, no `gap`.

## Contracts

`crates/tsr_compiler/tests/t1_contracts.rs` to `t8_contracts.rs`, with
receipts under `data/phase3/receipts/` written by
`python3 scripts/phase3_receipts.py observe tN-contracts`
(`receipt_current("tN-contracts")` for a producer). Each contract was
mutation checked.

| Witness | Contracts |
| --- | --- |
| `t1-contracts` | the printer prints deep inputs on a small stack (binary and `**` chains, parentheses, JSX, `else if`, conditional types); generated names restart in each file |
| `t2-contracts` | each output maps only its own source, in input order; deep inputs with maps |
| `t3-contracts` | a file's transform arena and side tables are released with its emit and the source is unchanged; deep inputs through the TypeScript transforms |
| `t4-contracts` | module helpers belong to the file that needs them; deep imported, const-enum and ambient chains |
| `t5-contracts` | downlevel helpers belong to the file that needs them; deep downlevel inputs |
| `t6-contracts` | JSX runtime imports belong to the files that use them; deep JSX in every mode |
| `t7-contracts` | the declaration transform is released per file; deep type nesting |
| `t8-contracts` | a panic in one file's emit retires the generation and fails the group; cancellation before emit; determinism across runs, loads and modes; results own no arena; bounded workers |

None of the named contracts needed a production change in the emitter. The
deep inputs did: 14 growth guards of the checker's `stacker::maybe_grow`
pattern, in the printer (`emitJsxChild`, `emitIfStatement`), the transformers
(JSX, optional chain, declarations, runtime syntax, legacy decorators, class
fields) and the checker (`resolveEntityName`, `getWidenedTypeWithContext`,
`isConstContext`, `getContextualType`).

Open findings, each kept as an ignored test in
`crates/tsr_compiler/tests/phase3_findings.rs`:

- Deep destructuring patterns and nested `using` blocks still overflow the
  stack (`recordDeclarationInScope`, two helpers of `destructuring.rs`,
  `usingDeclarationTransformer.visit`).
- The checker's live heap grows across repeated declaration emits of
  functions with inferred return types (12 to 140 KB per emit) while the
  type, symbol and signature counts stay constant; not root-caused, likely in
  the node builder's output path.
- `AstBuilder::factory_view` walks a node's parent chain on every
  imported-node read, so a transform is quadratic in nesting depth:
  `binderBinaryExpressionStress` takes about 60 seconds per mode in a debug
  build. The printer's `getTextOfNode` and several transforms are quadratic
  in the pin's own algorithm.
- With `noEmitOnError`, an emit after cancellation returns
  `Err(Checker(PreviouslyCanceled))` where the pin panics with "Checker was
  previously cancelled".

## Deviations and decisions taken in the port

1. The ledger move of `compiler/emitter.go` and `compiler/emitHost.go`
   (decision 1) is still deferred to the green-up: it changes
   `data/upstream.json` and would stale the Phase 2 captures. The audit takes
   the union, so its counts hold before and after the move.
2. Native rows keep per-file output digests; texts are captured only with
   `--texts`.
3. A transform's node is not readable by the checker behind the emit
   resolver. The resolver answers such a node as the pin's
   `!IsParseTreeNode` guards do. A transform that makes an identifier pass
   for a parse-tree node (the JSX namespace identifier, the metadata
   serializer's clone of a type name, whose flags `cloneNode` copies from the
   parsed name) declares it with `treat_as_parse_tree_identifier`, and the
   resolver answers later reference queries about it through an identifier
   of its own with the same name and parent.
4. The emit host is the declaration transformers' host, as the pin's is.
   `ProgramDeclarationHost` remains for declaration diagnostics and the
   probes, which have no checker host.
5. `Program.GetDeclarationDiagnostics` exists in two forms: the per-file
   checker form the pin has (`CheckedProgram::declaration_diagnostics`) and
   the single-owner form the Phase 2 harness still calls.
6. The entry points (decision 9) carry no acceptance claim. The WebAssembly
   request refuses `EmitOnly::BuilderSignature`, as the pin's API range does.
7. `scripts/phase2_producers.py` gained `LATER_PHASE_RUNS = ("emit",)` so
   C7's evidence check ignores the new run.

## Changes that reach Phase 2's checker

Each follows the pin and each can change a recorded `checker` result, so the
`checker` run has to be re-recorded at the green-up:

- `getTargetOfExportSpecifier` takes its meaning and alias parameters:
  `markLinkedAliases` asks with `Alias` in the meaning, so a local
  `export { x }` marks the import that declares `x` visible.
- An assignment declaration no longer late-binds an index signature
  (`late_members.rs`).
- Enum evaluation uses the full `isBlockScopedNameDeclaredBeforeUse`.
- The reference-resolver hooks `get_parent_of_symbol` and
  `get_symbol_of_declaration` read late-bound symbols (runtime syntax).

## Not done

- The six `incremental` rows above.
- The `transpile_parity` metric and the contract receipts in the `emit`
  producer (`status/runs.toml` does not list the receipts as inputs yet), the
  `tN_complete` facts, the residual and disposition files, the bounded emit
  timing capture (decision 10).
- The green-up: the ledger move, the S07 `operations.json` anchors and the
  re-freeze of the subset review, STATUS regeneration, the recorded `checker`
  and `emit` runs in both modes, the stale E7 and E8 captures (the entry-point
  unit edited `docs/S10.md` and `tools/s10/wasm/`), and Phase 2's
  `c7-audit.json`, which still lists as `later` or `equivalent` eight
  functions Phase 3 ported and marked.
- Found and left as they are: `data/phase2/inventory.json` records a stale
  digest of `data/phase1/syntax-schedule.json`; `tools/s10/sources.json`
  lacks `crates/tsr_ipc`, which fails one test of `scripts/test_s10.py`.

## Cost

A full Rust run takes about 190 seconds per mode on this host (16 jobs,
harness build cached), the comparison about 6 seconds, the producer about one
minute. `compiler/intersectionConstructorReductionCrash` checks for about 45
seconds on its own in the debug harness (the row checks a pre-emit and a
post-emit program) and can cross the default 60-second deadline under 16
jobs, which fails every domain of the row; the single-mode run of this record
used `--timeout 120`. The native captures are T0's. The probe suite takes about 13 minutes
for the transformer chains and 90 seconds for the declaration chains.

## Reproduction

```sh
export PATH="$(mise where go)/bin:$PATH"
python3 scripts/phase3_corpus.py run --native target/phase3/native-single --output target/phase3/rust-single --mode single --timeout 120
python3 scripts/phase3_corpus.py run --native target/phase3/native-concurrent --output target/phase3/rust-concurrent --mode concurrent
python3 scripts/phase3_compare.py report --native target/phase3/native-single --rust target/phase3/rust-single
python3 scripts/phase3_compare.py modes --rust target/phase3/rust-single --rust-concurrent target/phase3/rust-concurrent
python3 scripts/phase3_transpile.py run --output "$SCRATCH/transpile" && python3 scripts/phase3_transpile.py compare --rust "$SCRATCH/transpile"
python3 scripts/phase3_producers.py emit
python3 scripts/phase3_audit.py check --complete
```
