# JS object and function expando checks

Twenty-two native programs cover three related checker paths:

- Open JS object access: direct and indexed reads/writes, nested and returned
  objects, imported objects, all-JS versus mixed unions, intersections, and
  constrained type parameters. `noImplicitAny`, contextual annotations, known
  properties, index signatures, closed TS objects, `keyof` and assignment to a
  required-property type remain discriminating controls.
- Function exports with late-bound names: declarations, arrows and function
  expressions, contextual callable types, const-string keys and unique-symbol
  keys. Known member types still determine assignment compatibility.
- CommonJS aliases: destructured missing exports and valid renamed/bare imports,
  alongside ordinary destructuring and typed assignment errors.

The expected syntactic and semantic diagnostics are captured through the pinned
Go compiler. The Rust regression compares full payloads (locations, codes,
arguments, chains, related information and flags) and repeats each semantic
request. The fixture does not contain an implementation of JS openness or alias
resolution. Corpus rechecks compare types, symbols and public type display too.

`provenance.json` binds the request and native output to the upstream gitlink,
Go toolchain and committed bridge. The bridge defaults to the previous strict,
no-lib behavior; these requests explicitly choose JS/checkJs/strict/noImplicitAny
where needed. The stored earlier P6 fixtures retain their historical provenance.

Regenerate the native observations into a fresh directory:

```sh
PYTHONPATH=scripts python3 - <<'PY'
from s08_oracle import ROOT, run_overlay, strict_json_loads
run_overlay(
    ROOT / 'target/s08/p6-js-object-native-new', 'checker',
    (ROOT / 'tools/s08/p6/semantic_diagnostics_oracle_test.go').read_text(),
    strict_json_loads((ROOT / 'tools/s08/p6/js-object-expando/requests.json').read_bytes()),
    'TestS08P6SemanticDiagnostics',
)
PY
cargo test --locked -p ts_compiler --test checker_semantics js_open_object_access
```

The frozen 209-variant selection contains every acceptance variant under
`conformance/salsa` plus every acceptance variant with `expando` in its name:

```sh
python3 scripts/s08_p5_recheck.py --control target/s08/e2/corpus \
  --selection tools/s08/p6/js-object-expando/selection.json \
  --output target/s08/p6-js-object-recheck-new
```

These are development comparisons. They do not update full-corpus E2 ratios or
refresh acceptance evidence. The matching `tools/s08/p6/js-object-expando`
directory records the exact selection and measured result fingerprints.

The final capture is `target/s08/p6-js-object-recheck-02`, with stable sources.
`results.json` grades its rows with `s08_e2_contract.grade(..., partial=True)`
against the original E2 control's `control_indexes`, with no divergences. It
retains per-domain improvements, remaining differences and source/capture hashes.
The source hash is SHA-256 of `s08_p5_corpus.canonical(capture['build']['sources'])`.
`s08_p5_recheck.replay` authenticates the captured inputs/executable/producer
before grading. `openness-results.json` records the preceding partial candidate.

Errors improve from 184 to 205 matches, types/symbols from 167 to 181, and public
display from 175 to 180, with no regressions. One diagnostic improvement was
already delivered by the preceding alias-cycle commit and is identified
separately in both result files. Twenty diagnostic improvements are new here.
