# Architecture Decision Records

One decision per file, numbered. `Status` is `Proposed`, `Accepted`, `Amended` or `Superseded`; the status tool reads the `Status:` line. A change that alters a decision amends the record or supersedes it with a new one. See `docs/TRACKING.md`.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-independent-repository-with-upstream-pinned-as-a-submodule.md) | Independent repository with upstream pinned as a submodule | Accepted |
| [0002](0002-native-targets-and-the-glibc-floor.md) | Native targets and the glibc floor | Accepted |
| [0003](0003-memory--webassembly-and-embedding-are-all-required.md) | Memory, WebAssembly and embedding are all required | Accepted |
| [0004](0004-the-owner-approves-baseline-divergences.md) | The owner approves baseline divergences | Accepted |
| [0005](0005-sequencing-by-dependency-slices-and-parity-gates--with-no-ca.md) | Sequencing by dependency slices and parity gates, with no calendar or staffing model | Accepted |
| [0006](0006-node-ownership--arenas--lazy-file-storage--bundles-and-check.md) | Node ownership: arenas, lazy file storage, bundles and checked identities | Accepted |
| [0007](0007-symbol-ownership--file-owned-binding--checker-local-merges.md) | Symbol ownership: file-owned binding, checker-local merges, checker-owned types | Accepted |
| [0008](0008-checker-mutation-model.md) | Checker mutation model | Accepted |
| [0009](0009-concurrency-model-kept-from-corsa.md) | Concurrency model kept from Corsa | Accepted |
| [0010](0010-order-sensitive-outputs-are-enumerated--comparators-are-port.md) | Order-sensitive outputs are enumerated; comparators are ported exactly | Accepted |
| [0011](0011-deep-recursion--reserved-stacks-and-growth-guards.md) | Deep recursion: reserved stacks and growth guards | Accepted |
| [0012](0012-panics--recovery-and-generation-retirement.md) | Panics, recovery and generation retirement | Accepted |
| [0013](0013-source-bytes--javascript-strings-and-wire-positions.md) | Source bytes, JavaScript strings and wire positions | Accepted |
| [0014](0014-host-seams-as-traits.md) | Host seams as traits | Accepted |
| [0015](0015-generated-code-from-the-pinned-schemas--with-upstream-s-extr.md) | Generated code from the pinned schemas, with upstream's extractors authoritative | Accepted |
| [0016](0016-toolchain-and-lints.md) | Toolchain and lints | Accepted |
| [0017](0017-dependency-policy.md) | Dependency policy | Accepted |
| [0018](0018-tracking--evidence-driven-ledger--function-traceability--adr.md) | Tracking: evidence-driven ledger, function traceability, ADRs and a dashboard | Accepted |
| [0019](0019-test-host-protocol-and-transport-contract-tests.md) | Test-host protocol and transport contract tests | Accepted |
| [0020](0020-phase-0-gate-decision.md) | Phase 0 gate decision | Accepted |
| [0021](0021-parse-and-bind-performance-thresholds.md) | Parse-and-bind performance thresholds re-based on measured evidence | Accepted |
| [0022](0022-checker-type-footprint-threshold.md) | Checker per-type footprint threshold | Accepted |

The three contracts (0006, 0007, 0013) were accepted by the owner on 2026-09-05 after review of their design notes: [ownership](../design/ownership.md), [symbols](../design/symbols.md) and [text](../design/text.md). Each note cites the pinned upstream code it reproduces and lists what E3 or E4 asserts. ADR 0019 was accepted by the owner on 2026-09-19 with server-owned options, a raw plugin stream tunnel and explicit Phase 5 limitations. ADR 0020 was accepted on 2026-09-20: the owner approved the amended prototype limits, Phase 7 budgets and S12 closure using the indexed recent evidence. Normal current-source freshness checks remain unchanged.

## Proposed, not yet written

Pending technical decisions from the plan's section 13 become records when the owner decides them:

- Storage layout for nodes and types (item 6): an experiment, chosen on E5, E6 and checker-slice measurements.
- Interning (item 8): deferred; decided on measured string duplication.
- Trampolines at the four left-operand recursion sites (item 11): decided by the stress tests.
- Replacement for the `checkchildren` analyzer (item 14): a `rustc_driver`-based lint or dylint, chosen in the spike.
- The spike subset rule and frozen manifest (item 16).
- Whether the fourslash test-host transport is a patch on the pinned harness or a fork (open question 5).
