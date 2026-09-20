# S07-bis: measured storage and CPU improvements

Status: the [fresh ADR 0021 capture](S07-bis-gate-rebase.md) passes the four
owner-approved performance criteria, including both CPU confidence bounds.
All local evidence is current and S07 passes. Four-target CI is reported on PR #12.

The [bounded drain/hash follow-on](S07-bis-drain-hash-result.md) is complete
and **unretained**. It passes correctness and both full workload graph modes,
but CPU ratio intervals are 0.954296–1.008483 / 0.894933–1.022502 and memory is
effectively unchanged. The fixed runner reports `regressing_or_uncertain`;
this is inconclusive, not proof of regression. Both production edits were
restored to the preceding control. No extra samples or new acceptance batch
were run. The profile and worker diagnostic identify no proportionate second
candidate; no scheduling change or broad parse rewrite is selected. The owner has since changed the four final thresholds through ADR 0021; see
the closure subsection below. Historical experiment verdicts are unchanged.

The preceding [page/text/helper result](S07-bis-pages-text-helpers-result.md) remains retained.
Native checks, both full graph modes, the full binder producer and E3
(29 S06 / 83 S07 cases in every mode) pass. The fixed screen reports
`no_demonstrated_win`: CPU ratios are 0.9533396729 / 0.9606986456, with upper 95%
bounds 0.9707271814 / 0.9993300452. Requests fall about 97.7 MB to 2.14592 GB;
RSS rises about 7.1 MB to 2.32602 / 2.32931 GB. Nonregression passes. Retention
under the existing small-improvement policy is accepted after independent review;
the screen does not meet the 5% substantial-candidate threshold.

Candidate `8d837bc0ab365972d2d5e0eeb0ee95255a505e44dea01964fca2198e78850898`
contains Rust `02490d8`, frozen at HEAD `9a394bf`, and was retained as the
experimental control for this follow-up. Its
[fresh paired comparison control](S07-bis-control-refresh.md) is source `eaf50b7`,
immutable manifest `957421942258d954765fc88b494b2031982dfac849d9358850dfdba7078edcd4`.
That baseline remains the comparison control for the completed screen.
The retained implementation now has a [standard paired Go/Rust acceptance batch](S07-bis-candidate-acceptance.md):
CPU ratios **1.213911 / 1.404619**, allocation **0.738066 / 0.737854** and
RSS **0.737440 / 0.735532**. All original final gates failed in that batch. Its paired deficits
are 653.1 / 245.1 ms CPU, 110.7 / 110.1 MB allocation and 118.1 / 112.5 MB RSS.
These supersede the earlier cross-batch distance arithmetic. That source-bound
E5/E6 evidence records failures under the original thresholds and becomes stale
when the ADR 0021 benchmark changes are applied.
The next experimental control is the exact native bundle
`1c04605dcd248d78dcec1e42c4c6f82f036843a054cf5e27d81c1b3b229753b5`,
frozen from the unchanged implementation at `26b6c4e`.

The preceding [complete local binder with shared text](S07-bis-local-text-repair.md)
was accepted as an experimental control after source review, full workload
graphs, corrected E3 and the fixed screen. One/eight-worker CPU ratios are
0.854300 / 0.821120 against same-screen CP1, with upper 95% bounds
0.855997 / 0.831178. Requested allocation is 2.244 GB and peak RSS 2.322 GB.
That earlier freeze is `99d11d1f2efd383919663bece6459a0230bc0fe815723ba65b7178e5973c5abc`
from `6de7612`. The preceding completion keeps its failed E3 capture and
unpromoted disposition; the corrected inventory passes all 29 S06 / 76 S07
cases in every required mode. Earlier rejected experiments retain their original
dispositions. Final S07 CPU and memory gates remained open at that checkpoint.
Date: 2026-09-10. Work branch: `codex/s07-bis`.

The [earlier frozen allocation traffic diagnostic](S07-bis-allocation-traffic.md) is
complete. It attributes 225.443 MB of the 333.733 MB freed/superseded requests,
leaving 108.289 MB unclassified. No individual measured family establishes the
then-remaining 209 MB request saving, and this diagnostic supplies no RSS or CPU
acceptance. The shared-text repair removes about 58.5 MB of requests from the
preceding local combination. Its historical deficits were 208.3 / 207.8 MB
requested allocation, 109.7 / 104.9 MB RSS and 828.2 / 180.5 ms CPU. Those are
distances to old Go medians; the completed paired baseline now supplies current
gate distances. Keep older family attribution tied to its binary. The old
125.378 MB growth-only ceiling does not bound the selected changes to row-page
width and text-entry size; charge their tails and replacement storage in the
combined measurement.

Baseline: `53b523a` from `codex/s07-binder` / PR #11.
Initial plan reference committed by the user: `4173b89`.
Review: [independent plan findings and amendments](S07-bis-plan-review.md).

Keep this work on the separate S07-bis branch. Preserve the committed reference;
do not amend it or commit subsequent S07-bis work to the S07 branch.

S07-bis is the performance follow-through for S07, not a new tracker sprint or
a replacement tracker sprint. Its original objective was to make the existing
S07 parse-and-bind implementation pass both memory criteria and both CPU modes,
while keeping every existing correctness and ownership prerequisite current.

## 1. Decision

The completed selected experiment combines sixteen-row typed pages, eight-byte
text-pool entries preserving shared owned backing, and the remaining shared
binding helper migration. The [result record](S07-bis-pages-text-helpers-result.md)
records validation, the fixed combined screen and elapsed-phase attribution.
Independent result review found no blocker to this retention decision. The eight-worker
gain is 3.9% against its same-screen control; comparing against that control's
1,037 ms earlier native run instead of its 826 ms screen observation cannot
establish a roughly 24% improvement. No gate or raw verdict changes with retention.

### Prior decisions and measurements

The next sequence, approved after the first full access capture, is **one bounded
[lookup-reuse experiment](S07-bis-lookup-reuse.md), then integrated compact storage
using typed payload rows**. Defer expanded field-level tracing and the three-layout
replay harness. The existing capture supplies useful operation frequencies; it
does not measure residual lookup latency or guarantee a reduction to two reads
per node. Further diagnostics must resolve a specific decision left open by the
integrated result, rather than become another prerequisite to integration.

The lookup trial is now complete and **rejected**: 52.010 / 12.893 ms wall savings
(1.13% / 1.21%), with no meaningful memory change, miss the committed 5% screen.
The candidate's binder changes were restored to CP1 before the compact-storage
migration started. There is no further standalone lookup experiment queued.

The [first integrated compact-storage screen](S07-bis-compact-storage.md) is now
complete: 3.185 GB allocated and 2.837 GB peak RSS, but 7.265 / 1.624 s wall time
at one/eight workers. It passes full workload graphs and fails the unchanged
CPU non-regression rule. CP1 remains the retained control. Preserve this result
and test one combined repair of overhead introduced by the migration: direct
construction of already-resolved core reads, direct typed-row selection when the
caller knows the shape, and bypassing empty exceptional-reference maps during
ordinary local writes. A native sample of the exact frozen normal executable
identifies these paths; no expanded field trace or replay is needed. Keep owner,
slot, shape and overwrite validation. Re-run the fixed pipeline screen for the
changed candidate before choosing subsequent storage work; memory savings do
not authorize promotion with a CPU regression.

That combined repair has now completed its fixed screen: memory remains
3.185 / 2.837 GB, while one/eight-worker wall is 6.099 / 1.315 s versus
same-screen CP1 at 4.363 / 0.968 s. It still fails non-regression; no sample
extension or threshold change is permitted. A new short native sample identifies
physical-read construction as the largest remaining self-cost. The next bounded
implementation borrows the selected physical owner, constructs payload context
only on demand, and validates stored row references directly without a borrowed
union roundtrip. Preserve all validation and error order. This addresses overhead
of the current representation before trying another page policy, expanded trace,
or symbol/flow redesign. The result record names its limits and keeps CP1 as the
retained control.

The thin-owner candidate also fails its fixed screen: CPU median ratios are
1.306 in both modes, with upper bounds 1.374 / 1.337 and memory unchanged.
Preserve that third result. Test one ordinary per-shape vector policy instead
of four-row pages: core growth has exclusive ownership, and publication ends
that growth, so stable row addresses during construction are unnecessary.
Keep checked ordinals and actual lifetime guarantees. Use default vector growth;
measure capacity slack and total allocation as well as CPU. This is one changed
production candidate, not a page-size matrix. CP1 remains the measured control,
and no compact candidate has qualified for promotion.

The vector trial also fails (CPU ratios 1.285 / 1.259, allocation 3.536 GB),
and its implementation is removed. Continue CP4 as one combined unpromoted
candidate on the thin paged implementation: compact flow/symbol records, shared
names and table access, pooled declaration backings, and removal of eager parser
list reservations. The result record specifies the compatibility tests and the
limits of the historical size arithmetic. A short native sample of the frozen
thin candidate audits remaining CPU costs without expanding trace/replay work.
No more row-policy matrix or small lookup trial is scheduled. CP1 remains the
retained control until a complete candidate passes all required screens/evidence.

CP4 now passes all workload graphs and receipt verification but fails CPU
non-regression. Its CPU upper 95% ratios are 1.380 / 1.344; relative MAD is below
1.3% in both modes. The remaining distance to historical limits is about 0.54 GB
allocated and 0.29 GB RSS, with a much larger CPU gap. Preserve this result and
audit its exact native profile before choosing another change. CP5 must establish
which final graph obligations construction and subsequent narrow writes already
prove; removing a scan without that proof is not an optimization. Auxiliary
record compaction also needs a concrete current-layout cost bound. Do not reopen
field tracing, a row-policy matrix, or another standalone lookup trial.

