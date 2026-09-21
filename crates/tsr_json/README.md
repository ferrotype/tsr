# tsr_json

Byte-oriented JSON token streams and typed encoding/decoding for tsr.

Typed values, raw values and explicit tokens share one grammar and error model.
The codec preserves exact signed/unsigned integers, ordered object members and
partial decode state. `marshal_partial` exposes native partial output on failure;
`marshal` is the convenience API that discards it. Streaming encoders retain
decoded member names independently of the output buffer, so duplicate detection
survives flushing. Strict UTF-8 is the default, with explicit repair available.

The authority is the go-json-experiment revision pinned by the upstream compiler.
This is a Rust trait API, not Go reflection: application structs implement
`Encode`/`Decode` explicitly. See the native Phase 1 cases for measured coverage.

Part of [tsr](https://github.com/ferrotype/tsr), a Rust port of
the TypeScript compiler. This project is under development; the API and supported
compiler behavior are not stable. See the repository status and sprint records
for current coverage.

Requires **Rust 1.96 or newer**.

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE) for
license and attribution information.

For application integration, start with
[`tsr_embed`](https://github.com/ferrotype/tsr/tree/main/crates/tsr_embed).
