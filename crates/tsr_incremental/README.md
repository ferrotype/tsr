# tsr_incremental

The incremental program and its build info (`.tsbuildinfo`) for tsr: the
pin's `execute/incremental` package. A program built with `incremental` or
`composite` records each file's version, signature, references, diagnostics
and pending emit in its build info, and a later build reads it back to check
and emit only what changed.

Part of [tsr](https://github.com/ferrotype/tsr), a Rust port of
the TypeScript compiler. This project is under development; the API and supported
compiler behavior are not stable. See the repository status and sprint records
for current coverage.

Requires **Rust 1.96 or newer**.

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE) for
license and attribution information.

For application integration, start with
[`tsr_embed`](https://github.com/ferrotype/tsr/tree/main/crates/tsr_embed).
