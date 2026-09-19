# Focused P5 native walker observations

Twenty-four programs produce 930 ordered public checker/builder requests. The Go
driver uses the original pinned `tsbaseline` walker through the existing
access-only P0 hooks. It does not implement another walker. The Rust adapter
traverses its own AST and invokes production APIs; it never reads expected
queries or baseline bytes.

Cases cover disabled and NoContent results, an empty source file, alias and
property queries, the two native `any` paths, CRLF and Unicode line separators,
astral trivia, harness file order, library declaration coordinates, heritage,
imports/re-exports, overload declaration truncation, reparsed JSDoc, computed
and indexed properties, flow narrowing, renamed destructuring, assertions,
`noCheck` and `skipLibCheck`, numeric/quoted class members, and unresolved
aliases with raw symbol parent chains, and computed binding names containing calls
and parenthesized assignments. CommonJS exports distinguish `default` property
access from names requiring brackets. Computed index components also distinguish
reusable entity names from call-expression keys that require an index signature.

`requests.json` contains the exact captured input bytes. `observations.json`
retains raw result bytes as hex and native type IDs. `provenance.json` records
the pinned source, effective Go version and request/observation/overlay hashes.
`sources.json` records the producer source hashes at capture time. The local
capture with all overlay sources is `target/s08/p5-walker-default-native-01`; the committed
fixture is about 120 KB, without duplicate source snapshots or executables.

The supplemental comparison renames numeric type IDs by first occurrence within
each source file's checker, preserving repeated/distinct identities. It does
not assert a shared checker identity across files. The frozen full-corpus
contract separately compares its approved action columns, excluding raw IDs.

```sh
python3 scripts/s08_p5_walker.py capture --output target/s08/p5-walker-new
cargo run -p tsr_compiler --example p5_baseline -- target/s08/p5-walker-new/requests.json target/s08/p5-walker-new/rust.json
python3 scripts/s08_p5_walker.py compare --native target/s08/p5-walker-new --actual target/s08/p5-walker-new/rust.json
```

Use a fresh capture directory. This fixture tests the native walker and public
query/display APIs; it does not establish full E2 parity, error-baseline
rendering, cached-builder rotation, instrumentation or recursion acceptance.
