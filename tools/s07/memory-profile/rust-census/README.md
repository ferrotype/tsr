# Retained Rust storage census

This tooling appends safe read-only observers to a disposable copy of the Rust
crates. It never changes the production checkout. Run after the native retained
snapshot, with workers stopped and all endpoint roots alive:

```sh
python3 tools/s07/memory-profile/rust-census/apply.py --stage STAGED_REPO
python3 -m unittest discover -s tools/s07/memory-profile/rust-census -p 'test_*.py'
```

The destination must already contain copied crates. The script rejects the
production tree, symlink escapes, aliased source files and repeated application.
It validates all destinations before writing. `census.patch` preserves the exact
additive source changes; `census-manifest.json` records every before/after hash,
the patch hash and generator-source hashes. No struct fields, record layouts,
allocation sites or parse/bind operations are changed.

The staged APIs are:

```rust
let mut census = tsr_jsstring::census::Report::default();
for worker_files in &retained {
    for file in worker_files {
        tsr_ast::add_retained_file(file, &mut census);
    }
}
let json = census.json();
```

`tsr_ast::retained_census(&[BoundFile])` is the single-slice convenience API.
`Report` exposes `rows` publicly. The driver can add preloaded `SourceText` and
`JsString` objects through `tsr_jsstring::census::Walk::walk` into the **same**
collector, retaining shared-backing deduplication. Counting a retained root does
not count the driver's containing Vec, input metadata, channels, worker stacks
or runtime state. Those are distinct native-snapshot/adapter obligations.

Each row reports container/known-allocation counts, element count/capacity,
used and capacity payload bytes, shared references, a separate Arc header
estimate, and the count of containers whose private layout remains unreported.
Rows are additive for payload bytes; reference/record-only rows add no bytes.
Arena page directories and value pages are separate. Inline Node and Aux enum
storage is charged by arena capacity; boxed NodeData payload uses the actual
variant's `size_of_val`, then traverses owned children. Every allocated arena
record is visited, including obsolete declaration-list backings and nodes no
longer reachable by a semantic edge. IDs and slice descriptors never follow
edges or retain owners. Binding overlays, general binding maps and paged flow
bindings are separate categories.

Vec used bytes mean initialized elements; capacity includes its unused slots.
NodeSlots additionally reports occupied slots separately from page capacity.
Boxed slices have exact physical length; a declaration backing's initialized
nil tail is stored payload, even when no current slice exposes it. Lazy pages
include all OnceLock slots and separately count initialized/reserved slots.
The census reads existing locks and OnceLock values without initializing caches.

Shared Arc allocation identity is the live data pointer, without pointer
reconstruction. Full backing storage is charged once, including source byte
allocations shared with substrings. The first encountered owning field receives
its payload charge; later references are counted without bytes. Thus a field's
shared-byte charge is **not** a claim that this field uniquely owns those bytes.
Two Arc counters are reported as a separate estimate: Arc layout is not a public
ABI, and allocator alignment/size-class rounding is not inferred.

HashMap reports occupied tuple payload and public capacity-times-tuple-size.
Public capacity is a guaranteed-entry count, not the bucket count. Unreported
buckets, control bytes and padding remain explicit. BTreeMap reports occupied
key/value payload only: node count/capacity, child edges and padding are unknown,
so its allocation count is not invented. These lower bounds are **not** malloc
live bytes or RSS. Allocator metadata, free/retained pages, thread caches, code,
stacks, statics and census scratch allocations remain outside the graph census.

The generated NodeData observer matches every variant exhaustively and rejects
unknown ownership-bearing payload field types. Its checks are census contracts,
not Go parity or acceptance benchmarks. Build/test and report provenance must
bind the staged patch and executable independently from the untouched acceptance
artifacts.

## Reading the categories

| Category prefix | Retained allocation payload |
| --- | --- |
| `core.nodes.page_payload` | All initialized core Node records and their arena slot capacity |
| `core.auxiliary.page_payload` | Inline auxiliary enums and their arena slot capacity |
| `NodeData.*.owned` | Boxes for actual boxed payload variants; inline variants add no heap bytes |
| `AstStorageData.Nodes/Text.owned` | Separate boxed node-ID or JsString-header arrays |
| `AstStorageData.SourceFiles.owned` | Occupied source-state map key/value payload; tree-node capacity remains unknown |
| `SourceMetadataData.*.owned` | Separate typed metadata backing arrays |
| `BindResult.nodes` | Sparse overlay table key/Node payload and public entry capacity |
| `BindResult.bindings` | General NodeId/NodeBinding table payload and public entry capacity |
| `BindResult.flow_bindings.*` | Paged flow-only side fields, page directory and any foreign-owner map |
| `BindResult.symbols.*` | Symbol arena records and page directories |
| `SymbolTables.0.*` | Arena storage for HashMap headers plus the separately allocated map key/value payload |
| `DeclarationLists.0.*` | Arena storage for Box headers plus every separately allocated declaration backing |
| `FlowNodes.0.*`, `FlowLists.0.*` | Flow-node and linked-flow-list arenas |
| `JsString.storage`, `Pattern.text` | Deduplicated complete shared byte backing allocations |
| `SourceFileState.*`, `Diagnostic.*`, `BindResult.diagnostics` | Existing metadata, cache and diagnostic child allocations |
| `owner.*`, `lazy.*`, `arena.tracking` | Retention roots/directories, existing lazy storage/caches and counter-state allocations |

Every `.page_directory` row is separate from its value pages. All `.records`
rows are observation counts with zero bytes. Summing NodeData record counts
includes core nodes **and** separately allocated overlay nodes; it is not the
parser's node count. The core arena element count is the corresponding parser
allocation inventory. Auxiliary variant counts likewise reconcile against the
core and initialized lazy auxiliary arena records.

`SymbolTables.0.page_payload` and `DeclarationLists.0.page_payload` combine two
allocation layers. Their bytes are disjoint and additive: the outer arena stores
container headers, while each populated container separately owns its buffer.
Their element/container counts mix these layers, so deriving one element size
or one occupancy percentage from those rows would be misleading. The same
caution applies to a lazy cache row that contains both BTree payload and shared
cached-root arrays. Diagnostic Arc allocations reached through multiple chains
are charged once through the same identity set as other shared backings.
