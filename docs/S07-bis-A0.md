# S07-bis A0: exclusive binding pilot

The selected first candidate consumes `ParsedFile`, runs the existing binder on
its reserved worker stack, and publishes a `CompletedFile`. This is an experiment;
the S07 memory and CPU gates remain unchanged and unclaimed.

## Candidate scope

Ordinary single-source parser results bind node headers directly in the existing
core pages. Eligibility is checked before mutation: imported owners, several
logical sources, content mappers, factory hooks and already allocated lazy records
use the existing publication/overlay path. Eager JSDoc caches referring to core
nodes are eligible. The pilot does not add mutable access to committed lazy pages.

`BindBuilder` selects an exclusive or published storage backend. Both run one
binder algorithm. Exclusive node access preserves arena and slot checks; it skips
the overlay map and logical-source parent walk. Symbol, locals and flow fields
still use the existing field maps. Those allocations have not disappeared.

`CompletedFile` and its retained node/symbol handles expose the bound view only.
They do not offer `BoundFile::parsed_file()`, whose existing published-path contract
continues to preserve parsed headers. No completed cache entry is installed until
binding and publication finish. The compiler's weak reuse cache is still exclusively
borrowed; the pilot does not invent a concurrent pending cell or negative cache.
An exclusive initializer error or panic drops its private owner. Published-file
contention, reentry, terminal failure and independent mapped siblings retain their
existing binding-cell behavior.

Recursive binder traversal can no longer hold an immutable syntax borrow across
a direct node mutation. It snapshots immediate child descriptors on the stack
(the schema maximum is nine) and reborrows indexed list elements. It copies no
whole node, string or list backing. The descriptor stack and repeated list access
are replacement costs included in the candidate, not assumed free.

The public mutation surface still permits arbitrary node mutation. Consequently,
requesting the mutable parsed builder invalidates its parse validation proof and
publication validates the entire core again. This additional scan is deliberately
included in A0. A later narrow mutation API or validation consolidation needs its
own correctness evidence and measurements.

## Validation and measurement

The candidate must retain both routes' behavior, preserve full frozen graphs at
one and eight workers, and record direct/fallback counts from the same immutable
normal executable. The `tsr_bench --binding-paths` diagnostic runs after the measured
endpoint and uses a separate protocol; normal benchmark output is unchanged.

The experiment runner freezes actual binaries, source/configuration inventories
and all 13,094 inputs. It records graph capture separately from fixed paired
normal/allocation measurements. The accepted original control is never rebuilt
in place. CP0 layout projections do not substitute for measured A0 allocations.

The first review caught fragment-root handling before `BindResult` construction:
a completed non-source fragment must return `InvalidGraph`, matching the published
binding entry point, rather than panic while looking up source metadata. That
validation now precedes candidate selection.

The separate results below distinguish the initial prototype and its revised
candidate. No performance result is implied by compilation or unit tests.

## Predeclared follow-up: A0-b narrow flag mutation

The initial A0 binary remains frozen at commit `58db8c2`. A0-b adds a narrow
`BindBuilder::set_node_flags` API: only this edge-free write preserves the parsed
validation proof. Public unrestricted `node_mut` still calls `builder_mut` and
invalidates the proof before yielding access. Preexisting dirtiness is never reset.
Final binding-result validation remains mandatory. Installing its validated result
in the source's `OnceLock` does not require unrestricted source metadata mutation.

Hypothesis: removing the redundant post-bind core scan reduces complete-pipeline
wall time while preserving the same bound graphs and all owner checks. This
variant is declared before seeing the initial A0 timing result. It will have its
own immutable build, full graph check and fixed screen. The original A0 outcomes
will remain available, including a failure or regression.

A separate phase probe compares published and consuming binding on one source
revision, grouping binding and publication in the same elapsed interval. Its
numbers explain the change; only the normal binaries determine the pipeline
screen. The proof tests cover fresh and previously dirty parses, foreign narrow
writes, invalid parent/payload mutations and invalid binding metadata.

## Initial A0 result

Both worker modes matched all 13,094 frozen Go graphs and used direct binding for
every file. The fixed screen retained eight warmups and all 56 observations;
replay validation passed. Compared with the separately frozen original control:

| Metric | One worker | Eight workers |
| --- | ---: | ---: |
| Wall median, candidate / control | 1.01159 | 1.00612 |
| Timing bootstrap upper 95% ratio | 1.03507 | 1.05067 |
| Requested-allocation median ratio | 0.98183 | 0.98183 |
| Lifetime peak-RSS median ratio | 0.98733 | 0.98741 |

The observed allocation reduction is 85.93 MB and the one-worker RSS reduction
57.85 MB. There is no demonstrated wall-time win, and both timing upper bounds
exceed the 1.02 non-regression guard. **Do not promote this initial candidate.**
The already-declared A0-b revision is the next experiment; these samples will
not be extended or selectively replaced. Field maps remain in both candidates,
so neither claims the large savings from future inline binding fields.

Local capture: `target/s07-bis/a0-screen`; graph capture:
`target/s07-bis/a0-graphs`. Candidate manifest SHA-256:
`ee3399a930061772ca08d912d8bb1f3283169ef467066c7880f1df6d79fdde94`.

## A0-b result and architecture decision

**Keep the exclusive entry with narrow flag writes. CP2 stays on hold.** A0-b
passes the predeclared checkpoint screen and demonstrates a reduction in the
targeted binding/publication interval. The shared compact-layout work will put
binding fields into their applicable payloads. This result does not establish
that layout's feasibility or complete S07's memory/CPU gates.

