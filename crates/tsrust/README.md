# tsrust

`tsrust` is the command line of [tsr](https://github.com/ferrotype/tsr),
TypeScript's native compiler in Rust: a port of TypeScript 7's Go compiler
(the pin's `cmd/tsc`) that is held to the pin's own test suites. It takes the
same arguments as `tsc` and produces the same console output, exit statuses,
emitted files and `.tsbuildinfo`.

## Install

```sh
cargo install tsrust
```

This builds and installs the `tsrust` binary. It requires **Rust 1.96 or
newer**. The standard library declaration files are compiled into the binary,
so it needs no Node.js installation and no `node_modules/typescript`.

To build from a checkout of the repository instead:

```sh
git clone --recurse-submodules https://github.com/ferrotype/tsr
cd tsr
cargo build --release -p tsrust --bin tsrust
./target/release/tsrust --version
```

`--version` prints the version of the pinned TypeScript compiler
(`Version 7.1.0-dev`), not the crate version.

## Use

The arguments are those of `tsc`:

```sh
tsrust -p path/to/tsconfig.json
tsrust app.ts util.ts --target es2022 --module esnext --outDir dist
tsrust --incremental
tsrust --watch
tsrust -b --watch
tsrust -b --clean
```

A project or a file list, `--incremental`, `--watch`, and `-b` with clean, dry,
force, verbose and build-watch are supported, as are `--help`, `--init`,
`--showConfig`, `--listFiles`, `--diagnostics`, `--extendedDiagnostics` and
`--generateTrace`.

Supported targets: macOS arm64 and x64, Linux x64 and arm64 (glibc). Other
platforms, including Windows, are not supported yet.

There is no language server yet: `--lsp` and `--api` report that they are not
implemented. The language service and LSP server are Phase 5 of the
[plan](https://github.com/ferrotype/tsr/blob/main/PLAN.md), the JS API server
Phase 6.

## Status

This project is under development. On the pin's command-line scenarios
(`tsc`, `-b`, `--watch`, `--incremental`), 514 of 516 match the committed
baselines; the [status page](https://ferrotype.github.io/tsr/) has the current
numbers for every suite.

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE) for
license and attribution information.
