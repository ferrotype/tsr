# P5 direct display observations

These 208 source-selected requests exercise explicit builder calls and
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
cargo run -p tsr_compiler --example p5_display -- \
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

The raw-byte group's 28 requests exercise source reuse and regeneration through public
checker display and explicit builder APIs. A BOM-prefixed source travels as
hex, preserving raw malformed UTF-8 and raw surrogate encodings; the other
literals include escaped lone surrogates, a surrogate pair, controls, astral
text, combining characters and Unicode separators. JSON never decodes these
source bytes into text. Go and Rust parse their own identical byte input, and
all output is compared as hex. The latest capture is
`target/s08/p5-display-bytes-native-01`. These are production-path E4 probes;
they do not replace the existing complete E4 producer or its evidence.

Fourteen additional requests repeat symbol-backed object, conditional and
recursive tuple displays with distinct enclosing declarations and quote flags.
The latest complete direct-display capture is
`target/s08/p5-display-cache-native-01`: 82/82 exact results. Allocation counts
and cache-hit equivalence are not inferred from matching output bytes.

The current complete capture is `target/s08/p5-display-return-native-02`:
126/126 exact results. Its 44 added requests cover namespace aliases (including
an arbitrary self-export name), inferred cross-file import qualifiers, preserved
signature return annotations (`typeof` private names, unique symbols and union
order), and suppressed top-level `any` returns with unsuppressed callback types.

The import-mode increment extends the matrix to 178 exact requests. The latest
complete capture is `target/s08/p5-display-imports-native-final-01`. It adds
Node16/NodeNext ESM/CommonJS boundaries, sorted ambient-module attributes (named
and wildcard modules), single quotes, a package-path portability check with an
explicit absent result when unsafe paths are disallowed, and a contextual
export-equals namespace. The positive package case selects its package name
without a mode retry; it does not establish branch coverage of a successful
swapped-mode portability retry. The direct adapter accepts only its three named
module configurations, identically in Rust and Go.

The review follow-up captures 190/190 exact requests in
`target/s08/p5-display-original-imports-native-02`. Twelve added requests
alternate two namespace exports with different original import attributes and
then repeat the first, for file modules and attributed ambient modules. Both
public display and explicit node-building preserve each export's own attributes.
The driver now locates namespace-export declarations as well as its original
variable, interface and type-alias declarations.

The second review adds six cross-context attribute requests and twelve conditional
package-export probes. `enclosing_declaration` names the independent enclosing
argument of the public APIs; it does not change the declaration whose type or
symbol is queried. Displaying the CSS module type in the text import's context
must retain `type: "css"` because the original text attributes resolve to another
merged module symbol. The sequence repeats the CSS context afterward.

`branch-witnesses.json` records a 21-query diagnostic overlay, its source hashes,
raw counters and the unsuccessful fixture attempts. Its outputs equal the
uninstrumented native fixture exactly. The overlay inserts counters without
changing branch decisions; `override_other_symbol` distinguishes a different
resolved symbol from unresolved-module rejection. Reproduce with:

```sh
python3 scripts/s08_p5_display_branches.py \
  --requests tools/s08/p5/display-branch-requests.json \
  --output target/s08/p5-display-branches-new
```

The counters observe two different-symbol rejections and four accepted overrides.
Conditional import/require exports, including private-path imports, choose a
portable name without retry in both Node16 and NodeNext. The nonportable control
attempts and fails the retry once. No successful swapped-mode retry is claimed;
the bounded search stops here. The final ordinary capture is
`target/s08/p5-display-review-native-final` (208/208 exact Rust matches).
