# Release 0.2.0

The checklist for publishing tsr 0.2.0 to crates.io: 45 packages from one
commit, all at version `0.2.0`. They are 43 libraries, the compiler command line
`tsrust` (it installs the `tsrust` binary) and the facade `tsr`. Publishing is
manual and irreversible: a published version can be yanked but never replaced
or reused.

Why 0.2.0 and not 0.1.0: 28 of these libraries were published at `0.1.0` on
2026-09-20 from tag `v0.1.0` (`039226a`). crates.io never accepts a version
twice, and the current sources are not compatible with those releases (for
example, `tsr_core::PathMappings` changed type), so every public package moves to
`0.2.0` together. `tools/packaging/README.md` has the policy, the dependency order
and the package table this checklist is built from.

## Registry state (checked 2026-10-03)

All existing names below are owned by the crates.io account `iantocristian`.

- 28 names have a real `0.1.0`: `tsr_api`, `tsr_arena`, `tsr_ast`, `tsr_astnav`,
  `tsr_binder`, `tsr_bundled`, `tsr_checker`, `tsr_compiler`, `tsr_core`,
  `tsr_diagnostics`, `tsr_embed`, `tsr_encoder`, `tsr_format`, `tsr_jsnum`,
  `tsr_jsstring`, `tsr_module`, `tsr_nodebuilder`, `tsr_parser`, `tsr_printer`,
  `tsr_project`, `tsr_pseudochecker`, `tsr_scanner`, `tsr_semver`, `tsr_tspath`,
  `tsr_tsoptions`, `tsr_transformers`, `tsr_vfs`, `tsr_wasm`.
- 5 names hold only a `0.0.0` placeholder and get their first real version:
  `tsr`, `tsrust`, `tsr_glob`, `tsr_json`, `tsr_locale`.
- 11 names are new to crates.io: `tsr_fswatch`, `tsr_jsonrpc`, `tsr_sourcemap`,
  `tsr_ipc`, `tsr_contentmapper`, `tsr_tracing`, `tsr_incremental`,
  `tsr_transpile`, `tsr_tsc`, `tsr_build`, `tsr_execute`.
- `tsr_lsproto` is newly added by Phase 5; check its registry availability and ownership before publishing.
- Not part of this release, left at their `0.0.0` placeholders: `jscout`,
  `typescout`, `tsr_collections`.

Recheck this the day of the release, for example
`curl -s https://crates.io/api/v1/crates/tsr_tsc` for a new name (a 404 body
means it is still free) and `/api/v1/crates/<name>/owners` for an existing one.

## Prerequisites

