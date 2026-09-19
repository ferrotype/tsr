# Error-parity tail of the full E2 capture at `9a52b85`

The full E2 capture at `9a52b85` left 140 acceptance and 9 informational
variants with `errors_parity: different`. Grouping their diagnostics by
Go-only, Rust-only and same-position-different-payload gave the work list:
TS1036 (21 in 9 variants), TS7010 (14 in 8), TS2411 (12 in 9) and TS2304,
TS2741, TS2708, TS1156, TS1544, TS18016 missing; TS1015 (10 in 4), TS2339,
TS2322 and TS2345 added; and 45 variants whose codes and positions matched but
whose arguments, message chains or related information differed.

`selection.json` lists all 149 differing variants plus 300 acceptance variants
sampled with seed 7 from the error-matching set as regression controls, in
frozen inventory order. The bounded recheck uses the ordinary tool:

```sh
python3 scripts/s08_p5_recheck.py \
  --control target/s08/e2/corpus \
  --selection tools/s08/p6/error-parity-tail/selection.json \
  --output target/s08/p6-error-parity-tail-new
```

Score an existing recheck with the E2 grader, as in
[`../static-index-prototype/README.md`](../static-index-prototype/README.md).
`tools/s08/p6/error-parity-tail/results.json` records the capture
fingerprints, the before/after counts for all three domains, every changed
variant and the remaining error differences.

Result of `target/s08/p6-error-parity-tail-01` (sources stable, no execution
failures): acceptance error matches rise from 300 to 415 of 440, informational
from 0 to 8 of 9; acceptance types/symbols matches rise from 398 to 410, public
display from 403 to 412; no selected domain regresses. 26 error differences
remain and are listed in `results.json`.

Compiler tests in `crates/ts_compiler/tests/checker_semantics.rs` pin each
ported behavior to the named Go baseline; no native oracle programs were
captured for this pass.
