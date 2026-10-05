# Release 0.3.0

The checklist for publishing tsr 0.3.0 to crates.io: 48 packages from one
commit, all at version `0.3.0`. They are 46 libraries, the compiler command line
`tsrust` (it installs the `tsrust` binary) and the facade `tsr`. Publishing is
manual and irreversible: a published version can be yanked but never replaced
or reused.

Why every package moves, including the ones whose sources did not change since
0.2.0: Cargo treats a `0.x` minor bump as incompatible, so `^0.2.0` never
resolves to `0.3.0`. A package left at `0.2.0` would keep depending on the
`0.2` versions of its siblings, and a build mixing it with `0.3.0` packages
would carry two copies of `tsr_core` and the other shared libraries, whose types
do not unify. Of the 48 packages, 21 changed since the 0.2.0 commit (`d0faa28`),
4 of them new; 25 more depend on a changed package; only `tsr_fswatch` and
`tsr_locale` would be unaffected, and they move with the rest to keep one
release version (`scripts/package_verify.py` requires it).

## Registry state (checked 2026-10-05)

All existing names below are owned by the crates.io account `iantocristian`.

- 44 names have `0.2.0` as their newest version: every package of
  [RELEASE-0.2.0.md](RELEASE-0.2.0.md).
- 4 names are new to crates.io: `tsr_lsproto`, `tsr_autoimport`, `tsr_ls`,
  `tsr_lsp`.

Recheck this the day of the release, for example
`curl -s https://crates.io/api/v1/crates/tsr_ls` for a new name (a 404 body
means it is still free) and `/api/v1/crates/<name>/owners` for an existing one.

## Prerequisites

1. A crates.io API token for `iantocristian`, entered with `cargo login`. If
   the token is scoped, it needs both `publish-new` and `publish-update` and
   must cover every name above, including the four new ones.
2. A clean `main` at the release commit: `git switch main && git pull`, then
   `git status` shows nothing to commit inside `crates/`, and CI is green on that
   commit. `cargo publish` refuses uncommitted changes in a package directory;
   do not pass `--allow-dirty`.
3. On that commit, with the pinned toolchain (`rust-toolchain.toml`) and the
   upstream submodule initialized:

   ```sh
   python3 scripts/package_assets.py --check
   python3 scripts/package_verify.py
   ```

   The verifier must end with `Verified 48 Cargo archives at 0.3.0`. It builds
   every archive in an isolated workspace, runs the embedding consumer, builds
   wasm, then builds `tsrust` from the archives and runs it (`--version`, a
   one-file compile with `--outDir`, a TS2322 type error).
4. Run every command below from the repository root.

## Publish

One command per line, in this order: each package's dependencies, development
dependencies included, come before it. `cargo publish` packages the crate,
builds it against crates.io, uploads it and waits until the index serves the new
version, so the next line can resolve it. `--locked` makes it fail rather than
update `Cargo.lock`. If a line fails, fix the cause and rerun that same line;
the lines already published stay published, and the order is safe to resume.

