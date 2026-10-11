# Phase 7 R3: WebAssembly correctness progress

Status: **in progress**, 2026-10-11. Single-file transpile now crosses the
production wasm boundary. The full compiler-harness correctness run passes. Comprehensive deep-input
stress and the ADR 0020 budgets remain incomplete.

## Implemented boundary

`tsr_wasm`'s `checker` feature exports `transpile_module` and
`transpile_declaration`, both through `tsr_embed`'s public single-file
operations. The production crate does not import acceptance-tool schemas or
run a whole-program substitute. Its version-1 response retains output,
source-map bytes, structured diagnostics and every source needed to resolve
diagnostic file indices. Raw and display arguments, message identity, related
information, chains, flags and repopulation metadata cross that boundary.
Mapped and supplemental sources are explicitly rejected in this first
single-file response format.

The [external Node bridge](../tools/phase7/portable/wasm.mjs) requires an
explicit production artifact directory and verifies its build record,
artifact hashes, imports and exported operations. The executable
`tools/phase7/portable/wasm-consumer` starts Node with the existing pinned
`--stack-size=4096`. Requests keep source/name byte strings and raw ordered
compiler-options JSON. Malformed requests/results and artifact failures
fail the child; they produce no replacement baseline observation.

`tsr_testrunner` expands transpile cases, runs their native module/declaration
schedule, and renders and compares the ordinary baselines from returned
observations. There is no host compiler fallback for a missing response.
This is the `transpile-wasm` acceptance route; the Node capture commands and
expectation file are recorded with the task's final correctness checks.

## Browser smoke

The [committed browser page and runner](../tools/phase7/portable/browser/README.md)
exercise actual production parse/encode, memory-host checking, resolution
traces and type queries,
JavaScript/declaration emit, both single-file transpile modes, the committed
S10 nested-input fixture and trap invalidation followed by a fresh instance.
The trap uses the existing parser's invalid relative-filename precondition;
it is an actual Rust abort through wasm, not a modeled controller exception.
The smoke fixes exactly six case identities and all three engine identities;
an empty result cannot pass.

The existing bundled Playwright 1.62.1 package was used. Its matching engines
were provisioned to `/Users/cristian/git/ts-rust-benchmarks/tools/playwright`,
outside Cargo cleanup and capture directories: Chromium 151.0.7922.34
(revision 1234), Firefox 153.0 (revision 1538), and WebKit 26.5
(revision 2336). Downloaded archives total about 453 MiB; the unpacked cache
is about 1.1 GiB. No system-browser or mismatched cached-engine substitution
was used.

Initial development launch attempts aborted all engines before the page ran
under the OS sandbox. The unmodified failure record is retained under
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r3-browser-launch-failure/`.
An authorized launch outside that sandbox ran all six cases successfully in
Firefox and WebKit. Chromium rejected synchronous main-thread compilation
of the production wasm binary because it exceeds Chromium's 8 MiB limit.
That failed diagnostic capture is retained under
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r3-browser-development-escalated/`.
The page now uses asynchronous `WebAssembly.compile` and the shared
`createInstanceAsync` initializer, without raising an engine limit. Node's
synchronous entry point remains available and uses the same ownership and
trap controller.