Both worker modes again matched all 13,094 frozen Go graphs, with 13,094 direct
bindings and zero fallbacks. Eight warmups and all 56 measured observations are
retained; replay validation passed. Normal executables provide wall time and
lifetime peak RSS; separate instrumented executables provide requested bytes.

| Metric | One worker | Eight workers |
| --- | ---: | ---: |
| Original control wall median | 5.183442 s | 1.232323 s |
| A0-b wall median | 4.921846 s | 1.165151 s |
| Wall median, candidate / control | 0.949532 | 0.945492 |
| Timing bootstrap upper 95% ratio | 0.968732 | 0.958537 |
| Requested-allocation median ratio | 0.981827 | 0.981827 |
| Lifetime peak-RSS median ratio | 0.987347 | 0.987407 |

Every mode meets the 1.02 non-regression bounds and both variants' 5% relative-MAD
limits. Both wall medians clear the 0.95 improvement threshold; both timing upper
bounds are below 1.0. The one-worker request reduction is 85.93 MB and RSS
reduction 57.75 MB. A0-b still requests 4.643 GB and reaches 4.507 GB peak RSS;
the roughly 2.035 GB allocation and 2.209 GB RSS limits derived from the old Go
baseline remain distant. No fresh Go-relative acceptance result is claimed.

The separate phase probe compares the two backends on **the same A0-b revision
and executable**, with `layout-profile` enabled for both. It groups publication,
required final validation and binding together, retaining all completed files
through the endpoint. Seven alternating pairs produced:

| One-worker elapsed interval | Published | Consuming | Consuming / published |
| --- | ---: | ---: | ---: |
| Parse median | 2.524859 s | 2.523302 s | 0.999384 |
| Binding and publication median | 2.811072 s | 2.437283 s | 0.867030 |
| Binding and publication range | 2.767792–2.894290 s | 2.407393–2.483136 s | — |

The targeted interval is 13.3% lower while parse is essentially unchanged. These
are diagnostic elapsed timers, not sampled CPU or an independent acceptance
screen. They compare today's two backends, so their reduction must not be added
to the normal-binary result against the original frozen control. Near-one-second
binding remains a later milestone after compact binding-field storage.

## Distance to historical Go budgets

The [arithmetic replay](../tools/s07/performance-experiments/gate-distance/README.md)
compares this retained A0-b screen with the original Go medians. These are
historical distances, not a fresh Go-relative acceptance result. The wall budget
is the old Go median; each memory budget is 0.7 times its old Go median. Ratios
below use the full Go median, so the required ratio is at most 1.0 for wall time
and 0.7 for memory. GB is decimal; peak RSS and allocation traffic remain distinct.

| Domain | Workers | A0-b median | Historical budget | Median / old Go | Excess to budget |
| --- | ---: | ---: | ---: | ---: | ---: |
| Wall time | 1 | 4.921846 s | 2.937627 s | 1.675450 | 1.984219 s |
| Wall time | 8 | 1.165151 s | 0.633963 s | 1.837885 | 0.531188 s |
| Requested allocation | 1 | 4.642569 GB | 2.035226 GB | 1.596775 | 2.607343 GB |
| Requested allocation | 8 | 4.642572 GB | 2.035767 GB | 1.596352 | 2.606806 GB |
| Peak RSS | 1 | 4.506714 GB | 2.208948 GB | 1.428146 | 2.297766 GB |
| Peak RSS | 8 | 4.509221 GB | 2.217045 GB | 1.423721 | 2.292176 GB |

Final CPU acceptance also needs a fresh Rust/Go bootstrap upper 95% ratio bound
at most 1.0 and both runtimes' relative MAD at most 5%, in both worker modes.
The checkpoint's Rust/control bounds above do not establish that condition.
This historical comparison does not combine separate checkpoint improvements
or attribute the remaining gap to a particular compiler phase.

## Correctness and durable evidence

The implementation passed 177 workspace library tests, affected all-target
Clippy checks, the Rust 1.96 minimum-version build, six AST compile-fail
doctests, and both full frozen graph modes. The actual E3 producer passed all
27 S07 ownership cases in debug, release, strict-provenance Miri and ASan.
The new cases exercise both binding routes, fallback selection, private failure
cleanup, retained handles, traversal order and the narrow validation proof.
Owner and allocation counters return to baseline. The broader future E3/S09
scope is not claimed complete by those 27 cases.

The [raw result archives and replay instructions](../tools/s07/performance-experiments/results/2026-09-08/README.md)
preserve the unsuccessful initial A0 and the successful A0-b captures separately.
The [phase probe](../tools/s07/performance-experiments/phases/README.md) documents
its timing boundaries and provenance. Candidate A0-b manifest SHA-256:
`124956f668540814b95e6f677bdeae98bb2cad4f7586d3cb87f869e5c5af118f`;
phase-build manifest SHA-256:
`8b42fd1109b96fbb56ef2411e39ee2b5c15ef93fd10f84948d66ada334c8dda7`.
The E3 evidence object is
`14950fa0ae8904282d890070c360c79380fa38a9da753ef21dfa751231ac95b5`.

Committed status views are regenerated for this implementation. Prior evidence
whose source closure changed is stale until its producer runs again; neither
these diagnostic archives nor the current E3 success substitutes for those
acceptance prerequisites. The branch remains an implementation in progress.
