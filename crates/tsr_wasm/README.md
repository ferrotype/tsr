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

`MemoryHost.prepare()` freezes an input snapshot without parsing a config or
loading a program. The returned `PreparedHost.compile_config(path, arguments)`
uses the production tsconfig parser, including dependencies and file expansion,
then loads a single-checker session. Paths are bytes; `arguments` is JSON of
ordered byte-array compiler overrides. Builtin libraries use the production
bundled filesystem. Every other read stays in the supplied snapshot. A missing
main config returns an API error; incremental/watch/external mapper execution
is explicitly unsupported by this configured entry point.

`MemoryHost.add_unavailable_file(path, raw_size)` can declare a file's metadata
without supplying its contents. It remains visible to stat and enumeration;
reading it returns an explicit unavailable-content error, never an absent-file
result or an OS read. The JavaScript wrapper takes `raw_size` as a `bigint`.

`WasmSession.check()` retains sorted production diagnostic objects without
serializing them. It runs config, program, syntactic and global phases, then
semantic diagnostics and applicable declaration diagnostics for each file.
Checking and emission use the same compiler checker pool.
`emit_retained(request)` retains a production emit and every callback write;
its request format is the same as the existing `emit` method. Repeating either
successful operation replaces its previous retained result. A returned error
preserves the previous successful diagnostics, emit, and cached snapshot.

`retained_snapshot()` returns owned JSON bytes after a successful check. It
reports roots, raw options JSON, source name/path/text records, compiler checker
and type counts, node/identifier counts, complete diagnostic message values,
and retained emit writes/results. Strings are byte arrays; counts and signed
diagnostic positions are decimal strings. Diagnostic files index the source
array, including nested/config diagnostic sources. Snapshotting runs no check,
emit or type/symbol walk. Mapped sources are explicitly unsupported. `.free()`
releases the session and retained results; previously returned owned bytes stay
valid. A caller can delimit load/check/emit separately from serialization and
collection checkpoints without a test command protocol in the production API.

For bounded transfers, `retained_snapshot_header()` freezes the same diagnostic
indices and metadata, omits `sources`/`writes`, and adds decimal `source_count`
and `write_count`. `retained_source(index)` and `retained_write(index)` return
one original row as owned JSON bytes. The zero-based indices use unsigned
32-bit wasm parameters. Metadata retains source identities rather than clones
of every loaded text; each row copies only its own bytes. Repeated requests
reuse the frozen metadata until another successful `check` or `emit_retained`
invalidates it. These methods perform no extra diagnostic, emit or type/symbol queries.
