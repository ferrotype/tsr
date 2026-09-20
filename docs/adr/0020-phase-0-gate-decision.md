# ADR 0020: Phase 0 gate decision

Status: Accepted (2026-09-20)
Plan: section 9, Phase 0 gate; section 14, step 8
Sprint: S12

## Context

The spike is the initial go/no-go point. E1 to E4 must pass; E5 to E8 must meet
their thresholds for the parts they measure. Full checking and emit acceptance,
the WebAssembly and embedding release requirements, and the four-target release
matrix remain Phase 7 gates (ADRs 0002 and 0003).

The owner approved this decision on 2026-09-20 after the S11 closure, crate
rename and publishing-preparation work merged. The closure branch starts at
`e4f4dcae32080184813c6410fa49bb3b3e14a174`. The draft was preserved locally as
`d7274341b1d4ce5d10f042299bac768078f26010` and is included in the closure PR.

The owner subsequently explicitly approved closing S12 with the historical
evidence, without repeating correctness or benchmark captures after the crate
rename and publishing work. This replaces the earlier conditional closure
decision. It is acceptance of the recorded results, not a claim that every
intervening change was measured or proved semantically identical.

## Decision

**Accepted disposition: go; S12 closed on historical evidence, 2026-09-20.**
The owner accepts the architecture, Phase 0 implementation milestones, indexed
E1–E8 results, amended prototype limits and full acceptance matrix. S12 is a
dated owner-acceptance milestone, rather than a continuing current-HEAD test.

1. Preserve every correctness, ownership, denominator and approved-divergence
   requirement. E1/E2/E3/E4 and the E7/E8 correctness/lifetime criteria remain
   mandatory in the accepted historical collection. Preserve source, input,
   toolchain and execution identities on every record and leave the normal
   current-source checks unchanged.
2. Set E7 parser size at most **30%** of Go, E7 parser throughput at least
   **1.5 times** Go, and E8 Node parse latency at most **40%** of the socket path.
   These replace 25%, 2 times and 10%, respectively. The amendment
   recognizes measured prototype benefit; it is not a measured speedup or an
   implicit relaxation of the Phase 7 native goals. The ledger is the numerical
   authority for both gate evaluation and measurement confidence/stability.
3. Accept the exact records in the [historical evidence index](../S12-historical-evidence.md)
   for S12 without new captures. Each result retains its original revision,
   timestamp, source/input hashes and host/toolchain context. The collection
   spans several revisions; it is not one final-source measurement batch.
   Do not combine ratios from different batches or rewrite old capture
   metadata to mark it current. Retain unfavorable and historical results.
4. Adopt the [full acceptance matrix and Phase 7 budgets](../S12-acceptance.md).
   Those budgets apply to complete checking and emit workloads, independently
   of the parser-only prototype limits.
5. Record this historical acceptance through ADR 0020 in all three S12 items.
   S02–S10 and the live experiment checks retain their freshness requirements;
   they may report stale while the dated S12 milestone remains complete.
   Future correctness claims, release acceptance and performance claims must
   meet their own evidence requirements. This is a specific owner-approved
   closure decision, not general permission to reuse stale evidence.

The rejected alternative was to retain the original E7/E8 limits and require
more performance implementation before advancing. The owner accepted the
measured prototype benefit instead. The measured Node dispatch/return cost is
about 1.3% of its parse interval, so removing the worker boundary alone was not
a credible route to the original Node target; see [the S10 attribution](../S10-results.md).

## Evidence available for the decision

Values below describe the evidence accepted for this milestone. All 55 E1–E8
criteria and the S01–S10 required-item/exit predicates are satisfied by the
indexed historical collection under the approved limits. This evaluation does
not inject historical metrics into current-source reports. See the
[closure record](../S12-closure-plan.md) and [evidence index](../S12-historical-evidence.md).

| Experiment | Recorded result | Accepted historical scope |
| --- | --- | --- |
| E1 parser | 1.0 parity | S11-era CI evidence on the frozen parser corpus and libraries |
| E2 checker correctness | 1.0 types, errors and public display; comparator and recursion checks pass | Historical full acceptance capture at `b5aca216`; 9,369 acceptance variants, separate informational rows |
| E2 checker measurement | Rust/Go elapsed 2.125, allocated bytes 0.571, retained bytes 1.413 | Historical fixed checker-query workload; no speed target, but the slower checker and higher retained memory remain material risks |
| E2 alternative relater | 105/105 groups; reference/ID elapsed 1.184, allocations 1.611, retained bytes 2.437 | Historical equivalent-work comparison; no evidence to replace the production ID design with this prototype |
| E3 ownership | Every criterion passes | S11-era CI ownership suites, including Miri and ASan; real server integration remains later work |
| E4 strings | Every criterion passes | S11-era CI evidence, including checker/printer paths; transport Unicode limits remain explicit |
| E5 memory | Parse/bind RSS 0.700, allocations 0.700; per-type footprint 0.812 | Historical, all below their 0.85 limits; the footprint metric is not whole-checker retained memory |
| E6 CPU | One worker 1.153, eight workers 1.399; stable | Historical, below the 1.25/1.45 limits; parse/bind only |
| E7 WebAssembly | Size 0.268, throughput 1.677; checker parity 1.0, portable host true | Missed the former 0.25/2.0 limits; passes the amended 0.30/1.5 limits, including the recorded timing confidence bound |
| E8 embedding | Node latency 0.342; consumer parity 1.0 and lifetime checks pass | Missed the former 0.10 limit; passes the amended 0.40 limit, including the recorded timing confidence bound |

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

S12 is complete and Phase 1 may depend on this accepted milestone. The closure
avoids repeating the expensive captures solely to complete the milestone.
Its tradeoff is explicit: the accepted measurements do not certify current
HEAD. The live dashboard can still show stale evidence and incomplete live
experiment/sprint checks; those results are not overridden by S12.

Advancing authorizes the next dependency phase, not cut-over. The full
checker speed/retention risk, browser stack constraints, reduced active native
CI matrix and remaining semantic/emit coverage must remain visible in the
release acceptance work. No prototype-only exception becomes a full-baseline
exclusion automatically.

## Evidence

Prior correctness evidence: the immutable records and recorded execution
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

The [complete evidence index](../S12-historical-evidence.md) also identifies the
supporting quality, generation, scanner, binder, program and workload records.
`cargo xtask check S12` checks this accepted milestone; it does not reclassify
any of those immutable artifacts as fresh.
