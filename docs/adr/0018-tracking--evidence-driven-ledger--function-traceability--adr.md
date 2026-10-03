# ADR 0018: Tracking: evidence-driven ledger, function traceability, ADRs and a dashboard

Status: Superseded by ADR 0023 (2026-10-03) (2026-09-05)
Plan: [Tracking](../TRACKING.md); [PLAN.md, section 8](../../PLAN.md#8-strategy-and-crate-map)

## Context

The owner asked for a stricter way to distinguish implementation work from verified parity. The pinned Go implementation and baseline corpus provide an oracle, but file mappings and function markers alone do not establish semantic equivalence. Verification must be computed from current, reproducible evidence.

## Decision

Tracking has four parts: a port ledger in `PORTS.toml`; function mappings from `port:` markers to the pinned inventory in `data/go-functions.tsv`; Architecture Decision Records in `docs/adr/`; and generated status reports (`STATUS.md`, `status/status.json`, `docs/status.html`, with history in `status/history.jsonl`). The small JSON status summary links a complete package-grouped worklist in `status/unmapped-functions.json`. The ledger's `status`, `rust` and `verify` fields are editable. Manifest version 2 hashes only the canonical generated-field projection and global pin, alongside the complete inventory checksum; progress and verification-policy edits do not require regenerating upstream provenance. Function metrics report mapped and unmapped inventory entries, separately from test or baseline parity; they do not claim behavioral coverage.

`cargo xtask run <run-id>` executes a reviewed command specification from `status/runs.toml` and captures typed JSON metrics and version-bound artifacts. Each run declares its corpus/configuration inputs and may select source files with Git-style `sources` globs. Defaults cover compiler crates, Cargo manifests/lockfile, `.cargo` configuration, Rust toolchain selectors, `xtask` and scripts; documentation and policy files can be added explicitly. Evidence binds the upstream pin, that run's specification, selected source digest and declared input digests. `inputs` and `cases` are always hashed even outside the source set. Relevant changes invalidate the affected run; unrelated run-specification edits do not. Metric producers have reviewed contracts defining workload, source dependencies, denominator, units, success/error handling and every assertion they aggregate; a reported number or successful process exit alone is not proof of parity.

Experiment-threshold changes, sprint-check changes and editable ledger updates reevaluate existing measurements without rerunning unaffected producers. They invalidate evidence only if the producer explicitly consumes the changed files. This separation preserves measurement provenance while allowing acceptance policy to be reviewed independently; reevaluation does not create a new measured result or execution timestamp.

`cargo xtask status` validates evidence and computes effective verified states and experiment results; `--record` additionally records history. Neither verified status nor measured evidence is edited by hand. A file or crate is verified only when its required current test, baseline and generation gates pass. Experiments evaluate every required threshold and assertion rather than hiding secondary requirements in notes. `cargo xtask check <sprint-id>` enforces both sprint exit criteria and every required item's checks; a required item without enforceable checks cannot silently count as complete. The schema and command details live in [Tracking](../TRACKING.md).

## Consequences

Implementation labels remain human input; verified parity is a generated conclusion from validated evidence. Unknown `port:` markers fail status validation, but valid markers establish correspondence only. An incomplete inventory cannot support a claim of complete mapping coverage. A passing evidence record also cannot establish assertions its producer did not implement, which is why producer contracts and gate definitions require review.

A status workflow is installed at `.github/workflows/status.yml` (validate, archived-view check including the worklist, live producer metrics and S01 on macOS and Linux, minimum-Rust builds, artifacts); no run of it is recorded in the repository, and a rendered report does not establish that a push or nightly job ran. Archived-view checks preserve the recorded context and date while recomputing policy and validating source, input and artifact provenance; live gates require the current environment. S01's registered-submodule and oracle requirements have actual local checks and execution evidence: the canonical pinned checkout is initialized, and its Go build and version smoke pass. ADRs 0006, 0007 and 0013 and their design notes are accepted, and S01 passes against current bootstrap evidence; E1–E8 implementation and verification remain pending. Bootstrap evidence is not compiler-parity evidence.

Corrections and evidence additions may amend an ADR, recorded in its Amendments section. Ledger progress and ordinary implementation updates do not supersede a decision or require rewriting an ADR. Replacing an accepted architectural decision requires a new ADR that explicitly supersedes it. Rejected: hand-maintained completion checklists as the verification source of truth; Jira or Linear.

## Evidence

`docs/TRACKING.md`; `xtask/src/main.rs`; `scripts/ledger-init.py`; `scripts/go-inventory/main.go`.

## Amendments

The tracking-scaffold review separated editable implementation state and function mapping from generated verified parity, required version-bound runner evidence and complete gate evaluation, and made producer-contract review explicit. Claims of installed CI and manually entered experimental measurements were removed.

The follow-up review scoped source fingerprints per producer, separated acceptance-policy edits from measured inputs, introduced the version-2 generated-ledger projection and moved the complete unmapped-function list out of the status summary.

Bootstrap execution initialized the canonical pinned submodule and produced passing oracle build/version evidence; the design-note gates remain open and CI remains planned.
