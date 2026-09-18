# ADR 0021: Parse-and-bind performance thresholds re-based on measured evidence

Status: Accepted (2026-09-10)
Plan: section 5 (expected benefits); section 9, Phase 0 experiments table, rows E5 and E6; section 13, item 6
Sprint: S07, item S07-4
Amends: PLAN.md experiments table (E5, E6); `status/experiments.toml` criteria `E5.peak_rss`, `E5.allocated_bytes`, `E6.one_thread`, `E6.eight_threads`

Subsequent decision: [ADR 0022](0022-checker-type-footprint-threshold.md)
(2026-09-18) raises the S08 per-type footprint limit from 0.80 to 0.85.
The four parse/bind limits decided here are unchanged. References below to an
unchanged per-type limit describe the decision as accepted on 2026-09-10.

## Context

Section 5 of the plan expected 30 to 50 percent lower peak memory than Go and called CPU gains of 1.2 to 2 times "plausible from data layout but not guaranteed". The Phase 0 experiments table turned those expectations into thresholds: peak RSS and bytes allocated for parse and bind at most 0.7 of Go, and wall time at one and eight threads at most Go's. S07 measures them on the pinned VS Code workload (13,094 files, 161,740,237 bytes, 19,593,488 nodes) with the producers registered in `status/runs.toml`.

The first S07 capture (2026-09-07, evidence `30c6deb7…` and `494380cb…`) measured Rust at 1.789 / 1.931 of Go wall time at one / eight workers, 1.626 of Go's allocated bytes and 1.447 of Go's peak RSS. The S07-bis program that followed (`docs/S07-bis-performance-plan.md` and its records) replaced the uniform 80-byte node with a 24-byte header and typed payload rows, moved binding fields into those rows, replaced the binding side tables with a branded local scope for the consuming binder, compacted lists, text, symbols and flows, and removed the allocation churn it could attribute. The retained implementation passed full workload graph parity against Go and the applicable E3 ownership inventories in debug, release, Miri and AddressSanitizer. Intermediate failures and rejected experiments remain in their original records.

The paired acceptance capture of the retained implementation (2026-09-10, revision `26b6c4e`, bundle `1c04605d…`, E5 evidence `ae0e9fc6…`, E6 evidence `d5e7541f…`, record `docs/S07-bis-candidate-acceptance.md`) measured, in one batch on the same host:

| Metric | Rust / Go | 95% bootstrap interval | Original threshold |
| --- | ---: | --- | ---: |
| Wall time, 1 worker | 1.214 | 1.150 to 1.235 | 1.0 |
| Wall time, 8 workers | 1.405 | 1.280 to 1.443 | 1.0 |
| Bytes allocated | 0.738 | | 0.7 |
| Peak RSS | 0.737 | | 0.7 |

The original targets were not met. Allocation counts measure requested traffic within the parse/bind interval; peak RSS measures the process lifetime, including preload and allocator slack. Neither is a direct retained-object census. Rust uses about 26 percent less allocated memory and peak RSS than Go in this capture, short of the original 30 percent reduction. Earlier attribution identifies transient traffic and storage slack worth distinguishing, but does not prove that the remaining deficit is entirely transient or that retained storage needs no further work.

The predicted data-layout CPU gain did not establish parity for parse and bind. The retained implementation is 21 percent slower at one worker and 40 percent slower at eight in this batch. Earlier bind-only profiles locate distributed access costs; the later exact-control profile still contains both required work and access/allocation overhead. Those sample weights are not a causal decomposition of the current 653 ms one-worker elapsed gap. The last bounded drain/hash follow-on has point estimates near 2 percent / 1 percent improvement, with confidence intervals crossing parity, and was not retained. At eight workers Rust scales 4.4 times against Go's 5.0 times. The matched worker diagnostic found similar static assignments and receive-time shares; it did not establish the cause or exclude scheduling defects.

## Decision

The parse-and-bind thresholds are re-based on the measured evidence. Each new threshold is the measured 97.5 percent upper bootstrap bound of the retained implementation rounded up to the next five hundredths, or, for memory, a value that the measured ratio meets with margin and that still states a clear advantage over Go.

