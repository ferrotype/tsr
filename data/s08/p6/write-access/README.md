# Write-access classification for property references

Three strict, no-lib programs witness pinned Go `ast.IsWriteAccess` and
`ast.IsWriteOnlyAccess` at the property-access call sites of the checker:

- `markPropertyAsReferenced` skips only write-only accesses. With
  `noUnusedLocals`, `??=`, `||=`, `&&=`, non-null, spread, element-access and
  compound targets reference private members; only `this.plain = 1` reports TS6133.
- `checkPropertyAccessibility` receives `IsWriteAccess`, so a non-null-asserted
  assignment target selects the public getter (no TS2341); plain and
  parenthesized targets select the private setter.
- The property type uses the setter only for write-only accesses, so
  `o.x! = 1` checks against the getter type and reports TS2322.

The driver is `tools/s08/p6/semantic_diagnostics_oracle_test.go`, which records
`Program.GetSyntacticDiagnostics` and `Program.GetSemanticDiagnostics`. The
native capture is `target/s08/p6-write-access-native-01`. Regenerate with:

```sh
PYTHONPATH=scripts python3 - <<'PY'
from s08_oracle import run_overlay, strict_json_loads, ROOT
request = strict_json_loads((ROOT/'tools/s08/p6/write-access-requests.json').read_bytes())
driver = (ROOT/'tools/s08/p6/semantic_diagnostics_oracle_test.go').read_text()
run_overlay(ROOT/'target/s08/p6-write-access-native-new', 'checker', driver, request, 'TestS08P6SemanticDiagnostics')
PY
cargo test --locked -p ts_compiler --test checker_semantics write_access
```

This supplemental comparison does not certify E2 acceptance.
