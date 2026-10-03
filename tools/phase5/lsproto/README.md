# Pinned LSP protocol generation

`cargo xtask gen lsproto` runs the pinned resolver and emits Rust syntax. Use
`--check` to check drift. It requires the Node version in upstream's package.json.
The resolver reads the metamodel vendored here: model-source.json records the
exact URL/version/hash selected by the pinned package lock. The metamodel is MIT
licensed; LICENSE-metamodel.txt accompanies both this input and the public crate.
No npm install or network access occurs during generation.

export.mjs copies the generator and schema from the pin's Git objects to a
short-lived temporary directory. The appended access hook exports normalized
fields, nullability, union alternatives/discriminators and method signatures;
the Rust emitter in xtask/src/gen/lsproto.rs spells those facts as Rust. A hook
must match exactly once, so a pin update cannot silently skip it. Canonical
upstream is never edited.

With the pinned Go on PATH, `python3 tools/phase5/lsproto/check.py` checks:

- The resolver's Go output has the same syntax tree as the committed Go output.
  Comments and source positions are ignored, and dprint's grouping of adjacent
  `var` declarations is normalized; types, field tags, constants and function bodies must match.
- The codec unit-test fixture matches native execution, including partial
  destinations after errors and re-marshalling. `--update` refreshes that small
  fixture after inspection. It is not a suite expectation file.

`cargo test -p tsr_lsproto` compares Rust with the same fixture and checks union
arms, parameter admission, literal/cardinality rules and enum names. The matrix
starts from the pin's lsp_json_test.go inputs and adds reuse, raw escaped keys,
registration dispatch and partial failure cases. It currently has 101 operations
in 47 independent cases. These are protocol tests, not language-service passes.

The initial L0 slice implements protocol data/codecs and typed method descriptors.
The remaining L0 work includes URI/location utilities, resolved capability helpers,
the production server and private test entry point, test routing, the Go transport
patch, batch isolation and common parity integration. The ledger therefore keeps
lsproto files in progress; neither L0 nor Phase 5 is marked complete.
