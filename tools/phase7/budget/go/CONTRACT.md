# Explicit budget operation contract

Version 1 is implemented by the tiny prototype. Its implementation contract
follows the production diagnostic schedule, uses builtin libraries and takes
the retained checkpoint before observation serialization. Remaining
large-scenario and production wasm gaps do not count as completed budgets.

## Version 2 streamed preparation

Version 2 uses the same command envelope and canonical decimal operation ID.
Every JSON frame is at most 2 MiB. Preparation commands have null response
values and the following exact extra fields and response kinds:

| Command | Extra fields | Response kind |
| --- | --- | --- |
| `prepare_begin` | `input` (v1 input with `entries:[]`), `expected_entries`, `expected_files`, `expected_contents` | `preparing` |
| `prepare_metadata` | `entries` (array of compact metadata records, encoded array at most 1 MiB) | `metadata` |
| `content_begin` | `path_b64`, `raw_size`, `sha256` | `content_started` |
| `content_chunk` | `offset`, `bytes_b64` (at most 1 MiB decoded) | `content_chunk` |
| `content_end` | none | `content_verified` |
| `prepare_end` | none | `prepared` |

Counts, sizes and offsets are canonical unsigned 64-bit decimal strings.
Base64 is standard padded canonical base64. SHA-256 is lowercase hexadecimal.
Metadata records are the discovery manifest's headerless rows: `directory`
has `path_b64`; `physical_file` has `path_b64` and `raw_size`; `symlink` has
`path_b64` and `target_b64`. The controller separately verifies the complete
metadata manifest, scope header and physical identity provenance. Consumers
operate solely on the supplied frozen metadata and content.

Metadata must finish before content begins. Canonical metadata paths and
content paths must be unique. A content stream names a known physical-file
placeholder, repeats its exact raw size, uses contiguous offsets starting at
zero, and finishes only after its byte count and SHA-256 match. Only one file
stream is open at a time. Metadata entries, physical files and completed
content streams must match the three declared counts before `prepare_end`
freezes the host. Other physical-file placeholders remain visibly present
and fail explicitly if read. No duplicate full entry payload is retained.

`execute`, `checkpoint`, `observe` and `release` retain their v1 fields and
response kinds, with `version:2`. Malformed input or a failed content hash is
terminal; a later frame cannot repair an invalid stream into a successful row.
Whole-observation JSON remains bounded independently of streamed preparation.

After checkpoint, v2 also accepts `observe_begin` (no extra fields),
`observe_source {index}` and `observe_write {index}`. Indices are canonical
unsigned 64-bit decimal strings, zero-based. Response kinds are
`observation_begin`, `observation_source` and `observation_write`. Begin returns
the original observation except `sources`/`writes`, adding `source_count` and
`write_count` decimal strings; rows retain their original representation.
Repeated begin/row requests reuse a lazily frozen post-checkpoint observation.
Release drops it. Version-1 `observe` remains available. Production wasm uses
metadata/source identities and serializes one source/write at a time, avoiding
whole-observation JSON before the bounded transport.

Current prototype capability limits fail explicitly: Go's `fs.FileInfo.Size`
uses signed 64-bit sizes, so metadata above `MaxInt64` is rejected. The Node
production adapter limits supplied physical content to signed source-position
lengths and refuses case-insensitive non-ASCII metadata paths until it can use
the compiler's canonicalizer directly. Unknown file bytes are never replaced
with empty content to satisfy these limits.

## Representation and validation

Every byte string is an array of integers in `[0,255]`; names, paths, input
contents and output contents are never decoded through a JavaScript string.
Go's default JSON encoding of `[]byte` is base64 and must not accidentally
become the wire format. UTF-8 decoding applies only where a production parser
requires it. Raw source bytes and already-loaded text are distinct values.

Commands and results are tagged JSON objects. Unknown fields, duplicate
fields, malformed bytes, invalid state transitions and unknown versions fail
closed. Identifiers and potentially 64-bit metrics use canonical decimal
strings, avoiding JavaScript integer rounding. Object ordering must not
replace ordered argument, insertion, diagnostic or write arrays.

