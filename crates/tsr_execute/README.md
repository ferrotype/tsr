# tsr_execute

The command-line driver of tsr: the pin's `execute` package (`tsc.go`).
`command_line` parses `tsc` arguments and runs a project or file-list
compilation, `--incremental` and `--watch`, and hands `-b` to `tsr_build`. The
`tsrust` binary is a thin process boundary over this crate.

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
