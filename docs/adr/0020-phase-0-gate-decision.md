# ADR 0020: Phase 0 gate decision

Status: Proposed (2026-09-19)
Plan: section 9, Phase 0 gate; section 14, step 8
Sprint: S12

## Context

The spike is the initial go/no-go point. E1 to E4 must pass; E5 to E8 must meet
their thresholds for the parts they measure. Full checking and emit acceptance,
the WebAssembly and embedding release requirements, and the four-target release
matrix remain Phase 7 gates (ADRs 0002 and 0003).

The S11 merge and its closure records are on main at
`8729daff7b914867fe70715e3ef91dce91c53c9c`. The committed CI snapshot certifies
S01 through S06 and S11. It does not certify the remaining S12 prerequisites:
E2's CI producer records the frozen denominator without running the full checker
capture, and the saved performance captures predate relevant source changes.

Three historical S10 measurements also miss their numerical limits. Therefore
accepting a go decision requires an explicit performance-policy decision and
fresh qualifying evidence, not only changing this ADR's status.

## Decision

**Proposed disposition: go after the conditions below are met.** This is not an
accepted decision or a claim that S12 currently passes.

1. Preserve every correctness, ownership, denominator and approved-divergence
   requirement. E1/E2/E3/E4 and the E7/E8 correctness/lifetime criteria remain
   mandatory. Preserve source, input, toolchain and execution-identity checks.
2. Obtain the owner's decision on the three prototype performance limits. The
   proposal is E7 parser size at most **30%** of Go, E7 parser throughput at least
   **1.5 times** Go, and E8 Node parse latency at most **40%** of the socket path.
   The existing limits are 25%, 2 times and 10%, respectively. The proposal
   recognizes measured prototype benefit; it is not a measured speedup or an
   implicit relaxation of the Phase 7 native goals. Until accepted, the ledger
   and PLAN retain their existing limits.
3. Freeze any approved policy changes before capturing evidence. Revalidate or
   refresh the complete dependency chain on the resulting source revision.
   Do not substitute a CI build for a quiet-host timing capture, combine ratios
   from different batches, or mark an old capture current after rewriting its
   metadata. Retain unfavorable and historical results.
4. Review and approve the [full acceptance matrix and proposed Phase 7 budgets](../S12-acceptance.md).
   Those budgets apply to complete checking and emit workloads, independently
   of the parser-only prototype limits.
5. Accept this ADR only when the owner chooses go and every non-ADR S12 check
   passes. Record the final artifact identities and current measured values in
   this decision. A numerical miss remains a miss unless separately amended by
   the owner; unusable or stale evidence remains unavailable.

The alternative is to retain the original E7/E8 limits and continue performance
implementation before the final capture. No optimization is presumed to close
those gaps. The measured Node dispatch/return cost is about 1.3% of its parse
interval, so removing the worker boundary alone is not a credible route to the
original Node target; see [the S10 attribution](../S10-results.md).

## Evidence available for the decision

Values below distinguish the current CI snapshot from historical measurements.
Historical passes describe their recorded revision only and do not satisfy
today's gates. The [closure sequence](../S12-closure-plan.md) lists the refresh
work and the conditions for reusing an existing capture.

| Experiment | Recorded result | Scope and current qualification |
| --- | --- | --- |
| E1 parser | 1.0 parity | Current CI evidence on the frozen parser corpus and libraries |
| E2 checker correctness | 1.0 types, errors and public display; comparator and recursion checks pass | Historical full acceptance capture at `b5aca216`; 9,369 acceptance variants, separate informational rows |
| E2 checker measurement | Rust/Go elapsed 2.125, allocated bytes 0.571, retained bytes 1.413 | Historical fixed checker-query workload; no speed target, but the slower checker and higher retained memory remain material risks |
| E2 alternative relater | 105/105 groups; reference/ID elapsed 1.184, allocations 1.611, retained bytes 2.437 | Historical equivalent-work comparison; no evidence to replace the production ID design with this prototype |
| E3 ownership | Every criterion passes | Current CI ownership suites, including Miri and ASan; real server integration remains later work |
| E4 strings | Every criterion passes | Current CI evidence, including checker/printer paths; transport Unicode limits remain explicit |
| E5 memory | Parse/bind RSS 0.700, allocations 0.700; per-type footprint 0.812 | Historical, all below their 0.85 limits; the footprint metric is not whole-checker retained memory |
| E6 CPU | One worker 1.153, eight workers 1.399; stable | Historical, below the 1.25/1.45 limits; parse/bind only |
| E7 WebAssembly | Size 0.268, throughput 1.677; checker parity 1.0, portable host true | Historical; both performance criteria miss the existing limits |
| E8 embedding | Node latency 0.342; consumer parity 1.0 and lifetime checks pass | Historical; Node latency misses the existing 0.10 limit |

