# Recorded access workload for the compact node slice

Date: 2026-09-08. This replaces the small fixture as the **performance driver**
of the next node-layout comparison. Small fixtures remain semantic regressions.
The [first capture results](S07-bis-access-trace-results.md) now establish the
scoped observation milestone below. Layout timing remains pending.

2026-09-09 decision: **further field-level capture and layout replay are deferred**.
The approved [primary sequence](S07-bis-performance-plan.md#1-decision) chooses
typed rows for the first integrated storage candidate after one bounded lookup
trial. The remaining stages below preserve the earlier design as a conditional
diagnostic reference; they are no longer integration prerequisites.

Implementation boundary for the first recorder: freeze the accepted CP1 bundle
(`3a57976f667b9857891edbe9f76de5261c43480ac8c62d946b4545d9ee19d931`),
export physical syntax before each bind, and capture the explicitly named
ID-qualified operations. This first milestone does not reconstruct complete
binding state, record every payload-field read, or rank layouts. Its registry
lists excluded metadata, caches, copies, births and accessor families. A native
retained-graph comparison with the original accepted executable must pass on
exactly the selected inputs before the capture is accepted as an observation.

The initial host has about 10 GiB free. Stream binary blocks through gzip level1;
retain no uncompressed trace file. Declare a 32 GiB record-payload limit, a 3 GiB
compressed-output limit and a 1 GiB free-space reserve before capture. Records
and blocks are bounded at 1 MiB; logical text blobs use at most64 KiB chunks.
Any overflow or write failure preserves an incomplete capture with its child
exit receipt. First measure volume on the explicitly labelled union of the first
16 and largest16 workload inputs, in original order. This sizing subset is not
a full-workload capture or a CPU sample, and extrapolation from it is not a
guarantee that the full workload fits. Keep the limits active on the complete
13,094-file attempt; do not silently filter events or files to stay within them.

The verified sizing capture contains23,574,043 records for1,558,205 nodes. Scaling
its bytes by node count suggests about16 GB raw and1.71 GB compressed for the
full workload; this is a capacity estimate, not a guarantee. The Python reference
verifier takes minutes even on this subset. Full recording may therefore seal
its raw invocation and graph comparisons as **pending trace verification**, with
an explicit incompatible manifest kind. A separate native decoder must match
the Python reference on malformed fixtures and the recorded sizing trace before
verifying the full stream. Its binary, registry, input and result hashes must be
recorded separately. A pending recording is never a verified trace or gate result.

Fable is right that population counts and a tiny fixture omit working-set size
and real access order. Record actual binder operations from a separately staged,
instrumented copy of the accepted implementation over the full frozen workload.
Use the latest retained candidate as the source; record its immutable manifest,
patch, actual executable, tools and loaded-input identity.

An operation trace gives access-frequency/order coverage for its instrumented
surface. It does **not** by itself give CPU coverage: it omits uninstrumented
symbol/string/control work, and recording changes execution/cache behavior.
Trace replay also has its own decoding and memory traffic. Keep these limits
explicit when interpreting any layout speed difference.

## Capture contract

The tooling home is `tools/s07/performance-experiments/access-trace/`, with
`stage.py` for checked observer additions and `probe.py build/capture/verify` for
immutable artifacts. Store the operation registry and format version there;
do not widen `cargo xtask gen` or leave tracing on production hot paths.

1. Freeze all 13,094 files and their original order/options. Capture one-worker
   operation order first. Mark file and parse/bind/publication boundaries, and
   retain ordinary endpoint roots. Do not invent a deterministic global
   eight-worker interleaving or claim that serial replay measures contention.
2. Preserve the full physical initial state needed to reconstruct each candidate:
   node identity/allocation order, kind and shape separately, scalar fields,
   lists and shared subranges, text/backing identity, source bytes and required
   binding state. Include unvisited/obsolete storage in working-set accounting.
   Births during binding, including lazy records, must be explicit events.
3. Instrument actual reads/writes: header/kind/flags, payload field reads,
   child-list resolution and enumeration, and narrow flag/symbol/flow/locals/
   container writes. Include observed inputs/results and operation identity so
   replay can detect a changed value or order. Never replace a real field read
   with an unconditional synthetic read and call it captured behavior.
4. Use owner-qualified stable local node/list identities, not runtime IDs whose
   observation would allocate or initialize them. Copies and temporary values
   need explicit provenance; unmapped operations are counted and reported,
   never silently attributed to the preceding node. Define nil, foreign,
   imported, failed and lazy identities in the format.
5. Use per-owner/worker sequence numbers and bounded binary output blocks with
   lengths, hashes and a completion footer. Estimate event volume before full
   capture and enforce a declared byte limit; exceeding it produces an incomplete
   diagnostic, not a sampled or truncated successful trace. Preserve observer
   failures and never silently filter files, operations or shapes to fit.
6. Count captured operations by type, shape, caller/phase and owner independently
   of population counts. Match every workload input and resulting graph/work
   observation to the accepted semantic protocol. Check event ordering, births,
   references, values and final state; include missing/duplicate/reordered event
   counterexamples in the producer tests.

The staged observer should reuse the owner-census access-only pattern where it
fits, but must inventory actual accessor instrumentation rather than treating
every `Binder::n()` resolution as a read of every payload field. Cover helpers
that bypass `Binder::n()` through `AstView`, subtree-facts and generated accessors.
List every uninstrumented surface. Report an unknown-operation count only where
a separate observation measures it; otherwise mark the count unavailable. Preserve facts'
current sequentially consistent atomics and never materialize lazy/runtime state
merely to obtain a trace value.

## Replay and integration decision

Reconstruct current storage, typed rows and mixed plain/atomic rows with identical
logical state and matched list/text policies. Page-256 is the leading list
candidate, with page-64 a bounded memory challenger. The nine proposed shapes are
a first implementation slice, not sufficient CPU coverage because they occupy
62.23% of physical nodes. Select/extend supported operations from the captured
frequencies; use explicit unchanged fallbacks for the remainder and charge their
working set. Do not drop unsupported shapes or renormalize covered events to 100%.

Replay the actual operation sequence, comparing values, mutations, child order
and final graph state. Preserve narrow writes and disjoint read/write capabilities
where available. Charge reconstruction, directories, identity/escape/text tables,
trace buffers, scratch, stack-copy traffic and disposal in named domains. A field
registry derived from Go bases must also include direct binder-written fields;
an embedding count alone is not a complete generator specification.

Keep trace decoding and trace-buffer residency matched across adapters and
report them separately. A separately observed decoder baseline can expose a
dominating driver cost; do not subtract its elapsed time as a corrected native
benchmark or claim identical cache interference. Preserve file/phase order and
retention so replaying only binding after preconstructing every file does not
silently replace the production parse→bind-per-file working set. If parse activity
is omitted, document that remaining cache-state difference and confirm the
layout on an integrated parser/binder before promotion.

Publish absolute milliseconds and MB first, operation coverage/unknowns,
full-pipeline control distances and all raw samples. A recorded-trace result
selects the integrated experiment; only actual whole-pipeline parity, ownership
and CPU/memory measurements can promote production storage or pass S07 gates.

## Feasibility appendix: hook points and implementation sequence

This sequence preserves the trace contract and its explicit unchanged fallbacks.
Completeness applies to each operation family a comparison claims to cover;
full binder field coverage is not a prerequisite to every useful bounded pilot.
The first ID-qualified surface can validate provenance, event volume and replay
plumbing, but is not sufficient by itself to rank payload layouts. Subsequent
named read/write families can support the bounded nine-shape comparison with
the remainder unchanged and its full working set charged. Every capture starts
from the **latest accepted immutable candidate**; an unpromoted working-tree
optimization is not its source.

Several useful hooks already have an explicit NodeId. Other APIs return plain
borrows and public fields, so a lookup hook cannot observe everything the caller
does afterward:

| Surface | Concrete hook points | What a hook can truthfully record |
| --- | --- | --- |
| Binder entry and phase order | `tsr_binder/src/lib.rs::initialize_binding`; the parse → bind → retained-file loop in `tsr_bench/src/main.rs` | File/phase boundaries and binder entry/exit. Keep initial-state inspection in a separate observer domain. |
| Node resolution | `tsr_binder/src/state.rs::n`, `tsr_ast/src/bind_result.rs::BindBuilder::node`, `tsr_ast/src/storage.rs::AstView::node` | Requested owner-qualified ID, lookup mode and success/error. Distinguish a caller request from nested routing events so they are not counted twice. The exclusive-core branch bypasses AstView. Shape can enrich an event from the physical-state registry; that does not make the lookup a captured shape-field read. |
| Named kind/flag reads | `dispatch.rs::bind_node_head`, `bind_worker`, `bind_node_error`; `state.rs::set_flags` | These sites already hold NodeId and can log the actual getter result with one evaluation of the original expression. This is bounded staging work; it does not cover kind/flag reads elsewhere automatically. |
| Lists and immediate children | `containers.rs::syntax_slice`, `syntax_node`, `syntax_nodes`, `ImmediateChildren::of`, `ChildVisitor` implementation; `storage.rs::node_slice_read` | Resolution, explicit indexed reads and emitted child descriptors, with list/backing/subrange identity and order. A slice resolution is not a read of all its elements; a nonnil visitor callback does not capture the absent-field checks preceding it. |
| Narrow node/binding writes | `BindBuilder::set_node_flags`, `set_node_flow`; explicit assignments through `Binder::binding_mut` | Field, node ID, requested value and successful write. Preserve the existing validation and flag-write proof. Record failed attempts separately. |
| Copies | `expressions.rs::payload!`; explicit payload clones in `diagnostics.rs`; `tsr_ast/src/lib.rs::Node::clone` and `copy_for_binding` | A copy operation and its source/temporary identity. Subsequent reads of a copied payload are temporary reads, not repeated reads of the arena record. |
| Record births | `tsr_arena/src/file.rs::StorageBuilder::push/push_aux`; `tsr_arena/src/lazy.rs::StorageTransaction::push/push_aux/publish`; symbol/table/flow allocation paths | Assigned identities, physical creation order and publication/abort state. Lazy reservation is distinct from successful publication, and failed slots remain burned. |

The narrow binding-field assignment inventory is small enough to instrument
explicitly in the staged copy: `declarations.rs` writes symbol/local_symbol;
`state.rs` creates locals; `containers.rs` writes end_flow_node,
return_flow_node and next_container; `statements.rs` writes
fallthrough_flow_node; `modules.rs` restores the source symbol. Flow writes also
pass through `set_node_flow`. A hook at `binding_mut` alone observes obtaining
`&mut NodeBinding`, not which field is written later. A guard that compares the
record on drop observes only net changes and can lose repeated or reverted
writes; it must not stand in for the actual ordered write events.

The proposed 16-ID list-chunk helper is **conditional on that candidate being
promoted**. If it becomes the accepted source, capture each actual
`copy_from_slice` range and its temporary IDs, then distinguish later temporary
consumption from backing access. Do not replay its copied IDs as the previous
per-element `node_slice` resolutions. Conversely, a trace of the earlier control
must not invent chunk copies that it never executed. Keep copy traffic even
when a later consumer does less work with the copied range.

Implement in these bounded stages:

1. **Freeze the event registry and identity ledger.** Reuse the owner-census
   staging/build/loaded-input approach, but add a full per-file physical state
   export before binding and explicit births during binding. Existing census
   histograms do not contain node edges, scalar values or allocation events and
   cannot reconstruct that state. Retain obsolete records and shared list/text
   backing identities. Do not call runtime-ID allocation or subtree-facts
   computation to label records. Observer reads need suppression/domain tagging
   so serializing initial state does not become apparent binder activity.
2. **Capture the ID-qualified surface first.** Add node resolutions, the named
   kind/flag sites, explicit list indexing/enumeration/copies and narrow writes
   listed above, preserving original evaluation and mutation order. Check this
   surface's values and final-state effects against its snapshots and the
   semantic protocol. This milestone can expose event volume and missing
   mappings; record the uninstrumented surfaces and leave unavailable counts
   unavailable. Do not use it as a payload-layout performance result.
3. **Carry provenance through the compared borrows and copies.**
   `Node` has no owner ID and `NodeRead` is an alias for `StorageRead<Node>`;
   `Node::kind/flags/data` receive only `&self`. A feature-gated origin carried
   by staged nodes or explicit ID-bearing accessor contexts can cover general
   getter calls, but must distinguish resident records, overlay representations
   of the same logical node, new factory nodes and temporary clones. Structural
   Node cloning resets caches, while binding-overlay copying preserves the
   logical node and selected caches; trace origins must follow those different
   operations. Named sites already carrying NodeId can log their results
   directly; do not require a general accessor migration to observe those sites.
   Never attribute a getter to the most recently resolved node.
4. **Compare complete, explicitly scoped operation families.** Extend the first
   surface with actual shape checks, kind/flag reads, child enumeration, narrow
   binding writes and named representative payload reads for the bounded slice.
   Examples include property-access expression/name, call expression/arguments
   and binary left/operator/right reads at their actual consuming sites. Include
   the observed temporary-copy operations and their later reads where applicable.
   Capture all events within each declared compared family; do not invent reads
   for untouched fields. Retain explicit unchanged adapters for the remaining
   operations and shapes, charge their full working set and preserve their place
   in the sequence. Report coverage and unavailable counts without renormalizing
   the supported events to 100%. This can rank a bounded layout/accessor slice
   for the next integrated experiment; it establishes neither complete field
   coverage nor full-binder CPU performance.
5. **Expand getter/read-site coverage where a broader claim requires it.**
   Generated `NodeData::as_*` methods return payload references, and generated
   payloads expose public scalar/ID fields. Logging the shape borrow does not
   reveal later `.expression`, `.arguments` or `.initializer` reads.
   `payload!` clones those fields into a temporary without retaining its NodeId.
   Complete per-field coverage therefore needs explicit staged read-site
   instrumentation or migration to provenance-carrying getters, including
   temporary reads. This broader work is required before claiming that coverage,
   not automatically before the scoped comparison in stage 4. Extend only the
   needed surfaces in `node_accessors.rs`, `runtime_generated.rs`,
   `visitors_generated.rs`, `subtree_generated.rs` and handwritten subtree/helper
   paths from an audited operation inventory; do not emit synthetic reads for
   every field when only one accessor or copy ran. Likewise `NodeSliceRead`
   dereferences to an ordinary slice: indexing/iteration after that dereference
   needs consuming-site or iterator instrumentation to expose element reads.
   Returned slices and public fields prevent a single boundary hook from
   providing the full trace. Use bounded staged hooks to inform the production
   accessor decision; do not port thousands of accessors twice merely to obtain
   the first useful comparison.

Facts cache loads/stores are a separate operation family at
`Node::cached_subtree_facts/store_subtree_facts` and `subtree_facts.rs::cached`.
Preserve sequential consistency, hits versus misses and actual computation;
peeking or computing facts for metadata must not masquerade as a consumer read.
Symbol/flow/table contents needed by replay also require birth/state coverage
or an explicit unchanged adapter; recording a FlowId assignment alone does not
reconstruct the referenced flow graph.

These are observed **source-level operations**, not a hardware memory-access
trace. Instrumentation can prevent compiler elimination or coalescing of reads,
including fields in structural copies. Keep that distinction alongside the
existing decoder/cache caveats. Validate provenance and semantic completeness
for the compared families before expanding capture volume, then use results
within their reported coverage to choose the integrated experiment under the
original full-pipeline promotion requirements.