The refreshed production-artifact capture is retained at
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r3-browser-async-deep/browser-smoke.json`.
It uses wasm SHA-256
`d0e43655215dc9e5b9a15cf0091de95ba3d0a46c21d3c3b7907b9d3d2bbe419e`.
All three matching engines pass the five ordinary cases, including the actual
Rust abort and fresh-instance recovery. **The required deep case fails in all
three engines; the capture and runner exit status are failed.**

That case first checks the 64-parenthesis S10 fixture, then a 5,001-term
left-addition chain (5,000 binary levels) using the native ADR 0011 transform
contract's source family. Parsing, AST encoding and memory-host compilation
succeed; `session.diagnostics()` overflows the engine call stack before emit.
Chromium and WebKit report `RangeError: Maximum call stack size exceeded`;
Firefox reports `InternalError: too much recursion`. Repeated production wasm
frames are functions 224, 5896 and 935 (offsets `0x29a6f7`, `0xa01e86`,
`0x52e899`). The built response artifact has no function-name custom section.
The deep case remains required; it has not been reduced to obtain a passing
result. The preceding synchronous-instantiation diagnostic capture is retained
under `r3-browser-refreshed-deep/` beside the final capture.


The expression checker now uses explicit binary/parenthesis frames on wasm.
The native `wasm_expression_stack` integration test passes all three tests:
fresh-owner comparisons preserve complete diagnostics, type/allocation counts,
sampled expression traces and logical depth across the operator/context matrix,
the 5,001-term chain and 5,000 parentheses. Native default checking remains
recursive with stack growth.

A second browser capture at
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r3-browser-expression-stack/browser-smoke.json`
uses wasm SHA-256
`2b9175fdf3cdfc3de20ddcc81837ce41bfddaecf1503cf03992e279928c58e9e`.
All three engines now complete deep diagnostics, then overflow during emit.
Mapping the retained input with the pinned wasm-bindgen while retaining names
produces byte-identical noncustom wasm sections. The repeated frames map to
`subtree_facts::Facts::{propagate_present,compute,facts_worker}`; the mapping
record is retained under `r3-emit-symbol-map/` beside the captures.

Subtree facts now use explicit binary/parenthesized/partially-emitted spines on
wasm, preserving cache early exits, noncached transparent wrappers, original
propagation order, scope masks and binary operator rules. Four tests compare
separate cold native and iterative AST owners, including cache mutation,
pruned malformed type children and 5,000 mixed spine levels. All 250 AST
crate tests and clippy pass.

The next all-engine capture at
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r3-browser-facts-printer-stack/browser-smoke.json`
uses SHA-256 `e861510b30dfe1ffe38af9b187fe2b2e474464b74902fa57817b221faa5e58cc`.
The deep case gets past subtree facts and printer preparation, then overflows
in `ESModuleTransformer::visit` through the generated binary child visitor.
The byte-identical noncustom-section name mapping is retained under
`r3-emit-visitor-symbol-map/` beside the captures.

The ES module transformer now schedules its binary/parenthesized/partially
emitted expression spines explicitly on wasm. It preserves the generated
modifier/left/type/operator/right order and uses the original factory updaters;
other callbacks still enter `NodeVisitor` and its environment hooks. Native
cold-owner equivalence tests pass for rewritten dynamic imports, ranges,
flags, original links, callback order, first-failure unwind, partially emitted
wrappers after actual type erasure, and the complete 5,001-term expression.
The original three focused cases passed in 173.83 seconds in the debug build;
the newly added partially emitted case passed separately. A redundant repeat
of the unchanged deep case was stopped explicitly, not reported as a pass.
Transformer clippy passed with warnings denied. Native default remains recursive.

The production rebuild and all-engine capture at
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r3-browser-esmodule-stack/browser-smoke.json`
uses SHA-256 `585b90e3b0b880692e8edbb175245d3610922ede4cc72d607186b43f42a8f2c2`.
All engines again pass five ordinary cases; the unchanged required deep case
now overflows in the const enum inlining visitor during emit. The concrete
repeated frame is `new_const_enum_inlining_transformer::{closure#1}`, mapped
with byte-identical noncustom sections under
`r3-emit-next-visitor-symbol-map/`. This failed capture does not establish deep
emit acceptance.

Const enum inlining now uses the same private expression-spine continuation
as the ES module pass for its known default callbacks. Native cold-owner
comparisons cover real constant replacements/query order, comments, first
failure, partially emitted wrappers and a 5,001-level identity chain; the
focused tests and transformer clippy pass.

