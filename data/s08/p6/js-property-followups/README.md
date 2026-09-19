# JS property, expando and display follow-ups

The frozen 209-case JS object/expando selection had 30 variants still different
at `c86138e`. This increment diagnoses those existing captured rows before
changing the implementation. The twelve `typeFromPropertyAssignment*` failures
share canonical-error propagation; the other cases expose separate native
branches in inference, alias display, readonly checks and grammar.

The thirteen small programs in `requests.json` are independent native diagnostic
witnesses with controls for ambient rest parameters, non-strict optional methods,
writable descriptors, TS inference defaults, generic for-in keys and runtime
versus lexical globals. The request explicitly selects strict/JS options, and
all roots run through the same existing P6 diagnostic bridge. Native
`provenance.json` records the pin, toolchain and exact bridge/request/output
hashes. Earlier fixtures have not been refrozen.

Regenerate these observations into a fresh directory:

```sh
PYTHONPATH=scripts python3 - <<'PYTHON'
from s08_oracle import ROOT, run_overlay, strict_json_loads
run_overlay(
    ROOT / 'target/s08/p6-js-followups-native-new', 'checker',
    (ROOT / 'tools/s08/p6/semantic_diagnostics_oracle_test.go').read_text(),
    strict_json_loads((ROOT / 'tools/s08/p6/js-property-followups/requests.json').read_bytes()),
    'TestS08P6SemanticDiagnostics',
)
PYTHON
cargo test --locked -p ts_compiler --test checker_semantics
```

Focused Rust display regressions also preserve unresolved aliases on receivers
while discarding them on public/private property reads; retain JSDoc alias names
and optional methods across operations; and keep non-trailing variadics inside
one tuple rest parameter while still expanding fixed tuple parameters. Their
native witnesses are the recorded corpus outputs, compared through the unchanged
E2 type, symbol and public-display comparators.

Recheck the full selection using the prior 209-case capture as control:

```sh
python3 scripts/s08_p5_recheck.py \
  --control target/s08/p6-js-object-recheck-02 \
  --selection tools/s08/p6/js-property-followups/selection.json \
  --output target/s08/p6-js-followups-recheck-new
```

`remaining-selection.json` retains the thirty original failing variants. The
initial diagnostic screen is `target/s08/p6-js-remaining-recheck-01`; its source
predates the generic-nullability correction detected by the added native
control. Only the final full-selection screen is the result for this increment.
These are development comparisons, not fresh full-corpus E2 evidence.

Final result: all 209 cases match on types/symbols, diagnostics and public
TypeToString, with stable sources and no regressions. Before this increment,
those match counts were 181, 205 and 180. All thirty variants that differed in
at least one domain now match all three. Full-corpus E2 evidence remains stale.

The final capture is `target/s08/p6-js-followups-recheck-01`. Reproduce its
three-domain grading without rebuilding:

```sh
PYTHONPATH=scripts python3 - <<'PYTHON'
import json
from pathlib import Path
from s08_p5_recheck import replay, expected_rows
from s08_e2_contract import grade
p = Path('target/s08/p6-js-followups-recheck-01')
requests, rows, _ = replay(p)
metadata = json.loads((p / 'capture.json').read_text())
native = expected_rows(p, metadata)
pin = json.loads(Path('data/upstream.json').read_text())['pin']
result = grade(requests, rows, native, {'divergence': []}, pin, partial=True)
print(result['counts'])
PYTHON
```

The result record binds the control/candidate captures and selection. Its
candidate source hash is SHA-256 of
`s08_p4.canonical(capture['build']['sources'])`. Replay authenticates the
executable, producer and inputs; the record keeps exact improved IDs and an
empty remaining-difference list. The control is the preceding 209-case capture,
so improvements already made by `c86138e` are not counted again.
