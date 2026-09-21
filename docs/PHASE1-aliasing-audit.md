# F1b ownership and aliasing audit

Decision recorded 2026-09-21 against upstream
`1f70213d4922b434345f639b441681e470c7cfc1`, Go 1.27.1.

## Decision

Restore owned compiler-option fields. Keep the explicitly alias-observing
slice-helper and multimap APIs separate. Remove `SharedValue` and the unused
`SharedSlice::update` / `update_each` callback APIs. Do not turn ordinary compiler
containers into shared slices to reuse a helper.

The owner accepted the options correction and its narrow divergence after the
PR #46 review, then requested this audit. That approval does **not** waive the
other prepared aliasing cases. Their behavior remains unchanged. The audit is
complete for the current users; it does not claim that every future Go caller
needs shared mutable storage.

## Compiler options: approved divergence

The nine fields are `paths`, `checkers`, `max_node_module_js_depth`,
`custom_conditions`, `lib`, `module_suffixes`, `root_dirs`, `type_roots`, and
`types`. Restore ordered owned paths, owned vectors, and optional integers.
`JsString` can retain immutable text backing; option containers cannot share
mutation. Nil versus empty and path insertion order remain unchanged.

Go's reflective `CompilerOptions.Clone` copies pointers and slice headers.
Rust's clone isolates these containers. This differs for writes through a
retained alias, observed by the frozen case
`leaves/options/clone-shares-pointer-backed-fields`. Keep that case, its native
expectation, and its raw `different` result. Approval covers only this ownership
difference; it cannot excuse other values, ordering, nilness, or compiler output.
The no-mutation control and the complete field-roster case must still agree.

