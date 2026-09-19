# S08 plan review

Reviewed 10 September 2026 by Codex, against merged `main` `4c0818d` and pinned
Go source `1f70213d4922b434345f639b441681e470c7cfc1`. This is a review of the
[implementation plan](S08-implementation-plan.md), performed by its author.
It is not an independent review or evidence that checker code has been ported.
The independent review that followed is the last section of this file.

## Source and requirement checks

- Read all ten [S08 items](../sprints/S08.toml), their experiment consumers,
  [S09](../sprints/S09.toml), [S10](../sprints/S10.toml), and the current producer
  registrations/CI path. E2 currently verifies only the frozen denominator.
- Read the Rust guide, accepted symbol/ownership contracts, mutation model,
  comparator, recursion, divergence and performance-threshold ADRs.
- Inspected the concrete compiler loader API, checker identity/lease and symbol
  arena primitives, plus the compact binder's local access contract. There is
  no production Rust checker or printer to extend yet.
- Checked all 675 obligation records by family and read their provenance. Their
  111 library identities are actual loader observations; capability records
  explicitly say execution is required on use, not already observed.
- Read the source checker initialization/host interface, type allocation and
  representations, merge ownership contract, resolution-stack implementation,
  relation modes/cache structure, comparator branches, type display and builder
  release behavior. Read the compiler runner and actual type/error baseline
  walkers rather than assuming what `.types` files represent.

The source package inventory contains 60,678 non-test, non-generated Go lines
in 24 checker files and 11,692 in 16 printer files. These figures indicate the
surface to audit, not a mandate to port every function in S08 or a completion
percentage. The source-derived execution closure determines required work.

## Issues addressed in the plan

| Risk or incorrect shortcut | Resolution in the plan |
| --- | --- |
| Treat S08 as only literal/object/union evaluation | Preserve all 10,728 variants, complete loaded libraries and 675 capability obligations. Required generic/conditional/mapped/inference work has explicit checkpoints; unknown operations fail by name. |
| Claim syntax census records prove semantic execution | Keep obligation provenance immutable; join it to actual capabilities and direct supplemental observations. Supplemental tests do not inflate the primary baseline denominator. |
| Assume existing checker lease/symbol arenas form a concrete owner | Name the missing concrete state/retained-result APIs and the identity-reservation mismatch. Require a private one-time adoption path, exact-checker checks, one operation permit and non-owning internal links. |
| Copy the exclusive binder's local namespace directly into a multi-file checker | Separate the validated checker universe from immutable retained file/bundle resolution and lazy AST access. Keep mutation confined to checker-owned state. |
| Choose a uniform type enum and discover its memory cost after porting | Select small common records plus typed payload storage, then measure a real early slice including lists, capacity and transient traffic. No guessed byte size or inherited page constant becomes a target. |
| Let a reference-relater experiment succeed through precomputed graphs or calls into the ID implementation | Require the real algorithm, lazy resolution/allocation where fixtures demand it, Go observations, matched cache state, and early safe-reference feasibility. Its complete measured result is required even if slower. |
| Build generic signatures first and defer their relations to a later independent phase | P3 now grows signatures, inference, instantiation and their required relations together in executable query slices. P4 completes body checking and dependency closure. |
| Add stack protection only when final stress tests are written | Install guards with each recursive production path. P5 completes the adversarial stress inventory and growth/unwind observations. Keep a dispatch-independent entry for S10. |
| Compare just semantic values or normalize away baseline differences | Freeze source walker/query order and distinguish disabled output, `NoContent`, empty content and missing implementation. Compare exact Corsa output separately from old Strada baseline fixups. |
| Implement `TypeToString` as a shortcut formatter | Require real node building and printing, source flags/truncation, diagnostic-triggered serialization, builder generation rotation and emit metadata retention. |
| Certify checker-local merges only because two Rust checkers agree | Compare Go merge/symbol observations, use different additional declarations, and verify that the shared file graph remains unchanged before/after both operations. |
| Reuse S07's relaxed limits for checker storage or invent a checker CPU target | Keep per-type footprint ≤0.80. Checkerbench and relater numerical criteria remain usable-measurement checks. State throughput direction and raw endpoints explicitly. |
| Complete E3/S09 accidentally while adding merge tests | Audit every shared consumer; compose exact inventories and add negative gate tests. Full registry, builder-retention and retirement integration stays pending until its own complete tests run. |
| Replace E5 with a type-only record or silently reuse stale native samples | Compose validated parse/bind and type captures. Schedule a final native refresh if the new Cargo/source closure invalidates S07 evidence; preserve ADR 0021 limits. |
| Repeat expensive profile/trace work before any integrated checker exists | Bound P1 to storage/ownership feasibility. Later diagnostics require a specific unresolved mechanism; measure complete candidates and retain negative outcomes. |

