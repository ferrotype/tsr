# tsr_fswatch

Native filesystem watching for tsr: the pin's `fswatch` package. It
subscribes to directory trees through FSEvents on macOS and fanotify, falling
back to inotify, on Linux, debounces the events and delivers them to callbacks
whose panics are isolated. A retained `Watch` keeps its backend alive; closing
the last one releases the native worker.

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
