# Rust package preparation

This is publishing preparation on top of PR #34, not a release. The 28 public
packages remain version `0.1.0`; no crates.io upload has been performed. The
workspace defaults to `publish = false`. Package policy is explicit in
[packages.json](packages.json), including all 38 `tsr_` packages and six private
packages with other names. The Node addon is a separate distribution artifact.

## Reproduction

Run from the repository root with the pinned Rust toolchain and initialized
upstream gitlink:

```sh
python3 scripts/package_assets.py --check
python3 scripts/package_verify.py
```

`package_assets.py` without `--check` syncs package license/notice copies and the
bundled files from the gitlink's Git objects, not the working tree. Its check
rejects missing, changed and extra libraries, mismatched exported names, a
changed pin, unlisted packages and unpublishable dependencies. CI runs this
check before tracker validation. The manifest under `tsr_bundled/bundled/`
records all upstream asset sizes and SHA-256 digests.

`package_verify.py` asks Cargo to list and create the 28 actual normalized
archives with `--no-verify`, then extracts them to a temporary directory. It
builds every library with all features, runs an external parser/checker/ownership
consumer, builds the parser-only embedding surface and builds wasm in parser and
checker modes. The temporary workspace contains no `upstream/`, tools or source
checkout. Its only overrides are `[patch.crates-io]` entries pointing to extracted
archives. Cargo metadata rejects path dependencies outside that directory; the
root lockfile seeds external versions, which are checked against the repository
lockfile after resolution. No temporary registry is needed.

The archives include Cargo-generated lockfiles and normalized metadata. The
helper retains the extracted tree for inspection and writes detailed logs,
file lists, archive hashes and `verified.json` under `target/publishing-prep/`.
A failed rerun deletes the earlier passing summary. This verifies package
self-containment; it does not certify that dependencies already exist in the
public registry. A registry-only `cargo publish --dry-run` remains a release-time
check as sibling `0.1.0` versions become available. The four `0.0.0` placeholders
(`tsr_ast`, `tsr_checker`, `tsr_parser`, `tsr_wasm`) do not satisfy those versions.

## Content and private harnesses

Package manifests include source, README, license and notice files explicitly;
`tsr_bundled` also includes its pinned assets and `tsr_core` its Go BSD license.
Repository examples, integration tests, capture archives and fixture corpora
are excluded. Each package has a description and the current repository URL.
Tests embedded under `src` can still reference repository fixtures: running a
package's own tests outside the repository is not a supported promise here.

The include/path audit found external reads only in test modules after moving
the bundled assets. These cover the AST, binder, scanner, checker, compiler,
node-builder, printer, module, API and declaration-transformer fixtures. Internal
`#[path]` source splits remain inside each package. No public package has a build
script; the native Node addon build script belongs to a private package.

`tsr_compiler`'s private `s08_relater_prototype` development dependency is now
path-only. The encoder's parser development dependency is also path-only: its
codec fixtures are repository-only, and retaining a registry version creates a
parser/encoder test-dependency cycle on the first release. Other public dev
dependencies retain their versions. Cargo omits these two path-only entries from
normalized archives; repository tests retain the same local dependencies.

The former `tsr_wasm/corpus` feature is private capture functionality. It moves
to `s10_wasm_corpus`, which calls the shipped `tsr_embed` API and the unchanged
private `s10_corpus` walker. `scripts/s10_build.py --corpus` selects that package;
its output directory, `observe_corpus` ABI, error tag and JSON contract stay the
same. It no longer exports unrelated parser/checker classes and therefore no
longer imports the class throw shim. Public `tsr_wasm` parser/checker features
and exports stay unchanged. Old captures retain their own archived ABI wrappers.

## Before the first release

The project branding is **tsr**; the unrelated `ts-rust` crate on crates.io is
not this project. `tsr_embed` is the application entry point. A `tsr` facade that
re-exports the public embedding surface remains a separate follow-up; neither
`tsr` nor `tsrust` is added to this 28-package release set. All package READMEs
state the Rust 1.96 minimum, and package metadata includes search keywords and
the compiler category.

The current repository URL is still `https://github.com/iantocristian/ts-rust`.
On 2026-09-20, `ferrotype/tsr` did not resolve through GitHub. Decide the final
repository home before publication; if transferring/renaming, complete that move
and update the workspace repository field and README links before generating
release archives. Keeping a working URL now does not authorize a transfer.
Published archives retain their original manifest bytes; a later GitHub redirect
does not rewrite those bytes.