## Extrapolations and workload limits

- E5/E6 use the pinned VS Code source inventory: 13,094 files and 161,740,237
  source bytes, at one and eight workers. They establish parse/bind behavior,
  not full program checking/emit or performance on the other release workloads.
- Checkerbench runs the frozen 9,369-variant query schedule serially, with fresh
  checker state and declared live roots per variant. It excludes loading,
  parsing and binding. Its per-type footprint, allocation and retained-byte
  metrics have different denominators; the favorable footprint cannot cancel
  the unfavorable checker retained-byte result.
- The relater comparison covers 21 fixtures in five modes. It supports retaining
  the current production design for this slice, not a claim that every future
  relation workload favors IDs.
- E7 parser size excludes the checker and corpus adapter. Throughput compares
  parsing in a pinned Node host, not native Rust, and says nothing about full
  checking/emit throughput. Checker portability currently uses Node with a
  4 MiB engine stack; browser-default deep-input execution is not certified.
- E8's 10 KiB parse-and-encode comparison includes different service boundaries:
  the socket path also performs snapshot/loading/binding and JSON/base64 work.
  It is a caller-visible API comparison, not a pure parser ratio or a complete
  embedding workload. Both paths must return identical bytes on fresh inputs.
- Full generics/JSX/decorator coverage, complete emit, incremental programs,
  watch/build services and semantic LSP/fourslash behavior are not established
  by the frozen Phase 0 checker subset or transport prototype.

## Consequences

Until accepted and backed by current passing gates, S12 stays open and no
Phase 1 sprint may name it as done. A no-go decision stops advancement; the
implementation and evidence remain available. Approval of prototype limits
alone does not complete S12 or approve the separate Phase 7 budget proposal.

Advancing would authorize the next dependency phase, not cut-over. The full
checker speed/retention risk, browser stack constraints, reduced active native
CI matrix and remaining semantic/emit coverage must remain visible in the
release acceptance work. No prototype-only exception becomes a full-baseline
exclusion automatically.

## Evidence

Current correctness evidence: the immutable records and recorded execution
context behind the [S11 closure](../S11-closure.md), validated from CI run
`35446072791`.

Historical full E2:
[`b5b93a54…`](../../status/evidence/b5b93a54faa5b610971750c4a6278925f04e94b3f58c5c1a971925f43d5a04f1.json).
Historical checkerbench:
[`5c877c88…`](../../status/evidence/5c877c889f3630fa74103cc142b2b07234e3bb68d28fbc875789b74e9d507c97.json).
Historical relater:
[`981fff12…`](../../status/evidence/981fff12bacd54eab8b02804bdb78919731df5cccb1ad6a4c0890017a369c642.json).
Historical E5/E6:
[`0bf06ff8…`](../../status/evidence/0bf06ff8e9e2bcc77da389d9bc86c167c2ffe777ad666f3f6258f3a519963f92.json),
[`66ab8b7c…`](../../status/evidence/66ab8b7c8e5067d93ee54f115b5f9f1aacc9c66645f50992a47ac5831c39e7eb.json).
Historical E7/E8:
[`17a5f52a…`](../../status/evidence/17a5f52a9809d5a6c148572c189148b42273c84e53005eccd24ab700dad8604b.json),
[`55627695…`](../../status/evidence/556276951ed1b6a50a1a7a17946129a76c78ce6659f311d05e5050b1a54e9d67.json).

The final accepted revision must replace the pending qualification above with
the complete current artifact set. Historical values are not that set.
