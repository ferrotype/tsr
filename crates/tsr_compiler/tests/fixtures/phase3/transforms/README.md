# Native transform probes

Each `<name>.native.json` here is written by `scripts/phase3_probe.py capture`
from a hand-authored `<name>.requests.json` beside it:

```sh
export PATH="$(mise where go)/bin:$PATH"
python3 scripts/phase3_probe.py capture \
  --requests crates/tsr_compiler/tests/fixtures/phase3/transforms/<name>.requests.json \
  --output   crates/tsr_compiler/tests/fixtures/phase3/transforms/<name>.native.json
python3 scripts/phase3_probe.py names    # the transformer names a chain may use
```

A case is a small compiler-test source and the chains of pinned transformers
to run over each of its files. `tests/phase3_transforms.rs` loads the same
program, runs the same chains through the ported transformers and requires the
same printed bytes. Add a fixture with the transformer it witnesses; every case
of every committed fixture must match. Commit both files.