The earlier caller review in [the progress record](PHASE1-progress.md#the-one-divergence-left-standing-clone-shares-what-it-copies)
found whole-field replacement after cloning. More decisively,
`tsoptions/tsconfigparsing.go:1807-1849` copies affected target slices and the
paths map before `${configDir}` substitution; unchanged target slices may still
be shared. It is not a blanket recursive deep clone. The shared Rust version
mutated the original options here, unlike Go. Owned containers restore the
original's independence. Regression tests cover all nine fields and substitution
of paths, root directories and type roots, including unchanged/nil/empty entries.

The compiler loader clones configuration options into an `Arc<CompilerOptions>`
and gives the same options to resolution. Interior mutation through an escaped
clone could invalidate assumptions behind already computed results. Restoring
owned fields also removes per-list-element locks/clones and paths read locks;
this is a structural cost reduction, not a measured speedup.

## Remaining users: complete Rust inventory

After the options correction, `SharedSlice` is used by
`tsr_core::helpers`, `tsr_core::collections::MultiMap`, their leaf adapters and
the core regression tests. No production compiler, checker, resolver, parser,
transformer, or other crate calls these shared-slice APIs. `SharedValue` has no
remaining user and is deleted. `Values` is only the multimap's slice alias.
The ordinary scalar/borrowed-slice helpers and unrelated scoped collections do
not depend on this storage and need no change.

| Contract | Pinned behavior and witness | Disposition |
| --- | --- | --- |
| `Same` | `core.go:183` compares length and first-element address; all zero-length slices compare the same. `same-is-length-then-first-element` covers independent allocations, aliases and subslices. | Preserve explicit identity, not value equality. |
| `Filter`, `SameMap` | Return the original header on no change; changed results allocate. `filter-identity-and-empty-clone` and `same-map-identity-and-call-count` check identity, mutation, nil/empty and callback counts. | Preserve. |
| `Concatenate`, `Deduplicate` | Empty-operand/no-duplicate shortcuts return an input header. `concatenate-returns-an-operand` and `deduplicate-keeps-the-first-occurrence` distinguish reuse and copy. | Preserve. |
| `AppendIfUnique` | Existing elements return the input; appending can overwrite spare backing visible through another header. `append-if-unique-writes-into-spare-capacity` observes that write. | Preserve. |
| `Map`, `MapIndex` | Allocate a separate result, preserve nil input, and read each source element when visited. `map-preserves-nil` checks nilness and independent identity. The Rust callback regression additionally changes a later source element through an alias. | Preserve; these do not intrinsically require sharing for ordinary immutable callers. |
| `MapFiltered`, `FlatMap`, `Flatten` | Build a result through append, with nil accumulators. `map-filtered-nil-accumulator` and `flatmap-and-flatten-nil-accumulator` cover value/nilness/identity. | Preserve current compatibility APIs. |
| `MultiMap` | `Get` returns a header. `Remove` shifts the backing without clearing the old tail; an earlier header keeps its old length and observes the shifted values. `multimap-remove-aliases-the-slice` is a native mutation witness, not just a pointer comparison. | Preserve `retain_values`; ordinary `get` remains a borrow. |

Helper witness names above have prefix `leaves/helpers/`; multimap names have
prefix `leaves/core-collections/`. The other multimap cases are
`multimap-empty-and-size-hint` and `multimap-group-by-remove-all-clear`.
The three additional `helpers.Slices` cases (`find-zero-value-is-not-absence`,
`first-and-last-or-nil`, `some-stops-at-the-first-true`) use the same adapter
registers but call borrowed-slice operations; sharing is not needed by their
production APIs. This accounts for all 14 slice-register cases and three
multimap cases.

Identity has real Go callers, not only harness witnesses:
`checker/relater.go:2920-2931` calls `SameMap` then `Same` to decide whether to
construct new intersection constraints; `checker/inference.go:1537-1554` uses
the same pattern before adding nullable types back. A Rust caller may preserve
those decisions with an explicit changed flag or borrowed/owned result instead
of a shared allocation. That does not authorize changing the separately tested
generic operation into an always-copying function.

For multimap, the inspected Go uses include checker display-name grouping
(`nodebuilderimpl.go:385-439`), language-service expando grouping, auto-import
indexes, edit tracking, decorator grouping, external-module export tables and
fourslash grouping. The display/decorator examples build and then read groups;
they do **not** justify introducing locks into their Rust counterparts. This
audit does not claim a production caller requires the retained-tail behavior.
It preserves the explicit prepared `Get`/`Remove` contract without broadening
the approved options exception.

## Allocation growth: retain only as a compatibility detail

Backing identity and Go allocator size-class rounding are different issues.
`SameMap`/`Same` does not require modeling allocator growth. However, the two
native cases `append-sharing-across-go-size-class` and
`flatten-batch-retained-capacity` append **after** saving a header, then mutate
an existing element and read the saved header. Changing the detach boundary
changes their values, not just capacity metadata.

Keep the current pinned growth model behind `SharedSlice` for these explicit
compatibility contracts. It is no longer reachable from compiler-option
storage or module resolution. Neither a generic `Vec` clone nor an arbitrary
doubling policy preserves all the current observations. Moving the model into
the adapter would merely make the test implement the missing production
behavior, so that is rejected as well.

The native growth witnesses use Go strings on the pinned native toolchain.
They do not certify every element layout, capacity or target architecture. No
compiler performance or wasm-layout claim follows from them. Any future
production adoption must justify its caller's alias contract and costs; use
ordinary Rust containers otherwise. Removing this model later would require a
separate decision for the two named observations, not the options waiver.

Read guards must be released before mutating an alias. User callbacks in the
mapped/filtered operations run after releasing the read guard. The unused
write-lock callback APIs are removed rather than leaving a reentry trap for
future callers. The remaining public mutation operations do not accept a
caller-supplied closure.

## Evidence

The previous 230/230 capture is historical. The corrected tree must report
229 exact leaf matches and one approved ownership difference, with no missing
or failed leaf cases. The comparator continues to report that difference and
`--require-parity` continues to reject it; this audit does not redefine exact
parity. The new capture and validation results are recorded in the progress
record. Native expectations are not rewritten.
