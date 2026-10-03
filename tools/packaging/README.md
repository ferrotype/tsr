# Rust package preparation

The publication policy includes 44 public packages (42 libraries, the
`tsrust` command line and the `tsr` facade) and 27 private
packages, all named explicitly in [packages.json](packages.json). The workspace
defaults to `publish = false`. This policy and the dependency order below track
the current source tree; they are not a record of registry releases. The Node
addon is a separate distribution artifact.

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
changed pin, unlisted packages, unpublishable dependencies and any dependency
cycle among the public packages, retained development dependencies included
(publishing resolves those against the registry, so a cycle can never be
published at a new version). CI runs this check before tracker validation. The
manifest under `tsr_bundled/bundled/` records all upstream asset sizes and
SHA-256 digests.

`package_verify.py` asks Cargo to list and create all public packages' normalized
archives with `--no-verify`, then extracts them to a temporary directory. It
requires one release version across the public packages and rejects any private
package left in an archive's dependencies. It builds every package with all
features, runs an external parser/checker/ownership consumer, builds the
parser-only embedding surface and builds wasm in parser and checker modes. It
then builds `tsrust` with default features, as `cargo install` does, and runs
it: `--version` must print the pin's `Version 7.1.0-dev`, a one-file compile with
`--outDir` must emit exactly the expected JavaScript, and a type error must
report TS2322 with exit status 2. The temporary workspace contains no
`upstream/`, tools or source checkout. Its only overrides are
`[patch.crates-io]` entries pointing to extracted archives. Cargo metadata
rejects path dependencies outside that directory; the root lockfile seeds
external versions, which are checked against the repository lockfile after
resolution. No temporary registry is needed.

The archives include Cargo-generated lockfiles and normalized metadata. The
helper retains the extracted tree for inspection and writes detailed logs,
file lists, archive hashes and `verified.json` under `target/publishing-prep/`.
A failed rerun deletes the earlier passing summary. This verifies package
self-containment; it does not certify that dependencies already exist in the
public registry. A registry `cargo publish` follows the dependency order below.
A `0.0.0` name reservation does not satisfy a dependency on a real version.

## Content and private harnesses

Package manifests include source, README, license and notice files explicitly;
`tsr_bundled` also includes its pinned assets and `tsr_core`, `tsr_glob`, `tsr_json` and `tsr_locale` their Go BSD license.
Repository examples, integration tests, capture archives and fixture corpora
are excluded. Each package has a description and the current repository URL.
Tests embedded under `src` can still reference repository fixtures: running a
package's own tests outside the repository is not a supported promise here.

The include/path audit found external reads only in test modules after moving
the bundled assets. These cover the AST, binder, scanner, checker, compiler,
node-builder, printer, module, API, declaration-transformer and command-line
help fixtures. Internal `#[path]` source splits remain inside each package. No
public package has a build script; the native Node addon build script belongs to
a private package.

