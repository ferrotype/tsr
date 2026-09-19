# ts_arena

Storage and ownership primitives follow the accepted
[ownership contract](../../docs/design/ownership.md). `StorageBuilder<R, S>`
stores a concrete `NodeRecord` directly; its associated auxiliary record type
stores runtime list, slice and file metadata under the same owner. The S04
`FileBuilder<T, S>` and related names remain aliases using the `Node<T>` adapter,
so AST records do not acquire a second header.

## Access and retention

`StorageBuilder` owns mutable core nodes, auxiliary records and symbols. It
already owns lazy storage and a source `PositionMap` cache: `view()` supplies the
same borrowed interface before and after publication. Finishing transfers these
allocations, IDs and initialized caches unchanged into `StorageOwner`.
`PositionMap` is initialized only on its first request. `from_source_text`
preserves the caller's loaded source; the S04 `new(bytes, ...)` adapter decodes
file bytes. Runtime parse completion can retain the exclusive builder for later
binding; publication itself does not prove that binding occurred.
Core reads take no lock and perform no reference-count increment.
`Scope::import` and `FileHandle::node` return checked, borrowed `NodeRef`s. Use
`retain_node`, `import_retained` or `NodeRef::retain` when a result must outlive its
scope. Symbol access follows the same borrow/explicit-retention distinction.

`Scope::with_core_arena` creates a fresh invariant lifetime brand. Checking a raw
id yields a `LocalNode` usable only with that arena. Repeated reads elide the
owner check and retain bounds safety. Compile-fail documentation tests establish
that the brand cannot escape or cross arenas. Ordinary raw ids always receive
owner and published-slot checks, including in release builds.

`BundleOwner` consumes the builders for the canonical file and its supplemental
files, and privately retains an `Arc` for each published member. Mapper links are
non-owning `FileId`s. Every exported mapped `FileHandle` retains the entire
bundle; admitting any member to a scope makes every member resolvable. This
resolution set does not define a program's observable source-file list.

Node, auxiliary and symbol identities are separate `NonZeroU64` types with private
constructors. Both 32-bit fields use their full nonzero ranges. Actual arenas
share one checked process-global counter; injected boundary counters remain
crate-private. Arena-id exhaustion aborts before reuse, and slot exhaustion
panics before truncation. Ids never retain storage. `ScratchOwner` provides
exclusive append and borrowed lookup for request-local work.

## Lazy publication

One standard-library `RwLock` guards both caches, node/auxiliary page directories,
reservations and publication. Misses recheck after acquiring the write guard.
Separately allocated 256-slot `OnceLock` pages preserve published node addresses
when another slot initializes or the directory grows. Page references remain
private: every exported lazy reference also borrows or retains its file/bundle.

JSDoc initialization stages nodes and auxiliary records together before
publication. Its transaction-local resolver reads core, previously published and
staged records without reacquiring the lock. Exclusive construction can seed
validated core JSDoc roots; cache-only lookup never calls an initializer. Every minted
slot is permanently reserved. Errors, invalid roots and initializer unwinds
leave unresolvable tombstones, never reusable ids. Initializer panics are caught
while the write guard remains held, then resumed after unlocking, so valid
existing storage remains usable without ignoring a poisoned lock. Token
initializers use the same unwind boundary. A thread-local guard detects callback
reentry into the same file's lazy APIs and panics before locking; the unwind
boundary preserves retry behavior. Other threads still wait for publication.
Callbacks must not wait for another thread to use the same file's lazy APIs,
including indirect wait cycles involving another file.

JSDoc cache entries share immutable `Arc<[NodeId]>` lists, including empty
results. The token cache uses `(parent, range)` keys, checks cached kinds and
reads reparsed flags from the supplied parent's retained storage. Created
tokens store that supplied parent. `try_token` returns typed invariant errors;
`token` raises the corresponding invariant panics after releasing the lock.

Publication validates that returned JSDoc roots belong to the current
transaction. Metadata and arbitrary payload edges remain the caller's graph
construction responsibility: cyclic links use ids, and every edge followed
later must resolve through a scope retaining its owner. The generic storage
crate cannot inspect arbitrary payloads or prove their semantic graph shape.

## Evidence and limits

The seven `scenarios` functions back the frozen `tests::e3_*` tests and `e3`
example. They measure id boundaries, wrong-owner and stale-id rejection,
concurrent first use and page growth, failed/panicking transaction recovery,
bundle retention in all member-handle release orders, and final disposal.
Additional regressions exercise brands, allocation-free core access, cyclic
graphs, empty-result caching, token unwind recovery and decoded source maps.

`Counters` use independent observation domains. Owner counts cover exclusive and published
files, bundles, scratch owners, checker identities and generation tokens.
Allocation counts cover those owner objects, nonempty core/auxiliary/symbol slabs and
lazy pages. They measure reclamation of explicit storage units; they do not
measure every heap allocation, payload/cache/directory allocation, or RSS.
Each scenario compares final counts to its starting baseline and records a
nonzero observed peak. Separate payload-drop assertions check actual destruction.

```sh
cargo test -p ts_arena --all-features
cargo test -p ts_arena --release --lib
cargo run -p ts_arena --release --features harness --example e3
cargo clippy -p ts_arena --all-targets --all-features -- -D warnings
```

The example emits its measured JSON envelope on stdout; diagnostics and expected
caught panics use stderr. The external evidence runner executes the same arena
tests under strict-provenance Miri and AddressSanitizer and owns those metrics.

`CheckerIdentity` and `CheckerLease` provide identity, exclusive operation and
active-generation primitives only. A lease unwinding through a panic retires
its generation before releasing its permit; all sibling identities then reject
new operations. A panic caught inside a still-held operation requires the caller
to retire that generation. Retirement is distinct from storage disposal.

These leaves do not establish semantic checker correctness, builder/transform
dependency retention, registry/reentry integration or real API handle behavior.
In particular generation validation is not a publication gate: atomic retirement
versus response/registry commitment remains later integration work.

The S06 foundation adds AST/list/text-slice tests in `ts_ast::storage_tests`.
Those cases exercise pre-publication lazy data, transfer to a worker without a
`Sync` requirement on exclusive binder payloads, mapped retention, staged-list
rollback and first-use contention. Running the old seven-scenario example alone
does not establish those additional contracts; instrument them separately.

AST completion and final publication validate all stored core references, including
fields omitted from child enumeration. These are explicit linear passes over
nodes, auxiliary values and their stored edges, repeated after exclusive binder
mutation. Group publication validates against scoped borrows of all prospective
members before publishing any member. Published views keep the complete borrowed
bundle capability when selecting a sibling owner; bare owner views do not grant
foreign imports. Storage alone still cannot validate arbitrary generic payloads.

Published file imports are explicit: `StorageBuilder::retain_file` retains the
complete source bundle and flattens its existing imports into an arena lookup
index. Repeated imports of an already retained file do not grow that index.
Importing allocates bookkeeping and clones retention roots; ordinary core reads
borrow from the index without cloning an owner. Inputs must already be published,
so a new owner can only depend on older owners and cannot form an ownership cycle.
Dropping its last retained handle can release a transitive chain of imported
owners; retaining an imported mapped member keeps every member of that bundle.