The bounded CP5 implementation now retains checked construction and uses its
private edge proof to omit only duplicated final payload/list/backing checks.
Keep the final parent scan and metadata validation. Unrestricted node/list edits
and hooks dirty that proof and retain the original full scan/error order. Narrow
list range/flag edits preserve it. This is the later completion-scan reduction
allowed by CP5, with its explicit mutation audit and counterexamples recorded in
the [compact implementation record](S07-bis-compact-storage.md).

CP5's proof and affected tests are complete. Matched frozen CP1/CP4 elapsed-phase
diagnostics now show both phase groups regressing: parse 2.568 → 3.509 s and bind
with publication 2.137 → 2.971 s. The next combined candidate removes temporary
owned payload boxing/union dispatch from generated concrete construction and
generic read intermediates from already selected core reads, retaining the CP5
proof. Its [implementation scope](S07-bis-compact-storage.md) preserves custom
factory hooks, counters, error order and all compatibility readers. Screen this
combination once; do not impose independent promotion gates on its parts.

That combined screen is now complete and verified, with all workload graphs
matching in both modes and zero binding fallbacks. One-worker wall is 5.178 s
against 4.714 s CP1; eight-worker wall is 1.504 s against 1.397 s. CPU upper 95%
ratios are 1.123 / 1.113. Allocation is 2.434 GB and peak RSS is 2.506 GB.
Retain the complete result as an experiment, without changing the control or
calling the combination accepted. Its exact native sample leaves 541 ms of
disjoint parent-attachment/header-finishing weight, including required work.
Continue with concrete owner-local finishing/parent attachment, preserving the
generic path and its observation order. In parallel, price current payload-page
allocations/directories from the verified physical census and exact source before
selecting CP6 capacity changes. Judge the next coherent combination once; do not
infer individual component savings or add results from the earlier captures.
Auxiliary compaction remains a separately priced option, not a promised saving.
Final gates are unchanged.

Current page accounting now bounds that decision: 371.034 MB modeled retained
typed-row storage, plus 46.905 MB of discarded directory growth arrays in
the request total. Even free page/directory/root overhead would recover only
132.262 MB of requests, below the roughly 399 MB remaining allocation deficit.
Keep the four-row payload policy. Select the
[compact auxiliary implementation](S07-bis-auxiliary-plan.md) alongside the
checked owner-local parent-attachment work. Its implemented policy models
115.677 MB less retained storage and 96.154 MB fewer requests, with 1.813 million
additional allocation calls; these are projections. Preserve unrestricted
list mutation by cold promotion of only the touched header, retaining its AuxId
and direct `&mut NodeList` semantics. No allocation-bearing mutation guard is
introduced. Complete and screen the combination before any promotion decision.

That combination is now recorded and verified: 5.263 / 1.158 s against same-screen
CP1 at 4.704 / 1.083 s, CPU upper 95% ratios 1.175 / 1.102, allocation 2.331 GB,
and peak RSS 2.322 GB. All workload graphs match at both worker counts. It remains
experimental, with roughly 295 MB of allocation and 105–110 MB of RSS still above
historical limits, alongside the CPU deficit. Its complete archive preserves
the failed performance screen as well as passing correctness/instrumentation
checks. Do not promote it on memory alone or add separate captures as if paired.
The next bounded combination follows the reviewed
[parser list construction plan](S07-bis-parser-list-plan.md), removing short
temporary buffers without changing traversal or the row policy. Its conditional
request opportunity is useful but insufficient for gate closure by itself.

The inline parser-list combination is now complete and verified: 5.387 / 1.205 s
against same-screen CP1 at 4.748 / 1.137 s, CPU upper 95% ratios 1.162 / 1.146,
allocation 2.244 GB and peak RSS 2.322 GB. All workload graphs match in both
modes. The historical request deficit is now about 209 MB; RSS still needs
105–110 MB, and both CPU modes remain slower. Preserve this experimental result.
Before choosing another storage change, run one narrow diagnostic from the exact
frozen source: normal-binary parse versus consuming-bind/publication elapsed
time, and allocation-binary requested bytes still live at the retained endpoint
versus cumulative requests. Reuse the existing allocator counters; do not rebuild
the obsolete physical census adapter or claim current RSS from live requests.
This diagnostic selects subsequent work and cannot promote the candidate.
It is now complete: parse 2.749 s, binding/publication 2.742 s; pipeline requests
2.244 GB comprise 1.910 GB net live growth and 333.733 MB freed/superseded requests.
One native sample of the unchanged normal binary locates 398 ms of distinct
keyword-comparison/slice-validation weight. Follow the reviewed
[shared text-processing plan](S07-bis-text-processing-plan.md), using generated
keyword matching and proven UTF-8 slice boundaries. The
[current-cost record](S07-bis-current-costs.md) distinguishes these diagnostic
scopes; no sampled weight is a predicted saving or fresh acceptance result.

The shared text combination is now complete: 5.180 / 1.080 s against same-screen
CP1 at 5.078 / 1.088 s, with CPU upper 95% ratios 1.056 / 1.006. Allocation and
RSS remain 2.244 / 2.322 GB. All workload graphs and the fixed 8+56 schedule pass
verification. Its first freeze reused a phase diagnostic's Cargo cache and was
rejected before timing; measured builds now isolate both final and intermediate
artifacts, with a real regression fixture. The corrected complete result and
invalid attempt are both retained. Neither CPU mode demonstrates a win, and
one-worker nonregression is not established. Keep the combination experimental;
end keyword/slice tuning without another timing batch. The remaining historical
gaps are about 209 MB allocation, 105–110 MB RSS, and 2.243 / 0.446 s wall.
The different preceding batch cannot prove this component's isolated saving.
Any next storage change needs current retained/growth attribution before a
family's total bytes are treated as removable; do not reopen field tracing or
a row-policy matrix to rescue the verdict.
The bounded [name/table attribution](S07-bis-name-table-attribution.md) is now
complete after calibration and one full allocation run. Its five backings request
143.318 MB, retain 85.842 MB and replace 57.476 MB through growth. Even the
unattainable exact-element/no-growth bound saves only 98.124 MB against the
roughly 209 MB request deficit. End canonical-name/hash-backing tuning as the
next primary gate-closing direction; smaller measured components remain available
for a justified future combination. Table arenas and owned-string backing costs
are not inferred from logical counts. No timing retry, control change or gate
promotion follows from this diagnostic.
A [single native sample of the current shared-text binary](S07-bis-post-text-cpu.md)
now attributes 2.288 / 2.528 s of sampled worker CPU to parse/bind-publication.
Keyword lookup is 57 ms and no sampled UTF-8 validation has a slice caller;
remaining work is distributed across reads, construction, parenting and binding.
The broad groups include required work and overlap. End this diagnostic pass
without another profiling or small-lookup loop. No substantial next rewrite is
selected solely from those inclusive totals; a concrete removable mechanism is
still needed. This does not change the paired screen or final gates.

The subsequent [bind-only Rust/Go comparison](S07-bis-bind-cpu-comparison.md)
is now complete using those existing samples, with no new capture. Rust's
2,528 ms bind partition has leading self costs in node-entry dispatch, checked
reads, contextual identifiers and hashing. Go's three historical bind partitions
are 730/670/690 ms, mostly sampled at `runtime.madvise`; retain that runtime work
and the different sampler/denominator limits. The concrete removable mechanisms
are now named: full-payload decoding for flow capability, repeated generic
flow-field dispatch, and repeated immutable source-fact reads.
Implement the reviewed [flow/source-fact combination](S07-bis-bind-cpu-plan.md),
then run one complete-pipeline screen against CP1. Do not expand stack-guard,
hashing, facade or trace work to rescue its result. The current compact freeze
remains an experimental starting point, not an accepted infrastructure control;
all historical results and final gates retain their existing status. These
sampled costs justify a bounded trial, not a predicted saving or a complete
demonstrated path to Go CPU parity.

The subsequent review changes the next implementation scope: follow the
[local binder and traffic-attribution plan](S07-bis-local-bind-plan.md).
Local handles, typed reads, traversal and narrow writes are the primary CPU
candidate. The small flow/source-fact changes belong within it and receive no
separate prerequisite screen. Complete current allocation-family replacement
attribution without porting the obsolete census. Keep both gates in S07-bis;
neither a runtime-subtracted Go ratio nor a zero-churn assumption proves they
will pass. The earlier plans and measured outcomes remain recorded.

Typed rows are the selected first implementation, with mixed word rows held as
an alternative. Their current modeled premium is 51.545 MB retained and 74.336 MB
requested, before missing owner costs. This is an engineering choice to test,
not a measured CPU advantage or a guaranteed whole-owner fit. Establish contextual
borrowed access and construction on representative shapes, then measure one
complete generated backend for all 192 shapes. Do not maintain a permanent
nine-shape compact backend alongside the legacy syntax representation.

Keep the lookup trial to one coherent candidate and one fixed screen. Run focused
correctness checks and full workload graphs before that screen; only a promising
candidate proceeds to the broader binder/ownership producers required for
promotion. A failed screen ends this CPU detour. Do not tune another variant,
extend samples, or build more instrumentation to rescue it. Compact storage is
the next substantial work either way. Reuse existing prototypes and producers.

Choose **an early single-source exclusive-binding experiment, followed by
compact generated storage**. Keep today's published-file binder as the
compatibility path. Run CP0, then A0, and decide the architecture before building
the CP2 columns, locator and patch store. CP2 is now **on hold as the fallback**
if A0 fails its correctness or measured-cost exits.

