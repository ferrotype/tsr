# Phase 4 implementation checkpoint — 2026-10-02

Phase 4 is **not closed**. This checkpoint commits the implementation and
targeted validation before the owner's requested pause. It does not promote
development captures to current acceptance evidence.

## Implemented

- Native `tsrust`, shared `tsr_tsc` contracts, compiler command execution,
  incremental/build orchestration, watch program reuse and native filesystem
  watchers (FSEvents, inotify, fanotify with inotify fallback).
- Signal cancellation, mapper process teardown and stderr handling, statistics
  and tracing. New Phase 4 crates remain unpublished.
- Incremental diagnostic caches retain only referenced source/config owners,
  preserve cache identity across an unchanged rebuild, and release old programs.
- The owner's narrow eager-binding trace-order exception is in
  `data/divergences.toml`. Raw comparisons remain `different`;
  acceptance requires the approved exact transcript pair. The binary target
  is `tsrust`; `lib/tsc` is only X7's staging name.

## Observations retained locally

These runs cover different revisions during implementation. They must not be
combined into a claim that one final-source acceptance capture passed.

| Run | Result | Local artifacts |
| --- | --- | --- |
| Compiler scenarios | 215 exact, two approved trace-order differences | `target/phase4/x1-tsc-03` |
| Build scenarios | 191/192 exact; circular-reference case fixed and rechecked below | `target/phase4/x3-build-01` |
| Build-watch plus circular-reference recheck | 66/66 exact | `target/phase4/x3-buildwatch-fix-01` |
| Compiler-watch after diagnostic cache fix | 42/42 exact | `target/phase4/x5-watch-cache-fix-01` |
| Build sample determinism | 30 scenarios, 20 repeated runs, identical | `target/phase4/x3-sample-order-01` |
| Live compiler watch | 10 edits match; clean cancellation | `target/phase4/live-02` |
| Remaining live modes | All diagnostics/files and shutdowns agree; build-watch report still differs because its old clock normalizer missed terminal-clear prefixes | `target/phase4/live-03` |
| Bidirectional build-state sharing | Six families, five steps each, all-Go/all-Rust/Go→Rust/Rust→Go agree | `target/phase4/interop-dev-01` |
| Linux watcher tests on ext4 | 133 passed; new overflow test setup corrected and passed separately | `target/phase4/linux/fswatch-ext4.log`, `fswatch-overflow.log` |

The live clock normalizer now preserves terminal bytes and recognizes a cycle
start after the native clear-screen sequence; its focused regression passes.
The `live-03` report remains the original failed report, and its sources changed
during execution. It needs a fresh stable capture.

Other targeted checks passed: macOS fswatch137 and the later Close regression;
watch manager14; compiler reuse3; tracing8; compile-fail nil callback contract;
build10; native build/mapper contracts14; watcher race contracts12; native child
process3; controlled-clock emit1; incremental diagnostic retention2. These are
local checks, not CI results. The four command-line ownership contracts also
passed: 50 compile/incremental/build/watch generations each, retained-source
readability, watch counter plateau, and zero tracked owners/allocations after
release. The final all-targets build check and package asset policy check passed. The Linux overlayfs failures are retained: the
forced native fanotify test requires a filesystem supporting file handles.
The private ext4 loop image supplied that requirement without skipping tests.

## Remaining before closure

The build-cache review and function audit are complete (follow-up below).
Phase 4 still needs these acceptance and infrastructure steps:

1. The stable full scenario capture, comparison, blocker register and
   generated inventories are refreshed below. The five full determinism
   repetitions remain to run.
2. Run `scripts/phase4_native.py`'s authenticated release build and
   smoke/interop witnesses. The six-family development interop observation used
   earlier copied binaries; it is not the final build's receipt.
3. Re-run all four live modes on stable built images. Select the completed
   witnesses in `target/phase4/acceptance.json` and record the `tsc` producer.
   The acceptance integration below now authenticates and replays these results.
4. Run ThreadSanitizer; add scheduled Linux instrumentation and native Linux
   witnesses; check the release ELF's glibc 2.28 requirements. No smoke,
   sanitizer or glibc-floor result is claimed by this checkpoint.
5. Refresh the required checker/emit and other stale producers on final sources,
   update the Phase 4 record and PR, and pass P4B. Thresholds stay unchanged.

Native build entry point:
`python3 scripts/phase4_native.py build --output target/phase4/native-final`.
Then `run --build target/phase4/native-final --output target/phase4/native-witnesses`.
The build exports the pinned Go sources and creates the release target `tsrust`;
its runner checks the source closure and immutable executable digests.

## Refusal-path correction after the checkpoint

The production command entry now returns `Result<CommandLineResult, Error>`.
Unsupported operations are ordinary typed errors; the old panic payload and
helper are removed. Watch cycles propagate the same result through a loop that
closes subscriptions on exit. The native binary reports named refusals with
exit status 5 and other operational errors with status 3. Real invariant panics
still unwind. The harness joins both incremental and clean-build jobs before
propagating errors, emits `unsupported` for explicit unported operations, and
uses a separate `failed/error` classification for other returned errors.

