# P5 development corpus comparison

`full-summary.json` describes the complete, source-stable capture of commit
`5e93fce`: 10,728 frozen variants, with acceptance and informational counts kept
separate. It compares both native baseline bytes and structured diagnostics;
it does not emit an E2 metric. Matching error bytes alone is weaker than the
combined result. Rust did not execute emit, and native pre/post-emit diagnostics
were compared independently.

`display-recheck-summary.json` describes a separate capture of all 637 variants
whose type/symbol walk failed in that full capture, after porting element-access
printing and raw alias parent chains. It is not a new full-corpus measurement;
do not add its counts to the original report to infer a current full result.
Five explicit failures remained in that historical selection. Additional
completed queries had exact-byte or diagnostic differences.

The raw captures remain local rather than adding hundreds of MB to Git:

- Native full inventory: `target/s08/p5-corpus-native-full-01`.
- Full Rust inventory and authenticated replay: `target/s08/p5-corpus-rust-full-02`.
- Supplemental recheck, raw requests/outputs, build snapshot and producer:
  `target/s08/p5-corpus-display-recheck-01`.
- Initial unoptimized partial capture, preserved separately:
  `target/s08/p5-corpus-rust-full-01`.

The summaries record capture, executable and native observation hashes. They
are review records, not self-contained replacement evidence for the raw
captures. Reproduction commands and remaining P5 work are in
[`docs/S08-P5.md`](../../../../docs/S08-P5.md). The small, independent native
walker fixtures run in the Rust regression suite.

`final-display-recheck-summary.json` covers those five remaining failures after
expression printing, mapped wrappers, iterable default-argument elision,
parameter annotation reuse and computed index-name reuse were ported. All five
match type/symbol bytes and query schedules. Three also match error baselines;
the two computed-destructuring cases retain structured diagnostic differences.
This is a separate five-case result, not a new full-corpus total. Raw data lives
in `target/s08/p5-corpus-display-recheck-04`; intermediate rechecks `-02` and
`-03` remain available and are not added to the final counts.

`pre-module-recheck-summary.json` records the 95 completed-but-different rows
from the 637-row selection at `df01608`: 12 match and 83 differ, with no failed
execution. `module-and-return-recheck-summary.json` repeats exactly that selection
after the module-root and signature-reuse fixes: 30 match and 65 differ, again
with no failed execution. Of the latter 65, 23 match type/symbol output and query
contracts but differ in diagnostics; 42 still differ in types, symbols or queries.
The records retain every row, tier and source/build fingerprint.

`default-name-recheck-summary.json` is a separate one-case verification of the
CommonJS `default` spelling fix: all bytes and queries match. It must not be
added to the 95-case report to manufacture a newly measured aggregate.
Intermediate alias and module-root captures are also retained locally under
`target/s08/p5-corpus-{module-alias,module-root}-recheck-01`.

The inspected remaining differences include absent symbols for private `this`,
missing optional/expando properties, inferred property/return types and widening,
and diagnostics affecting the native intrinsic-`any` fast path. The latter changes
the query schedule even where emitted text agrees. The formatter must preserve
that branch rather than fabricate queries to match Go. These are P6 investigation
items; this attribution is not an accepted divergence or an E2 pass claim.
