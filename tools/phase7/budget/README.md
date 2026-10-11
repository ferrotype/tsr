# Production embedding budget foundation

These tools capture comparable compiler work and resource measurements over
frozen inputs. A capture records evidence; it does not change budget thresholds.
The native authorities are the pinned Go compiler and public `tsr_embed`.
The wasm authorities are the same Go operation and production `tsr_wasm`.
The test-only compiler-wasm acceptance artifact is rejected by this route.

The [first-slice contract](go/CONTRACT.md) fixes a memory snapshot, real config
parsing/loading, one checker, production diagnostic phases, optional emit,
checkpoint before observation, and explicit release. The host compares source,
diagnostic and write bytes; implementation-specific type creation counts remain
separate. Each consumer drops the decoded file transport after preparing its
snapshot. V1 is a correctness prototype with bounded JSON frames.

## Commands

Build the pinned Go native and wasm consumer in a new durable directory:

```sh
python3 tools/phase7/budget/go/build.py --output /absolute/new/go-artifacts
```

The [Rust native consumer](rust/README.md) has an independent manifest. The
production wasm artifact is built through `scripts/s10_build.py --checker`
with the pinned wasm-bindgen CLI. This artifact must expose prepared config,
retained check/emit and owned observation methods; an older checker artifact
fails the transport's export guard.

```sh
TSR_WASM_ARTIFACT_DIR=/absolute/production/checker \
  tools/phase7/budget/production-wasm-consumer < commands.ndjson
node --expose-gc tools/phase7/budget/wasm-smoke.mjs \
  --go-artifacts /absolute/go-artifacts \
  --rust-artifacts /absolute/production/checker \
  --input /absolute/input.json --output /absolute/new/capture
node --test tools/phase7/budget/test-production-wasm.mjs \
  tools/phase7/budget/go/test-instance.mjs
```

The smoke persists the exact production wasm and bindings in its capture,
checks artifact hashes, compares owned work observations, checks the child
transport and terminal version failure, and records the distinct checkpoint
authorities. The focused tests use simulated exports for stage ordering and
trap handling; they are not evidence of a production wasm panic. The existing
browser acceptance record covers an actual invalid-filename precondition trap.

The production Node checkpoint reports wasm linear-memory capacity, process
RSS, pre-observation process peak RSS, V8 heap, external bytes and array-buffer bytes separately. None is labeled
Rust live allocation, and none is compared directly with Go `HeapAlloc`.
Go collects its runtime after preparing the host and at the retained checkpoint.
Both Node frontends also collect V8 after preparation and at that checkpoint.
Preparation collection completes before the execute request, so its cost cannot
leak into the operation interval. Native Rust releases preparation temporaries
through ordinary ownership. The current correctness captures do not establish
accepted performance budgets.

## Large snapshot preparation

`snapshot.py` captures complete metadata and OS identity provenance. Native
discovery reads only files requested by the real config/loader/check/emit
operation, preserving ordered read events, misses, raw physical hashes and
sizes. Discovery is an untimed OS acquisition tool; ordinary consumers use
frozen memory hosts and never fall back to disk. Its CLI and event contract are
shared by the native Go and Rust tools:

```sh
budget-go --discover --metadata metadata.ndjson \
  --input input-with-empty-entries.json --output new-events.ndjson
```

Unioning both read closures must precede final snapshot construction. Known
files without transported content require explicit unavailable-content entries,
so an omitted read fails rather than pretending that the file does not exist.
Version 2 streams metadata batches and bounded content chunks through each
consumer. `prepare_end` verifies entry/file/content counts, contiguous offsets
and every provided raw-file hash before freezing the host. Files whose content
was not supplied remain present and explicitly unreadable. `stream.py` checks
the archived metadata and raw blobs before sending them, then checks identities
again before compilation. Replay reads no original workload file.

```sh
python3 tools/phase7/budget/snapshot.py inventory \
  --root /absolute/provisioned/workload --output /absolute/new/inventory
python3 tools/phase7/budget/snapshot.py freeze \
  --inventory /absolute/inventory --reads /absolute/go-reads.ndjson \
  --reads /absolute/rust-reads.ndjson --output /absolute/new/contents
python3 tools/phase7/budget/stream.py \
  --go /absolute/budget-go --rust /absolute/phase7_rust_budget \
  --inventory /absolute/inventory --contents /absolute/contents \
  --input /absolute/input.json --output /absolute/new/correctness
```

This controller still reports **correctness and diagnostic observations**, not
acceptance performance. `scenario_input.py` selects check or emit options from
the committed descriptors and verifies the provisioned source/install marker;
its `selection.json` retains the complete descriptor identity. Native discovery
is separate from replay and must complete on both implementations. A different
check/emit mode needs its own read closure or a verified union covering both.

Each child records streamed responses in one `responses.ndjson.gz`; its summary
points to the observation rows and their digest. Older uncompressed captures
remain readable. Source and output bytes are not duplicated as an indented
summary. `probe.read_observation` authenticates those rows for later comparisons.
The command log records hashes and
diagnostic round-trip times. Saved binaries and external archive paths preserve
reproduction inputs without placing captures in `target`.

V2 observes a header, then every source and callback write by index. The header
retains roots/options/diagnostics/counts/emit facts; each row preserves the original
owned byte fields. The controller validates every byte before compacting text
into a byte buffer, compares complete work, and retains the raw compressed wire
responses. No child checksum replaces this comparison. Observation occurs only
after the retained checkpoint and performs no additional compiler queries.
Each individual response still has a 256 MiB cap, and each source/write roster
has a one-million-row host limit; exceeding either fails visibly. V1's whole
observation remains a tiny-fixture compatibility route.

