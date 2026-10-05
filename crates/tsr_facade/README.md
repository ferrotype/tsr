# tsr

TypeScript's native compiler in Rust: the facade over the `tsr_*` library
crates. tsr is a port of TypeScript 7's Go compiler, held to the pinned
upstream test suites.

This crate re-exports each public library as a module named after the package
without its `tsr_` prefix: `tsr::parser` is `tsr_parser`, `tsr::checker` is
`tsr_checker`, `tsr::embed` is `tsr_embed`. It adds no API of its own and stays
a plain re-export until the embedding API settles (Phase 7). `tsr_wasm` and the
command-line layers are not re-exported.

```toml
[dependencies]
tsr = "0.3.0"
```

For application integration, start with `tsr::embed`
([`tsr_embed`](https://github.com/ferrotype/tsr/tree/main/crates/tsr_embed)):
parsing, owned program sessions, diagnostics and scoped type queries. To
compile TypeScript from a shell, install the command line instead:
`cargo install tsrust`.

This project is under development; the API and supported compiler behavior are
not stable. See the repository status and sprint records for current coverage.

Requires **Rust 1.96 or newer**.

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE) for
license and attribution information.
