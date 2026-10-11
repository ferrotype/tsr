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
encoded byte. It implements only the named TS parse/index-before/encode/index-after
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
archive root. These are bounded smoke results, not a full-corpus or enforced
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
normalization. Message bodies, related messages and order remain observable.
All emitted paths and bytes must match exactly. Exit statuses are compared;
crashes and timeouts are findings even if both compilers suffer them. The total
per-case allowance is split equally across the two children; the run reserves
a complete pair before starting it. Process groups are killed on completion,
including descendants. Linux enforces per-process address-space limits through
`RLIMIT_AS`; this is not aggregate RSS enforcement. macOS memory enforcement
is explicitly unsupported/informational. The smoke below therefore does not
witness an enforced memory cap.

Findings retain original and minimized files/options, complete normalized
observations, raw output, emitted bytes and executable identities outside
`target`. Chunk-deletion minimization retains the original failure categories
and exit/timeout statuses after executing both children, subject to a trial
limit and the shared budget. The original is saved first. No automatic parity
acceptance, approval or fixture promotion occurs. A zero-completed-pair run
exits nonzero. [Usage and replay](../tools/phase7/fuzz/README.md) describe the
options and the timing/artifact overhead excluded from the work budget.

The twenty CLI/archive [fake-child controls](../scripts/tests/test_phase7_fuzz.py) pass
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
test failure. The review identifies extension diagnostics, computed entity-name
display, package diagnostic chains and project-reference-output importer
queries among the remaining functionality. It creates no new approvals and
does not close R5. A smaller count would not itself mean improvement.

## Existing witnesses and smallest remaining work

The following pointers were inspected; their expensive producers were **not
rerun** for this record. Historical receipts retain their original scope and
source freshness requirements.

| Area | Existing witness | Missing R5 work |
| --- | --- | --- |
| Native parser/binder depth | `crates/tsr_parser/src/recursion_tests.rs:12` exercises twelve grammar shapes on 512 KiB initial stacks and observes real growth. `crates/tsr_binder/src/recursion_tests.rs:35`, `binary_trampoline_tests.rs:134` and `data/s07/binder-depth-cases.json` cover binary continuations and nested syntax. `crates/tsr_parser/src/worker.rs:16` reserves 256 MiB for native parser workers. | Join the existing deep inputs through full parse/bind/check/transform/emit, including left/right chains, JSX and conditional types, with recorded native stack configuration; audit the four left-operand sites and record the item-11 decision. Small generated fuzz inputs are not a deepest-input audit. |
| Checker/printer growth and unwind | `crates/tsr_checker/src/node_builder_stack_tests.rs:55` observes grown-stack type display and factory-panic retirement. `crates/tsr_printer/src/tests.rs:145` exercises recursive printing, growth, unwind and final counters. | These component witnesses do not certify the full compiler or every transform site; add the missing end-to-end stress cases and retain failure/limit observations. |
| WebAssembly stack | [S10 historical results](S10-results.md) record two default-Node stress traps and completion with `--stack-size=4096`, separate from the 16 MiB wasm linear stack. | Execute the applicable full public check/emit deep cases at R4's current artifact boundary with stack settings explicit. Parser/type-query historical evidence does not certify current full emit or browser-default stacks. |
| Watch storage plateau | `tools/phase4/tsctests/tests/ownership.rs:214` drives 50 real command-line watch cycles through a mock backend, retains early files, checks plateau after cycle two, releases superseded roots and asserts empty tracked counters after close. | Preserve that deterministic contract; add repeated equivalent edit cycles on the owner's largest real project with live-owner/storage observations and a separately reported RSS trend. RSS high-water alone is not a leak result. |
| Native watch descriptors | `crates/tsr_fswatch/src/linux/tests.rs:253` checks worker/descriptor release after shutdown; `:504` checks failed-worker descriptor retirement. [Phase 4 acceptance](PHASE4-acceptance.md) requires host-specific watcher receipts. | Measure native descriptor use and release on the owner's largest project on each applicable host; small shutdown fixtures do not establish scaling limits. |
| E3 ownership and instrumentation | [S09-5](S09-5.md) describes `scripts/s08_ownership.py`, exact S04/S06/S07/S09 inventories and its historical debug/release/Miri/ASan closure. [S10](S10-results.md) records six historical external-consumer lifetime cases. The current `tools/s10/lifetime-cases.json` names all fourteen public integration tests; `scripts/s10_lifetime.py` requires exact source/manifest agreement and executed-name agreement in all four modes. | Reuse those contracts and check current source applicability before claiming them for R5/R4. Their counters measure tracked storage, not whole-process heap or RSS, and they do not replace large-project watch measurements. |

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

1. Record evidence-based outcomes for all four recursion/trampoline sites under
   [ADR 0011](adr/0011-deep-recursion--reserved-stacks-and-growth-guards.md) and
   PLAN section 13 item 11; the binder's existing continuations settle only its
   exercised slice.
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