The native front end uses synchronous NDJSON; the JS front end exposes the
same commands as synchronous calls. One prepared operation per consumer is
sufficient. A panic/trap invalidates the instance and all retained handles;
the host must discard it and start a fresh process/wasm instance, without
retrying the failed observation as a successful row.

## Input proposal

`prepare` supplies `{version:1,command:"prepare",id,input:{...}}` before the
measured operation. The input contains these fields:

| Field | Proposed meaning |
| --- | --- |
| `current_directory` | Normalized absolute path bytes, validated before use. |
| `case_sensitive` | Snapshot filesystem's explicit boolean policy. |
| `entries` | Ordered snapshot insertion events, described below. |
| `configuration` | Config path plus ordered command-line override arguments. |
| `operation` | `check` or `check_emit`; one checker in both modes. |

An insertion event is one of:

- `directory { path }` for explicit directories, including empty directories.
- `physical_file { path, bytes }` for on-disk bytes. `ReadFile` must use the
  pinned physical loader's BOM/UTF-16/UTF-8 behavior, not a guessed decoder.
- `symlink { path, target }` with its explicit target path bytes.

Prepare explicitly inserts the current directory before the supplied events;
the native Rust adapter does the same. This is adapter policy, not behavior
inherent to an empty `MemoryBuilder`. Insertion order is retained; overwrite,
implied-directory and root-to-leaf
relative-link resolution follow Rust's `MemoryBuilder`, with 40 link hops.
The first prototype accepts normalized absolute POSIX entry paths only;
unsupported path forms fail explicitly. Symlink targets may be relative.
`Stat`, accessible entries, realpath and walking must behave consistently;
implementing only `ReadFile`/`FileExists` is insufficient for config expansion
and module resolution. Write methods of the input FS return explicit errors;
emit writes use a separate callback and never mutate the input snapshot.

Builtin libraries use production `bundled.WrapFS`/`bundled.LibPath` on Go and
production `BundledFs` on Rust. Their loaded text and filesystem behavior are
observed as production values, without a second physical decoding. Explicit
library transport is not in this slice; add it only if a real scenario needs
it, after matching production prefix/case/directory/stat/realpath semantics.

`configuration` is `{ config_path, arguments }`, with every argument a byte
string in its original order. The real parser reads the config and all
`extends`, include/exclude and referenced-config dependencies from the
snapshot. Overrides use the pinned command-line parser and existing option
precedence. The first slice has no pre-resolved-options mode: it must not
flatten configs or project references into roots and lose their semantics.
Argument ordering and supported project references require tiny witnesses.

`check_emit` selects all files, ordinary full emit, with the compiler's
ordinary no-emit/error gates. Unsupported partial-file/force/signature modes
have no fields in this protocol and fail validation until
explicitly implemented; the five scenarios must not silently select them.
Incremental/build-mode and external content mappers are unsupported in the
first slice. If a selected scenario requires one, report a capability failure
instead of omitting it or spawning an unrecorded host process.

## Operation and retention

States are `empty → prepared → retained → checkpointed → empty`. Each command
after prepare is `{version:1,command,id}`. Responses are
`{version:1,id,kind,value}`. Acknowledgements use a null `value`; checkpoint
and observation carry their owned values. Errors use `kind:"failure"` and a
text value, terminate the native child and invalidate the JS consumer.

1. `prepare` validates and constructs the frozen snapshot. It creates no
   compiler program and performs no config parse, bind or check. Go collects
   its runtime after successful prepare/prepare_end; both Node adapters
   collect V8 there. These collections occur before the prepare acknowledgement
   and outside the measured execute request/ack boundary.
2. `execute { id }` creates a fresh host, parses config, loads a fresh program,
   runs the agreed diagnostic phases and optionally emits. It retains the
   input snapshot, parsed configuration, program, diagnostic objects, emitted
   write strings and emit result. The response is a minimal acknowledgement;
   observation formatting/hashing is a separate command.
