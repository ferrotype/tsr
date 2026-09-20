<p align="center">
  <img src="assets/tsr-icon.png" alt="tsr logo" width="320">
</p>

# ts-rust

The Rust rewrite of the TypeScript 7 native compiler and language server (the Go module under `tsc/` in microsoft/TypeScript, codename Corsa). Upstream is consumed as a pinned dependency; this repository is the workspace.

The first contract leaves are implemented: `tsr_jsstring` preserves source/string bytes and Go position semantics; `tsr_arena` provides checked identities, immutable file storage, lazy publication and bundle retention. S03 adds pinned schema generation for `tsr_ast`, `tsr_diagnostics` and `tsr_encoder`, with drift checks and byte-identical TypeScript client regeneration; see [S03](docs/S03.md). S05 adds the scanner, rescans, regexp recovery and Unicode tables with frozen Go differential evidence; see [S05](docs/S05.md) and its [reviewed implementation plan](docs/S05-implementation-plan.md). Subsequent slices add parsing, binding, program loading, the S08 checker subset and S09 ownership contracts. S10 adds [wasm and Rust/Node embedding interfaces](docs/S10.md); their performance and parity gates are tracked separately. The [S04 synthesis plan](docs/S04-synthesis-plan.md) records the implementation choices; [S04 documentation](docs/S04.md) describes the supported APIs and verification. Mapped functions and implementation labels are reported separately from verified parity.

## Layout

| Path | What it is |
|---|---|
| [PLAN.md](PLAN.md) | The canonical plan. Update the HTML mirror when it changes. |
| [Plan page](docs/corsa-in-rust.html) | The designed HTML mirror. An earlier version was published as a private page at https://claude.ai/code/artifact/6c72abf7-0d30-43fe-a7a8-6457828dcce8; that external copy is not automatically synchronized. |
| [Tracking](docs/TRACKING.md) | Ledger, function-mapping, evidence, experiment and sprint-check contracts. |
| [Rust implementation guide](docs/CODEX-RUST-GUIDELINES.md) | Codex/Astra rules for Rust implementation and code review; loading conditions are in [AGENTS.md](AGENTS.md). |
| [Current status](STATUS.md), [dashboard](docs/status.html) | Generated reports; evidence validity and parity are separate from implementation and mapping counts. |
| [Unmapped functions](status/unmapped-functions.json) | Complete function worklist linked from the compact JSON status summary. |
| [Architecture decisions](docs/adr/README.md) | Accepted ADRs 0001 to 0018 and the Proposed placeholders 0019 (test-host protocol) and 0020 (Phase 0 gate). |
| `PORTS.toml`, `data/go-functions.tsv` | Upstream file ledger and function inventory used for traceability. |
| `status/runs.toml`, `status/experiments.toml` | Reviewed run specifications and experiment gates. |
| [Phase 0 implementation plan](sprints/README.md), `sprints/` | Sprint files S01 to S12 with machine-checked exit criteria; the README gives the order, producers and conventions. |
| `rust-toolchain.toml`, `rustfmt.toml`, `deny.toml`, `Cargo.toml` lints | Pinned stable toolchain, formatting, dependency policy and the clippy allow-list (ADRs 0016 and 0017); `scripts/checks.py` runs them as producers. |
| `data/divergences.toml` | Owner-approved baseline divergences (ADR 0004); an input of the E2 producer. |
| `.github/workflows/status.yml` | Status workflow: provenance, archived-view check, live producer metrics and S01/S03/S04/S05 on the four macOS/Linux targets, minimum-Rust builds, artifacts including the worklist and scanner failure logs. |
| `crates/tsr_jsstring/`, `crates/tsr_arena/` | Text and ownership contract leaves; see [S04](docs/S04.md). |
| `crates/tsr_scanner/`, `crates/tsr_jsnum/`, `crates/tsr_core/` | Byte scanner, numeric conversion and shared target/range slices; see [S05](docs/S05.md). |
| `crates/tsr_embed/`, `crates/tsr_wasm/`, `crates/tsr_node/` | Rust sessions, bare wasm and Node-API adapters; see [S10](docs/S10.md). |
| `xtask/` | Local commands for evidence capture, status generation and sprint validation. |
| `data/import-graph.txt` | Internal import edges of the Go module (`importer imported`), produced by `go list`. 766 edges. |
| `data/topological-order.txt` | The packages in dependency order, leaves first, produced by `tsort` over the graph. The plan's crate map groups related packages; its dependency slices also use the actual import edges. |
| `data/MEASURED.txt` | Which TypeScript commit and Go version the data was measured with. |
| `scripts/import-graph.sh` | Regenerates `data/` from a TypeScript checkout: `scripts/import-graph.sh ~/git/TypeScript`. |

