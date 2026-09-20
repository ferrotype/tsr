# Crate prefix rename: `ts_` to `tsr_`

Implements steps 1–4 and the step-6 validation of
[CRATE-PREFIX-RENAME-PLAN.md](CRATE-PREFIX-RENAME-PLAN.md). Publishing
preparation (step 5) and the final evidence refresh (step 7) are separate.

Baseline commit `8729daff7b914867fe70715e3ef91dce91c53c9c`.

## What moved

The inventory re-enumerated at implementation time reproduces the plan exactly:
38 repository-owned prefixed packages (31 under `crates/`, 7 under `tools/`),
44 tracked `Cargo.toml`, 8 tracked `Cargo.lock`, 2,585 tracked tree entries.
No package has a `package = ` dependency override, and no package declares an
explicit `[lib] name`.

| | |
| --- | --- |
| Crate directories moved | 31 (723 files, byte-identical) |
| Packages renamed | 38 |
| Declared future crate names in `PORTS.toml` | 32 |
| Tracked files carrying a renamed identifier | 1,123 |
| Occurrences rewritten | 23,197 |

Preserved as specified: the explicit CLI names `ts-bench` and `list-pilot`, the
wasm-bindgen output names `parser`, `checker` and `corpus`, and every wire
protocol, diagnostic text and JavaScript API name. Artifact names that follow
the package name did change: `tsr_wasm.wasm`, `libtsr_node.dylib`,
`tsr_node.dll`, `libtsr_node.so` and the copied addon `tsr_node.node`.

## The boundary hazard

An unanchored `ts_module` matches the corpus file
`preserveValueImports_module.ts`, whose tail spells the crate name. That single
false positive accounted for fourteen `data/` files in a first pass. The
mapping therefore requires a non-word character on both sides and is built from
the baseline manifest inventory rather than a `ts_` prefix wildcard.

Eight residual `ts_*` tokens remain and are deliberately not crate names:

| Token | What it is |
| --- | --- |
| `ts__` | inside the corpus filename `lib.decorators.d.ts__.ts` |
| `ts_build_info_file` | the `tsBuildInfoFile` compiler option |
| `ts_compiler_error` | a local alias so sources shared between the crate and external harnesses compile either way |
| `ts_extension`, `ts_file`, `ts_or_json_extension`, `ts_priority` | `.ts` file predicates in the checker |
| `ts_other` | a synthetic fixture crate in an inventory unit test |

`libts_ast-f7392d082e67be11.rlib` also survives, in a recorded build-artifact
path inside a historical profiling README.

## What was deliberately left on the old names

- `status/evidence/**` (213 files): content-addressed evidence. It goes
  legitimately stale and is refreshed by its producers in step 7.
- Five S08 capture records — `data/s08/p2/record.json`,
  `data/s08/p2/review-fixes.json`, `data/s08/p3a/record.json`,
  `data/s08/p3b/record.json`, `data/s08/p3b/review.json` — whose `rust_command`
  field records what was executed at capture time. `scripts/s08_ownership.py`
  reads only `archive.path`, `archive.sha256` and `upstream_pin`; it never
  replays `rust_command`, so rewriting it would only falsify the record.
- Historical S07/S08 capture manifests, recorded reports, symbol names and
  frozen replay helpers. Their paths identify the original archive members,
  not the current checkout. `PLAN.md` and the active architecture document now
  both use the new prefix.

The `data/s08/p*/README.md` replay instructions were migrated, because their
"Regenerate with:" blocks invoke `cargo` against the *current* tree. An
instruction is not an observation.

## Evidence re-derived, never hand-edited

Migrating `scripts/s04.py`'s crate paths invalidated every fingerprint closure
that covers it. Each was re-derived by its own producer:

