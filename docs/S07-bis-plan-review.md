# S07-bis plan review

Date: 2026-09-08. Scope: implementation plans, not new production code or new
performance evidence. The [revised primary plan](S07-bis-performance-plan.md)
now selects the early single-source exclusive prototype. The
[exclusive-binding record](S07-bis-exclusive-binding-alternative.md) activates
that bounded path and keeps a general published-file rewrite on hold.

Sections 1–3 below record the initial review of the plan frozen by the user in
`4173b89`. The subsequent Claude review and new decision are recorded afterward;
the initial reviews are not presented as approval of changes made later.

## 1. Initial inputs and review questions

The author re-read the current Rust guide, S07/ADR ownership and symbol contracts,
the S07 implementation and binding operation records, CPU/memory reports, actual
storage/generator interfaces, and the benchmark producers/statistics/consumers.
Existing local edits outside these plan documents are not part of the change.

Two independent review passes covered separate areas:

- Storage/API/ownership: whether compact fields and payloads can preserve current
  read, mutation, identity, mapped-source, lazy and failure contracts; whether the
  sequencing supplies the indexes required by an earlier checkpoint.
- Measurement/gates: exact thresholds and stopping rules, replacement accounting,
  control/candidate provenance, evidence dependencies and final acceptance.

The reviews found no unresolved contract or measurement blocker in the selected
plan. Architectural selection remains the author's decision. The reviews did
not establish that provisional layout budgets or eventual gate results are
achievable; checkpoint 0 must test that.

## 2. Initial findings and amendments

| Finding | Amendment |
| --- | --- |
| "Per-kind" could incorrectly imply selecting physical payloads from syntax kind | Require a separate concrete payload-shape tag; retain open kind/data pairings, with TokenData + identifier/unknown-kind counterexamples |
| Typed binding columns need a locator before typed AST pages exist | Require an explicitly selected provisional index, charged construction/scan timing, source partition and lazy fallback, and a concrete reuse/removal decision in checkpoint 3 |
| A 24-byte header silently depends on compressed parent storage | Model a full-parent candidate first; state the compact-parent/escape and separate-shape requirements for the smaller target |
| Existing build helpers overwrite shared executable destinations | Require immutable per-variant normal/allocation artifacts with before/after hashes and negative overwritten/swapped-binary tests |
| Optional allocator replacement contradicted the fixed final producer/validator policy | Keep mimalloc selected; any alternative requires a separate ADR/configuration/producer/validator/accounting/calibration amendment with unchanged gates |
| The held alternative inherited an incompatible parse→publish→bind ordering | Explicitly map parse→bind→publish into the same combined interval; preserve all work, roots and excluded setup boundaries, and update diagnostic phase provenance |
| Arena-only edits were said to invalidate generator evidence | Distinguish AST/generator edits from arena-only edits; renew evidence from actual narrowed input closures |
| Screening's 5% improvement / 2% regression wording was statistically ambiguous | Define median thresholds, timing bootstrap upper bounds, both-variant MAD limits, RSS/allocation uncertainty treatment and neutral-infrastructure exceptions numerically |

Author review also added per-shape empty-directory costs, rejected all-node
index duplication, distinguished source-backed from cooked/synthetic text,
and kept full public ID range/foreign escapes explicit. Source census/live
bytes, cumulative requests and OS peak RSS remain separate budgets.

## 3. Initial remaining decisions and validation

No reviewed correctness or gate requirement is intentionally deferred. Physical
representation choices remain experimental: direct columns versus packed sparse
pages, the provisional locator, compact edge/text encodings, runtime-ID storage,
and page/capacity policy. Each has an assigned checkpoint and rejection rule.

The 1.70 GB live-storage, 1.90 GB allocation, 2.10 GB RSS and 0.90 CPU-ratio
working targets are engineering headroom objectives, not changed sprint gates
or forecasts. The plan cannot guarantee success before implementation and
measurement. A missed budget triggers a named design decision, not optimistic
addition of independently estimated savings.

Documentation validation checks local links/anchors, arithmetic, exact gate
definitions, whitespace and existing tracker/provenance consistency. No compiler
tests, new profiles or acceptance captures are claimed for this planning change.

Validation passed: 29 local links/anchors across the four affected documents,
the 1,700 MB budget sum, whitespace checks, `cargo xtask validate` and
`cargo xtask status --check-committed`. Both reviewers verified the amendments
in their respective scopes. Initial delivery was uncommitted on `codex/s07-bis`;
the user subsequently committed the reference plans in `4173b89`. That commit
and the S07 history remain unchanged.

