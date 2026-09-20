# Crate publishing preparation

2026-09-20. Stacked on PR #34 (`83c6351`). Implements step 5 of
[the rename plan](CRATE-PREFIX-RENAME-PLAN.md); no uploads or version bumps.
[Package policy and reproduction](../tools/packaging/README.md) cover all 44
packages, of which 28 opt into crates.io. This changes packaging, assets and a
private harness; it is reviewed separately from the mechanical rename.

## Changes

- Public libraries have descriptions, repository metadata, package README files,
  explicit included content, Apache license and attribution notices. Private
  tools retain the workspace's unpublished default.
- `tsr_bundled` contains exact copies of 108 libraries and `CopyrightNotice.txt`,
  3,785,888 bytes, from gitlink `1f70213d4922b434345f639b441681e470c7cfc1`.
  The bytes, names and ordering exposed by the Rust crate are unchanged. A
  pin-bound SHA-256 inventory and CI check protect those copies.
- `tsr_wasm` has no private corpus dependency. The existing walker entry point is
  in a private wasm harness over the public embedding API; the runner selects it
  explicitly. The checker and parser API contracts stay unchanged.
- Cargo strips the compiler's private prototype dev dependency and the encoder's
  repository-only parser dev dependency. The latter breaks a first-release
  development-dependency cycle; legitimate remaining versions stay in place.
- External dependency versions and checksums are unchanged in the repository
  lockfile. Its only graph changes move the corpus dependency to the new harness.

## Validation

- `python3 scripts/package_assets.py --check`: 167 files checked, including the
  complete bundled inventory and all package license/notice copies.
- `python3 scripts/package_verify.py`: 28 real Cargo archives generated and
  inspected; normalized manifests contain resolved metadata and no local paths.
  All libraries build with all features in a temporary extracted workspace.
  The external consumer parses/encodes, observes TS2322, retains a type across
  session drop, then returns tracked allocations to zero. The parser-only
  embedding crate and both wasm feature modes build from the extracted packages.
- `cargo xtask validate` passes. Tracker views are regenerated from the final
  staged source tree and checked against the archived evidence.
- Scoped clippy (bundled, wasm and the new harness, all features, warnings denied)
  and formatting pass. Eight S10 producer-contract tests and 12 operation-contract
  tests pass. Source port anchors and S07 operation selection are unchanged.
- The packaged parser and checker wasm binaries pass all 10 parser fixtures,
  exact import checks and the existing JS ownership/error-isolation checks after
  pinned wasm-bindgen processing (and the normal parser `wasm-opt -Oz` step).
- The private wasm harness reproduces all 40 retained S10 smoke observations by
  decoded JSON equality. Request digest:
  `a40b269a537c8228df6cf5bb1fc16e9b1c3f7a6204ba65997d6ee9852db047a5`.
  This compares against the pre-split capture, not a fresh Go acceptance batch.

Package build logs, archives, runtime checks and detailed provenance are retained
under `target/publishing-prep/`. The compact checked-in verification record is
[verification.json](../tools/packaging/verification.json). It does not feed sprint
acceptance metrics.

## Remaining work

Step 7 is still pending: refresh final producer evidence after all package
changes, including the quiet-host paired E5/E6 capture and complete S10 evidence,
then review final gates and native CI. No thresholds, workload membership or
historical captures were changed. Tracker views must describe the stale evidence
honestly; package checks do not make those captures current.

Actual publication, registry-only dry runs as sibling `0.1.0` releases become
available, and any API stability commitment remain outside this PR. The package
archive checks and isolated builds above are complete without those releases.
