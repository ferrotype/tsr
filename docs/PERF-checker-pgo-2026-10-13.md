# Checker profile-guided optimization, 2026-10-13

Status: retain an opt-in PGO build experiment. On this Apple Silicon host,
profile-guided compilation materially reduces checking time on both training
and held-out projects. No Rust implementation, default build profile, release
pipeline, parity expectation or performance threshold changes in this step.

The two additional source candidates [N and O](PERF-checker-algorithms-2026-10-13.md#follow-up-retained-list-candidates-n-and-o)
were neutral and removed. This experiment tests compiler optimization of the
retained H/I/J/K source at `9169eec093a2f65aece01ece80a0e89478a37736`.
It does not establish a new overload/inference algorithm or explain exactly
which inlining, branch or layout decisions account for the improvement.

## Build and training

Both executables use Rust 1.97.1, LLVM 22.1.6, native target
`aarch64-apple-darwin`, optimization level 3, fat LTO, one codegen unit and
unwinding. All 92 Cargo artifact feature/profile records agree. No target-CPU
override, unsafe access shortcut or diagnostic suppression is introduced.

The instrumented build used `-Cprofile-generate=<archive>/pgo/raw`. Training
ran full checking once for each of VS Code with two checkers, VS Code with
eight, and the self-compiler with two. Every run matched frozen diagnostic and
file output; type counts were respectively 2,509,704, 3,225,692 and 142,282.
The matching toolchain's `llvm-profdata` merged their three profiles. The
candidate used `-Cprofile-use=<archive>/pgo/merged.profdata` and
`-Cllvm-args=-pgo-warn-missing-function`.

The build produced 2,849 missing-function-profile warnings, chiefly for paths
outside the training workload such as protocol/language-server code; no
profile mismatch warning occurred. Raw logs are retained. The instrumented
and optimized builds took about 265 and 221 seconds respectively; these are
build costs, not benchmark measurements.

**MUI and XState were not used for training.** This reduces one overfitting
risk; it does not certify emit, language-service, other architectures or all
checker modes. Profiles must be rebuilt for changed sources/toolchains rather
than reused as permanent release inputs.

## Concurrent control/candidate screens

The primary screen uses `tools/phase7/bench/pairs.py`: both executables start
at a barrier, submission order alternates, and output validation waits until
both finish. No builds, tests or profiler overlap these intervals. All 27
pairs complete with equal diagnostics, file output and exact type counts.
Ratios are medians of individual pair ratios; smaller is better.

| Scenario; checkers | Pairs | Median wall seconds, control / PGO | Wall ratio | Check ratio | CPU user ratio | RSS ratio |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| VS Code; 2 | 3 | 17.882 / 13.611 | 0.7612 | 0.7128 | 0.7227 | 1.0010 |
| VS Code; 8 | 5 | 8.125 / 7.251 | 0.8893 | 0.7500 | 0.7528 | 0.9862 |
| Self-compiler; 2 | 7 | 1.349 / 0.891 | 0.6654 | 0.6490 | 0.6475 | 0.9915 |
| XState; 2 (held out) | 7 | 1.396 / 1.058 | 0.7573 | 0.7072 | 0.7297 | 1.0099 |
| MUI docs; 2 (held out) | 5 | 15.707 / 13.146 | 0.8369 | 0.8213 | 0.7996 | 0.9993 |

Concurrent execution shares host resources. In particular, the faster process
can finish while the control is still checking, so these ratios alone are not
standalone throughput measurements.

## Standalone confirmation

The same runner's explicit `--serial` mode runs one compiler process at a time,
alternating control/candidate order. It retains the same output/count checks.
All eleven pairs pass; no build, test or profiler overlaps them.

| Scenario; checkers | Pairs | Median wall seconds, control / PGO | Wall ratio | Check ratio | CPU user ratio | RSS ratio |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| VS Code; 2 | 3 | 16.327 / 11.928 | 0.7313 | 0.7082 | 0.7174 | 1.0027 |
| XState; 2 (held out) | 5 | 1.338 / 0.991 | 0.7387 | 0.6979 | 0.7224 | 0.9984 |
| MUI docs; 2 (held out) | 3 | 14.747 / 12.053 | 0.8159 | 0.8090 | 0.7883 | 0.9966 |

Standalone checking time falls about 19–30% in these samples. RSS is essentially
unchanged; this is not a memory improvement claim. Samples are small, and these
results do not close R1's full scenario/mode/checker-count matrix or substitute
for paired Go/Rust acceptance.

## Reproduction and preserved artifacts

Artifacts are outside `target/`, in
`../ts-rust-benchmarks/2026-10-13/checker-context/`. They include both binaries,
the exploratory build script, source hashes, Cargo output, raw and merged
profiles, every training output, and every timing sample. `pgo/manifest.json`
records the builds/training. `pgo-{scenario}-{checkers}` holds concurrent pairs;
`pgo-serial-{scenario}-{checkers}` holds standalone pairs.

| Executable | SHA-256 |
| --- | --- |
| `control/tsrust` | `3ec1aa6a822179cb92e25bbc1f5f5007be37f3af48e181e07beba8b718e0aa51` |
| `pgo/optimized/tsrust` | `37b16aeb7721426288410145f684edabfd172e2c005e8e83991a88d5f7b94e62` |

Before and after the screens, an independent audit hashed the listed source
bytes, including dependencies under `node_modules`, checked both executables'
resolved options against the frozen descriptors, and recorded bundled library
and binary hashes. These audits agree. The executable/configuration/source
identity and all concurrent work comparisons also received independent review.

The committed `tools/phase7/bench/pgo.py` reproduces the build/train/merge/build
sequence with explicit pinned-toolchain/profile checks and saved Cargo artifact
selection. It additionally preflights training input bytes and options before
training and checks them afterward. Preflight profiles are excluded from the
training merge. It fails closed and preserves failed-stage output, refuses an
existing archive directory, and does not publish performance results itself.

From the repository root, with provisioned scenarios and the pinned toolchain's
`llvm-tools` component installed:

```sh
ARCHIVE=../ts-rust-benchmarks/2026-10-13/checker-context
CACHE=../ts-rust-wt/.ts-rust-workloads/phase7.noindex
python3 tools/phase7/bench/pgo.py --cache "$CACHE" --output "$ARCHIVE/NEW-pgo"
python3 tools/phase7/bench/pairs.py --scenario vscode --mode check \
  --checkers 2 --pairs 3 --cache "$CACHE" \
  --control "$ARCHIVE/control/tsrust" --candidate "$ARCHIVE/NEW-pgo/optimized/tsrust" \
  --output "$ARCHIVE/NEW-vscode-2"
```

Repeat the pair command with the scenario/checker/pair counts in the tables.
Add `--serial` and use a new output directory for standalone confirmation.
The original measurements used the archived exploratory script; the committed
tool makes that workflow portable and adds input/configuration checks. The
saved measured binaries remain the authority for the tables above.

Validation of the committed tooling: 46 focused Python tests pass, including
build/train/merge failure sequencing, exact Cargo artifact selection, changed
inputs/options, timeout/interruption cleanup, default concurrent execution and
serial ordering. The real training preflight passes for all three configurations
against the saved instrumented executable. A real timeout probe confirms that
the helper kills a spawned child as well as its parent and preserves output.
`cargo xtask validate` and `git diff --check` pass. The source candidates' Rust
and selected parity validation is recorded separately; no new Rust code remains
in this follow-up.

The useful next step is to evaluate a reproducible PGO release strategy across
the remaining workloads and targets, including emit and language-service work.
Automatically enabling these training-specific profiles in normal builds is
not justified by this check-only, single-host experiment.