## Implementation-time decisions that remain explicit

1. **Exact source closure and fixture manifests:** P0 must generate and review
   them from the pin. The plan's source inspection is not a completed typed
   closure audit. New required dependencies become work, not exclusions.
2. **Type census accounting:** P0 freezes type/payload/list/cache membership and
   sharing rules before Rust results. P1 records actual capacities and counts.
   Runtime heap endpoints, structural retained storage and requested traffic
   remain distinct. An unaccounted family cannot be assigned zero bytes.
3. **Payload sizes/page policy:** select from the first real Go/Rust census and
   query slice. The plan promises neither a 24-byte type nor a particular page
   count, and does not use type-count inflation to satisfy a per-type mean.
4. **Reference storage feasibility:** P1 must prove safe recursive access and
   relation-time allocation, selecting and reviewing a dependency if required.
   A prototype that cannot execute the frozen contract is unfinished; it is not
   a measured unfavorable result.
5. **Corpus runtime and deterministic CI partitioning:** learn the cost from
   the first full Go capture. Any shards must merge into the exact frozen
   request set and preserve per-program query order. Hosted CI does not supply
   performance acceptance samples.

These are scheduled work with concrete exits, not requests for permission to
skip requirements. No thresholds, divergence approvals, scope changes, source
code or evidence records are changed by this planning task.

## Review conclusion

The plan covers every S08 item and the dependencies identified by source
inspection. It separates actual baseline authority, ownership proof, semantic
parity and measurement. Proceed to P0 before implementation; do not interpret
this review as satisfying P0's generated manifests or later acceptance gates.

## Independent review (10 September 2026)

Reviewed against the pinned source, the frozen S07 inputs and the tracker on
`codex/s08-plan` `295e639`, independently of the author, for the owner. Findings
were folded into the plan in the same change; each item names where.

### Confirmed

The selected approach (ID-based `&mut self` checker, small common record with
typed payload storage, one owner and one operation, baseline authority through the
pinned harness walkers, measurement gates without new speed targets, stop rules)
follows the accepted ADRs and the S07 lessons. The plan's reading of the 10,728
denominator and the 675 obligations is correct: obligations are syntax
observations, and generic machinery is required through the loaded libraries.

### Corrected or added

1. **Declaration diagnostics were missing.** The Go harness appends
   `GetDeclarationDiagnostics` to `.errors.txt` whenever declaration emit is on;
   1,482 eligible variants set `declaration`, 4 `composite`; at least 65 of 7,301
   reference `.errors.txt` baselines carry declaration-emit codes. That path is
   the Phase 3 declarations transformer plus the checker's emit resolver. Added
   to §5.1 and P0 as an owner decision: port it in S08, or record the phase as not
   executed and list the affected baselines as named pending failures. Silent
   matching by absence is rejected.
2. **The denominator's composition was not stated.** §1 now lists the nine
   families with zero eligible variants and the counts of what remains, so P3 and
   P4 scope is legible without opening the rule file.
3. **Homes missing from §3:** `tsr_nodebuilder` (the cycle-breaking package
   upstream keeps for the declarations transformer), `tsr_evaluator`, the
   collections the checker uses (24 `collections.Set` sites plus ordered and
   copy-on-write structures), and the fact that `resolveName` already exists as
   `tsr_binder::name_resolver` with hooks the checker implements.
4. **Go accounting baseline measured, not modeled.** `unsafe.Sizeof` at the pin:
   `Type` header 56 bytes embedded in every payload, `UnionType` 272,
   `InterfaceType` 376, `TupleType` 424, `ast.Symbol` 96 (§6.1 table, fixture in
   `data/s08/`). 24 of the 56 header bytes are Go-only pointers, so the 0.80 risk
   is in lists, maps, caches and slack, not headers.
