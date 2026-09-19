# Alias accessibility in type display

The full E2 capture at `827c353` exposed a changed line inside a variant that
already differed: `typedefOnStatements` printed the local `alpha` as `Alpha`,
while native prints `{ alpha: string; }`. A matched-variant regression count
cannot detect deterioration within an already-different variant.

`getAliasSymbolForTypeNode` correctly retains JSDoc aliases. The missing check
was in `NodeBuilderImpl.typeToTypeNodeWorker`: use an alias only when
`UseAliasDefinedOutsideCurrentScope` is set or `IsTypeSymbolAccessible` succeeds.
Otherwise serialize its underlying type. The Rust change calls the existing
accessibility worker with type meaning, without computing visibility aliases,
and with modules allowed, matching pinned `symbolaccessibility.go`.

The committed native fixture exercises both `TypeToStringEx` and
`TypeToTypeNode`, with local, global and shadowed TypeScript aliases. Its 24
queries distinguish declaration, source-file and absent enclosing contexts,
including the explicit out-of-scope flag. The compiler regression separately
covers the JSDoc alias attached to a return statement, using the pinned
`typedefOnStatements.types` output, and repeats across checker operations.

Regenerate the native fixture into a fresh directory:

```sh
PYTHONPATH=scripts python3 - <<'PYTHON'
from s08_oracle import ROOT, run_overlay, strict_json_loads
from s08_p5_display import validate
request = strict_json_loads((ROOT / 'tools/s08/p6/alias-accessibility/requests.json').read_bytes())
observed = run_overlay(
    ROOT / 'target/s08/p6-alias-accessibility-native-new', 'checker',
    (ROOT / 'tools/s08/p5/display_oracle_test.go').read_text(),
    request, 'TestS08P5Display',
)
print(validate(request, observed))
PYTHON
cargo test --locked -p ts_compiler --test checker_display --test checker_semantics
```

The corpus selection contains 503 acceptance variants: all JSDoc cases, all
previous public-display differences, and the completed JS object/expando
selection as controls. Reproduce the development screen with:

```sh
python3 scripts/s08_p5_recheck.py \
  --control target/s08/e2/corpus \
  --selection tools/s08/p6/alias-accessibility/selection.json \
  --output target/s08/p6-alias-accessibility-recheck-new
```

This is a bounded development comparison; it does not replace full E2 evidence.

The final screen at `target/s08/p6-alias-accessibility-recheck-01` is source
stable. Only `typedefOnStatements` changes, from differing types/symbols to a
match; its errors and public display already matched. All other comparisons,
including already-different rows, are unchanged. The completed 209-case JS
object/expando selection remains matching throughout.