## 4. Claude's second review: decision and disposition

The author accepts the sequencing criticism and changes the selected path to
**CP0 → single-source A0 → immediate architecture decision → shared layout work**.
CP2's replacement columns/locator/patch store is now on hold as the fallback.
The initial plan unnecessarily made a general publication protocol replacement
the prerequisite for trying the ordinary exclusive cache path. The original
mapped-sibling finding required independent binding, not overlays for every file.

Two new independent audits checked actual production callers, exclusive/lazy
storage and binder borrowing; and separately checked request/live accounting,
header arithmetic and phase milestone definitions. Their code findings support
trying A0 first. They do not establish its implementation cost or eventual gain.

| Finding | Disposition in the revised plan |
| --- | --- |
| Cache and benchmark bind before exposing a result | Accepted; both use one proposed consuming production entry, tested before CP2 |
| A once-cell swap describes the whole change | Qualified: FileCache is an exclusively borrowed weak cache of completed entries, not a pending once cell. Preserve retry/no-entry-on-failure behavior |
| Existing published readers/mapped inputs prevent the exclusive pilot | Rejected as a universal obstacle; keep those inputs on the existing path with eligibility decided before mutation |
| Retaining today's binder is not a second full implementation | Accepted; require one shared binder algorithm with two storage backends, while measuring dispatch/code-size and testing both |
| The binder can immediately use mutable construction | New concrete issue: parsed node/list borrows currently span recursive mutable calls. A0 must prove traversal without per-node/list copies or a duplicate graph |
| Lazy transaction mutation supplies committed lazy access | Corrected: it handles pending nodes only. Implement checked owner-exclusive committed-page access or explicitly select fallback before mutation |
| No pre-bind read escapes, therefore view semantics need no changes | Qualified: BoundFile::parsed_file still exposes parsed state after success. Specify a truthful new bound-result capability and retain old published-file guarantees |
| A0 removes the large binding-field maps | Not yet: initial A0 retains them and isolates node-overlay/access costs; compact inline payload fields remove them in CP3 |
| Temporary request traffic needs a CP0 budget | Accepted; model the full native counter identity and adopt a provisional 350 MB traffic ceiling with charged list/buffer/map/conversion work |
| 1.70 GB live inside 1.90 GB allocation leaves exactly 200 MB for traffic | Corrected: pipeline-start live is excluded from allocation. At the observed 166.647 MB starting live, the allowance is 366.647 MB; remeasure for every candidate |
| The 24-byte compact-parent header should lead | Accepted; retain the 32-byte header as a control/fallback only when its complete weighted budget fits |
| Per-phase CPU continuation exits are needed early | Accepted: targeted A0 improvement, near-1 s bind and near-2 s parse milestones at the relevant storage steps, with explicit immediate review on a miss |
| 1 s bind + 2 s parse predicts passing | Corrected: 3 s already exceeds the old 2.938 s pipeline median gate before orchestration; full-wall headroom and the unchanged statistical gate remain required |

The phase/corpus adapters also need explicit dual-path coverage. The current
publish-first corpus adapter would otherwise continue proving only fallback
behavior while cache/benchmark used the new entry. Exclusive measurements map
parse→bind→publish into the same combined interval and retain all original work.

The revised native endpoint budget includes retained driver/queue allocations
and every remaining requested-live byte. Syntax and inline-binding storage share
one physical allocation budget; the 780/100 MB sub-budgets are not counted twice.
Request traffic is planned concurrently with layout; known list/growth costs are
implemented with CP3/CP4 rather than deferred automatically to CP6.

Both new reviewers verified the amended plans in their respective scopes.
Final clarifications restrict disjoint global phase-counter attribution to one
worker and distinguish syntax payload bytes from inline binding bytes. Eight
workers require whole-pipeline reconciliation unless separate phase attribution
is validated. Replacement projections deduplicate old overlay copies.

Revised-document validation passes 34 local links/anchors, both budget sums and
the request/live equation, whitespace checks, `cargo xtask validate` and
`cargo xtask status --check-committed`. No implementation, benchmark run, new
acceptance result or history rewrite is claimed by these amendments.

## 5. First implementation checkpoint review

The selected route was implemented in `58db8c2`; the predeclared narrow flag-write
revision is `584a7fe`. An independent implementation review checked eligibility,
private-owner failure cleanup, retained API capabilities, recursive traversal
and validation-proof boundaries. It found no substantive unresolved defect in
the final A0-b candidate. This is a new review of the implementation, separate
from the earlier plan reviews above.

