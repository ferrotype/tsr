# Interface inheritance diagnostics

Twenty-five strict, no-lib programs exercise pinned Go's
`checkInterfaceDeclaration` and `checkInheritedPropertiesAreIdentical` through
the native compiler's syntactic and semantic diagnostic APIs. The Rust
regression compares every diagnostic, including locations, arguments, nested
chains, related information and flags, and repeats the semantic request.

The cases cover incompatible properties, call and construct signatures, index
signatures, optional/readonly/private property identity, explicit overrides,
merged declarations (including class/interface bases), generic constraints,
polymorphic `this`, a compatible diamond, recursive bases and heritage grammar.
Only the first `extends` clause supplies bases; the second is rejected without
resolving its missing identifier. Two cases establish
the failure ordering: conflicting inherited properties suppress base relations
and index constraints, while member and duplicate-index diagnostics still run.

`provenance.json` binds the request and result to the source pin, Go toolchain
and the committed bridge `tools/s08/p6/semantic_diagnostics_oracle_test.go`.
The source request is `tools/s08/p6/interface-inheritance/requests.json`.
Regenerate into a new directory:

```sh
PYTHONPATH=scripts python3 - <<'PY'
from s08_oracle import ROOT, run_overlay, strict_json_loads
run_overlay(
    ROOT / 'target/s08/p6-interface-inheritance-native-new', 'checker',
    (ROOT / 'tools/s08/p6/semantic_diagnostics_oracle_test.go').read_text(),
    strict_json_loads((ROOT / 'tools/s08/p6/interface-inheritance/requests.json').read_bytes()),
    'TestS08P6SemanticDiagnostics',
)
PY
cargo test --locked -p ts_compiler --test checker_semantics interface_inheritance
```

The separate 130-variant selection includes all 33 acceptance cases with missing
TS2430 or TS2320 counts in the recorded E2 control, plus related interface cases
as regression controls. Its scope and order are committed in `selection.json`:

```sh
python3 scripts/s08_p5_recheck.py --control target/s08/e2/corpus \
  --selection tools/s08/p6/interface-inheritance/selection.json \
  --output target/s08/p6-interface-inheritance-recheck-new
```

These focused comparisons are development evidence; they do not update E2
acceptance ratios or certify the whole corpus.