The final focused browser capture at
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r3-browser-constenum-trace/browser-smoke.json`
**passes all six required cases in all three engines**, exit status zero.
The production wasm SHA-256 is
`52684989e221a57298fd3b5718ff95da819964792c767950374beb789066986d`
(21,498,192 bytes), with binding SHA-256
`90d8e31d532cd6752f61ab27b460f9f8de79c4f9a20c93b6bf82685c4aa8eb3e`.
The eight exact served inputs are preserved beside that capture under
`artifacts/`, with a route/file/hash manifest and detached replay notes. All
archived hashes were verified against the original capture, then independently
reverified using only files outside `target`; the original capture is unchanged.
Chromium 151.0.7922.34, Firefox 153.0 and WebKit 26.5 run without engine stack
flags. The unchanged 5,001-term case parses 15,015 nodes/5,003 identifiers
without parse diagnostics, encodes 480,853 bytes, checks without source errors
and emits the exact complete 20,020-byte JavaScript output. The 64-parenthesis
fixture and real abort/poison/refusal/fresh-instance case also pass.

The check/API-error case now additionally exercises a real two-file import
and the production `WasmSession.resolution_trace()` snapshot. The recorded
trace retains byte message keys, typed text/bool arguments and original order;
the browser asserts exact resolving/success identities and path arguments.
Two native controls pass for typed lossless argument encoding and equality
with the public embedding program's trace. `tsr_wasm` library/test clippy
passes with warnings denied. The snapshot performs no checker queries or
acceptance schedule and its production schema imports no tool modules.

This is one deep grammar family, not the whole native ADR 0011 visitor matrix.
Native stack growth and Node's larger engine stack are separate contracts.
Artifact identity is verified against the recorded build; this capture does
not claim that concurrently edited workspace sources match the built artifact.

## Checks and remaining work

Focused Node wrapper tests pass: 8 tests cover raw byte/option transport,
malformed/duplicate shapes, instance poisoning and fresh recovery, empty or
malformed child responses, artifact identity, and explicit artifact selection.
Three browser-wrapper regressions pass, including asynchronous initialization,
terminal trap handling and automatic archive preservation. The runner now saves
the exact served buffers to each capture's `artifacts/` before engine launch;
a focused test mutates and deletes the source artifact directory, then verifies
all eight saved assets against the live HTTP snapshot hashes. The accepted
browser capture predates this automatic archival addition and was preserved
separately without rewriting its original record. The output guard verifies
that an output under `target`,
including a symlink alias, is rejected before launch; existing capture
directories are also rejected.

Owner decision **2026-10-11** permits a test-only wasm consumer of public
`tsr_embed` for `compiler-wasm`. The independent manifest in
`tools/phase7/portable/wasm-compiler` links public embedding operations and value
APIs; its adapter imports the same acceptance executor, diagnostic codec,
mapper codec and pinned walker as the native external consumer. No production
crate imports those acceptance modules. The synchronous Node callback import
preserves the native mapper and observation schedule without native fallback.
Production `tsr_wasm` remains the tested artifact for browser and transpile
acceptance. The test-only compiler artifact is explicitly ineligible for
production performance or artifact-size acceptance.

The new compiler transport requires `TSR_WASM_COMPILER_ARTIFACT_DIR` and is
selected by `tools/phase7/portable/wasm-consumer --compiler`; single-file
transpile still requires `TSR_WASM_ARTIFACT_DIR`. The separate build command is
`python3 tools/phase7/portable/wasm-compiler/build.py --wasm-bindgen CLI --output DIR`,
where DIR must be new and outside every `target` ancestor. The build retains
wasm, generated bindings, source hashes, import identities and pinned flags.
It does not invoke a native compiler consumer or infer an artifact directory.
Future builds also retain the exact Node transport and executable shim beside
wasm; the accepted development build below had those matching assets saved
separately without modifying its original build record.

The initial test-only artifact builds successfully with wasm-bindgen 0.2.126,
Rust's pinned 16 MiB linear stack and Node's `--stack-size=4096`. Its wasm
SHA-256 is
`f7eb864789716c91bfdbdb3a7f78a66d2b2e3018203d26daef0c2d933510de0b`
(21,826,439 bytes), binding SHA-256
`0823477234495b31979e57d3e94627b1ef9d551fa14ba77b5e5c4fce167f4e98`.
Exactly four imports are present: the synchronous acceptance callback,
wasm-bindgen throw/debug-string helpers and externref initialization. There
are no host filesystem or thread imports. The artifact, bindings, transport,
shim, source/build metadata and detached replay notes are retained at
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r3-test-compiler-wasm`.
Every runtime asset hash matches the build source/artifact record; the separate
`acceptance.json` binds both result captures and the exact selected roster.