5. **Effort was unstated.** §8 now says what the exclusions leave (about 50,000
   lines of `checker.go`, `relater.go`, `flow.go`, `inference.go` and
   `grammarchecks.go`) and that S08 is the entry to the Phase 2 critical path,
   reported by pass count per checkpoint rather than time-boxed.
6. **Program host mapped member by member.** `tsr_checker::CheckerHost` carries
   the members whose types exist below the compiler and documents the rest as P2
   obligations, including that `SourceFileMetaData` must move below the checker
   and that project-reference members are explicitly unsupported.
7. **Staleness stated.** The first crate addition stales S07's E5/E6 evidence
   through `Cargo.lock`; §9 says so, so the dashboard change is not chased.

### Scaffold delivered with this review

`crates/tsr_checker`: `CheckerOwner`/`Operation` (identity adoption exactly once,
same-thread reentry refused before waiting, panic retires the generation),
`ResolutionStack` (exact port of the four resolution-guard functions),
`LinkStore` (paged per arena on first use), `TypeStore`/`TypeRecord`/`TypeAlias`,
all `types.go` flag families and enums, `CheckerHost`, and the type-display
constants and flag mask. `crates/tsr_printer`: `EmitTextWriter`, `TextWriter`,
`SingleLineStringWriter`, with Go's strict last-rune decoding.
`crates/tsr_nodebuilder`: flags and `SymbolTracker`. `tsr_arena`:
`CheckerIdentity::adopt_symbol_arena` and the `IdentityAdopted` error.
`data/s08/checker-flag-observations.json`: 238 constants and 23 record sizes read
out of the pinned Go packages through `go test -overlay`, asserted by the Rust
flag ports. The ledger marks the eight source files these start as in progress.
No algorithm, no producer, no evidence claim is included.

### Open for the owner

- Declaration diagnostics: port in S08, or defer with named pending failures.
- Collections home (`tsr_core` or `tsr_collections`): decide at P1 with the first
  ordered-structure use.
- Whether to fold checker state into the `tsr_arena` permit: decide from P1's
  per-operation measurement, not now.

## Codex takeover review

The owner delegated the declaration-diagnostics decision on takeover. Codex chose
to implement the required declaration-transform/emit-resolver path inside S08,
preserving the frozen `.errors.txt` requirement. This is recorded in §5.1 and P0;
it does not approve any baseline divergence or claim general emit parity.

Two factual corrections to the independent review:

- `harnessutil.compileFilesWithHost` compares pre/post diagnostic **counts**. It
  does not detect equal-count changes in payloads. The new oracle must compare
  complete pre/post observations rather than infer equivalence from this check.
- The 23 Go `unsafe.Sizeof` observations are fixed-record measurements. They are
  not the live per-type census, and removing Go back-pointers does not prove that
  Rust's replacement indices, payloads and capacity meet the memory gate.

The scaffold also exposes unbranded `TypeId`/`TypeStore` values and mutable
`CheckerState` through public APIs. A caller can move a same-numbered ID or swap
stores between owners. P1 must close that boundary before algorithm consumers
arrive; constructors being private does not prevent an issued ID from escaping.
The scaffold flag fixture describes an overlay generator that is not committed.
P0 will add the reproducible request inventory and overlay source rather than
accept a prose derivation as a regeneration command.


### Takeover implementation corrections

The phase-policy capture calls pinned Go's `CompilerOptions.GetEmitDeclarations`
for every eligible variant. It identifies **1,459** obligations: 1,450 with
`declaration=true` and nine with `composite=true`. There are 40 explicit
`declaration=false` options and 109 `emitDeclarationOnly=true` options. These
replace the earlier review's 1,482/4/107 counts; checking key presence is not
checking the effective boolean value.

The regenerated flag fixture contains **289 constants** across checker and
node-builder families, including the three display constants, and 23 fixed
record sizes. Both compiled Go overlays reproduce the original fixture.

The takeover closes public local-ID/state escape and detects reentry in the
identity lease itself, including callers that acquire that lease directly.
Compile-fail API tests and direct-lease/operation tests cover both boundaries.
Full P0/P1 completion remains tracked in `S08-P0-P1.md`.
