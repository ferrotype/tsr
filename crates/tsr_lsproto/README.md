# tsr_lsproto

Language Server Protocol types and Corsa extensions resolved by the pinned Go
implementation's generator, with strict JSON codecs on `tsr_json` and JSON-RPC
framing provided by `tsr_jsonrpc`.

Requires Rust 1.96 or newer. Part of the tsr 0.2.0 lockstep release.

From the repository, regenerate with `cargo xtask gen lsproto`, or check drift
with `cargo xtask gen lsproto --check`. This requires the pinned Node version
and initialized upstream submodule, but no npm installation or network access.
The metamodel is vendored under `tools/phase5/lsproto` with its source and license.
