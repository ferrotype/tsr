# Phase 7 R5: hardening

Status: **in progress**, 2026-10-11. This record covers the first bounded
compiler CLI fuzz route, bounded scanner/parser routes, runner controls, a saved-baseline smoke and a
lexical refusal inventory. It does not complete [R5](PHASE7-plan.md#r5--hardening).
The owner approved pprof compatibility out of scope on 2026-10-11;
[ADR 0024](adr/0024-profiling-compatibility-out-of-scope.md) records the decision
and [the native profiling guide](PERF-profiling-2026-10.md#method) supplies
the development workflow. The existing CLI/API/LSP refusals are preserved.

## Implemented route and failure controls

The named `--target scanner` route now reuses the rebuildable
[S05 Go adapter](../scripts/s05_oracle),
[Rust example](../crates/tsr_scanner/examples/s05.rs), and existing Python
request/stream validators through
[`fuzz/protocol.py`](../tools/phase7/fuzz/protocol.py). It invokes each explicitly
supplied adapter in a fresh process/directory, sends identical saved NDJSON
bytes, and compares the complete canonical ordered token and diagnostic stream.
Its fixed schedule is `scan_all`, target 99, variant 0, skip trivia, without
source decoding. Generated inputs are capped at 4 KiB; explicit source replay
above that bound is refused. Per-channel regular-file/capture limits use
`RLIMIT_FSIZE` at 16 MiB plus an overflow sentinel; overflow fails and retained
artifacts explicitly contain only the prefix. Every scanner panic is a finding,
including matching panics; missing EOF/end, reordered or trailing records cannot
pass. Existing wall/group cleanup and seed/budget/minimization apply. Archives
include request bytes, target and limits; exact manifested replay supports the
scanner target without ordinary-compiler fallback.

Seven additional fake-child scanner controls pass locally (4.4 seconds), covering
fixed options/invalid UTF-8 preservation, token/diagnostic differences, incomplete
and trailing streams, equal panics, timeout/capture limits, manifest replay and
tampering, and oversized explicit input refusal.

The parser route reuses
[`crates/tsr_parser/examples/s06.rs`](../crates/tsr_parser/examples/s06.rs) and
[`scripts/s06_protocol.py`](../scripts/s06_protocol.py), with a new narrow
[Go wrapper](../tools/phase7/fuzz/parser_oracle/main.go). The wrapper observes
the pinned parser and encoder: source metadata and hash, parse/JS/JSDoc
diagnostics, both node-index traversals and lookup/cache identities, and every
encoded byte. The `--fuzz-related` adapter mode additionally reports recursive
related diagnostics, including file names, spans, message keys, text and
arguments. It implements only the named TS parse/index-before/encode/index-after
schedule. Its [builder](../tools/phase7/fuzz/build_parser.py) exports and checks
the current pin without editing `upstream/`; the old S06 builder's absent
factory/codec adapters are not restored or needed.

Current-build correctness smokes each completed **100 pairs, zero findings**,
seed 42, 60-second total work budget and 3-second per-pair allowance. Both exited
0 at the iteration limit. They used the current Rust S05/S06 examples and
freshly built pinned Go adapters, with no compiler fallback. The retained
directories outside Cargo output are:

- `../ts-rust-benchmarks/2026-10-11/r5-scanner-current-smoke-01/`
- `../ts-rust-benchmarks/2026-10-11/r5-parser-current-smoke-01/`

`run.json` records executable hashes, the corpus, limits and completed inventory;
`cases.ndjson` retains the complete paired observations. The saved scanner pair
and Rust parser are in `r5-protocol-current-build/`; the pinned parser executable
and its build record are in `r5-parser-current-build/`, under the same dated
archive root. Those historical parser observations omitted related diagnostics.
The corrected adapter mode is checked separately; it does not add coverage
retroactively to that run. These are bounded smoke results, not a full-corpus or enforced
macOS-memory-limit claim. The [commands](../tools/phase7/fuzz/README.md) rebuild
both routes from source.

[`tools/phase7/fuzz/run.py`](../tools/phase7/fuzz/run.py) invokes explicitly
supplied saved `tsgo` and `tsrust` executables. It neither builds nor supplies
missing results. Alternating deterministic small-program generation and
byte/token mutation of three initial `.ts` corpus files exercise whole-program
parse/check/emit through CLI processes. Every pair receives identical source,
minimal globals, tsconfig and CLI arguments in fresh isolated directories.
This first input model has no ambient packages or standard libraries; it does
not claim scanner/parser-specific observations, broad module coverage or
coverage-guided mutation.

Complete stdout and stderr bytes are compared after case-root and CRLF
normalization. New CLI runs explicitly select `--pretty true` so the pinned
writer includes related diagnostics; ANSI formatting and source excerpts are
compared too. The historical CLI smoke below used `--pretty false` and did not
observe related diagnostics. Message bodies and order remain observable.
All emitted paths and bytes must match exactly. Exit statuses are compared;
crashes and timeouts are findings even if both compilers suffer them. The total
per-case allowance is split equally across the two children; the run reserves
a complete pair before starting it. Process groups are killed on completion,
including descendants. Linux enforces per-process address-space limits through
`RLIMIT_AS`; this is not aggregate RSS enforcement. macOS memory enforcement
is explicitly unsupported/informational. The smoke below therefore does not
witness an enforced memory cap. Launch and resource-limit setup failures are
runner errors (exit 2), retained separately from compiler findings and never
minimized. A runner failure during reduction preserves the original finding.

Findings retain original and minimized files/options, complete normalized
observations, raw output, emitted bytes and executable identities outside
`target`. Chunk-deletion minimization retains the original failure categories
and exit/timeout statuses after executing both children, subject to a trial
limit and the shared budget. The original is saved first. No automatic parity
acceptance, approval or fixture promotion occurs. A zero-completed-pair run
exits nonzero. [Usage and replay](../tools/phase7/fuzz/README.md) describe the
options and the timing/artifact overhead excluded from the work budget.

The 22 CLI/archive [fake-child controls](../scripts/tests/test_phase7_fuzz.py) pass
locally. They exercise missing outputs, changed emit bytes,
complete diagnostic/stderr differences, equivalent crashes, timeout cleanup
with a descendant heartbeat, a minimizer retaining a real reproducing trigger,
saved raw artifacts and identities, seed reproducibility, and insufficient
budget refusing a partial pair. Archive replay controls also prove integrity
refusal for tampered manifests, inputs, emits, raw outputs and executables;
recorded crash/timeout reproduction; resolved and changed failures; visible
alternating outcomes; exact archived options despite changed generator defaults;
explicit candidate Rust identities with pinned Go verification; preservation
of original archives and first-child output when the second cannot launch.
A control also proves that identical finding categories with changed common
diagnostics remain an execution failure; replay reports the changed fields.
Ordinary fake Python children have a separate bounded startup allowance from
the deliberately short timeout controls, and archives assert their intended
initial failure category before testing replay.
They execute controlled children, not the real compilers. Run them with:

```sh
python3 -m unittest scripts.tests.test_phase7_fuzz
```

[`replay.py`](../tools/phase7/fuzz/replay.py) verifies each new case's creation-time
manifest and every archived file against the hashes anchored in `run.json`,
then reruns the exact archived inputs/options/arguments through the same
bounded child runner. An optional independent `run.json` hash supplies an
external integrity anchor. Default replay requires the saved executable pair;
`--candidate-rust PATH` explicitly substitutes a fixed Rust build while
verifying the saved Go executable and recording original/candidate Rust
identities separately. Candidate results never claim original-binary replay.
Every attempt and raw observation is preserved in a new directory outside
`target`. Exit/status 1 `reproduced`, 0 `resolved`, and 2 `execution-failure`
distinguish exact observed reproduction, agreement on these attempts only,
and changed failures or integrity/execution errors. Alternating outcomes are
retained individually; no nondeterminism is waived. See the [replay command
and status definitions](../tools/phase7/fuzz/README.md).

[`.github/workflows/phase7-fuzz.yml`](../.github/workflows/phase7-fuzz.yml) is an
opt-in `workflow_dispatch` matrix for scanner, parser and ordinary compiler CLI.
It verifies the upstream source pin and uses pinned toolchains. Each lane builds
its own explicit adapter/compiler pair and retains the exact binaries beside
the results for replay after Cargo output is cleaned. Its work budget is a choice
of 60, 120 or 300 seconds; individual pairs have 5 seconds and Linux children
have a 2 GiB address-space cap. Findings fail the job and artifacts upload on
failure. Local YAML, shell syntax and inline-Python checks pass. **The workflow
has not executed in GitHub.** It contains no performance measurement.

## Saved-baseline smoke and preserved finding

The saved pair came from the verified archive
`../ts-rust-benchmarks/2026-10-11/target/phase7/bin/`. SHA-256 identities match
both `manifest.jsonl` and the archived
`target/phase7/api/capture-review-fixed/capture.json`:

| Executable | SHA-256 |
| --- | --- |
| `tsgo` | `57ab2bcc2c59862d56fe53a3c87240f22bcd692316e7d261a012f736ae5f4d87` |
| `tsrust` | `5abc9fb5b89a3276335c49cffecd7a1413540462ea0442faeb8c85175f8ad247` |

That capture names clean revision
`725aa04b302e2712cadaf8b56020c87f6fa0df3e` and upstream pin
`1f70213d4922b434345f639b441681e470c7cfc1`. This is a historical saved compiler
pair, **not a correctness claim about the current branch's executable**.

Reproduction from the repository root, choosing a new output directory:

```sh
python3 tools/phase7/fuzz/run.py \
  --rust ../ts-rust-benchmarks/2026-10-11/target/phase7/bin/tsrust \
  --go ../ts-rust-benchmarks/2026-10-11/target/phase7/bin/tsgo \
  --output ../ts-rust-benchmarks/2026-10-11/NEW_R5_RUN \
  --seed 42 --iterations 1000 --budget-seconds 120 --case-seconds 4 \
  --memory-mb 1024 --minimize-trials 8 --max-failures 100
```

The retained run `../ts-rust-benchmarks/2026-10-11/r5-fuzz-smoke-seed42/`
completed 1,000 pairs in 27.8 seconds, stopped at the iteration limit and
returned exit 1 for one finding, index 523. Its eight minimizer trials retained
the same diagnostic difference. Original/minimized inputs, outputs and emits
are under `failure-000523/`; `run.json` and `cases.ndjson` retain the inventory.
The differing TS1435 suggestion for `constructorundefinedpublic` was:

- Go: `constructor undefinedpublic`
- Rust: `const ructorundefinedpublic`

Exit statuses and emitted bytes matched. Thirty fresh-process replays of the
same minimized source then observed Go's `const` choice 17 times and its
`constructor` choice 13 times. Every replay is retained under
`../ts-rust-benchmarks/2026-10-11/r5-fuzz-keyword-replays-seed42/`, including
`summary.json`. Rust consistently chose the lexical `const` prefix.
These historical smoke/replay archives predate creation-time case manifests.
The new verified replay command refuses them; they have not been rewritten
or retroactively certified. The historical source replays above remain
observations of that saved pair.

This is the already documented upstream map-order variation, not evidence
requiring a Rust algorithm change: pinned
`upstream/tsc/internal/scanner/scanner.go:GetViableKeywordSuggestions` iterates
a map, and `parser.go:getSpaceSuggestion` selects the first matching prefix.
Rust's `crates/tsr_parser/src/diagnostics.rs:get_space_suggestion` documents its
lexical choice. Existing [S06 diagnostic witnesses](S06-implementation-plan.md)
include `constructorabcdefghij x;` and `typeofabcdefghijklmno x;`, generated by
`scripts/s06_fixtures.py` and checked by `scripts/s06_diagnostics.py`.
**The exact CLI fuzzer still reports and retains this mismatch.** No diagnostic
normalization, baseline, expectation or approval was changed to waive it.

## Static refusal inventory

[`tools/phase7/fuzz/audit.py`](../tools/phase7/fuzz/audit.py) writes a sorted,
reproducible lexical inventory of `Unsupported` tokens and `not available`
phrases in regular `crates/**/*.rs` files. Each entry retains file, one-based
line, complete source line and matching text; the inventory includes its source
fingerprint. It includes comments, tests and generated code. Other wording,
multiline phrases and source outside that scope are not enumerated.

```sh
python3 tools/phase7/fuzz/audit.py \
  --output ../ts-rust-benchmarks/2026-10-11/NEW_R5_REFUSAL_INVENTORY.json
```

The saved inventory under
`../ts-rust-benchmarks/2026-10-11/r5-unsupported-lexical-inventory-02.json`
contains 216 matched lines in a 1,359-file source scope and matches an
independent `rg` inventory. Its own source fingerprint identifies that snapshot;
regenerate to review current source. These counts are review inventory only.
They do not classify reachability, functionality, intentional limits, owner
approvals or completion. The [semantic review](../tools/phase7/fuzz/refusals.md)
now separates tests/declarations, handled retry signals, contract guards,
approved scope and open implementation/invariant work. A fresh review input
has 217 lines; its only addition over the saved 216 is an injected transform
test failure. Five subsequent narrow fixes have native witnesses and focused
Rust regressions:

- Nullable printer name helpers now construct the pin's node-generated name;
  printing its absent original still panics. This is a direct factory witness.
- Alternate-resolution failure emits the exact TS2307/TS6278 chain and
  repopulate data. This uses supplied unresolved host state; ordinary resolver
  reachability remains unproven.
- A computed unique-symbol initializer displays
  `(typeof Symbols)[typeof key]`; transferred syntax still prints after its
  checker and program drop.
- Circular auto-accessor annotations return `any` and add fileless TS2502 only
  after semantic checking. Reading globals before checking cannot witness that
  late diagnostic.
- A referenced output importer sorts candidates from its source directory,
  while computing specifiers with the output's own imports and options. Native
  and Rust both select `() => import("./a").Foo` from `b.d.ts`.

Frozen native inputs/results and producer locations are linked in the
[semantic review](../tools/phase7/fuzz/refusals.md). The refreshed lexical
inventory is `r5-refusal-after-projectref.json` beside the earlier captures,
with 210 matching lines. The nonidentifier tuple-label refusal was reclassified:
reviewed producers exclude destructured parameter labels; directly supplying
one panics at the pin. Native ordinary inference controls and a focused Rust
regression are retained. The extension task-queue guard remains open for routes
other than prevalidated roots and path references; recursive export maps,
deferred-property storage and unsupported type-family invariants also remain
open. None of these findings creates an approval or closes R5. A smaller lexical
count would not itself mean improvement.

## Existing witnesses and smallest remaining work

### Full native depth checks

The public embedding consumer and the pinned Go consumer now execute the same
config/load/check/full-emit operation on six generated deep-input shapes. Every pair
completed and matched loaded sources, ordered diagnostics, emitted files and
source maps. Type-allocation counts are recorded separately, not required to
match. This is correctness evidence, not a timing comparison.

| Input | Explicit depth | Result |
| --- | --- | --- |
| Left-associated addition | 5,000 and 100,000 | Exact; no diagnostics |
| Right-associated exponentiation | 5,000 and 100,000 | Exact; no diagnostics |
| Parenthesized expression | 5,000 | Exact; no diagnostics |
| JSX elements | 600 | Exact on both the diagnostic-heavy and valid controls |
| Nested conditional types | 600 | Exact; no diagnostics |
| Nested statements | 1,600 | Exact; no diagnostics |

The first JSX input declared its JSX namespace inside a module and produced
1,200 diagnostics on each side. Its archive is preserved. The corrected global
namespace control also matches and emits successfully without diagnostics.
Each operation loaded 94 source files, including builtin libraries, and emitted
JavaScript/JSX plus its source map. The Rust consumer runs on the production
256 MiB reserved native worker stack (`workgroup::RESERVED_STACK`); its parser
and existing growth guards retain their production settings. Go uses its
ordinary growing goroutine stack. These are tested bounds, not unlimited-depth
or browser-stack claims.

The exact inputs, immutable executable identities, raw responses and comparisons
are in `../ts-rust-benchmarks/2026-10-11/r5-native-depth-01/`,
`../ts-rust-benchmarks/2026-10-11/r5-native-depth-100k/`, and
`../ts-rust-benchmarks/2026-10-11/r5-native-depth-jsx-valid/`. The committed
generator is [`deep_cases.py`](../tools/phase7/budget/deep_cases.py); the paired
controller is [`probe.py`](../tools/phase7/budget/probe.py). For example:

```sh
python3 tools/phase7/budget/deep_cases.py jsx-600 --output /absolute/NEW-input.json
python3 tools/phase7/budget/probe.py \
  --go /absolute/saved/budget-go --rust /absolute/saved/phase7_rust_budget \
  --input /absolute/NEW-input.json --output /absolute/NEW-comparison --timeout 180
```

The consumer build commands live in their respective
[Go](../tools/phase7/budget/go/README.md) and
[Rust](../tools/phase7/budget/rust/README.md) READMEs. No generated binary or
capture is required under `target/` to reproduce the probes.

The following pointers were inspected; their expensive producers were **not
rerun** for this record. Historical receipts retain their original scope and
source and applicability limits.

| Area | Existing witness | Missing R5 work |
| --- | --- | --- |
| Native parser/binder depth | `crates/tsr_parser/src/recursion_tests.rs:12` exercises twelve grammar shapes on an initial stack 64 KiB above the production red zone (192 KiB today) and observes real growth. `crates/tsr_binder/src/recursion_tests.rs:35`, `binary_trampoline_tests.rs:134` and `data/s07/binder-depth-cases.json` cover binary continuations and nested syntax. `crates/tsr_parser/src/worker.rs:16` reserves 256 MiB for native parser workers. | Full native check/emit results are recorded above; ADR 0011 now names the four site strategies. The parser growth control passes locally in both debug and release; Linux CI verification of the revised test remains separate. |
| Checker/printer growth and unwind | `crates/tsr_checker/src/node_builder_stack_tests.rs:55` observes grown-stack type display and factory-panic retirement. `crates/tsr_printer/src/tests.rs:145` exercises recursive printing, growth, unwind and final counters. | The end-to-end native check/emit comparisons above and the wasm results in R3 supplement these component witnesses; neither certifies untested grammar/depth combinations. |
| WebAssembly stack | [S10 historical results](S10-results.md) record two default-Node stress traps and completion with `--stack-size=4096`, separate from the 16 MiB wasm linear stack. | The [R3 record](PHASE7-R3.md) records actual full check/emit results for six deep families: five pass at the tested depths, while 5,000 parentheses exhaust the Rust parser stack. A 500-parenthesis control passes; the limit stays visible. |
| Watch storage plateau | `tools/phase4/tsctests/tests/ownership.rs:214` drives 50 real command-line watch cycles through a mock backend, retains early files, checks plateau after cycle two, releases superseded roots and asserts empty tracked counters after close. | The fifty-cycle real MUI docs result below establishes a tracked plateau and close-to-zero counters. RSS is reported separately. |
| Native watch descriptors | `crates/tsr_fswatch/src/linux/tests.rs:253` checks worker/descriptor release after shutdown; `:504` checks failed-worker descriptor retirement. [Phase 4 acceptance](PHASE4-acceptance.md) requires host-specific watcher receipts. | Measure native descriptor use and release on the owner's largest project on each applicable host; small shutdown fixtures do not establish scaling limits. |
| E3 ownership and instrumentation | [S09-5](S09-5.md) describes the historical, now-removed `scripts/s08_ownership.py`, exact S04/S06/S07/S09 inventories and its historical debug/release/Miri/ASan closure. [S10](S10-results.md) records six historical external-consumer lifetime cases. The current `tools/s10/lifetime-cases.json` names all sixteen public integration tests; `scripts/s10_lifetime.py` requires exact source/manifest agreement and executed-name agreement in all four modes. | The [R4 applicability table](PHASE7-R4.md#public-ownership-applicability) maps the public contracts. The fourteen-case receipt plus the two-case follow-up and mapped/supplemental service case pass all four modes. Counters measure tracked storage, not whole-process heap or RSS; the real watch result is below. |

The lifetime harness requires an explicit output outside `target`, refuses
existing capture directories, rejects filtered/ignored/missing tests and
returns nonzero for failed measured checks. Ten focused
[`test_s10_lifetime.py`](../scripts/tests/test_s10_lifetime.py) controls exercise
roster/output completeness, capture preservation and source-input scope.
Native lifetime inputs cover Rust code, manifests, bundled assets and the
shared consumer; unrelated browser scripts and documentation do not stale this
native instrumented result.

All **fourteen cases pass in debug, release, Miri and AddressSanitizer**. The
complete final capture and logs are retained at
`../ts-rust-benchmarks/2026-10-11/r4-lifetime-final/`; its capture SHA-256 is
`ed48d8ef6a2d98a76b181710cd74114fdcea36f95654e9af11d2fcb3d51400fc`.
The Miri tests took 391.34 seconds. This proves the named public ownership and
failure contracts; it does not measure whole-process heap or large-project
watch behavior. The earlier four-mode run is also retained unchanged under
`r4-lifetime-four-modes`; its overbroad freshness wrapper rejected unrelated
browser/documentation edits after every instrumentation mode had passed.
The final run uses the corrected native-only input scope. A subsequent
`RecursionProbe::default()` qualification in a separate feature-gated checker
unit test only resolves an all-target clippy lint and does not change the
lifetime executable.

Run the focused controls with
`python3 -m unittest scripts.tests.test_phase7_fuzz scripts.tests.test_s10_lifetime`.
A new instrumented capture, when the native ownership surface changes, uses
`python3 scripts/s10_lifetime.py capture --output /absolute/path/NEW_LIFETIME_CAPTURE`.

The remaining decisions are narrow and explicit:

1. [ADR 0011](adr/0011-deep-recursion--reserved-stacks-and-growth-guards.md)
   now records all four recursion-site strategies and their measured limits.
   Production wasm's 5,000-parenthesis parse trap remains open; the separate
   500-parenthesis check/emit control passes. Other deep shapes and browser
   bounds retain their own results, rather than inheriting the native limits.
2. The automated `checkchildren` replacement now runs through `rustc_driver`
   on the existing instrumentation nightly. It detected all eight planted
   omissions and passed repaired/type-identity/allowance/cache controls. The
   current checker run observes two workers and 104 dispatched methods, zero
   findings and six per-return reviewed limitations; its log is
   `../ts-rust-benchmarks/2026-10-11/r5-checkchildren-reviewed.log`.
   [The tool README](../tools/phase7/checkchildren/README.md) names the scope,
   blind spots and reproduction. CI now has a dedicated job; local execution
   does not claim a completed CI run. ADR 0016 records this tooling choice.
3. Carry R1's measured type/symbol storage and interning decisions into ADRs
   (PLAN items 6 and 8; [ADR index](adr/README.md)). The current
   [R1 record](PHASE7-R1.md) describes factory-binding reuse, not closure of
   those broader decisions. Do not infer interning necessity from string bytes
   copied for other reasons.
Profiling is settled separately: the owner approved pprof compatibility out
of scope on 2026-10-11 ([ADR 0024](adr/0024-profiling-compatibility-out-of-scope.md)).
`PORTS.toml` now marks `tsc/internal/pprof/pprof.go` with editable status
`out-of-scope` and an owner/ADR reason comment. Its generated source kind,
source hash, pin and function inventory remain unchanged; no ledger regeneration
or `data/upstream.json` rewrite is needed for an editable status decision.
Native profiling is documented in [the profiling guide](PERF-profiling-2026-10.md#method).
The existing CLI behavior at `crates/tsr_execute/src/command.rs:49`, API methods
at `crates/tsr_api/src/session/checker.rs:1179`, and LSP handlers at
`crates/tsr_lsp/src/runtime.rs:1383` remain explicit unavailable/refusal responses.
This settles that scope choice without changing response behavior or waiving
other refusal-path review, performance budgets or the remaining R5 work.

R5 still needs coverage-guided evaluation if blind mutation stalls, route-specific retained
regressions for actual discoveries after fixes, and replay jobs for each
compiler/API/wasm/LSP failure boundary used. The CLI controls establish this
runner's failure detection and CLI archive replay only. Actual discovery
regressions after fixes, plus the existing LSP route under `tools/phase5/replay`,
must keep each failing entry point observable.
The dispatch fuzz workflow must execute and retain its receipt. Reviewed refusal
decisions and the remaining resource/ADR witnesses are required before R5 can
be marked complete.


## Native watch resource result

The [live-watch probe](../tools/phase7/watch/README.md) uses the production
command, compiler, OS filesystem, allocator and default native watcher. Its
passive testing hooks retain the watcher and expose counters; a controlled
scheduler calls `do_cycle` after actual native callbacks. This does not exercise
the blocking CLI run loop. The hooks enable incremental testing mode, which
stores full source text in file versions and retains the previous snapshot.
The memory figures are therefore **testing-mode watch memory**, not ordinary
CLI `--watch` memory. Those limitations remain part of the result; the tracked
owner/storage plateau and teardown checks still observe their stated counters.

Fifty equivalent edit/rebuild cycles pass on the canonical MUI docs workload,
the largest frozen Phase 7 program by file count: **16,766 files**. The input
is copied using APFS clonefile with independent inodes and all internal links
remapped. No original source is edited; its complete 239,261-file inventory
still matches after the run. External links and special files are refused.
The original scenario and provision marker are retained with the capture.

After five warmup edits, every sample has **16,775 tracked owners and 216,313
storage allocations**. Each superseded edited `ProgramFile` is released; after
watcher close the last file is released and both tracked counts are zero.
Numeric file descriptors remain at **four**, with zero difference between
before-watch and after-close. This is macOS FSEvents: `lsof` counts numeric
file descriptors, not Mach ports. Linux resource measurements remain pending.

Testing-mode watch RSS after warmup rises from 5,015,142,400 to 5,033,656,320 bytes (about 18.5 MB),
with a fitted slope of 360,631 bytes per cycle. RSS and allocator observations
remain separate from the exact ownership gates; a retained process high-water
mark is not a tracked-storage leak or a promise of zero RSS on close.

The full capture, raw reports, compiler log, isolated workload and probe
identity are at
`../ts-rust-benchmarks/2026-10-11/r5-watch-mui-docs-50/`.
The separate fifty-cycle three-file control also passes. Release build,
clippy with warnings denied, and the isolation controls pass, including a real
watch config naming external emit/build-info locations; neither those locations
nor the original fixture changes. Reproduction is in the probe README.

The encoding correction preserves BOM-marked UTF-8/UTF-16 LE/BE during edits
and compares decoded compiler text on both the controller and probe sides.
New captures explicitly label their memory scope. The historical capture above
is unchanged; its RSS is not reinterpreted as a production-mode measurement.
The focused encoding controls pass, including four real FSEvents edit/rebuild
cycles for each encoding and the existing output-isolation control. These use
the tiny fixture and are correctness checks, not a new MUI resource capture.

## Review corrections: fuzz observations and regression witnesses

The CLI and parser fuzz routes now both observe the related missing-brace
diagnostic for `function f() {\n`. Native and Rust match: the CLI includes the
explanatory parser note, and the structured parser route reports related
TS1007 with its filename, span and arguments. These focused observations and
their executable identities are retained in
`../ts-rust-benchmarks/2026-10-12/item9-fuzz-related/`. Prior captures keep their
original arguments and coverage. Replay uses an archived adapter's explicit
arguments; it does not add related-diagnostic coverage to old observations.

Launch and limit-setup failures retain inputs and partial output in separate
runner-error directories, stop with exit 2, and never enter minimization as
compiler findings. A failure while minimizing keeps the original compiler
finding. An explicitly classified compiler output-limit failure can reproduce
exactly; unclassified historical execution errors remain execution failures.

The public lifetime runner now executes each listed supporting service case
in every mode and verifies its exact executed name and exit status. The
concurrent first-use witnesses retain and compare the actual checker `Arc`s,
so fixed session identities cannot hide duplicate checker construction. The
configured wasm test compares whole file lists, the dependency-closure test
excludes nested Cargo output, the computed-entity Go probe uses the same strict
options as Rust, and project-reference expectations come directly from the
native JSON fixture. Strict computed-entity regeneration reproduces the
existing fixture byte for byte.

The revised lifetime lane passes **16 main cases plus one supplemental-services
case in each of debug, release, Miri and ASan**, 68 executions total. Sources
remained stable and independent replay verification passes. Logs and the
verified receipt are retained at
`../ts-rust-benchmarks/2026-10-12/item11-lifetime/`.
The 34 fuzz controls, 19 S10/lifetime controls, three watch-encoding controls,
focused Rust integration/example tests, affected clippy checks, formatting and
tracker validation also pass. No full corpus or performance measurement was
rerun for these corrections.

## Review follow-up and ordinary watch measurement, 2026-10-13

The fuzz protocol now refuses every non-object JSON frame before examining its
fields, so malformed adapter output is a retained protocol finding rather than
an uncaught `AttributeError`. Failure during minimization preserves both the
global and per-finding executed-trial counts. The 35 focused fuzz tests cover
non-object frames in beginning, middle and ending positions through actual
adapter processes and archival. Seven watch controls cover encoding, completion
acknowledgement, process failure and rejection of sources outside the copy.

The new [ordinary watch runner](../tools/phase7/watch/production.py) executes
the release `tsrust --watch` binary without testing hooks. On macOS it completed
50 equivalent edit/rebuild cycles on the canonical MUI docs project, with five
warmup cycles, a constant 16,766-file roster and one expected TS2304 per cycle.
A fixed-width unresolved name changes each generation; the actual diagnostic
must name that generation before its ordinary watch-completion message. Emit,
build-info writes and external content-mapper execution are disabled, as in the
separate ownership probe. Inputs are an isolated APFS copy. Its original source
inventory was unchanged after completion.

Post-warmup production RSS started at **5,098,045,440 bytes**, ended at
**4,825,186,304 bytes**, and peaked among cycle samples at **5,100,371,968 bytes**.
Numeric descriptor counts did not vary. RSS is sampled immediately after each
completion, not forced-GC live allocation or a whole-run peak measurement.
The negative fitted slope is an observation, not a proof of bounded retention.
SIGINT shutdown returned zero and the process was reaped. Process exit does not
replace the separate testing-mode owner/storage teardown assertions above.

The capture, compiler output and source inventory are retained at
`../ts-rust-benchmarks/2026-10-13/production-watch-mui-docs-50/`.
`production-watch-build/` alongside it contains Cargo's selected executable,
build output, version and artifact hash. The tiny three-file control is in
`production-watch-tiny-01/`. Both runs completed successfully. This fills the
ordinary production-watch memory observation on macOS; Linux resource behavior
and the wider R5 outstanding scope remain separate obligations.

The same follow-up corrects portable runtime identities, explicit incremental
selection in the compiler test consumer and retained-result failure semantics.
Both embed and wasm composite comparisons pass nine subtests. The production
wasm regression preserves full snapshots and cached row/header bytes after
returned errors. CI now scans the private budget workspace's dependencies and
executes its allocator/loader controls. The combined focused Python checks pass
84 tests and 95 subtests; no full corpus or timed compiler benchmark was rerun.

## Continuation review

The new AST facts, checker expression, transform expression and printer binary
continuations were independently reviewed for evaluation order and error handling.
A private synthetic malformed-expression witness enters three checker frames,
then verifies returned-error state restoration, inner-first trace closure and
successful reuse of the operation. It passes with the native recursion-probe
route that exercises the wasm driver. Full expression equivalence tests remain
separate witnesses for diagnostics, queries and type-creation/trace order.

The plain binary-chain evidence does not establish arbitrary wrapper depth:
printer parentheses and the checker's nested destructuring assignment fallback
still recurse. The browser case's 64 wrappers are its measured bound, not a
claim of unlimited depth. Native reserved/growing stacks remain in place.
