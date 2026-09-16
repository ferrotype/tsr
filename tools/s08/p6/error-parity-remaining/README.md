# Remaining error-parity cases

This bounded development comparison reuses the 449 IDs in
[`../error-parity-tail/selection.json`](../error-parity-tail/selection.json):
149 originally differing variants plus 300 sampled error-matching controls.
The control is `target/s08/p6-error-parity-tail-01`, captured before these
changes. The candidate is `target/s08/p6-error-parity-remaining-01`.

All 26 remaining error differences (25 acceptance, one informational) now
match. All 449 cases execute, sources are stable, and no selected error,
type/symbol or public-display domain regresses. Two acceptance variants still
differ on types/display; their changed lines were also inspected so a
different-to-different transition does not hide a regression. Their residual
differences and the complete domain transitions are in `results.json`.

Capture using the committed producer:

```sh
python3 scripts/s08_p5_recheck.py \
  --control target/s08/p6-error-parity-tail-01 \
  --selection tools/s08/p6/error-parity-tail/selection.json \
  --output target/s08/p6-error-parity-remaining-01
```

Use a new output directory when repeating a capture. Replay without rebuilding
or rerunning variants:

```sh
python3 scripts/s08_p5_recheck.py \
  --output target/s08/p6-error-parity-remaining-01 --replay
```

The P5 replay authenticates the executable, producer, requests, native
observations and per-case outputs. To include the public-display domain,
grade its authenticated rows through the unchanged E2 comparator:

```python
import sys
from pathlib import Path
sys.path.insert(0, "scripts")
import s08_p5_recheck as recheck
import s08_e2_contract as e2

directory = Path("target/s08/p6-error-parity-remaining-01")
requests, rows, _ = recheck.replay(directory)
metadata = recheck.p5.read(directory / "capture.json")
native = recheck.expected_rows(directory, metadata)
result = e2.grade(requests, rows, native, {"divergence": []},
                  "1f70213d4922b434345f639b441681e470c7cfc1", partial=True)
print(result["counts"])
```

`candidate_sources_sha256` hashes the capture's `build.sources` map serialized
as sorted compact JSON without a trailing newline. Other hashes refer to the
raw captured files or fields copied from `capture.json`.

The checked-in diagnostic regression inputs and native expectations are under
[`data/s08/p6/error-parity-remaining`](../../../../data/s08/p6/error-parity-remaining/README.md).
They cover the 26 witnesses without invoking the full baseline walk. The full
449-case comparison separately covers type, symbol, display and error baselines.

These are bounded results, not refreshed full E2 evidence or gate claims.