The [A0 implementation record](S07-bis-A0.md) records both experiments. Initial
A0 failed its screen; the predeclared A0-b variant preserves the parse-validation
proof across narrow flag writes and passes. Keep that exclusive path, with CP2
still held. The [storage pilot](S07-bis-storage-pilot.md) now implements four-byte
identifier text and compact list construction, records a fresh physical owner
census, and prices concrete binding/auxiliary records. **Use 256-word edge pages
as the leading list integration candidate**, retaining 64-word pages as a bounded
memory challenger. The [list tradeoff review](S07-bis-list-tradeoff-review.md)
corrects the earlier decision: the isolated traversal percentage was insufficient
reason to hold back a policy with material request/live savings. A second capture
finds that contiguous chunks add cost without recovering checked traversal speed;
those tested chunk policies remain rejected as replacements for pages. The separate
[CP1 access trial](S07-bis-CP1.md) recovers most of that gap with owner-descriptor
reads and more with explicitly charged cached slices. Its bounded production
node-lookup candidate passes a full-pipeline screen and becomes the next control;
this does not promote production list storage or implement the general borrowed
facade. The subsequent [stack-copy trial](S07-bis-list-copy.md) passes parity and
non-regression checks but misses the predeclared 5% win (0.83% / 0.19% wall median
reductions, no meaningful memory change); reject that standalone shortcut. Carry pages into the integrated compact-storage implementation and resolve
lists once where the actual caller can retain that proof; do not require pages
to match a raw boxed-slice microbenchmark before integration. Complete the
whole-owner accounting (including name interning, escapes and residuals) through
implementation and measured attribution; it remains required before claiming a
path to the memory gates. Resolve the borrowed access/factory contract first,
without requiring a further payload-family comparison. The
[CP0 model](S07-bis-CP0.md) is a projection, not measured replacement storage.

Claude's second review identifies a sequencing error in the initial plan. The
ordinary `FileCache::acquire` path parses, publishes and binds synchronously
before returning a `ProgramFile`; the benchmark does the same. No parsed view
escapes between those steps. The earlier plan imposed the general published-file
constraints on that exclusive path and scheduled substantial replacement
side-table work before testing whether it was needed.

The mapped-sibling finding required independent initialization. Choosing
overlays for every file was my broader implementation choice, not a requirement
of that finding. A single-source consuming path can be tested without changing
the existing published-unbound, mapped, imported or constructed-file contracts.

The [exclusive-binding design record](S07-bis-exclusive-binding-alternative.md)
now has its bounded single-source A0 path **activated**. A wholesale replacement
of published-file binding remains on hold. Compact nodes/text, symbols/flows,
validation, hashing and capacity work are shared between both storage paths.

A0 initially keeps today's symbol/flow field maps. It tests direct syntax-field
mutation and lookup routing, not elimination of the 633 MB full-binding-map
request site. After a successful A0, put applicable binding fields directly in
the compact per-shape payloads at CP3. Do not insert them into today's large
inline enum first and accidentally enlarge every node or box identifiers. The
[compiled layout audit](../tools/s07/performance-experiments/binding-layout-audit/README.md)
checks the current 40-byte enum / 80-byte node and the 48/88-byte result of an
inline eight-byte field. Model compact fields from both inherited Go bases and
direct fields: `CaseOrDefaultClause.FallthroughFlowNode` is outside the 83-type
base union.

### Contract history