1. A crates.io API token for `iantocristian`, entered with `cargo login`. If
   the token is scoped, it needs both `publish-new` and `publish-update` and
   must cover every name above, including the eleven names above and `tsr_lsproto`.
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

   The verifier must end with `Verified 45 Cargo archives at 0.2.0`. It builds
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
cargo publish -p tsr_fswatch --locked          # new name
cargo publish -p tsr_jsstring --locked
cargo publish -p tsr_locale --locked           # placeholder 0.0.0 only
cargo publish -p tsr_arena --locked
cargo publish -p tsr_core --locked
cargo publish -p tsr_glob --locked             # placeholder 0.0.0 only
cargo publish -p tsr_jsnum --locked
cargo publish -p tsr_semver --locked
cargo publish -p tsr_json --locked             # placeholder 0.0.0 only
cargo publish -p tsr_tspath --locked
cargo publish -p tsr_diagnostics --locked
cargo publish -p tsr_jsonrpc --locked          # new name
cargo publish -p tsr_lsproto --locked          # Phase 5; verify availability first
cargo publish -p tsr_sourcemap --locked        # new name
cargo publish -p tsr_vfs --locked
cargo publish -p tsr_ast --locked
cargo publish -p tsr_bundled --locked
cargo publish -p tsr_ipc --locked              # new name
cargo publish -p tsr_encoder --locked
cargo publish -p tsr_nodebuilder --locked
cargo publish -p tsr_scanner --locked
cargo publish -p tsr_parser --locked
cargo publish -p tsr_astnav --locked
cargo publish -p tsr_binder --locked
cargo publish -p tsr_tsoptions --locked
cargo publish -p tsr_contentmapper --locked    # new name
cargo publish -p tsr_format --locked
cargo publish -p tsr_module --locked
cargo publish -p tsr_printer --locked
cargo publish -p tsr_pseudochecker --locked
cargo publish -p tsr_checker --locked
cargo publish -p tsr_transformers --locked
cargo publish -p tsr_compiler --locked
cargo publish -p tsr_tracing --locked          # new name
cargo publish -p tsr_embed --locked
cargo publish -p tsr_incremental --locked      # new name
cargo publish -p tsr_project --locked
cargo publish -p tsr_transpile --locked        # new name
cargo publish -p tsr_api --locked
cargo publish -p tsr_tsc --locked              # new name
cargo publish -p tsr_wasm --locked
cargo publish -p tsr_build --locked            # new name
cargo publish -p tsr_execute --locked          # new name
cargo publish -p tsrust --locked               # placeholder 0.0.0 only
cargo publish -p tsr --locked                  # placeholder 0.0.0 only
```

`tsr` goes last because it depends on every library it re-exports; nothing
depends on it. Do not use `cargo publish --workspace` for this release: it would
stop at the new-crate rate limit in the middle of the set.

### Rate limits

crates.io limits each account separately for new crates and for new versions of
existing crates
([`rate_limiter.rs`](https://github.com/rust-lang/crates.io/blob/main/src/rate_limiter.rs)):

- New crates: a burst of 5, then one more every 10 minutes. This list has 11
  new names (marked above). The first five (lines 1, 12, 13, 17, 25) use the
  burst; `tsr_tracing` (line 33) is the sixth and needs a refilled slot. With a
  full bucket at the start, the eleventh new name is publishable 60 minutes
  after the first at the earliest. The verification builds between the lines
  take time too, so the limit may never be hit; if it is, `cargo publish` fails
  with HTTP 429 and "You have published too many new crates in a short period
  of time", and the message says when to retry. Wait, then rerun the same line.
- Updates (the other 33 lines): a burst of 30, then one per minute. Rerun after
  a minute if the 31st to 33rd update is refused.

To raise the limit before the release day instead, email help@crates.io from the
account's address with the account name, the eleven new crate names and the
reason (the first release of a workspace that publishes its crates together).

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
   `cargo new tsr-check && cd tsr-check && cargo add tsr@0.2.0 && cargo build`.
2. Tag the release commit and push the tag:

   ```sh
   git tag -a v0.2.0 -m "tsr 0.2.0"
   git push origin v0.2.0
   ```

3. Create the GitHub release from the tag, for example
   `gh release create v0.2.0 --title "tsr 0.2.0" --notes-file <notes>`. The notes
   should say what is in it (the `tsrust` command line, `cargo install tsrust`,
   the 43 libraries and the `tsr` facade, the supported targets) and what is
   not (below), and link the status page.
4. Trusted publishing from GitHub can be configured per crate after this first
   manual publish: on crates.io, each crate's Settings, Trusted Publishing, with
   the repository `ferrotype/tsr` and the workflow file that will publish. Later
   releases can then publish from a workflow without a long-lived token.

## Not in 0.2.0

- The language server. `tsrust --lsp` reports that it is not implemented; the
  language service, project system and LSP server are Phase 5.
- The JS API server (`--api`), which the TypeScript npm package speaks: Phase 6.
- An npm package. The names are reserved under the `tsrust` npm organisation;
  nothing is published to npm.
- A settled embedding or WebAssembly API. `tsr_embed`, `tsr_wasm` and the `tsr`
  facade are published as prototypes; `tsr` is a plain re-export of the
  libraries until the embedding API settles in Phase 7.
- Platforms other than macOS arm64 and x64 and Linux x64 and arm64 (glibc).
  `tsrust` does not build on Windows.
- The Node addon (`tsr_node`), which is not a crate and stays private.

One difference from a repository build: `cargo install tsrust` builds with
Cargo's default release profile. The workspace's release profile (fat LTO, one
codegen unit) applies only to builds inside the repository, so an installed
`tsrust` may be somewhat slower than the measured repository build.
