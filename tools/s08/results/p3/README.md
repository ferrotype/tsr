# S08 P3 implementation replay

The archive records the P3 implementation after PR #18: 14 native capability
suites (108 programs, 331 queries), 380 exact frozen relation actions, 8 direct
residual comparator families, 7 union matrices/77 permutations, and two P1
constructor/accounting replays. The native authority remains the pinned Go
checkout. No full-corpus or performance gate is claimed.

`report.json` gives request/result and executable identities. `sources.json`
in the archive lists the exact implementation inputs, including new files that
were not yet committed during capture. All binaries ran in debug mode with
`relation-probe` and `storage-pilot`; these are correctness captures, not timings.
Tracker validation subsequently corrected four Go-attribution comments in two
files. The original files and `post-capture-attribution.patch` are retained; the
report records both hashes. No executable source lines changed.

The archive has raw observations, native request/source snapshots and reports,
Rust logs, comparisons, and validation logs. Its adjacent manifest records each
member's SHA256 and size. Binaries can be rebuilt from this PR and are not stored.

The 40 actions in `flow-return-inference` and `jsdoc-module` remain pending P4.
The full post-action P0 display/ordering/final-state and source/global-checking
schedule is also pending P4/P5 and must precede P7 measurements. The relation
report deliberately says `full_p0_contract_match: false`. No frozen requests,
acceptance denominator or thresholds were changed.

## Replay

Extract `capture.tar.xz` into a temporary directory. Build the examples:

```sh
cargo build -p ts_compiler -p ts_checker \
  --features ts_checker/storage-pilot,ts_compiler/relation-probe \
  --example p2_checker --example p3_relations \
  --example p3_comparators --example storage_families
```

For each `capabilities/<suite>/native/` directory, run `p2_checker` with its
`requests.json` and a fresh output filename. Compare with `scripts/s08_p2.py
compare --native <directory> --actual <output> --output <new-comparison>`.
The native snapshots are self-contained; no new Go run is needed for replay.

Extract the existing `../p0-contracts/native.tar.xz` to obtain its requests and
observations. Run `p3_relations <requests> <actual>` and validate with
`scripts/s08_p3_relations.py --requests <requests> --native <observations>
--policy tools/s08/p3c/relation-policy.json --actual <actual> --output <result>`.
For `p3_comparators`, pass the same requests, `tools/s08/p3c/order-inputs.json`,
and a fresh output filename; validate with `scripts/s08_p3_comparators.py`
using the same inputs plus `--inputs tools/s08/p3c/order-inputs.json`.

The archive's `storage/` directory contains each frozen constructor request and
its native/Rust observations. Feed a request to `storage_families` on stdin.
`scripts/s08_families.validate` compares constructor observations and requires
the exact old Go inventory plus the 21 named unpaired Rust census families.
Those new families are not treated as zero-byte Go families.

`tools/s08/p3c/obligations.json` maps all 675 frozen obligations in 16 families
to executed capability slices and concrete remaining P4/P5 work. It does not
mark corpus obligations complete from a syntax census or a similar fixture.
The broader pending expressions/library-member probes and Go's oversized-tuple
query-before-checking panic remain in `tools/s08/p3c/` for subsequent work.