All **18 selected compiler variants / 159 subtests pass**, zero failures or
skips. The disjoint five- and thirteen-variant captures are
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r3-compiler-wasm-focused-initial`
and
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r3-compiler-wasm-focused-remaining`.
They cover query/assertion observations, JS checking, maps and declarations,
malformed configuration, incremental/build-info output, symlink resolution
traces, mapper transforms/supplemental sources/typed errors and suggestions.
Both commands exit zero. The real invalid-version child control emits the
exact failure frame and exits 1; its stdout/stderr are retained beside the
artifact. Native crate tests pass (9), native all-target clippy passes with
warnings denied, transport tests pass (6), and the shared public-dependency
boundary guard passes (5). Wasm compilation retains six existing compiler
preload dead-code warnings; these are not test failures or hidden warnings.
The subsequent full `compiler-wasm` run exits zero: **13,432 variants,
119,615 passes, zero failures and eight existing output skips**. Its results
match the full native embedding counts. The durable capture is
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r3-compiler-wasm-full`;
its test-only artifact is distinct from production wasm. The exact
`status/parity/compiler-wasm.json` expectation was accepted and checked by the
parent integration task; both commands exit zero.

Remaining R3 obligations include broader deep input and trap replay and the
separate production artifact-size, elapsed and memory budgets.

## Production budget foundation (correctness only)

The existing S10 measurement is not a full compiler budget route:
`scripts/s10_go.py` builds a parser-only `GOOS=js` authority, and
`scripts/s10_measure.py --kind portable` runs correctness checks rather than
compiler elapsed/RSS measurements. The Phase 7 CLI benchmark uses ordinary
executables at 2/4/8 checkers and has no retained-memory checkpoint. Those
measurements do not certify one-checker wasm or embedding budgets.

The new foundation under `tools/phase7/budget` calls real config parsing,
loading, checking and optional in-memory emit from frozen physical file,
directory and symlink metadata. Native Rust uses public `tsr_embed`; native
and wasm Go share one production program operation. Production wasm uses
`tsr_wasm::MemoryHost/PreparedHost/WasmSession`. The acceptance pre/post
schedule, baseline walker and test-only compiler wasm artifact are excluded.

Production wasm now exposes composable snapshot preparation, real configured
program loading, retained checking, retained emit and owned observations.
Checking uses the same compiler pool as emit: config, program, syntactic,
global and per-file semantic diagnostics, plus declarations when enabled,
followed by production sort/deduplication. Tests verify checker identity across
check/emit, owned result lifetime and error behavior. All 17 `tsr_wasm` tests
and all-target clippy with warnings denied pass. Its original convenience
methods retain their output representation. MemoryHost also forwards explicit
unavailable-file metadata; a requested missing payload fails instead of
pretending that the file does not exist.

A source-guarded production release build completed successfully. The exact
runtime module (26,268,600 bytes, SHA-256
`ee3e8b242e67d1e2d4def674aeccd9908f097d9df045cedbd0fc2137ab5a1afb`),
bindings (`242fd7e31deaead398dbb53ab356e361d9597fb64bb08a27081e9ee063e57d05`)
and original build record are retained once under
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r3-budget-production-wasm-tiny-01/production-assets`.
The build includes the VFS parent index and the previously checked narrow
printer/checker refusals. Later source edits are not represented by these
immutable bytes.

Actual Go wasm versus production Rust wasm tiny comparisons pass for both v1
and streamed v2 preparation. Both load exactly 83 sources, use one checker,
produce zero diagnostics and one identical emitted write. Source identities,
loaded texts, diagnostic values, options and outputs match; type creation
counts remain separate (Go 30,928; Rust 30,942). The checkpoint occurs before
owned observation serialization. Version failure is terminal, and the real
Node child transport also passes. Captures are
`r3-budget-production-wasm-tiny-01` and
`r3-budget-production-wasm-streamed-tiny-02` beneath the durable dated record
root; the second reuses the verified retained production assets rather than
copying another module. Focused simulated-export controls separately test
stage ordering, content hashes/offsets/duplicates and terminal traps. They do
not claim a newly observed production panic.

