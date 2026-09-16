# Catch destructuring

The ten strict, no-lib programs distinguish catch-binding checks from ordinary
variable-declaration grammar. They cover object, array and nested catch bindings,
annotations, initializers, redeclarations, omitted bindings, ordinary declarations
that still require TS1182, and for-in grammar. Expected codes, ranges, arguments,
chains and flags are captured from the pinned native compiler, not inferred from
Rust output.

The 32-variant selection includes all 11 acceptance variants with extra TS1182 in
the recorded E2 control, plus every acceptance variant with `catch` in its path.
Errors improve from 21 to 32 full matches; types/symbols improve from 29 to 31;
public display stays at 31. `compiler/implicitAnyInCatch` retains its pre-existing
type/display difference. No domain regressed.

`provenance.json` binds the request and result to the source pin, Go toolchain
and committed bridge `tools/s08/p6/semantic_diagnostics_oracle_test.go`.
Regenerate into a new directory:

```sh
PYTHONPATH=scripts python3 - <<'PY'
from s08_oracle import ROOT, run_overlay, strict_json_loads
run_overlay(
    ROOT / 'target/s08/p6-catch-destructuring-native-new', 'checker',
    (ROOT / 'tools/s08/p6/semantic_diagnostics_oracle_test.go').read_text(),
    strict_json_loads((ROOT / 'tools/s08/p6/catch-destructuring/requests.json').read_bytes()),
    'TestS08P6SemanticDiagnostics',
)
PY
cargo test --locked -p ts_compiler --test checker_semantics catch_destructuring
python3 scripts/s08_p5_recheck.py --control target/s08/e2/corpus \
  --selection tools/s08/p6/catch-destructuring/selection.json \
  --output target/s08/p6-catch-destructuring-recheck-new
```

The measured candidate is `target/s08/p6-catch-recheck-01`. Its sources stayed stable.
`tools/s08/p6/catch-destructuring/results.json` retains capture/source/selection hashes,
per-domain counts, exact improved IDs and remaining differences. The counts use
`s08_e2_contract.grade(..., partial=True)` with no approved divergences, comparing
the selected requests and native observations against both the captured Rust rows
and the corresponding `control_indexes` rows from the original E2 capture.
`s08_p5_recheck.replay` authenticates the candidate before grading it. The source
hash is SHA-256 of `s08_p5_corpus.canonical(capture['build']['sources'])`.

These are development comparisons. They neither update full-corpus E2 ratios
nor refresh acceptance evidence.
