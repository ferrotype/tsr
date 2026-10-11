# Phase 7 R3: WebAssembly correctness progress

Status: **in progress**, 2026-10-11. Single-file transpile now crosses the
production wasm boundary. Full compiler-harness acceptance, comprehensive
deep-input stress and the ADR 0020 budgets remain incomplete.

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
This bounded roster is not the full 13,432-variant compiler suite.

Remaining R3 obligations include full `compiler-wasm` execution and exact
expectations (including pre/post schedule, resolution traces, mapped outputs
and additional assertions), broader deep input and trap replay, and the
separate production artifact-size, elapsed and memory budgets. Implementing
the adapter alone does not establish these results.

## Budget route assessment (implementation pending)

The existing S10 measurement is not a full compiler budget route:
`scripts/s10_go.py` builds a parser-only `GOOS=js` authority, and
`scripts/s10_measure.py --kind portable` runs correctness checks rather than
compiler elapsed/RSS measurements. The existing Phase 7 CLI benchmark uses
ordinary executables at 2/4/8 checkers and has no retained-memory checkpoint;
its measurements do not certify one-checker wasm or embedding budgets.

Reuse the five committed scenario descriptors, provisioned dependency closure
and frozen input/options/diagnostic/emit obligations. Reuse the serial
alternating warmup/pair/stability rules, host eligibility checks, omitted-work
and changed-output controls, and the child `wait4` peak-RSS reader. S10 can
supply pinned Rust/Go/Node/bindgen provisioning, exported instance factories,
Go's `wasm_exec` loader and artifact archival. These are shared mechanisms;
new compiler authorities and readers remain necessary.

The smallest equivalent operation is a fresh session from an explicit frozen
physical file/directory/symlink snapshot and identical resolved options/roots,
at one checker: load and check, optionally emit in memory, retain the same
positive session/diagnostic/output roots through a declared post-operation
checkpoint, then dispose. A separate native consumer must call public
`tsr_embed`; a thin pinned Go consumer must call the real compiler program,
diagnostic and emit methods. Its native and `GOOS=js` front ends can share
that operation. Production wasm must call `tsr_wasm::MemoryHost/WasmSession`.
The acceptance pre/post schedule, baseline walker and test-only compiler wasm
artifact are excluded from this route.

Concrete gaps to resolve before measurement:

- `WasmSession::diagnostics` currently checks through `Session::operation`,
  while emit uses a separate retained compiler pool. The public native
  `Session::diagnostics` phase API now uses the pool retained by emit. Review
  moving production wasm diagnostics to that route while preserving output;
  comparing two initialized Rust checker states with Go's one program pool
  would measure different work.
- Production wasm needs owned loaded-file/source identities and checked
  type-count facts to reject omitted work. Empty diagnostics alone do not
  prove the expected closure was checked. Preparing resolved options and
  roots must also preserve configuration/project-reference behavior; the
  current host takes only raw compiler options and roots. Verify equivalence
  for every scenario or extend production values explicitly.
- Arena `Counters` count owners/storage objects, not requested/live bytes.
  Separate allocator observation from ordinary timing. The existing native
  `cap::Cap` stats wrapper is a possible observation mechanism, validated
  against ordinary-consumer work. Go needs explicit `ReadMemStats`, GC and
  `KeepAlive` checkpoints with the same positive live roots. Allocation
  traffic is separate from retained bytes.
- Declare Rust, Go and V8 result retention/disposal/GC checkpoints. Add live
  process RSS sampling: `wait4` supplies peak at exit, not retained RSS while
  the session/results remain alive. Report wasm linear memory and allocator
  counters beside process RSS; linear memory is not its substitute.
- Build size authorities with equivalent full production parse/check/emit
  capabilities, without CLI/watch or corpus adapters, and declare glue/runtime
  bytes separately. The test-only compiler artifact must be rejected by
  budget readers. Its passing correctness rows cannot satisfy any size,
  elapsed or memory gate.

No new budget builds or measurements were started during this assessment.
