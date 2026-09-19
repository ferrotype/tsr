# S04 text slice

This crate implements the byte/string contract in `docs/design/text.md` against
the worktree's pinned `upstream/`. `JsString` clones and byte slices share an
immutable `Arc<[u8]>`; each slice is independently classified as UTF-8, WTF-8 or
raw bytes. Equality, ordering and hashing observe only the bytes. An `as_str`
view is available only for valid UTF-8. There is no implicit lossy string view.

`SourceText` performs the pinned Go BOM decoding, including replacement of
unpaired UTF-16 surrogates and ignoring an odd final UTF-16 byte. The standard
UTF-8 decoder and the JavaScript sentinel-aware decoder are separate functions;
callers select the one their upstream operation uses.

The helper slice includes `ToLowerJS`, `ToUpperJS`, `LowerFirstChar`,
`TruncateByRunes`, surrogate encoding/decoding and pair combination, and the
literal escape worker with all quote choices and escape flags. Escaping returns
literal contents without surrounding quotes. The source-reuse decision and
whole-printer integration remain S08 work, as does literal-type construction.
These helpers do not implement a scanner, encoder, numeric semantics,
identifier tables or collation.

JavaScript casing and its Final_Sigma properties are generated from the pinned
`stringutil/js_case_generated.go` (Unicode 15.1). `LowerFirstChar` instead uses
the exact simple lowercase table of the oracle Go 1.27.1 toolchain (Unicode 17),
as the upstream function does. Rust's toolchain Unicode casing is not used.
Regenerate with `python3 crates/ts_jsstring/tools/generate_case_tables.py`;
`--check` verifies deterministic output. The generator checks the clean upstream
pin and Go version, and the output records the input hash and both Unicode
versions. No network access or build-time generation is required.

`go_quote` preserves the pinned Go `strconv.Quote` spelling required by source
panic payloads. It escapes malformed UTF-8 one byte at a time and uses generated
Go `strconv.IsPrint` data, including that toolchain's Unicode version. Regenerate
with `python3 crates/ts_jsstring/tools/generate_go_quote.py`; `--check` verifies
the checked-in table. This operation is separate from JavaScript literal escaping.

The public byte helper functions return owned transformed buffers. Casing and
surrogate combination return `Cow<[u8]>` to preserve Go's unchanged fast paths;
truncation returns a borrowed prefix. `JsString` storage sharing applies to its
construction from an `Arc`, clones and slices. Inline strings remain subject to
measurement.

The modules follow their responsibilities: `jsstring` holds shared value bytes,
`source_text` handles BOM decoding, `wtf8` holds explicit decoders and surrogate
helpers, `helpers` handles casing and truncation, and `escape` writes literal
contents. `position_map`, `line_map`, `lsp` and `scanner_positions` preserve the
separate API, LSP and scanner conversion rules, including signed Go `int` and
`int32` arithmetic, malformed bytes, surrogate sentinels and partial offsets.

The `e4` example validates the complete probe shape before its panic boundary.
Expected upstream panics remain observable results; selected probes also compare
the exact panic payload. Invalid fixture requests fail the adapter instead of
being recorded as text-library behavior.
