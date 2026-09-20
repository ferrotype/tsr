# CP1 follow-up: bounded syntax-list snapshots

Date: 2026-09-08. Control: retained CP1 core-read candidate, manifest
`3a57976f667b9857891edbe9f76de5261c43480ac8c62d946b4545d9ee19d931` at
`target/s07-bis/cp1-node-read-candidate`. This is a production CPU experiment;
it changes neither list storage nor binding-field representation.

## Contract and implementation

Fable identifies an alternative to keeping a `NodeSliceRead` alive across
recursive binding: resolve a chunk, copy its IDs to a stack buffer, release the
borrow, and bind those IDs. The previous CP1 record correctly describes the
requirements for a persistent borrow, but omitted this bounded-copy option.

Use at most 16 `Option<NodeId>` entries (128 bytes on the supported targets).
Copy only IDs, not node kind, flags, payloads or binding fields. Evaluate those
fields at the same point as the existing algorithm. Preserve ordinary list
order and both complete functions-first passes; do not interleave the two passes
per chunk. Handle the final partial chunk and nil IDs without introducing a heap
allocation. The average backing length is not an upper bound on list length.

Target `bind_each` and `bind_each_statement_functions_first` in
`crates/tsr_binder/src/containers.rs`. The existing child visitor delegates to
`bind_each`. Keep its upfront empty-slice validation, and preserve the direct
`bind_each` empty-descriptor no-op. Raw, mapped and lazy lookup still uses the
checked `parsed_view().node_slice(...)` route at every chunk boundary.

The production binder does not mutate syntax-list backings; it mutates node and
binding fields. That is what permits copying future IDs in a chunk before
recursing. A generic caller permitted to replace backing entries would need a
different contract. The copied IDs borrow no payload and retain no extra owner;
the existing binder owner continues to keep their graph alive.

The cost hypothesis is fewer complete slice resolutions, from one per element
to one per nonempty chunk, at the cost of stack space, copying and loop control.
For short lists, initialization/copy overhead can erase the saving. Recursive
list calls can have several buffers alive, so depth and actual generated stack
frames matter. Do not infer a pipeline gain from average list length alone.

## Counterexamples and validation

Test lengths around 16 and 32, partial tails, subranges, nil entries, empty and
missing descriptors, wrong owners and lazy reads. Observe callback order and
changes to node fields after the IDs have been copied. Verify functions-first
ordering across chunk boundaries, and compare the complete published/exclusive
binding graphs. Preserve existing fallback and terminal-failure behavior.

Add the ownership-relevant runtime cases to the actual S07 E3 inventory; execute
them in debug/release/Miri/ASan. Run affected MSRV, lint and depth checks. Before
timing, freeze the actual normal/allocation executables and source/configuration,
then compare all 13,094 Go workload graphs at both worker counts. Run the broader
binder producer as a separate semantic check. No new instrumentation or tracing
may run during the normal timing capture.

## Predeclared screen and decision

Use the existing full-pipeline runner with the immutable CP1 control. Keep its
eight warmups and 56 measured children, alternating variant order, with separate
normal and allocation binaries at one and eight workers. Retain all raw samples
and failures. No extended batch, sample filtering or retuning of chunk width
after looking at this candidate's timing is part of this experiment.

Report milliseconds and MB, the per-worker candidate/control ratios, and the
distance to historical Go budgets. The ordinary screen applies: both variants'
relative MAD at most 5%, wall upper 95% ratio at most 1.02 and allocation/RSS
median ratios at most 1.02 in both worker modes. A targeted wall win needs a
median ratio at most 0.95 in at least one mode and its upper bound below one.
This temporary leaf optimization does not itself provide the general borrowed
facade: do not invoke the infrastructure exception to keep an inconclusive
result. Keep only after the correctness, cost and screen reviews; otherwise
remove the production shortcut and retain its recorded experiment.

An accepted candidate becomes the next immutable control. Neither decision
completes the memory gates or proves where the remaining CPU gap resides. The
page-backed integrated slice and [recorded access trace](S07-bis-access-trace.md)
remain separate work, not prerequisites for this bounded copy experiment.

## Depth-validator correction before timing

The first broader binder capture recorded every primary/supplemental graph row
as passing but reported `depth: false`. Its three native depth tests all passed.
The injected-unwind observation stopped after the first actual stack growth at
468 guard entries, with the expected panic and terminal failure. The producer
incorrectly applied a minimum of 501 entries to this deliberately early-aborted
scenario as well as to completed deep traversals.

Keep the 501-entry minimum for completed scenarios. An injected unwind instead
requires positive guard entries, actual segment growth and terminal failure;
native tests also check the exact injected panic and rejection of a retry. Nine
Python depth tests cover the distinct thresholds and reject missing/zero/boolean
growth or failure observations. This corrects the observation contract, not the
candidate's production stack policy. The additional stack buffer can still
change growth frequency and remains a cost to screen.

Preserve the first evidence (`923e6da7…`), native log, request observations and
old producer separately. Replay of its 12 native observations passes the corrected
validator, but does not rewrite its recorded failed depth metric or complete the
skipped depth graph comparisons. Rerun the complete producer before timing.

## Recorded result: reject the standalone shortcut

