# Alias circularity

Fourteen strict, no-lib programs distinguish speculative spelling suggestions
from actual alias cycles. Missing exports, missing qualified targets, valid alias
suggestions and chains are paired with real self, mutual, namespace and cross-file
cycles. Expected diagnostics come from the pinned native compiler; the Rust test
compares the complete payload and repeats the semantic request.

The 44-variant selection includes all 20 acceptance variants with extra TS2303,
every acceptance variant whose native baseline contains TS2303, and named valid
alias, merged-symbol and mixed-property cycle controls. All 20 spurious-cycle
cases lose their extra TS2303. Errors improve from 22 to 40 full matches; types
and public display remain at 43 matches each. No domain regressed. Two targeted
variants retain unrelated missing diagnostics; the other two diagnostic mismatches
were already present in the cycle controls. Exact IDs are in `results.json`.

`provenance.json` binds the request and result to the source pin, Go toolchain
and committed bridge `tools/s08/p6/semantic_diagnostics_oracle_test.go`.
Regenerate into a new directory:

```sh
PYTHONPATH=scripts python3 - <<'PY'
from s08_oracle import ROOT, run_overlay, strict_json_loads
run_overlay(
    ROOT / 'target/s08/p6-alias-circularity-native-new', 'checker',
    (ROOT / 'tools/s08/p6/semantic_diagnostics_oracle_test.go').read_text(),
    strict_json_loads((ROOT / 'tools/s08/p6/alias-circularity/requests.json').read_bytes()),
    'TestS08P6SemanticDiagnostics',
)
PY
cargo test --locked -p ts_compiler --test checker_semantics alias_circularity
python3 scripts/s08_p5_recheck.py --control target/s08/e2/corpus \
  --selection tools/s08/p6/alias-circularity/selection.json \
  --output target/s08/p6-alias-circularity-recheck-new
```

The measured candidate is `target/s08/p6-alias-recheck-01`. Its sources stayed stable.
`tools/s08/p6/alias-circularity/results.json` retains capture/source/selection hashes,
per-domain counts, exact improved IDs and remaining differences. The counts use
`s08_e2_contract.grade(..., partial=True)` with no approved divergences, comparing
the selected requests and native observations against both the captured Rust rows
and the corresponding `control_indexes` rows from the original E2 capture.
`s08_p5_recheck.replay` authenticates the candidate before grading it. The source
hash is SHA-256 of `s08_p5_corpus.canonical(capture['build']['sources'])`.

These are development comparisons. They neither update full-corpus E2 ratios
nor refresh acceptance evidence.
