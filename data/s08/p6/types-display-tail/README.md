# Types and display tail of the full E2 capture at `2b5cc4d`

The full E2 capture at `2b5cc4d` reached errors parity 1.0 and left 35
acceptance and 9 informational `types_parity` differences plus 10 acceptance
`type_to_string_parity` differences. Diffing the native and Rust `.types` and
`.symbols` text per variant grouped them as: `import.meta` and `new.target`
names without a symbol (19), `this` inside type queries without a symbol (8),
binding patterns printed without native's trailing comma (4), `new` on a union
with abstract construct signatures typed as the union instead of `any` (2), and
five one-offs (a negative numeric property name, a literal return type widened
under `Object.freeze`, literal heritage expressions without type rows, an `in`
narrowing that lost the union, and the display side of those).

`selection.json` lists all 44 differing variants plus 300 acceptance variants
sampled with seed 11 from the set matching both domains as regression controls,
in frozen inventory order. The bounded recheck uses the ordinary tool:

```sh
python3 scripts/s08_p5_recheck.py \
  --control target/s08/e2/corpus \
  --selection tools/s08/p6/types-display-tail/selection.json \
  --output target/s08/p6-types-display-tail-new
```

Score an existing recheck with the E2 grader, as in
[`../static-index-prototype/README.md`](../static-index-prototype/README.md).
`tools/s08/p6/types-display-tail/results.json` records the capture
fingerprints, the before/after counts for all three domains and every changed
variant.

Result of `target/s08/p6-types-display-tail-01` (sources stable, no execution
failures):

| Tier and domain | Before (`2b5cc4d`) | After |
| --- | ---: | ---: |
| Acceptance types/symbols | 300 match, 35 different | 335 match |
| Acceptance public display | 325 match, 10 different | 335 match |
| Acceptance errors | 335 match | 335 match |
| Informational types/symbols | 9 different | 9 match |
| Informational public display | 9 match | 9 match |

All 344 selected comparisons match in every domain; 44 variants improved and
none regressed. Compiler tests in `crates/ts_compiler/tests/checker_semantics.rs`
pin the abstract-union, `in`-narrowing and negative-name displays to their Go
baselines; no native oracle programs were captured for this pass.