Validation for this correction: the real command-entry refusal regression,
three harness refusal/panic tests, 15 watch-manager tests, 30 build/watch/ownership
integration tests, the complete-transcript regression selection, and 31 Python
capture/comparison tests passed. All-targets compilation and formatting passed.
No full corpus, benchmark or acceptance producer was rerun.

## Build-cache and function-audit follow-up — 2026-10-02

The build orchestrator now gives each project its own file cache. Only
declaration and JSON files share a build-cycle owner, keyed by the complete
parse options (and the inferred script kind). A brief map lock selects a
per-file entry; filesystem reads, parsing and binding occur under that entry's
lock. Ordinary source files no longer share owners between projects, and an
unrelated project load no longer waits for a whole program load to finish.
Missing, failed and panicking loads do not publish a value and can be retried.
Reset releases the build-cycle cache after the project workers have joined;
retained programs and escaped files keep their owners independently.

Every one of the 256 previously pending function rows was checked against the
pin and its Rust implementation. `phase4_audit.py check --complete` now reports
734 mapped, 246 reviewed equivalents and 27 previously assigned later-phase
operations: **zero pending, gaps or duplicate markers** out of 1,007. A reviewed
equivalent records its concrete Rust site and why no separate Go-shaped helper
is required. This is implementation disposition, not a claim that the X7
runtime acceptance has passed. The native unit-test roster remains 169 ported
and five justified not-applicable rows, with zero pending.

The review also closed actual implementation gaps:

- `Program::reuse_program` reruns mapped-file transforms and replaces canonical
  and supplemental owners together after checking paths and graph structure.
  Transform failures, collisions, changed imports and synthetic imports retain
  the full-load fallback. Compiler watch's existing mapped-file full-load
  policy stays consistent with the pin.
- Watch debug output includes event previews and subscription changes/failures.
- A failed Linux event worker retires its native descriptor before another
  subscription can be admitted. Inotify failures retain their watched directory
  and underlying error, as fanotify failures already did.
- Native directory traversal skips inode-zero records. Implicit event sequence
  increment wraps independently of the monotonic explicit-sequence updates.

Configured Phase 4 content mappers cannot carry plugin `ContributionID` values.
The watch/audit comments name the Phase 5 requirement to exclude contributed
mapper manifests once that representation is introduced; no unreachable field
or expanded exception was added here.

Focused validation: four source-cache tests, all 11 build unit tests (including
the real two-project load-concurrency regression), five program-reuse tests and
17 watch-manager tests pass. Linux's 19 handler/lifecycle and 19 event/directory
tests pass; macOS's ten directory and nine event tests pass. This Linux run was
unprivileged and did not exercise live fanotify subscriptions; the earlier ext4
run above remains historical. The 21 audit/roster script tests pass. Scoped
warnings-denied Clippy checks pass for compiler, build, command/watch and native
watcher libraries/tests (compiler tests enable their required features). The
cleanup added no lint allowances; option-metadata generation now emits digit
separators, with native observations unchanged. Six help/init/color tests,
three statistics tests and the controlled-clock emit regression also pass.
No full scenario capture, benchmark, sanitizer or acceptance producer ran in
this follow-up. Workspace formatting and diff checks pass.

## X7 witness integration — 2026-10-02

The `tsc` producer now consumes smoke, live-watch, build-state interoperability,
five-run determinism and ThreadSanitizer evidence independently through an
external capture index. The commands and index format are in
[PHASE4-acceptance.md](PHASE4-acceptance.md).

Native/live replay authenticates the release and Go builds, the pinned source
export, host, copied images and exact mode/step inventories. Per-step raw
streams and project snapshots are re-read to compute the result. The recorded
Linux evidence can be moved here and joined without modifying its reports.
Host-specific facts remain separate from the additional two-host coverage
metrics. A stale scenario capture does not hide current independent witnesses.

The determinism runner builds once (or reuses a verified full capture) and
retains five complete invocations, their outcomes and raw transcripts. The
sanitizer runner binds actual rustc invocations to copied instrumented images,
checks rebuilt std/core, discovers and runs the Phase 4 test inventory, and
runs the full scenario inventory. It preserves registry/toolchain homes,
clears the incremental-test narrowing switch and fingerprints the tests'
transitive Rust helpers. Stale or malformed proof withholds a metric; a
complete measured failure reports false. The index/report identities are
retained in the tracker's evidence stderr.

Validation: 158 Phase 4 tooling tests passed in the combined run (28.62 s).
The final malformed-compiler-argument regression and sanitizer/join recheck
also passed (35 tests, including that additional regression). These use bounded
fixtures; no full scenario, native build, live session or sanitizer execution
ran, and no acceptance evidence was re-recorded. The integration is complete;
the actual acceptance runs listed above remain pending.

## Latest-change review and CI repairs — 2026-10-03

The 59 pinned Go test references now use `source:` comments, preserving their
test-roster provenance without claiming production port coverage. The function
audit still has the same 1,007 dispositions and the native roster still has
169 ported tests. The isolated S10 consumer lockfile adds the missing
`tsr_tspath` dependency edge; no dependency versions change. The command entry
borrows the compilation inputs to satisfy the four `tsr_execute` Clippy errors.

