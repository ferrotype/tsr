# P6 dynamic-import evidence

`record.json` identifies the compressed archive and scopes. It contains native
requests, observations, drivers, source snapshots and logs; full Rust P2
observations and comparisons; and the 23-variant corpus recheck with raw case
results, source snapshots and build fingerprints. Executables are not committed.
The captured recheck producer is byte-identical to `scripts/s08_p5_recheck.py`.

The focused regression test checks 59 top-level public type displays and the
complete semantic diagnostic payloads of 11 programs. It passes. The separate
full P2 comparison also observes nested property signatures: 3 programs match,
8 differ, with 50 differing nested display fields and no other native
observation differences. Those failures are retained, including the truncated
`Promise.then` signatures. This is not full display or E2 acceptance.

The corpus selection has 22 acceptance variants and one informational variant.
Exact acceptance matches increased from 15 to 19. All 15 controls stayed matched;
all 23 executions completed; source fingerprints remained stable. Type/symbol
bytes and query schedules match on 21/23 variants, error results on 19/23.
The four newly matching acceptance variants are:

- `compiler/importAttributeDynamicImports.ts#configuration=0`
- `conformance/node/nodeModulesDeclarationEmitDynamicImportWithPackageExports.ts#configuration=0`
- `conformance/node/nodeModulesDeclarationEmitDynamicImportWithPackageExports.ts#configuration=1`
- `conformance/node/nodeModulesDeclarationEmitDynamicImportWithPackageExports.ts#configuration=2`

Remaining differences are `esmModuleExports1` configuration 0 (errors),
`esmModuleExports2` configurations 0 and 1 (errors and types/queries), and
`esmModuleExports3` configuration 0 (errors). `esmModuleExports2` configuration 0
is informational. The full 9,369/1,359 inventory has not been recaptured.

## Reproduce without executing a compiler

Run from the repository root. Use a fresh extraction directory:

```sh
python3 - <<'PY'
import hashlib, json, tarfile
from pathlib import Path
root = Path('data/s08/p6')
record = json.loads((root / 'record.json').read_bytes())
archive = root / record['archive']
assert hashlib.sha256(archive.read_bytes()).hexdigest() == record['archive_sha256']
destination = Path('target/s08/p6-evidence-replay')
destination.mkdir()
with tarfile.open(archive) as source:
    source.extractall(destination, filter='data')
PY
python3 tools/s08/p6/project_dynamic_import_tests.py \
  target/s08/p6-evidence-replay/p6-dynamic-imports-native-02 \
  target/s08/p6-evidence-replay/p6-dynamic-imports-missing-native-02 \
  target/s08/p6-evidence-replay/p6-dynamic-imports-commonjs-native-01 \
  --output data/s08/p6/dynamic-import-tests.json --check
```

Recompute the full P2 comparisons with `scripts/s08_p2.py compare`, using the
archived native directory and the corresponding `*-final-rust.json` as
`--actual`, and a fresh `--output`. The main and CommonJS comparisons correctly
exit nonzero. Only the missing-global comparison fully matches.

The existing case replay validates source snapshots, input identities and raw
artifact hashes without needing the original executable:

```sh
PYTHONPATH=scripts python3 - <<'PY'
from pathlib import Path
import s08_p5_recheck as recheck
p5 = recheck.p5
root = Path('target/s08/p6-evidence-replay/p6-imports-recheck-01')
metadata = p5.read(root / 'capture.json')
recheck.authenticated(root, 'producer.py', metadata['producer_sha256'])
expected = recheck.expected_rows(root, metadata)
_, _, report = p5.p4.replay(
    root, validator=p5.validate_row,
    summarizer=lambda requests, rows: p5.summarize(requests, rows, expected))
recorded = p5.read(root / 'report.json')
assert report == {key: value for key, value in recorded.items() if key != 'source_stable'}
print(report['counts_by_tier'])
PY
```

## Execute new observations

For each specification under `tools/s08/p6/dynamic-imports*.json`, use the
existing `scripts/s08_p2.py capture --spec ... --output <fresh-directory>`, run
`cargo run --locked -p ts_compiler --example p2_checker -- <requests> <output>`,
and compare through `scripts/s08_p2.py compare`. Regenerate the integration-test
projection from authenticated native captures with the helper above.

The corpus recheck can use the extracted capture as its authenticated control:

```sh
python3 scripts/s08_p5_recheck.py \
  --control target/s08/p6-evidence-replay/p6-imports-recheck-01 \
  --selection tools/s08/p5/import-attributes-corpus-selection.json \
  --output target/s08/p6-imports-recheck-new
```

This rebuilds and executes only the selected 23 variants. The native baseline,
selection and comparator remain unchanged. No supplemental result emits E2
metrics or grades nonexecuted JavaScript/declaration output as matching.
