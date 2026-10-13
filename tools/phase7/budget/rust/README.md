# Rust compiler budget consumer foundation

This independent, private executable performs production config parsing and
compilation through the public `tsr_embed` API. It has no dependency on the
compiler acceptance runner, corpus walkers, or acceptance protocol. The v1 JSON integer-array transport supports tiny-fixture controls; v2 streams
complete metadata and only the acquired raw content closure in bounded frames.
These transports are correctness foundations, not performance acceptance evidence.

From the repository root, preserve a guarded release artifact in a new directory
outside `target`:

```sh
python3 tools/phase7/budget/rust/build.py --output /absolute/new/native-budget-artifact
```

The builder records the exact Cargo executable, dependency-source fingerprint,
compiler/build settings and executable digest. The ordinary artifact uses pinned
mimalloc 0.1.48 without accounting hooks and the native production release profile:
unwind panics, fat LTO and one codegen unit. Add `--allocation-metrics` with a
different output directory to preserve a separately instrumented artifact wrapping
the same allocator. The builds may share the Cargo cache; the builder copies each
artifact before a later build can overwrite the mutable target executable.
Instrumented artifacts cannot qualify timing or binary-size acceptance. A valid
build identity establishes the artifact contract, not a passing performance budget.

The NDJSON command and observation fields match
[`../go/CONTRACT.md`](../go/CONTRACT.md) and the concrete Go body DTO. Each process
holds one active operation. The required order is `prepare`, `execute`, `checkpoint`,
`observe`, `release`; repeated observation is allowed after checkpoint. A failed
command or panic terminates the process. A released identifier may be reused.
Input paths must be normalized absolute POSIX paths. Entries preserve their
insertion order, case policy, physical byte content, directories and symlinks.
The filesystem has production builtin bundled libraries and no operating-system
fallback.

Preparation builds and retains the input snapshot. Execution parses command-line
compiler overrides, resolves the config with `CompilerConfigHost`, loads a
single-threaded `Session`, requests config/program/syntactic/global diagnostics,
then semantic and (when declaration or composite is enabled) declaration
diagnostics for each program file. It uses the program's production diagnostic
sort/deduplication. `check_emit` additionally performs ordinary all-file emit.
Incremental, watch, external content mapper execution, CLI roots and CLI project
selection fail explicitly in this prototype.

The retained roots at checkpoint are the cwd/configuration request, filesystem
snapshot, parsed configuration, session/program/compiler pool, diagnostics and
optional emit output. The original physical-byte transport payload is consumed
and dropped during preparation; it is not a second retained copy. Observation materializes source/diagnostic/output DTOs only afterward.
The checkpoint reports storage owner/allocation counts. Those counts are not
heap bytes and cannot be compared with Go `HeapAlloc`. The optional allocator
reports live/total/peak **requested** and **usable** bytes plus event counts
across the process. Usable bytes come from mimalloc’s `mi_usable_size` through
its feature-gated extended API; this feature is enabled only in the instrumented
consumer. Neither series is RSS or an operation-only delta. Its
snapshot is sampled before allocating the checkpoint response. External process
RSS measurements and allocator measurements require separate authorities and
phase labels; the prototype does not certify a budget.

Focused checks:

```sh
cargo test --offline --manifest-path tools/phase7/budget/rust/Cargo.toml --target-dir target
cargo test --offline --manifest-path tools/phase7/budget/rust/Cargo.toml --target-dir target --features allocation-metrics
cargo clippy --offline --manifest-path tools/phase7/budget/rust/Cargo.toml --target-dir target --all-targets --all-features -- -D warnings
```

Untimed native content discovery uses a complete metadata inventory and the
same configured check/emit operation:

```sh
phase7_rust_budget --discover --metadata metadata.ndjson \
  --input input.json --output NEW_EVENTS.ndjson
```

The input is the current request input with `entries: []`. The metadata header
is `{version:1,kind:"header",root_b64:...}` followed by ordered directory,
physical-file (`raw_size` decimal string), and symlink (`target_b64`) entries
with `path_b64`. Paths retain their absolute byte spelling. Lookup and directory
queries use the frozen metadata. Only actual `ReadFile` requests acquire raw OS
content; they record requested/canonical paths, found/missing, raw size and
SHA-256. Builtin bundled reads are outside this logger. Reads are cached once
per physical path during discovery; the host must validate all recorded hashes
and metadata identities before producing a frozen replay archive.

Discovery is an explicit live host (`snapshot_id() == None`), uses production
`CompilerConfigHost::new_live`, and is never a measured operation. Raw reads
outside the declared scope fail. Recorded dangling external symlink targets
remain missing only while an OS check confirms that state; existing external
resources are rejected. The log ends with a complete event/count only after the
operation succeeds; failures are terminal events. Output files are never
overwritten.

V2 replay replaces `prepare` with `prepare_begin`, one or more
`prepare_metadata` frames, zero or more `content_begin` / `content_chunk` /
`content_end` sequences, then `prepare_end`. The remaining lifecycle is unchanged
and every command must retain version 2 and the same identifier. Counts include
all headerless metadata entries, all physical file placeholders, and supplied
content paths, respectively. Chunk offsets are contiguous canonical decimal
strings; base64 is canonical and each completed file must match its raw size and
SHA-256. Encoded metadata arrays are at most 1 MiB, decoded chunks at most 1 MiB, and all
v2 command frames at most 2 MiB. Duplicate canonical paths, count/hash/size
mismatches and ordering errors terminate the consumer. Unsupplied physical files
retain their stat/directory identity and fail explicitly if compilation reads
them. No replay operation consults the OS. Preparation staging and lookup maps
are dropped before execution; only the immutable snapshot and operation request
remain retained.

After a v2 checkpoint, `observe_begin` returns the original observation fields
except `sources` and `writes`, with canonical decimal `source_count` and
`write_count`. `observe_source` and `observe_write` take a zero-based canonical
`index` string and return one original owned row. Response kinds are
`observation_begin`, `observation_source` and `observation_write`. These commands
lazily cache the owned observation after checkpoint, perform no additional
compiler queries, and may be repeated. Release drops the cache. The original v1
`observe` command remains available. This limits individual response frames; it
does not yet avoid the post-checkpoint cache's full owned source/output bytes.

Native macOS/Linux checkpoints also sample
`process_peak_rss_before_observation_bytes` using
`getrusage(RUSAGE_SELF).ru_maxrss`, before allocating the response or materializing
observations. macOS reports bytes; Linux KiB are converted with checked
arithmetic. This process high-water mark includes startup, preparation and
execution. It is not a retained-only or compiler-only RSS measurement. A failed
or unsupported sampling call fails explicitly; it never reports zero as a
substitute. Final process peaks that include observation serialization belong
to a separate authority and cannot replace this checkpoint value.


Use a fresh process for each measured operation. Although the protocol permits
another preparation after `release`, `getrusage` high-water marks and the optional
allocator's total/peak/event counters do not reset on release. Reusing the process
would mix earlier preparation, compilation and observation peaks into a later
checkpoint. Live requested bytes are also process-wide, including runtime and
transport allocations; they are not a compiler-only retained-byte measure.