3. `checkpoint { id }` records a declared collection/metrics checkpoint while
   all agreed positive roots remain reachable. Go calls `runtime.GC`, reads
   `runtime.MemStats`, then calls `runtime.KeepAlive` on the retained operation
   after the final metric read. The JS front end also reports wasm linear
   memory size; the outer Node process's retention and GC policy remain
   separate. Both Node adapters collect V8 at checkpoint, with the explicit
   policy `v8_after_prepare_and_at_checkpoint`. Their outer `process_peak_rss_bytes`
   uses Node resourceUsage maxRSS (KiB converted to bytes) before observation;
   it includes startup/preparation/execute/collection and is distinct from the
   whole-process lifecycle peak. Native Go adds
   `process_peak_rss_before_observation_bytes` with authority
   `getrusage(RUSAGE_SELF).ru_maxrss`; macOS bytes/Linux KiB are normalized.
   Native-only peak fields never enter the nested wasm Go runtime metrics.
   Returning metrics may allocate after the sampled point.
4. `observe { id }` returns owned work facts and byte outputs without running
   additional compiler queries or phases. Serialization and any host hashing
   happen after the retained checkpoint and outside execute. Observation DTOs
   are frozen only after checkpoint and released with the operation. Observe
   before checkpoint fails.
5. `release { id }` drops every retained operation root. No compiler values
   or result closures may remain in global maps. Optional post-release GC
   belongs to a separately labeled observation, not the retained checkpoint.

Configuration read/parse failures are structured operation outcomes, not
successful zero-work checks. Their sources/diagnostics remain observable.
Unsupported capabilities, cancellation, input schema errors and traps have
distinct failure outcomes; none becomes an empty successful result.

The fixed phase schedule matches production wasm: Config, Program, Syntactic,
Global, then per loaded file Semantic and Declaration when declaration or
composite is enabled, followed by production sort/deduplication. There is no
error-dependent early exit, explicit Bind phase or second Global phase.
Calling the pinned CLI-like diagnostic helper only on Go would introduce a
mismatch. Production wasm must also use the same one-checker pool for checking
and emit; its previous separate query checker is not an equivalent authority.

## Owned observations proposal

The host receives ordered, byte-preserving facts suitable for independent
hashing, not just an unverified checksum reported by the child:

- Parsed roots/options/config dependency identities, loaded source names,
  canonical paths and loaded texts; default-library membership and missing
  dependency diagnostics remain distinguishable.
- Final sorted/deduplicated structured diagnostics and their order. Preserve source indices,
  ranges, code/category, message identity/arguments, nested chains and related
  records rather than relying only on formatted text or a total count.
- Source/node/identifier counts where available, actual checker count and
  production `Program.TypeCount`, with unsupported observations explicit.
- Every callback write's name/content in order, the emitted-file list,
  emit-skipped status and emit diagnostics. Map/build-info facts must be
  declared if the scenario uses them; do not silently omit them.
- Retained-checkpoint `HeapAlloc`, `HeapInuse`, `HeapSys`, `StackInuse`,
  `StackSys`, `Sys`, `TotalAlloc`, `Mallocs`, `Frees` and GC count, as decimal
  strings. These are Go runtime facts with their own units/meaning, not
  directly interchangeable with Rust allocator live/requested bytes.

Observe and checkpoint ordering, temporary serialization retention and any
child-internal hashes need controls showing unchanged compiler work. The
host's canonical hashing rule is not defined by this draft. Timing, peak RSS,
live RSS, allocation traffic, retained heap and wasm linear memory must remain
separate measurements.

## Engineering verification and implementation remaining

- Extend the passing tiny/Xstate schedule and configuration controls to all
  five scenarios in check and emit modes. Production wasm retained checking
  and emit already separate compilation from observation.
- Verify physical decoding, ordered insertion and builtin bundled behavior
  after tiny BOM/case/realpath/missing-library controls.
- Verify configuration override and project-reference behavior from the five
  scenario descriptors without inheriting their existing multi-checker counts.
- Scale snapshot transport without extension filtering or treating unread
  known files as absent. The tiny NDJSON front end is limited to 256 MiB;
  integer-array JSON of every provisioned MUI file is not a viable foundation
  for a large-scenario retained-memory comparison. Version-2 metadata/content
  streaming is implemented; bounded observation rows require actual large
  replay and equal-work verification.
- Verify Go native versus Go wasm runtime initialization/GC controls and
  record them, rather than charging one side's startup to the other side's
  steady operation.
- Define a separate capability-equivalent size artifact, without allocator
  observation front ends. Record included glue/runtime bytes explicitly.