The criteria were committed as `6fa3557` before measurement. The fixed screen
completed all eight warmups and 56 measured children without replacement or
extension. [The durable archive](../tools/s07/performance-experiments/results/2026-09-08-list-copy/README.md)
retains the candidate, control, complete observations and failed first depth
capture. Offline replay recomputes the screen from its raw observations.

| Workers | CP1 control | List-copy candidate | Difference | Candidate/control | 95% timing ratio interval |
| --- | ---: | ---: | ---: | ---: | --- |
| 1 | 4,344.062 ms | 4,308.221 ms | −35.842 ms | 0.991749 | [0.989408, 0.995231] |
| 8 | 970.698 ms | 968.844 ms | −1.854 ms | 0.998090 | [0.989107, 1.001143] |

There is a small observed one-worker benefit (0.83%). The eight-worker median
is 0.19% lower, but its interval includes no benefit. All declared noise and
non-regression checks pass; neither mode meets the 5% targeted-win requirement.
Requested allocation remains about 4.643 GB and peak RSS about 4.51 GB. Allocation
median differences are −0.000384 MB and +0.001136 MB; RSS differences are −0.081920
MB and −0.016384 MB. These are not meaningful memory savings.

Independent inspection of the frozen normal arm64 binaries confirms resolution,
copying and lazy-guard release before recursive binding, with both complete
functions-first passes and live kind reads. The identified caller-frame subtotals
increase from 176 to 496 bytes in functions-first and from 960 to 1,200 bytes in
the child-list path. Those subtotals exclude descendants, temporary resolution
and stack-growth slow paths; they are not peak-stack or elapsed-time estimates.

**The production shortcut and its candidate-only tests/inventory entries are removed.**
The previously accepted CP1 node-read implementation remains the control. The
small observed benefit does not meet this experiment's promotion contract and
does not justify invoking the infrastructure exception. The separate depth
validator correction remains: it fixes an erroneous assertion independently of
whether the list-copy optimization is retained.

After restoration, every benchmark-source byte matches the CP1 control manifest
(`fcae7873d405b67d59421e523e4efd2379e77a648da5613248516ed374bff84b`),
and all 179 retained workspace library tests pass. Independent full archive
replay succeeds against this restored checkout, including the archived 32-case
inventory while the current inventory has 29. The new producer records describe
the rejected candidate and are stale for the restored checkout; they are not
relabelled as fresh retained-control evidence. Normal CI can refresh live gates.

This rejection concerns copying IDs in today's binder. **Page-256 remains the
leading compact-list integration candidate**, with its separately measured
request/live savings. Neither this CPU-only leaf result nor its stack cost
reverses the earlier absolute-cost decision for storage.

The historical candidate passes all 13,094 Go graph comparisons at both worker
counts, with 13,094 exclusive bindings and zero fallbacks. The eight-worker
comparison normalizes 59 raw differences; it is not byte identity. Its 182
workspace library tests, six AST doctests, affected MSRV/Clippy checks and all 32
S07 ownership cases in debug/release/Miri/ASan pass. The final broader binder
capture passes 22,343 primary requests over 12,829 rows, 18 supplemental requests,
32 protocol tests, 18 helper tests and all 12 depth graph comparisons. The
32-case/182-test totals describe the archived candidate, not the restored control.

The measurement host had 18 CPUs, 64 GiB RAM and initial load averages
6.60 / 7.73 / 8.48. Other coordinated work stopped before timing; unrelated host
activity remains possible. The current control median differs from its earlier
CP1 promotion capture, so do not interpret 4.591→4.308 seconds across those two
batches as the list-copy effect. The measured paired-batch difference is 35.842 ms.

Frozen candidate manifest:
`b03b64ebf3faed4bb2245fa7b76db4289c3a1e510e796e86337c0782fb5f3578`.
Final binder evidence:
`fd6389d0fb7035c747e2991e37b6e595d87dad2c91e16fc2730c5f970b668334`.
E3 evidence:
`31cde17440f7568517c8a5ad81ab473e959848d05bb80cac5774b66e2dc66914`.
All four final S07 performance gates remain open.

## Running distance for the retained implementation

Use the **control** from this latest screen after rejecting the candidate. The
[distance replay](../tools/s07/performance-experiments/gate-distance/README.md)
retains both roles, marks the candidate rejected and verifies all medians against
their seven observations. These comparisons use older Go denominators; they do
not supply fresh cross-runtime confidence bounds or phase attribution.

| Domain | Workers | Retained CP1, this screen | Historical budget | Retained / old Go | Excess to budget |
| --- | ---: | ---: | ---: | ---: | ---: |
| Wall time | 1 | 4.344062 s | 2.937627 s | 1.478766 | 1.406435 s |
| Wall time | 8 | 0.970698 s | 0.633963 s | 1.531159 | 0.336735 s |
| Requested allocation | 1 | 4.642569 GB | 2.035226 GB | 1.596775 | 2.607343 GB |
| Requested allocation | 8 | 4.642571 GB | 2.035767 GB | 1.596352 | 2.606804 GB |
| Peak RSS | 1 | 4.506698 GB | 2.208948 GB | 1.428140 | 2.297750 GB |
| Peak RSS | 8 | 4.509221 GB | 2.217045 GB | 1.423721 | 2.292176 GB |

GB is decimal. The ratio column uses the full old Go median; required final
maxima remain 1.0 for CPU and 0.7 for memory. Newer control medians do not revise
the older A0-b/CP1 capture records or establish a cumulative routing improvement.