| Artifact | Result |
| --- | --- |
| `data/s09/printing-observations.json` | recaptured through the pinned Go toolchain; 13 rows byte-identical, only `environment_helper_sha256` moved |
| `target/s07-subset/source-observations.ndjson` | re-exported; **byte-identical** (`0a616509…`), only its provenance changed |
| `data/s07/e2-acceptance.json` | re-frozen from a fresh native capture; all 10,728 variants, the 9,369/1,359 counts and `e2-policy-observations.json` byte-identical |
| `data/s07/subset-rule.json` | re-frozen; `subset.json` and `checker-obligations.json` byte-identical, so selected variants, loading closure and checker obligations are unchanged |
| `data/s08` P0 contract | recaptured; `dependency-closure.json.xz` and `supplemental-observations.json.xz` byte-identical |
| `data/s03/generated.json` | regenerated; `cargo xtask gen --verify` reports `drift: false` |
| `data/upstream.json` | the ledger projection hash, which `crate` feeds |

The S07 subset re-freeze appends finding
`RENAME-2026-09-19-crate-prefix-operation-mappings`. It falls under the owner's
standing approval: `data/s07/operations.json` changed **only** inside
`rust_mappings`, at all 547 sites, in the leading directory component alone,
with everything outside `rust_mappings` byte-identical. The subsequent
`RENAME-2026-09-20-final-anchor-and-provenance` finding corrects one final-format
anchor: `PathIsRelative` moved from line 1452 to 1453 in
`crates/tsr_checker/src/node_builder_names.rs`. Source observations were exported
again after the final ledger projection; their bytes remain identical, and their
provenance now records the current `data/upstream.json` hash. The subset rule and
review were re-frozen from these exact inputs; subset and checker-obligation bytes
remain unchanged.

A fresh P0 capture drops the reviewer's `observation_reuse` provenance note.
Because the frozen observation is byte-identical to the committed one, that
note still holds and was preserved rather than deleted.

## Reverse-mapping comparison

The initial mechanical comparison at `963a848` is recorded below. It checks
invertibility of the namespace edit; it does not by itself establish whether a
historical reference should have been edited. Review repairs below add a strict
baseline-byte check for historical captures. All comparisons use read-only Git
plumbing; nothing was reversed in the working checkout.

```
baseline 8729daf: 2585 entries        proposed HEAD: 2587 entries
mapping collisions                                     0
destination names that already existed in the baseline 0
paths missing from the proposed tree                   0
paths added to the proposed tree                       0
file mode or object type changes                       0
byte differences after reversing the mapping           0
exact-path exceptions, each with a stated reason      60
RESIDUAL DIFF: 0
```

The two added paths are this file and the plan it implements. The 60
exceptions are the 56 below plus the four regenerated tracker artifacts
(`data/upstream.json`, `STATUS.md`, `status/status.json`, `docs/status.html`).

The 100755 entry and the `upstream` gitlink at
`1f70213d4922b434345f639b441681e470c7cfc1` compare identically. Everything
under `status/evidence/` compares byte-for-byte with no normalisation, because
the forward pass never touched it.

### The 56 exceptions

**44 rustfmt re-wraps.** `tsr_` is one character wider than `ts_`, so lines
near the column limit wrap differently. Verified mechanically: after removing
whitespace and wrap-induced trailing commas, 43 of 44 files are identical; the
44th (`tools/s09/format-harness/src/main.rs`) gained braces around a wrapped
closure body, which then required a semicolon for
`clippy::semicolon_if_nothing_returned`.

**12 content-derived fingerprints** that a text mapping cannot reverse: the
seven re-derived artifacts above, plus `data/s07/subset-review.json`,
`data/s08/query-contract.json`, `data/s08/dependency-audit.json`,
`data/s08/type-footprint.json` and `data/s08/checker-workload.json` (pinned
input digests), and `xtask/src/tests.rs`, whose expected projection hash
follows its own inline fixture — `scripts/ledger-init.py` independently
produces the new value `637f37b9…`, so the Rust and Python canonicalisations
still agree.

## Review repairs and targeted verification

Historical S07 capture metadata and frozen replay helpers were restored from
`8729daf` (31 files), along with five S08 historical reports/manifests. Current
reproduction instructions still name the current crates. No archive payload was
rewritten. The restored gate-rebase archive replays successfully with all 468
members and its declared validator; all restored files match the baseline bytes.

The live CPU analyzers accept both old and new crate namespaces for selectors
only. Captured frame names, self/inclusive report keys and artifact hashes remain
unchanged. Offline bind comparison reproduces 2,528,000,000 ns for Rust and
730,000,000 / 670,000,000 / 690,000,000 ns for the three Go runs. The full
comparison equals the archived final result apart from provenance. New coverage
checks both spellings, rejects generic/foreign-name false positives, and verifies
that report labels retain the original spelling.

