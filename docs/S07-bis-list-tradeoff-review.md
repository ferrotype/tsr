# List policy review: absolute costs and production scope

Date: 2026-09-08. This reviews Fable's feedback on the three recorded list
experiments. The captures, raw observations, helper hashes and production CP1
promotion remain unchanged. This is a decision amendment, not a new measurement.

## Decision

**Restore 256-word edge pages as the leading list integration candidate.** Keep
64-word pages as a bounded memory challenger: the same capture gives them lower
requests/live storage, without establishing a robust CPU ordering between those
page sizes. Keep the tested contiguous-chunk policies rejected as replacements
for pages. The later chunk access experiment remains useful evidence about
resolution costs, not a production list-storage choice.

My earlier decision gave the isolated traversal percentage too much weight.
The replay establishes a construction/allocation versus traversal tradeoff; it
does not justify holding pages out of the integrated storage slice until they
match raw boxed-slice traversal. Neither the complete owner budget nor the
production CPU/RSS effect has been measured for pages yet. That prevents final
promotion, but is not a reason to drop this integration candidate.

## Exact scale of the first trial

All numbers below derive from the [first fixed capture](../tools/s07/performance-experiments/storage-pilot/README-list-results.md).
They are not mixed with later same-batch controls. Times are milliseconds;
memory is decimal MB. Each child constructs all 13,094 files and traverses
3,114,989 physical backings and 6,047,867 synthetic edge slots eight times.
The census includes obsolete and empty backings, not just reachable AST lists.

| Quantity | Legacy | Page-256 | Difference |
| --- | ---: | ---: | ---: |
| Construction | 49.767625 ms | 32.893709 ms | −16.873916 ms |
| Eight sweeps | 94.159541 ms | 127.652375 ms | +33.492834 ms |
| Average per synthetic sweep | 11.769943 ms | 15.956547 ms | +4.186604 ms |
| Requested bytes | 310.417768 MB | 109.475920 MB | −200.941848 MB |
| Retained requested bytes | 119.563816 MB | 72.780112 MB | −46.783704 MB |

The request saving is about **9.9%** of the historical one-worker allocation
budget of 2,035.226 MB (`0.7 ×` the old Go median). It is material even though
it is only 4.33% of current CP1's 4,642.570 MB requests. The final gate remains
relative to a fresh Go capture. The 46.784 MB retained reduction is not an RSS
measurement, and neither isolated saving can be deducted from production totals
as an observed result. The replay does not reproduce all production Vec starting
capacities, nesting, headers, IDs, initialization, routing or owner retention.

Fable's two/three-sweep conversion is useful **conditional arithmetic**:

| Assumed equivalent sweeps | Traversal penalty | Fraction of a nominal 5,000 ms pipeline | Construction plus traversal delta |
| --- | ---: | ---: | ---: |
| 2 | +8.373209 ms | +0.167% | −8.500708 ms |
| 3 | +12.559813 ms | +0.251% | −4.314103 ms |

Under those assumptions the replay breaks even near 4.03 sweeps. Production
frequency and cost equivalence are unmeasured, so this is neither a pipeline
prediction nor an upper bound. The latest CP1 one-worker wall median is
4,590.853 ms; five seconds is a historical scale, not its exact control. Neither
denominator turns the synthetic numerator into a production observation.
The original load averages of 27.79 / 30.06 / 29.03 on 18 CPUs further limit CPU
inference. They do not invalidate the exact requested-byte accounting.

## Caller and proof audit

Fable correctly identifies an asymmetric traversal baseline. The legacy arm in
[`list_main.rs`](../tools/s07/performance-experiments/storage-pilot/list_main.rs)
walks raw boxed slices. Production first routes an auxiliary record, checks its
variant and validates its range in
[`node_slice_read`](../crates/tsr_ast/src/storage.rs). `NodeSliceRead` retains the
resulting record and range; its
[`Deref`](../crates/tsr_ast/src/lists.rs) still matches the variant and slices the
backing with safe bounds checks. Extra work on the control makes the isolated
comparison an unsuitable direct production estimate. It does not establish a
mathematical upper bound: replacement routing, page boundaries, compact-ID
decoding, cache behavior and optimization can change as well.

The claim that binding walks each list once is not the implemented access count.
[`syntax_node` and `bind_each`](../crates/tsr_binder/src/containers.rs) re-resolve
the complete slice for each element. Functions-first statement binding walks
the list twice to select functions and non-functions; binding a statement once
does not mean reading its list entry once. Construction and final
validation inspect edges as well, and parser/factory utilities have their own
reads. Conditional top-level-await and JSDoc reparsing also defeat a universal
"parent fixing once" assumption. Conversely, obsolete physical backings in the replay need not be visited
by reachable-tree operations. Count actual resolutions and visited elements by
phase/caller in an untimed diagnostic if a conversion needs their frequency.

A resolved `NodeSliceRead` is a useful proof, but it is currently borrowed from
the whole `BindBuilder`/`ParsedFile`. Keeping it across recursive mutable binding
requires disjoint immutable list access and a restricted node/binding writer.
This is the real integration obligation, not a reason to defer page selection
as a candidate. It benefits boxed, paged and chunked backings alike.

Publication is not by itself a blanket license to skip range checks. Production
completion validates stored core edges/list slices; later raw/imported/lazy
access still needs its checked boundary. In the isolated page implementation,
[`finish`](../tools/s07/performance-experiments/storage-pilot/lists.rs) privately
constructs valid backing ranges; `publish` checks that staging is complete, not
every range again. A resolved reader can carry that construction/resolution
proof and avoid reminting IDs or redoing logical start/length validation while
retaining safe physical bounds checks. Do not add another publication scan
merely to establish a fact already maintained by construction.

The existing page sweep already calls `range()` once per backing, then visits
page spans; it does not repeat that logical validation for every element.
Avoiding repeated public-ID resolution across operations and avoiding the real
binder's per-element re-resolution are related but distinct changes.

The chunk result supports retaining pages: the tested chunks spend more storage
without a measured traversal gain. It does not prove range validation is the
entire cause. The later access experiment changes owner/backing resolution and
improves traversal, but tests owner-scoped **chunks**, not owner-scoped pages.
No comparison between resolved pages and resolved chunks has been measured.

## Consequences for the next slice

Carry page-256 into the representative node/list slice now. Match its list policy
across typed versus mixed node-row candidates, with the current production AST
access path as the semantic/control adapter. Preserve page-64 as the bounded
size challenger. Resolve a list once per operation where the disjoint reader
allows it, support page spans without copying on read, and retain checked raw
IDs, aliasing, empty/missing distinctions, lazy/import fallback and narrow writes.

Report absolute milliseconds and allocation/live/RSS MB before ratios, and map
them to the measured whole pipeline and remaining gate budgets. Unknown caller
frequency stays unknown. The [screening rules](S07-bis-performance-plan.md#6-screening-and-decision-rules)
now make that mandatory. The unchanged 5%/2% screen thresholds apply only to
complete-pipeline measurements, including the named infrastructure exception;
they do not screen isolated traversal percentages. Matching raw boxed-slice
speed is not a gate before integration, and no final S07 threshold changes.
