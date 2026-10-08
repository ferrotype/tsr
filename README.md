<p align="center">
  <img src="assets/tsr-icon.png" alt="tsr" width="280">
</p>

<h1 align="center">tsr</h1>

<p align="center"><b>TypeScript's native compiler, in Rust.</b><br>
A port of TypeScript 7's Go compiler (Corsa) that passes the pin's own test suites against the pin's own baselines.</p>

<p align="center">
  <a href="https://ferrotype.github.io/tsr/">Status page</a> ·
  <a href="PLAN.md">The plan</a> ·
  <a href="docs/PHASE7-plan.md">Phase 7</a> ·
  <a href="docs/EVIDENCE-plan.md">How parity is measured</a> ·
  <a href="docs/adr/README.md">Architecture decisions</a>
</p>

---

## What it is

TypeScript 7 moved its compiler to Go (microsoft/TypeScript, module `tsc/`). `tsr` ports that compiler to Rust and holds it to one pinned upstream commit, a submodule under `upstream/`, which supplies the test cases, the baselines, the lib files, the schemas and the clients. Every suite the pin runs against its Go binary runs in CI against the Rust one; `status/parity/<suite>.json` names every sub-test that differs, with a reason, and CI fails when that set changes.

What you get:

- **`tsrust`**: the compiler command line with `tsc`'s arguments (a project or a file list, `--incremental`, `--watch`, `-b` with clean, dry, force and build-watch), the same console output, exit statuses, emitted files and `.tsbuildinfo`; `tsrust --lsp --stdio`, the language server the pin's VS Code extension speaks to; `tsrust --api`, the JS API server the pin's `packages/typescript` client spawns.
- **A library workspace** of `tsr_*` crates: scanner, parser, binder, checker, transformers, printer, source maps, module resolution, program loading, incremental build, the build orchestrator, the file watcher, the language service, the project system, the LSP and API servers. Ownership is modelled on arenas and checked identities instead of a garbage collector, so a checker can be embedded, retained and dropped from Rust (`tsr_embed`), or run as a bare `wasm32` instance (`tsr_wasm`).

Why: type-aware tooling in Rust (a usage index, a bundler that knows types, an editor service you can link) needs a checker that answers exactly what TypeScript answers. The Go compiler is that answer; porting it and keeping it under the pin's tests is how the port stays one.

## Status

Phases are sequenced by dependency, not by calendar (ADR 0005); each closes when the pinned suites it covers pass.

| Phase | Scope | State |
|---|---|---|
| 0 | Contracts, scanner, parser, encoder, the arena and ownership model, the test-host transport | Done |
| 1 | Core, collections, text, JSON, paths, virtual file systems, config and command-line parsing, binder, module resolution | Done |
| 2 | The checker | Done ([records](docs/PHASE2-C7.md)) |
| 3 | Emit: transformers, printer, source maps, declaration emit, transpile | Done ([record](docs/PHASE3-T8.md)) |
| 4 | Programs, command line, build orchestrator, watcher, watch mode, tracing: `tsrust` | Done ([record](docs/PHASE4-X6.md)) |
| 5 | Language service, project system, LSP server | Done ([record](docs/PHASE5-L7.md)) |
| 6 | JS API server | Done ([record](docs/PHASE6-A5.md)) |
| 7 | Hardening, performance acceptance, WebAssembly and embedding acceptance, release and cut-over | Proposed ([plan](docs/PHASE7-plan.md)) |

**Parity with the pin** (`status/parity/`, recomputed by CI on every pull request):

