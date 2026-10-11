# tsr_bundled

Pinned TypeScript standard library files and bundled filesystem for tsr.

Part of [tsr](https://github.com/ferrotype/tsr), a Rust port of
the TypeScript compiler. This project is under development; the API and supported
compiler behavior are not stable. See the repository status and sprint records
for current coverage.

Requires **Rust 1.96 or newer**.

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE) for
license and attribution information.

For application integration, start with
[`tsr_embed`](https://github.com/ferrotype/tsr/tree/main/crates/tsr_embed).

`BundledFs::new` serves the built-in assets. `BundledFs::with_libraries` instead
serves only its explicit ordered immutable snapshot, including an empty one.
Both preserve the bundled URL policy: exact-case names, loaded bytes without BOM
decoding, bundled entry/stat/walk behavior, unchanged bundled real paths, and no
inner-file fallback under that prefix. Duplicate or non-flat names are rejected.
An explicit snapshot gets a distinct cache identity only when the inner host is
also immutable; wrapping a live host continues to return no snapshot identity.