S06 and the original S07 plan described exclusive binding before publication.
S07 commit `1637157` explicitly changed that to per-source binding cells over
published parsed cores following the mapped-sibling review. The current
[S07 lifecycle](S07-implementation-plan.md#31-initialization-before-immutable-publication)
and [review disposition](S07-implementation-plan.md#claude-fable-51-review-disposition)
record the implemented choice. Retain it for inputs that are already published.
For the new consuming path, explicitly document parse→bind→publish, its bound
result API and failure behavior; it does not claim a preserved parsed snapshot
that was never published. Reconcile the cross-references in the implementation's
contract update without overwriting unrelated work.


### Closure: thresholds re-based by ADR 0021 (2026-09-10)

The performance experiments end with the retained implementation measured in the paired acceptance capture of revision `26b6c4e`
([record](S07-bis-candidate-acceptance.md)): wall 1.214 / 1.405 of Go at one /
eight workers with 95% intervals 1.150–1.235 and 1.280–1.443, requested bytes
0.738, peak RSS 0.737. The retained implementation is the compact typed storage,
exclusive binding and the branded local binder view; the drain/hash follow-on
([result](S07-bis-drain-hash-result.md)) is the last bounded CPU experiment and
is not retained.

The owner re-based the four parse-and-bind criteria in
[ADR 0021](adr/0021-parse-and-bind-performance-thresholds.md): E5 peak RSS and
requested bytes 0.7 → 0.85; E6 one / eight workers 1.0 → 1.25 / 1.45, with
`run.e6.stable` and the sample-extension rule judged against each mode's
threshold instead of parity. This is an owner decision recorded outside this
plan; section 6's rule that no threshold changes to finish an experiment stands
for every screen and record written under it, and none is relabelled.

Because the benchmark scripts and the threshold ledger are part of every
capture's source fingerprint, the acceptance capture and its E5/E6 evidence are
stale once the decision is applied. S07-4 can close on one fresh paired capture of
the retained implementation at the revision containing the decision, under the
standard sequence. The [fresh capture](S07-bis-gate-rebase.md) of `9ff695b`
passes: CPU 1.232309 / 1.346110, upper bounds 1.242925 / 1.387920; memory
ratios at most 0.738071. It retains all 84 samples after the prescribed
one-worker extensions. S07 passes after refreshing correctness evidence and correcting the old
ownership-count and mapping-location bookkeeping. Four-target CI is reported
on PR #12. A future miss is recorded and reported, not re-sampled.

Open questions, without a new experiment commitment, are the eight-worker scaling
difference (4.4× against Go's 5.0×), the 653 ms one-worker elapsed gap, and residual
allocation traffic. The earlier 334 MB traffic / 108 MB unclassified attribution
belongs to its frozen binary and predates later retained changes
([attribution](S07-bis-allocation-traffic.md)). No Linux measurement exists; the
Darwin samples do not predict a Linux ratio. ADR 0021 limits acceptance to macOS
arm64 until a separate host-class decision is made.

## 2. Exact destination and starting gap

The authority remains [S07](../sprints/S07.toml),
[E5/E6](../status/experiments.toml), and the existing
[aggregation](../scripts/s07_benchmark_measure.py) and
[statistics](../scripts/s07_benchmark_stats.py). Every ratio is Rust / Go.

| Criterion | Required, unchanged | Original baseline one / eight workers |
| --- | --- | --- |
| Pipeline wall time | Each median ratio <= 1.0 | 1.789 / 1.931 |
| Timing qualification | Both runtimes' relative MAD <= 5%; bootstrap upper 95% ratio bound <= 1.0, in both modes | Fails |
| Lifetime peak RSS | Worse worker-mode median ratio <= 0.7 | 1.447 / 1.442 |
| Pipeline allocated bytes | Worse worker-mode median ratio <= 0.7 | 1.626 / 1.626 |

At the old one-worker Go medians, the limits would be 2.035 GB of allocation,
2.209 GB peak RSS and 2.938 seconds. Eight-worker wall time would need to be at
most 0.634 seconds. These are planning conversions, not fixed future Go
denominators. Rust needs approximately 57% fewer requested bytes, 52% lower RSS,
and 44% / 48% less wall time than its current one/eight-worker results.

Internal engineering targets provide headroom: allocation <= 1.90 GB, RSS
<= 2.10 GB on this workload/host, and median CPU ratios <= 0.90. These targets
help choose candidates; they neither modify nor replace the real gates.

The frozen workload remains 13,094 files, 161,740,237 loaded bytes, 19,593,488
nodes, 2,459,867 symbols, 423 parse diagnostics and 5,250 bind diagnostics.
The loaded-byte/options digest is
`d4ff2ad3b0d590babb596522a7105d961dbb3c4f2ebc987488d88b7ebd8e0c88`.
Matching counts is a screening check; complete graph parity is still required.

### What the profiles justify

The [memory report](S07-memory-profile.md) and [CPU report](S07-cpu-profile.md)
are the starting evidence, not results for the future implementation.

| Observation | Consequence for the plan |
| --- | --- |
| Binding map + flow slots retain >= 445 MB, including 177 MB spare; full-map insertion requests 633 MB | Replace representation and growth policy first; include replacement indexes in the comparison |
| Binding node-access union is 38.9% / 36.4% of binding CPU | Measure fewer resolutions and cheaper access; the whole union is not removable overhead |
| Uniform core nodes use 1,567 MB before spare capacity and separate payloads | Small map optimizations alone cannot meet the memory gate |
| Completion validation uses 10.1% / 8.8% of parse CPU; construction edge checks use 4.4% / 4.4% | Establish a construction proof before removing repeated checks; do not claim their sum as savings |
| 923 MB of requests are freed or superseded | Attribute and reduce request traffic; this is not a measurement of physical copies |
| About 480 MB of the RSS gap remains outside the compared live-byte counters | Measure allocator/VM domains if RSS fails after storage improves |

Both Go and Rust already allocate successive stable arena pages. Neither
pre-sizing nor a different allocator is assumed to recover the whole gap.

## 3. Invariants and intended API costs

Follow the [Rust working guide](CODEX-RUST-GUIDELINES.md),
[ADR 0006](adr/0006-node-ownership--arenas--lazy-file-storage--bundles-and-check.md),
[ADR 0007](adr/0007-symbol-ownership--file-owned-binding--checker-local-merges.md)
and [binding operation record](S07-binding-operations.md).

- Preserve byte, integer, missing/null/list-backing, source-order and failure
  semantics. Keep current unsupported clone-plus-rebind behavior explicitly
  unsupported; do not turn it into a new passing comparison.
- Keep full public 64-bit arena/slot IDs, with both full 32-bit components.
  Internal compression must have a checked full-range fallback, including
  imported and lazy references. No workload-sized ID limit or stolen slot bit.
- Keep per-logical-source once initialization, independent mapped siblings,
  initiator panic versus waiter failure, reentry handling, and unchanged parsed
  observations after success or failure on the existing published-file path.
  The new consuming path may mutate only exclusively owned storage and publish
  a completed bound result. Never mutate through a published borrow.
- Retained nodes/symbols keep the complete required file or bundle. Cache
  validation uses the cache owner's retained graph. No new owning cycles.
- Preserve lazy JSDoc/token identity and rollback, including nodes materialized
  before binding. Do not force lazy caches or runtime identities to initialize
  during ordinary parsing merely to simplify indexing.
- Use safe Rust. Proven private scopes can remove repeated owner checks;
  public imports, scope entry, callbacks and foreign/lazy fallbacks stay checked.
  Bounds safety remains separate from identity validation.

The intended common access path is a borrowed header or typed payload plus a
direct local index: no allocation, owner Arc increment, hash lookup or lock.
After the compact layout step, the exclusive path reads applicable binder fields
from concrete payloads. The compatibility path resolves its existing overlays;
compact replacement columns are conditional on the CP2 fallback decision.
These are objectives to measure, not claims about all mapped/lazy operations.

## 4. Storage design and budget

### 4.1 Separate the read contract from the physical record

Today `NodeRead` and retained-node dereferencing expose a physical `Node`, and
factory mutation returns `&mut Node`. Replacing the enum alone is insufficient.
Introduce an AST-specific borrowed read/mutation facade behind generated
accessors. Migrate binder, parser, visitors, factory/clone/update, utilities and
encoder callers deliberately. Preserve semantic APIs where possible, but
replace physical `Deref<Target = Node>` assumptions where necessary.

The facade must borrow actual headers and typed payloads. It must not rebuild
an 80-byte temporary node, clone a payload or retain an Arc on every read.
Owned factory inputs can remain convenient construction values; convert them
once on insertion. Keep the generic arena API's existing contracts available
to its other consumers rather than making every arena AST-specific.

The first implementation step replaces the `NodeRead` alias with an AST read
that borrows the existing record and carries its full identity, physical file
identity and physical source. Imported and lazy records use their actual owner,
not the importing caller or a logical `SourceFile` text override. Core lookup
still reads one slot; bound overlays retain their original priority. Lazy
transactions borrow source context before taking their publication lock, and
retained reads borrow their already-held record without reacquiring that lock.

This is migration infrastructure over the existing 80-byte node, with a temporary
`Deref<Target = Node>` for unmigrated consumers. It adds no owner retention or
payload copies, but enlarges the transient read descriptor; its runtime cost is
not yet measured. Do not treat this step as a storage or performance promotion.

Validation for this boundary: 182 workspace library tests, six AST doctests,
workspace all-target Clippy with warnings denied and Rust 1.96 checks, plus the
three new namespace/overlay/lazy-retention tests in release, strict-provenance
Miri and ASan. Independent review caught the retained-read lock reacquisition;
the fixed path is exercised inside a same-owner initializer. Local commands,
source hashes and logs are in
`target/s07-bis/contextual-read-step1-validation/`. These are development checks;
they do not refresh the tracker producers or final performance evidence.

The second implementation step generates contextual payload views for all 192
shapes. Each view borrows its originating read and actual typed payload; zero-field
shapes keep only the read. Scalar and graph getters expose semantic values rather
than stored field widths. Text getters return borrowed bytes, with a separate
explicit owned-string operation, so source suffixes and exception pools can
replace physical `JsString` fields later. Shape dispatch remains independent of
the open syntax kind. A compile-fail example enforces the lazy read-guard lifetime.

Generated clone/update/transform methods now consume these getters, preserving
field capture, byte equality, list identity, flags and callback order. `NodeText`
uses the byte/owned distinction, and one binder property-access site extracts its
two child IDs before recursive reads. Existing owned factory inputs and physical
storage remain unchanged. Compact header/parent/list resolution, insertion and
the typed backend are still outstanding; the generic arena's owner-free
`storage_parent` contract must change before parents can be stored as local words.

Validation for the second step: pinned generation check, 20 generator tests,
183 workspace library tests, 25 additional AST integration tests and seven
doctests. Workspace all-target Clippy/Rust 1.96, release tests and formatting
pass; all four contextual-read cases pass under strict-provenance Miri and ASan.
Independent review found no substantive issue. Two test-only Clippy findings
were fixed and the affected checks repeated. Commands, source hashes, the initial
failures and final logs are under
`target/s07-bis/contextual-payload-step2-validation/`; no tracker producer or
performance result is refreshed by these development checks.

### 4.2 Direct binding fields, with a conditional side-table fallback

Use the audited Go binder-write inventory to define separate storage for
declaration symbols, locals/containers, flow links and actual syntax-field
overrides. Preserve all fields currently represented by `NodeBinding`, including
return/end/fallthrough flow links and source metadata. No field may disappear
because the benchmark rarely uses it.

Here, "per-kind" storage means the concrete payload shape, independently of
syntax `NodeKind`. Keep a separate payload-shape discriminator: `TokenData`
paired with an identifier or unknown syntax kind must not select identifier
storage. Field eligibility follows the audited payload and source operations,
with a checked fallback for unusual constructed combinations.

After A0 succeeds, the primary default is binding fields inside their applicable
concrete payloads, using result-local symbol/flow/table references where the
owner proves provenance. Common node flags and parents are mutated directly.
During A0, retain current field maps to isolate the ownership/access experiment.
Shared binder operations must support direct and compatibility storage without
two independent ports of the binder algorithm.

The remainder of this section specifies **CP2 only if A0 is rejected**. Do not
build these replacement columns or patch indexes speculatively beforehand.
The fallback default is direct storage indexed by an eligible shape's local ordinal,
with source-local symbol/flow references where the owning result proves their
arena. Read-only syntax access bypasses binding storage; flag/parent/payload
queries apply their specific overrides. A sparse patch store replaces full
node copies only where a field really changes.

Compare this against occupancy-mask pages with packed values for sparse fields.
Account for rank lookup, insertion movement, directories and absent values.
Do not adopt a new all-node `u32` rank array casually: it costs about 78.4 MB.
Reuse the eventual typed payload locator or charge a temporary index explicitly,
with its removal scheduled in the node-layout step. Before typed payload pages
exist, checkpoint 0 must choose the provisional core-slot-to-shape/source-row
mapping. Compare a direct index with eligibility bitmaps/rank checkpoints. Build
it during construction or fuse it with required completion work, charging the
scan and allocations; it is not free because another validation pass may later
be removed. At checkpoint 3, store the same row ordinal in the typed locator
and delete the provisional core index, or retain and charge one shared index
if physical payload ordering cannot supply the binding row. Lazy additions and
unusual source partitions use a separately measured checked fallback.
Keep a checked general
fallback for uncommon mapped, imported, constructed and lazy shapes.

Logical source ownership and physical arena membership are different. The
single-source common case may use the established source scope. Multi-source
owners need an audited source partition/index or the existing checked fallback;
an arena-ID comparison alone must never authorize sibling writes. Avoid a new
whole-tree scan on every field access or every binding request.

### 4.3 Compact syntax, links and text

Implement a small common header and generated typed payload storage with
independent concrete-shape tags. Typed pages are the first integrated candidate;
word classes remain a conditional alternative after measured attribution.
Tokens have no general payload allocation.
A header-held payload ordinal, or a charged
slot directory, maps the stable public node identity to its concrete payload.
Do not change identity when a physical page or directory grows.

The candidate matrix includes ordinary typed pages, scalar word rows with
separately indexed facts, all-atomic word rows, and mixed rows with one inline
atomic facts word only on composite shapes. In a mixed composite row, its
plain-word count excludes the facts word already charged in the payload.
Non-composite rows carry no atomic field. Compare actual class occupancy,
capacity, directories and allocation calls, not only equal used-row widths;
the [additive mixed-row model](../tools/s07/performance-experiments/layout-followups/README.md)
records this distinction.

Also model a bounded chunk allocation policy. Safe homogeneous encoded backing
or separately typed arenas must preserve the selected accessor contract; do not
assume arbitrary typed references can be carved from raw byte storage. Charge
chunk descriptors, locators, alignment/tails, initialization, full-range escapes
and partial-construction/drop behavior. A source-sized first chunk needs minimum
and maximum bounds; the global bytes/node ratio is not a per-file prediction.
Obtain allocation order for exact packing/locality claims or label volume and
padding bounds as such. Compare complete replaced allocation regions, including
the existing payload boxes, before assigning an allocator CPU penalty.

Model the compact-parent, approximately 24-byte header first. It requires a
checked full-range escape mechanism;
include a separate payload-shape tag as well as the open syntax kind. Measure
the complete weighted representation,
including payloads, locators and page slack. These are provisional design sizes;
actual compiler layout on all supported targets decides them. Node kind remains
an open source-defined value; nullable fields and unusual factory kind/data
pairs must retain their current behavior.

Keep a roughly 32-byte full-parent header as a control or fallback only if its
complete weighted model fits. At 19,593,488 used nodes, 32-byte headers cost
627.0 MB and 24-byte headers 470.2 MB. With today's 20,968,456 header-slot capacity,
they cost 671.0 and 503.2 MB, leaving only 109.0 or 276.8 MB of the 780 MB syntax
sub-budget for the syntax portions of payloads and locators. Inline binding
fields use the separate 100 MB sub-budget. New occupancy may differ; count
it. The smaller header is the leading candidate, not a late optional saving.

Internal same-owner edges can use private compact references. A separate escape
mechanism preserves foreign/lazy references and the full slot range, rather
than reserving a high bit from the accepted public ID domain. Entry and exit
conversion must recover the exact public identity and retained dependency.

Use owner-relative text handles for source ranges and a separate pool for cooked,
synthetic and arbitrary-byte names. Borrow the selected bytes during lookup;
materialize an independently retained `JsString` only at an escaping API that
requires it. Test Unicode escapes, malformed bytes, WTF-8 slices, missing names,
shared source ownership, clones and foreign-owner text. Reducing the identifier
payload alone does not shrink an enum while other variants set its maximum.

Store subtree caches only on applicable composite payloads. Prototype lazy
owner-held runtime-identity storage, including the cost of initialization and
lookup; preserve lazy process-global assignment, clone identity, overlay
identity and concurrent queries. Reject a hot locking side table that trades a
small field saving for a larger CPU regression.
Measure actual identity occupancy and calls: module-instance-state caching and
ambient-module naming still request runtime node IDs during binding. Removal of
`copy_for_binding` from the exclusive path did not remove these consumers.
Likewise, deferred TypeScript JSDoc at the parse/bind endpoint does not remove
the published owner's later lazy-materialization and foreign-parent contracts.

### 4.4 A budget that includes replacements

At checkpoint 0, produce both an executable layout model and a request-traffic
model using the owned-slot census
and actual `size_of`/alignment. The following is an **initial requested-live
storage budget**, not a measured achievable layout or a sum of predicted savings.
Values are decimal MB; categories are disjoint and include replacement storage.

| Retained requested-live category | Working ceiling |
| --- | ---: |
| Syntax headers, actual payloads including inline binding fields, locators, compatibility patches/indexes and capacity | 880 |
| Syntax lists and all auxiliary backing/capacity | 140 |
| Symbols, flow records/lists, declaration backing and symbol tables | 440 |
| Unique source and other text backing, including pooled names | 190 |
| File metadata, exercised caches, retained driver/queue buffers, other directories and all remaining native requested-live bytes | 50 |
| **Total native requested-live endpoint target** | **1,700** |

The combined syntax/binding row retains the original sub-budgets of 780 MB for
syntax and 100 MB for binding fields/patches. Moving fields into concrete
payloads moves their accounting into the same physical allocation; it does not
make them free or justify counting them twice. Charge each actual allocation
once and preserve the sub-budget attribution in the layout model. These category
ceilings are provisional allocations of the whole-owner target, not independent
acceptance gates. A category miss identifies an explicit tradeoff to price;
it does not by itself reject a storage family. In particular, do not select
all-atomic word rows over ordinary typed pages merely to satisfy the 780/880 MB
split before the other retained categories are modeled.

Compare complete owned allocations for compact symbols, flow records/lists,
declaration backing, symbol tables and syntax lists using the Rust physical
census, compiled layouts, actual per-file occupancy, directory/bucket/control
costs and replacement traffic. Retain the current ceilings until that model
supports a recorded reallocation; plausible smaller used-payload totals do not
constitute available headroom. The final allocation and RSS gates remain
independent of this internal retained-live target.

Keep the current 24-byte header with its independent `u16` shape and full
`u32` payload ordinal. Packing an eight-bit shape with a 24-bit ordinal buys no
header-size reduction and creates an unnecessary narrowing case. Add a separate
four-byte identifier-text candidate to the existing eight-byte source-range
model: the bound identifier would shrink from 12 to eight bytes. Its source-end
derivation or pool handle must preserve arbitrary factory text, decoded names,
range mutation, synthetic positions, cloned/imported text and the full public
domains. Price the fallback pool, handles, conversion work and page changes;
do not treat all identifier text as an immutable suffix of its current range.
The proposed low-bit tag leaves 31 bits for either raw length or pool index;
define checked overflow/escape behavior without narrowing the public domains.
A pool entry's `(offset, len)` needs an identified, retained backing for decoded,
synthetic and foreign text. Factory hooks can observe text before the final
parser range exists. Count each fallback case before assuming the pool is small;
the current physical identifier count permits a 27,171,044-byte used-row saving,
before any pool, backing, capacity or access costs.

Hash bucket/control allocations, Arc headers and temporary indexes must be
included, not hidden outside the model. Unique source backing is charged once
even when preloaded. The table must reconcile to the native requested-live
counter, not stop at the reachable compiler census. Preload is excluded from
pipeline allocation but included in endpoint live bytes and lifetime RSS.
The residual row is a hard modeling challenge: if it is unknown or exceeded,
report that fact and revise the design, not the observed counter.

Do not spread a missed budget across categories without an explicit record of
the replacement tradeoff. A design that fits only by assuming zero map overhead,
perfect page occupancy or zero temporary allocation does not pass this checkpoint.
The model must also show tiny-file and rare-kind overhead, not only a giant file.

### 4.5 Explicit request-traffic budget, required at CP0

Use the same native allocator counters and boundaries for the identity:

`pipeline_requests = endpoint_live - pre_pipeline_live + freed_or_superseded_requests`

The last term includes freed requests and the full old requests superseded by
reallocation; it is not physical copying or a pure cohort of pipeline temporaries.
The diagnostic pre-pipeline live median was approximately 166.647 MB. If endpoint
live is 1,700 MB, a 1,900 MB pipeline allocation ceiling leaves about **366.647 MB**
for freed/superseded requests, not 200 MB. Measure the actual starting counter for
each candidate; never assume the old preload/driver accounting remains constant.

Set a provisional **350 MB** request-traffic ceiling: parse 200 MB, bind 125 MB,
and publication/scheduling 25 MB. At the old starting counter and the endpoint
ceiling, that implies 1,883.353 MB of pipeline requests. This requires about a
62% reduction from the current 923.350 MB traffic and is a design constraint,
not evidence that the reduction is available. If starting live changes, recompute
the permitted traffic and enforce the independent 1,900 MB request target too.

At CP0, model list Vec construction and final backing, scanner/cooked-text
buffers, map growth, arena/directory requests, fallback and escape storage,
freeze-time conversion and driver/scratch retention. Account for shared backing
and native realloc behavior; do not assume every Vec-to-box conversion copies.
Use disjoint global phase-counter deltas only in one-worker runs and retain an
unknown remainder. Eight-worker phases overlap: reconcile their whole-pipeline
counters, and add per-phase attribution only with separately validated accounting.
Do not sum overlapping global phase intervals or treat missing phase values as
zero traffic. A layout
that fits retained storage but has no credible traffic budget fails CP0/CP3.
Implement unavoidable list/buffer/growth changes alongside CP3/CP4; CP6 is the
remaining traffic audit, not the first time temporary costs are considered.

## 5. Execution sequence

Each checkpoint ends with a recorded keep/reject decision and the remaining
distance to all four performance gates. A locally useful improvement is not
S07 completion. Retain one approved candidate as the next control; discarded
experiments remain documented, not enabled in the production path. Each record
includes a running table of wall time, requested allocation and peak RSS at both
worker counts against the historical Go budgets, with absolute excess and source
identities. The [distance replay](../tools/s07/performance-experiments/gate-distance/README.md)
provides the A0-b/CP1 reference. Label these historical comparisons explicitly;
they supply neither fresh cross-runtime confidence bounds nor phase attribution.
Do not multiply improvements from separate screening batches.

Selected order: **CP0 → A0 → CP1 → CP3 → CP4 → CP5 → CP6 → CP7 → CP8**.
Decide immediately after A0. **CP2 stays on hold** unless A0 is rejected or a
later measured contract/cost problem requires that specific fallback. Keep the
checkpoint numbers stable so the committed initial plan remains comparable.

### Checkpoint 0 — Freeze the experiment and check feasibility

1. Record current source inventory, binary/configuration hashes, pins, workload
   digest and archived baseline captures. Preserve unrelated user changes.
2. Add diagnostic-only experiment manifests/results under
   `tools/s07/performance-experiments/`, with a short index linking this plan.
   Fields: hypothesis, parent control, touched components, predeclared variants,
   semantic counterexamples, layout budget, sample policy, actual artifacts,
   outcome, and separate wall/RSS/allocation results. No tracker success metrics.
3. Extend the existing census with payload-kind distributions, field occupancy,
   actual per-file capacities, map control bytes where observable, and an
   explicitly unknown remainder. Use existing captures where sufficient.
   Deduplicate current overlay copies when projecting replacement core payloads;
   they are not additional syntax nodes in the compact representation.
4. Build safe layout sketches for compact nodes/text with inline binding fields,
   leading with the 24-byte compact-parent candidate. Keep typed side fields as
   a fallback model, not implementation work. If activated later, CP2 must freeze
   its provisional locator's construction timing and later replacement.
   Include open kind/payload pairings, lazy additions and empty per-shape
   directories: allocating a Vec header for every shape in every tiny file can
   consume substantial storage even before the first payload is inserted.
5. Model the 350 MB request-traffic ceiling in parallel with retained layout,
   including list backing, scanner buffers and map growth. Review whether the
   1.70/1.90/2.10 GB working budgets have a plausible route.
   If not, identify the specific category that needs a different representation
   before porting thousands of accessors. No feasibility claim from Go census
   subtraction or adding overlapping profiler rows.

Exit: a reviewed ownership/API sketch, concrete module boundaries, measured
layout sizes, candidate budget and frozen diagnostic protocol. No production
optimization is claimed at this stage.

### A0 — Test single-source exclusive binding before building CP2

1. Add a consuming production entry, provisionally `bind_parsed(ParsedFile)`,
   for eligible ordinary single-source files. Preserve `bind_source_file(&AstFile)`
   as the existing published-file path. Route both the compiler cache and the
   benchmark through the same new entry; do not create a benchmark-only shortcut.
2. Use actual cache semantics: `FileCache::acquire` takes `&mut self` and stores
   weak references to completed files. A failed acquisition installs no entry.
   It has no contended pending-file once cell to replace. Drop failed private
   construction and preserve cache retry behavior; do not add negative caching.
   Install successful source bind completion with publication so later bind
   requests are no-ops. Retain the existing once/panic/waiter protocol on the
   shared published-file path rather than inventing new contention here.
3. Keep current node layout and symbol/flow field maps initially. Bind core
   syntax writes in place and bypass the full-node overlay for this path. Use
   one binder algorithm with a narrow storage interface and record its code-size
   and dispatch costs. Retain checked raw IDs and local provenance; exclusivity
   alone does not validate an arbitrary ID or permit sibling writes.
4. Resolve the main borrowing issue before broad migration: `parsed_view()`
   currently provides syntax/list borrows across recursive mutable binder calls.
   An exclusive mutable owner cannot provide that unrestricted lifetime while
   changing nodes. Prove indexed traversal, split immutable child access, or
   bounded small observations in representative container/binary/JSDoc paths.
   Reject a workaround that clones every node/list or adds a second syntax graph.
5. Classify eligibility before the first mutation and preserve the original
   input for fallback. Published inputs, mapped/multi-source storage, transformed
   shared children and unsupported imported/lazy ownership stay on the existing
   path. Do not retry through fallback after partially mutating exclusive input.
   For committed lazy nodes, either implement owner-exclusive access with a
   real uniqueness proof or select fallback up front. `StorageTransaction::node_mut`
   only accesses pending transaction nodes; it is not committed-lazy mutation.
   If binding can create such nodes after eligibility, define that path before
   starting the prototype. Record actual path counts and fallback reasons.
6. Specify the result API and post-bind observations. Existing
   `BoundFile::parsed_file()` exposes a pristine parsed view on the old route.
   The consuming route must return a bound-only capability, or another explicitly
   defined interface, rather than relabel mutated storage as that old snapshot.
   Audit ProgramFile, factories, imported owner lookup, encoder and retained
   handles. Never change existing published-file parsed-view guarantees silently.
7. Extend corpus observation to execute the exclusive entry as well as the old
   entry. Capture parsed observations before consumption, then compare complete
   bound graphs, repeated binding and explicit retention. Existing adapters that
   always publish first otherwise exercise only the fallback. Run focused
   debug/release, scope/lifetime and E3 Miri/ASan coverage for both paths, including
   success, unwind/disposal, wrong owner, eligibility fallback and lazy cases.
8. Update diagnostic CPU/memory adapters and full-workload graph observations.
   The new path executes parse→bind→publish inside the same measured interval;
   phase timers and path counts must reflect it. Keep work, queues, preloading,
   retained roots and final acceptance definitions unchanged.
9. Screen the full frozen workload in both worker modes. Require verified
   elimination of the targeted overlay operations on eligible files, a bind-time
   improvement, complete applicable parity/ownership and the section-6 pipeline
   promotion criteria. Include any new publication revalidation scan triggered
   by `builder_mut()` in costs. The field maps remain, so A0 is not expected to
   meet the final memory budget or a one-second bind by itself.

Exit: an explicit **keep exclusive / activate CP2 / reject and revise** decision,
before implementing replacement columns or the general accessor migration.
If the borrowing or result protocol needs broad duplication, or the measured
benefit does not survive complete-pipeline accounting, keep the existing path
and activate CP2. Otherwise proceed to shared compact layout with inline fields.
Do not defer this decision to CP7 or predict a calendar duration from file count.

### Checkpoint 1 — Make field reads cheap and representation-independent

The list pilots show why this must precede broad storage migration: making each
backing contiguous did not recover checked traversal speed. First compare raw
public-ID resolution with an already resolved owner borrow in a bounded access
diagnostic. Keep safe bounds checks; validate imported/foreign/stale identities
at the boundary. Establish which proofs survive real recursive binder calls and
narrow writes before treating fewer checks in a synthetic sweep as an available
production optimization. Preserve both recorded list captures as controls. The
bounded [CP1 implementation record](S07-bis-CP1.md) fixes the access comparison,
charges cached slice metadata, and separately tests a checked exclusive-core
node lookup. It requires a measured pipeline win for that leaf shortcut; the
infrastructure exception below does not apply to it. The bounded
[list-copy candidate](S07-bis-list-copy.md) tested resolution once per stack chunk
without waiting for a persistent disjoint borrow. It missed the same independent
full-pipeline win requirement and is rejected; its complete evidence is retained.

Drive the compact-node CPU comparison with a separately captured
[actual workload access trace](S07-bis-access-trace.md), including operation order,
values and narrow writes, and reconstruct the full retained working set. Small
fixtures remain semantic counterexamples. Report operation coverage separately
from node population and CPU coverage, and confirm replay results in production.

1. Introduce the borrowed facade in `tsr_ast::storage`/`node_accessors`; change
   generated accessors and factory mutations at their emitter in
   `xtask/src/gen/ast.rs` and related generator modules. Use `cargo xtask gen`;
   do not hand-edit generated outputs or alter upstream schema semantics.
2. Migrate the binder's repeated `n()`/symbol/locals reads first, then all callers
   required for the representation swap. Carry an already resolved borrow or
   small field observation across adjacent operations where mutation permits.
3. Keep backing storage unchanged for this experiment. Inspect optimized code
   and count lookups/retentions in a separate diagnostic build to detect hidden
   materialization, repeated field resolution or Arc increments.
4. Run accessor, factory, encoder, identity, byte-string and binder countertests
   in debug/release. Screen on the full workload at both worker counts.

Exit: equivalent observations with no per-read allocation/retention and no
unexplained wall-time or memory regression. This is infrastructure for the next
change; it is not justified by a large speculative future gain alone.

### Checkpoint 2 — On-hold fallback: compact binding side tables

Execute only after a recorded decision rejecting the exclusive path. This is
the initial plan's column/patch experiment, retained as an alternative; ordinary
compatibility use of today's published-file binder does not activate this work.

1. Implement the selected typed binding columns and sparse field patches. Include
   promotion/removal behavior currently split between full records and flow slots.
   Replace general overlay probing on ordinary syntax reads with field-specific
   resolution. Keep source metadata and diagnostic publication in the same cell.
2. Enter a validated binder source scope once. Mint non-forgeable local handles
   from checked imports or already validated edges; retain safe bounds access
   and debug assertions. Define callback/reentry boundaries and the fallbacks
   for imported, mapped and lazy nodes before eliding a check.
3. Resolve all field writes through the same owner proof. Keep explicit wrong
   sibling/wrong owner rejection and no partial publication after a panic.
4. Measure total replacement bytes, occupancy, request growth, patch count,
   lookup count and binder CPU. Compare complete wall/RSS/allocation results;
   removing 445 MB of old storage does not imply a 445 MB net saving.
5. Add compile-fail tests for scope escape/cross-owner mixing and execute actual
   new storage through the S06/S07 ownership instrumentation, not a test-only map.

Exit: full binder-field parity and ownership, a measured reduction in binding
storage/request traffic, and a defensible CPU result. Apply the phase milestones
below immediately; do not accumulate more replacement maps if this route still
has no measured path to both CPU gates.

### Checkpoint 3 — Integrate compact syntax and owner-relative text

1. Introduce the selected generated payload storage with independent shape tags
   and compact-parent headers. On the
   selected exclusive route, put each audited binding field in its applicable
   payload and remove its temporary map/flow-slot entry. If CP2 was activated,
   share the payload locator with its binding indexes and remove any provisional
   all-node index. Keep one syntax representation with the required binding
   backends, rather than two copies of the parser or binder algorithm.
   Give inline binding fields narrow typed setters from the start. Symbol,
   locals and flow writes may preserve the syntax-edge proof, but introduce
   binding-graph references whose owner/bounds validation must remain proved.
   `next_container` also carries a syntax-node reference. Do not expose raw
   payload mutation as proof-preserving or discard final binding-graph checks
   merely because their fields have moved out of maps.
2. Establish common tokens/identifiers/edge payloads as the first implementation
   checkpoint, including
   parser construction, mutation, publication, binding and encoding. Include
   TokenData with identifier/unknown kinds to test the independent shape tag,
   and parameter/argument list edges with recursively nested list construction.
   Compare direct owner-local `u32` edge storage with the current backing path,
   charging staging, segmentation, reservation or completion compaction needed
   to preserve contiguous/shared slices and distinct identities. An append-only
   global edge buffer alone does not solve nested-list interleaving. Attribute
   initial Vec construction separately from list completion and auxiliary growth.
   Preserve nil/allocated-empty/missing states, locations and modifiers in this
   checkpoint. Validate the contextual access/construction contract before
   extending generation; the pipeline candidate must cover all 192 shapes.
3. Migrate the remaining concrete shapes and auxiliary/list storage. Preserve
   backing identity, nil versus allocated-empty lists, parent updates, cycles,
   shared children and clone/update behavior. Do not compact mutable lists by
   copying backing whose identity is observable.
4. Introduce source/pool text handles and compact internal edge forms. Charge
   foreign escape maps, pooling metadata and conversions; keep general owned
   `JsString` APIs where they are required. Avoid a global interner or global lock.
5. Move composite caches and test the runtime-ID storage candidate separately.
   Retain the existing runtime-ID field if the proposed replacement loses its
   CPU/memory tradeoff; record how the remaining budget is then met.
6. Update diagnostic observers for the new representation and recalibrate them.
   Their old patches must fail visibly on incompatible source, not silently
   omit new payloads. Recheck actual layouts on all four supported native targets.

Exit: full AST/parser/binder/encoder parity and ownership, a new complete census,
and a measured candidate whose live storage and request traffic can fit the total
budget. For the vertical slice, compare hot flag reads, child enumeration and
representative binder traversal against A0-b, alongside generated-accessor
complexity and actual requests, allocation calls, initialization, live bytes
and RSS. CPU is the principal unresolved selection question, not the sole exit
criterion: the model leaves real owner storage unpriced. Do not promote on a
microbenchmark or modeled byte count alone.
Apply the bind/parse milestones below before continuing. Implement list
construction and capacity changes needed to satisfy CP0's traffic model here;
do not defer a known structural allocation miss to CP6.
Do not continue polishing a node representation that makes the memory target
impossible even with optimistic remaining categories.

### Checkpoint 4 — Compact symbols, flows and list/table storage

1. Audit actual fields and source-defined nil/union distinctions in `symbols.rs`,
   `flow.rs` and list backing. Use owning-result-local references where proven;
   preserve explicit full IDs at imports and escaping results.
2. Prototype compact flow discriminants and union payloads rather than assuming
   every current 48-byte record can become 32 bytes. Likewise, do not budget an
   80-byte symbol until the current 112-byte record has a measured replacement.
3. Reduce declaration-list and flow-list overhead while preserving ordered
   entries, shared backing, cycles and obsolete backing still reachable by
   observations. Count allocated records as well as reachable records.
4. Audit table keys and clone traffic. Start by borrowing name bytes on lookup
   and avoiding redundant retains; symbol-table ordering remains produced at the
   existing observation points. Under ADR 0007, tables remain byte-semantic
   `JsString`-keyed maps unless an explicit equivalent facade/ADR amendment is
   recorded. Internal text pooling does not authorize identity-based equality.
5. Compare the current randomized hasher with at most one justified fast keyed
   candidate. Verify licenses, MSRV, hostile-name collision behavior and the
   real end-to-end benefit before adding a dependency or changing hashing.

Exit: all symbol/flow graph and declaration-backing observations preserved;
measured combined storage/request reduction. Hashing is optional if its gain is
below the diagnostic decision threshold or another path remains dominant.
Reassess the bind milestone now if it was missed at CP3; do not postpone a
dominant access/storage cost until the final profiling checkpoint.

### Checkpoint 5 — Remove proved duplicate validation work

1. Inventory construction, mutation, imported-edge, source-metadata and final
   validation obligations. Construction validation is not equivalent to final
   validation: parents and payloads can change after insertion, including hooks.
2. Add a private trusted parser-construction path only where local handles and
   retained imports prove the same edges valid. Keep checked public factory
   entry points and their immediate ownership errors.
3. Initially retain completion validation and remove only repeated edge checks
   covered by that proof. A later reduction of completion scanning needs proof
   for every mutation/auxiliary path and its own measured experiment.
4. Countertest wrong owner, out-of-range IDs, post-construction mutation, invalid
   metadata/list edges, callback reentry and lazy commit rollback in release.
5. Reprofile parse CPU after the layout changes. Compare actual elapsed time;
   do not translate the old 14.5% combined attribution into expected savings.

Exit: unchanged failure/ownership behavior and a measured parse improvement.
Skip or reject check elision if the proof requires broad new unsafe authority.

### Checkpoint 6 — Reduce allocation traffic and capacity waste

1. Re-run the allocation-source census on the surviving design. Expand the
   parser's currently unclassified regions if they are still material. Locate
   actual map rehashes, vector replacement, unused page capacity, cooked-string
   temporaries and repeated list compaction before choosing reservations.
2. Prefer exact counts already available from construction. Compare bounded
   per-file hints only when counts are unavailable; use file-size/kind histograms
   and tiny-file/adversarial cases. Do not hardcode nodes/8 or nodes/6.6 as a
   universal allocation requirement or tune to individual workload paths.
3. Compare page size/directory reservation separately from payload representation.
   Both runtimes already keep old pages stable; reserve only where it reduces
   measured request traffic without excessive slack or new giant allocations.
4. Consider reusable per-worker scratch for genuinely temporary parser/binder
   work. Include initialization and retained scratch in the measured pipeline
   and RSS. No excluded warmup may preallocate compiler storage for a child.
5. Do not compact by allocating a second complete graph then discarding the
   first: that can lower endpoint live bytes while failing allocation and peak
   RSS. Any freeze-time compaction must report its transient/request costs.

Exit: request traffic and capacity fit the combined candidate's budget, with
bounded small-file overhead and no hidden work outside the measurement interval.

### Checkpoint 7 — Resolve the remaining CPU/RSS gap

1. Take new CPU and retained-memory profiles of the combined candidate. Rank
   current costs; the baseline's ordering is no longer assumed correct.
2. If CPU misses, separate useful traversal, remaining field resolution,
   allocations, hash work, stack guards and waiting. Capture waiting/lock evidence
   if needed; the existing Running-only samples cannot rule out contention.
3. If live storage fits but RSS misses, inspect allocator statistics and native
   VM domains before changing allocation strategy. Report stacks, mappings,
   rounding, retained free pages and unknowns separately where observable.
4. Prefer fixing demonstrated size-class/page/retirement behavior under the
   existing mimalloc policy. Mimalloc remains fixed in the selected plan and
   existing acceptance validators. A different allocator needs a separate
   reviewed amendment to ADR 0016, the producer and report validator, artifact
   configuration, native accounting tests and diagnostic calibration before
   it can become an eligible production candidate. That amendment must preserve
   thresholds, work and measurement domains; both normal and allocation builds
   must use the same selected allocator. Never force collection, trim
   after the fact, drop roots early, subtract baseline RSS or alter Go's GC.
5. Review only unresolved measured costs; A0's architecture decision has already
   happened. If the exclusive candidate regresses, compare the specific CP2
   fallback or a named storage fix using current evidence. Do not reopen a
   wholesale published-file rewrite without a measured reason. If no examined
   candidate can pass, report the blocker and next specific experiment.
   Do not declare the targets impossible from this plan or relax them to finish.

Exit: either a combined candidate ready for acceptance, or a documented decision
to investigate a named remaining cost. This checkpoint
prevents an endless sequence of small optimizations with no route to the gates.

### Checkpoint 8 — Independent review and final acceptance

1. Review ownership/semantics and performance/accounting independently. Reviewers
   receive the final representation, all failed candidate records and raw results.
   Review borrowed facade materialization, fallback correctness and retained roots
   explicitly; a generic code review is insufficient for these changes.
2. Remove obsolete production paths and temporary dual-representation features.
   Update generator manifests, provenance/coverage mappings and current design
   documentation. Stage the final file inventory before recording evidence.
3. Run the affected correctness producers and all native checks listed below.
   Fix failures before freezing the exact candidate for full workload parity.
4. Run fresh full `bindworkload` graph comparison on the exact normal binaries,
   then one complete unchanged E5/E6 capture. Consume that same capture for both
   producers, inspect actual gate results, and verify the whole S07 exit.
5. Deliver implementation through the separate S07-bis branch and PR, based on
   the S07 lineage. Use new commits and normal pushes there; do not add these
   plans or performance work to the S07 branch/PR. Do not rewrite published
   history or stage unrelated work. Preserve failed captures and final raw
   artifacts with their provenance.

Done means `cargo xtask check S07` passes with current evidence. It does not
mean full E3, E5's checker per-type footprint, or any future checker sprint passes.

### Early CPU continuation exits

Use matched phase observations on the designated host to decide whether to
continue a representation, not just a distant final wall-time target:

| Point | Required observation / continuation decision |
| --- | --- |
| A0, before CP2 or general facade migration | Targeted bind-time reduction, measured bypass of overlay operations, and section-6 complete-pipeline promotion; otherwise reject/revise A0 or activate CP2 immediately |
| Completed binding-field storage, CP3 on the selected route or CP2 on the fallback | Aim for one-worker bind elapsed near or below 1.0 s; if missed, profile and name the next bounded change or reconsider the storage path before proceeding |
| Compact syntax at CP3, rechecked after validation at CP5 | Aim for one-worker parse elapsed near or below 2.0 s; if missed, resolve the measured construction/access/validation cost before proceeding automatically |
| Every integrated checkpoint | Record both full wall ratios, both memory ratios and remaining headroom; a good isolated phase is insufficient |

These are review exits and engineering milestones, not promised A0 results or
new acceptance thresholds. A miss requires a recorded measurement and decision
before continuing; it cannot be waved through solely because correctness passed.
Compare fresh Go and control timings alongside the nominal phase values so
environmental changes are not mistaken for an implementation gain.

One second of bind plus two seconds of parse already totals three seconds before
orchestration, above the old 2.938-second one-worker gate. The internal 0.90
headroom goal would be about 2.644 seconds at that Go median. Reaching these
milestones therefore still requires further measured improvement; record that
remaining combined budget explicitly. One-worker phase elapsed, eight-worker
summed worker elapsed and sampled Running CPU are distinct. Final acceptance
still uses uninstrumented whole-pipeline wall time and its unchanged uncertainty
gate, not these diagnostic phase timers.

## 6. Screening and decision rules

### Establish absolute relevance before applying ratios

For every candidate, first report absolute CPU deltas in milliseconds and memory
deltas in decimal MB, alongside the complete-pipeline control and the applicable
Go-derived gate budgets. Keep pipeline requests, retained requested-live storage
and peak RSS distinct. State the source revision, worker mode, workload,
measurement endpoints, invocation counts and denominator for every conversion.
Historical Go medians contextualize scale; fresh Go measurements still determine
the final gates.

A microbenchmark ratio is not a pipeline regression or an early veto. Divide
multi-sweep timings by the declared sweep count and show the absolute difference.
If actual production frequency and matching operations are measured, report the
resulting estimate with its assumptions. Otherwise show conditional sensitivity
rows and label the production effect unknown; do not invent a pass count or
describe a raw-slice baseline as the full production access path. Shared memory
and whole-program cache effects also prevent an isolated difference from being
a rigorous upper bound on integration cost.

For example, the first page-256 replay saved 16.874 ms of construction,
200.942 MB of requests and 46.784 MB of retained storage, while eight synthetic
sweeps added 33.493 ms (4.187 ms per sweep on average). Two/three equivalent
sweeps would add 8.373/12.560 ms before construction, but those frequencies and
cost equivalences have not been established for the compiler. The request saving
is about 9.9% of the historical 2,035 MB allocation budget; it is a reason to
prioritize integration, not a measured reduction in the production allocator.
See the [review](S07-bis-list-tradeoff-review.md) for the baseline and proof limits.

Use these absolute costs and remaining budget gaps to rank bounded experiments.
Keep semantic validity and complete replacement costs mandatory. In particular,
page-backed lists enter CP3 with a named resolved-reader contract; matching
legacy's isolated traversal percentage is not a prerequisite. A component may
remain a leading candidate while its integrated CPU/memory effect is unmeasured.
The screening rules below apply to actual complete-pipeline measurements, never
to microbenchmark percentages or hypothetical conversions. Distinguish whether
to invest in another experiment, retain reviewed code, promote a measured control,
or accept S07; those decisions do not have identical thresholds.

### Components, combined experiments and small improvements

The compact slice is one combined candidate by default. Its header, payloads,
inline binding fields, edge storage and dependent CPU repairs structurally trade
costs. Intermediate full-pipeline screens attribute those costs; they are not a
requirement that every component independently pass before integration. Keep
graph parity and ownership checks during implementation. A CPU-regressing
memory change can remain on the experimental branch while its named dependent
CPU work is built. Record the regression, next dependency and remaining gate
distances; do not call it an accepted performance control.

Judge the measured combination against the complete vector of CPU modes,
allocation and RSS, with the same workload and same-screen control. No weighted
score permits a failed final gate, and no arithmetic sum of separate experiments
predicts a combined result. Normal screening tolerances below account for noise;
they do not require every metric to improve strictly. Final Go-relative gates
remain unchanged.

Use a likely 5% pipeline effect to prioritize *further experimental investment*.
Do not discard an already measured, reviewed and semantically clean small CPU
improvement solely because its median win is below 5%. It may be retained when
at least one timing mode has upper 95% candidate/control ratio < 1.0, all metrics
meet the non-regression/noise conditions below, and it does not complicate a
surface scheduled for replacement. Record its absolute saving and maintenance
cost. This is a prospective retention rule, not permission to relabel historical
screens or to assume a small improvement survives a storage migration.

The earlier list-copy and lookup-reuse implementations remain archived and
removed. Their original screens used the original 5% rule. Both modified binder
access that the compact facade subsequently replaced; restoring them now would
require adapting and measuring the actual combination. No new experiment in
those low-payoff directions is scheduled just to recover a historical percentage.

### Complete-pipeline capture and promotion

Implement a small diagnostic runner under the checkpoint-0 tools directory,
reusing existing build/input/protocol validation. It may compare isolated
checkouts/build artifacts; it must not infer an executable from a guessed path
or rebuild concurrently with timing. The implemented commands and immutable artifact workflow are documented in
[the runner README](../tools/s07/performance-experiments/README.md).

Existing build helpers copy executables into shared cache destinations. After
each variant build, immediately copy its normal/allocation executable and
required runtime artifacts into a distinct immutable experiment directory.
Bind each to that variant's source/configuration and check hashes before and
after every sampling batch. Never measure both variants from the shared helper
paths. Negative runner tests must reject overwritten/swapped binaries, changed
loaded bytes/options, partial sample sets and missing allocation observations.

Use the full frozen 13,094-file workload for screening. Its native runs are
short enough to avoid introducing a conveniently selected performance subset.
For each predeclared candidate, use one validated warmup and seven fresh-process
control/candidate pairs at both worker counts, separately for normal and
allocation builds; alternate order. Keep all rows, failures and stderr.
Screening validates work counts and loaded input identity; it emits no E5/E6
metrics and cannot substitute for complete graph comparison.

The default substantial-candidate promotion rule is a demonstrated reduction in
the targeted cost and a median candidate/control ratio <= 0.95 in at least one full-pipeline
metric/mode. Every timing mode must additionally have bootstrap upper 95%
candidate/control ratio <= 1.02 and both variants' relative MAD <= 5%; a claimed
timing win needs upper < 1.0. Use the existing fixed bootstrap algorithm for this
diagnostic comparison, with candidate/control identities named explicitly.
For RSS and allocation, require median candidate/control ratios <= 1.02 and
both variants' relative MAD <= 5% in every mode; report all samples/ranges and
make no confidence-bound claim for those two domains. These are screening rules,
not the final E6 requirement, whose upper bound remains <= 1.0 against Go.

If those noise/upper-bound conditions fail, mark the screen inconclusive or
regressing; do not silently extend it. A necessary representation-infrastructure
step can be retained after review without a 5% win only if it meets every 1.02
non-regression/noise condition above, with its dependent checkpoint named.
Any other tradeoff requires a recorded combined-candidate result showing a
better route to all gates; no individual savings are added to predict that result.
This limits promotion, not temporary retention of a named component in the
combined experiment. The reviewed-small-improvement rule above is a separate
retention path; preserve the runner's original screen verdict and record the
reason for retaining it rather than rewriting the captured evidence.

Before promoting a complete storage slice as the next implementation control,
run full relevant parity and ownership evidence. A diagnostic winner alone is
not permission to leave its semantic fallback untested. Re-run screening only
for a changed candidate or documented environmental/instrumentation defect;
do not repeat an identical candidate until a favorable batch appears.

Useful supplementary guardrails are tiny files, declaration-heavy files,
escaped-name/JSX/JSDoc cases, sparse multi-source owners, imported/lazy graphs,
deep input and hostile identifier sets. Their selection is committed before
results; they never replace the acceptance denominator. Existing graph
`--diagnostic` mode still uses the full request set, not a subset selector.

## 7. Validation and unchanged final measurement

Each checkpoint runs affected debug/release tests and focused semantic and
compile-fail counterexamples. Representation/ownership changes must also enter
the real E3 scenario manifests and execute under strict-provenance Miri and
ASan with rebuilt std. Preserve existing S04/S06/S07 scenario scope; no bare
instrumentation boolean may complete unrelated future criteria.

Before final acceptance, run workspace debug/release and doc tests, pinned
MSRV/all-target/all-feature checks, generator verification, formatting, Clippy,
dependency policy and tracker self-tests. Build and run affected native paths
on macOS arm64/x86_64 and Ubuntu arm64/x86_64, including stack growth and
allocator accounting. CI checkouts must initialize embedded upstream assets.

At minimum, AST/arena changes require fresh `scanner`, `e1`, `binder`,
`program`, `e3`, `bindworkload`, `e5`, `e6` and affected quality evidence.
AST/generator edits also renew `gen`; an arena-only implementation edit does
not invalidate its deliberately narrowed source closure.
Inspect [every declared input closure](../status/runs.toml): renew `e4`, `e2`,
`workspace`, `oracle` and prerequisite sprint evidence whenever those inputs
change. Do not broaden source globs merely for convenient bookkeeping.

Final commands, after all source and file-inventory changes are frozen:

```sh
cargo xtask run bindworkload
python3 scripts/s07_benchmark.py capture
python3 scripts/s07_benchmark.py verify-capture
cargo xtask run e5
cargo xtask run e6
cargo xtask validate
cargo xtask status
cargo xtask check S07
cargo xtask status --check-committed
```

Archive each previous capture before using the standard output directory; the
consumer commands above read that explicit complete current capture. Evidence
must bind current sources, actual normal/allocation executables, registry lock,
effective Cargo configuration, toolchains, workload transport and full graphs.

The final statistical policy remains one warmup, seven recorded samples per
runtime/mode/configuration, alternating runtime order. Timing extends to 14 or
21 only under the existing fixed uncertainty/noise rule. Clear failures stop;
unresolved uncertainty at 21 fails. Keep the 10,000-resample bootstrap, seed
`0x5307`, fixed order-statistic bounds and both-runtime MAD conditions unchanged.

Under the selected, unchanged allocator policy, retain the normal Rust mimalloc
executable for timing/RSS, a separately
identified cap build for original-layout allocation requests, and normal Go
`GOGC=100`, no imposed `GOMEMLIMIT`, `GOMAXPROCS` equal to workers, and
`GOTOOLCHAIN=local`. The exclusive path uses per-file parse→bind→publish; the
existing published-file path uses parse→publish→bind. Include every operation
inside the same combined pipeline interval, record path/phase provenance and
preserve bounded queues,
retained endpoint roots, and the distinct preload/worker/pipeline boundaries.
Requested allocation, live storage and lifetime peak RSS are never substituted
for one another. Toolchain pins come from their existing manifests, not this doc.

## 8. Decisions deferred to measured checkpoints

The implementation path is selected. These are bounded engineering decisions
within it, not reasons to postpone starting checkpoint 0:

| Decision | Default / bounded challenger | Resolve by |
| --- | --- | --- |
| Binding lifecycle | Consuming single-source entry + existing published-file compatibility / CP2 side-table fallback | A0, before general facade or column implementation |
| Binding fields | Applicable concrete payload fields / eligible-shape columns only if CP2 activated | 0 model, A0 direction, 3 integration |
| Node layout | Compact-parent ~24-byte header + per-shape pages / 32-byte full-parent control only if budget fits | 0 model, 3 vertical slice |
| Internal edge/text width | Owner-relative handles + full-range escapes / retain full ID where compression loses | 3 correctness and census |
| Runtime identity cache | Lazy owner-held storage / existing inline field | 3 identity traces and timing |
| Symbol hashing | Current keyed hashing / one reviewed fast keyed candidate | 4 observed hash cost |
| Validation | Keep final scan, remove proved repeated construction checks / later completion proof | 5 counterexamples and profile |
| Capacity and traffic | Explicit 350 MB traffic model, exact counts and bounded hints / current pages | 0 budget, 3/4 structural implementation, 6 remaining audit |
| General published-file rewrite | On hold; existing semantics remain on the compatibility path | Only a later measured fallback bottleneck warrants reconsideration |

No checkpoint is credited in advance. The implementation record must state the
last completed checkpoint, accepted/rejected candidates, current four ratios,
remaining uncertainty and the next experiment capable of changing the decision.