The initial A0 capture is retained as rejected: its timing upper bounds exceed
the checkpoint guard and it demonstrates no pipeline wall-time win. The revised
A0-b candidate passes both full graph modes, the actual four-mode E3 ownership
producer and the fixed pipeline screen. Independent recomputation agrees with
every reported median/MAD and the promotion decision. The phase probe confirms
a lower combined binding/publication interval without moving publication or
validation out of that timer; parse remains essentially unchanged.

Decision: keep A0-b as the next experimental control, with CP2 still on hold.
The [implementation record](S07-bis-A0.md) contains the measured values and
durable raw-result references. Promotion applies to this frozen workload on
macOS ARM against the previous Rust control. It does not establish fallback
performance, eliminate the binding-field maps, validate a future compact node
layout, or satisfy any Go-relative acceptance gate. The next layout checkpoint
must account for actual shape occupancy, directories, escape storage, runtime
IDs and request traffic before the general accessor migration.

## 6. Layout-budget review after A0

Claude's subsequent layout assessment correctly challenges treating the 780 MB
syntax allocation as a fixed design constraint. It was an initial division of
the 1,700 MB whole-owner working target, not an acceptance gate. The preceding
CP0 result let that split favor all-atomic word rows before the other categories
were modeled. The amended next step completes the owner budget and evaluates
tighter identifier text before selecting typed pages or word-class storage.
No numerical category reallocation or new performance result is claimed yet.

Two independent read-only audits checked the physical census and accounting,
and separately checked the header, payload and text contracts. The disposition
is as follows:

| Claim or proposal | Verified result and decision |
| --- | --- |
| 24 bytes is the floor and needs an eight-bit shape plus 24-bit ordinal | Keep 24 bytes as the preferred design, not a proved universal minimum. The compiled existing header already fits an independent `u16` shape and full `u32` ordinal. Packing them narrows the domain without saving header bytes |
| Zero-payload tokens, local links, composite-only facts and sparse/small pages make the budget possible | Those properties are already charged in CP0. They are not additional savings against its reported totals. Full-range link escapes remain required, including a valid maximum local slot if MAX is reserved |
| Bound identifiers can occupy eight bytes | A useful new variant: four bytes of text encoding plus four of flow. CP0 currently models 12, not the production 32-byte string representation. Rust has 6,792,761 physical identifiers, so the additional used-byte saving is 27,171,044 before pool, capacity and conversion costs |
| Text is recoverable from end minus raw length except for Unicode escapes | Incomplete fallback contract. Arbitrary factory text, decoded names, independent range edits, negative synthetic positions and cross-source clones also require preservation. A range edit must not silently change identifier text |
| BinaryExpression payload is 20 bytes | Five compact syntax links cost 20; composite facts add four and the applicable symbol adds four. The existing bound model correctly charges 28 bytes |
| Roughly 25% payload slack is sufficient | Not a property established for replacement pages. Use the actual per-file shape counts and explicit growth/directory policies; the recorded matrix already does so |
| Other categories provide 200–250 MB of headroom | Plausible design targets, not established spare capacity. Model their complete replacements before reallocating the budget; the sensitivity calculations below retain unpriced costs explicitly |

The original physical counts come from `native/rust-1-0-census.json` in the
[memory archive](../tools/s07/memory-profile/results/2026-09-08/raw-captures.tar.xz).
The [layout projection](../tools/s07/performance-experiments/phases/layout-projection.md)
uses the later separate core-shape census. These have different scopes; Go's
reachable counts and mixed backing/header counters are not interchangeable with
Rust's allocated physical records.

For symbols, flows, flow lists, declaration backing and symbol tables, applying
the proposed compact widths, additionally assuming eight-byte declaration-backing
descriptors and 16-byte symbol-table headers, gives **231.27 MB of used storage**.
Those two descriptor widths are assumptions of this sensitivity, not explicit
widths established in Claude's text. Applying the
observed capacities instead gives **297.99 MB**, with another **17.64 MB** for
the existing five arena directories. That 315.64 MB sensitivity is still before
hash controls, synthetic flow payloads, runtime identities, escapes and pooling.
It is not a lower bound for a redesigned page policy or a compiled replacement.
In particular, 48-byte symbols depend on a four-byte name handle and relocated
runtime identity; 16-byte flows need an independent discriminant and charged
exceptional payloads. The public symbol-table entry capacity alone is 1.605
times used entries, before bucket/control storage, so 1.5 times is not an
observed complete hash-overhead factor.