Development dependencies that only repository tests use are path-only, and
Cargo omits them from normalized archives: `tsr_compiler`'s private
`s08_relater_prototype` and `tsr_contentmappertest` and its `tsr_incremental`
and `tsr_project`, `tsr_incremental`'s `tsr_contentmappertest`, and the
encoder's `tsr_parser`. The public ones among them would otherwise close a
development-dependency cycle (`tsr_project` and `tsr_incremental` depend on the
compiler, the parser's tests on the encoder). Other public dev dependencies
retain their versions; repository tests retain the same local dependencies.

The former `tsr_wasm/corpus` feature is private capture functionality. It moves
to `s10_wasm_corpus`, which calls the shipped `tsr_embed` API and the unchanged
private `s10_corpus` walker. `scripts/s10_build.py --corpus` selects that package;
its output directory, `observe_corpus` ABI, error tag and JSON contract stay the
same. It no longer exports unrelated parser/checker classes and therefore no
longer imports the class throw shim. Public `tsr_wasm` parser/checker features
and exports stay unchanged. Old captures retain their own archived ABI wrappers.

## Releases

The project branding is **tsr**. `tsr_embed` is the application entry point,
`tsr` re-exports every public library except `tsr_wasm` as a module, and the
command line is the `tsrust` package, which installs the `tsrust` binary. All
package READMEs state the Rust 1.96 minimum, and package metadata includes search
keywords and the compiler category.

28 libraries were published at `0.1.0` on 2026-09-20 from tag `v0.1.0`
(`039226a`). The public packages are released together at one version; the next
release is `0.2.0`, and [docs/RELEASE-0.2.0.md](../../docs/RELEASE-0.2.0.md) is
its checklist. crates.io's
[limiter](https://github.com/rust-lang/crates.io/blob/main/src/rate_limiter.rs)
allows five new crates in a burst and replenishes one slot per ten minutes;
updates to existing crates have a separate allowance (a burst of 30, then one
per minute). Server configuration and account overrides can change these;
check the actual response rather than treating an estimate as a schedule.

## Dependency-first release order

This order includes retained internal development dependencies as well as normal,
build and optional dependencies. `tsr` depends on every library it re-exports and
goes last:

1. `tsr_fswatch`
2. `tsr_jsstring`
3. `tsr_locale`
4. `tsr_arena`
5. `tsr_core`
6. `tsr_glob`
7. `tsr_jsnum`
8. `tsr_semver`
9. `tsr_json`
10. `tsr_tspath`
11. `tsr_diagnostics`
12. `tsr_jsonrpc`
13. `tsr_sourcemap`
14. `tsr_vfs`
15. `tsr_ast`
16. `tsr_bundled`
17. `tsr_ipc`
18. `tsr_encoder`
19. `tsr_nodebuilder`
20. `tsr_scanner`
21. `tsr_parser`
22. `tsr_astnav`
23. `tsr_binder`
24. `tsr_tsoptions`
25. `tsr_contentmapper`
26. `tsr_format`
27. `tsr_module`
28. `tsr_printer`
29. `tsr_pseudochecker`
30. `tsr_checker`
31. `tsr_transformers`
32. `tsr_compiler`
33. `tsr_tracing`
34. `tsr_embed`
35. `tsr_incremental`
36. `tsr_project`
37. `tsr_transpile`
38. `tsr_api`
39. `tsr_tsc`
40. `tsr_wasm`
41. `tsr_build`
42. `tsr_execute`
43. `tsrust`
44. `tsr`

## Package policy

| Package | crates.io | Reason |
| --- | --- | --- |
| `phase1_config` | Private | Repository-only benchmark, experiment, test or capture tool |
| `phase1_filesystem` | Private | Repository-only benchmark, experiment, test or capture tool |
| `phase1_harness` | Private | Repository-only benchmark, experiment, test or capture tool |
| `phase1_leaves` | Private | Repository-only Phase 1 foundation test harness |
| `phase1_mutants` | Private | Repository-only Phase 1 mutation-witness tool |
| `phase1_mutation_driver` | Private | Repository-only Phase 1 mutation-witness tool |
| `phase1_mutation_splicer` | Private | Repository-only Phase 1 mutation-witness tool |
| `phase1_syntax` | Private | Repository-only Phase 1 syntax test harness |
| `phase4_tsctests` | Private | Repository-only Phase 4 command-line test harness |
| `s08_relater_prototype` | Private | Repository-only benchmark, experiment, test or capture tool |
| `s09_format_harness` | Private | Repository-only benchmark, experiment, test or capture tool |
| `s10_corpus` | Private | Repository-only benchmark, experiment, test or capture tool |
| `s10_rust_consumer` | Private | Repository-only benchmark, experiment, test or capture tool |
| `s10_wasm_corpus` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr` | Public | Facade re-exporting the public libraries; a plain re-export until the embedding API settles |
| `tsr_api` | Public | Rust library and its public dependency closure |
| `tsr_arena` | Public | Rust library and its public dependency closure |
| `tsr_ast` | Public | Rust library and its public dependency closure |
| `tsr_astnav` | Public | Rust library and its public dependency closure |
| `tsr_bench` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_binder` | Public | Rust library and its public dependency closure |
| `tsr_build` | Public | Rust library in the tsrust command line's public dependency closure |
| `tsr_bundled` | Public | Rust library and its public dependency closure |
| `tsr_checker` | Public | Rust library and its public dependency closure |
| `tsr_compiler` | Public | Rust library and its public dependency closure |
| `tsr_contentmapper` | Public | Rust library and its public dependency closure |
| `tsr_contentmappertest` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_core` | Public | Rust library and its public dependency closure |
| `tsr_cpu_profile` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_diagnostics` | Public | Rust library and its public dependency closure |
| `tsr_embed` | Public | Rust library and its public dependency closure |
| `tsr_encoder` | Public | Rust library and its public dependency closure |
| `tsr_execute` | Public | Rust library in the tsrust command line's public dependency closure |
| `tsr_format` | Public | Rust library and its public dependency closure |
| `tsr_fswatch` | Public | Rust library in the tsrust command line's public dependency closure |
| `tsr_glob` | Public | Rust library and its public dependency closure |
| `tsr_incremental` | Public | Rust library and its public dependency closure |
| `tsr_ipc` | Public | Rust library and its public dependency closure |
| `tsr_jsnum` | Public | Rust library and its public dependency closure |
| `tsr_json` | Public | Rust library and its public dependency closure |
| `tsr_jsonrpc` | Public | Rust library and its public dependency closure |
| `tsr_jsstring` | Public | Rust library and its public dependency closure |
| `tsr_locale` | Public | Rust library and its public dependency closure |
| `tsr_memory_profile` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_module` | Public | Rust library and its public dependency closure |
| `tsr_node` | Private | Node addon distributed separately; not a Rust library package |
| `tsr_nodebuilder` | Public | Rust library and its public dependency closure |
| `tsr_parser` | Public | Rust library and its public dependency closure |
| `tsr_printer` | Public | Rust library and its public dependency closure |
| `tsr_project` | Public | Rust library and its public dependency closure |
| `tsr_pseudochecker` | Public | Rust library and its public dependency closure |
| `tsr_s07_access_trace_native_verify` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_s07_bis_access_trace` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_s07_bis_owner_census` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_s07_bis_phases` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_s07_storage_pilot` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_scanner` | Public | Rust library and its public dependency closure |
| `tsr_semver` | Public | Rust library and its public dependency closure |
| `tsr_sourcemap` | Public | Rust library and its public dependency closure |
| `tsr_testhost` | Private | Repository-only benchmark, experiment, test or capture tool |
| `tsr_testrunner` | Private | Repository-only port of the pinned compiler and transpile test runners (docs/EVIDENCE-plan.md) |
| `tsr_tracing` | Public | Rust library in the tsrust command line's public dependency closure |
| `tsr_transformers` | Public | Rust library and its public dependency closure |
| `tsr_transpile` | Public | Rust library and its public dependency closure |
| `tsr_tsc` | Public | Rust library in the tsrust command line's public dependency closure |
| `tsr_tsoptions` | Public | Rust library and its public dependency closure |
| `tsr_tspath` | Public | Rust library and its public dependency closure |
| `tsr_vfs` | Public | Rust library and its public dependency closure |
| `tsr_wasm` | Public | Rust library and its public dependency closure |
| `tsrust` | Public | The compiler command line; installs the tsrust binary |
| `xtask` | Private | Repository-only benchmark, experiment, test or capture tool |

See [the validation record](../../docs/CRATE-PUBLISHING-PREP.md) for the
measurements of the first preparation.