## Reading order

1. `PLAN.md`, section 1, for the scope and eight-point summary.
2. Section 6 for the architecture decisions, which are where the plan differs from a straight port.
3. Section 9 for prototype dependencies, full parity gates and the spike experiments.
4. Section 5 for the native, WebAssembly and Rust embedding cut-over criteria.
5. Section 13 for remaining implementation choices and settled contracts.
6. [Tracking](docs/TRACKING.md), [status](STATUS.md) and the [ADR index](docs/adr/README.md) for recorded work, current evidence and unresolved decisions.

## Tracking work

`cargo xtask run <run-id>` executes a reviewed specification from `status/runs.toml` and captures typed metrics with evidence bound to the upstream pin, command, that run's selected sources and declared corpus/configuration inputs. `cargo xtask status` validates that evidence and regenerates the reports; `cargo xtask check <sprint>` enforces sprint exit criteria and required item checks. Source globs default to compiler crates, Cargo manifests/lockfile, `.cargo` configuration, Rust toolchain selectors, `xtask` and scripts. Documentation and policy edits do not invalidate measurements unless that producer explicitly consumes those files; declared inputs and case manifests are always hashed.

The ledger's `status`, `rust` and `verify` fields are editable without regenerating upstream provenance. Threshold, sprint-check and ledger-progress changes reevaluate existing metrics; selected source or run-input changes invalidate affected evidence. Metric-producer contracts define the workload and assertions behind each measurement, so neither function markers nor a successful command alone establishes parity. See [Tracking](docs/TRACKING.md) for version-2 provenance, source selection and the run schema.

`cargo xtask check-metrics 'run.fmt.clean == true'` enforces measured results independently of unfinished sprints. `cargo xtask status --check-committed` verifies all four committed views against validated evidence using the recorded context and date, without rewriting files; live gates still require evidence valid for the current host and environment.

## Status

Draft 3.2, 5 September 2026, measured against microsoft/TypeScript commit `1f70213d49`. The canonical `upstream/` submodule is registered, initialized and clean at `1f70213d4922b434345f639b441681e470c7cfc1`; the actual Go oracle build and `--version` smoke test have passing execution evidence. ADRs 0006, 0007 and 0013 and the [ownership](docs/design/ownership.md), [symbols](docs/design/symbols.md) and [text](docs/design/text.md) design notes are accepted; S01 passes against the current bootstrap evidence. The S04 leaf portions of E3 and E4 and S05 scanner parity have executable producers; full E1–E8 verification remains pending; their sprints are the [Phase 0 implementation plan](sprints/README.md). A status workflow is installed under `.github/workflows/`; no run of it is recorded here. Bootstrap success does not establish Rust compiler parity or complete function coverage.

Native targets are macOS arm64/x64 and Linux x64/arm64 (glibc). The plan specifies file/lazy/bundle ownership, checker-local merges, generation-aware invalidation and raw-byte handling. Repeated identity checks may be elided only within a proven ownership scope, with release-mode rejection required at every unproven boundary. The spike tests bounded memory, WebAssembly and embedding prototypes; full compiler WebAssembly and Rust-consumer acceptance are required before cut-over. The owner approves baseline divergences, and work is sequenced by dependency slices and parity gates rather than a calendar.

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE)). ts-rust is a port of Microsoft's TypeScript compiler, itself licensed under Apache-2.0; [NOTICE](NOTICE) carries the attribution. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in ts-rust by you, as defined in the Apache-2.0 license, shall be licensed as above, without any additional terms or conditions.
