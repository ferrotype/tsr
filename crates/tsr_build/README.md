# tsr_build

Project-reference build orchestration for tsr (`tsc -b`): the pin's
`execute/build` package. It builds the project graph, checks each project's
up-to-date status, builds projects in dependency order with bounded
parallelism, and supports clean, dry, force, verbose and build-watch, with the
ordered transcript and cross-project error summary `tsc` prints.

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
