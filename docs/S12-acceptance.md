# Full compiler acceptance and Phase 7 budgets

Status: proposal for owner review with ADR 0020; not yet approved. This matrix
turns PLAN sections 5 and 9 into named acceptance obligations. It does not claim
that Phase 0 executed full checking, emit or server workloads.

## Correctness and release matrix

| Entry point / surface | Required workload and observations | Acceptance |
| --- | --- | --- |
| Native compiler | All pinned compiler/conformance cases, including source generics, JSX, decorators and type forms excluded by the Phase 0 selection | Exact applicable diagnostics, types and symbols, modulo explicit owner-approved divergences |
| Native emit and transpile | Complete pinned emit/transpile corpus, all applicable targets/options and declaration/isolated-declaration cases | Exact JavaScript, source maps and declaration output; exact diagnostics and exit behavior |
| Native program services | Complete config, CLI, incremental build, watch and project baselines | Exact outputs, dependency/update behavior and diagnostics; no unsupported required operations |
| Editor / API | Pinned semantic fourslash, LSP/project/replay suites and unchanged synchronous/asynchronous JS API clients | At least 99.5% semantic fourslash with the rest triaged; required LSP/project/API suites pass |
| Bare WebAssembly | Full compiler/conformance/transpile comparisons through an in-memory host | Same applicable diagnostics/type/symbol/emit baseline comparisons as native; no native process/filesystem requirement |
| External Rust consumer | Independent manifest, public embedding API and consumer-supplied host; same full compiler/conformance/transpile work | Same baseline comparisons as native; documented public operations and callbacks all exercised |
| Public lifetime/failure contracts | E3 cases through actual API/service entry points, malformed inputs, cancellation, reentry, retained results, repeated create/query/drop and panic/trap invalidation | Correct surviving outputs and rejection of invalid identities; owner/storage counters return to baseline after final roots drop; applicable release, Miri and ASan checks |
| Browser host | Parse, check, emit and deep-input smoke in each supported browser engine | Correct outputs and declared failure behavior; stack/worker configuration recorded and validated rather than inferred from Node |
| Native release targets | macOS ARM64/x86-64 and Linux glibc ARM64/x86-64 | Release artifacts and suites on all four; final Linux ELF imports no symbols above glibc 2.28 and executes on that floor |

The temporary pause of macOS Intel and Ubuntu ARM CI is not removal from the
release matrix. Restore the required target coverage before Phase 7 acceptance.
Similarly, Node's current `--stack-size=4096` is a measured host requirement,
not evidence that browser defaults pass deep-input tests.

Phase 0 informational exclusions do not automatically exclude cases from the
full matrix. At the release pin, list native test-selection guards and genuinely
inapplicable host operations explicitly; keep divergences in the approved
ledger. A Rust Unsupported result is never a matching baseline.

## Workload and measurement contracts

Use the TypeScript-benchmarking scenarios named in PLAN: **vscode,
self-compiler, mui-docs, xstate and bluesky**. Before implementation tuning,
freeze each scenario's source revision, dependency/library closure, compiler
options, requested output modes and oracle work digest. The existing VS Code
parse/bind inventory is a separate workload and cannot stand in for full checks.

Measure cold full checking and checking plus emit separately. Native runs use
2, 4 and 8 checkers; report every scenario/mode independently. The portable
WASM and single-session consumer comparisons use one checker with equivalent
input and output work, also reported independently. No weighted aggregate may
hide a required failing scenario.

Keep startup/module compilation separate from warm operation latency. Use fresh
processes for memory checkpoints and fixed identical live result roots. Report
peak RSS, requested allocation traffic and retained bytes separately; Go GC,
Rust disposal and result ownership must be declared. Do not compare wasm linear
memory directly with a native RSS or type-census denominator. The full WASM
process-memory comparison runs both runtimes under the same pinned Node engine.

## Proposed numerical budgets

The native goals below are already in PLAN and remain unchanged. The portable
and embedding budgets are proposed release targets, not fitted conclusions from
the Phase 0 parser data. The owner must approve them before accepting ADR 0020.

| Deliverable and named workload | Budget | Authority / qualification |
| --- | --- | --- |
| Native full checking and checking+emit, each of the five scenarios at 2/4/8 checkers | Rust elapsed / Go elapsed ≤1.0; Rust peak RSS / Go peak RSS ≤0.70 | Existing PLAN cut-over goals; Phase 0's relaxed parse/bind gates do not replace them |
| Full WASM compiler artifact | Rust bytes / equivalent full Go `GOOS=js` artifact bytes ≤1.0 | Proposed; compare shipped uncompressed modules, declare glue separately, exclude corpus adapters from both |
| Full WASM checking and checking+emit, each scenario at one checker in Node | Rust elapsed / Go `GOOS=js` elapsed ≤1.0 | Proposed; loaded inputs, equivalent work, identical engine and declared stack configuration |
| Full WASM memory on those operations | Peak process RSS and retained process RSS each ≤1.0 times the corresponding Go-in-Node value | Proposed; same process/engine boundary and live results; report linear memory and allocator counters separately |
| External Rust consumer full checking and checking+emit, each scenario at one checker | Elapsed / equivalent one-checker Go native elapsed ≤1.0; peak RSS / Go ≤0.70 | Proposed application of native goals to the public API; no CLI/IPC cost credited to the consumer |
| External Rust consumer retained checker/results on those operations | Rust retained bytes / Go retained bytes ≤1.0 | Proposed; positive comparable baseline and the same declared live roots; stricter than the historical checker-slice retained ratio 1.413 |
| Linked standalone Rust consumer artifact | Bytes / equivalent stripped Go benchmark-consumer executable bytes ≤1.0 | Proposed; executable plus its required runtime/library closure, not a static archive containing unused code |
| Public session teardown across repeated operation cycles | Zero live-owner and tracked-storage delta after final roots drop | Existing ownership requirement; process RSS is reported and need not immediately fall to baseline |

The E7/E8 parser-only budgets remain separate and will use whatever explicit
Phase 0 limits the owner adopts. They do not substitute for any full-compiler
row above. Cold startup is reported alongside the operation budgets; no unmeasured
startup claim follows from warm parser throughput.

All required release budgets and correctness checks must hold for four
consecutive weekly runs, on frozen comparable workloads, alongside PLAN's
four-week dogfood requirement without an open P1 crash. A later budget change
requires a separate recorded owner decision and new qualifying evidence.

## Risks that remain after a go decision

The historical checker slice takes about 2.125 times Go's checker interval and
retains about 1.413 times its measured bytes. Better parse/bind memory and the
0.812 per-type footprint do not offset those results. Full-checker profiling and
optimization must establish a route to the native release targets after parity.

Complete transforms/emit and actual project/LSP/API integration are still
substantial implementation work. The Phase 0 gate establishes that the tested
architecture can proceed; it does not certify release readiness or make the
four-week acceptance period optional.