```sh
cargo publish -p tsr_fswatch --locked
cargo publish -p tsr_jsstring --locked
cargo publish -p tsr_locale --locked
cargo publish -p tsr_arena --locked
cargo publish -p tsr_core --locked
cargo publish -p tsr_glob --locked
cargo publish -p tsr_jsnum --locked
cargo publish -p tsr_semver --locked
cargo publish -p tsr_json --locked
cargo publish -p tsr_tspath --locked
cargo publish -p tsr_diagnostics --locked
cargo publish -p tsr_jsonrpc --locked
cargo publish -p tsr_lsproto --locked          # new name
cargo publish -p tsr_sourcemap --locked
cargo publish -p tsr_vfs --locked
cargo publish -p tsr_ast --locked
cargo publish -p tsr_bundled --locked
cargo publish -p tsr_ipc --locked
cargo publish -p tsr_encoder --locked
cargo publish -p tsr_nodebuilder --locked
cargo publish -p tsr_scanner --locked
cargo publish -p tsr_parser --locked
cargo publish -p tsr_astnav --locked
cargo publish -p tsr_binder --locked
cargo publish -p tsr_tsoptions --locked
cargo publish -p tsr_contentmapper --locked
cargo publish -p tsr_format --locked
cargo publish -p tsr_module --locked
cargo publish -p tsr_printer --locked
cargo publish -p tsr_pseudochecker --locked
cargo publish -p tsr_checker --locked
cargo publish -p tsr_transformers --locked
cargo publish -p tsr_compiler --locked
cargo publish -p tsr_tracing --locked
cargo publish -p tsr_embed --locked
cargo publish -p tsr_incremental --locked
cargo publish -p tsr_autoimport --locked       # new name
cargo publish -p tsr_project --locked
cargo publish -p tsr_ls --locked               # new name
cargo publish -p tsr_lsp --locked              # new name
cargo publish -p tsr_transpile --locked
cargo publish -p tsr_api --locked
cargo publish -p tsr_tsc --locked
cargo publish -p tsr_wasm --locked
cargo publish -p tsr_build --locked
cargo publish -p tsr_execute --locked
cargo publish -p tsrust --locked
cargo publish -p tsr --locked
```

`tsr` goes last because it depends on every library it re-exports; nothing
depends on it. The order is the dependency order of
[tools/packaging/README.md](../tools/packaging/README.md).

### Rate limits

crates.io limits each account separately for new crates and for new versions of
existing crates
([`rate_limiter.rs`](https://github.com/rust-lang/crates.io/blob/main/src/rate_limiter.rs)):

- New crates: a burst of 5, then one more every 10 minutes. The four new names
  fit in the burst.
- Updates (the other 44 lines): a burst of 30, then one per minute. The builds
  between lines take time, so the limit may never be hit; if an update is
  refused with HTTP 429, wait a minute and rerun the same line.

## After publishing

1. Smoke test from crates.io, in a directory outside the repository:

   ```sh
   cargo install tsrust --locked
   tsrust --version        # Version 7.1.0-dev
   printf 'declare const x: number;\nconst y: number = x + 1;\n' > hello.ts
   tsrust hello.ts --outDir out && cat out/hello.js
   ```

   Expect exit status 0 and `out/hello.js` containing `"use strict";` and
   `const y = x + 1;`. Optionally check the facade resolves:
   `cargo new tsr-check && cd tsr-check && cargo add tsr@0.3.0 && cargo build`.
2. Tag the release commit and push the tag:

   ```sh
   git tag -a v0.3.0 -m "tsr 0.3.0"
   git push origin v0.3.0
   ```

3. Create the GitHub release from the tag, for example
   `gh release create v0.3.0 --title "tsr 0.3.0" --notes-file <notes>`. The
   notes should say what is new (below) and what is not in it, and link the
   status page.

## New in 0.3.0

- The Phase 5 language-service crates: `tsr_lsproto` (protocol types and wire
  codecs), `tsr_lsp` (server session and project dispatch), `tsr_ls` (language
  service queries) and `tsr_autoimport` (auto-import indexes and import edits).
  `tsrust --lsp` starts the server; Phase 5 is still in progress
  ([PHASE5-plan.md](PHASE5-plan.md)). The facade re-exports the protocol
  library.

## Not in 0.3.0

- The JS API server (`--api`), which the TypeScript npm package speaks: Phase 6.
- An npm package. The names are reserved under the `tsrust` npm organisation;
  nothing is published to npm.
- A settled embedding or WebAssembly API. `tsr_embed`, `tsr_wasm` and the `tsr`
  facade are published as prototypes; `tsr` is a plain re-export of the
  libraries until the embedding API settles in Phase 7.
- Platforms other than macOS arm64 and x64 and Linux x64 and arm64 (glibc).
  `tsrust` does not build on Windows.
- The Node addon (`tsr_node`), which is not a crate and stays private.

As with 0.2.0, `cargo install tsrust` builds with Cargo's default release
profile, not the workspace's (fat LTO, one codegen unit), so an installed
`tsrust` may be somewhat slower than the measured repository build.
