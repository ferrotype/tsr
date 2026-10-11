# tsr_wasm

Bare WebAssembly parser and compiler bindings for tsr.

Part of [tsr](https://github.com/ferrotype/tsr), a Rust port of
the TypeScript compiler. This project is under development; the API and supported
compiler behavior are not stable. See the repository status and sprint records
for current coverage.

Requires **Rust 1.96 or newer**.

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE) for
license and attribution information.

For application integration, start with
[`tsr_embed`](https://github.com/ferrotype/tsr/tree/main/crates/tsr_embed).

With the `checker` feature, `transpile_module(source, filename,
compiler_options, report_diagnostics)` and `transpile_declaration` expose
the public embedding crate's single-file operations. Source and filename
are `Uint8Array` byte strings. Source is already loaded text: these operations
preserve its bytes and do not apply physical-file BOM or encoding conversion.
An empty filename selects the embedding API's default. Compiler options are
raw JSON text in `tsr_tsoptions::raw`'s numeric-enum format; preserve the text's
object-entry order rather than parsing and serializing it in JavaScript.
Malformed JSON, duplicate option keys and unknown options return API errors.

Both functions return owned JSON bytes. The version-1 response has `version`,
`output`, `source_map`, `diagnostics` and `sources`. All string payloads,
including output and source-map text, are JSON byte arrays. `sources` retains
each program source in program order as `{name, path, text, canonical_name}`;
the declaration operation includes its synthetic minimal library.

Each diagnostic contains a nullable `file` index into `sources`, signed
UTF-8 byte positions `pos` and `end`, `code`, `category`, byte strings
`source`, `key`, `text`, and byte-string arrays `args` and `display_args`.
It also preserves `has_message` (a pinned message identified by `key`), optional
`ad_hoc_message` bytes, recursive `chain` and `related` records,
`reports_unnecessary`, `reports_deprecated`, `skipped_on_no_emit`, and optional
`repopulate` metadata (`kind`, `module_reference`, `mode`, `package_name`).
The program remains alive until all diagnostic source identities and display
arguments have been serialized; callers need no wasm source owners afterward.

This response format rejects mapped and supplemental sources; `canonical_name`
is always null. The functions apply the embedding API's transpile option
overrides and do not implement whole-program compiler-harness execution.
Use the instance wrapper's trap handling: explicit API errors leave the
instance usable, while an unknown exception or trap requires discarding it.

`WasmSession.resolution_trace()` returns owned JSON bytes for the loaded
program's recorded resolution trace, in original order. Each row has
`message_key` bytes and `args`: `{kind: "text", value: [bytes...]}` or
`{kind: "bool", value: boolean}`. Enable the numeric-enum compiler option
`traceResolution` before compiling to collect entries. The caller renders
messages; reading this snapshot performs no checker queries or test schedule.
