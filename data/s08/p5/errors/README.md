# P5 diagnostic writer fixture

24 synthetic diagnostic/source cases observed through the pinned Go
`diagnosticwriter` and `tsbaseline.GetErrorBaseline` implementations. The request
contains diagnostic records, source bytes and input-file order, never expected
formatted output. Every byte field uses lowercase hex.

Each case compares five outputs: plain diagnostics, diagnostics with ANSI color
and source context, the error summary, plain `.errors.txt`, and pretty
`.errors.txt`. Coverage includes all categories, global and file errors, empty
and omitted inputs, library/config locations, input order, message chains,
related information, zero-width and multiline ranges, CRLF, CR-only and Unicode
line separators, astral characters, invalid source/argument bytes, custom
sources, relative paths, and Unicode case folding in the library-location
regexp. Missing diagnostics produce `no_content`, not an empty file.

Go groups fresh external-diagnostic FileLike wrappers by pointer, even if names
match. The external fixture uses identical locations so unordered equal-name
map keys cannot make its bytes nondeterministic; both summary entries remain.
Different locations under equal-name wrappers remain a native ordering risk
for full-corpus comparisons, not an invented normalization rule.

The native error writer's CR?LF splitting is distinct from its ECMA line map.
Its duplicate-input map is never populated at the pin. The Rust harness keeps
these behaviors and fails its coverage assertions instead of repairing the
reference during comparison. Content-map span translation remains an explicit
production boundary. These focused formatter results do not establish corpus
checker or `.errors.txt` parity and do not emit E2 metrics.

```sh
python3 scripts/s08_p5_errors.py capture --output target/s08/p5-errors-new
cargo run -p ts_compiler --example p5_errors -- target/s08/p5-errors-new/requests.json target/s08/p5-errors-rust-new.json
python3 scripts/s08_p5_errors.py compare --native target/s08/p5-errors-new --actual target/s08/p5-errors-rust-new.json
```

`provenance.json` authenticates the request, driver and raw Go output at the
repository pin. `sources.json` records the capture-time script/driver hashes.
The Rust integration test executes current production formatting over the
frozen inputs and compares all five outputs directly.
