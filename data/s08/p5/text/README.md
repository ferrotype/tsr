# Production checker/printer E4 observations

The `checkertext` producer runs 64 frozen queries in four groups against the
pinned Go checker and the Rust checker/builder/printer. It measures 52 literal
construction/display probes and 64 printer probes. Every group requires emitted
content; absent results, missing queries and transport failures cannot pass.

The byte-source group covers BOM, raw malformed UTF-8, raw surrogate encoding,
escaped lone surrogates and pairs, controls, astral text and Unicode separators.
Numeric cases include negative zero, exponent boundaries, hexadecimal input and
large/negative-zero bigint spellings. Property annotations compare source-node
reuse against context-free regeneration. Truncation probes bracket 317/318-byte
string values and also exercise the untruncated explicit builder path.

`tools/s08/p5/text-requests.json` is the human-readable request inventory.
`data/s08/p5/text-coverage.json` freezes its canonical digest and every ordered
query-to-criterion assignment. `text-cases.json` is the producer case inventory.
The reference observation and provenance here came from
`target/s08/checkertext-1789331928372937000`; all 64 outputs matched. No recorded
Go output is replayed into the Rust producer, which evaluates every query itself.

Run `cargo xtask run checkertext` to capture current host evidence. The runner
binds the result to the input manifest and production dependency closure. The
producer retains raw native and Rust observations plus exact mismatches under
`target/s08/checkertext-*`; CI uploads those directories even after a measured
failure. A successful capture with a false metric still fails CI's metric check.
The existing S04 leaf, S05 scanner and S06 encoder scopes are unchanged.