The final operation inventory is generated from actual source lines. Two cheap
checks now catch a moved marker or stale syntax-producer input before a full Go
producer run. The 12 operation-contract tests, five CPU comparison tests and 13
analyzer contracts pass. `cargo xtask validate` passes. These repairs do not change
Rust implementation code, acceptance thresholds or selection.

The additional non-rename edits are explicitly scoped to:

- `data/s07/operations.json`: the single `PathIsRelative` line number.
- `data/s07/subset-rule.json`: the operation hash, syntax provenance and review hash.
- `data/s07/subset-review.json`: the candidate rule hash and appended repair finding.
- `scripts/tests/test_s07_operations.py`: marker-location and input-fingerprint checks.
- `tools/s07/cpu-profile/analyze_xctrace.py`: selector-only compatibility normalization.
- `tools/s07/cpu-profile/compare_bind.py`: apply compatibility to phase/group selection.
- `tools/s07/cpu-profile/test_compare_bind.py`: old/new capture regression coverage.
- This record and regenerated tracker views: current validation and source identity.

Restored historical files have no remaining diff from the baseline. `PLAN.md`
changes only mapped crate names. The final reverse comparison reports no missing
paths or mode/type changes; the added paths remain this record and the plan.

## Original implementation validation

These existing results are retained; the repair did not rerun the workspace
build, lint, wasm, addon or embedding suites. CI reruns on the pushed revision.

| Check | Result |
| --- | --- |
| `cargo check --workspace --all-targets --all-features --locked` | pass |
| MSRV 1.96.0, same flags | pass |
| `cargo xtask run fmt` / `clippy` / `deny` | clean |
| `cargo test --package xtask` | 65 passed |
| `python3 -m pytest scripts/tests` | 517 passed, 1 skipped, 952 subtests |
| `cargo xtask gen --verify` | `drift: false`, `patches_apply: true`, `client_identical: true` |
| `cargo xtask validate` | pass |
| `cargo xtask status --check-committed` | committed views match validated evidence |
| `python3 scripts/s11.py capture` | every testhost case passes |
| Bare wasm `parser` and `checker` builds | pass, emitting `tsr_wasm.wasm` |
| Native addon + `tools/s10/node/test.mjs` | 10 fixtures, 4 lifecycles, 40 retained outputs |
| External Rust consumer and lifetime checks | 6 passed |
| 186 crate source globs in `status/runs.toml` | each selects exactly the same files as its baseline form |

## Pre-existing conditions, not regressions

Each reproduces identically at the baseline commit:

- Four standalone workspaces fail `cargo metadata --locked`:
  `tools/s07/cpu-profile/rust` and the `access-trace`, `owner-census` and
  `phases` performance experiments. The cpu-profile lockfile predates a
  `hashbrown` dependency in the crates; updating it would be a dependency
  change, which this rename does not make.
- Three of those lockfiles order their root `[[package]]` before
  `tsr_diagnostics`, which is not sorted. The rename preserves that ordering
  exactly rather than reordering them.
- `tools/s07/memory-profile/build.py` stages only `crates` and `xtask` but
  copies the root manifest verbatim, so the staged workspace references
  `tools/s08/relater-prototype`, which is not staged.
- Three `status/runs.toml` globs match nothing: `clippy.sources: clippy.toml`,
  `e3.sources: tools/s09/printing*/**` and
  `program.sources: data/s07/config-mappers-*.json`. None names a crate.

## Evidence state on this branch

Moving every crate directory changed every producer's source fingerprint, so
the tracker reports 0 verified lines (from 20,735) and 0 of 8 experiments
passing (from 3 of 8). No threshold, corpus or ledger status changed, and 23
files remain ported. The plan refreshes this only after publishing preparation
is final, so these rows stay explicitly stale until then. The checker wasm
artifact also grew from 19,623,774 to 19,645,079 bytes on a rebuild, which is
the symbol-name size effect the plan anticipates for S10 artifact evidence.
