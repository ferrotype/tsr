# tsr_core

Compiler primitives and collections for tsr.

Part of [tsr](https://github.com/ferrotype/tsr), a Rust port of
the TypeScript compiler. This project is under development; the API and supported
compiler behavior are not stable. See the repository status and sprint records
for current coverage.

Requires **Rust 1.96 or newer**.

The default-off `go-slice-compat` feature exposes `SharedSlice`, the slice-header
helpers and `MultiMap`. These preserve Go's mutable backing aliases and pinned
allocation-growth behavior for explicit compatibility contracts. Ordinary
compiler consumers use owned or borrowed containers instead. The Phase 1 leaf
harness enables the feature; production builds are checked separately without
it by `python3 scripts/check_production_features.py`.

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE) for
license and attribution information.

For application integration, start with
[`tsr_embed`](https://github.com/ferrotype/tsr/tree/main/crates/tsr_embed).
