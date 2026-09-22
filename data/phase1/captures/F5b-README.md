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

The integration archive retains all seven executed receipts and their stdout,
stderr, commands and source maps, plus the registry used by foundations replay.
Replay it with `cargo xtask run foundations` after restoring the family captures
and their `target/phase1-acceptance/{leaves,filesystem,config,syntax}` links.
These seven receipts cover the required executable witnesses; the three
case-based integration witnesses are evaluated from the family captures.
