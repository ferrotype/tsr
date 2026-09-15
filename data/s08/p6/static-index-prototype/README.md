# Static index constraints and config diagnostic recovery

Pinned `Checker.checkIndexConstraints` skips a property only when checking the
static side and its symbol has `SymbolFlagsPrototype`. Filtering by the text
`prototype` would incorrectly suppress real instance/interface properties.
The seven native programs cover that distinction, real and inherited static
properties, independent instance/static indexes, and incompatible numeric and
string indexes. Compiler tests compare complete diagnostic payloads and repeat
the requests across checker operations.

Regenerate the native observations into a fresh directory:

```sh
PYTHONPATH=scripts python3 - <<'PYTHON'
from s08_oracle import ROOT, run_overlay, strict_json_loads
request = strict_json_loads((ROOT / 'tools/s08/p6/static-index-prototype/requests.json').read_bytes())
run_overlay(
    ROOT / 'target/s08/p6-static-index-prototype-native-new', 'checker',
    (ROOT / 'tools/s08/p6/semantic_diagnostics_oracle_test.go').read_text(),
    request, 'TestS08P6SemanticDiagnostics',
)
PYTHON
cargo test --locked -p ts_compiler --test checker_semantics --test checker_config_diagnostics
```

The combined development selection includes all 128 frozen variants with
`loading.options.configFilePath`, plus 17 static-index/index-declaration cases.
This covers all seven extra-TS2411 witnesses and all six deprecated-option
fixtures, with 93 acceptance and 52 informational variants. Config fixture
provenance and harness details are in `../config-diagnostics/README.md`.

```sh
python3 scripts/s08_p5_recheck.py \
  --control target/s08/e2/corpus \
  --selection tools/s08/p6/static-index-prototype/selection.json \
  --output target/s08/p6-static-index-config-recheck-new
```

To authenticate and score an existing recheck, including public TypeToString:

```sh
PYTHONPATH=scripts python3 - <<'PYTHON'
from pathlib import Path
from s08_p5_recheck import replay, expected_rows
from s08_e2_contract import grade
from s08_oracle import strict_json_loads
p = Path('target/s08/p6-static-index-config-recheck-03')
requests, rows, _ = replay(p)
metadata = strict_json_loads((p / 'capture.json').read_bytes())
pin = strict_json_loads(Path('data/upstream.json').read_bytes())['pin']
result = grade(requests, rows, expected_rows(p, metadata), {'divergence': []}, pin, partial=True)
print(result['counts'])
PYTHON
```

The final result is recorded in
`tools/s08/p6/static-index-prototype/results.json`. All selected comparisons
match (or are natively disabled), with no execution failures or regressions.
Acceptance error matches rise from 72 to 93; types/symbols from 83 to 84 with
nine disabled in both captures; public display stays at 84 plus nine disabled.
Informational error matches rise from zero to 52. Those informational results
remain outside the gate.

The first screen (`-01`) exposed a config-spec omission and was rejected.
Its new regression also caught missing syntax-error aggregation and incorrect
path-matching context. The corrected screen (`-02`) matched all cases. Final
review then changed the root-config entry point to parse the original test
unit directly, preserving BOM bytes as native does, and added a byte-retention
test. Screen `-03` validates that final source revision. All captures remain
available locally; none substitutes for full-corpus E2 evidence.