The CLI's mimalloc change exposed two additional defects. ThreadSanitizer now
selects `tsr/system-allocator`; replay checks both the Cargo artifact's feature
and the actual compiler invocation. Ordinary builds keep mimalloc. Allocation
snapshots sample cumulative released bytes before allocated bytes, avoiding
the cross-shard underflow caused by sampling net live-byte counters during a
cross-thread free. Concurrent samples remain estimates, and the documentation
no longer claims that 64 reused shards can never contend.

Four allocator regressions and scoped warnings-denied Clippy pass with each
allocator. The isolated consumer resolves with its lockfile unchanged, and
`tsr_execute` Clippy and both command-entry unit tests pass. All 163 Phase 4
tooling tests, workspace formatting, tracker validation and the audit/roster
checks pass. No full capture or sanitizer execution was run for these repairs.
At that checkpoint the full scenario comparison was historical and its two
TS6059 differences were still open; the closure work below resolves them.

## Ledger, baselines and contract evidence — 2026-10-03

B01 is fixed in production. The build task no longer prepares project-reference
input/output maps on its primary parsed config: that added a plain TS6059 to
config diagnostics before the program produced the same error with its file
inclusion explanation. A separate lazily prepared reference config preserves
the config source's identity, and reload retires that cache. Both outputPaths
cases now match the pinned transcript, with no new exception.

The current full capture is `target/phase4/rust`: **514 exact matches and the
two previously approved trace-order pairs, out of 516 scenarios**. Every
scenario completes and all **1,255 incremental edit comparisons agree**.
`first-comparison.json` and `blockers.json` bind this capture; the blocker
register is empty. The two approvals now live in the existing
`data/divergences.toml` with `domain = "phase4"`; the separate Phase 4 JSON
registry is removed. ADR 0004 names the shared registry. The exact native/Rust
hash pairs and the raw `different` classifications are unchanged, and a
Phase 4 approval cannot waive an E2 difference.

The ledger applies decision 4, assigns `execute/tsc` to `tsr_tsc`, and records
the implemented Phase 4 homes as ported. S07 refreshes 76 Rust mapping locations
and their review hashes, plus the ledger hash in the syntax observation's
authenticated provenance; the complete observation digest is unchanged.
Native Phase 1 observations were recaptured: all 15,152 native rows and 15,206
schedule rows are unchanged. The syntax reach experiment was rerun in both
shardings; all reach entries agree with the previous recording.
Phase 2's inventory history records that input-only change; its C7 audit and
the Phase 3/4 audits are refreshed. The Phase 4 audit remains 734 mapped,
246 equivalents and 27 later-phase operations, with no gaps. The roster is
169 ported and five justified not-applicable tests.

The owner approved carrying forward the existing mutation results. The
syntax native archive and reach archive differ only in provenance: requests,
observations, instrumentation and reach entries are identical. The existing
mutation results record the old/new archive hashes and this comparison; every
Rust mutant, kill and control observation is preserved, with no claim of a new
Rust execution. Updating these bindings restores 20 of the 23 syntax operation
witnesses. The other three historical kills are also retained, but their Rust
function bodies changed in Phase 4 before this refresh: import resolution
gained tracing, type-reference resolution gained tracing and an empty-input
branch, and `GetSourceFile` switched to `FileCache.load`. Those three remain
pending current mutation validation. Their failure is a source-span change,
not the archive or ledger regeneration.

`target/phase4/contracts` retains the normal test receipt, copied executable
identities, full inventories and raw streams. It passes 302 Rust tests,
including the required compile-fail callback witness, and accounts for every
one of this host's 161 applicable pinned tests. The codec compares all 1,271
raw build-info texts against Go decode/re-encode observations (including five
intentional parse failures) and all 1,257 readable renderings. X3's 30 sample
scenarios repeat twenty times with four builders and identical transcripts.

The recorded `tsc` producer now emits `buildinfo_codec = 1`,
`unit_rosters = 1`, `watcher_tests = true`, `residuals = 0`, and X1–X6 complete
on macOS. X7 is explicitly false: the five independent acceptance witnesses
and the final cross-phase checks have not been refreshed. Linux still needs
its own current native receipt; this macOS observation does not certify its
backends. Phase 2's native-only reference runs were refreshed in both single
and concurrent modes (13,432 variants each), with every normalized row digest
unchanged. Its four creation-order fixtures were also recaptured. These refresh
the native inputs required by the self-tests; they do not refresh the Rust
correctness or performance evidence. No Rust checker/emit corpus or performance
benchmark was run here.

Formatting, warnings-denied workspace Clippy, dependency policy, workspace
build, testhost, generation and scanner checks are recorded and passing. The
final self-test passes 69 tracker tests and 1,844 script tests, with three
declared skips. Ledger validation passes. The status views record the remaining
Phase 4 and cross-phase requirements rather than treating these checks as
whole-phase completion.