Syntax lists contain **6,047,867 elements**, **3,511,932 NodeList headers** and
**3,114,989 distinct backing records**. The quoted 4.9 million figure mixes
declaration-list census counters. Even assumed 16-byte list headers plus four-byte
elements use 80.38 MB before backing descriptors, capacity and other auxiliary
metadata. The header must still preserve locations, modifiers and slice identity.

Unique `JsString` backing currently occupies **171.17 MB**, plus **11.22 MB** of
separately estimated Arc headers. A 175 MB text category needs a real pooling
and ownership design; the 161.74 MB source input alone does not price its indexes,
descriptors or other text. The **257.48 MB** baseline native-live residual remains
unattributed and cannot silently become the 50 MB other/unknown allowance.

The proposed optimistic rows sum to 1,445–1,525 MB including the existing 50 MB
other allowance. Under the recorded 166.647 MB starting-live counter, those
endpoints would permit 541.647–621.647 MB of freed/superseded requests inside
the 1,900 MB request target. Even at 1,700 MB endpoint live the allowance is
366.647 MB, not 200 MB. These are conditional accounting identities, not measured
headroom or evidence that the current 923.35 MB traffic can be reduced that far.

Decision: retain A0-b and the 24-byte/full-ordinal header baseline. Finish the
whole-owner and four-byte identifier-text models, then compare ordinary typed
pages, scalar word rows with separate facts, and all-atomic word rows on complete
storage and hot-access costs. The existing eight-/16-row word policies remain
recorded candidates, not the selected production representation. Category misses
require an explicit whole-budget tradeoff; final CPU, allocation and RSS gates
remain unchanged. No Rust implementation or capture was changed by this review.

## 7. Pilot review: mixed rows, chunks, list traffic and proof boundaries

Claude's next review accepts the existing independent shape tag and full ordinal,
and adds useful pilot candidates. Three focused independent audits checked
compiled layouts, source contracts and the archived allocation evidence. The
[additive diagnostics](../tools/s07/performance-experiments/layout-followups/README.md)
record the arithmetic and reproduction commands. A0-b remains the control;
no production layout or new timing result is selected by this amendment.

| Claim or proposal | Verified result and decision |
| --- | --- |
| Four-byte tagged identifier text saves about 26 MB and needs only a small exception pool | The Rust physical count gives 27.171 MB of used-row savings. Keep the candidate, but pool occupancy and total cost are unmeasured. A low-bit tag leaves 31-bit length/index domains and requires checked full-range escapes. `(offset, len)` alone retains no backing for cooked, synthetic or foreign text |
| Mixed rows keep one inline atomic facts field and ordinary payload loads | Accepted and modeled now. Non-composite rows need no atomic; composite plain-word count excludes the existing facts word. Used payload bytes remain 298.696 MB, but splitting classes produces 23 classes / 177,234 active file-class pairs instead of 15 / 137,282 |
| Equal mixed-row widths imply a small directory delta | The compiled and replayed model prices it: matched eight-/16-row policies use 835.907 / 849.147 MB, adding 5.022 / 9.106 MB and 102,963 / 85,913 allocation calls. These are modeled storage totals, not CPU results |
| All-atomic fields cannot be merged or retained in registers | Too absolute. LLVM permits some Monotonic CSE/DSE and other special cases; private access can use `get_mut`. Keep ordinary-field access in the matrix and compare actual generated code and traversal CPU |
| Typed pages should not lose because they exceed the provisional 880 MB category | Accepted; already corrected in section 6. Whole-owner storage and generator/accessor complexity decide the tradeoff. Suggested symbol/flow widths remain conditional headroom until all replacement allocations are priced |
| Chunk carving belongs in the allocation-policy matrix | Accepted as a bounded experiment. Safe encoded backing or separate typed arenas are candidates; arbitrary heterogeneous typed references cannot simply be carved from raw bytes under the current safe-Rust contract |
| The modeled page calls are 15–20 times today's allocation count and add 0.1–0.2 s | Unsupported comparison and timing forecast. The proposed counts include stores/directories while the denominator omits 3,149,779 existing payload-box calls. Current allocator samples are not marginal per-call latency. Compare the complete replaced regions and measure the implementation |
| 8.25 source bytes/node gives an initial per-file chunk size | It is a global ratio, not a bound. The file-weighted 5th/95th percentiles are 5.895/31.469 bytes/node; that predictor oversizes node count by more than twofold in 1,635 files. Add minimum/cap/growth policies and charge actual slack |
| List edges belong in the vertical slice | Accepted, including nested parameter/argument construction. A shared append-only edge arena needs a strategy for nested-list interleaving and observable slice/backing identity |
| Vec-to-box lists are the largest identifiable parse-request source | Not established. The combined compaction/auxiliary scope requests 178.788 MB and excludes initial Vec construction; core-node allocation requests 1,692.598 MB. Neither the unclassified parse remainder nor the 464 MB freed/superseded total is measured list traffic |
| Narrow binding setters should preserve the parse proof | Accepted, with a separate binding-graph proof. Symbol/table/flow IDs and `next_container` still need correct owner/bounds validation; storage relocation does not delete that obligation |
| Pilot exits should be CPU-only because bytes are already known | Rejected. CPU and accessor complexity decide among viable layouts, but actual allocation, calls, initialization, live/RSS, parity and ownership remain required. The byte model is incomplete |
| Runtime IDs have no parse/bind consumers after removing exclusive node copies | False: module-instance-state caching and ambient-module naming still request runtime node IDs. Measure occupancy rather than pricing it as zero |
| No TypeScript JSDoc materialization means foreign parents are negligible | Deferred TypeScript JSDoc supports the current endpoint fast path, not removal of later materialization or imported/foreign-owner behavior. Endpoint occupancy and lifetime API obligations are different questions |
| The 257 MB residual is mostly disappearing map controls and Vec/Arc headers | Unproved. The residual is after Arc-header estimates; inline fields, known backing capacity and page directories are already counted. Reconcile remaining bucket/control and other storage, then remeasure the redesigned owner; neither write it off nor impose it as a permanent fixed cost |

