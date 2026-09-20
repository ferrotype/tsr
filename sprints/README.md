# Phase 0 implementation plan

Phase 0 is the spike in [PLAN.md](../PLAN.md), section 9: the three contracts, the AST, scanner, parser and encoder, the dependency slices, and the eight experiments E1 to E8 with explicit thresholds. This directory holds it as sprint files that `cargo xtask check <id>` evaluates against current evidence under the [tracking contract](../docs/TRACKING.md). A sprint is done only when every exit check and every required item passes; a check that names a metric no producer emits yet cannot pass, so an unimplemented sprint stays open by construction.

## Sequence

Phases are ordered by dependency, not by time, and so are sprints. Each sprint's `exit` names its direct prerequisites. Sprints can run in parallel once their prerequisites are complete.

| Sprint | Title | Needs | Gate inputs it settles |
|---|---|---|---|
| S01 | Contracts, workspace, oracle | | done: ADRs 0006, 0007, 0013 accepted; oracle builds |
| S02 | Toolchain, lints, dependency policy, CI | S01 | fmt, clippy, deny, selftest producers |
| S03 | Generators from the pinned schemas | S02 | `gen` producer: no drift, client byte-identical |
| S04 | Contract leaves: `tsr_jsstring` and `tsr_arena` | S02 | E4 decoding, helpers, slices, positions; E3 ids, lazy storage, bundles, counters |
| S05 | Scanner | S03, S04 | `scanner` producer; E4 scanner diagnostics, token bytes and literal values |
| S06 | Parser, JSDoc, AST runtime and encoder | S03, S05 | E1; E4 encoder success/error and output bytes |
| S07 | Binder, resolution slice, program host, parse-and-bind benchmark | S06 | `binder`, `program` producers; frozen subset; E5 parse/bind; E6; E3 programs and snapshots |
| S08 | Checker slice, printer and node builder | S07 | E2 including checker costs and alternative relater comparison; E4 complete; E5 per-type footprint; E3 checker merges |
| S09 | Ownership and registry harness | S08 | E3 complete, including Miri and AddressSanitizer |
| S10 | WebAssembly and Rust embedding | S08 | E7, E8 |
| S11 | Test-host transport prototype | S06 | none; not a gate input |
| S12 | Phase 0 gate | S02 to S10 | E1 to E8 pass; ADR 0020 records the decision |

S03 and S04 run in parallel after S02. S09 and S10 run in parallel after S08. S11 runs in parallel with S08 to S10 and does not gate S12.

E4 is staged by the production paths available in each sprint: leaf case/truncation/escape helpers in S04, scanner token and literal-value bytes in S05, encoder paths in S06, and literal-type construction plus original-source/regenerated printing in S08. The early helper and token-value metrics do not complete the final printer and literal-type criteria. E3's early identity tests use production arena/resolver/lease components with minimal owners; they can construct two checker-owner identities before the semantic checker exists.

## Producers

A producer is registered in `status/runs.toml` only when its harness exists; until then the metrics it will emit are named in the sprint file's header comment and the criteria stay pending. Producers with a `cases` manifest get `parity` and the test counts from the runner; the harness cannot override them.

| Producer | Registered | Cases | Emits |
|---|---|---|---|
| `workspace`, `oracle` | yes | | build, smoke |
| `fmt`, `clippy`, `deny`, `selftest` | yes | | `clean`, `pass` |
| `gen` | yes | | `patches_apply`, `ast_schema`, `drift`, `client_identical` |
| `e3` | yes; extended through S09 | `data/s04/e3-cases.json` | S04 ids, lazy storage, bundles, live counters, Miri and ASan; later E3 metrics as scenarios land |
| `e4` | yes; extended through S08 | `data/s04/e4-cases.json` plus frozen probe inventory | S04 decoding, helpers, slice validity and positions; later E4 metrics as production paths land |
| `scanner` | S05 | frozen scanner case list | `parity`, `regexp_parity`, `rescan_parity` |
| `e1` | S06 | frozen corpus manifest | `parity` (derived), `frozen_denominator` |
| `binder` | S07 | corpus | `parity` |
| `program` | S07 | | `subset_loads` |
| `e5`, `e6` | S07, `type_footprint_ratio` in S08 | | the ratio metrics; workload pinned in `data/workloads.toml` |
| `e2` | S07 (`frozen_subset`), completed in S08 | frozen subset | `types_parity`, `errors_parity`, `comparators`, `frozen_subset`, `divergences_approved`, `type_to_string_parity`, `recursion_fixtures` |
| `checkerbench` | S08 | | `throughput_ratio`, `allocated_bytes_ratio`, `retained_bytes_ratio`: Rust checker slice / pinned Go checker |
| `relater` | S08 | frozen relater fixture manifest | `parity` (derived), `throughput_ratio`, `allocated_bytes_ratio`, `retained_bytes_ratio`: arena-reference/interior-mutability prototype / id-and-`&mut self` implementation |
| `e7`, `e8` | S10 | | the E7 and E8 metrics |
| `testhost` | S11 | transport fixtures | `parity`, `controls` |