| Criterion | Metric | Old | New |
| --- | --- | ---: | ---: |
| E5 peak RSS | `run.e5.peak_rss_ratio` | ≤ 0.7 | ≤ 0.85 |
| E5 bytes allocated | `run.e5.allocated_bytes_ratio` | ≤ 0.7 | ≤ 0.85 |
| E6 one thread | `run.e6.one_thread_wall_time_ratio` | ≤ 1.0 | ≤ 1.25 |
| E6 eight threads | `run.e6.eight_threads_wall_time_ratio` | ≤ 1.0 | ≤ 1.45 |

The E6 stability rule (`run.e6.stable`) requires each mode's 97.5 percent upper bootstrap bound to be at or below that mode's threshold, in place of the hardcoded 1.0; the sample-extension rule uses the same threshold. The benchmark scripts read the thresholds from `status/experiments.toml`, which is their executable source of truth, and that file joins the capture's source fingerprint and the E5/E6 producers' inputs so that a later threshold edit stales the evidence instead of silently re-grading it. Both runtimes' relative MAD limit of 5 percent is unchanged. The E5 per-type footprint criterion (`run.e5.type_footprint_ratio` ≤ 0.8) is unchanged; it is measured in S08.

These thresholds are authorized for the measured host class, enforced at capture and consumption: macOS arm64 with at least eight schedulable CPUs, Go pinned by `data/s04/toolchains.toml` with `GOGC=100`, `GOMAXPROCS` equal to the worker count and `GOTOOLCHAIN=local`, Rust on the pinned stable toolchain with mimalloc. No Linux measurement exists. Earlier Go profiles on Darwin assign roughly 0.5 seconds of sample weight per one-worker run to `runtime.madvise` on the `MADV_FREE_REUSE` accounting path. That is not an elapsed saving that can be subtracted to predict Linux results. Linux ratios remain unknown. An informational Linux experiment would require a separate runner; it cannot supply acceptance evidence until the owner extends this decision to that host class.

The S07-bis rule that no threshold changes to finish an experiment (`docs/S07-bis-performance-plan.md`, section 6) is overridden by this owner decision for these four criteria only. No experiment record, screen verdict or control is relabelled; the S07-bis records remain the evidence trail as written.

## Consequences

The acceptance capture of 2026-09-10 is the evidence for this decision, not the evidence that closes S07-4. The benchmark scripts are part of every capture's source fingerprint, so applying this decision stales that capture and its E5/E6 evidence; S07-4 can close on one fresh paired capture of the retained implementation at the revision containing this decision under the re-based rule, produced by the standard sequence with no protocol change. That capture is a check of the decision, not a re-derivation of it: the measured margins are thin (one-worker upper bound 1.235 against 1.25, eight-worker upper bound 1.443 against 1.45), and a capture that misses is recorded as a miss and reported to the owner. It is not repeated until it passes. `cargo xtask check S07` then depends only on its remaining obligations.

The Phase 0 gate (ADR 0020) evaluates E5 and E6 against these thresholds and must record them as re-based, with the original values, the measured ratios and this ADR beside them. The extrapolation from parse-and-bind to full checking is unchanged and remains an extrapolation.

The following remain open questions, not commitments of S07 or this decision: the eight-worker scaling difference (4.4 versus 5.0 times) and its cause; the 653 ms one-worker elapsed gap in the preceding paired capture; and residual allocation traffic. The earlier attribution measured 334 MB of freed/superseded requests, including 108 MB unclassified, on its own frozen binary before later retained changes. Those historical amounts are not a current traffic census. Any renewed investment is a separate decision with its own plan and stop rule.

The pinned Go and Rust executables, the workload transport and the loaded-input digest `d4ff2ad3…` used for the acceptance capture are retained in `tools/s07/performance-experiments/results/2026-09-10-candidate-acceptance`; the thresholds may be revisited when the host class, the Go pin or the workload changes, by amending this record.

## Evidence

`docs/S07-bis-candidate-acceptance.md` (paired capture, E5 `ae0e9fc6…`, E6 `d5e7541f…`); `docs/S07-bis-control-refresh.md` (paired baseline of the preceding control, Go 3.120 s / 0.730 s); `docs/S07-bis-bind-cpu-comparison.md` and `docs/S07-bis-drain-hash-profile.md` (CPU attribution); `docs/S07-bis-allocation-traffic.md` (allocation attribution); `docs/S07-bis-drain-hash-result.md` (last bounded CPU follow-on, unretained); `docs/S07.md` (original S07 capture); `tools/s07/performance-experiments/results/` (immutable bundles and replay).