The identifier contract also includes observations before a parser range exists:
[factory hooks](../crates/tsr_ast/src/factory.rs) can see new nodes before
[parser finishing](../crates/tsr_parser/src/state.rs) assigns the final range.
[Range mutation](../crates/tsr_ast/src/lib.rs) is independent of
[observable text](../crates/tsr_ast/src/node_text.rs). Unicode escapes, JSX names,
arbitrary factory identifiers, reparsed clones, foreign owners, synthetic
positions and subsequent range edits therefore remain explicit fallback tests.

The runtime-ID counterexample is a live path through
[module binding](../crates/tsr_binder/src/modules.rs) into
[`module_instance_state_cached`](../crates/tsr_ast/src/binder_helpers.rs), with
another consumer in [ambient-module declarations](../crates/tsr_binder/src/declarations.rs).
This establishes nonzero-capable consumers, not their workload frequency.
The [binding-result validator](../crates/tsr_ast/src/bind_result.rs) separately
checks symbol/table/flow references, next-container links and result graphs;
narrow inline setters must preserve those obligations while avoiding another
whole syntax scan.

Decision: add mixed rows, bounded chunks and actual list construction to the
pilot, and require narrow binding setters from its first implementation. Retain
ordinary typed pages and the whole-owner comparison. Model/census gaps are
explicit work items, not assumed savings. The additive diagnostic passes seven
Python tests and two compiled Rust tests; the source-density report replays
archived metadata with workload/hash checks. These additions change no compiler
behavior, gate thresholds, historical capture or promoted timing result.

## 8. Implemented storage pilot and first measured list result

The [implementation record](S07-bis-storage-pilot.md) now distinguishes observed
occupancy, compiled layout projections and measured isolated construction.
The physical census finds 2,207 assigned node runtime IDs, 117 symbol IDs and no
source-suffix exceptions among 6,795,224 identifier/private-identifier rows. It
preserves obsolete physical backings and confirms no lazy nodes were materialized
at this endpoint. Those observations do not remove later factory, mutation,
foreign-owner or lazy API obligations.

The four-byte text implementation and list construction API pass their isolated
debug/release/MSRV/Miri contracts. Independent arithmetic reproduces all five
new text-page projections against the old controls. A root review of staged
census accessors and the collector found no substantive observation defect;
the documented metadata recovery retains the unchanged successful child's bytes.
The compiled owner model keeps canonical-name interning, packed-flow escapes,
owner dispatch and the unexplained native residual as explicit obligations.

The first fixed list replay exposes a tradeoff: 256-word pages reduce
construction requests by 64.7% and construction time by 33.9%, while eight-sweep
traversal is 35.6% slower (+33.493 ms total). Host load also limits timing
confidence. The initial response tested contiguous typed chunks and checked
slice reads, preserving the capture and charging descriptors and tails. Section
10 corrects the excessive weight given to the isolated traversal ratio. Broad
generated-storage migration still requires the representative node-access pilot
and a complete owner/traffic budget; no S07 gate changes or new passing metrics
are introduced by these diagnostics.

