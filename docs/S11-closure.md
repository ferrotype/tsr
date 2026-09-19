# S11 closure — 2026-09-19

S11 is complete in the validated CI evidence for merged PR [#20](https://github.com/iantocristian/ts-rust/pull/20).
[ADR 0019](adr/0019-test-host-protocol-and-transport-contract-tests.md) records
the owner's approval as **Accepted**, the ledger's approved status.

The successful [CI run 35446072791](https://github.com/iantocristian/ts-rust/actions/runs/35446072791)
checked PR head `20a4348c1f4e407155f3e592ea63ea8bc33a0aed` through GitHub's test
merge `098941fd9e7a723bc606748bc85de5ad06d910c3`. The landed merge is
`60b65d4e127bd98640ac44964d4c47fdae06f205`. The archived records' source, input
and command fingerprints validate against the landed tree; a different commit
identity alone does not invalidate equal inputs.

| S11 requirement | macOS ARM64 | Linux x86-64 |
| --- | --- | --- |
| S06 prerequisite, including its transitive gates | pass | pass |
| ADR 0019 | Accepted | Accepted |
| `run.testhost.parity` | 1 | 1 |
| `run.testhost.controls` | true | true |
| `sprint.S11.done` | 1 | 1 |

Each transport capture covers 89 cases: 31 pinned Go filesystem cases, five
actual Go mapper byte-stream comparisons, 47 subprocess contracts and six
internal Session contracts. Both native producer jobs also refreshed the
correctness evidence required by S06, including Miri and AddressSanitizer.

## Retained evidence

The committed views and the 16 refreshed producer records come unchanged from
the `status-macos-15` artifact. Existing historical records remain available.
The views retain the runner's environment, host, Rust toolchain and revision;
they do not claim these commands executed on the local workstation. Run
`cargo xtask status --check-committed` to revalidate the archived rendering
against current sources and evidence. Ordinary `status` and live sprint checks
continue to use the caller's execution identity and freshness rules.

Both artifacts were checked for record and output hashes, matching source/input
declarations, successful captures and the closure metrics above. The original
downloads are retained locally under `target/s11-closure/`.

| Artifact | GitHub artifact ID | ZIP SHA-256 |
| --- | --- | --- |
| `status-macos-15` | `10588241376` | `144ba3156a855aab47441b9ee78eb118aa4679a6d13aae40e4f76806314acbc0` |
| `status-ubuntu-24.04` | `10587542081` | `18bfddca1d46cad6cec29040ff1092b379a85a0fc19c1f1a08324bc32b38e0d8` |

This closure changes no implementation, thresholds or evidence rules and runs
no new benchmark. S11 does not certify the Phase 5 synchronous blocked-worker
bridge, parse-cache injection, project-system integration, production mapper
host or semantic LSP/fourslash behavior. Those remain the explicit boundaries
accepted in ADR 0019. Other sprints retain their own evidence requirements.
