# F5b reviewed capture archive

These archives preserve the 2026-09-23 review refresh. They contain raw
requests, native and Rust observations, and provenance; no build caches or
executables. Extract from the repository root with `tar -xzf ARCHIVE -C target`.
Existing captures must not be overwritten: extract into a separate scratch
root if a target directory is already present.

| Archive | Bytes | SHA-256 |
| --- | ---: | --- |
| `f5b-reviewed-families.tar.gz` | 5,681,974 | `efc0988bbecab4efd4f2087ee6eb707b372fdb0bbab5ee299bdb19ae686e5afc` |
| `f5b-reviewed-syntax-full.tar.gz` | 18,828,031 | `8a05f0a8f725ff52da4aac78575352b26ac7a1b3e8aed1c326b4c32e0d08b17b` |
| `f5b-reviewed-integration.tar.gz` | 1,051,413 | `89554ca83ee749a518786ce601335fbcef5a447b9cd5b4af733b41dc7e755c31` |
| `f5b-reviewed-integration-program-refresh.tar.gz` | 1,051,639 | `e5cc24fb78fbf01fcd1f37dc3d37f29609af147273d707ab35ae4b047e9e2060` |

Replay after extraction (each family also supports `pilot`):

```sh
python3 scripts/phase1.py compare --capture target/phase1-f5b-review-20260923-03/leaves
python3 scripts/phase1.py compare --capture target/phase1-f5b-review-20260923-03/filesystem
python3 scripts/phase1.py compare --capture target/phase1-f5b-review-20260923-03/config
python3 scripts/phase1.py compare --capture target/phase1-f5b-review-20260923-03/syntax
python3 scripts/phase1_syntax.py replay target/phase1-f5b-review-full-20260923/full
```

Replay rejects source or reviewed-claim drift; an archive is not a freshness
waiver. Family counts are 229/230 leaves, 355/359 filesystem, 498/499 config,
and 1,123/1,123 syntax. Five raw differences have existing scoped approvals;
one filesystem case requires Linux. The six-row historical pilot has two
matches and four explicit missing driver paths and supplies no acceptance
metric. Full program syntax is 15,152/15,152, with the separate 54 native
selection boundaries retained in the committed syntax inventory.

The integration archives retain all seven executed receipts and their stdout,
stderr, commands and source maps, plus the registry used by foundations replay.
The `program-refresh` archive is current after the S07 producer replay fix;
the first archive preserves the preceding receipt set for diagnosis. That
earlier set failed final receipt replay (localized request ordering and a
binder test filter); it is not passing acceptance evidence. Use the registry in
`program-refresh` when replaying current sources. Replay with `cargo xtask run foundations` after restoring the family captures
and their `target/phase1-acceptance/{leaves,filesystem,config,syntax}` links.
These seven receipts cover the required executable witnesses; the three
case-based integration witnesses are evaluated from the family captures.

## Separate host capture after B1-3/B2-3

`f5b-filesystem-darwin-20260923.tar.gz` (1,704,979 bytes) preserves the complete
Darwin capture under `filesystem-darwin/`, its child bytes, comparison and
single-host record. SHA-256:
`68dbc06cee4c1edc7bfb37a23ae31648c9ce358bd470ba0a21a424d4e4147b69`.
Capture identity:
`c4b4496205832745435692a0c7391d8b8a00ebf6a927bd777e764f498bc5afac`.

It records 356 matches, three unchanged approved differences, and one
`not_applicable` Linux-only realpath row. The composed live-filesystem/program
witness matches. The raw native Linux row remains `native_unavailable` with its
reason; only the comparison excludes it from Darwin's 359-row denominator.
This archive does **not** certify Linux. CI preserves a distinct Linux archive,
which must cover every Linux-applicable case, including the composed witness.

After extraction, use `python3 scripts/phase1_producers.py host --capture
filesystem=PATH/filesystem-darwin --output target/phase1-host-records` to verify
this host. Pass the other independently authenticated host directory using
`--host-capture` when recording foundations/config. Replay still rejects changed
sources or case claims. The earlier family/receipt archives above are historical
after this increment; their former current-state descriptions apply to their
checkpoint, not to the host-metadata and live-loader edits.
