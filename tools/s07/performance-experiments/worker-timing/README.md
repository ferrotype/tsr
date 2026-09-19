# Bounded worker timing diagnostic

This copies the exact frozen native benchmark source and adds elapsed clocks to
its existing workers. It does not modify production crates, change scheduling,
emit E5/E6 metrics, or estimate an acceptance ratio. Run **one Rust and one Go
invocation**, in that order, with eight workers and no warmup/repeats. The fixed
order and single observations prevent statistical attribution; the purpose is
to distinguish observed worker work, receive time and end-of-work tails.

Preserved behavior:

- `index % 8` round-robin assignment, one bounded queue of capacity eight per
  worker, original worker creation/start/end coordination and retention endpoint.
- All original parse/bind work, consuming exclusive Rust binding, Go GOMAXPROCS
  eight/GOGC 100, mimalloc and normal Rust release settings (no profile feature).
- Original length-framed loaded-input digest, file/node/symbol/diagnostic counts,
  input order, and complete result retention through the native endpoint.

Instrumentation:

- Each worker accumulates receive-call elapsed and parse/bind/result-retention
  elapsed. Receive elapsed includes waiting, scheduling and channel overhead;
  work elapsed includes descheduling and runtime/allocation activity. These are
  **not sampled CPU** and do not identify why an observed wait happened.
- Rust workers can enter receive before the original main pipeline clock starts.
  Their first receive interval is clipped at that start, using a common monotonic
  epoch. Go keeps its existing start channel, which opens after its main clock.
- Start/completion offsets and completion-to-pipeline-end tails expose workers
  finishing at different times. The final receive until queue closure is included
  in receive elapsed. Main send-call elapsed includes queue blocking, scheduling
  and channel work; it must not be labeled pure blocked time.
- Actual receive indices are recorded into preallocated per-worker buffers
  (104,752 bytes total for this workload). Ordered SHA-256 assignment digests and
  per-worker counts/bytes are produced **after** the timed endpoint and compared
  with an independent `range(worker, file_count, 8)` assignment. This catches
  missing, duplicated, reordered or wrongly routed indices beyond global counts.
- Per-file clocks and small index writes perturb the worker path. There is no
  uninstrumented diagnostic control and no subtraction of timer cost.

`patches.py` requires every frozen anchor exactly once. `probe.py build` archives
source patches, copied compiler/driver source, build commands/stdout/stderr,
registry-lock/configuration identities and native artifacts. The frozen source
closure lacks unrelated workspace members: the copied root manifest narrows the
workspace, removes the parser’s uncompiled test-only `tsr_encoder` dependency
(the encoder is absent from the frozen closure), and metadata may prune its copied
lock offline. Every resolved registry
version/checksum must remain identical to an entry in the original frozen lock.
Rust compiles in a new empty target directory per build, with both target-dir
and build.build-dir enforced and Cargo’s reported executable checked inside it.
No source outside these explicit driver/membership/test-only-manifest/lock changes is transformed.
The pinned Go source closure is freshly exported and hash-bound; the original
frozen Go bridges are used. Only the instrumented driver is replaced.

Build and capture both acquire the existing measurement lock. Capture also
rejects competing compilers and benchmarks. Every raw invocation/error survives;
an existing output directory is never reused. `verify` replays strict native
work/protocol checks plus worker assignment and timer conservation checks without
executing a workload.

```sh
python3 tools/s07/performance-experiments/worker-timing/probe.py build \
  --control target/s07-bis/current-candidate-acceptance-2026-09-10/frozen \
  --control-sha256 1c04605dcd248d78dcec1e42c4c6f82f036843a054cf5e27d81c1b3b229753b5 \
  --output target/s07-bis/drain-hash/worker-timing/build
python3 tools/s07/performance-experiments/worker-timing/probe.py capture \
  --build target/s07-bis/drain-hash/worker-timing/build \
  --build-sha256 BUILD_SHA \
  --output target/s07-bis/drain-hash/worker-timing/capture
python3 tools/s07/performance-experiments/worker-timing/probe.py verify \
  target/s07-bis/drain-hash/worker-timing/capture
```