The contiguous-chunk follow-up was implemented and independently reviewed, with
12 additional debug/release/MSRV/Miri contract tests. Its fixed capture includes
fresh legacy and page controls. It does not recover traversal speed and adds
requests/retained bytes relative to pages, so those chunk policies are rejected.
The next experiment isolates resolution/validation from reads through an already
resolved owner borrow. Safe bounds checks and checked foreign/stale entry paths
remain required. The whole-owner model and the actual binder access contract,
not this microprobe alone, will determine production feasibility.

## 9. CP1 access implementation and promotion review

The [CP1 record](S07-bis-CP1.md) fixes separate access and full-pipeline
experiments, with promotion criteria committed before measurement. Independent
reviews of the isolated Rust access paths, their capture/archive protocols and
the production lookup candidate found no unresolved substantive issue. Safe
short borrows, checked physical ranges, foreign/lazy/published fallback and exact
cached-metadata allocation charges were verified. Both new production cases
belong to the explicit 29-case S07 E3 inventory and passed in all four modes.

The review corrected the performance attribution: A0-b already avoids overlay
hashing on exclusive reads. Frozen normal arm64 disassembly shows that the CP1
shortcut removes remaining view routing while keeping core owner/slot checks.
It does not remove the whole previously measured lookup union. Full raw replay
supports keeping the candidate: wall median reductions of 7.1% / 5.6%, both
upper confidence bounds below one, and all declared noise/non-regression limits
met. The broad one-worker confidence interval and effectively unchanged memory
are stated explicitly. All warmups and samples remain present.

Archive review added exact graph-stream inventory checks and independent hashes
for the binder report/request ledger. Initial offline replay also caught omitted
subprocess-only helper snapshots; the archive closure now includes every helper
fingerprinted by capture. These packaging corrections preserve the original
native observations and reject missing streams, altered request identities or
missing source snapshots. Superseded preparation attempts remain local diagnostics.

All frozen Go graphs pass using the existing normalization, including 49
eight-worker raw differences. The broader binder producer, expanded E3 run and
workspace library tests pass. An initial binder attempt failed on sandbox access
to the existing Go cache; its original error and successful retry are preserved.

The third list trial separately finds a 23.2% traversal reduction from direct
owner descriptors, with cached slices faster again at a 50.154 MB metadata cost.
This supports a real disjoint list-reader/node-writer pilot, not an all-list
cache or a production storage selection. The next representative row comparison
must preserve sequentially consistent facts, actual binary-expression fields,
directory costs and short payload borrows across recursive binding. The general
facade, whole-owner budget and final S07 acceptance remain unfinished.

## 10. Fable's list-policy and absolute-cost review

The [detailed disposition](S07-bis-list-tradeoff-review.md) accepts the central
criticism: an isolated traversal percentage was insufficient reason to hold back
page integration. The first capture saves 16.874 ms construction, 200.942 MB of
requests and 46.784 MB retained, at an extra 4.187 ms per synthetic sweep on
average. Restore page-256 as the leading list candidate and page-64 as a bounded
memory challenger; the tested contiguous chunks remain rejected as replacements.

The caller audit qualifies the proposed pipeline conversion. Actual binding
re-resolves slices per element and scans statement lists twice; other parser,
factory and validation passes also read lists. The replay includes obsolete
physical backings and omits production record routing. Two/three equivalent
sweeps are useful sensitivity rows, not established production frequency, and
the isolated delta is not a rigorous upper bound. Requested-live bytes remain
distinct from RSS and are not subtracted from production allocation as a measured
saving.

Checked `NodeSliceRead` supplies a scoped proof, but recursive mutable binding
cannot currently keep that whole-owner borrow. A disjoint list reader carries
the proof across narrow node writes. The page prototype maintains valid backing
ranges during private construction; publication itself only checks completion.
Hoist logical range/identity resolution into that reader without adding another
publication scan or dropping safe physical bounds and raw/imported checks.

The plan and Rust guide now require absolute milliseconds/MB and explicit
denominators before ratios. Whole-pipeline screening thresholds and final S07
gates are unchanged. Pages enter the next integrated node/list slice without an
extra requirement to match the raw boxed-slice microbenchmark. No native
measurement or historical capture was rerun or rewritten for this amendment.