All new captures and retained artifacts must be outside every `target`
ancestor and must use a new directory. Build intermediates may use `target`.

## Separate native allocation accounting

`allocation.py` runs seven fresh, alternating native pairs against an explicit
ordinary capture. That reference must have complete, equivalent compiler work
and authenticated artifacts; busy-host or timing-MAD failures do not prevent
allocation accounting. The Go executable must be the same as the reference.
The separate Rust artifact must use `--allocation-metrics`, the same source
closure, and the ordinary release profile and mimalloc allocator.

```sh
python3 tools/phase7/budget/rust/build.py --allocation-metrics \
  --output /absolute/new/instrumented-rust
python3 tools/phase7/budget/allocation.py \
  --reference /absolute/ordinary-native-capture \
  --go /absolute/budget-go --go-build /absolute/go/build.json \
  --rust /absolute/instrumented-rust/phase7_rust_budget \
  --rust-build /absolute/instrumented-rust/build.json \
  --output /absolute/new/allocation-capture
```

The retained ratio compares Go `runtime.MemStats.HeapAlloc` after collection
with Rust live requested allocation layout bytes. These are different heap
accounting authorities; allocator overhead and Rust RSS are not inferred from
requested bytes. Cumulative allocation traffic has its own samples and ratio.
Every sample retains complete owned observations after the checkpoint and must
match the ordinary reference, including a same-runtime checking-work floor.
The reader rechecks raw responses and artifact identities without starting a
child. `scripts/perf.py record` accepts this capture only through explicit
`--allocation-capture`; it never substitutes RSS for retained allocation bytes.
The instrumented run supplies no elapsed, RSS, or executable-size acceptance.

## Separate wasm startup diagnostics

`startup.py` launches fresh processes with the same pinned Node executable and
`--expose-gc --stack-size=4096` flags. It calls each existing frontend's exported
`loadProcessor` once, without preparing a project or checking or emitting code.
One discarded warmup pair precedes seven serial pairs with alternating order.

```sh
python3 tools/phase7/budget/startup.py \
  --go-artifacts /absolute/retained-go-artifacts \
  --rust-artifacts /absolute/retained-production-checker \
  --output /absolute/new/startup-capture
```

`load_processor_ns` includes the loader's artifact reads and hash checks,
bindings loading, wasm compilation and instantiation, and runtime initialization.
It excludes importing the frontend JavaScript module. The separate parent
`launch_to_ready_ns` includes Node startup, that frontend import, the loader,
and delivery of the ready response. Neither number isolates pure module
compilation. The live processor remains retained until the ready response is
flushed; the process then exits. These are warm-cache startup diagnostics with
no project work, budget threshold, or performance acceptance. Raw stdout/stderr,
commands, copied manifests/helpers, and immutable artifact identities remain
in the new capture outside `target`; the referenced artifacts must also be
retained outside `target`. No existing compiler measurement is changed.

## Serial resource capture and readonly recording

Select the committed scenario and operation before discovery/freezing, using
the provisioned workload directory (never a modified project):

```sh
python3 tools/phase7/budget/scenario_input.py xstate check \
  --directory /absolute/provisioned/xstate-checkout \
  --output /absolute/new/selection
```

After the inventory, discovery and freeze commands above, measure the immutable
pair with explicit build manifests. Substitute `--runtime wasm` and artifact
directories for a Node/wasm pair; native arguments are executable paths.

```sh
python3 tools/phase7/budget/measure.py --runtime native \
  --go /absolute/go-artifacts/budget-go \
  --rust /absolute/rust-artifacts/phase7_rust_budget \
  --go-build /absolute/go-artifacts/build.json \
  --rust-build /absolute/rust-artifacts/build.json \
  --inventory /absolute/inventory --contents /absolute/frozen \
  --input /absolute/selection/input.json \
  --selection /absolute/selection/selection.json \
  --output /absolute/new/native-xstate-check
python3 scripts/perf.py record embed-check-xstate \
  --capture /absolute/new/native-xstate-check
```

The controller discards one warmup pair, then captures seven serial alternating
pairs, extending once to fourteen under the S07 elapsed uncertainty rule. It
retains busy and failed rows; quietness and uncertainty determine measurement
eligibility independently of threshold misses. Prototype manifests remain
diagnostic. Production budget consumers require contract 2 with source-guarded
release provenance; ordinary production checker wasm uses its checker manifest
and bound Node transport. The selected descriptor binds config/options and
frozen file/diagnostic counts. Checking-work reduction is compared with each
runtime's own one-checker warmup; the older four-checker CLI type aggregate is
not a one-checker authority. Only the descriptor's declared emit-omission policy
permits the Go subset, with shared bytes and order checked exactly.

Elapsed covers execute request through acknowledgement, including RPC.
Preparation, checkpoint GC and observation are excluded. Process peak before
observation includes launch/preparation/execute; retained RSS is sampled after
the checkpoint. Full-lifecycle peak includes observation and remains separate.
This whole-operation capture has no independent startup acknowledgement;
the separate startup-only runner above measures that boundary. Native retained heap requires the separate instrumented
allocation capture (`perf.py record --allocation-capture ...`); RSS cannot fill
that threshold. `embed-size` and `wasm-size` use `size.py` captures and report
body bytes, with glue and totals separately.

The perf readers replay saved raw observations and recompute comparisons and
ratios without launching a compiler or reading the original project. They
authenticate frozen archives, saved helper/manifests and immutable artifacts.
Capture output is rejected inside the source workload or frozen archives,
including symlink aliases.
