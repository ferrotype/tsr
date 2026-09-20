# Staged Rust allocation regions

This diagnostic instruments a disposable Rust source copy after `rust-census`.
It does not edit production crates, change their representations, add object
headers, replace mimalloc, or claim an acceptance timing result.

```
python3 tools/s07/memory-profile/rust-sites/apply.py --stage STAGED_REPOSITORY
cargo run --manifest-path STAGED_REPOSITORY/Cargo.toml -p tsr_jsstring --example memory_sites_probe
```

The applier requires the census manifest, checks every modified Rust file still
starts with the production bytes, rejects repeated application and paths that
alias production, and validates the complete patch before writing. Its output
`sites.patch` and `sites-manifest.json` retain before/after hashes, generator
hashes, all source occurrences and the allocation dependency's crate checksum.
Generated payload coverage is an exact checked inventory of 37 existing
`Box::new(data)` conversions; a changed inventory fails generation.

The stage adds `alloc_tracker = "=0.5.25"`, with default features disabled, to
`tsr_jsstring`. The MIT-licensed dependency has no runtime dependencies. Its
generic `Allocator` forwards original allocation layouts and all four native
operations, including `alloc_zeroed` and `realloc`. The driver can declare a
global `cap::Cap<alloc_tracker::Allocator<mimalloc::MiMalloc>>`, allowing safe
direct access to cap's requested/live counters. All workspace instrumentation
uses safe APIs; no custom `GlobalAlloc` implementation or allocator FFI is added.

Driver API in staged `tsr_jsstring::memory_sites`:

- `initialize()` pre-registers every operation and warms every operation's
  metrics lock through its safe getter; call before the first baseline. At the
  pinned macOS std implementation, those mutexes allocate backing on first
  lock. Cold observer locks would otherwise be charged to enclosing scopes.
- `initialize_thread()` warms each worker's counter and scope TLS before work.
- `phase(Phase::{Preload,Parse,Publish,Bind,Retire})` returns an optional RAII
  guard; keep it alive for the complete corresponding synchronous phase.
- `report() -> Vec<Row>` reads totals after every worker and scope is quiescent.
  Report construction allocates and belongs after native snapshots and counters.

The driver supplies its actual per-file parse, publication and bind boundaries;
work remains interleaved per file. Phase guards may nest only outside source
regions, restore their previous thread phase when dropped, and cannot move to
another thread. Site guards similarly remain on their originating thread.
Scope code is inert before initialization. No phase is inferred from a symbol
name or the thread on which an object is eventually freed.

Every output row has `name`, `phase`, `source_file`, `source_line`, `scope_kind`,
`requested_bytes`, `allocation_calls` and `observations`. `source_line` is the
first production occurrence; the manifest records additional occurrences.
An observation is one completed guard, including calls that allocated nothing.
Rows are sorted deterministically by phase, scope kind and name, and zero rows
remain present. Scope kinds have different aggregation rules:

- `inclusive_site`: requests made within one selected source region, including
  calls to nested regions. These rows overlap and must not be summed.
- `selected_union`: a thread-local nesting counter measures only outermost
  selected regions. Nested sites count once in this union, even when the same
  source function recurses. This is a union of executed source regions, not
  a partition of allocation-stack leaves.
- `phase`: all requests made on that thread during a driver phase. Sum completed
  worker guards for the process's phase total. For properly enclosed regions,
  `phase - selected_union` is the unclassified request remainder. Check the
  subtraction for underflow. `unscoped` source rows have no enclosing phase
  denominator and therefore no derived remainder.

The 58 regions cover arena pages and page directories, core node/auxiliary
slots, flow side slots, full binding and header-overlay maps, declaration
backing, symbol slots/table inserts, flow nodes/lists, string Arc construction,
scanner cooked text/number caches, slice compaction, and each boxed payload
conversion. The current parser/scanner share borrowed source ranges and cooked
storage; no nonexistent general string interner is invented. Default-derived
payload clones, list construction in parser loops, miscellaneous hash maps,
diagnostics, snapshots and runtime allocations can remain outside selected
regions. They remain in their enclosing phase total and its remainder.

These are **requested allocation bytes and call counts**, not copied bytes,
malloc usable size, free traffic, live bytes or backtrace observations.
`alloc_tracker` counts before forwarding, including unsuccessful requests, and
counts the complete new size of each reallocation, even shrink/in-place cases.
Accepted full-work rows must have completed without allocation failure. Cap
counts successful requests and includes the tracker's small counter/registry
allocations; the tracker intentionally skips its recursive TLS registration.
Prewarming keeps those registration differences outside the measured phase.
No `.iterations(n)` normalization is used, because that API truncates byte and
call remainders. Span recording uses reference counts and locks and perturbs
execution; this binary cannot supply native timing or RSS acceptance values.

The separate retained-object census and cap live-byte snapshots answer different
questions. Subtracting retained bytes from cumulative requests is not pure
pipeline churn when preloaded allocations can be freed or reallocated. The
difference between resident memory and requested/census bytes remains
unattributed; it cannot be labelled measured fragmentation or allocator overhead.

The standalone probe checks exact zeroed/nonzero request bytes, growth and
shrinking reallocation, inclusive parent/child overlap versus union, a phase
remainder, eight concurrent thread scopes and cross-thread retirement. It uses
System only to isolate the wrapper's accounting. The driver's native mimalloc
control, cap reconciliation, frozen workload identities and retained endpoints
must additionally qualify a real workload capture.
