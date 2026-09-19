# Rename Rust crates from `ts_` to `tsr_`

Status: proposed; implementation has not started.
Inventory date: 2026-09-19.
Owner decisions: split directory moves from content edits; include publishing
preparation before the final evidence refresh. Actual publication is out of scope.

## Goal and boundaries

Rename every repository-owned Rust package whose name starts with `ts_` to
`tsr_`, preserving its suffix. Rename the corresponding Rust import namespace
and directories under `crates/`, and migrate all active consumers. The rename
itself is mechanical. A separately reviewed publishing-preparation change adds
package metadata, explicit publication policy and self-contained package assets.
Algorithms, layouts, dependency versions, upstream pins, acceptance thresholds
and shipped API/protocol behavior stay the same. Any capture-only feature
relocation needed for packaging is an explicit exception, not part of the rename.

The inventory currently contains 31 main crates and seven diagnostic or
experimental packages. Re-enumerate it at implementation time because S08 and
other development can add packages. Packages without the prefix, including
`xtask` and the S08–S10 harness packages, retain their names but must have their
dependencies and imports updated.

No GitHub repository or organization rename, npm package rename, or TypeScript
API rename is included. Current workspace manifests disable publishing. Four
new names already have `0.0.0` reservation packages owned by `iantocristian`,
verified against the crates.io crate and owner APIs on 2026-09-19:
[`tsr_ast`](https://crates.io/crates/tsr_ast),
[`tsr_checker`](https://crates.io/crates/tsr_checker),
[`tsr_parser`](https://crates.io/crates/tsr_parser), and
[`tsr_wasm`](https://crates.io/crates/tsr_wasm). These contain no implementation;
retain the workspace's existing versions rather than adopting `0.0.0`.
Do not introduce old-name compatibility crates or dependency aliases. Record
the Rust namespace change for external consumers instead. No registry upload,
additional name reservation, ownership change or release tagging is included.

## Rename inventory

Each main package's directory changes from `crates/<old>` to `crates/<new>`.

| Current package | New package |
| --- | --- |
| `ts_api` | `tsr_api` |
| `ts_arena` | `tsr_arena` |
| `ts_ast` | `tsr_ast` |
| `ts_astnav` | `tsr_astnav` |
| `ts_bench` | `tsr_bench` |
| `ts_binder` | `tsr_binder` |
| `ts_bundled` | `tsr_bundled` |
| `ts_checker` | `tsr_checker` |
| `ts_compiler` | `tsr_compiler` |
| `ts_core` | `tsr_core` |
| `ts_diagnostics` | `tsr_diagnostics` |
| `ts_embed` | `tsr_embed` |
| `ts_encoder` | `tsr_encoder` |
| `ts_format` | `tsr_format` |
| `ts_jsnum` | `tsr_jsnum` |
| `ts_jsstring` | `tsr_jsstring` |
| `ts_module` | `tsr_module` |
| `ts_node` | `tsr_node` |
| `ts_nodebuilder` | `tsr_nodebuilder` |
| `ts_parser` | `tsr_parser` |
| `ts_printer` | `tsr_printer` |
| `ts_project` | `tsr_project` |
| `ts_pseudochecker` | `tsr_pseudochecker` |
| `ts_scanner` | `tsr_scanner` |
| `ts_semver` | `tsr_semver` |
| `ts_testhost` | `tsr_testhost` |
| `ts_transformers` | `tsr_transformers` |
| `ts_tsoptions` | `tsr_tsoptions` |
| `ts_tspath` | `tsr_tspath` |
| `ts_vfs` | `tsr_vfs` |
| `ts_wasm` | `tsr_wasm` |

The following tool package names also change. Their descriptive directories
under `tools/` remain unchanged.

| Manifest directory | Current package → new package |
| --- | --- |
| `tools/s07/cpu-profile/rust` | `ts_cpu_profile` → `tsr_cpu_profile` |
| `tools/s07/memory-profile/rust` | `ts_memory_profile` → `tsr_memory_profile` |
| `tools/s07/performance-experiments/access-trace-native-verify` | `ts_s07_access_trace_native_verify` → `tsr_s07_access_trace_native_verify` |
| `tools/s07/performance-experiments/access-trace` | `ts_s07_bis_access_trace` → `tsr_s07_bis_access_trace` |
| `tools/s07/performance-experiments/owner-census` | `ts_s07_bis_owner_census` → `tsr_s07_bis_owner_census` |
| `tools/s07/performance-experiments/phases` | `ts_s07_bis_phases` → `tsr_s07_bis_phases` |
| `tools/s07/performance-experiments/storage-pilot` | `ts_s07_storage_pilot` → `tsr_s07_storage_pilot` |

`PORTS.toml` also declares 32 future `ts_` crate names without current crate
directories. Migrate those declarations and planned paths to the new prefix;
do not create those crates or change their completion state.

## Artifact and compatibility decisions

| Surface | Decision |
| --- | --- |
| Cargo package names, dependency keys and Rust library names | Use `tsr_` consistently; no old-name aliases. |
| Implicit binary names, including testhost and profiling tools | Follow the new package name; update launchers and executable matching. |
| Explicit binaries such as `ts-bench` and `list-pilot` | Preserve their existing CLI names. |
| Feature names | Preserve shipped features and update dependency-qualified references such as `ts_ast/layout-profile` and `dep:ts_checker`; separately review capture-only feature relocation in publishing preparation. |
| Raw Cargo wasm artifact | `ts_wasm.wasm` becomes `tsr_wasm.wasm`. |
| wasm-bindgen outputs | Preserve existing explicit output names (`parser`, `checker`, `corpus`) and JS exports. |
| Native addon | Rename Cargo library artifacts and the internal copied addon to `tsr_node.node`; migrate all loaders together. |
| Wire protocols, diagnostic text and JavaScript API | Preserve behavior and names, except executable usage text that names a renamed binary. |
| Historical evidence and captures | Preserve their original contents and identities. |

## Commit structure

Use new commits in this order, without rewriting published history:

1. Directory moves only: move `crates/ts_*` to `crates/tsr_*`, preserving every
   file's bytes and mode. Record exact old/new path pairs.
2. Rename content: package names, imports, generators, regenerated namespace
   references, active data/configuration and documentation from steps 2–4 below.
   This commit restores a buildable tree and gets the strict reverse-mapping check.
3. Publishing preparation: metadata, publish policy, package assets and any
   necessary separation of capture-only dependencies. Review this diff separately
   from the rename; give each non-rename change its own validation.
4. Final evidence and generated tracker views, after both scopes are complete.

The directory-only intermediate commit deliberately has stale workspace paths
and will not build. Treat the first two commits as one integration unit; do not
merge or validate the intermediate state as a completed migration. The split
makes the changes easier to inspect; Git still detects renames from snapshots.

## Implementation sequence

### 1. Establish a complete baseline

Start a dedicated rename branch from the agreed integration revision. Do not
perform the migration over another session's dirty checkout. Coordinate its
landing with ongoing S08 work; refresh the package inventory immediately before
implementation and again before merge.

Enumerate every tracked `Cargo.toml`, not just root workspace members. Capture
package names, target names, path dependencies, features and lockfile ownership.
Some tools are independent workspaces; others are templates staged by runners.
In particular, the memory-profile manifest has paths intended for staging, so
validate it through its runner rather than assuming it builds in place.

Record the pre-change dependency graph and current tracker state, including any
already-stale evidence or existing failures. Identify active consumers in
scripts, CI, generator templates, examples, documentation and sibling projects
such as jscout if they reference these crates. Changes outside this repository
should be listed separately, not silently bundled into the rename.

Include tracked `data/` files and root metadata in this inventory. At this
revision, 37 tracked text files under `data/` mention current crate names.
Classify each reference by its consumer: executable configuration, maintained
source-location metadata, generated manifest, or historical observation. Being
under `data/` does not make a file immutable evidence. Record the exact baseline
commit and a tracked-tree inventory for the reverse-mapping comparison in step 6.

### 2. Rename packages, directories and Rust references

Use one explicit old-to-new mapping derived from the manifest inventory. Move
the 31 crate directories and update root workspace membership, all package
names, path dependencies, dependency keys and any explicit `package`/target
name overrides. Apply the same namespace mapping to the seven tool packages.

Update imports, qualified paths, macros, doctests, examples and compile-fail
fixtures. Inspect feature references, `--extern` arguments, Cargo executable
environment variables, build scripts and embedded source strings. Preserve
suffixes exactly: `ts_tsoptions` becomes `tsr_tsoptions`, not `tsr_options`.

Avoid an unrestricted replacement of the substring `ts_`: it can occur inside
unrelated identifiers, upstream content or recorded outputs. Rewrite known crate
identifiers and paths, then inspect residual matches individually.

Update all eight currently tracked lockfiles, including independent profiling
workspaces and `tools/s10/rust-consumer/Cargo.lock`. Preserve third-party versions,
checksums and dependency resolution; do not run a dependency upgrade. Compare
the resulting graphs modulo the rename mapping and investigate any other change.

### 3. Update generators and regenerate their outputs

Change generator source templates, emitted imports, output paths and validation
lists in `xtask/src/gen/`, including AST, runtime, diagnostics and encoder
generation. Updating only generated files would let the next generation restore
the old namespace.

Run the existing generator using pinned prerequisites after its templates and
paths are updated. Review the generated diff for namespace-only changes and
verify the existing drift check. Preserve upstream schemas, symbol identities,
`// port:` markers and encoded output semantics.

### 4. Migrate build, runtime and tracking consumers

Update Cargo package selectors and paths in CI, producer registries, Python
scripts, shell commands and maintained documentation. Specific audit points:

- `scripts/s10_build.py`: raw wasm and native library names on macOS, Linux and
  Windows, the copied addon, and external Rust consumer dependencies.
- `tools/s10/node/` and `scripts/s10_measure.py`: addon loading, hashing and
  recorded artifact paths. Keep explicitly named wasm-bindgen outputs stable.
- `scripts/s11.py`: Cargo JSON target matching for the testhost executable.
- S07 profiling and experiment runners: staging templates, executable selection,
  process filters, stack-symbol classification and paths used to analyze new runs.
- `PORTS.toml`, `status/runs.toml` and active sprint/configuration files: both
  implemented and planned crate names, source globs and fingerprint inputs.
- `data/s09/ownership-cases.json`: migrate package fields used directly by
  `scripts/s09_ownership.py` in `cargo test --package`, along with that script's
  package validation. Preserve test filters, identities and expected outcomes.
- `data/s07/operations.json`: migrate maintained Rust source paths without
  changing operation identities or classifications. Audit producer dependencies
  through both explicit paths and globs; `program` and `e2` explicitly list this
  file, and `selftest` includes it through `data/**` in the current registry.
  Also inspect indirect consumers rather than assuming a fixed run count.
- The remaining `data/` inventory: migrate live package/path fields and regenerate
  affected generated manifests with their producers. Preserve historical records
  and archive contents; review mixed-purpose files field by field.
- `.gitattributes`: migrate all three generated-file rules for AST, diagnostics
  and encoder paths. Verify the same files retain `linguist-generated=true`.
- `NOTICE`: update `crates/ts_core/src/go_sort.rs` to its new location while
  preserving the attribution and license text.

Verify source globs still select the intended files after directory moves. A
producer that silently matches no crate files is a correctness defect, even if
its command succeeds. Keep port counts and upstream function identities stable.

Update current build instructions, architecture references and code links. Keep
historical experiment records, archived reports and content-addressed evidence
unchanged. If a maintained replay tool needs both naming schemes, use an explicit
historical compatibility mapping in that tool; do not rewrite the old capture.

### 5. Prepare packages for publishing

Complete this work before refreshing evidence so the planned metadata and asset
changes do not require a second refresh immediately after the rename. The owner
has approved this scope, including the capture-harness separation and tracked
bundled-file copies described below. These belong in the publishing-preparation
commit, not the mechanical rename commits. Preparation is not a release and does
not make an API stability promise.

**Metadata and publication policy.** Add accurate per-package descriptions,
repository metadata (currently `https://github.com/iantocristian/ts-rust`), and
package-appropriate README/license/notice files. Inherit common metadata where
useful, but verify the normalized archive contains the resolved values and files.
Do not guess a future organization URL. Keep the existing license declarations
and preserve all third-party notices.

Keep the workspace's default `publish = false`. Record an explicit package
publication table covering all 38 packages and the otherwise-named harnesses.
Library packages intended for Rust consumers and their complete normal/build
and optional dependency closure may opt into `publish = ["crates-io"]` once
packaging checks pass. Benchmarks, testhost, experiment tools, `xtask` and private
capture harnesses stay unpublished; the Node addon remains unpublished on
crates.io unless there is a separate Rust distribution use for it. Record a
reason for each exception rather than enabling every package globally.

Audit version-plus-path dependencies throughout that closure, including optional,
target-specific and development dependencies. Inspect the normalized packaged
manifest as well as the workspace manifest: retained development dependencies
must resolve for packaging even though downstream consumers do not inherit them.
Build a dependency-first release order for later use.
An unpublished optional normal dependency is still a packaging concern even when
its feature is off. Specifically, `tsr_wasm` currently depends on the private
`s10_corpus` package through its capture-only `corpus` feature. Prefer moving that
capture surface into a private wasm harness that uses the shipped crate; migrate
its runners and verify capture parity separately. Do not publish the harness or
silently delete coverage to make packaging pass. Keep `tsr_wasm` publication
blocked until this dependency is resolved, and record any changed capture-only
feature contract in the packaging diff.

`tsr_compiler` has a second, smaller blocker: its development dependency on the
private `s08_relater_prototype` currently specifies both a path and `version =
"0.1.0"`. Remove that dependency's version and retain its local path. Cargo omits
unversioned development dependencies from the published manifest; verify the
prototype is absent from the normalized archive manifest and remains available
for repository tests. Keep the prototype unpublished. This compiler package is
in the embedding dependency closure, so include this check alongside the wasm
case. Do not apply this fix to normal/build dependencies, and do not remove
versions indiscriminately from legitimate published development dependencies.
See [Cargo's development-dependency packaging rule](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#development-dependencies).

**Self-contained bundled assets.** Copy the pinned library files and
`CopyrightNotice.txt` into `crates/tsr_bundled/` and use crate-local include paths.
This explicitly includes tracking the copies in Git. At upstream gitlink
`1f70213d4922b434345f639b441681e470c7cfc1`, the current include inventory is
108 library files plus the copyright notice, totaling 3,785,888 bytes. The size
is accepted as part of making the package self-contained; recalculate the
inventory if the pin changes.
Provide a deterministic sync/check command tied to the upstream gitlink; retain
file bytes exactly, including line endings. Record the relative file inventory
and hashes, and require equality with the pinned source. Update any producer that
assumes the old asset location. Check ordering, exposed library names, copyright
text and bundled lookup results remain unchanged. Consumers of the packaged
crate must not need an `upstream/` checkout or a build-time download.

Audit every release package for other compile-time reads outside its directory,
including build scripts, includes, generated source, README and license inputs.
Classify reads by their compilation context. The external fixture reads in
`tsr_binder/src/containers.rs` and `tsr_module/src/package_json.rs` are inside
`#[cfg(test)]` modules; they do not block consumer library builds. Keep those
fixtures and the private prototype in the repository test workflow. Running the
full test suite from an extracted package is a separate capability, not a
requirement for this preparation pass; document that limitation instead of
copying the fixture corpora or deleting tests. Apply the same classification to
other external reads found by the audit.
Inspect `cargo package --list` and the resulting archives; avoid accidentally
shipping profiling archives or fixture corpora.

**Packaging verification.** Follow the pinned Cargo's supported packaging flow.
Use `cargo package` or `cargo publish --dry-run` where registry dependencies are
available; neither step uploads a release. The `0.0.0` placeholders do not satisfy
local `0.1.0` dependencies. For the not-yet-published dependency graph, prefer
isolated local dependency overrides pointing only to extracted package contents.
A temporary registry is not a required deliverable; do not build registry or
release infrastructure merely for this preparation pass. Use the package set
selected in the publication table and its actual dependency closure, not an
assumed 31-package release.

`--no-verify` is allowed as an archive-generation step where useful; it is not
verification and does not bypass dependency resolution failures. Inspect Cargo's
normalized package manifests and file lists, then build the extracted package
closure and representative external Rust consumers/wasm outputs with no access
to the repository or upstream tree. Resolve all internal dependencies to the
extracted copies and record any staging-only manifest/configuration overrides.
Use a temporary directory and a small helper if needed, not a general packaging
framework. Keep existing repository tests as the semantic checks; do not rerun
the entire test corpus from every package archive.

If pinned Cargo cannot generate an archive before sibling versions exist in the
registry, record that exact registry-dependent check as pending. Verify what can
be checked locally and distinguish any staged approximation from a Cargo-built
archive. Label isolated builds as local package verification, not a successful
crates.io dry-run. Never publish dependencies just to unblock verification.

Reference: [Cargo publishing and package inspection](https://doc.rust-lang.org/cargo/reference/publishing.html)
and [version-plus-path dependencies](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#multiple-locations).

### 6. Validate once on the integrated change

Begin with cheap structural checks: Cargo metadata for the root and standalone
workspaces, the full package mapping, dependency graph comparison, and a search
for remaining active old-name references. Allowlist historical references with
a stated reason. Confirm generated files and tracker source paths resolve.

Prove the mechanical scope with a reverse-mapping tree comparison, not merely
Git's rename similarity display:

1. Compare the recorded baseline commit with the proposed tree using a read-only
   checker or temporary snapshots. Never reverse names in the working checkout.
2. Reverse the approved directory, package, import and artifact-name mappings in
   the new snapshot, using the same identifier/path boundaries and file/field
   scope as the forward migration. Reject mapping collisions and unexpected
   additions or deletions. Check for pre-existing destination names first.
3. Compare every tracked path and file's bytes after normalization, plus file
   modes, symlink targets and submodule identities. Unchanged historical evidence
   must compare byte-for-byte without any normalization. Include `data/`, root
   metadata, generated Rust and all lockfiles; do not limit the check to `.rs`.
4. Require an empty residual diff for the mechanical migration. Review any
   necessary exception explicitly: for example, a formatter-induced line wrap or
   Cargo lockfile reordering. Do not normalize arbitrary whitespace or exempt
   whole directories to hide such differences.
5. Run this on the rename-content commit before publishing preparation, then
   again on the final PR tree. Publishing metadata, copied assets, capture-harness
   separation, new evidence, refreshed status views and migration documentation
   require an exact-path exception list with reasons and their own validation.
   For mixed-purpose files, enumerate the permitted fields/hunks rather than
   exempting the entire manifest or source file. Existing capture contents remain
   immutable. Keep both comparison results and the exception list in the PR.

This check establishes that changes follow the approved rename mapping; it does
not replace loader smoke tests or prove that every required consumer was found.

Then use the existing CI validation commands and feature matrices rather than
inventing another test suite for a rename:

- Workspace build/check and existing tests, formatting, clippy and deny checks.
- The declared MSRV check with `--workspace --all-targets --all-features --locked`.
- Generator verification and affected correctness/parity producers.
- Native CI on all four supported runners, including addon artifact loading and
  testhost launch behavior where those jobs already run.
- Bare wasm parser/checker builds and JS smoke tests, plus the independent Rust
  embedding consumer and its lifetime checks.
- Profiling-tool build/launch checks through their actual runners, without
  repeating full profiling captures just to prove executable discovery works.

Do not rerun completed checks without a relevant change or failure. Reuse CI
results for the exact final revision rather than duplicating them locally.

### 7. Refresh evidence honestly and close the migration

Changes to crate paths, source contents and Cargo.lock legitimately invalidate
evidence. Never edit historical source hashes, content-addressed JSON, or latest
pointers to make old evidence appear fresh.

After both the rename and publishing-preparation changes are final, regenerate
the affected required
correctness evidence and tracker views using the existing producers. Run
`cargo xtask validate` and `cargo xtask status --check-committed` on the resulting
state. Compare status changes with the baseline; report pre-existing failures
separately from migration regressions.

Treat performance evidence separately from correctness. A namespace-only diff
does not make an old binary fingerprint valid for a new build. If fresh E5/E6
evidence is required for closure, schedule one final standard paired capture on
a quiet host. Until then, leave those rows explicitly stale and closure pending;
do not change thresholds or claim current performance acceptance. Refresh affected
S10 artifact-size/performance evidence under its existing policy as well, because
rebuilt binary names and symbols can change measured output sizes.

Prepare one coherent PR describing the package/artifact mapping, validation and
any outstanding evidence. Review the final diff for accidental behavior changes,
dependency updates, historical-data edits and unrelated concurrent work. Rollback
is a normal revert, not a rewrite of published history.

## Completion criteria

- Every current repository-owned `ts_` package has a unique `tsr_` replacement;
  root and independent manifests, lockfiles and generated outputs agree.
- Future crate declarations use the new prefix without claiming new port work.
- Active Rust consumers, CI commands, runtime loaders and source globs use the
  new names; remaining old names are documented historical references.
- Explicit CLI names and JS/protocol contracts remain stable as specified above.
- Required checks pass for the final revision, and evidence is either freshly
  valid or explicitly pending. No acceptance threshold or corpus changed.
- Publication policy is explicit for every package; metadata, bundled assets and
  packaged dependency closure are verified independently of the source checkout.
  Any unresolved package blocks are named; none are disguised as readiness.
- Publishing preparation lands before the final evidence refresh; no packages
  are uploaded as part of this task.
- Historical captures remain intact; tracker views match the actual source tree.
- Active `data/` consumers, `.gitattributes` and `NOTICE` resolve the new paths;
  the reverse-mapping comparison has no unexplained residual changes.

## Plan review

The primary risks are incomplete scope and false freshness, not Rust algorithm
changes. A root-workspace-only rename misses seven tool packages; editing imports
alone misses generated source and dynamic artifact loaders; rewriting every file
corrupts historical evidence. The inventory, platform checks and evidence policy
above address these separately. Publishing preparation introduces intentional
non-rename changes, so it has a separate commit, an exact exception list, asset
byte checks and isolated package verification. No implementation or validation
runs are part of writing this plan.
