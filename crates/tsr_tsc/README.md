# tsr_tsc

The shared command-line layer of tsr: the pin's `execute/tsc` package, and
the `execute` watcher and watch manager. It holds the command-line system and
exit statuses, diagnostic, emit and statistics reporting, `--help` and `--init`
output, trace sessions for `--generateTrace`, and the watch sessions that both
the compilation driver (`tsr_execute`) and the build orchestrator (`tsr_build`)
use. Keeping it separate avoids a driver/build dependency cycle.

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
