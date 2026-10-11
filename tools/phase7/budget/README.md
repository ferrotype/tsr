# Production embedding budget foundation

These tools currently establish comparable compiler work on bounded inputs.
They do not establish timing, retained-memory, allocation, or size budgets.
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