Version 2 streams complete metadata and bounded base64 content frames,
verifies canonical identities, counts, contiguous offsets and per-file raw
SHA-256, and drops decoded transport/assembly roots before execution. Known
physical files without supplied content remain explicit unavailable
placeholders. Native discovery is an untimed acquisition route over frozen
metadata and logs actual production reads, repeats and misses; only that route
may acquire canonical raw files from the recorded OS scope. Its controls reject
stale size, disappeared physical files, overwritten captures and target paths.
The Go prototype passes twelve focused body tests and native/wasm builds. The
controller verifies both read closures before freezing their union.

The retained Node checkpoint labels its own authority and reports wasm linear
memory, process RSS, V8 heap, external bytes and array buffers separately. Go
reports GC/ReadMemStats with retained roots kept alive. Native Rust allocator
instrumentation remains a separate artifact. Arena storage counters are not a
byte denominator, and none of these distinct observations is relabeled as an
equivalent retained allocation measurement.

The actual streamed production wasm Xstate check comparison also passes:
1,536 loaded sources, zero diagnostics and equal roots/options/texts/counts;
type counts remain separate. Capture: `r3-budget-production-wasm-xstate-check-01`.
All nine remaining Go native discovery routes complete, with their ordered
read events retained in `r3-budget-go-discovery-scenarios-01`.

Production wasm depth probes compare actual complete Go/Rust check-and-emit
observations. Left addition and right exponentiation at 5,000 operators,
JSX at 600 levels, conditional types at 600 and nested statements at 1,600
all pass. **5,000 parentheses fail during Rust parsing** with
`RangeError: Maximum call stack size exceeded`; Go completes. Verified wasm
name mapping locates repeated `parse_primary_expression` → left-hand-side →
update/unary → assignment → `parse_expression` frames. Runtime noncustom
sections match the named mapping artifact exactly. A separate 500-parenthesis
control passes with equal observations. These are bounded results, not a
claim of unlimited recursion. Captures `r3-production-wasm-depth-01`,
`r3-production-wasm-depth-remaining-02` and
`r3-production-wasm-parentheses-500-control-01` preserve successes, the initial
stalled failed child, terminal-I/O correction and the symbolicated failure.

A subsequent guarded production build adds bounded owned row APIs. Its exact
runtime is 26,276,385 bytes, SHA-256
`849ca393f51a4b063f9566aa055abb17f2fe3945b196b814909d1bdc14f2bf78`,
retained with bindings/build metadata under
`r3-budget-production-paged-wasm-assets-02`. A subsequent `cargo fmt -p tsr_wasm`
changed only test assertion formatting in `checker.rs`; these retained bytes
were not rebuilt solely for formatting. The header caches source identities
and diagnostic metadata; each source/write request copies only that row's bytes.
Native equivalence verifies assembled rows match the complete snapshot and
checks invalidation after another check/emit. Actual Go/production-wasm paged
tiny replay passes with unchanged helper hashes at
`r3-budget-production-paged-wasm-tiny-03`; the subsequent paged Xstate check
also passes at `r3-budget-production-paged-wasm-xstate-check-02`. Both Node
adapters now collect V8
after successful preparation and at checkpoint, outside execute; Go also
collects its runtime after prepare and at checkpoint. Modern checkpoints report
a distinct pre-observation process peak with its metric authority. These
foundation controls are not performance acceptance.

The final production browser rerun uses those exact retained assets and passes
all six required smoke cases in Chromium 151.0.7922.34, Firefox 153.0 and
WebKit 26.5. This rerun covers the current compiler-pool diagnostics path,
parse/check/emit, both transpile modes, bounded deep input and the real
invalid-filename trap followed by invalidation and fresh-instance recovery.
Receipt `r3-browser-production-paged-final-01/browser-smoke.json` retains the
served asset bytes and manifest outside target. No rebuild was needed.