Do not schedule the first release as 28 immediate uploads. crates.io's current
[default limiter](https://github.com/rust-lang/crates.io/blob/5723cfaf552efd5e870d25c71f2bb5193b21a958/src/rate_limiter.rs)
allows five new crates in a burst and replenishes one slot per ten minutes.
Updates have a separate default allowance. If 24 of these names are still new
and the new-crate bucket starts full, the theoretical refill wait is 190 minutes
(3 h 10 min), plus publication, index visibility and validation time. Server
configuration and account overrides can change that allowance; check the actual
response rather than treating this estimate as a guaranteed schedule. Recheck
name ownership and availability before release.

Registry-only dry runs must follow dependency availability, using the order
below; the local extracted-package verification is already possible before any
publication. Do not remove dependency versions or publish placeholders merely
to make a registry dry run pass.

## Dependency-first release order

This order includes retained internal development dependencies as well as normal,
build and optional dependencies. It is for a later, explicitly authorized release:

1. `tsr_jsstring`
2. `tsr_arena`
3. `tsr_core`
4. `tsr_diagnostics`
5. `tsr_ast`
6. `tsr_jsnum`
7. `tsr_scanner`
8. `tsr_encoder`
9. `tsr_parser`
10. `tsr_binder`
11. `tsr_semver`
12. `tsr_tspath`
13. `tsr_vfs`
14. `tsr_bundled`
15. `tsr_tsoptions`
16. `tsr_module`
17. `tsr_nodebuilder`
18. `tsr_printer`
19. `tsr_pseudochecker`
20. `tsr_checker`
21. `tsr_astnav`
22. `tsr_format`
23. `tsr_transformers`
24. `tsr_compiler`
25. `tsr_project`
26. `tsr_api`
27. `tsr_embed`
28. `tsr_wasm`

## Package policy

| Package | crates.io | Reason |
| --- | --- | --- |
| `tsr_api` | Prepared | Rust library and its public dependency closure |
| `tsr_arena` | Prepared | Rust library and its public dependency closure |
| `tsr_ast` | Prepared | Rust library and its public dependency closure |
| `tsr_astnav` | Prepared | Rust library and its public dependency closure |
| `tsr_bench` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_binder` | Prepared | Rust library and its public dependency closure |
| `tsr_bundled` | Prepared | Rust library and its public dependency closure |
| `tsr_checker` | Prepared | Rust library and its public dependency closure |
| `tsr_compiler` | Prepared | Rust library and its public dependency closure |
| `tsr_core` | Prepared | Rust library and its public dependency closure |
| `tsr_diagnostics` | Prepared | Rust library and its public dependency closure |
| `tsr_embed` | Prepared | Rust library and its public dependency closure |
| `tsr_encoder` | Prepared | Rust library and its public dependency closure |
| `tsr_format` | Prepared | Rust library and its public dependency closure |
| `tsr_jsnum` | Prepared | Rust library and its public dependency closure |
| `tsr_jsstring` | Prepared | Rust library and its public dependency closure |
| `tsr_module` | Prepared | Rust library and its public dependency closure |
| `tsr_node` | Private | Node addon distributed separately; not a Rust library package |
| `tsr_nodebuilder` | Prepared | Rust library and its public dependency closure |
| `tsr_parser` | Prepared | Rust library and its public dependency closure |
| `tsr_printer` | Prepared | Rust library and its public dependency closure |
| `tsr_project` | Prepared | Rust library and its public dependency closure |
| `tsr_pseudochecker` | Prepared | Rust library and its public dependency closure |
| `tsr_scanner` | Prepared | Rust library and its public dependency closure |
| `tsr_semver` | Prepared | Rust library and its public dependency closure |
| `tsr_testhost` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_transformers` | Prepared | Rust library and its public dependency closure |
| `tsr_tsoptions` | Prepared | Rust library and its public dependency closure |
| `tsr_tspath` | Prepared | Rust library and its public dependency closure |
| `tsr_vfs` | Prepared | Rust library and its public dependency closure |
| `tsr_wasm` | Prepared | Rust library and its public dependency closure |
| `tsr_cpu_profile` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_memory_profile` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_s07_access_trace_native_verify` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_s07_bis_access_trace` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_s07_bis_owner_census` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_s07_bis_phases` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_s07_storage_pilot` | Private | Repository-only benchmark, experiment, test or capture tool |
| `s08_relater_prototype` | Private | Repository-only benchmark, experiment, test or capture tool |
| `s09_format_harness` | Private | Repository-only benchmark, experiment, test or capture tool |
| `s10_corpus` | Private | Repository-only benchmark, experiment, test or capture tool |
| `s10_rust_consumer` | Private | Repository-only benchmark, experiment, test or capture tool |
| `s10_wasm_corpus` | Private | Repository-only benchmark, experiment, test or capture tool |
| `xtask` | Private | Repository-only benchmark, experiment, test or capture tool |

See [the validation record](../../docs/CRATE-PUBLISHING-PREP.md) for measured checks
and the separate acceptance-evidence work still pending.
