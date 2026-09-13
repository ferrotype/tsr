# Pinned syntactic type skeletons

`native.json` contains 850 observations from the pinned Go pseudochecker, using
nine parsed and bound source files with strict null checks both disabled and
enabled. The observer calls the native lookup functions; it does not reproduce
the Rust lookup algorithms. Each row records the entire pseudo-type tree, source
kind/range identities, ordered error nodes, const-context lookup, and the native
conservative undefined test. This is a correctness fixture, not E2 evidence or a
storage measurement.

The cases cover literal widening, nested const arrays, omitted/spread elements,
object methods and accessor pairs, error relocation, single/nested/async/generator
returns, contextual assertions, initialized parameters before required ones,
merged declarations, and reparsed JSDoc signatures. Empty inferred error lists
and `NoResult` are deliberately distinct. The conservative native undefined rule
for intersections remains unchanged.

To capture a new observation without changing the upstream checkout, choose a
fresh target directory and run from the repository root:

```sh
python3 - <<'PY'
import sys
from pathlib import Path
sys.path.insert(0, 'scripts')
from s08_oracle import run_overlay, strict_json_loads
folder = Path('tools/s08/p4/pseudochecker')
run_overlay('target/s08/p4-pseudochecker-native-03', 'pseudochecker',
            (folder / 'oracle_test.go').read_text(),
            strict_json_loads((folder / 'requests.json').read_bytes()),
            'TestS08PseudoChecker')
PY
```

The shared overlay runner verifies the gitlink and local pinned Go toolchain,
retains the exact observer and requests, and records source/request/output hashes
in `provenance.json`. The checked-in observation and provenance came from native
capture `p4-pseudochecker-native-02`. The first capture failed while compiling the
observer; it produced no result. Rust replays the fixture with:

```sh
cargo test -p ts_pseudochecker --test native
```
