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
  `data/phase4/approved-differences.json`. Raw comparisons remain `different`;
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

1. Finish the final review and ordinary checks (including warnings denied).
   Review build host cache scope: Rust currently caches all source kinds,
   whereas the pinned build host only shares declaration/JSON sources; source
   loading also uses a mutex. Do not treat this as a proven equivalent.
2. Close the remaining function-audit dispositions: the current valid audit
   contains 256 pending rows (639 mapped, 85 equivalent, 27 later). The native
   test roster has 169 ported and five justified not-applicable rows, none
   pending. Forty additional reviewed build mapping candidates are retained in
   `target/phase4/checkpoint/build-equivalents-to-review.json`.
3. Run a single stable full scenario capture and five determinism repetitions.
   Refresh the comparison, blocker register and generated inventories.
4. Validate `scripts/phase4_native.py` build provenance and run its release
   smoke/interop witnesses. The six-family development interop observation used
   earlier copied binaries; it is not the final build's receipt.
5. Re-run all four live modes on stable built images. Add authenticated witness
   consumption to the producer; currently the new runners are development
   tools and do not emit the remaining X7 gate metrics.
6. Run ThreadSanitizer; add scheduled Linux instrumentation and native Linux
   witnesses; check the release ELF's glibc 2.28 requirements. No smoke,
   sanitizer or glibc-floor result is claimed by this checkpoint.
7. Refresh the required checker/emit and other stale producers on final sources,
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
