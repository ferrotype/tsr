# Phase 4 witness capture and replay

The `tsc` producer consumes the scenario capture and five independent X7
witnesses. Connecting these runners does not certify a result: each metric
needs a current capture from its runner. Previous development captures remain
historical observations. Native/live version-1 reports lack the build and raw
observation proof required by the version-2 replay contract.

## Capture commands

Run these on stable sources when ready for acceptance. They are real builds
and executions, not part of a producer replay or tooling unit test.

```sh
python3 scripts/phase4_native.py build --output target/phase4/native-final
python3 scripts/phase4_native.py run --build target/phase4/native-final --output target/phase4/native-witnesses
python3 scripts/phase4_live.py --build target/phase4/native-final --output target/phase4/live-final
python3 scripts/phase4_determinism.py run --output target/phase4/determinism-final
python3 scripts/phase4_sanitizer.py run --output target/phase4/tsan-final
python3 scripts/phase4_contracts.py run --output target/phase4/contracts
```

Native capture compares both smoke thread configurations and all six
build-state families, in both runtime directions and against the all-Go
control. Live capture requires all four watch/build-watch modes, including
symlink roots, and every scripted edit. Raw streams, invocations, process
outcomes and per-step project snapshots are retained; replay derives the
diagnostics and files again instead of trusting summary `pass` fields.

Determinism builds once and executes the complete scenario inventory five
times in independent processes. `--capture target/phase4/rust` can reuse the
authenticated build from a current full scenario capture. A synthetic test
capture cannot supply that build. The five runs compare raw transcripts and
row states; this does not replace the separate baseline-parity gate.

ThreadSanitizer uses the pinned nightly, `-Zsanitizer=thread`, `-Zbuild-std`
and the system allocator. The CLI uses `tsr/system-allocator` in this build;
ordinary CLI builds retain mimalloc. Replay checks the feature in both Cargo
artifacts and the actual compiler arguments. It retains compiler invocations, Cargo
artifact mappings, test discovery and test output, and the complete scenario
run. An empty suite, skipped required tests or a sanitizer report cannot
certify the gate. This command does not install a nightly or replace the
user's Cargo/Rustup homes. A missing toolchain is an execution prerequisite,
not an approved exception.

The normal contract run builds all Phase 4 unit and integration targets plus
the VFS nativepath witness. It records Cargo artifact identities, complete
test inventories and raw outputs, and checks every applicable pinned roster
entry against an executed Rust test. The nil-callback rejection is witnessed
by its separately inventoried and executed compile-fail doctest, rather than
by a runtime test. The codec test compares all 1,271 input
texts with the pinned Go codec (including five intentionally malformed texts)
and all 1,257 readable renderings. The X3 contract repeats all 30 sample
scenarios twenty times with four builders. A skipped native backend cannot
pass `watcher_tests`; another host's receipt cannot certify this host.
`replay --output DIR` validates the receipt without rerunning tests. Use
`phase4_producers.py tsc --contracts DIR` to inspect another retained receipt;
the ledger producer uses `target/phase4/contracts`.

`unit_rosters` is the observed/applicable test ratio on the current host,
`buildinfo_codec` is 1 only when the entire codec witness passes, and
`residuals` counts unapproved scenarios in the freshly rebuilt blocker
register. Missing or stale observations withhold these metrics. The X1–X6
completion predicates require current inventory verification, audit,
comparison and blocker records, their full scenario families and their
contract suites. X2 and X5 also require exact incremental correctness; X4
requires the native watcher run; X6 accepts only the exact approved trace
pairs. X7 joins those completions, a complete roster, zero residuals and all
five X7 witness metrics. The sprint separately retains the required current
Phase 2/3 gates; an X7 metric alone never declares P4B closed.

## Selecting evidence

Create `target/phase4/acceptance.json` when the captures exist. Paths are
relative to that index's directory unless absolute. Each host appears at most
once per group. For example, a macOS capture index is:

```json
{
  "version": 1,
  "native": [
    {"host": "macos", "build": "native-final", "capture": "native-witnesses"}
  ],
  "live": [
    {"host": "macos", "build": "native-final", "capture": "live-final"}
  ],
  "determinism": [{"capture": "determinism-final"}],
  "thread_sanitizer": [{"host": "macos", "capture": "tsan-final"}]
}
```

To join the recorded Linux evidence from decision 9, retain the complete Linux
build and capture directories and add `host: "linux"` entries pointing at
them. Replay validates their recorded build host and original invocations;
moving the directories does not require editing their reports. It never
executes a foreign binary. Omit an unavailable group instead of inventing a
successful result. The producer also accepts a different index with
`--witnesses PATH` for local inspection; `cargo xtask run tsc` uses the default.

## What the producer records

| Metric | Required witness |
| --- | --- |
| `smoke` | Both compiler-fixture thread modes on the producer's host |
| `live_watch_parity` | All four live modes and all edits on that host |
| `buildinfo_interop` | All six families and four runtime orders on that host |
| `determinism` | Five complete runs of the same current harness build |
| `thread_sanitizer` | Instrumented Phase 4 test inventory and full scenario run on that host |

Native metrics also have `_macos` / `_linux` facts when those captures are
present. `smoke_all_hosts` and `live_watch_parity_all_hosts` are emitted only
when both host proofs are present. This preserves the plan's host-specific
tracker gates and makes the second provenance visible; a macOS result does
not stand in for Linux validation.

Missing, partial, stale or malformed evidence withholds the corresponding
gate metric and reports why. A complete, authenticated measured failure
reports `false`. One unavailable group does not discard other valid groups,
and an unavailable scenario capture does not hide independent witnesses.
The exact index and verified report digests are included in producer stderr,
which the tracker retains and hashes in its committed evidence record. The
producer protocol still contains only numeric/boolean metrics.

All verifiers recompute the current source closure and required inventories.
The `[tsc]` source list includes these runners and their tests. No producer
invocation launches these five expensive witnesses. The existing native
scenario-inventory verification still runs as before.

The five X7 witnesses alone do not set `x7_complete` or close P4B: the
checkpoint and contract requirements above must also hold. Final correctness
captures, the binary-floor evidence and cross-phase gates retain their own
requirements.