| Suite | Variants | Failing | Approved |
|---|---:|---:|---:|
| `compiler` (compiler and conformance tests, the pin's single-threaded mode) | 13,432 | 0 | 0 |
| `compiler-concurrent` (the production checker pool) | 13,432 | 0 | 0 |
| `transpile` | 28 | 0 | 0 |
| `tsc` (command line, `-b`, `--watch`, `--incremental` scenarios) | 516 | 2 | 2 |
| `fourslash` (the language service, through the pin's own harness) | 4,534 | 0 | 0 |
| `lsp` (the pin's LSP and project suites) | 13 | 0 | 0 |
| `jsapi` (the pin's synchronous and asynchronous client suites, 706 cases) | 12 | 0 | 0 |

The two approved `tsc` entries are trace-event order in `generateTrace` output; everything else the pin checks, the Rust binary produces byte for byte.

**Performance** (`status/perf/`, Rust over Go on the owner's host, 2026-10-08):

| Workload | Rust / Go |
|---|---:|
| Parse and bind of the VS Code tree, one worker / eight workers | 1.10 / 1.26 |
| Parse and bind, peak memory | 0.67 |
| Checker query workload, elapsed / per-type memory | 1.63 / 0.81 |
| LSP: first diagnostics, completion, hover, references, rename | 1.26, 2.33, 1.82, 1.55, 1.30 |

The targets are 1.0 for every time ratio and 0.70 for memory, on the five TypeScript benchmarking scenarios at 2, 4 and 8 checkers; reaching them, or deciding otherwise with the attribution in hand, is Phase 7's.

**Port coverage:** 8,464 of the pin's 11,485 functions carry a `// port:` marker to their Rust counterpart (73.7%), and 219 of 456 files are ported whole; the language service and project system pass their suites with the fewest markers. `cargo xtask validate` rejects a marker that names nothing in the pinned inventory; the [status page](https://ferrotype.github.io/tsr/) breaks coverage down by package.

## Try it

From crates.io (Rust 1.96 or newer):

```bash
cargo install tsrust
tsrust --version
```

From source, with the pinned toolchain (`rust-toolchain.toml` selects it) and the upstream submodule for the test data:

```bash
git clone --recurse-submodules https://github.com/ferrotype/tsr
cd tsr
cargo build --release -p tsrust --bin tsrust
```

```bash
./target/release/tsrust -p path/to/tsconfig.json
./target/release/tsrust app.ts util.ts --target es2022 --module esnext
./target/release/tsrust -b --watch
./target/release/tsrust --lsp --stdio
```

Targets: macOS arm64 and x64, Linux x64 and arm64 (glibc 2.28 or newer). CI runs the suites on macOS arm64 and Linux x64 today; the other two builds return in Phase 7.

Libraries: `tsr` on crates.io re-exports every library crate as a module (`tsr::parser`, `tsr::checker`); `tsr_embed` is the embedding API with an in-memory host; `tsr_wasm` the bare WebAssembly build. Their full-corpus acceptance is Phase 7's; until then they are what the S10 record describes.

## Repository map

| Path | Contents |
|---|---|
| `crates/` | 53 crates: 48 on crates.io (46 `tsr_*` libraries, the `tsr` facade and `tsrust`), 5 repository-only |
| `upstream/` | The pinned microsoft/TypeScript checkout (submodule) |
| `status/parity/`, `status/perf/` | Expectation files per suite; performance runs per workload |
| `scripts/parity.py`, `scripts/perf.py` | The suite driver and the performance recorder |
| `tools/` | Harnesses: the `tsc` scenario runner, the fourslash and client harnesses, the benchmark workloads, the WebAssembly and consumer adapters |
| `xtask/` | `cargo xtask gen` (code from the pinned schemas), `validate` (ledger and markers), `status` (the rendered page) |
| `docs/` | The per-phase plans and records, [ADRs](docs/adr/README.md), [design notes](docs/design/) |
| `PLAN.md` | The plan: goals, exit criteria, architecture decisions, phases |
| `PORTS.toml`, `data/go-functions.tsv` | The upstream file ledger and function inventory |

Working in the repository: [CLAUDE.md](CLAUDE.md) has the commands; [AGENTS.md](AGENTS.md) the implementation rules.

## License

Apache License, Version 2.0 ([LICENSE](LICENSE)). tsr is a port of Microsoft's TypeScript compiler, itself Apache-2.0; [NOTICE](NOTICE) carries the attribution. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in tsr by you, as defined in the Apache-2.0 license, shall be licensed as above, without any additional terms or conditions.
