# tsr_tracing

Compiler tracing for tsr (`--generateTrace`): the pin's `tracing` package.
A trace session receives checker events through the production trace boundary
and writes the files `tsc` writes (`trace.json`, one `types_<n>.json` per
checker and `legend.json`), resolving type identities while the checker is
still held.

Part of [tsr](https://github.com/ferrotype/tsr), a Rust port of
the TypeScript compiler. This project is under development; the API and supported
compiler behavior are not stable. See the repository status and sprint records
for current coverage.

Requires **Rust 1.96 or newer**.

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE) for
license and attribution information.

To compile TypeScript from a shell, install the command line instead:
`cargo install tsrust`. For application integration, start with
[`tsr_embed`](https://github.com/ferrotype/tsr/tree/main/crates/tsr_embed).
