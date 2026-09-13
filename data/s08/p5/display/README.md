# P5 direct display observations

These sixty-eight source-selected requests exercise explicit builder calls and
context-sensitive type/symbol display, after semantic and global diagnostics.
They are supplemental API evidence, not an E2 baseline capture. The native
driver invokes the pinned checker and printer without expected-output hooks.

`provenance.json` records the Go pin, toolchain, platform and byte hashes.
`sources.json` identifies the capture driver and helper sources. The request
records numeric flags as passed to Go, including the baseline writer's
`NoTruncation | GenerateNamesForShadowedTypeParams | AllowUniqueESSymbolType |
IgnoreErrors` builder flags and `AllowUnresolvedNames` internal flag.

```sh
python3 scripts/s08_p5_display.py capture --output target/s08/p5-display-new
cargo run -p ts_compiler --example p5_display -- \
  data/s08/p5/display/requests.json target/s08/p5-display-rust.json
python3 scripts/s08_p5_display.py compare --native data/s08/p5/display \
  --actual target/s08/p5-display-rust.json
```

Context changes `{ text: "é"; }` to `{ text: 'é'; }` through annotation reuse.
At this pin both contexts normalize hexadecimal/binary numeric annotations.
The tests compare actual node kinds and exact text bytes, including qualifiers.

Enum property names use their computed form when the enum is accessible from
the enclosing declaration; context-free display keeps the literal name.

The expanded matrix exercises both modifier-preserving mapped wrappers,
`T`/`T_1` shadowing, generated-name flags on and off, parameter annotation
reuse, and trailing default arguments for the four native global iterable
identities. A namespaced `Iterable` is a negative identity case.
The latest native capture is `target/s08/p5-mapped-display-native-03`.

The final 28 requests exercise source reuse and regeneration through public
checker display and explicit builder APIs. A BOM-prefixed source travels as
hex, preserving raw malformed UTF-8 and raw surrogate encodings; the other
literals include escaped lone surrogates, a surrogate pair, controls, astral
text, combining characters and Unicode separators. JSON never decodes these
source bytes into text. Go and Rust parse their own identical byte input, and
all output is compared as hex. The latest capture is
`target/s08/p5-display-bytes-native-01`. These are production-path E4 probes;
they do not replace the existing complete E4 producer or its evidence.
