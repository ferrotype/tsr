# Cross-file display contexts

Sixteen direct type-node requests compare the queried declaration's own file
with a separate enclosing declaration's file, using both declaration and source
contexts. Escaped property names, parameter names, literal annotations and
multiline object types exercise annotation reuse and printer source handling.

All results match pinned Go. The previous adapter also produced these bytes:
these fixtures cover cross-file requests but are not a byte-level witness of the
wrong-file argument. The correction follows the native driver's explicit
`GetSourceFileOfNode(enclosing)` printer argument; the Rust adapter now passes
`context_source` instead of the queried declaration's `source`.

The native capture is `target/s08/p5-display-cross-file-native-01`. Regenerate
with the unchanged driver and supplemental request path:

```sh
PYTHONPATH=scripts python3 - <<'PY'
from pathlib import Path
import s08_p5_display as display
display.REQUESTS = Path('tools/s08/p5/display-cross-file-requests.json').resolve()
display.capture(Path('target/s08/p5-display-cross-file-native-new').resolve())
PY
cargo test --locked -p ts_compiler --example p5_display
```

The existing 208-request display corpus and its branch-witness archive are
unchanged. This supplemental comparison does not certify E2 acceptance.
