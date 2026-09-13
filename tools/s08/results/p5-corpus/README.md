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
Five explicit failures remain in that selection. Additional completed queries
still have exact-byte or diagnostic differences.

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
