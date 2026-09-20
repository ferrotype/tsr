# A0 phase attribution and owned-core shape census

This diagnostic crate compares `published` and `consuming` binding using the
**same current source revision and executable**, on one reserved-stack worker.
It helps explain the normal-binary A0 screen; it is not the old control binary,
does not emit E5/E6 metrics, and cannot promote a candidate.

Each file follows one of these complete measured paths:

| Backend | Parse timer | Binding and publication timer |
| --- | --- | --- |
| `published` | Parser including completion validation | Publish unbound, bind through overlays, retain completed file |
| `consuming` | Same parser | Select eligible ownership, bind directly or use published fallback, perform required final validation, publish and retain |

The grouped second timer prevents moving final validation/publication outside
the consuming measurement or describing their combined duration as binder CPU.
Timers are elapsed wall clocks, include descheduling and timer overhead, and
are separate from sampled Running CPU. There are no native allocation hooks or
RSS claims. All completed roots survive the pipeline endpoint and reporting.
Preload, worker setup, bounded queue and retained-root boundaries follow the
existing CPU attribution driver. Both modes retain the same enum wrapper.

The executable always enables `tsr_ast/layout-profile`; that diagnostic feature
is recorded in its manifest and applies to both backends. Its shape traversal
is run only when requested, after all measured endpoints. Do not compare this
driver's timings with a differently built normal or allocation executable.

Build after implementation and helper inputs stop changing:

```sh
python3 tools/s07/performance-experiments/phases/probe.py build \
  --control target/s07-bis/control \
  --control-sha c8e5dd18abb62a9cdd3c3af47051cc933c7744995a9a7029ac20a53bf8ea6363 \
  --output target/s07-bis/a0-phase-build
```

The builder uses `--offline --locked`, preserves caller Cargo/Rustup homes,
verifies registry checksums against the workspace lock and consumes Cargo's
actual executable path. It enforces the normal release profile, copies the
artifact before reuse, snapshots the production source and records tool,
configuration, compiler, input and executable hashes. Output directories must
be new and are made read-only. A build holds the measurement exclusion lock;
it may coexist with semantic graph checks, but timing screens cannot overlap.

After the host is quiet and other profiling/benchmark work has stopped:

```sh
python3 tools/s07/performance-experiments/phases/probe.py capture \
  --build target/s07-bis/a0-phase-build \
  --build-sha SHA_PRINTED_BY_BUILD \
  --output target/s07-bis/a0-phase-capture
```

This runs one warmup per backend, then seven alternating published/consuming
pairs on the full 13,094-file frozen workload. Every child reports the bytes
and options it loaded, all work counts and actual direct/fallback selection.
Complete raw stdout/stderr, all observations, ranges of raw values and medians
are retained; there is no sample extension, gate result or confidence claim.
The complete group and pipeline ratios are labeled `consuming_over_published`.
The external build-manifest SHA and immutable inventory are verified around
each child. Output directories are never reused.

For an **untimed census** without the sixteen phase-attribution children:

```sh
python3 tools/s07/performance-experiments/phases/probe.py shapes \
  --build target/s07-bis/a0-phase-build \
  --build-sha SHA_PRINTED_BY_BUILD \
  --output target/s07-bis/a0-core-shapes
```

This runs the consuming backend once and writes one ordered record per input to
`core-shapes.ndjson`. Each record contains its physical core payload-shape
counts and direct/fallback status. The validator checks every input index and
the summed allocated-node obligation. This removes the old census's merged
core/overlay ambiguity. It can price proposed sparse shape directories and
per-file page policies, but **does not measure existing or replacement payload
page capacities**. The report retains incidental clocks from that run without
treating them as a timing comparison.

```sh
python3 -m unittest discover -s tools/s07/performance-experiments/phases -p test_probe.py -v
```

These validator tests challenge changed bytes, wrong backends, overlapping or
misnamed phase intervals, duplicate/missing shape records, allocated-node drift
and standalone registry drift. Runtime correctness still comes from the actual
full workload and the separate parser/binder/ownership evidence.

The [actual occupancy projection](layout-projection.md) records the untimed
census, compiled directory envelopes, page-policy matrix and bounded next
word-class prototype. Its deterministic compressed raw census and model replay
live alongside this driver. The separate B phase attribution is preserved in
[the B result archive](../results/2026-09-08/a0b-README.md).
