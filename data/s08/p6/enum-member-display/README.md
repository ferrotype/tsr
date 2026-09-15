# Enum member display through import type parents

Three direct type-node requests display identifier and quoted enum members
through an import type parent (`b.ts`, where `E` is not in scope) and through a
local reference parent (`a.ts`). They witness pinned Go
`NodeBuilderImpl.appendReferenceToType` for the identifier member and the
`IsTypeOf` import type indexed access for the quoted member. The compiler test
`declarationEmitQualifiedName.ts` covers only the identifier path.

All results match pinned Go. The native capture is
`target/s08/p6-enum-member-display-native-01`. Regenerate with the unchanged
P5 display driver and this request path:

```sh
PYTHONPATH=scripts python3 - <<'PY'
from pathlib import Path
import s08_p5_display as display
display.REQUESTS = Path('tools/s08/p6/enum-member-display-requests.json').resolve()
display.capture(Path('target/s08/p6-enum-member-display-native-new').resolve())
PY
cargo test --locked -p ts_compiler --test checker_display enum_member_display
```

This supplemental comparison does not certify E2 acceptance.