Every oracle-side tool is built from the ledger's upstream pin. S03 applies its carried extractor patches in an independent tooling worktree, runs the pinned resolvers and checks exported-contract TypeScript and enums against the untouched client. `cargo xtask gen` writes generated artifacts; `cargo xtask gen --check` checks drift; `cargo xtask run gen` records the producer's observations. Node/npm pins come from upstream's package and its lockfile, and generation installs dependencies only in the disposable checkout. See [S03](../docs/S03.md) for reproduction, generated surfaces and runtime limits.

S04, S05 and S06 export clean copies into temporary directories and add access-only wrappers for private Go helpers; they never write to the canonical submodule. Scanner and parser/encoder parity use these clean native Go exports, independently of S03's patched extractor worktree. Future symbol and WebAssembly tools must declare their own source closure and bridge policy. See [S04](../docs/S04.md), [S05](../docs/S05.md) and [S06](../docs/S06.md) for reproduction and limits.

The S08 measurements use checked-in workload manifests, query sequences, harnesses and benchmark configuration declared as producer inputs. Record the upstream pin, both implementation revisions, target, build flags, allocator, thread count, warm-up and sampling method with the raw numerator and denominator samples in each evidence artifact. Checker costs cover the fixed checking/query phase separately from parse and bind. Retained bytes are sampled after that phase while the specified checker and result roots remain alive; the workload defines the same logical roots for both implementations. Final-drop disposal remains an E3 check.

The relater fixture manifest fixes the supported operations and expected observable results from the pinned Go oracle. A case passes only when both implementations match those expectations; the runner derives parity over the entire manifest. Measure the same operations and root set in both implementations, and exercise each implementation's arena access and mutation model. Report the comparison before selecting the fallback or type layout. The E2 numeric gates require actual finite ratios with positive measured baseline denominators: positive throughput and nonnegative allocated/retained-byte ratios establish usable evidence, with no performance target. Missing measurements or unusable denominators keep the criterion pending.

## Conventions

- **Crates.** Each sprint names the crates it creates from the crate map in PLAN.md, section 8. A crate used before its full-parity phase carries a slice manifest listing supported operations; unsupported operations fail explicitly.
- **Traceability.** Ported Rust items carry `port:` markers (docs/TRACKING.md). Sprints S05 to S07 require a mapping ratio for their packages; the ratio measures traceability, never parity. Parity comes from the behavioral evidence the ledger's `verify` checks name.
- **Ledger.** The 36 Phase 0 files in `PORTS.toml` already carry the `verify` checks they must pass; `status` moves from `planned` to `in-progress` and `ported` as the sprints land, and `verified` is computed.
- **Divergences.** A difference from a Corsa baseline passes E2 only with an owner-approved entry in `data/divergences.toml` (ADR 0004).
- **Decisions.** ADR 0019 (test-host protocol) and ADR 0020 (gate decision) exist as Proposed so their sprints can name them; they are accepted only after the owner's review, like ADRs 0006, 0007 and 0013.
- **Not in Phase 0.** The release matrix, the glibc 2.28 symbol check on shipped binaries, code signing and the full WebAssembly and embedding acceptance are Phase 7 gates (ADRs 0002 and 0003).