The production Go wasm VSCode check reaches a capacity failure during
`execute`, after successful preparation. Its runtime reports:
`out of memory: cannot allocate 524288-byte block (4203118592 in use)`.
The host rejects the preserved non-JSON fatal line; no checkpoint, observation
or ratio is produced. Capture: `r3-budget-production-paged-wasm-vscode-check-01`.
Independent Rust wasm VSCode check also fails during execute. Verified name
mapping identifies `rust_oom` → allocation error handler → `RawVec::handle_error`
→ `TypeStore::new_type` → object-type instantiation, rather than a compiler
assertion. Its stderr, matching wasm hashes and symbolicated frames are retained
under `r3-budget-production-paged-wasm-scenarios-01/vscode-check`. Neither
VSCode check authority reaches checkpoint; no ratio is available.

All ten production wasm scenario/mode replays are complete. The exact paired
observations pass for Xstate, self-compiler and Bluesky in both check and emit
modes. Rust wasm independently passes MUI docs check and emit; Go wasm fails
during execute with explicit runtime out-of-memory lines at roughly 4.2 GB in
use. VSCode emit also fails during execute on both authorities: Go reports
out of memory and Rust terminates with `RuntimeError: unreachable`. The Rust
check failure above is symbolicated; the emit failure is retained without
claiming an independently mapped cause. Both VSCode modes have no checkpoint
or complete observations on either authority.

| Scenario | Check | Emit |
| --- | --- | --- |
| Xstate | Paired exact pass | Paired exact pass |
| Self-compiler | Paired exact pass | Paired exact pass |
| Bluesky | Paired exact pass | Paired exact pass |
| MUI docs | Rust pass; Go OOM | Rust pass; Go OOM |
| VSCode | Go OOM; Rust allocator failure | Go OOM; Rust unreachable |

The completed queue receipt is
`r3-budget-production-paged-wasm-scenarios-01`, with unchanged helper hashes
and per-authority raw responses/stderr. Its exit status is 1 because required
cases fail. One authority's failure cannot become a paired pass or silently
remove a required scenario. No memory limit or input was retuned to avoid
these failures. The Go Node launcher now validates the reviewed build record
combination `phase7-budget`, measurement contract 2, source guard true,
build exit 0 and performance eligibility true. Historical prototype records
remain readable only with performance eligibility false. Three focused Node
controls cover the two accepted modes, missing/malformed gates, GC/accounting
stages and terminal failure; accepting build metadata alone does not establish
performance acceptance.

Remaining work before production budget acceptance:

- Complete native production comparisons and resolve or explicitly report the
  failed production wasm capacity cases. All ten wasm modes have concrete
  outcomes above; only six have paired complete work. The full compiler-wasm
  acceptance run never supplies budget measurements.
- Verify scenario options, references, diagnostics and emit gates. Unsupported
  incremental/watch/external mapper execution must fail explicitly.
- Validate bounded observation transport on large scenarios. The tiny
  observation is about 10.5 MB because it includes loaded builtin texts;
  VSCode's raw read closure is about 125 MB. Version-2 row commands and
  production source/write APIs now avoid whole-observation JSON; an
  exceptionally large individual source/write still has its own frame bound.
- Validate ordinary versus instrumented work, allocation authorities and
  retained roots. Attribute filesystem layout and transport costs; do not
  present a whole-operation comparison as isolated compiler CPU.
- Reuse the serial alternating warmup/pair/stability rules and negative
  controls only after equal work is established. `wait4` peak RSS and live
  checkpoint RSS are different quantities. Keep serialization, preparation,
  process overhead, wasm linear memory and allocator facts separate.
- Measure artifact size from equivalent full production capabilities, with
  glue/runtime bytes separately declared. The test-only compiler acceptance
  artifact is ineligible for size, elapsed or memory acceptance.

No production performance budget has been measured or accepted by this
foundation. Current captures are correctness evidence only.
